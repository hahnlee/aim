//! Writable-mount provenance tests.
//!
//! A guest-prefix string is not sufficient authority: a nested resolver mount
//! can have a canonical path below the writable prefix while belonging to a
//! different retained mount. Same-mount links remain valid.

use super::*;
use darwin_art_fs_broker::guest_path::GuestRoot;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "darwin-writable-origin-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create writable-origin fixture");
        fs::create_dir(path.join("root")).expect("create fixture guest root");
        fs::create_dir(path.join("storage")).expect("create fixture storage");
        fs::create_dir(path.join("nested")).expect("create fixture nested mount");
        Self(path)
    }

    fn guest(&self, nested: bool) -> Arc<GuestRoot> {
        let mut guest = GuestRoot::from_directory(File::open(self.0.join("root")).unwrap())
            .expect("open fixture guest root");
        guest
            .mount_directory(b"/storage", File::open(self.0.join("storage")).unwrap())
            .expect("mount fixture storage");
        if nested {
            guest
                .mount_directory(
                    b"/storage/nested",
                    File::open(self.0.join("nested")).unwrap(),
                )
                .expect("mount fixture nested tree");
        }
        Arc::new(guest)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nested_mount_below_writable_prefix_is_rejected() {
    use crate::{O_DIRECTORY, O_RDONLY};

    let fixture = Fixture::new();
    // Keep a distinct directory at the same host-relative name. Without the
    // final-origin check, openat would incorrectly reach this one instead of
    // the resolver-selected nested mount root.
    fs::create_dir(fixture.0.join("storage/nested")).unwrap();
    fs::write(fixture.0.join("nested/value"), b"nested").unwrap();
    let guest = fixture.guest(true);
    let mut storage = WritableMount::open(fixture.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(guest.clone()).unwrap();

    let error = storage
        .open_file(b"nested", O_RDONLY | O_DIRECTORY, 0)
        .expect_err("nested mount must not inherit parent write authority");
    assert_eq!(error.raw_os_error(), Some(libc::EROFS));
    assert_eq!(
        storage
            .mkdir(b"nested/new", 0o700)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
    let parent = guest.open(b"/storage/nested").unwrap().node.into_file();
    assert_eq!(
        storage
            .mkdir_at(&parent, &CString::new("new").unwrap(), 0o700)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EACCES)
    );
    assert!(!fixture.0.join("nested/new").exists());
    assert!(!fixture.0.join("storage/nested/new").exists());
}

#[test]
fn same_mount_absolute_symlink_remains_writable() {
    use crate::O_RDONLY;

    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("storage/inner")).unwrap();
    fs::write(fixture.0.join("storage/inner/value"), b"same-mount").unwrap();
    symlink("/storage/inner", fixture.0.join("storage/alias")).unwrap();
    let guest = fixture.guest(false);
    let mut storage = WritableMount::open(fixture.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(guest).unwrap();

    let mut value = Vec::new();
    storage
        .open_file(b"alias/value", O_RDONLY, 0)
        .expect("same-mount link should retain write authority")
        .read_to_end(&mut value)
        .unwrap();
    assert_eq!(value, b"same-mount");
    use std::io::Write;
    storage
        .open_file(b"alias/value", crate::O_WRONLY, 0)
        .unwrap()
        .write_all(b"updated!!!")
        .unwrap();
    assert_eq!(
        fs::read(fixture.0.join("storage/inner/value")).unwrap(),
        b"updated!!!"
    );
}

#[test]
fn mount_check_does_not_require_leaf_read_permission_or_follow_exclusive_link() {
    use crate::{O_CREAT, O_EXCL, O_WRONLY};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let leaf = fixture.0.join("storage/write-only");
    fs::write(&leaf, b"old").unwrap();
    fs::set_permissions(&leaf, fs::Permissions::from_mode(0o200)).unwrap();
    symlink("/missing/outside", fixture.0.join("storage/exclusive-link")).unwrap();
    let mut storage = WritableMount::open(fixture.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(fixture.guest(false)).unwrap();
    storage
        .open_file(b"write-only", O_WRONLY, 0)
        .unwrap()
        .write_all(b"new")
        .unwrap();
    let error = storage
        .open_file(b"exclusive-link", O_WRONLY | O_CREAT | O_EXCL, 0o600)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EEXIST));
    assert!(
        storage
            .open_file(b"new-file", O_WRONLY | O_CREAT | O_EXCL, 0o600)
            .is_ok()
    );
}
