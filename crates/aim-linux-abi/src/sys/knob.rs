//! Kernel files whose writes act rather than store: selinuxfs's `access`
//! transaction, a thread's `comm`, a device's `uevent`.
//!
//! The fd is an unlinked file holding what a read returns, so reads, seeks
//! and fstat stay native. A write goes to the file's handler instead; a
//! reply replaces the contents and is read from the start, as a Linux
//! transaction file answers the write that precedes the read. A refused
//! write fails with the handler's errno.

use std::sync::Arc;

use super::fdtab::{self, Kind};
use crate::errno::{self, Errno};

type OnWrite = dyn Fn(&[u8]) -> Result<Option<Vec<u8>>, Errno> + Send + Sync;

pub struct Knob(Box<OnWrite>);

/// A knob fd reading `contents`, whose writes go to `on_write`.
pub fn open(
    contents: &[u8],
    cloexec: bool,
    on_write: impl Fn(&[u8]) -> Result<Option<Vec<u8>>, Errno> + Send + Sync + 'static,
) -> i64 {
    let fd = super::procfs::content_fd_unpublished(contents, cloexec);
    if fd >= 0 {
        fdtab::insert(fd as i32, Kind::Knob(Arc::new(Knob(Box::new(on_write)))));
        if let Err(error)=fdtab::publish_guest(fd as i32){
            fdtab::on_close(fd as i32);unsafe{libc::close(fd as i32);}return -(error as i64);
        }
    }
    fd
}

/// write/writev on a knob: the whole write is one request.
pub fn write(fd: i32, k: &Knob, iov: &[libc::iovec]) -> i64 {
    let mut req = Vec::new();
    for v in iov {
        // SAFETY: guest buffers.
        req.extend_from_slice(unsafe {
            std::slice::from_raw_parts(v.iov_base as *const u8, v.iov_len)
        });
    }
    let reply = match (k.0)(&req) {
        Ok(reply) => reply,
        Err(e) => return -(e as i64),
    };
    if let Some(reply) = reply {
        // SAFETY: rewriting our own unlinked file.
        unsafe {
            libc::ftruncate(fd, 0);
            if libc::pwrite(fd, reply.as_ptr().cast(), reply.len(), 0) < 0 {
                return -(errno::last() as i64);
            }
            libc::lseek(fd, 0, libc::SEEK_SET);
        }
    }
    req.len() as i64
}
