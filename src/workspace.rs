use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rand::{Rng, distr::Alphanumeric};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{
    Failure, HarnessEvent, RunRow, RunSummary, SessionMessage, SessionRow, TaskSpec,
};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

pub fn id() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(21)
        .map(char::from)
        .collect()
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Clone, Debug)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub db: PathBuf,
    pub runs: PathBuf,
    pub sessions: PathBuf,
}
#[derive(Clone, Debug)]
pub struct RunPaths {
    pub dir: PathBuf,
    pub events: PathBuf,
    pub task: PathBuf,
    pub summary: PathBuf,
    pub artifacts: PathBuf,
    pub checkpoints: PathBuf,
}

pub struct Workspace {
    pub paths: WorkspacePaths,
    pub db: Connection,
    _state_lease: File,
}

/// Advisory lock held for the lifetime of a run.
///
/// The lock lives in `runs/<run-id>/lock` and is released by the OS when the
/// owning process exits, including on `SIGKILL`. Another process can therefore
/// tell a live run from a run whose harness died: the lock stays held for the
/// former and becomes acquirable again for the latter.
pub struct RunLock {
    _file: File,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("invalid workspace root: {}", root.as_ref().display()))?;
        let state = crate::state::open(&root)?;
        let dir = state.dir.clone();
        let state_lease = crate::state::lease(&dir)?;
        let paths = WorkspacePaths {
            root,
            db: dir.join("harness.db"),
            runs: dir.join("runs"),
            sessions: dir.join("sessions"),
            dir,
        };
        fs::create_dir_all(&paths.runs)?;
        fs::create_dir_all(&paths.sessions)?;
        let db = initialize_database(&paths.db)?;
        let workspace = Self {
            paths,
            db,
            _state_lease: state_lease,
        };
        // The JSONL files are the fact source and the index is derived from
        // them, so anything the index lost is restored first. Only then can a
        // row still saying `running` be judged: the events that would have
        // closed it may have been what was missing.
        workspace.reconcile_events()?;
        // The previous harness may have been killed mid-run; repair those rows
        // so a crashed run does not linger as `running` forever.
        workspace.reconcile_stale_runs()?;
        Ok(workspace)
    }

    /// Rebuild the SQLite index from the `events.jsonl` audit log.
    ///
    /// Every event is written to the log before it is indexed, so a database
    /// that lost rows (a partial write, a deleted `harness.db`, a copy of a
    /// workspace taken mid-run) can always be brought back in line with the
    /// files. Returns the run ids that had to be repaired.
    ///
    /// The index is only re-read when it cannot already agree with the log:
    /// the line count of the file is compared with the indexed event count,
    /// so the usual startup touches every run once without parsing it.
    pub fn reconcile_events(&self) -> Result<Vec<String>> {
        let Ok(entries) = fs::read_dir(&self.paths.runs) else {
            return Ok(Vec::new());
        };
        let mut repaired = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let Some(run_id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let events_path = path.join("events.jsonl");
            if !events_path.exists() {
                continue;
            }
            let indexed = self.count_run_events(run_id)?;
            if self.get_run(run_id)?.is_some() && count_lines(&events_path)? <= indexed {
                continue;
            }
            let events = read_event_log(&events_path)?;
            // The row is created before its events so a terminal event finds
            // something to close; a row that already exists is left alone
            // rather than overwritten with a guess.
            if self.get_run(run_id)?.is_none() {
                self.recreate_run_row(run_id, &path, &events)?;
            }
            for event in &events {
                self.insert_event(event)?;
            }
            repaired.push(run_id.to_owned());
        }
        Ok(repaired)
    }

    /// Recreate the `runs` row for a run whose events survive but whose row
    /// does not, from `task.json` and the log itself.
    fn recreate_run_row(&self, run_id: &str, dir: &Path, events: &[HarnessEvent]) -> Result<()> {
        // `task.json` is what the run was announced from. When it is gone the
        // log still names the task: every event carries its id and
        // `run.started` carries the name.
        let mut task_id = None;
        let mut task_name = None;
        if let Ok(task) = fs::read_to_string(dir.join("task.json"))
            && let Ok(task) = serde_json::from_str::<TaskSpec>(&task)
        {
            task_id = Some(task.task_id().to_owned());
            task_name = Some(task.name);
        }
        for event in events {
            if event.event_type == "run.started" {
                task_id.get_or_insert_with(|| event.task_id.clone());
                if let Some(name) = event.payload.get("taskName").and_then(Value::as_str) {
                    task_name.get_or_insert_with(|| name.to_owned());
                }
            }
        }
        let started_at = events.first().map(|event| event.timestamp.clone());
        self.db.execute(
            "INSERT OR IGNORE INTO runs VALUES (?1,?2,?3,'running',?4,NULL)",
            params![
                run_id,
                task_id.unwrap_or_else(|| run_id.to_owned()),
                task_name.unwrap_or_else(|| run_id.to_owned()),
                started_at.unwrap_or_else(now)
            ],
        )?;
        Ok(())
    }

    /// Take the run lock. Must be acquired *before* the run is announced to the
    /// database, otherwise a concurrent process could observe a `running` row
    /// whose lock is not held yet and wrongly declare it dead.
    pub fn lock_run(&self, run_id: &str) -> Result<RunLock> {
        let dir = self.paths.runs.join(run_id);
        fs::create_dir_all(&dir)?;
        let path = dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("failed to open run lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(RunLock { _file: file }),
            // A single message covers both "held by someone else" and any other
            // locking error; the run must not start if we cannot own the lock.
            Err(error) => bail!("could not lock run {run_id}: {error}"),
        }
    }

    /// Mark every run whose owning process is gone as `interrupted`.
    ///
    /// Returns the repaired run ids. Runs that are still executing (their lock
    /// is held) are left untouched.
    pub fn reconcile_stale_runs(&self) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare(
            "SELECT run_id,task_id,task_name,started_at FROM runs WHERE status='running'",
        )?;
        let stale = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut repaired = Vec::new();
        for (run_id, task_id, task_name, started_at) in stale {
            if !self.run_is_stale(&run_id)? {
                continue;
            }
            self.mark_run_interrupted(&run_id, &task_id, &task_name, &started_at)?;
            repaired.push(run_id);
        }
        Ok(repaired)
    }

    /// A run is stale when its lock can be acquired, i.e. nobody owns it.
    /// Errors are treated as "still alive" so a live run is never repaired by
    /// accident.
    fn run_is_stale(&self, run_id: &str) -> Result<bool> {
        let dir = self.paths.runs.join(run_id);
        if !dir.is_dir() {
            return Ok(true);
        }
        // The probe must not create the lock file: `prune --dry-run` asks the
        // same question, and a dry run touches nothing.
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("lock"))
        {
            Ok(file) => file,
            // No lock file means no run ever took one, so nobody owns it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error).context("failed to read the run lock"),
        };
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    fn mark_run_interrupted(
        &self,
        run_id: &str,
        task_id: &str,
        task_name: &str,
        started_at: &str,
    ) -> Result<()> {
        let total = self.count_events(run_id, "step.started")?;
        let succeeded = self.count_events(run_id, "step.finished")?;
        let step_id = self.last_started_step(run_id)?;
        let summary = RunSummary {
            run_id: run_id.to_owned(),
            task_name: task_name.to_owned(),
            status: "interrupted".into(),
            steps_total: total,
            steps_succeeded: succeeded,
            // No step failed on its own: the harness was killed while one was
            // in flight, which the `interrupted` status records instead.
            steps_failed: 0,
            started_at: started_at.to_owned(),
            finished_at: now(),
            failure: Some(Failure {
                error_type: "Interrupted".into(),
                message: "run was interrupted: the harness exited before the task finished".into(),
                retryable: true,
                step_id,
                details: HashMap::new(),
                cause: None,
            }),
        };
        let event = HarnessEvent {
            event_id: id(),
            run_id: run_id.to_owned(),
            task_id: task_id.to_owned(),
            event_type: "run.interrupted".into(),
            timestamp: summary.finished_at.clone(),
            step_id: None,
            step_index: None,
            payload: json!({"summary": summary}),
        };
        let events_path = self.paths.runs.join(run_id).join("events.jsonl");
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)?
            .write_all(&line)?;
        self.insert_event(&event)?;
        fs::write(
            self.paths.runs.join(run_id).join("summary.json"),
            serde_json::to_vec_pretty(&summary)?,
        )?;
        Ok(())
    }

    fn count_events(&self, run_id: &str, event_type: &str) -> Result<usize> {
        let count: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM events WHERE run_id=?1 AND type=?2",
            params![run_id, event_type],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// How many of a run's events are indexed, whatever their type.
    fn count_run_events(&self, run_id: &str) -> Result<usize> {
        let count: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM events WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    fn last_started_step(&self, run_id: &str) -> Result<Option<String>> {
        let mut stmt = self.db.prepare(
            "SELECT step_id FROM events WHERE run_id=?1 AND type='step.started' ORDER BY timestamp DESC, rowid DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([run_id])?;
        Ok(match rows.next()? {
            Some(row) => row.get(0)?,
            None => None,
        })
    }
    pub fn prepare_run(&self, run_id: &str) -> Result<RunPaths> {
        let dir = self.paths.runs.join(run_id);
        let paths = RunPaths {
            events: dir.join("events.jsonl"),
            task: dir.join("task.json"),
            summary: dir.join("summary.json"),
            artifacts: dir.join("artifacts"),
            checkpoints: dir.join("checkpoints"),
            dir,
        };
        fs::create_dir_all(&paths.artifacts)?;
        fs::create_dir_all(&paths.checkpoints)?;
        Ok(paths)
    }
    pub fn create_run(&self, id: &str, task: &TaskSpec, started: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO runs VALUES (?1,?2,?3,'running',?4,NULL)",
            params![id, task.task_id(), task.name, started],
        )?;
        Ok(())
    }
    pub fn insert_event(&self, event: &HarnessEvent) -> Result<()> {
        // `OR IGNORE` rather than `OR REPLACE`: an event id is never reused,
        // so a conflict only means the row is already there, and re-inserting
        // it would give it a fresh rowid — which would reorder a run whose
        // events share a timestamp, since they are read back by `rowid`.
        self.db.execute(
            "INSERT OR IGNORE INTO events VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                event.event_id,
                event.run_id,
                event.task_id,
                event.event_type,
                event.timestamp,
                event.step_id,
                event.step_index,
                event.payload.to_string()
            ],
        )?;
        let status = match event.event_type.as_str() {
            "run.finished" => Some("finished"),
            "run.failed" => Some("failed"),
            "run.cancelled" => Some("cancelled"),
            "run.interrupted" => Some("interrupted"),
            _ => None,
        };
        if let Some(status) = status {
            self.db.execute(
                "UPDATE runs SET status=?1,finished_at=?2 WHERE run_id=?3",
                params![status, event.timestamp, event.run_id],
            )?;
        }
        Ok(())
    }
    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunRow>> {
        let mut stmt = self.db.prepare("SELECT run_id,task_id,task_name,status,started_at,finished_at FROM runs ORDER BY started_at DESC LIMIT ?1")?;
        Ok(stmt
            .query_map([limit], |r| {
                Ok(RunRow {
                    run_id: r.get(0)?,
                    task_id: r.get(1)?,
                    task_name: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    finished_at: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>> {
        let mut stmt = self.db.prepare("SELECT run_id,task_id,task_name,status,started_at,finished_at FROM runs WHERE run_id=?1")?;
        let mut rows = stmt.query([run_id])?;
        Ok(match rows.next()? {
            None => None,
            Some(r) => Some(RunRow {
                run_id: r.get(0)?,
                task_id: r.get(1)?,
                task_name: r.get(2)?,
                status: r.get(3)?,
                started_at: r.get(4)?,
                finished_at: r.get(5)?,
            }),
        })
    }
    pub fn events(&self, run_id: &str) -> Result<Vec<HarnessEvent>> {
        let mut stmt=self.db.prepare("SELECT event_id,run_id,task_id,type,timestamp,step_id,step_index,payload_json FROM events WHERE run_id=?1 ORDER BY timestamp,rowid")?;
        Ok(stmt
            .query_map([run_id], |r| {
                let payload: String = r.get(7)?;
                Ok(HarnessEvent {
                    event_id: r.get(0)?,
                    run_id: r.get(1)?,
                    task_id: r.get(2)?,
                    event_type: r.get(3)?,
                    timestamp: r.get(4)?,
                    step_id: r.get(5)?,
                    step_index: r.get(6)?,
                    payload: serde_json::from_str(&payload).unwrap_or_default(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn list_checkpoints(&self, run_id: &str) -> Result<Vec<Checkpoint>> {
        let dir = self.paths.runs.join(run_id).join("checkpoints");
        let mut checkpoints = Vec::new();
        if !dir.exists() {
            return Ok(checkpoints);
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(checkpoint) = serde_json::from_slice::<Checkpoint>(&fs::read(&path)?)
            {
                checkpoints.push(checkpoint);
            }
        }
        // File names are random ids; order by creation time instead.
        checkpoints.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(checkpoints)
    }

    /// The transcript file a session id names. Sessions used to be written as
    /// one throwaway file per run with an unguessable name; the id is now the
    /// only handle a conversation needs.
    ///
    /// The id is part of a file name, so it is validated first: an id like
    /// `../../config` would otherwise let a caller read or write outside the
    /// workspace.
    pub fn session_path(&self, session_id: &str) -> PathBuf {
        self.paths.sessions.join(format!("{session_id}.jsonl"))
    }

    /// Append one turn to a session, creating the session registry row on the
    /// first turn. Returns the session's updated summary.
    pub fn append_session_message(
        &self,
        session_id: &str,
        message: &SessionMessage,
    ) -> Result<SessionRow> {
        validate_session_id(session_id)?;
        let path = self.session_path(session_id);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        serde_json::to_writer(&mut file, message)?;
        writeln!(file)?;
        // The turn count comes from the registry rather than from re-reading the
        // transcript, so appending stays O(1) as a conversation grows.
        let messages = self.session(session_id)?.map_or(0, |row| row.messages) as i64 + 1;
        // A run contributes two turns (prompt and answer) but is one run, so
        // only its opening turn counts.
        let run = i64::from(message.role == "user" && message.run_id.is_some());
        self.db.execute(
            "INSERT INTO sessions (session_id,title,created_at,updated_at,messages,runs) \
             VALUES (?1,?2,?3,?3,?4,?5) \
             ON CONFLICT(session_id) DO UPDATE SET \
               updated_at=excluded.updated_at, messages=excluded.messages, runs=sessions.runs+?5",
            params![
                session_id,
                title_from(message),
                message.timestamp,
                messages,
                run,
            ],
        )?;
        self.session(session_id)?
            .with_context(|| format!("session {session_id} disappeared after being written"))
    }

    /// Read a session transcript. A missing file is an empty conversation, not
    /// an error: `hy session <id>` on an unknown id simply has nothing to show.
    pub fn session_messages(&self, session_id: &str) -> Result<Vec<SessionMessage>> {
        validate_session_id(session_id)?;
        let path = self.session_path(session_id);
        let Ok(content) = fs::read_to_string(&path) else {
            return Ok(Vec::new());
        };
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<SessionMessage>(line)
                    .with_context(|| format!("invalid session line in {}", path.display()))
            })
            .collect()
    }

    pub fn session(&self, session_id: &str) -> Result<Option<SessionRow>> {
        let mut stmt = self.db.prepare(
            "SELECT session_id,title,created_at,updated_at,messages,runs FROM sessions WHERE session_id=?1",
        )?;
        let mut rows = stmt.query([session_id])?;
        Ok(match rows.next()? {
            None => None,
            Some(row) => Some(SessionRow {
                session_id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                messages: row.get::<_, i64>(4)?.max(0) as usize,
                runs: row.get::<_, i64>(5)?.max(0) as usize,
            }),
        })
    }

    /// Recent conversations, newest first.
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionRow>> {
        let mut stmt = self.db.prepare(
            "SELECT session_id,title,created_at,updated_at,messages,runs FROM sessions \
             ORDER BY updated_at DESC LIMIT ?1",
        )?;
        Ok(stmt
            .query_map([limit], |row| {
                Ok(SessionRow {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    messages: row.get::<_, i64>(4)?.max(0) as usize,
                    runs: row.get::<_, i64>(5)?.max(0) as usize,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Forget a conversation: the transcript file and its registry row. The
    /// runs it produced are left alone, since deleting them would rewrite the
    /// meaning of the events that already happened.
    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        validate_session_id(session_id)?;
        let path = self.session_path(session_id);
        let existed = path.exists() || self.session(session_id)?.is_some();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.db
            .execute("DELETE FROM sessions WHERE session_id=?1", [session_id])?;
        Ok(existed)
    }

    /// The conversations to drop when only the `keep` most recent ones are
    /// wanted, newest first. `dry_run` reports them without deleting.
    pub fn prune_sessions(&self, keep: usize, dry_run: bool) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT session_id FROM sessions ORDER BY updated_at DESC, session_id DESC")?;
        let all = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut pruned = Vec::new();
        for session_id in all.into_iter().skip(keep) {
            if dry_run {
                pruned.push(session_id);
                continue;
            }
            self.delete_session(&session_id)?;
            pruned.push(session_id);
        }
        Ok(pruned)
    }

    /// The runs to drop when only the `keep` most recent ones are wanted.
    ///
    /// A run that is still executing is never a candidate: its lock is held,
    /// so deleting it would pull the log out from under a live writer. Runs
    /// that are kept are untouched, `dry_run` only reports.
    pub fn prune_runs(&self, keep: usize, dry_run: bool) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT run_id FROM runs ORDER BY started_at DESC, run_id DESC")?;
        let all = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let mut pruned = Vec::new();
        for run_id in all.into_iter().skip(keep) {
            if !self.run_is_stale(&run_id)? {
                continue;
            }
            pruned.push(run_id.clone());
            if dry_run {
                continue;
            }
            fs::remove_dir_all(self.paths.runs.join(&run_id))
                .with_context(|| format!("failed to delete run directory for {run_id}"))?;
            self.db
                .execute("DELETE FROM events WHERE run_id=?1", [&run_id])?;
            self.db
                .execute("DELETE FROM runs WHERE run_id=?1", [&run_id])?;
        }
        Ok(pruned)
    }
}

/// Newline-terminated records in an audit log file.
///
/// The last line is only counted once it is terminated, so a harness killed
/// mid-write leaves a count that matches what can actually be parsed.
fn count_lines(path: &Path) -> Result<usize> {
    let content = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(content.iter().filter(|byte| **byte == b'\n').count())
}

/// The events recorded for a run, in log order.
///
/// A line that does not parse is skipped instead of failing the recovery: a
/// truncated line is still being written by someone, and every other event is
/// worth restoring.
fn read_event_log(path: &Path) -> Result<Vec<HarnessEvent>> {
    let content = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(content
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|line| serde_json::from_slice::<HarnessEvent>(line).ok())
        .collect())
}

/// A conversation id is used as a file name, so only characters that cannot
/// escape the `sessions/` directory are accepted.
pub fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() {
        bail!("session id must not be empty")
    }
    if session_id.len() > 64 {
        bail!("session id must be at most 64 characters")
    }
    if session_id == "." || session_id == ".." {
        bail!("session id must not be a directory reference")
    }
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("session id may only contain letters, digits, '-' and '_': {session_id}")
    }
    Ok(())
}

/// A conversation is titled by its first prompt, trimmed to something a list
/// view can show. Later turns never rename it.
fn title_from(message: &SessionMessage) -> String {
    let first_line = message
        .content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let title: String = first_line.chars().take(60).collect();
    if title.is_empty() {
        "(untitled)".into()
    } else {
        title
    }
}

pub fn resolve_path(root: &Path, target: &str) -> Result<PathBuf> {
    let joined = root.join(target);
    let mut normalized = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            p => normalized.push(p.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        bail!("path escapes workspace root: {target}")
    }
    // Follow symlinks in the existing portion of the path so a link inside the
    // workspace cannot smuggle reads or writes outside of it. Fail closed when
    // the prefix cannot be resolved (e.g. a dangling symlink).
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut existing = normalized.clone();
    let mut tail = Vec::new();
    while fs::symlink_metadata(&existing).is_err() {
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                existing.pop();
            }
            None => return Ok(normalized),
        }
    }
    let canonical = fs::canonicalize(&existing)
        .with_context(|| format!("cannot resolve symlink path: {target}"))?;
    if !canonical.starts_with(&root) {
        bail!("path escapes workspace root: {target}")
    }
    let mut resolved = canonical;
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Agent file tools cannot access the harness audit/control area, including
/// symlink aliases. Administrative checkpoint/recovery APIs use resolve_path.
pub fn resolve_tool_path(root: &Path, target: &str) -> Result<PathBuf> {
    let resolved = resolve_path(root, target)?;
    let audit = root.canonicalize()?.join(".harness");
    if resolved.starts_with(&audit)
        || audit
            .canonicalize()
            .is_ok_and(|canonical| resolved.starts_with(canonical))
    {
        bail!("protected audit path is unavailable to agent file tools: {target}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = fs::metadata(&resolved)
            && metadata.is_file()
            && metadata.nlink() > 1
        {
            // A workspace hardlink to an audit file is the same inode even
            // though canonicalize leaves its spelling outside .harness.
            let mut pending = vec![crate::state::directory(root)?];
            if audit.is_dir() {
                pending.push(audit);
            }
            while let Some(directory) = pending.pop() {
                for entry in fs::read_dir(directory)? {
                    let entry = entry?;
                    let file_type = entry.file_type()?;
                    if file_type.is_dir() {
                        pending.push(entry.path());
                    } else if file_type.is_file() {
                        let protected = entry.metadata()?;
                        if protected.dev() == metadata.dev() && protected.ino() == metadata.ino() {
                            bail!(
                                "protected audit hardlink is unavailable to agent file tools: {target}"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(resolved)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub target_path: String,
    pub snapshot_path: PathBuf,
    pub existed: bool,
    pub created_at: String,
}
pub fn create_checkpoint(root: &Path, dir: &Path, target: &str) -> Result<Checkpoint> {
    let absolute = resolve_path(root, target)?;
    fs::create_dir_all(dir)?;
    let id = id();
    let snapshot = dir.join(format!("{id}.snapshot"));
    let existed = absolute.exists();
    if existed {
        fs::copy(&absolute, &snapshot)?;
    } else {
        fs::write(&snapshot, "")?;
    }
    let cp = Checkpoint {
        id: id.clone(),
        target_path: absolute
            .strip_prefix(root)?
            .to_str()
            .context("non-UTF-8 checkpoint target")?
            .into(),
        snapshot_path: snapshot,
        existed,
        created_at: now(),
    };
    fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&cp)?,
    )?;
    Ok(cp)
}
pub fn restore_checkpoint(root: &Path, cp: &Checkpoint) -> Result<()> {
    let target = resolve_path(root, &cp.target_path)?;
    if !cp.existed {
        if target.exists() {
            fs::remove_file(target)?;
        }
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&cp.snapshot_path, target)?;
    Ok(())
}

pub(crate) fn initialize_database(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    // WAL lets readers continue while another process appends events, and
    // the busy timeout absorbs short write contention between a TUI and a
    // concurrently running command. These are connection-local settings
    // except for journal_mode, which is persisted in the database.
    db.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS runs (run_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, task_name TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT); CREATE TABLE IF NOT EXISTS events (event_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, task_id TEXT NOT NULL, type TEXT NOT NULL, timestamp TEXT NOT NULL, step_id TEXT, step_index INTEGER, payload_json TEXT NOT NULL); CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id,timestamp); CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at); CREATE TABLE IF NOT EXISTS sessions (session_id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, messages INTEGER NOT NULL, runs INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);")?;
    Ok(db)
}
