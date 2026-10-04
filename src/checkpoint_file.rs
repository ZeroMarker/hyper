//! Linux checkpoint sources and output commits anchored to directory descriptors.
use crate::{
    tool_file::linux::{open_at, open_at_resolve},
    workspace::Checkpoint,
};
use anyhow::{Context, Result, bail};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

pub(crate) fn create(root: &Path, dir: &Path, target: &str) -> Result<Checkpoint> {
    let root = root.canonicalize()?;
    let target = crate::workspace::resolve_path(&root, target)?;
    let mut source = open_source(&root, &target)?;
    write(&root, dir, &target, source.is_some(), source.as_mut())
}

fn open_source(root: &Path, target: &Path) -> Result<Option<File>> {
    let relative = target.strip_prefix(root)?;
    match crate::tool_file::linux::open(root, relative, false, false) {
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
        // complete JSON are written. Never replace a colliding manifest/link.
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
        Self::open_resolved(path.canonicalize()?)
    }

    fn open_resolved(path: PathBuf) -> Result<Self> {
        let root = File::open("/")?;
        let relative = path.strip_prefix("/")?;
        let relative = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };
        // Cross-mount host-selected checkpoint storage is permitted. A path
        // substituted after canonicalization must not redirect output.
        let file = open_at_resolve(
            &root,
            relative,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
            0x02 | 0x04 | 0x08,
        )?;
        Ok(Self { path, file })
    }

    fn create(&self, name: &str) -> Result<File> {
        Ok(open_at(
            &self.file,
            Path::new(name),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?)
    }

    fn publish(&self, from: &str, to: &str) -> Result<()> {
        let from = CString::new(from)?;
        let to = CString::new(to)?;
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error())
                .context("checkpoint manifest commit failed");
        }
        Ok(())
    }

    fn remove(&self, name: &str) {
        if let Ok(name) = CString::new(name.as_bytes()) {
            // Cleanup only entries created by this operation in the pinned
            // directory; no recursive traversal or following of final links.
            unsafe {
                libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::tempdir;

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
        // Also verify the writer rewinds a previously consumed descriptor.
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
        assert!(Directory::open_resolved(resolved).is_err());
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
        // A descriptor open for writing cannot supply snapshot bytes.
        let mut unreadable = File::create(root.path().join("file")).unwrap();
        let directory = Directory::prepare(dest.path()).unwrap();
        assert!(write_in(&directory, "file", true, Some(&mut unreadable), "failed").is_err());
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
        assert_eq!(fs::read(root.path().join("file")).unwrap(), b"");
    }
}
