#![cfg(target_os = "linux")]
use harness::{Checkpoint, restore_checkpoint, workspace::create_checkpoint};
use std::{
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
    },
};
use tempfile::{tempdir, tempdir_in};

#[test]
fn standalone_checkpoint_round_trips_binary_modes_and_relative_roots() {
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
    assert_eq!(fs::read_dir(snapshots.path()).unwrap().count(), 2);
}

#[test]
fn missing_source_does_not_create_source_parents_and_undo_is_idempotent() {
    let root = tempdir().unwrap();
    let snapshots = tempdir().unwrap();
    let cp = create_checkpoint(root.path(), snapshots.path(), "parent/new/file").unwrap();
    assert!(!cp.existed);
    assert!(fs::read(&cp.snapshot_path).unwrap().is_empty());
    assert!(!root.path().join("parent").exists());
    restore_checkpoint(root.path(), &cp).unwrap();
    fs::create_dir_all(root.path().join("parent/new")).unwrap();
    fs::write(root.path().join("parent/new/file"), "created").unwrap();
    restore_checkpoint(root.path(), &cp).unwrap();
    restore_checkpoint(root.path(), &cp).unwrap();
    assert!(!root.path().join("parent/new/file").exists());
}

#[test]
fn unsafe_and_special_sources_fail_before_creating_output() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret"), "SECRET").unwrap();
    symlink(outside.path().join("secret"), root.path().join("escape")).unwrap();
    symlink("missing", root.path().join("dangling")).unwrap();
    fs::create_dir(root.path().join("directory")).unwrap();
    let fifo = std::ffi::CString::new(root.path().join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let mut targets = vec!["escape", "dangling", "directory", "fifo", "../escape"];
    if unsafe { libc::geteuid() } != 0 {
        fs::write(root.path().join("unreadable"), "PRIVATE").unwrap();
        fs::set_permissions(
            root.path().join("unreadable"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        targets.push("unreadable");
    }
    for target in targets {
        let dest = outside
            .path()
            .join(format!("output-{}", target.replace('/', "-")));
        assert!(
            create_checkpoint(root.path(), &dest, target).is_err(),
            "{target}"
        );
        assert!(!dest.exists(), "{target}");
    }
    assert_eq!(
        fs::read_to_string(outside.path().join("secret")).unwrap(),
        "SECRET"
    );
}

#[test]
fn cross_mount_checkpoint_storage_preserves_snapshot_and_restore() {
    use std::os::unix::fs::MetadataExt;
    let root = tempdir().unwrap();
    let Ok(snapshots) = tempdir_in("/dev/shm") else {
        return;
    };
    if fs::metadata(root.path()).unwrap().dev() == fs::metadata(snapshots.path()).unwrap().dev() {
        return;
    }
    fs::write(root.path().join("file"), "original").unwrap();
    let cp = create_checkpoint(root.path(), snapshots.path(), "file").unwrap();
    fs::write(root.path().join("file"), "changed").unwrap();
    restore_checkpoint(root.path(), &cp).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("file")).unwrap(),
        "original"
    );
}
