//! Regression coverage for resolver-issued mount provenance.
//!
//! These tests deliberately inspect the origin attached to the opened lease,
//! rather than inferring it from the requested or canonical pathname.  The
//! host directories are only fixtures; the resolver's retained descriptors
//! remain the authority under rename/replacement.

use super::*;
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let serial = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "darwin-guest-origin-{label}-{}-{serial}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create origin fixture: {error}"),
            }
        }
        panic!("could not allocate origin fixture");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn resolver(root: &TempDir, data: Option<&TempDir>) -> GuestRoot {
    let mut guest = GuestRoot::from_directory(fs::File::open(root.path()).unwrap()).unwrap();
    if let Some(data) = data {
        guest
            .mount_directory(b"/data", fs::File::open(data.path()).unwrap())
            .unwrap();
    }
    guest
}

#[test]
fn absolute_symlink_crossing_reports_target_mount_origin() {
    let root = TempDir::new("absolute-crossing-root");
    let data = TempDir::new("absolute-crossing-data");
    fs::write(data.path().join("value"), b"mounted").unwrap();
    symlink("/data/value", root.path().join("link")).unwrap();

    let resolved = resolver(&root, Some(&data)).open(b"/link").unwrap();
    assert_eq!(resolved.canonical_path, b"/data/value");
    assert_eq!(resolved.origin().mount_path(), b"/data");
}

#[test]
fn relative_symlink_inside_mount_preserves_mount_origin() {
    let root = TempDir::new("relative-root");
    let data = TempDir::new("relative-data");
    fs::write(data.path().join("value"), b"mounted").unwrap();
    symlink("value", data.path().join("link")).unwrap();

    let resolved = resolver(&root, Some(&data)).open(b"/data/link").unwrap();
    assert_eq!(resolved.canonical_path, b"/data/value");
    assert_eq!(resolved.origin().mount_path(), b"/data");
}

#[test]
fn nofollow_final_link_keeps_the_link_containing_mount_origin() {
    let root = TempDir::new("nofollow-root");
    let data = TempDir::new("nofollow-data");
    fs::write(data.path().join("value"), b"mounted").unwrap();
    symlink("value", data.path().join("link")).unwrap();
    symlink("/data/value", root.path().join("root-link")).unwrap();

    let guest = resolver(&root, Some(&data));
    let mounted_link = guest.open_no_follow(b"/data/link").unwrap();
    assert!(mounted_link.node.metadata().file_type().is_symlink());
    assert_eq!(mounted_link.origin().mount_path(), b"/data");

    // A final absolute link is itself a root node when no-follow is requested;
    // its target must not change the retained origin to /data.
    let root_link = guest.open_no_follow(b"/root-link").unwrap();
    assert!(root_link.node.metadata().file_type().is_symlink());
    assert_eq!(root_link.origin().mount_path(), b"/");
}

#[test]
fn parent_from_mount_root_returns_to_root_origin() {
    let root = TempDir::new("parent-root");
    let data = TempDir::new("parent-data");
    fs::write(root.path().join("root-value"), b"root").unwrap();
    fs::write(data.path().join("value"), b"mounted").unwrap();

    let resolved = resolver(&root, Some(&data))
        .open(b"/data/../root-value")
        .unwrap();
    assert_eq!(resolved.canonical_path, b"/root-value");
    assert_eq!(resolved.origin().mount_path(), b"/");
}

#[test]
fn absolute_link_from_mount_resets_to_root_origin() {
    let root = TempDir::new("reset-root");
    let data = TempDir::new("reset-data");
    fs::write(root.path().join("root-value"), b"root").unwrap();
    symlink("/root-value", data.path().join("reset")).unwrap();

    let resolved = resolver(&root, Some(&data)).open(b"/data/reset").unwrap();
    assert_eq!(resolved.canonical_path, b"/root-value");
    assert_eq!(resolved.origin().mount_path(), b"/");
}

#[test]
fn nested_mount_and_parent_restore_their_respective_origins() {
    let root = TempDir::new("nested-root");
    let data = TempDir::new("nested-data");
    let obb = TempDir::new("nested-obb");
    fs::write(data.path().join("data-value"), b"data").unwrap();
    fs::write(obb.path().join("obb-value"), b"obb").unwrap();

    let mut guest = resolver(&root, Some(&data));
    guest
        .mount_directory(b"/data/obb", fs::File::open(obb.path()).unwrap())
        .unwrap();

    let nested = guest.open(b"/data/obb/obb-value").unwrap();
    assert_eq!(nested.origin().mount_path(), b"/data/obb");

    let parent = guest.open(b"/data/obb/../data-value").unwrap();
    assert_eq!(parent.canonical_path, b"/data/data-value");
    assert_eq!(parent.origin().mount_path(), b"/data");
}

#[test]
fn same_guest_path_in_distinct_guest_roots_has_distinct_origin_identity() {
    let first = TempDir::new("identity-first");
    let second = TempDir::new("identity-second");
    fs::write(first.path().join("value"), b"first").unwrap();
    fs::write(second.path().join("value"), b"second").unwrap();

    let first_origin = resolver(&first, None).open(b"/value").unwrap();
    let second_origin = resolver(&second, None).open(b"/value").unwrap();
    assert_eq!(first_origin.origin().mount_path(), b"/");
    assert_eq!(second_origin.origin().mount_path(), b"/");
    assert_ne!(first_origin.origin(), second_origin.origin());
}

#[test]
fn opened_descriptor_keeps_origin_when_host_leaf_is_renamed_and_replaced() {
    let root = TempDir::new("rename-root");
    fs::write(root.path().join("value"), b"original").unwrap();

    let guest = resolver(&root, None);
    let resolved = guest.open(b"/value").unwrap();
    assert_eq!(resolved.origin().mount_path(), b"/");

    fs::rename(root.path().join("value"), root.path().join("old")).unwrap();
    fs::write(root.path().join("value"), b"replacement").unwrap();
    let mut bytes = Vec::new();
    resolved.node.into_file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn mounted_descriptor_keeps_origin_when_mount_directory_is_replaced() {
    let root = TempDir::new("mount-replace-root");
    let data = TempDir::new("mount-replace-data");
    fs::write(data.path().join("value"), b"original").unwrap();
    let guest = resolver(&root, Some(&data));
    let resolved = guest.open(b"/data/value").unwrap();
    assert_eq!(resolved.origin().mount_path(), b"/data");
    let origin = resolved.origin().clone();

    let old_data = data.path().with_extension("old");
    fs::rename(data.path(), &old_data).unwrap();
    fs::create_dir(data.path()).unwrap();
    fs::write(data.path().join("value"), b"replacement").unwrap();

    let mut bytes = Vec::new();
    resolved.node.into_file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    assert_eq!(origin.mount_path(), b"/data");
    fs::remove_dir_all(old_data).unwrap();
}

#[test]
fn relative_api_follows_absolute_link_into_mount_and_returns_mount_origin() {
    let root = TempDir::new("relative-api-root");
    let data = TempDir::new("relative-api-data");
    fs::write(data.path().join("value"), b"mounted").unwrap();
    symlink("/data/value", root.path().join("link")).unwrap();

    let guest = resolver(&root, Some(&data));
    let directory = guest.open(b"/").unwrap();
    let (file, origin) = guest
        .open_relative_with_origin(directory.node.file(), directory.origin(), b"link", false)
        .unwrap();
    assert_eq!(origin.mount_path(), b"/data");
    let mut bytes = Vec::new();
    file.take(64).read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"mounted");
}

#[test]
fn relative_api_parent_from_mount_root_returns_root_origin() {
    let root = TempDir::new("relative-parent-root");
    let data = TempDir::new("relative-parent-data");
    fs::write(root.path().join("root-value"), b"root").unwrap();

    let guest = resolver(&root, Some(&data));
    let directory = guest.open(b"/data").unwrap();
    let (file, origin) = guest
        .open_relative_with_origin(
            directory.node.file(),
            directory.origin(),
            b"../root-value",
            false,
        )
        .unwrap();
    assert_eq!(origin.mount_path(), b"/");
    let mut bytes = Vec::new();
    file.take(64).read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"root");
}

#[test]
fn relative_api_rejects_a_foreign_guest_root_origin_token() {
    let first = TempDir::new("relative-foreign-first");
    let second = TempDir::new("relative-foreign-second");
    fs::write(second.path().join("value"), b"local").unwrap();

    let foreign_guest = resolver(&first, None);
    let local_guest = resolver(&second, None);
    let foreign = foreign_guest.open(b"/").unwrap();
    let local = local_guest.open(b"/").unwrap();
    let error = local_guest
        .open_relative_with_origin(local.node.file(), foreign.origin(), b"value", false)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(13));
}
