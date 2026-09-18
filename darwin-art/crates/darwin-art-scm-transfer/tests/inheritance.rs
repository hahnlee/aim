//! Native producer-window fixture; production creator adoption is tested separately.
use darwin_art_scm_transfer::inheritance::{guard, spawn_owned};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[cfg(target_os = "macos")]
#[test]
fn native_operation_preserves_result_errno_and_rejects_null_callback() {
    use darwin_art_scm_transfer::inheritance::with_native_operation;
    unsafe extern "C" fn open_descriptor(_: *mut libc::c_void) -> isize {
        let descriptor =
            unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        unsafe { *libc::__error() = libc::ERANGE };
        descriptor as isize
    }
    let descriptor = unsafe { with_native_operation(Some(open_descriptor), std::ptr::null_mut()) };
    assert!(descriptor >= 0);
    assert_eq!(unsafe { *libc::__error() }, libc::ERANGE);
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor as i32) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::FD_CLOEXEC, 0);
    assert_eq!(
        unsafe { with_native_operation(None, std::ptr::null_mut()) },
        -1
    );
    assert_eq!(unsafe { *libc::__error() }, libc::EINVAL);
}

#[test]
fn spawn_waits_for_cloexec_window_and_child_does_not_keep_writer_alive() {
    let protected = guard().unwrap();
    let mut raw = [-1; 2];
    assert_eq!(unsafe { libc::pipe(raw.as_mut_ptr()) }, 0);
    let reader = unsafe { OwnedFd::from_raw_fd(raw[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(raw[1]) };
    let (started_tx, started_rx) = mpsc::channel();
    let (spawned_tx, spawned_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut command = Command::new("/bin/sleep");
        command
            .arg("10")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        started_tx.send(()).unwrap();
        spawned_tx.send(spawn_owned(&mut command)).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        spawned_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    for fd in [&reader, &writer] {
        assert_eq!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    drop(protected);
    let mut child = spawned_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
    assert!(child.try_wait().unwrap().is_none());
    drop(writer);
    let mut poll = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll, 1, 1000) };
    let mut byte = 0u8;
    let result = if ready > 0 {
        unsafe { libc::read(reader.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) }
    } else {
        -1
    };
    let still_alive = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(result, 0, "only legitimate writer closure must produce EOF");
    assert!(still_alive, "EOF must precede child exit");
}
