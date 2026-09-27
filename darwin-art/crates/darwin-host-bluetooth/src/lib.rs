//! Host-call module [`darwin_hostcall::module::BLUETOOTH`]: a virtual HCI
//! controller over CoreBluetooth for the guest `android.hardware.bluetooth`
//! service (`docs/bluetooth.md`).
//!
//! The original Android Bluetooth stack drives it with HCI as it would a
//! chip. [`controller`] answers as an LE-only controller, [`att`] plays each
//! connected device's ATT server over its GATT tree, and [`corebluetooth`]
//! is the radio. Controller-to-host packets wait in a queue; a pipe the
//! guest polls says when it is not empty.

mod adv;
mod att;
mod backend;
mod controller;
#[cfg(target_os = "macos")]
mod corebluetooth;
mod gatt;
mod hci;
mod l2cap;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use darwin_hostcall::bluetooth::{
    FN_CLOSE, FN_OPEN, FN_RECV, FN_SEND, MAX_PACKET, Packet, VERSION, kind,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub use adv::Addr;
pub use backend::{Backend, Event};
pub use controller::Controller;

pub static MODULE: HostModule = HostModule {
    id: module::BLUETOOTH,
    name: "bluetooth",
    version: VERSION,
    call,
};

fn neg(e: i32) -> i64 {
    -(e as i64)
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_OPEN => open(),
        FN_CLOSE => {
            close();
            0
        }
        FN_SEND | FN_RECV => {
            // SAFETY: the registry passes the guest's argument block.
            let p = match unsafe { args_mut::<Packet>(args, len) } {
                Ok(p) => p,
                Err(e) => return e,
            };
            if p.data == 0 {
                return neg(errno::EINVAL);
            }
            if func == FN_SEND {
                if p.len as usize > MAX_PACKET {
                    return neg(errno::EINVAL);
                }
                // SAFETY: the guest's buffer of `len` bytes, for this call.
                let data =
                    unsafe { std::slice::from_raw_parts(p.data as *const u8, p.len as usize) };
                send(p.kind, data)
            } else {
                // SAFETY: the guest's buffer of `capacity` bytes.
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(p.data as *mut u8, p.capacity as usize)
                };
                match recv(buf) {
                    Ok((k, n)) => {
                        p.kind = k;
                        p.len = n as u32;
                        0
                    }
                    Err(e) => neg(e),
                }
            }
        }
        _ => neg(errno::ENOSYS),
    }
}

struct Session {
    controller: Controller,
    queue: VecDeque<(u32, Vec<u8>)>,
    /// The pipe's write end: one byte in the pipe while `queue` is not
    /// empty.
    wake: i32,
    /// Our own copy of the read end, the guest's being the guest's.
    wake_read: i32,
    generation: u64,
}

impl Session {
    /// Queue what the controller produced.
    fn flush(&mut self) {
        let was_empty = self.queue.is_empty();
        self.queue.extend(self.controller.drain());
        if was_empty && !self.queue.is_empty() {
            // SAFETY: a byte into our non-blocking pipe; a full pipe
            // already wakes the reader.
            unsafe { libc::write(self.wake, [1u8].as_ptr().cast(), 1) };
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: our fds; the guest owns its read end.
        unsafe {
            libc::close(self.wake);
            libc::close(self.wake_read);
        }
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Run `f` on the open session of `generation`.
fn with_session(generation: u64, f: impl FnOnce(&mut Session)) {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = s.as_mut().filter(|s| s.generation == generation) {
        f(s);
        s.flush();
    }
}

/// Where host-call fds the guest must not see go: high up, as the syscall
/// layer keeps its own.
fn move_high(fd: i32) -> i32 {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain getrlimit into a local.
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };
    let base = (lim.rlim_cur.min(1 << 20) * 3 / 4).max(64) as i32;
    // SAFETY: duplicating and closing our own fd.
    unsafe {
        let high = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, base - 1);
        if high < 0 {
            return fd;
        }
        libc::close(fd);
        high
    }
}

fn open() -> i64 {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return neg(errno::EBUSY);
    }
    let mut fds = [0i32; 2];
    // SAFETY: a pipe into a local array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return neg(errno::ENODEV);
    }
    for fd in fds {
        // SAFETY: flags on our new fds.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
        }
    }
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let Some(backend) = new_backend(generation) else {
        // SAFETY: closing the fds just made.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        return neg(errno::ENODEV);
    };
    *slot = Some(Session {
        controller: Controller::new(backend, local_address()),
        queue: VecDeque::new(),
        wake: move_high(fds[1]),
        // SAFETY: duplicating the fd just made.
        wake_read: move_high(unsafe { libc::dup(fds[0]) }),
        generation,
    });
    fds[0] as i64
}

#[cfg(target_os = "macos")]
fn new_backend(generation: u64) -> Option<Box<dyn Backend>> {
    let sink = move |e: Event| with_session(generation, |s| s.controller.on_event(e));
    corebluetooth::CoreBluetooth::new(Box::new(sink)).map(|b| Box::new(b) as Box<dyn Backend>)
}

#[cfg(not(target_os = "macos"))]
fn new_backend(_generation: u64) -> Option<Box<dyn Backend>> {
    None
}

fn close() {
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut s) = session {
        s.controller.reset();
    }
}

fn send(k: u32, data: &[u8]) -> i64 {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = slot.as_mut() else {
        return neg(errno::ENODEV);
    };
    match k {
        kind::COMMAND => s.controller.command(data),
        kind::ACL => s.controller.acl(data),
        // No synchronous or isochronous link is ever set up.
        kind::SCO | kind::ISO => {}
        _ => return neg(errno::EINVAL),
    }
    s.flush();
    0
}

fn recv(buf: &mut [u8]) -> Result<(u32, usize), i32> {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let s = slot.as_mut().ok_or(errno::ENODEV)?;
    let Some((k, packet)) = s.queue.front() else {
        return Err(errno::EAGAIN);
    };
    if packet.len() > buf.len() {
        return Err(errno::EMSGSIZE);
    }
    let (k, n) = (*k, packet.len());
    buf[..n].copy_from_slice(packet);
    s.queue.pop_front();
    if s.queue.is_empty() {
        let mut sink = [0u8; 64];
        // SAFETY: draining our non-blocking pipe into a local buffer.
        while unsafe { libc::read(s.wake_read, sink.as_mut_ptr().cast(), sink.len()) } > 0 {}
    }
    Ok((k, n))
}

/// The controller's public address: derived from the Mac's hardware UUID,
/// so it is stable per Mac and never a real device's address.
fn local_address() -> Addr {
    let mut uuid = [0u8; 16];
    let timeout = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: a 16-byte buffer, as gethostuuid requires.
    unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) };
    Addr::derive(&uuid, false)
}

pub(crate) fn random(buf: &mut [u8]) {
    // SAFETY: fills our buffer.
    unsafe { libc::arc4random_buf(buf.as_mut_ptr().cast(), buf.len()) };
}
