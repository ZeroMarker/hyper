//! Fail closed on Windows file aliases using the actual open file handle.
//!
//! Reject all multiply linked files rather than relying on a path-based audit
//! scan or file IDs that may not be unique on every Windows filesystem.
use anyhow::{Context, Result, bail};
use std::{ffi::c_void, fs::File, mem::MaybeUninit, os::windows::io::AsRawHandle};

#[repr(C)]
struct FileTime {
    low: u32,
    high: u32,
}

// BY_HANDLE_FILE_INFORMATION from fileapi.h. Every field is a DWORD or FILETIME.
#[repr(C)]
struct FileInformation {
    attributes: u32,
    creation: FileTime,
    access: FileTime,
    write: FileTime,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(handle: *mut c_void, info: *mut FileInformation) -> i32;
}

pub(crate) fn require_single_link(file: &File) -> Result<()> {
    let mut info = MaybeUninit::<FileInformation>::uninit();
    // The handle belongs to a live File; the API fills the complete C struct
    // on success. Never read its contents when the API reports failure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error())
            .context("tool permission cannot verify Windows file hardlink count");
    }
    if unsafe { info.assume_init() }.links != 1 {
        bail!("tool permission denies Windows file hardlink alias in file tools or audit storage");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn windows_link_check_uses_the_open_handle_after_path_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        fs::write(&path, "original").unwrap();
        let file = File::open(&path).unwrap();
        require_single_link(&file).unwrap();
        fs::rename(&path, root.path().join("moved")).unwrap();
        fs::write(&path, "replacement").unwrap();
        fs::hard_link(root.path().join("moved"), root.path().join("alias")).unwrap();
        assert!(require_single_link(&file).is_err());
        require_single_link(&File::open(&path).unwrap()).unwrap();
    }

    #[test]
    fn windows_opened_tool_rechecks_alias_inserted_after_resolution() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("target"), "original").unwrap();
        let root = root.path().canonicalize().unwrap();
        let target = crate::workspace::resolve_tool_path(&root, "target").unwrap();
        fs::hard_link(&target, root.join("alias")).unwrap();
        let error =
            crate::tool_file::ToolFile::open_resolved(&root, target, true, false).unwrap_err();
        assert!(error.to_string().contains("hardlink alias"));
        assert_eq!(fs::read_to_string(root.join("target")).unwrap(), "original");
    }

    #[test]
    fn windows_audit_validation_rejects_preexisting_hardlinks() {
        let root = tempfile::tempdir().unwrap();
        let audit = tempfile::tempdir().unwrap();
        let record = audit.path().join("record");
        fs::write(&record, "audit").unwrap();
        crate::state::validate_shell_boundary(root.path(), audit.path()).unwrap();
        fs::hard_link(&record, root.path().join("alias")).unwrap();
        assert!(crate::state::validate_shell_boundary(root.path(), audit.path()).is_err());
    }
}
