//! Administrative restore. Linux commits through a pinned parent directory.
use crate::workspace::Checkpoint;
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail};
// The legacy path copy is only used off Linux/macOS; its Linux tests reach `fs`
// through `use super::*`. macOS uses the descriptor-anchored POSIX walker.
#[cfg(any(
    all(test, target_os = "linux"),
    not(any(target_os = "linux", target_os = "macos"))
))]
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::File;
use std::path::Path;

pub(crate) fn restore(root: &Path, cp: &Checkpoint) -> Result<()> {
    let root = root.canonicalize()?;
    let target = crate::workspace::resolve_path(&root, &cp.target_path)?;
    let legacy = root.join(".harness");
    if target.starts_with(&legacy)
        || legacy
            .canonicalize()
            .is_ok_and(|path| target.starts_with(path))
    {
        bail!("protected audit path cannot be a restore target");
    }
    #[cfg(target_os = "linux")]
    {
        linux::restore_resolved(&root, &target, cp)
    }
    #[cfg(target_os = "macos")]
    {
        crate::posix_admin::restore(&root, &target, cp)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
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
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::tool_file::linux::{open_at, open_at_resolve, parent};
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };

    fn name(path: &Path) -> Result<CString> {
        Ok(CString::new(path.as_os_str().as_bytes())?)
    }

    pub(super) fn restore_resolved(root: &Path, target: &Path, cp: &Checkpoint) -> Result<()> {
        // Read-only source is pinned before any workspace directories are made.
        let mut snapshot = if cp.existed {
            let parent = cp
                .snapshot_path
                .parent()
                .context("snapshot has no parent")?
                .canonicalize()?;
            let filename = cp
                .snapshot_path
                .file_name()
                .context("snapshot has no filename")?;
            let file = open_snapshot_resolved(&parent.join(filename))?;
            Some(file)
        } else {
            None
        };
        let (parent, filename) = match parent(root, target.strip_prefix(root)?, cp.existed) {
            Ok(pair) => pair,
            Err(error)
                if !cp.existed
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let filename = name(Path::new(&filename))?;
        if let Some(snapshot) = &mut snapshot {
            let staged = Staged::new(root, snapshot)?;
            staged.commit(&parent, &filename)
        } else {
            remove(&parent, &filename)
        }
    }

    fn open_snapshot_resolved(path: &Path) -> Result<File> {
        // The full canonical parent path is checked again from a filesystem
        // root descriptor; a substituted ancestor link cannot redirect reads.
        // Snapshot storage may use a separate mount, so NO_XDEV is not set here.
        let file = open_at_resolve(
            &File::open("/")?,
            path.strip_prefix("/")?,
            libc::O_RDONLY,
            0,
            0x02 | 0x04 | 0x08,
        )?;
        if !file.metadata()?.is_file() {
            bail!("restore snapshot must be a regular file");
        }
        Ok(file)
    }

    fn remove(parent: &File, filename: &CString) -> Result<()> {
        // unlinkat never follows the final link. Missing files are idempotent.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), filename.as_ptr(), 0) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        Ok(())
    }

    struct Staged {
        _directory: tempfile::TempDir,
        parent: File,
        filename: CString,
    }
    impl Staged {
        fn new(root: &Path, snapshot: &mut File) -> Result<Self> {
            // A same-UID restricted shell can mutate names inside the workspace.
            // Keep the rename source outside its Landlock write hierarchy.
            let outside = root
                .parent()
                .context("workspace has no external staging parent")?;
            let directory = tempfile::Builder::new()
                .prefix(".hyper-restore-")
                .tempdir_in(outside)
                .context("cannot create external restore staging directory")?;
            let parent = File::open(directory.path())?;
            let filename = CString::new("content")?;
            let mut output = open_at(
                &parent,
                Path::new("content"),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            std::io::copy(snapshot, &mut output)?;
            output.set_permissions(snapshot.metadata()?.permissions())?;
            output.sync_all()?;
            Ok(Self {
                _directory: directory,
                parent,
                filename,
            })
        }
        fn commit(&self, parent: &File, filename: &CString) -> Result<()> {
            if unsafe {
                libc::renameat(
                    self.parent.as_raw_fd(),
                    self.filename.as_ptr(),
                    parent.as_raw_fd(),
                    filename.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error()).context("atomic restore commit failed; external staging must share the destination mount");
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{PermissionsExt, symlink};
        use std::path::PathBuf;
        use tempfile::tempdir;

        fn checkpoint(snapshot: PathBuf, existed: bool) -> Checkpoint {
            Checkpoint {
                id: "test".into(),
                target_path: "target".into(),
                snapshot_path: snapshot,
                existed,
                created_at: crate::workspace::now(),
            }
        }

        #[test]
        fn target_link_substituted_after_resolution_is_replaced_without_following() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(root.path().join("target"), "changed").unwrap();
            fs::write(outside.path().join("victim"), "OUTSIDE").unwrap();
            fs::write(outside.path().join("snapshot"), "original").unwrap();
            let resolved = crate::workspace::resolve_path(root.path(), "target").unwrap();
            fs::remove_file(&resolved).unwrap();
            symlink(outside.path().join("victim"), &resolved).unwrap();
            restore_resolved(
                root.path(),
                &resolved,
                &checkpoint(outside.path().join("snapshot"), true),
            )
            .unwrap();
            assert_eq!(fs::read_to_string(&resolved).unwrap(), "original");
            assert!(
                !fs::symlink_metadata(resolved)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                fs::read_to_string(outside.path().join("victim")).unwrap(),
                "OUTSIDE"
            );
        }

        #[test]
        fn parent_link_substitution_refuses_both_restore_and_removal() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::create_dir(root.path().join("dir")).unwrap();
            fs::write(outside.path().join("snapshot"), "original").unwrap();
            fs::write(outside.path().join("target"), "OUTSIDE").unwrap();
            let resolved = crate::workspace::resolve_path(root.path(), "dir/target").unwrap();
            fs::remove_dir(root.path().join("dir")).unwrap();
            symlink(outside.path(), root.path().join("dir")).unwrap();
            for existed in [true, false] {
                assert!(
                    restore_resolved(
                        root.path(),
                        &resolved,
                        &checkpoint(outside.path().join("snapshot"), existed)
                    )
                    .is_err()
                );
            }
            assert_eq!(
                fs::read_to_string(outside.path().join("target")).unwrap(),
                "OUTSIDE"
            );
        }

        #[test]
        fn removal_never_follows_a_substituted_final_link() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(root.path().join("target"), "changed").unwrap();
            fs::write(outside.path().join("victim"), "OUTSIDE").unwrap();
            let resolved = crate::workspace::resolve_path(root.path(), "target").unwrap();
            fs::remove_file(&resolved).unwrap();
            symlink(outside.path().join("victim"), &resolved).unwrap();
            restore_resolved(
                root.path(),
                &resolved,
                &checkpoint(outside.path().join("unused"), false),
            )
            .unwrap();
            assert!(fs::symlink_metadata(&resolved).is_err());
            assert_eq!(
                fs::read_to_string(outside.path().join("victim")).unwrap(),
                "OUTSIDE"
            );
        }

        #[test]
        fn staged_source_and_destination_are_pinned_through_commit() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::create_dir(root.path().join("dir")).unwrap();
            fs::write(outside.path().join("snapshot"), "original").unwrap();
            let mut input = File::open(outside.path().join("snapshot")).unwrap();
            let staged = Staged::new(root.path(), &mut input).unwrap();
            assert!(!staged._directory.path().starts_with(root.path()));
            let (directory, filename) = parent(root.path(), Path::new("dir/target"), true).unwrap();
            fs::rename(root.path().join("dir"), root.path().join("retained")).unwrap();
            symlink(outside.path(), root.path().join("dir")).unwrap();
            staged
                .commit(&directory, &name(Path::new(&filename)).unwrap())
                .unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("retained/target")).unwrap(),
                "original"
            );
            assert!(!outside.path().join("target").exists());
        }

        #[test]
        fn hardlinked_target_does_not_modify_other_aliases_and_mode_is_restored() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(outside.path().join("victim"), "ALIAS").unwrap();
            fs::hard_link(outside.path().join("victim"), root.path().join("target")).unwrap();
            fs::write(outside.path().join("snapshot"), [0, 255, 1, 0, 42]).unwrap();
            fs::set_permissions(
                outside.path().join("snapshot"),
                fs::Permissions::from_mode(0o751),
            )
            .unwrap();
            restore_resolved(
                root.path(),
                &root.path().join("target"),
                &checkpoint(outside.path().join("snapshot"), true),
            )
            .unwrap();
            assert_eq!(
                fs::read(root.path().join("target")).unwrap(),
                [0, 255, 1, 0, 42]
            );
            assert_eq!(
                fs::metadata(root.path().join("target"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
            assert_eq!(
                fs::read_to_string(outside.path().join("victim")).unwrap(),
                "ALIAS"
            );
        }

        #[test]
        fn invalid_snapshot_and_failed_commit_leave_original_target_intact() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(root.path().join("target"), "UNCHANGED").unwrap();
            fs::write(outside.path().join("victim"), "OUTSIDE").unwrap();
            symlink(
                outside.path().join("victim"),
                outside.path().join("snapshot"),
            )
            .unwrap();
            assert!(
                restore_resolved(
                    root.path(),
                    &root.path().join("target"),
                    &checkpoint(outside.path().join("snapshot"), true)
                )
                .is_err()
            );
            assert_eq!(
                fs::read_to_string(root.path().join("target")).unwrap(),
                "UNCHANGED"
            );
            fs::create_dir(root.path().join("directory")).unwrap();
            fs::write(outside.path().join("valid"), "original").unwrap();
            let mut input = File::open(outside.path().join("valid")).unwrap();
            let staged = Staged::new(root.path(), &mut input).unwrap();
            let stage_path = staged._directory.path().to_owned();
            assert!(
                staged
                    .commit(
                        &File::open(root.path()).unwrap(),
                        &CString::new("directory").unwrap()
                    )
                    .is_err()
            );
            drop(staged);
            assert!(!stage_path.exists());
            assert!(root.path().join("directory").is_dir());
        }
        #[test]
        fn snapshot_parent_substituted_after_resolution_cannot_redirect_reads() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::create_dir(root.path().join("source")).unwrap();
            fs::write(root.path().join("source/snapshot"), "original").unwrap();
            fs::write(outside.path().join("snapshot"), "SECRET").unwrap();
            let resolved = root
                .path()
                .join("source")
                .canonicalize()
                .unwrap()
                .join("snapshot");
            fs::remove_file(&resolved).unwrap();
            fs::remove_dir(root.path().join("source")).unwrap();
            symlink(outside.path(), root.path().join("source")).unwrap();
            assert!(open_snapshot_resolved(&resolved).is_err());
        }

        #[test]
        fn absent_target_and_parents_are_idempotent_without_creating_directories() {
            let root = tempdir().unwrap();
            let cp = checkpoint(root.path().join("unused"), false);
            for path in ["missing/target", "target"] {
                restore_resolved(root.path(), &root.path().join(path), &cp).unwrap();
                assert!(!root.path().join(path).exists());
            }
            assert!(!root.path().join("missing").exists());
        }
        #[test]
        fn restricted_shell_cannot_mutate_external_staging_names_or_content() {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(outside.path().join("snapshot"), "original").unwrap();
            let mut input = File::open(outside.path().join("snapshot")).unwrap();
            let staged = Staged::new(root.path(), &mut input).unwrap();
            let source = staged._directory.path().join("content");
            let script = format!(
                "import pathlib,os,errno\np=pathlib.Path({})\nfor f in [lambda:p.write_text('FORGED'),lambda:p.unlink(),lambda:p.rename('stolen'),lambda:os.link(p,'alias')]:\n try: f()\n except OSError as e: assert e.errno in (errno.EACCES,errno.EPERM,errno.EXDEV)\n else: raise AssertionError('staging was writable')\nprint('blocked')",
                serde_json::to_string(&source).unwrap()
            );
            let instruction = format!("bash:python3 -c '{}'", script.replace('\'', "'\\''"));
            let task = crate::TaskSpec {
                id: None,
                name: "staging-boundary".into(),
                metadata: Default::default(),
                steps: vec![crate::StepSpec {
                    id: "attack".into(),
                    mode: crate::AgentMode::Build,
                    instruction,
                    tools: None,
                    timeout_ms: None,
                    limits: None,
                    verify: None,
                    metadata: Default::default(),
                }],
            };
            let run = crate::run_task(&task, root.path()).unwrap();
            assert_eq!(run.status, "finished", "{:?}", run.failure);
            let (_, events) = crate::get_run_details(root.path(), &run.run_id).unwrap();
            assert!(events.iter().any(|e| e.event_type == "tool.started"));
            assert!(!events.iter().any(|e| e.event_type == "tool.denied"));
            assert_eq!(fs::read_to_string(source).unwrap(), "original");
        }

        #[test]
        fn snapshot_on_another_mount_restores_via_local_external_staging() {
            use std::os::unix::fs::MetadataExt;
            // Linux CI may not provide writable shared memory; do not require a
            // mount privilege merely to construct this optional fixture.
            let Ok(root) = tempfile::tempdir_in("/dev/shm") else {
                return;
            };
            let outside = tempdir().unwrap();
            if fs::metadata(root.path()).unwrap().dev()
                == fs::metadata(outside.path()).unwrap().dev()
            {
                return;
            }
            fs::write(outside.path().join("snapshot"), "cross-mount").unwrap();
            restore_resolved(
                root.path(),
                &root.path().join("target"),
                &checkpoint(outside.path().join("snapshot"), true),
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("target")).unwrap(),
                "cross-mount"
            );
        }
    }
}
