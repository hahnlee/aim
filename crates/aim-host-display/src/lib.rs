//! Host-call module [`aim_hostcall::module::DISPLAY`]: the host side of
//! the composer HAL (`docs/composer.md`).
//!
//! The window lives in the display server, `aim-display` (this crate's
//! binary), not in the composer's process: AppKit needs the main thread,
//! which `linux-run` gives to the guest. The module connects to the server
//! named by `linux-run --display` and forwards the composer's requests
//! ([`wire`]). A buffer travels once, as its memfd over `SCM_RIGHTS`; the
//! server maps the same memory, so nothing is copied. A present carries its
//! fences the same way: the acquire fence the server waits for, and the
//! writer of the present fence it signals once the frame is on screen. The
//! guest gets the connection itself to read vsync events from.
//!
//! The guest's task bridge (`docs/windows.md`) gets a connection of its own
//! ([`FN_WINDOWS`]) and exchanges window records on it directly.

pub mod input;
pub mod media;
pub mod notify;
pub mod shell;
pub mod windows;
pub mod wire;

use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use aim_hostcall::display::{
    Buffer, Connect, Cursor, FN_CONNECT, FN_CURSOR, FN_IMPORT, FN_PRESENT, FN_RELEASE,
    FN_SET_VSYNC, FN_WINDOWS, Import, Present, SetVsync, VERSION, Windows,
};
use aim_hostcall::{HostModule, args_mut, errno, module};
use wire::Request;

pub static MODULE: HostModule = HostModule {
    id: module::DISPLAY,
    name: "display",
    version: VERSION,
    call,
};

static SERVER: OnceLock<PathBuf> = OnceLock::new();

/// The display server's socket (`linux-run --display SOCKET`). Without it
/// [`FN_CONNECT`] fails with `ENODEV`.
pub fn set_server(socket: &Path) {
    let _ = SERVER.set(socket.to_owned());
}

/// The module's end of the connection, for requests.
static CONNECTION: Mutex<Option<OwnedFd>> = Mutex::new(None);

fn neg(e: i32) -> i64 {
    -(e as i64)
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    // SAFETY (all arms): the registry passes the guest's argument block.
    let r = match func {
        FN_CONNECT => unsafe { args_mut::<Connect>(args, len) }.map(connect),
        FN_IMPORT => unsafe { args_mut::<Import>(args, len) }.map(|i| import(i)),
        FN_PRESENT => unsafe { args_mut::<Present>(args, len) }.map(present),
        FN_RELEASE => unsafe { args_mut::<Buffer>(args, len) }.map(|b| {
            request(
                &Request {
                    op: wire::OP_RELEASE,
                    id: b.id,
                    ..Default::default()
                },
                &[],
            )
        }),
        FN_SET_VSYNC => unsafe { args_mut::<SetVsync>(args, len) }.map(|v| {
            request(
                &Request {
                    op: wire::OP_SET_VSYNC,
                    flag: (v.enabled != 0) as u32,
                    ..Default::default()
                },
                &[],
            )
        }),
        FN_WINDOWS => unsafe { args_mut::<Windows>(args, len) }.map(windows),
        FN_CURSOR => unsafe { args_mut::<Cursor>(args, len) }.map(|c| cursor(c)),
        _ => Err(neg(errno::ENOSYS)),
    };
    r.unwrap_or_else(|e| e)
}

/// Connect to the server, send `hello` and read its answer.
fn open<T: Copy + Default>(hello: &Request) -> Result<(UnixStream, T), i64> {
    let path = SERVER.get().ok_or(neg(errno::ENODEV))?;
    let stream = UnixStream::connect(path).map_err(|_| neg(errno::ECONNREFUSED))?;
    // A write after the server has gone must fail, not raise SIGPIPE.
    let on: libc::c_int = 1;
    // SAFETY: setsockopt on our socket with a local int.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    // The server answers at once; a hung one must not hang the guest.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    match wire::send(stream.as_fd(), wire::bytes(hello), None)
        .and_then(|()| wire::recv_record::<T>(stream.as_fd()))
    {
        Ok(Some(answer)) => {
            let _ = stream.set_read_timeout(None);
            Ok((stream, answer))
        }
        _ => Err(neg(errno::ECONNREFUSED)),
    }
}

/// A guest fd for our end of `stream`; we keep the original.
fn give_to_guest(stream: &UnixStream) -> Result<i64, i64> {
    // SAFETY: fcntl on a fd we own; the guest owns the duplicate.
    let guest = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if guest < 0 {
        return Err(neg(errno::ENODEV));
    }
    Ok(guest as i64)
}

fn connect(out: &mut Connect) -> i64 {
    let hello = Request {
        op: wire::OP_HELLO,
        flag: out.display,
        id: wire::VERSION,
        ..Default::default()
    };
    let r = open::<Connect>(&hello).and_then(|(stream, info)| {
        let guest = give_to_guest(&stream)?;
        *out = Connect {
            display: out.display,
            ..info
        };
        *CONNECTION.lock().unwrap() = Some(stream.into());
        Ok(guest)
    });
    r.unwrap_or_else(|e| e)
}

/// The task bridge's connection: the guest reads and writes it directly,
/// so the module keeps nothing of it.
fn windows(out: &mut Windows) -> i64 {
    let hello = Request {
        op: wire::OP_WINDOWS,
        id: wire::VERSION,
        ..Default::default()
    };
    let r = open::<Windows>(&hello).and_then(|(stream, info)| {
        let guest = give_to_guest(&stream)?;
        *out = info;
        Ok(guest)
    });
    r.unwrap_or_else(|e| e)
}

fn import(i: &Import) -> i64 {
    // SAFETY: F_GETFD only checks that the guest's fd is open.
    if unsafe { libc::fcntl(i.fd, libc::F_GETFD) } < 0 {
        return neg(errno::EBADF);
    }
    if i.width == 0 || i.height == 0 || i.length == 0 {
        return neg(errno::EINVAL);
    }
    request(
        &Request {
            op: wire::OP_IMPORT,
            id: i.id,
            import: *i,
            ..Default::default()
        },
        &[i.fd],
    )
}

/// Send the present with the present fence's writer (and the acquire
/// fence); the guest gets the present fence.
fn present(p: &mut Present) -> i64 {
    // SAFETY: F_GETFD only checks that the guest's fd is open.
    if p.acquire >= 0 && unsafe { libc::fcntl(p.acquire, libc::F_GETFD) } < 0 {
        return neg(errno::EBADF);
    }
    let Ok((fence, writer)) = aim_sync_file::pair() else {
        return neg(errno::ENOMEM);
    };
    let mut fds = vec![writer.as_raw_fd()];
    let mut flag = 0;
    if p.acquire >= 0 {
        fds.push(p.acquire);
        flag |= wire::PRESENT_ACQUIRE;
    }
    let r = request(
        &Request {
            op: wire::OP_PRESENT,
            flag,
            id: p.id,
            ..Default::default()
        },
        &fds,
    );
    // The server has its own copy of the writer now.
    drop(writer);
    if r == 0 {
        p.present = aim_sync_file::give_to_guest(fence);
    }
    r
}

/// The hardware cursor, with its acquire fence.
fn cursor(c: &Cursor) -> i64 {
    // SAFETY: F_GETFD only checks that the guest's fd is open.
    if c.acquire >= 0 && unsafe { libc::fcntl(c.acquire, libc::F_GETFD) } < 0 {
        return neg(errno::EBADF);
    }
    let mut flag = if c.changed != 0 {
        wire::CURSOR_CHANGED
    } else {
        0
    };
    let fds: &[i32] = if c.acquire >= 0 {
        flag |= wire::CURSOR_ACQUIRE;
        &[c.acquire]
    } else {
        &[]
    };
    request(
        &Request {
            op: wire::OP_CURSOR,
            flag,
            id: c.id,
            x: c.x,
            y: c.y,
            ..Default::default()
        },
        fds,
    )
}

fn request(r: &Request, fds: &[i32]) -> i64 {
    let guard = CONNECTION.lock().unwrap();
    let Some(conn) = guard.as_ref() else {
        return neg(errno::ENOTCONN);
    };
    match wire::send_fds(conn.as_fd(), wire::bytes(r), fds) {
        Ok(()) => 0,
        Err(_) => neg(errno::ENOTCONN),
    }
}

/// The guest's `CLOCK_MONOTONIC`, in nanoseconds.
pub use aim_hostcall::clock::monotonic_ns;
