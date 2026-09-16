//! Contract tests for the coherent cwd descriptor/path/provenance snapshot.
//!
//! These tests deliberately exercise the `WorkingDirectory` owner directly:
//! the descriptor and its guest name must describe the same directory even
//! while other threads publish new cwd values.  The facade must never change
//! the host process cwd as a side effect of either operation.

use super::*;
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "darwin-fs-cwd-snapshot-{}-{nonce}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create cwd snapshot fixture");
        fs::create_dir(path.join("a")).expect("create fixture a");
        fs::create_dir(path.join("b")).expect("create fixture b");
        Self(path)
    }

    fn facade(&self) -> Facade {
        Facade::new(File::open(&self.0).expect("open fixture root"), b"/", b"/")
            .expect("construct facade")
    }

    fn host_path(&self, guest_path: &[u8]) -> PathBuf {
        assert!(guest_path.starts_with(b"/"));
        self.0.join(Path::new(
            std::str::from_utf8(&guest_path[1..]).expect("fixture path is UTF-8"),
        ))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove cwd snapshot fixture");
    }
}

#[test]
fn named_lease_keeps_path_fd_and_origin_coherent_during_concurrent_chdir() {
    let fixture = Fixture::new();
    let facade = Arc::new(fixture.facade());
    let host_cwd = std::env::current_dir().expect("read host cwd");
    let initial = facade
        .namespace
        .cwd
        .named_lease()
        .expect("initial cwd lease");
    assert_eq!(initial.path, b"/");
    let root_origin = initial.origin.clone();
    assert!(
        root_origin.is_some(),
        "guest-root cwd must carry mount origin"
    );

    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for path in [b"/a".as_slice(), b"/b".as_slice()] {
        let facade = Arc::clone(&facade);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..256 {
                assert_eq!(facade.chdir(path), 0);
            }
        }));
    }
    barrier.wait();

    for _ in 0..512 {
        let snapshot = facade
            .namespace
            .cwd
            .named_lease()
            .expect("concurrent cwd lease");
        assert!(
            snapshot.path == b"/" || snapshot.path == b"/a" || snapshot.path == b"/b",
            "unexpected guest cwd path: {:?}",
            snapshot.path
        );
        let expected_inode = fs::metadata(fixture.host_path(&snapshot.path))
            .expect("metadata for snapshot path")
            .ino();
        assert_eq!(
            snapshot
                .directory
                .metadata()
                .expect("snapshot fd metadata")
                .ino(),
            expected_inode
        );
        assert_eq!(snapshot.origin.as_ref(), root_origin.as_ref());
    }

    for worker in workers {
        worker.join().expect("chdir worker");
    }
    assert_eq!(
        std::env::current_dir().expect("read host cwd after leases"),
        host_cwd
    );
}

#[test]
fn named_lease_retains_old_directory_after_subsequent_chdir() {
    let fixture = Fixture::new();
    let facade = fixture.facade();
    let host_cwd = std::env::current_dir().expect("read host cwd");

    assert_eq!(facade.chdir(b"/a"), 0);
    let snapshot = facade.namespace.cwd.named_lease().expect("lease /a");
    let root_origin = snapshot.origin.clone();
    assert_eq!(snapshot.path, b"/a");
    assert!(
        root_origin.is_some(),
        "named guest cwd must carry mount origin"
    );
    let a_inode = fs::metadata(fixture.host_path(b"/a"))
        .expect("metadata for a")
        .ino();
    assert_eq!(
        snapshot
            .directory
            .metadata()
            .expect("metadata for /a fd")
            .ino(),
        a_inode
    );

    assert_eq!(facade.chdir(b"/b"), 0);
    let current = facade.namespace.cwd.named_lease().expect("lease /b");
    assert_eq!(current.path, b"/b");
    assert_eq!(current.origin.as_ref(), root_origin.as_ref());
    assert_eq!(
        current
            .directory
            .metadata()
            .expect("metadata for /b fd")
            .ino(),
        fs::metadata(fixture.host_path(b"/b"))
            .expect("metadata for b")
            .ino()
    );

    // The first lease owns an independent descriptor and remains anchored to
    // /a after the WorkingDirectory publishes /b.
    assert_eq!(snapshot.path, b"/a");
    assert_eq!(
        snapshot
            .directory
            .metadata()
            .expect("retained /a fd metadata")
            .ino(),
        a_inode
    );
    assert_eq!(
        std::env::current_dir().expect("read host cwd after chdir"),
        host_cwd
    );
}

#[test]
fn immutable_open_uses_the_same_snapshot_as_mount_selection() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("a/value"), b"first").unwrap();
    fs::write(fixture.0.join("b/value"), b"second").unwrap();
    let facade = fixture.facade();
    assert_eq!(facade.chdir(b"/a"), 0);
    let snapshot = facade.namespace.cwd.named_lease().unwrap();
    let resolution = facade.resolve_from(&snapshot.path, b"value").unwrap();
    // Simulate a concurrent publication between admission and the actual open.
    assert_eq!(facade.chdir(b"/b"), 0);
    let fd = facade.open_immutable(b"value", resolution, crate::O_RDONLY, &snapshot);
    assert!(fd >= 0);
    let mut bytes = [0u8; 6];
    assert_eq!(
        unsafe { facade.read(fd, bytes.as_mut_ptr().cast(), bytes.len()) },
        5
    );
    assert_eq!(&bytes[..5], b"first");
    assert_eq!(facade.close(fd), 0);
}
