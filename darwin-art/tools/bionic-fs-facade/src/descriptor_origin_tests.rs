//! Descriptor-table coverage for resolver-issued mount provenance.
//!
//! The table only retains a resolver token supplied by an eventual open
//! caller.  `None` remains the default for host-adopted, synthetic, and all
//! legacy insertions until those callers are explicitly wired.

use super::DescriptorTable;
use crate::{Descriptor, RandomDeviceKind};
use darwin_art_fs_broker::guest_path::GuestRoot;
use std::fs;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos();
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "darwin-fs-descriptor-origin-{}-{nonce}-{serial}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("create descriptor-origin fixture");
        fs::create_dir(path.join("root")).expect("create fixture root");
        fs::create_dir(path.join("data")).expect("create fixture data");
        fs::write(path.join("data/value"), b"resolver-issued").expect("write fixture value");
        Self(path)
    }

    fn guest_root(&self) -> GuestRoot {
        let mut guest = GuestRoot::from_directory(File::open(self.0.join("root")).unwrap())
            .expect("open fixture root");
        guest
            .mount_directory(b"/data", File::open(self.0.join("data")).unwrap())
            .expect("mount fixture data");
        guest
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resolver_origin_survives_take_and_restore() {
    let fixture = Fixture::new();
    let guest = fixture.guest_root();
    let resolved = guest.open(b"/data/value").expect("resolve fixture value");
    let origin = resolved.origin().clone();
    let descriptor = Descriptor::File(resolved.node.into_file());

    let mut table = DescriptorTable::default();
    let fd = table
        .insert_with_origin(descriptor, false, Some(origin.clone()))
        .expect("insert resolver-issued descriptor");
    assert_eq!(table.origin(fd), Some(&origin));

    let descriptor = table.take(fd).expect("take descriptor lease");
    // `take` is a temporary ownership move (sendfile uses this path), so the
    // origin must remain associated with the still-live guest FD number.
    assert_eq!(table.origin(fd), Some(&origin));
    table.restore(fd, descriptor);
    assert_eq!(table.origin(fd), Some(&origin));
}

#[test]
fn close_then_reuse_does_not_inherit_previous_origin() {
    let fixture = Fixture::new();
    let guest = fixture.guest_root();
    let resolved = guest.open(b"/data/value").expect("resolve fixture value");
    let origin = resolved.origin().clone();
    let descriptor = Descriptor::File(resolved.node.into_file());

    let mut table = DescriptorTable::default();
    let fd = table
        .insert_with_origin(descriptor, false, Some(origin))
        .expect("insert resolver-issued descriptor");
    drop(
        table
            .close_entry(fd)
            .expect("close resolver-issued descriptor"),
    );
    assert_eq!(table.origin(fd), None);

    // A legacy/default insertion into the recycled slot must stay unknown;
    // source metadata authority may not leak across FD-number reuse.
    let reused = table
        .insert(Descriptor::Random(RandomDeviceKind::Random))
        .expect("reuse descriptor number");
    assert_eq!(reused, fd);
    assert_eq!(table.origin(reused), None);
}

#[test]
fn default_insertions_are_unknown_until_explicitly_annotated() {
    let mut table = DescriptorTable::default();
    let fd = table
        .insert(Descriptor::Random(RandomDeviceKind::Urandom))
        .expect("insert unclassified descriptor");
    assert_eq!(table.origin(fd), None);
}

#[test]
fn production_open_dup_openat_and_cwd_keep_the_same_resolver_identity() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("root/dir")).unwrap();
    fs::write(fixture.0.join("root/dir/value"), b"value").unwrap();
    let facade =
        crate::Facade::new(File::open(fixture.0.join("root")).unwrap(), b"/", b"/").unwrap();
    let (_, initial_origin) = facade.namespace.cwd.lease().unwrap();
    let expected = initial_origin.expect("guest-root cwd has resolver identity");
    let directory = facade.open(b"/dir", 0);
    assert!(directory >= 0);
    let duplicate = facade.fcntl(directory, 0, 0);
    assert!(duplicate >= 0);
    let file = facade.openat_with_mode(duplicate, b"value", 0, 0);
    assert!(file >= 0);
    for fd in [directory, duplicate, file] {
        assert_eq!(
            facade.descriptors.lock().unwrap().origin(fd),
            Some(&expected)
        );
    }
    assert_eq!(facade.fchdir(duplicate), 0);
    assert_eq!(
        facade.namespace.cwd.lease().unwrap().1,
        Some(expected.clone())
    );
    let relative = facade.open(b"value", 0);
    assert!(relative >= 0);
    assert_eq!(
        facade.descriptors.lock().unwrap().origin(relative),
        Some(&expected)
    );
    assert_eq!(facade.chdir(b".."), 0);
    assert_eq!(facade.namespace.cwd.lease().unwrap().1, Some(expected));
    for fd in [directory, duplicate, file, relative] {
        assert_eq!(facade.close(fd), 0);
    }
}
