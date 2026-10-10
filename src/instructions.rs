//! Project instructions loaded from the workspace root.
//!
//! Only `<root>/AGENTS.md` is read, and it is treated as untrusted guidance for
//! the model: the bytes are read through the anchored file entry (no symlink
//! escape, no audit path), never executed, capped by a fixed byte budget, and
//! recorded verbatim in the model request so a replay rebuilds the exact prompt.
//! Nothing here can change the host's authorization, tool permissions or
//! execution boundaries.

use anyhow::Result;
use std::path::Path;

/// The one project instruction file this version loads.
pub(crate) const FILE: &str = "AGENTS.md";
/// Cap on the bytes taken from the project instruction file. The model request
/// still carries the whole block, so this is a context budget, not a truncation
/// of what the file says.
pub(crate) const MAX_BYTES: usize = 16_000;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProjectInstructions {
    pub source: String,
    pub text: String,
    /// Size of the file on disk, before any budget truncation.
    pub bytes: usize,
    pub truncated: bool,
}

/// Read the root `AGENTS.md`, if present. `None` means there is nothing to load
/// (the file is absent). A symlink escaping the workspace or an audit path is
/// refused rather than followed, matching the direct file tools.
pub(crate) fn load(root: &Path) -> Result<Option<ProjectInstructions>> {
    let resolved = match crate::workspace::resolve_tool_path(root, FILE) {
        Ok(path) => path,
        Err(error) if is_missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut file = match crate::tool_file::ToolFile::open_resolved(root, resolved, false, false) {
        Ok(file) => file,
        Err(error) if is_missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let raw = file.read()?;
    let (text, truncated) = budget(&raw, MAX_BYTES);
    Ok(Some(ProjectInstructions {
        source: FILE.to_owned(),
        bytes: raw.len(),
        text,
        truncated,
    }))
}

fn is_missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// Keep at most `max` bytes on a UTF-8 boundary, with an explicit marker so the
/// model knows the tail was dropped rather than omitted by the author.
fn budget(bytes: &[u8], max: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= max {
        return (text.into_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        format!(
            "{}\n... [truncated to {max} of {} bytes] ...\n",
            &text[..end],
            text.len()
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn absent_file_loads_nothing() {
        let root = tempdir().unwrap();
        assert_eq!(load(root.path()).unwrap(), None);
    }

    #[test]
    fn present_file_is_returned_verbatim() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(FILE), "Always run `cargo test`.\n").unwrap();
        let loaded = load(root.path()).unwrap().unwrap();
        assert_eq!(loaded.source, FILE);
        assert_eq!(loaded.text, "Always run `cargo test`.\n");
        assert_eq!(loaded.bytes, 25);
        assert!(!loaded.truncated);
    }

    #[test]
    fn oversized_file_is_capped_on_a_character_boundary() {
        let root = tempdir().unwrap();
        // Multi-byte characters straddle the cap so the split must back up.
        let text = "界".repeat(MAX_BYTES);
        fs::write(root.path().join(FILE), &text).unwrap();
        let loaded = load(root.path()).unwrap().unwrap();
        assert!(loaded.truncated);
        assert_eq!(loaded.bytes, text.len());
        assert!(loaded.text.contains("[truncated to"));
        assert!(loaded.text.is_char_boundary(loaded.text.len()));
        assert!(loaded.text.len() <= MAX_BYTES + 64);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_escaping_the_workspace_is_refused() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("AGENTS.md"), "SECRET").unwrap();
        symlink(outside.path().join("AGENTS.md"), root.path().join(FILE)).unwrap();
        assert!(load(root.path()).is_err());
    }
}
