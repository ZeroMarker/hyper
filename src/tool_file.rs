//! Direct file I/O anchored to the workspace, with Linux descriptor confinement.
use anyhow::{Context, Result, bail};
#[cfg(not(target_os = "linux"))]
use std::fs;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct ToolFile {
    file: File,
    pub target: PathBuf,
    pub existed: bool,
}

impl ToolFile {
    #[cfg(all(test, target_os = "linux"))]
    pub fn open(root: &Path, target: &str, writable: bool, create: bool) -> Result<Self> {
        let resolved = crate::workspace::resolve_tool_path(root, target)?;
        Self::open_resolved(root, resolved, writable, create)
    }

    pub(crate) fn open_resolved(
        root: &Path,
        target: PathBuf,
        writable: bool,
        create: bool,
    ) -> Result<Self> {
        let relative = target.strip_prefix(root)?;
        #[cfg(target_os = "linux")]
        let (file, existed) = linux::open(root, relative, writable, create)?;
        #[cfg(not(target_os = "linux"))]
        let (file, existed) = {
            if create && let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let existing = target.exists();
            (
                fs::OpenOptions::new()
                    .read(true)
                    .write(writable)
                    .create(create)
                    .truncate(false)
                    .open(&target)?,
                existing,
            )
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("file tool requires a regular file: {}", relative.display());
        }
        crate::workspace::check_tool_inode(root, &metadata)?;
        Ok(Self {
            file,
            target,
            existed,
        })
    }

    pub(crate) fn check_scope_links(
        &self,
        permissions: &crate::ToolPermissions,
        tool: &str,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if permissions
                .rules
                .iter()
                .any(|r| r.tool == tool && r.path.is_some())
                && self.file.metadata()?.nlink() > 1
            {
                bail!(
                    "permission scope denies hardlinked file: {}",
                    self.target.display()
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (permissions, tool);
        Ok(())
    }

    pub fn read(&mut self) -> Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn read_text(&mut self) -> Result<String> {
        String::from_utf8(self.read()?).context("file is not valid UTF-8")
    }

    pub fn replace(&mut self, content: &[u8]) -> Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(content)?;
        self.file.set_len(content.len() as u64)?;
        Ok(())
    }

    pub fn checkpoint(&mut self, root: &Path, dir: &Path) -> Result<crate::workspace::Checkpoint> {
        #[cfg(target_os = "linux")]
        {
            crate::checkpoint_file::write(
                root,
                dir,
                &self.target,
                self.existed,
                Some(&mut self.file),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            fs::create_dir_all(dir)?;
            let id = crate::workspace::id();
            let snapshot = dir.join(format!("{id}.snapshot"));
            fs::write(&snapshot, self.read()?)?;
            fs::set_permissions(&snapshot, self.file.metadata()?.permissions())?;
            let cp = crate::workspace::Checkpoint {
                id: id.clone(),
                target_path: self
                    .target
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF-8 target")?
                    .into(),
                snapshot_path: snapshot,
                existed: self.existed,
                created_at: crate::workspace::now(),
            };
            fs::write(
                dir.join(format!("{id}.json")),
                serde_json::to_vec_pretty(&cp)?,
            )?;
            Ok(cp)
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    use super::*;
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    pub(crate) fn open_at(
        parent: &File,
        path: &Path,
        flags: i32,
        mode: u64,
    ) -> std::io::Result<File> {
        open_at_resolve(parent, path, flags, mode, 0x01 | 0x02 | 0x04 | 0x08)
    }

    pub(crate) fn open_at_resolve(
        parent: &File,
        path: &Path,
        flags: i32,
        mode: u64,
        resolve: u64,
    ) -> std::io::Result<File> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        // NO_XDEV | NO_MAGICLINKS | NO_SYMLINKS | BENEATH. Existing internal
        // symlinks are resolved by policy first; a substituted link is refused.
        let how = OpenHow {
            flags: (flags | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64,
            mode,
            resolve,
        };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                parent.as_raw_fd(),
                path.as_ptr(),
                &how,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd as i32) })
    }

    pub(crate) fn parent(
        root: &Path,
        relative: &Path,
        create: bool,
    ) -> Result<(File, std::ffi::OsString)> {
        let mut parent = File::open(root)?;
        let components: Vec<_> = relative.components().collect();
        let Some((name, directories)) = components.split_last() else {
            bail!("file path is empty");
        };
        for component in directories {
            let path = Path::new(component.as_os_str());
            parent = match open_at(&parent, path, libc::O_RDONLY | libc::O_DIRECTORY, 0) {
                Ok(file) => file,
                Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
                    let name = CString::new(path.as_os_str().as_bytes())?;
                    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) };
                    if result < 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::AlreadyExists
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    open_at(&parent, path, libc::O_RDONLY | libc::O_DIRECTORY, 0)?
                }
                Err(e) => {
                    return Err(e)
                        .context("workspace directory changed or cannot be opened safely");
                }
            };
        }
        Ok((parent, name.as_os_str().to_owned()))
    }

    pub(crate) fn open(
        root: &Path,
        relative: &Path,
        writable: bool,
        create: bool,
    ) -> Result<(File, bool)> {
        let (parent, name) = parent(root, relative, create)?;
        let path = Path::new(name.as_os_str());
        let flags = if writable {
            libc::O_RDWR
        } else {
            libc::O_RDONLY
        };
        match open_at(&parent, path, flags, 0) {
            Ok(file) => Ok((file, true)),
            Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
                // O_EXCL refuses any file/link inserted between absence and creation.
                Ok((
                    open_at(&parent, path, flags | libc::O_CREAT | libc::O_EXCL, 0o666)?,
                    false,
                ))
            }
            Err(e) => Err(e).context("workspace file changed or cannot be opened safely"),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    use tempfile::tempdir;

    #[test]
    fn replacement_link_after_resolution_cannot_redirect_io() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(root.path().join("victim"), "original").unwrap();
        fs::write(outside.path().join("secret"), "SECRET").unwrap();
        let resolved = crate::workspace::resolve_tool_path(root.path(), "victim").unwrap();
        fs::remove_file(&resolved).unwrap();
        symlink(outside.path().join("secret"), &resolved).unwrap();
        for writable in [false, true] {
            assert!(
                ToolFile::open_resolved(root.path(), resolved.clone(), writable, false).is_err()
            );
        }
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "SECRET"
        );
    }

    #[test]
    fn replacement_parent_link_cannot_redirect_creation() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::create_dir(root.path().join("parent")).unwrap();
        let resolved = crate::workspace::resolve_tool_path(root.path(), "parent/new/file").unwrap();
        fs::remove_dir(root.path().join("parent")).unwrap();
        symlink(outside.path(), root.path().join("parent")).unwrap();
        assert!(ToolFile::open_resolved(root.path(), resolved, true, true).is_err());
        assert!(!outside.path().join("new").exists());
    }

    #[test]
    fn opened_inode_is_shared_by_read_snapshot_and_modification() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let snapshots = tempdir().unwrap();
        fs::write(root.path().join("victim"), "before").unwrap();
        fs::write(outside.path().join("secret"), "SECRET").unwrap();
        let mut file = ToolFile::open(root.path(), "victim", true, false).unwrap();
        fs::rename(root.path().join("victim"), root.path().join("original")).unwrap();
        symlink(outside.path().join("secret"), root.path().join("victim")).unwrap();
        assert_eq!(file.read_text().unwrap(), "before");
        let cp = file.checkpoint(root.path(), snapshots.path()).unwrap();
        file.replace(b"after").unwrap();
        assert_eq!(fs::read_to_string(cp.snapshot_path).unwrap(), "before");
        assert_eq!(
            fs::read_to_string(root.path().join("original")).unwrap(),
            "after"
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "SECRET"
        );
    }
    #[test]
    fn audit_hardlink_inserted_after_resolution_is_checked_on_opened_inode() {
        let root = tempdir().unwrap();
        let workspace = crate::Workspace::open(root.path()).unwrap();
        let protected = workspace.paths.dir.join("marker");
        fs::write(&protected, "AUDIT").unwrap();
        fs::write(root.path().join("victim"), "ordinary").unwrap();
        let resolved = crate::workspace::resolve_tool_path(root.path(), "victim").unwrap();
        fs::remove_file(&resolved).unwrap();
        fs::hard_link(&protected, &resolved).unwrap();
        for writable in [false, true] {
            assert!(
                ToolFile::open_resolved(root.path(), resolved.clone(), writable, false)
                    .unwrap_err()
                    .to_string()
                    .contains("protected audit hardlink")
            );
        }
        assert_eq!(fs::read_to_string(protected).unwrap(), "AUDIT");
    }

    #[test]
    fn internal_symlinks_and_ordinary_hardlinks_remain_usable() {
        let root = tempdir().unwrap();
        crate::Workspace::open(root.path()).unwrap();
        fs::write(root.path().join("original"), "ordinary").unwrap();
        symlink(root.path().join("original"), root.path().join("link")).unwrap();
        fs::hard_link(root.path().join("original"), root.path().join("hard")).unwrap();
        for path in ["link", "hard"] {
            assert_eq!(
                ToolFile::open(root.path(), path, false, false)
                    .unwrap()
                    .read_text()
                    .unwrap(),
                "ordinary"
            );
        }
    }

    #[test]
    fn fifo_is_rejected_without_waiting_for_a_writer() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let root = tempdir().unwrap();
        let name = CString::new(root.path().join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(
            ToolFile::open(root.path(), "fifo", false, false)
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }
}
