//! Retained writable-mount contracts, independent of launcher or APK fixtures.
use super::writable_mount::WritableMount;
use darwin_art_fs_broker::guest_path::GuestRoot;
use std::ffi::CString;
use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "darwin-writable-mount-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        for name in ["root", "data", "storage", "other"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::create_dir(path.join("root/system")).unwrap();
        Self(path)
    }
    fn namespace(&self) -> Arc<GuestRoot> {
        let mut guest =
            GuestRoot::from_directory(File::open(self.0.join("root")).unwrap()).unwrap();
        for (prefix, host) in [
            (b"/data".as_slice(), "data"),
            (b"/storage", "storage"),
            (b"/storage-other", "other"),
        ] {
            guest
                .mount_directory(prefix, File::open(self.0.join(host)).unwrap())
                .unwrap();
        }
        Arc::new(guest)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn file_open_resolves_virtual_links_and_preserves_create_exclusive_semantics() {
    use crate::{O_APPEND, O_CREAT, O_EXCL, O_NOFOLLOW, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
    use std::io::{Read, Write};
    let f = Fixture::new();
    let mut storage = WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(f.namespace()).unwrap();
    storage
        .open_file(b"target", O_RDWR | O_CREAT | O_EXCL, 0o600)
        .unwrap()
        .write_all(b"original")
        .unwrap();
    symlink("/storage/target", f.0.join("storage/absolute")).unwrap();
    symlink("target", f.0.join("storage/relative")).unwrap();
    let mut value = String::new();
    storage
        .open_file(b"absolute", O_RDONLY, 0)
        .unwrap()
        .read_to_string(&mut value)
        .unwrap();
    assert_eq!(value, "original");
    storage
        .open_file(b"relative", O_WRONLY | O_APPEND, 0)
        .unwrap()
        .write_all(b"+")
        .unwrap();
    assert_eq!(fs::read(f.0.join("storage/target")).unwrap(), b"original+");
    assert_eq!(
        storage
            .open_file(b"absolute", O_WRONLY | O_CREAT | O_EXCL | O_TRUNC, 0o600)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EEXIST)
    );
    assert_eq!(fs::read(f.0.join("storage/target")).unwrap(), b"original+");
    assert_eq!(
        storage
            .open_file(b"relative", O_RDONLY | O_NOFOLLOW, 0)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP)
    );
    storage
        .open_file(b"absolute", O_WRONLY | O_TRUNC, 0)
        .unwrap();
    assert!(fs::read(f.0.join("storage/target")).unwrap().is_empty());
    symlink("/storage/created", f.0.join("storage/dangling")).unwrap();
    storage
        .open_file(b"dangling", O_WRONLY | O_CREAT, 0o600)
        .unwrap()
        .write_all(b"new")
        .unwrap();
    assert_eq!(fs::read(f.0.join("storage/created")).unwrap(), b"new");
}

#[test]
fn writable_file_open_keeps_link_parent_order_and_rejects_cross_mount_targets() {
    use crate::{O_CREAT, O_DIRECTORY, O_RDONLY, O_TRUNC, O_WRONLY};
    use std::io::Read;
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("storage/real/nested")).unwrap();
    fs::write(f.0.join("storage/real/value"), b"correct").unwrap();
    fs::write(f.0.join("storage/value"), b"wrong").unwrap();
    fs::write(f.0.join("root/system/value"), b"immutable").unwrap();
    symlink("/storage/real/nested", f.0.join("storage/alias")).unwrap();
    symlink("/system/value", f.0.join("storage/outside")).unwrap();
    symlink("loop", f.0.join("storage/loop")).unwrap();
    let mut storage = WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(f.namespace()).unwrap();
    let mut value = Vec::new();
    storage
        .open_file(b"alias/../value", O_RDONLY, 0)
        .unwrap()
        .read_to_end(&mut value)
        .unwrap();
    assert_eq!(value, b"correct");
    assert!(
        storage
            .open_file(b"alias/..", O_RDONLY | O_DIRECTORY, 0)
            .unwrap()
            .metadata()
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        storage
            .open_file(b"outside", O_WRONLY | O_TRUNC, 0)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
    assert_eq!(
        fs::read(f.0.join("root/system/value")).unwrap(),
        b"immutable"
    );
    assert_eq!(
        storage
            .open_file(b"loop", O_RDONLY, 0)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP)
    );
    assert_eq!(
        storage
            .open_file(b"missing", O_RDONLY, 0)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOENT)
    );
    assert_eq!(
        storage
            .open_file(b"value/", O_RDONLY, 0)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOTDIR)
    );
    assert_eq!(
        storage
            .open_file(b"../escape", O_WRONLY | O_CREAT, 0o600)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
}

#[test]
fn storage_mutations_use_the_configured_mount_after_host_rename() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("storage/emulated/0")).unwrap();
    symlink("/storage/emulated/0", f.0.join("storage/current")).unwrap();
    let mut storage = WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(f.namespace()).unwrap();
    fs::rename(f.0.join("storage"), f.0.join("retained-storage")).unwrap();
    fs::create_dir(f.0.join("storage")).unwrap();
    storage
        .seed_directory(b"current/Android/data/example/files")
        .unwrap();
    storage.mkdir(b"current/pending", 0o700).unwrap();
    storage
        .rename(b"current/pending", b"current/ready")
        .unwrap();
    assert!(f.0.join("retained-storage/emulated/0/ready").is_dir());
    assert!(
        f.0.join("retained-storage/emulated/0/Android/data/example/files")
            .is_dir()
    );
    assert_eq!(fs::read_dir(f.0.join("storage")).unwrap().count(), 0);
    storage.remove(b"current/ready").unwrap();
    assert!(!f.0.join("retained-storage/emulated/0/ready").exists());
    assert_eq!(fs::read_dir(f.0.join("data")).unwrap().count(), 0);
}

#[test]
fn mkdir_at_uses_retained_parent_fd_after_mount_path_rename() {
    let f = Fixture::new();
    let guest = f.namespace();
    let mut storage = WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(guest.clone()).unwrap();
    let parent = guest.open(b"/storage").unwrap().node.into_file();

    fs::rename(f.0.join("storage"), f.0.join("retained-storage")).unwrap();
    fs::create_dir(f.0.join("storage")).unwrap();
    storage
        .mkdir_at(&parent, &CString::new("created").unwrap(), 0o700)
        .unwrap();

    assert!(f.0.join("retained-storage/created").is_dir());
    assert!(!f.0.join("storage/created").exists());
    assert_eq!(
        storage
            .mkdir_at(&parent, &CString::new(".").unwrap(), 0o700)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EEXIST)
    );
    assert_eq!(
        storage
            .mkdir_at(&parent, &CString::new("..").unwrap(), 0o700)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EEXIST)
    );
}

#[test]
fn writable_mount_does_not_grant_writes_to_sibling_or_prefix_lookalikes() {
    let f = Fixture::new();
    for (name, target) in [
        ("data-link", "/data"),
        ("system-link", "/system"),
        ("lookalike", "/storage-other"),
    ] {
        symlink(target, f.0.join("storage").join(name)).unwrap();
    }
    let mut storage = WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    storage.attach_guest_root(f.namespace()).unwrap();
    for path in [
        b"data-link/created".as_slice(),
        b"system-link/created",
        b"lookalike/created",
    ] {
        assert_eq!(
            storage.mkdir(path, 0o700).unwrap_err().raw_os_error(),
            Some(libc::EROFS)
        );
    }
    assert_eq!(fs::read_dir(f.0.join("data")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(f.0.join("other")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(f.0.join("root/system")).unwrap().count(), 0);
    // Removing the link is an operation on storage, not on its target mount.
    storage.remove(b"data-link").unwrap();
    assert!(f.0.join("data").is_dir());
}

#[test]
fn invalid_prefix_and_mismatched_mount_authority_are_rejected() {
    let f = Fixture::new();
    for prefix in [
        b"".as_slice(),
        b"/",
        b"storage",
        b"/storage/",
        b"/storage//x",
        b"/storage/../data",
        b"/storage/./x",
        b"/storage\0",
    ] {
        assert!(WritableMount::open(f.0.join("storage"), prefix).is_err());
    }
    let mut mismatched = WritableMount::open(f.0.join("data"), b"/storage").unwrap();
    assert_eq!(
        mismatched
            .attach_guest_root(f.namespace())
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EACCES)
    );
}
