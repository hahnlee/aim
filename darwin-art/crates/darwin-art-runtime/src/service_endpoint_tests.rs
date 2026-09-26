use super::*;
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixStream;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/dar-end-{}-{stamp}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn socket(&self) -> PathBuf {
        self.0.join("service.sock")
    }
}

#[test]
fn unowned_listener_is_rejected_without_connecting_or_waiting() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let listener = UnixListener::bind(&path).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let start = std::time::Instant::now();
    let inode = fs::symlink_metadata(&path).unwrap().ino();
    // No connect-based liveness probing: a refused full backlog is not death.
    for _ in 0..128 {
        assert!(ServiceEndpoint::bind(&path).is_err());
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
    assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), inode);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn listener_owner_is_exclusive_and_removes_only_its_socket() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let endpoint = ServiceEndpoint::bind(&path).unwrap();
    assert!(endpoint.descriptor() >= 0);
    assert!(UnixStream::connect(&path).is_ok());
    assert!(ServiceEndpoint::bind(&path).is_err());
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
    drop(endpoint);
    assert!(!path.exists());
    let rebound = ServiceEndpoint::bind(&path);
    assert!(
        rebound.is_ok(),
        "immediate rebind failed: {:?}",
        rebound.as_ref().err()
    );
}

#[test]
fn owner_drop_releases_lock_even_with_inherited_descriptor() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let endpoint = ServiceEndpoint::bind(&path).unwrap();
    // Darwin dup/fork descriptors reference the SAME flock. A spawn can hold
    // such a copy until exec despite CLOEXEC, after the Rust owner has ended.
    let inherited = endpoint._lock.file.try_clone().unwrap();
    assert!(ServiceEndpoint::bind(&path).is_err());
    drop(endpoint);
    assert!(!path.exists());
    let replacement = ServiceEndpoint::bind(&path);
    assert!(
        replacement.is_ok(),
        "inherited descriptor prolonged owner lock: {:?}",
        replacement.as_ref().err()
    );
    // Closing the old duplicate must not unlock the new owner's independent
    // open-file description or remove its published socket.
    drop(inherited);
    assert!(path.exists());
    assert!(ServiceEndpoint::bind(&path).is_err());
    drop(replacement);
    assert!(!path.exists());
}

#[test]
fn different_process_copy_cannot_unpublish_or_unlock_owner() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let mut endpoint = ServiceEndpoint::bind(&path).unwrap();
    let inherited = endpoint._lock.file.try_clone().unwrap();
    // Exercise the PID ownership check without running Rust destruction in the
    // unsafe interval between a multithreaded fork and exec.
    endpoint._lock.owner_pid = std::process::id() ^ 1;
    drop(endpoint);
    assert!(path.exists(), "non-owner removed the published socket");
    let lock_path = path.with_file_name("service.sock.owner-lock");
    let independent = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    assert_ne!(
        unsafe { libc::flock(independent.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
    drop(inherited);
    // A child another test is spawning holds a copy of every descriptor
    // between fork and exec (#21); the lock frees once that exec happens.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while unsafe { libc::flock(independent.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
        assert!(
            std::time::Instant::now() < deadline,
            "the owner lock stayed held"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn unowned_stale_socket_is_not_removed() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let old = UnixListener::bind(&path).unwrap();
    let inode = fs::symlink_metadata(&path).unwrap().ino();
    assert!(ServiceEndpoint::bind(&path).is_err());
    assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), inode);
    drop(old);
    assert!(ServiceEndpoint::bind(&path).is_err());
    assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), inode);
}

#[test]
#[ignore = "executed in a child by recorded_dead_owner_socket_is_recovered"]
fn crash_child_fixture() {
    let path = std::env::var_os("DARWIN_ART_TEST_ENDPOINT_CHILD_PATH").unwrap();
    let _endpoint = ServiceEndpoint::bind(Path::new(&path)).unwrap();
    // Abrupt exit deliberately skips Drop, as with a crashed service.
    std::process::exit(0);
}

#[test]
fn recorded_dead_owner_socket_is_recovered() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "service_endpoint::tests::crash_child_fixture",
        ])
        .env("DARWIN_ART_TEST_ENDPOINT_CHILD_PATH", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
    let endpoint = ServiceEndpoint::bind(&path).unwrap();
    assert!(UnixStream::connect(&path).is_ok());
    drop(endpoint);
    assert!(!path.exists());
}

#[test]
fn foreign_leaf_and_replacement_are_preserved() {
    let fixture = Fixture::new();
    let path = fixture.socket();
    fs::write(&path, b"owned by someone else").unwrap();
    assert!(ServiceEndpoint::bind(&path).is_err());
    fs::remove_file(&path).unwrap();
    symlink("elsewhere", &path).unwrap();
    assert!(ServiceEndpoint::bind(&path).is_err());
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_file(&path).unwrap();
    let endpoint = ServiceEndpoint::bind(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    drop(endpoint);
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
}

#[test]
fn ffi_handles_reject_stale_close_and_preserve_borrowed_descriptor() {
    use crate::service_endpoint_ffi::{
        darwin_art_service_endpoint_close, darwin_art_service_endpoint_open,
    };
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let path = fixture.socket();
    let bytes = path.as_os_str().as_bytes();
    let (mut handle, mut descriptor) = (0, -1);
    assert_eq!(
        unsafe {
            darwin_art_service_endpoint_open(
                bytes.as_ptr(),
                bytes.len(),
                &mut handle,
                &mut descriptor,
            )
        },
        0
    );
    assert_ne!(handle, 0);
    assert!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) } >= 0);
    assert_eq!(darwin_art_service_endpoint_close(handle), 0);
    assert_eq!(darwin_art_service_endpoint_close(handle), -1);
    assert!(!path.exists());
}
