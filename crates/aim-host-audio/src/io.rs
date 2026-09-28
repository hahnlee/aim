//! The CoreAudio process: `linux-run --audio-io`, which the module starts
//! for the HAL process on first use (`docs/audio.md`, "Teardown").
//!
//! It owns every CoreAudio object of the HAL process: the device queries
//! and the AUHAL units. It runs in a process group of its own, so nothing
//! that stops the guest's services (a SIGKILL of the service's process
//! group, a terminal's SIGINT) reaches it. When the HAL process goes, however it
//! goes, its end of the connection and of the lifeline close; this process
//! then stops, uninitializes and disposes every unit, lets input setups in
//! flight finish, and exits. A SIGKILLed HAL can so never leave a unit
//! running or a CoreAudio client half made.

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use aim_hostcall::audio::{FN_CLOSE, FN_DEVICES, FN_OPEN, FN_START, FN_STOP};
use aim_hostcall::errno::{EBADF, EINVAL, ENOSYS};

use crate::stream::{self, Stream};
use crate::wire::{self, Reply, Request};

/// The descriptors the module passes: the connection and the lifeline (a
/// pipe's read end, whose writer only the HAL process holds).
pub const CONNECTION_FD: i32 = 3;
pub const LIFELINE_FD: i32 = 4;

/// How long teardown may take once the HAL process is gone, before this
/// process exits regardless (a CoreAudio call that never returns: the
/// daemon itself is stuck).
const TEARDOWN: Duration = Duration::from_secs(10);

/// The process's entry point (`linux-run --audio-io`).
pub fn main() -> ! {
    // SAFETY: plain signal dispositions, before any thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    // SAFETY: the descriptors the module passed us.
    let (conn, lifeline) = unsafe {
        use std::os::fd::FromRawFd;
        (
            OwnedFd::from_raw_fd(CONNECTION_FD),
            OwnedFd::from_raw_fd(LIFELINE_FD),
        )
    };
    let _ = std::thread::Builder::new()
        .name("audio-lifeline".into())
        .spawn(move || watch(lifeline, TEARDOWN));
    serve(conn.as_fd());
    // SAFETY: plain exit, after an orderly teardown.
    unsafe { libc::_exit(0) }
}

/// Wait for the HAL process to go, then give the main thread `grace` to
/// tear down before exiting.
fn watch(lifeline: OwnedFd, grace: Duration) {
    let mut byte = 0u8;
    // Nothing is ever written: the read ends at EOF, when the writer goes.
    // SAFETY: a blocking read into a local byte.
    while unsafe { libc::read(lifeline.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) } < 0
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    {}
    std::thread::sleep(grace);
    eprintln!("audio-io: teardown did not end in {grace:?}; exiting");
    // SAFETY: plain exit.
    unsafe { libc::_exit(1) }
}

/// Answer requests until the connection ends, then close every stream.
pub fn serve(conn: BorrowedFd) {
    let mut streams: HashMap<u64, Stream> = HashMap::new();
    let mut next = 1u64;
    while let Ok(Some((req, fd))) = wire::recv::<Request>(conn) {
        let mut reply = Reply::default();
        reply.status = match req.op {
            FN_DEVICES => {
                reply.devices = crate::devices();
                0
            }
            FN_OPEN => match fd {
                Some(fd) => {
                    let mut open = req.open;
                    open.fd = fd.as_raw_fd();
                    // The stream maps the ring; the descriptor can go.
                    match Stream::open(&open) {
                        Ok(s) => {
                            streams.insert(next, s);
                            reply.stream = next;
                            next += 1;
                            0
                        }
                        Err(e) => -(e as i64),
                    }
                }
                None => -(EBADF as i64),
            },
            FN_START | FN_STOP | FN_CLOSE => {
                let ok = match req.op {
                    FN_START => streams.get(&req.stream).map(Stream::start),
                    FN_STOP => streams.get(&req.stream).map(Stream::stop),
                    _ => streams.remove(&req.stream).map(|_| true),
                };
                if ok == Some(true) {
                    0
                } else {
                    -(EINVAL as i64)
                }
            }
            _ => -(ENOSYS as i64),
        };
        if wire::send(conn, &reply, None).is_err() {
            break;
        }
    }
    // Stop, uninitialize and dispose every unit, then let input setups in
    // flight finish (each drops its unit on seeing its stream closed).
    streams.clear();
    stream::wait_for_setups(TEARDOWN);
}
