//! macOS direct file traversal through pinned directories and single-component
//! openat calls. Also compiled in Linux tests to exercise the portable walker.
use anyhow::{Context, Result, bail};
use std::{
    ffi::{CString, OsString},
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path},
};

fn component(path: &Path) -> std::io::Result<CString> {
    let mut parts = path.components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file traversal requires a single normal component",
        ));
    }
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

pub(crate) fn open_at(
    parent: &File,
    path: &Path,
    flags: i32,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    let path = component(path)?;
    // Darwin's mode_t is u16; C variadic arguments require integer promotion.
    #[cfg(target_os = "macos")]
    let mode = libc::c_uint::from(mode);
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            path.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Own the descriptor immediately, including on subsequent validation errors.
    let file = unsafe { File::from_raw_fd(fd) };
    if file.metadata()?.dev() != parent.metadata()?.dev() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file traversal crosses filesystem device",
        ));
    }
    Ok(file)
}

pub(crate) fn parent(root: &Path, relative: &Path, create: bool) -> Result<(File, OsString)> {
    // Validate the complete path before creating any directory. In particular,
    // do not let an absolute name or '..' bypass the descriptor traversal.
    let parts: Vec<_> = relative
        .components()
        .map(|part| {
            if let Component::Normal(name) = part {
                component(Path::new(name))?;
                Ok(name)
            } else {
                bail!("file traversal requires a relative path without dot components")
            }
        })
        .collect::<Result<_>>()?;
    let Some((name, directories)) = parts.split_last() else {
        bail!("file path is empty");
    };
    let mut parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    for name in directories {
        let path = Path::new(name);
        parent = match open_at(&parent, path, libc::O_RDONLY | libc::O_DIRECTORY, 0) {
            Ok(file) => file,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                let name = component(path)?;
                if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) } < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                // A racing creator can insert a link; never follow it.
                open_at(&parent, path, libc::O_RDONLY | libc::O_DIRECTORY, 0)?
            }
            Err(error) => {
                return Err(error)
                    .context("workspace directory changed or cannot be opened safely");
            }
        };
    }
    Ok((parent, (*name).to_owned()))
}

pub(crate) fn open(
    root: &Path,
    relative: &Path,
    writable: bool,
    create: bool,
) -> Result<(File, bool)> {
    let (parent, name) = parent(root, relative, create)?;
    let flags = if writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    let (file, existed) = match open_at(&parent, Path::new(&name), flags, 0) {
        Ok(file) => (file, true),
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            // Never overwrite an entry introduced after the failed lookup.
            (
                open_at(
                    &parent,
                    Path::new(&name),
                    flags | libc::O_CREAT | libc::O_EXCL,
                    0o666,
                )?,
                false,
            )
        }
        Err(error) => {
            return Err(error).context("workspace file changed or cannot be opened safely");
        }
    };
    if !file.metadata()?.is_file() {
        bail!("file tool requires a regular file");
    }
    Ok((file, existed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        os::unix::fs::symlink,
    };

    #[test]
    fn invalid_components_are_rejected_before_directory_creation() {
        let root = tempfile::tempdir().unwrap();
        for path in ["", "/absolute", "../outside", "new/../outside", "./file"] {
            assert!(
                open(root.path(), Path::new(path), true, true).is_err(),
                "{path}"
            );
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let root_fd = File::open(root.path()).unwrap();
        for path in ["..", "/outside", "parent/file", "."] {
            assert!(open_at(&root_fd, Path::new(path), libc::O_RDONLY, 0).is_err());
        }
    }

    #[test]
    fn substituted_final_and_parent_links_cannot_redirect_io() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "SECRET").unwrap();
        symlink(outside.path().join("secret"), root.path().join("target")).unwrap();
        for writable in [false, true] {
            assert!(open(root.path(), Path::new("target"), writable, true).is_err());
        }
        symlink(outside.path(), root.path().join("parent")).unwrap();
        assert!(open(root.path(), Path::new("parent/new/file"), true, true).is_err());
        assert!(!outside.path().join("new").exists());
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "SECRET"
        );
    }

    #[test]
    fn pinned_parent_stays_on_the_original_directory_after_path_replacement() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("parent")).unwrap();
        let (parent, name) = parent(root.path(), Path::new("parent/file"), false).unwrap();
        fs::rename(root.path().join("parent"), root.path().join("retained")).unwrap();
        symlink(outside.path(), root.path().join("parent")).unwrap();
        let mut file = open_at(
            &parent,
            Path::new(&name),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .unwrap();
        file.write_all(b"original directory").unwrap();
        assert_eq!(
            fs::read(root.path().join("retained/file")).unwrap(),
            b"original directory"
        );
        assert!(!outside.path().join("file").exists());
    }

    #[test]
    fn exclusive_creation_refuses_collisions_and_nested_files_remain_usable() {
        let root = tempfile::tempdir().unwrap();
        let (mut file, existed) =
            open(root.path(), Path::new("new/nested/file"), true, true).unwrap();
        assert!(!existed);
        file.write_all(b"content").unwrap();
        let (parent, name) = parent(root.path(), Path::new("new/nested/file"), false).unwrap();
        assert!(
            open_at(
                &parent,
                Path::new(&name),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600
            )
            .is_err()
        );
        let (mut file, existed) =
            open(root.path(), Path::new("new/nested/file"), false, false).unwrap();
        assert!(existed);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"content");
    }

    #[test]
    fn fifo_and_directory_targets_are_rejected_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let fifo = CString::new(root.path().join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        fs::create_dir(root.path().join("directory")).unwrap();
        for path in ["fifo", "directory"] {
            assert!(
                open(root.path(), Path::new(path), false, false)
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
        }
    }
}
