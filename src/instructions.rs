//! Project instructions loaded from the workspace.
//!
//! Every non-ignored `AGENTS.md` under the workspace root is read, ordered root
//! first and then by directory depth. Each is treated as untrusted guidance for
//! the model: the bytes are read through the anchored file entry (no symlink
//! escape, no audit path), never executed, capped by a fixed total byte budget,
//! and recorded verbatim in the model request so a replay rebuilds the exact
//! prompt. Nothing here can change the host's authorization, tool permissions or
//! execution boundaries.

use anyhow::Result;
use std::path::Path;

/// The project instruction file name this version loads.
pub(crate) const FILE: &str = "AGENTS.md";
/// Total bytes taken across all project instruction files. The request still
/// carries the whole block, so this is a context budget, not a statement about
/// what any single file says.
pub(crate) const MAX_TOTAL_BYTES: usize = 16_000;
/// Upper bound on how many files are read, so a hostile tree cannot make the
/// loader walk unbounded work.
pub(crate) const MAX_FILES: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProjectInstructions {
    /// Workspace-relative path, which is also the block's source label.
    pub source: String,
    pub text: String,
    /// Size of the file on disk, before any budget truncation.
    pub bytes: usize,
    pub truncated: bool,
}

/// Read every non-ignored `AGENTS.md`, root first then shallower before deeper.
/// An empty result means there is nothing to load.
pub(crate) fn load(root: &Path) -> Result<Vec<ProjectInstructions>> {
    let mut loaded = Vec::new();
    let mut used = 0_usize;
    for relative in discover(root) {
        let remaining = MAX_TOTAL_BYTES.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        let resolved = match crate::workspace::resolve_tool_path(root, &relative) {
            Ok(path) => path,
            Err(error) if is_missing(&error) => continue,
            Err(error) => return Err(error),
        };
        let mut file = match crate::tool_file::ToolFile::open_resolved(root, resolved, false, false)
        {
            Ok(file) => file,
            Err(error) if is_missing(&error) => continue,
            Err(error) => return Err(error),
        };
        let raw = file.read()?;
        let (text, truncated) = budget(&raw, remaining);
        used += text.len();
        loaded.push(ProjectInstructions {
            source: relative,
            bytes: raw.len(),
            text,
            truncated,
        });
        if truncated {
            break;
        }
    }
    Ok(loaded)
}

/// Discover instruction files with the same ignore rules as search and the
/// automatic context, so `.git`/`target`/audit directories never contribute.
fn discover(root: &Path) -> Vec<String> {
    let suffix = format!("/{FILE}");
    let mut found: Vec<String> = crate::engine::workspace_files(root, false, usize::MAX)
        .into_iter()
        .filter(|path| path == FILE || path.ends_with(&suffix))
        .collect();
    // Root first, then by directory depth, then lexically for determinism.
    found.sort_by(|a, b| {
        (a.matches('/').count(), a.as_str()).cmp(&(b.matches('/').count(), b.as_str()))
    });
    found.truncate(MAX_FILES);
    found
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

    fn sources(loaded: &[ProjectInstructions]) -> Vec<&str> {
        loaded.iter().map(|item| item.source.as_str()).collect()
    }

    #[test]
    fn absent_files_load_nothing() {
        let root = tempdir().unwrap();
        assert_eq!(load(root.path()).unwrap(), Vec::new());
    }

    #[test]
    fn present_file_is_returned_verbatim() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(FILE), "Always run `cargo test`.\n").unwrap();
        let loaded = load(root.path()).unwrap();
        assert_eq!(sources(&loaded), ["AGENTS.md"]);
        assert_eq!(loaded[0].text, "Always run `cargo test`.\n");
        assert_eq!(loaded[0].bytes, 25);
        assert!(!loaded[0].truncated);
    }

    #[test]
    fn nested_files_are_ordered_root_first_then_by_depth() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(FILE), "root\n").unwrap();
        fs::create_dir_all(root.path().join("src/deep")).unwrap();
        fs::write(root.path().join("src").join(FILE), "src\n").unwrap();
        fs::write(root.path().join("src/deep").join(FILE), "deep\n").unwrap();
        // An ignored directory must not contribute.
        fs::create_dir_all(root.path().join("target")).unwrap();
        fs::write(root.path().join("target").join(FILE), "ignored\n").unwrap();
        let loaded = load(root.path()).unwrap();
        assert_eq!(
            sources(&loaded),
            ["AGENTS.md", "src/AGENTS.md", "src/deep/AGENTS.md"]
        );
    }

    #[test]
    fn the_total_budget_is_shared_across_files() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join(FILE), "a".repeat(MAX_TOTAL_BYTES)).unwrap();
        fs::write(root.path().join("sub").join(FILE), "later\n").unwrap();
        let loaded = load(root.path()).unwrap();
        // The root file consumes the whole budget, so the nested one is skipped.
        assert_eq!(sources(&loaded), ["AGENTS.md"]);
        assert!(!loaded[0].truncated);
        let root = tempdir().unwrap();
        fs::write(root.path().join(FILE), "a".repeat(MAX_TOTAL_BYTES - 8)).unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub").join(FILE), "b".repeat(100)).unwrap();
        let loaded = load(root.path()).unwrap();
        assert_eq!(sources(&loaded), ["AGENTS.md", "sub/AGENTS.md"]);
        assert!(loaded[1].truncated);
        assert!(loaded[1].text.contains("[truncated to"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_instruction_file_is_not_followed() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join(FILE), "SECRET").unwrap();
        symlink(outside.path().join(FILE), root.path().join(FILE)).unwrap();
        // Discovery reports the entry as a symlink, so it is skipped: the
        // external bytes are never read and no block is produced.
        assert_eq!(load(root.path()).unwrap(), Vec::new());
        assert_eq!(
            fs::read_to_string(outside.path().join(FILE)).unwrap(),
            "SECRET"
        );
    }
}
