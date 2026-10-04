//! Workspace identity and audit storage outside the agent-writable checkout.
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    layout_version: u32,
    workspace_root: PathBuf,
}

/// The checkout never supplies the locator. Configuration comes from the host
/// environment, and the key is a digest of the canonical workspace path.
pub fn directory(root: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let base = match std::env::var_os("HYPER_STATE_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        Some(_) => bail!("HYPER_STATE_DIR must not be empty"),
        None => dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("cannot locate state directory; set HYPER_STATE_DIR")?
            .join("hyper"),
    };
    if !base.is_absolute() {
        bail!("HYPER_STATE_DIR must be absolute");
    }
    let mut normalized = PathBuf::new();
    for component in base.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    // Resolve existing ancestors even before this state directory is created.
    let mut ancestor = normalized;
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .context("invalid state directory")?
                .to_owned(),
        );
        if !ancestor.pop() {
            bail!("invalid state directory");
        }
    }
    let mut base = ancestor.canonicalize()?;
    for component in missing.iter().rev() {
        base.push(component);
    }
    if base.starts_with(&root) {
        bail!("protected audit state directory must be outside workspace");
    }
    let key = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
    Ok(base.join("workspaces").join(key))
}

pub(crate) struct StateGuard {
    pub dir: PathBuf,
    _lock: File,
}

fn lock(root: &Path) -> Result<StateGuard> {
    let dir = directory(root)?;
    let parent = dir.parent().context("invalid audit directory")?;
    reject_link(parent)?;
    private_directory(parent)?;
    if parent.canonicalize()?.starts_with(root) {
        bail!("protected audit state directory must be outside workspace");
    }
    let lock_path = dir.with_extension("lock");
    reject_link(&lock_path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lock_path)?;
    lock.lock()
        .context("could not lock workspace state initialization")?;
    Ok(StateGuard { dir, _lock: lock })
}

/// A shared lease lets ordinary readers/runs coexist while an explicit import
/// refuses to copy a source still opened by this version of Hyper.
pub(crate) fn lease(dir: &Path) -> Result<File> {
    let path = dir.join("migration.lock");
    reject_link(&path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.lock_shared()
        .context("could not lease workspace state")?;
    Ok(file)
}

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(path)?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("protected audit directory must be owned by current user");
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn reject_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => bail!(
            "protected audit path must not be a symlink: {}",
            path.display()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn write_identity(dir: &Path, root: &Path) -> Result<()> {
    fs::write(
        dir.join("workspace.json"),
        serde_json::to_vec_pretty(&Identity {
            layout_version: 1,
            workspace_root: root.to_owned(),
        })?,
    )?;
    Ok(())
}

pub(crate) fn open(root: &Path) -> Result<StateGuard> {
    let guard = lock(root)?;
    reject_link(&guard.dir)?;
    if guard.dir.exists() {
        reject_link(&guard.dir.join("workspace.json"))?;
        let identity: Identity =
            serde_json::from_slice(&fs::read(guard.dir.join("workspace.json"))?)
                .context("invalid protected audit identity")?;
        if identity.layout_version != 1 || identity.workspace_root != root {
            bail!("protected audit identity does not match workspace");
        }
    } else {
        if fs::symlink_metadata(root.join(".harness")).is_ok() {
            bail!(
                "legacy .harness found; stop old runs, then run hyper migrate-state --from .harness; automatic import is disabled"
            );
        }
        let staging = tempfile::Builder::new()
            .prefix(".initialize-")
            .tempdir_in(guard.dir.parent().unwrap())?;
        write_identity(staging.path(), root)?;
        fs::rename(staging.path(), &guard.dir)?;
    }
    private_directory(&guard.dir)?;
    Ok(guard)
}

/// Explicit import never replaces existing state or changes the source. Copy
/// fresh inodes, rather than renaming files that may have workspace hardlinks.
pub fn migrate(root: &Path, source: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let source = source
        .canonicalize()
        .context("invalid state import source")?;
    let guard = lock(&root)?;
    if guard.dir.exists() {
        bail!("protected audit destination already exists; migration never overwrites it");
    }
    for name in [
        "runs",
        "sessions",
        "harness.db",
        "workspace.json",
        "migration.lock",
    ] {
        reject_link(&source.join(name))?;
    }
    if guard.dir.parent().unwrap().starts_with(&source) {
        bail!("state import source must not contain destination registry");
    }
    if !source.is_dir() || !source.join("runs").is_dir() || !source.join("sessions").is_dir() {
        bail!("state import source must contain runs and sessions directories");
    }
    // Old releases do not take our initialization lock. Require them to be
    // stopped; retain every discovered run lock until the import is committed.
    let mut run_locks = Vec::new();
    for entry in fs::read_dir(source.join("runs"))? {
        let entry = entry?;
        reject_link(&entry.path())?;
        if !entry.file_type()?.is_dir() {
            bail!("invalid imported run directory");
        }
        let lock_path = entry.path().join("lock");
        reject_link(&lock_path)?;
        if lock_path.exists() {
            let file = File::open(&lock_path)?;
            file.try_lock()
                .context("state import refused: a source run is active")?;
            run_locks.push(file);
        }
    }
    let source_lease = source.join("migration.lock");
    reject_link(&source_lease)?;
    let _source_lease = if source_lease.exists() {
        let file = File::open(source_lease)?;
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => return Err(anyhow::anyhow!(error)).context(
                    "state import refused: source workspace is open; stop source runs and commands",
                ),
            }
        }
        Some(file)
    } else {
        None
    };
    let staging = tempfile::Builder::new()
        .prefix(".import-")
        .tempdir_in(guard.dir.parent().unwrap())?;
    copy_tree(&source, staging.path(), true)?;
    let database = source.join("harness.db");
    if database.exists() {
        reject_link(&database)?;
        let source_db = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut target_db = Connection::open(staging.path().join("harness.db"))?;
        Backup::new(&source_db, &mut target_db)?.run_to_completion(
            128,
            Duration::from_millis(10),
            None,
        )?;
        let integrity: String = target_db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            bail!("imported audit database failed integrity check");
        }
    }
    validate_records(staging.path())?;
    rebind_checkpoints(&source, staging.path(), &guard.dir, &root)?;
    validate_database(staging.path())?;
    write_identity(staging.path(), &root)?;
    fs::rename(staging.path(), &guard.dir)?;
    Ok(guard.dir)
}

fn copy_tree(source: &Path, destination: &Path, top_level: bool) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let kind = entry.file_type()?;
        if top_level && name == "tmp" {
            continue;
        }
        if kind.is_symlink() {
            bail!("state import refuses symlink: {}", entry.path().display());
        }
        if top_level
            && matches!(
                name.to_str(),
                Some(
                    "harness.db"
                        | "harness.db-wal"
                        | "harness.db-shm"
                        | "workspace.json"
                        | "migration.lock"
                        | "tmp"
                )
            )
        {
            continue;
        }
        let target = destination.join(&name);
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_tree(&entry.path(), &target, false)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if target.extension().is_none_or(|e| e != "snapshot") {
                    let mode = fs::metadata(&target)?.permissions().mode();
                    fs::set_permissions(&target, fs::Permissions::from_mode(mode | 0o600))?;
                }
            }
        } else {
            bail!("state import refuses non-regular file");
        }
    }
    Ok(())
}

fn rebind_checkpoints(
    source: &Path,
    staging: &Path,
    destination: &Path,
    root: &Path,
) -> Result<()> {
    let old_root = match fs::read(source.join("workspace.json")) {
        Ok(bytes) => Some(serde_json::from_slice::<Identity>(&bytes)?.workspace_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    for entry in fs::read_dir(staging.join("runs"))? {
        let checkpoints = entry?.path().join("checkpoints");
        if !checkpoints.exists() {
            continue;
        }
        for entry in fs::read_dir(checkpoints)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let mut cp: crate::Checkpoint = serde_json::from_slice(&fs::read(&path)?)?;
            let stem = path.file_stem().context("invalid checkpoint name")?;
            if stem != std::ffi::OsStr::new(&cp.id) {
                bail!("checkpoint id does not match file name");
            }
            let snapshot = path.with_extension("snapshot");
            if !snapshot.is_file() {
                bail!("checkpoint snapshot is missing");
            }
            let target = Path::new(&cp.target_path);
            let target = if target.is_absolute() {
                let legacy_root = cp
                    .snapshot_path
                    .ancestors()
                    .nth(4)
                    .filter(|p| p.file_name().is_some_and(|n| n == ".harness"))
                    .and_then(Path::parent);
                target
                    .strip_prefix(
                        old_root
                            .as_deref()
                            .or(legacy_root)
                            .context("cannot determine original checkpoint workspace")?,
                    )
                    .context("checkpoint target escapes original workspace")?
                    .to_owned()
            } else {
                target.to_owned()
            };
            crate::workspace::resolve_tool_path(
                root,
                target.to_str().context("non-UTF-8 checkpoint target")?,
            )?;
            cp.target_path = target
                .to_str()
                .context("non-UTF-8 checkpoint target")?
                .into();
            cp.snapshot_path = destination.join(snapshot.strip_prefix(staging)?);
            let mut updated = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
            use std::io::Write as _;
            updated.write_all(&serde_json::to_vec_pretty(&cp)?)?;
            updated.persist(path)?;
        }
    }
    Ok(())
}

/// Detect aliases prepared before the shell starts. Landlock cannot revoke a
/// writable workspace hardlink to an otherwise external audit inode.
pub(crate) fn validate_shell_boundary(root: &Path, audit: &Path) -> Result<()> {
    if audit.canonicalize()?.starts_with(root) {
        bail!("protected audit storage must be outside workspace");
    }
    let mut pending = vec![audit.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                bail!("protected audit storage contains a symlink");
            }
            #[cfg(windows)]
            if metadata.is_file() {
                crate::windows_file::require_single_link(&fs::File::open(entry.path())?)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.is_file() && metadata.nlink() != 1 {
                    bail!("protected audit storage has a hardlink alias");
                }
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

fn validate_database(staging: &Path) -> Result<()> {
    let db = crate::workspace::initialize_database(&staging.join("harness.db"))?;
    let executable_schema: i64 = db.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type IN ('trigger','view') OR (type='table' AND sql IS NOT NULL AND UPPER(sql) NOT LIKE 'CREATE TABLE%')",
        [], |row| row.get(0),
    )?;
    if executable_schema != 0 {
        bail!("imported audit database contains unsupported executable schema objects");
    }
    for (table, expected) in [
        (
            "runs",
            vec![
                "run_id",
                "task_id",
                "task_name",
                "status",
                "started_at",
                "finished_at",
            ],
        ),
        (
            "events",
            vec![
                "event_id",
                "run_id",
                "task_id",
                "type",
                "timestamp",
                "step_id",
                "step_index",
                "payload_json",
            ],
        ),
        (
            "sessions",
            vec![
                "session_id",
                "title",
                "created_at",
                "updated_at",
                "messages",
                "runs",
            ],
        ),
    ] {
        let mut query = db.prepare(&format!("PRAGMA table_info({table})"))?;
        let fields = query
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if fields != expected {
            bail!("unsupported imported audit database schema: {table}");
        }
    }
    for (table, field) in [
        ("runs", "run_id"),
        ("events", "run_id"),
        ("sessions", "session_id"),
    ] {
        let mut query = db.prepare(&format!("SELECT DISTINCT {field} FROM {table}"))?;
        for id in query.query_map([], |r| r.get::<_, String>(0))? {
            crate::workspace::validate_session_id(&id?)
                .context("invalid imported database path identifier")?;
        }
    }
    // The registry is derived from transcripts. Rebuild missing entries and
    // repair counts when a legacy crash left the DB behind a completed append.
    for entry in fs::read_dir(staging.join("sessions"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid session id")?;
        let messages: Vec<crate::SessionMessage> = fs::read_to_string(&path)?
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<serde_json::Result<_>>()?;
        let Some(first) = messages.first() else {
            continue;
        };
        let title: String = first
            .content
            .lines()
            .find(|s| !s.trim().is_empty())
            .unwrap_or("Untitled session")
            .trim()
            .chars()
            .take(60)
            .collect();
        let runs = messages
            .iter()
            .filter(|m| m.role == "user" && m.run_id.is_some())
            .count();
        db.execute("INSERT INTO sessions(session_id,title,created_at,updated_at,messages,runs) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id) DO UPDATE SET updated_at=excluded.updated_at,messages=excluded.messages,runs=excluded.runs",
            rusqlite::params![id,title,first.timestamp,messages.last().unwrap().timestamp,messages.len(),runs])?;
    }
    Ok(())
}

fn validate_records(staging: &Path) -> Result<()> {
    for entry in fs::read_dir(staging.join("runs"))? {
        let dir = entry?.path();
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid imported run id")?;
        crate::workspace::validate_session_id(id)?;
        let task = dir.join("task.json");
        if task.exists() {
            serde_json::from_slice::<crate::TaskSpec>(&fs::read(task)?)?.validate()?;
        }
        let summary = dir.join("summary.json");
        if summary.exists()
            && serde_json::from_slice::<crate::RunSummary>(&fs::read(summary)?)?.run_id != id
        {
            bail!("imported summary run id does not match directory");
        }
        let events = dir.join("events.jsonl");
        if events.exists() {
            let content = fs::read(&events)?;
            let lines: Vec<_> = content.split(|b| *b == b'\n').collect();
            for (index, line) in lines.iter().enumerate() {
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                match serde_json::from_slice::<crate::HarnessEvent>(line) {
                    Ok(event) if event.run_id == id => {}
                    Ok(_) => bail!("imported event run id does not match directory"),
                    Err(_) if index + 1 == lines.len() && !content.ends_with(b"\n") => {}
                    Err(error) => {
                        return Err(error).context("invalid completed imported event record");
                    }
                }
            }
            if !content.is_empty() && !content.ends_with(b"\n") {
                let tail_start = content
                    .iter()
                    .rposition(|b| *b == b'\n')
                    .map_or(0, |i| i + 1);
                let tail = &content[tail_start..];
                if serde_json::from_slice::<crate::HarnessEvent>(tail).is_ok() {
                    let mut normalized = content;
                    normalized.push(b'\n');
                    fs::write(&events, normalized)?;
                } else {
                    // Preserve a crashed partial append as forensic output,
                    // so recovery can append a separate valid terminal event.
                    let artifacts = dir.join("artifacts");
                    fs::create_dir_all(&artifacts)?;
                    let mut fragment = tempfile::Builder::new()
                        .prefix("import-partial-event-")
                        .suffix(".log")
                        .tempfile_in(artifacts)?;
                    use std::io::Write as _;
                    fragment.write_all(tail)?;
                    fragment.keep()?;
                    fs::write(&events, &content[..tail_start])?;
                }
            }
        }
    }
    for entry in fs::read_dir(staging.join("sessions"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        crate::workspace::validate_session_id(
            path.file_stem()
                .and_then(|s| s.to_str())
                .context("invalid imported session id")?,
        )?;
        for line in fs::read_to_string(path)?
            .lines()
            .filter(|s| !s.trim().is_empty())
        {
            serde_json::from_str::<crate::SessionMessage>(line)
                .context("invalid imported session record")?;
        }
    }
    Ok(())
}
