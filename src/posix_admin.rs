//! POSIX descriptor-anchored checkpoints and restore.
//!
//! Apple platforms have no `openat2` and no `RENAME_NOREPLACE`. Snapshot
//! sources, host-selected output directories and restore targets are walked
//! one component at a time with `openat`/`O_NOFOLLOW` from a pinned parent, and
//! the manifest is published with `linkat`, which fails rather than replacing an
//! existing entry. The module is compiled in Linux tests too, so the portable
//! walker and commit semantics are exercised without a macOS host.

use crate::workspace::Checkpoint;
use anyhow::{Context, Result, bail};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path, PathBuf},
};

/// A single normal component as a C string, rejecting `.`/`..`/separators.
fn component(name: &Path) -> std::io::Result<CString> {
    let mut parts = name.components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "checkpoint path requires a single normal component",
        ));
    }
    Ok(CString::new(name.as_os_str().as_bytes())?)
}

/// Open a single component relative to a pinned parent. The caller has already
/// resolved any existing internal symlink; a substituted one is refused.
fn openat(parent: &File, name: &Path, flags: i32, mode: libc::mode_t) -> std::io::Result<File> {
    let name = component(name)?;
    // Darwin's mode_t is u16; C variadic arguments require integer promotion.
    #[cfg(target_os = "macos")]
    let mode = libc::c_uint::from(mode);
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Walk an absolute, canonical host-selected path from `/`. Cross-mount storage
/// is allowed; a link substituted after resolution is refused, not followed.
fn open_directory_absolute(path: &Path) -> Result<File> {
    let mut current = File::open("/")?;
    for value in path.strip_prefix("/")?.components() {
        let Component::Normal(name) = value else {
            bail!(
                "absolute path {} contains a non-normal component",
                path.display()
            );
        };
        current = openat(
            &current,
            Path::new(name),
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )
        .with_context(|| format!("directory {} cannot be opened safely", path.display()))?;
    }
    Ok(current)
}

/// Open a checkpoint snapshot by its stored canonical absolute path. Every
/// component is walked from the filesystem root with `O_NOFOLLOW`, so a link
/// substituted after the checkpoint was recorded cannot redirect the read; the
/// final name is opened without following a link either.
fn open_snapshot_absolute(path: &Path) -> Result<File> {
    let parent = path.parent().context("snapshot has no parent")?;
    let name = path.file_name().context("snapshot has no filename")?;
    let directory = open_directory_absolute(parent)?;
    let file = openat(&directory, Path::new(name), libc::O_RDONLY, 0)?;
    if !file.metadata()?.is_file() {
        bail!("checkpoint snapshot must be a regular file");
    }
    Ok(file)
}

fn open_source(root: &Path, target: &Path) -> Result<Option<File>> {
    let relative = target.strip_prefix(root)?;
    match crate::posix_file::open(root, relative, false, false) {
        Ok((file, _)) => {
            if !file.metadata()?.is_file() {
                bail!("checkpoint source must be a regular file");
            }
            Ok(Some(file))
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn create(root: &Path, dir: &Path, target: &str) -> Result<Checkpoint> {
    let root = root.canonicalize()?;
    let target = crate::workspace::resolve_path(&root, target)?;
    let mut source = open_source(&root, &target)?;
    write(&root, dir, &target, source.is_some(), source.as_mut())
}

pub(crate) fn write(
    root: &Path,
    dir: &Path,
    target: &Path,
    existed: bool,
    source: Option<&mut File>,
) -> Result<Checkpoint> {
    let target = target
        .strip_prefix(root)?
        .to_str()
        .context("non-UTF-8 checkpoint target")?
        .to_owned();
    let directory = Directory::prepare(dir)?;
    write_in(
        &directory,
        &target,
        existed,
        source,
        &crate::workspace::id(),
    )
}

fn write_in(
    directory: &Directory,
    target: &str,
    existed: bool,
    source: Option<&mut File>,
    id: &str,
) -> Result<Checkpoint> {
    let snapshot = format!("{id}.snapshot");
    let manifest = format!("{id}.json");
    let temporary = format!(".{id}.manifest.tmp");
    let cp = Checkpoint {
        id: id.to_owned(),
        target_path: target.to_owned(),
        snapshot_path: directory.path.join(&snapshot),
        existed,
        created_at: crate::workspace::now(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&cp)?;
    let mut snapshot_created = false;
    let mut temporary_created = false;
    let result = (|| {
        let mut output = directory.create(&snapshot)?;
        snapshot_created = true;
        if let Some(source) = source {
            source.seek(SeekFrom::Start(0))?;
            std::io::copy(source, &mut output)?;
            output.set_permissions(source.metadata()?.permissions())?;
        }
        output.sync_all()?;
        let mut metadata = directory.create(&temporary)?;
        temporary_created = true;
        metadata.write_all(&manifest_bytes)?;
        metadata.sync_all()?;
        // A checkpoint becomes discoverable only after its snapshot and
        // complete JSON are written. `linkat` refuses an existing entry.
        directory.publish(&temporary, &manifest)?;
        Ok(cp)
    })();
    if result.is_err() {
        if temporary_created {
            directory.remove(&temporary);
        }
        if snapshot_created {
            directory.remove(&snapshot);
        }
    }
    result
}

struct Directory {
    path: PathBuf,
    file: File,
}

impl Directory {
    fn prepare(path: &Path) -> Result<Self> {
        fs::create_dir_all(path)?;
        let resolved = path.canonicalize()?;
        Ok(Self {
            file: open_directory_absolute(&resolved)?,
            path: resolved,
        })
    }

    fn create(&self, name: &str) -> Result<File> {
        Ok(openat(
            &self.file,
            Path::new(name),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?)
    }

    /// Publish a complete, synced manifest under its final name. `linkat`
    /// creates the new name only if it is absent, so a collision is never
    /// overwritten; the temporary name is dropped afterwards.
    fn publish(&self, from: &str, to: &str) -> Result<()> {
        let from = component(Path::new(from))?;
        let to = component(Path::new(to))?;
        if unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
                0,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error())
                .context("checkpoint manifest commit failed");
        }
        // Cleanup only the temporary name created by this operation.
        unsafe {
            libc::unlinkat(self.file.as_raw_fd(), from.as_ptr(), 0);
        }
        Ok(())
    }

    fn remove(&self, name: &str) {
        if let Ok(name) = component(Path::new(name)) {
            unsafe {
                libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
            }
        }
    }
}

pub(crate) fn restore(root: &Path, target: &Path, cp: &Checkpoint) -> Result<()> {
    // Read-only source is pinned before any workspace directory is created.
    let mut snapshot = if cp.existed {
        Some(open_snapshot_absolute(&cp.snapshot_path)?)
    } else {
        None
    };
    let (parent, filename) =
        match crate::posix_file::parent(root, target.strip_prefix(root)?, cp.existed) {
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
    let filename = component(Path::new(OsStr::new(&filename)))?;
    if let Some(snapshot) = &mut snapshot {
        let staged = Staged::new(root, snapshot)?;
        staged.commit(&parent, &filename)
    } else {
        remove(&parent, &filename)
    }
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
        let mut output = openat(
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
            return Err(std::io::Error::last_os_error()).context(
                "atomic restore commit failed; external staging must share the destination mount",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Checkpoint;
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
    fn final_link_substitution_cannot_redirect_checkpoint_reads() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(root.path().join("file"), "original").unwrap();
        fs::write(outside.path().join("secret"), "SECRET").unwrap();
        let resolved = crate::workspace::resolve_path(root.path(), "file").unwrap();
        fs::remove_file(&resolved).unwrap();
        symlink(outside.path().join("secret"), &resolved).unwrap();
        assert!(open_source(root.path(), &resolved).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "SECRET"
        );
    }

    #[test]
    fn parent_link_substitution_is_not_mistaken_for_missing_source() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::create_dir(root.path().join("parent")).unwrap();
        fs::write(root.path().join("parent/file"), "original").unwrap();
        fs::write(outside.path().join("file"), "SECRET").unwrap();
        let resolved = crate::workspace::resolve_path(root.path(), "parent/file").unwrap();
        fs::rename(root.path().join("parent"), root.path().join("saved")).unwrap();
        symlink(outside.path(), root.path().join("parent")).unwrap();
        assert!(open_source(root.path(), &resolved).is_err());
    }

    #[test]
    fn replaced_filename_does_not_change_the_pinned_snapshot_inode() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let snapshots = tempdir().unwrap();
        let target = root.path().join("file");
        let original = [0, 255, 10, 1];
        fs::write(&target, original).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
        let mut source = open_source(root.path(), &target).unwrap().unwrap();
        source.seek(SeekFrom::End(0)).unwrap();
        fs::rename(&target, root.path().join("original")).unwrap();
        fs::write(outside.path().join("secret"), "SECRET").unwrap();
        symlink(outside.path().join("secret"), &target).unwrap();
        let cp = write(
            root.path(),
            snapshots.path(),
            &target,
            true,
            Some(&mut source),
        )
        .unwrap();
        assert_eq!(fs::read(&cp.snapshot_path).unwrap(), original);
        assert_eq!(
            fs::metadata(cp.snapshot_path).unwrap().permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "SECRET"
        );
    }

    #[test]
    fn output_directory_substitution_is_refused_or_commits_to_pinned_directory() {
        let parent = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let path = parent.path().join("snapshots");
        fs::create_dir(&path).unwrap();
        let resolved = path.canonicalize().unwrap();
        fs::rename(&path, parent.path().join("saved")).unwrap();
        symlink(outside.path(), &path).unwrap();
        assert!(open_directory_absolute(&resolved).is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(parent.path().join("saved"), &path).unwrap();
        let directory = Directory::prepare(&path).unwrap();
        fs::rename(&path, parent.path().join("saved")).unwrap();
        symlink(outside.path(), &path).unwrap();
        let cp = write_in(&directory, "file", false, None, "pinned").unwrap();
        assert!(parent.path().join("saved/pinned.snapshot").is_file());
        let metadata: Checkpoint =
            serde_json::from_slice(&fs::read(parent.path().join("saved/pinned.json")).unwrap())
                .unwrap();
        assert_eq!(metadata.id, cp.id);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn collisions_preserve_existing_entries_and_roll_back_new_snapshot() {
        for collision in ["snapshot", "manifest", "temporary"] {
            let dir = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(outside.path().join("secret"), "KEEP").unwrap();
            let name = match collision {
                "snapshot" => "collision.snapshot",
                "manifest" => "collision.json",
                _ => ".collision.manifest.tmp",
            };
            symlink(outside.path().join("secret"), dir.path().join(name)).unwrap();
            let directory = Directory::prepare(dir.path()).unwrap();
            assert!(write_in(&directory, "file", false, None, "collision").is_err());
            assert!(
                fs::symlink_metadata(dir.path().join(name))
                    .unwrap()
                    .is_symlink()
            );
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            assert_eq!(
                fs::read_to_string(outside.path().join("secret")).unwrap(),
                "KEEP"
            );
        }
    }

    #[test]
    fn failed_copy_leaves_no_snapshot_or_manifest() {
        let root = tempdir().unwrap();
        let dest = tempdir().unwrap();
        let mut unreadable = File::create(root.path().join("file")).unwrap();
        let directory = Directory::prepare(dest.path()).unwrap();
        assert!(write_in(&directory, "file", true, Some(&mut unreadable), "failed").is_err());
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
        assert_eq!(fs::read(root.path().join("file")).unwrap(), b"");
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
        restore(
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
    fn restore_parent_link_substitution_refuses_both_restore_and_removal() {
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
                restore(
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
        restore(
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
        assert!(open_snapshot_absolute(&resolved).is_err());
    }

    #[test]
    fn absent_target_and_parents_are_idempotent_without_creating_directories() {
        let root = tempdir().unwrap();
        let cp = checkpoint(root.path().join("unused"), false);
        for path in ["missing/target", "target"] {
            restore(root.path(), &root.path().join(path), &cp).unwrap();
            assert!(!root.path().join(path).exists());
        }
        assert!(!root.path().join("missing").exists());
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
        restore(
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
    fn cross_mount_snapshot_storage_round_trips_via_local_staging() {
        use std::os::unix::fs::MetadataExt;
        let root = tempdir().unwrap();
        let Ok(snapshots) = tempfile::tempdir_in("/dev/shm") else {
            return;
        };
        if fs::metadata(root.path()).unwrap().dev() == fs::metadata(snapshots.path()).unwrap().dev()
        {
            return;
        }
        fs::write(root.path().join("file"), "cross-mount").unwrap();
        let cp = create(root.path(), snapshots.path(), "file").unwrap();
        fs::write(root.path().join("file"), "changed").unwrap();
        restore(root.path(), &root.path().join("file"), &cp).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("file")).unwrap(),
            "cross-mount"
        );
    }
}
