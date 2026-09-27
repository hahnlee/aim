//! Host-call module [`darwin_hostcall::module::DISPLAY`]: the host side of
//! the composer HAL (`docs/composer.md`).
//!
//! The window lives in the display server, `darwin-display` (this crate's
//! binary), not in the composer's process: AppKit needs the main thread,
//! which `linux-run` gives to the guest. The module connects to the server
//! named by `linux-run --display` and forwards the composer's requests
//! ([`wire`]). A buffer travels once, as its memfd over `SCM_RIGHTS`; the
//! server maps the same memory, so nothing is copied. The guest gets the
//! connection itself to read vsync events from.

pub mod wire;

use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use darwin_hostcall::display::{
    Buffer, Connect, FN_CONNECT, FN_IMPORT, FN_PRESENT, FN_RELEASE, FN_SET_VSYNC, Import, SetVsync,
    VERSION,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};
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
        FN_PRESENT | FN_RELEASE => unsafe { args_mut::<Buffer>(args, len) }.map(|b| {
            let op = if func == FN_PRESENT {
                wire::OP_PRESENT
            } else {
                wire::OP_RELEASE
            };
            request(
                &Request {
                    op,
                    id: b.id,
                    ..Default::default()
                },
                None,
            )
        }),
        FN_SET_VSYNC => unsafe { args_mut::<SetVsync>(args, len) }.map(|v| {
            request(
                &Request {
                    op: wire::OP_SET_VSYNC,
                    flag: (v.enabled != 0) as u32,
                    ..Default::default()
                },
                None,
            )
        }),
        _ => Err(neg(errno::ENOSYS)),
    };
    r.unwrap_or_else(|e| e)
}

fn connect(out: &mut Connect) -> i64 {
    let Some(path) = SERVER.get() else {
        return neg(errno::ENODEV);
    };
    let Ok(stream) = UnixStream::connect(path) else {
        return neg(errno::ECONNREFUSED);
    };
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
    let hello = Request {
        op: wire::OP_HELLO,
        flag: out.display,
        id: wire::VERSION,
        ..Default::default()
    };
    // The server answers at once; a hung one must not hang the guest.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let info = match wire::send(stream.as_fd(), wire::bytes(&hello), None)
        .and_then(|()| wire::recv_record::<Connect>(stream.as_fd()))
    {
        Ok(Some(info)) => info,
        _ => return neg(errno::ECONNREFUSED),
    };
    let _ = stream.set_read_timeout(None);
    let mine = OwnedFd::from(stream);
    // SAFETY: fcntl on a fd we own; the guest owns the duplicate.
    let guest = unsafe { libc::fcntl(mine.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if guest < 0 {
        return neg(errno::ENODEV);
    }
    *out = Connect {
        display: out.display,
        ..info
    };
    *CONNECTION.lock().unwrap() = Some(mine);
    guest as i64
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
        Some(i.fd),
    )
}

fn request(r: &Request, fd: Option<i32>) -> i64 {
    let guard = CONNECTION.lock().unwrap();
    let Some(conn) = guard.as_ref() else {
        return neg(errno::ENOTCONN);
    };
    match wire::send(conn.as_fd(), wire::bytes(r), fd) {
        Ok(()) => 0,
        Err(_) => neg(errno::ENOTCONN),
    }
}
