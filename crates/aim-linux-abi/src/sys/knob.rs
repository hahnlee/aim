//! Kernel files whose writes act rather than store: selinuxfs's `access`
//! transaction, a thread's `comm`.
//!
//! The fd is an unlinked file holding what a read returns, so reads, seeks
//! and fstat stay native. A write goes to the file's handler instead; a
//! reply replaces the contents and is read from the start, as a Linux
//! transaction file answers the write that precedes the read.

use std::sync::Arc;

use super::fdtab::{self, Kind};
use crate::errno;

type OnWrite = dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync;

pub struct Knob(Box<OnWrite>);

/// A knob fd reading `contents`, whose writes go to `on_write`.
pub fn open(
    contents: &[u8],
    cloexec: bool,
    on_write: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> i64 {
    let fd = super::procfs::content_fd(contents, cloexec);
    if fd >= 0 {
        fdtab::insert(fd as i32, Kind::Knob(Arc::new(Knob(Box::new(on_write)))));
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
    if let Some(reply) = (k.0)(&req) {
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
