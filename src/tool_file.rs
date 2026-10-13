//! Direct file I/O anchored to the workspace, with Linux and macOS descriptor traversal.
use anyhow::{Context, Result, bail};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    root: PathBuf,
    baseline: Option<Vec<u8>>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    parent: File,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    filename: std::ffi::OsString,
}

impl ToolFile {
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub fn open(root: &Path, target: &str, writable: bool, create: bool) -> Result<Self> {
        let root = root.canonicalize()?;
        let resolved = crate::workspace::resolve_tool_path(&root, target)?;
        Self::open_resolved(&root, resolved, writable, create)
    }

    pub(crate) fn open_resolved(
        root: &Path,
        target: PathBuf,
        writable: bool,
        create: bool,
    ) -> Result<Self> {
        let relative = target.strip_prefix(root)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let (parent, filename) = native::parent(root, relative, create)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let (file, existed) =
            match native::open_at(&parent, Path::new(&filename), libc::O_RDONLY, 0) {
                Ok(file) => (file, true),
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    (tempfile::tempfile()?, false)
                }
                Err(error) => {
                    return Err(error).context("workspace file changed or cannot be opened safely");
                }
            };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let _ = writable;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let (file, existed) = {
            if create && let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            match File::open(&target) {
                Ok(file) => (file, true),
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    (tempfile::tempfile()?, false)
                }
                Err(error) => return Err(error.into()),
            }
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = writable;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("file tool requires a regular file: {}", relative.display());
        }
        crate::workspace::check_tool_inode(root, &metadata)?;
        #[cfg(windows)]
        crate::windows_file::require_single_link(&file)?;
        Ok(Self {
            file,
            target,
            existed,
            root: root.to_owned(),
            baseline: None,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            parent,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            filename,
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
        self.baseline = Some(bytes.clone());
        Ok(bytes)
    }

    pub fn read_text(&mut self) -> Result<String> {
        String::from_utf8(self.read()?).context("file is not valid UTF-8")
    }

    pub fn replace(&mut self, content: &[u8]) -> Result<()> {
        self.replace_with(|output| {
            output.write_all(content)?;
            Ok(())
        })
    }

    fn replace_with(&mut self, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
        let permissions = self.file.metadata()?.permissions();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
            let staged = NativeStaged::write(&self.root, permissions, write)?;
            if self.existed {
                let mut current =
                    native::open_at(&self.parent, Path::new(&self.filename), libc::O_RDONLY, 0)?;
                let original = self.file.metadata()?;
                let metadata = current.metadata()?;
                if !metadata.is_file()
                    || metadata.dev() != original.dev()
                    || metadata.ino() != original.ino()
                {
                    bail!("file changed since it was opened; read the file again");
                }
                let mut bytes = Vec::new();
                current.read_to_end(&mut bytes)?;
                if self
                    .baseline
                    .as_ref()
                    .is_some_and(|before| before != &bytes)
                {
                    bail!("file changed since it was read (stale hash); read the file again");
                }
            }
            let filename = std::ffi::CString::new(self.filename.as_bytes())?;
            if self.existed {
                staged.commit(&self.parent, &filename)?;
            } else {
                staged.commit_new(&self.parent, &filename)?;
            }
            self.file =
                native::open_at(&self.parent, Path::new(&self.filename), libc::O_RDONLY, 0)?;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let outside = self
                .root
                .parent()
                .context("workspace has no external staging parent")?;
            let mut staged = tempfile::NamedTempFile::new_in(outside)?;
            write(staged.as_file_mut())?;
            staged.as_file().set_permissions(permissions)?;
            staged.as_file().sync_all()?;
            if self.existed {
                let current = fs::read(&self.target)?;
                if self
                    .baseline
                    .as_ref()
                    .is_some_and(|before| before != &current)
                {
                    bail!("file changed since it was read (stale hash)");
                }
                #[cfg(windows)]
                crate::windows_file::require_single_link(&File::open(&self.target)?)?;
            }
            // The target handle was opened without FILE_SHARE_DELETE, so it must
            // be closed before MoveFileExW can replace the file it names.
            #[cfg(windows)]
            {
                self.file = tempfile::tempfile()?;
                crate::windows_file::atomic_replace(staged.path(), &self.target, self.existed)?;
            }
            #[cfg(not(windows))]
            if self.existed {
                fs::rename(staged.path(), &self.target)?;
            } else {
                staged.persist_noclobber(&self.target)?;
            }
            self.file = File::open(&self.target)?;
        }
        self.existed = true;
        self.baseline = None;
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
        #[cfg(target_os = "macos")]
        {
            crate::posix_admin::write(root, dir, &self.target, self.existed, Some(&mut self.file))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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

#[cfg(target_os = "macos")]
use crate::posix_admin::Staged as NativeStaged;
#[cfg(target_os = "macos")]
use crate::posix_file as native;
#[cfg(target_os = "linux")]
use crate::restore_file::linux::Staged as NativeStaged;
#[cfg(target_os = "linux")]
use linux as native;

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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    use tempfile::tempdir;

    #[test]
    fn interrupted_staging_keeps_existing_bytes_and_missing_target_absent() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("existing"), "完整内容").unwrap();
        for path in ["existing", "missing"] {
            let mut file = ToolFile::open(root.path(), path, true, true).unwrap();
            file.read().unwrap();
            let error = file.replace_with(|output| {
                output.write_all("半个文件".as_bytes())?;
                bail!("injected write failure")
            });
            assert!(error.is_err());
        }
        assert_eq!(
            fs::read_to_string(root.path().join("existing")).unwrap(),
            "完整内容"
        );
        assert!(!root.path().join("missing").exists());
    }

    #[test]
    fn atomic_replacement_preserves_hardlink_alias_and_snapshot() {
        let root = tempdir().unwrap();
        crate::Workspace::open(root.path()).unwrap();
        let snapshots = tempdir().unwrap();
        fs::write(root.path().join("target"), "世界\n世界\n").unwrap();
        fs::hard_link(root.path().join("target"), root.path().join("alias")).unwrap();
        let mut file = ToolFile::open(root.path(), "target", true, false).unwrap();
        file.read().unwrap();
        let cp = file
            .checkpoint(&root.path().canonicalize().unwrap(), snapshots.path())
            .unwrap();
        file.replace("世界\n新内容 🦀\n".as_bytes()).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("target")).unwrap(),
            "世界\n新内容 🦀\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("alias")).unwrap(),
            "世界\n世界\n"
        );
        assert_eq!(
            fs::read_to_string(cp.snapshot_path).unwrap(),
            "世界\n世界\n"
        );
    }

    #[test]
    fn external_write_during_staging_is_refused_as_stale() {
        let root = tempdir().unwrap();
        let path = root.path().join("target");
        fs::write(&path, "original").unwrap();
        let mut file = ToolFile::open(root.path(), "target", true, false).unwrap();
        file.read().unwrap();
        let error = file
            .replace_with(|output| {
                output.write_all(b"proposed")?;
                std::thread::scope(|scope| {
                    scope
                        .spawn(|| fs::write(&path, "external").unwrap())
                        .join()
                        .unwrap()
                });
                Ok(())
            })
            .unwrap_err();
        assert!(error.to_string().contains("stale hash"));
        assert_eq!(fs::read_to_string(path).unwrap(), "external");
    }

    #[test]
    fn concurrent_creation_is_never_overwritten() {
        let root = tempdir().unwrap();
        let path = root.path().join("target");
        let mut file = ToolFile::open(root.path(), "target", true, true).unwrap();
        assert!(!path.exists());
        assert!(
            file.replace_with(|output| {
                output.write_all(b"proposed")?;
                fs::write(&path, "external")?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "external");
    }

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
                ToolFile::open_resolved(
                    &root.path().canonicalize().unwrap(),
                    resolved.clone(),
                    writable,
                    false
                )
                .is_err()
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
        assert!(
            ToolFile::open_resolved(&root.path().canonicalize().unwrap(), resolved, true, true)
                .is_err()
        );
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
        let cp = file
            .checkpoint(&root.path().canonicalize().unwrap(), snapshots.path())
            .unwrap();
        assert!(file.replace(b"after").is_err());
        assert_eq!(fs::read_to_string(cp.snapshot_path).unwrap(), "before");
        assert_eq!(
            fs::read_to_string(root.path().join("original")).unwrap(),
            "before"
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
                ToolFile::open_resolved(
                    &root.path().canonicalize().unwrap(),
                    resolved.clone(),
                    writable,
                    false
                )
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
