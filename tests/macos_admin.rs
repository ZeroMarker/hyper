#![cfg(target_os = "macos")]

use harness::{Checkpoint, restore_checkpoint, workspace::create_checkpoint};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
use tempfile::{tempdir, tempdir_in};

/// The POSIX administrative path round-trips binary content, permission bits and
/// relative roots, and publishes a discoverable 0600 manifest.
#[test]
fn macos_checkpoint_round_trips_binary_modes_and_relative_roots() {
    let root = tempdir_in(".").unwrap();
    let snapshots = tempdir_in(".").unwrap();
    let original = [0, 255, 10, 42];
    fs::write(root.path().join("file"), original).unwrap();
    fs::set_permissions(root.path().join("file"), fs::Permissions::from_mode(0o751)).unwrap();
    symlink("file", root.path().join("alias")).unwrap();
    let cp = create_checkpoint(root.path(), snapshots.path(), "alias").unwrap();
    assert!(cp.existed);
    assert_eq!(cp.target_path, "file");
    assert!(cp.snapshot_path.is_absolute());
    assert_eq!(fs::read(&cp.snapshot_path).unwrap(), original);
    let manifest = snapshots.path().join(format!("{}.json", cp.id));
    let saved: Checkpoint = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(saved.snapshot_path, cp.snapshot_path);
    assert_eq!(
        fs::metadata(manifest).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(root.path().join("file"), b"changed").unwrap();
    restore_checkpoint(root.path(), &saved).unwrap();
    assert_eq!(fs::read(root.path().join("file")).unwrap(), original);
    assert_eq!(
        fs::metadata(root.path().join("file"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
}

/// A link substituted after resolution cannot redirect a snapshot read or a
/// restore commit, and a missing source stays missing.
#[test]
fn macos_checkpoint_and_restore_reject_substituted_links() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let snapshots = tempdir().unwrap();
    fs::write(outside.path().join("secret"), "SECRET").unwrap();
    // A missing source is recorded as absent and undo is idempotent.
    let cp = create_checkpoint(root.path(), snapshots.path(), "parent/new/file").unwrap();
    assert!(!cp.existed);
    restore_checkpoint(root.path(), &cp).unwrap();
    assert!(!root.path().join("parent").exists());
    // Replacing the source with an external link is refused, not followed.
    fs::write(root.path().join("file"), "original").unwrap();
    fs::remove_file(root.path().join("file")).unwrap();
    symlink(outside.path().join("secret"), root.path().join("file")).unwrap();
    assert!(create_checkpoint(root.path(), snapshots.path(), "file").is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("secret")).unwrap(),
        "SECRET"
    );
}
