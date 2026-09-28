//! The layer's per-process state, carried to a fork child.
//!
//! A fork child is a fresh `linux-run` (`spawn`), so what Linux keeps in
//! the task and a Darwin fork would have copied with the heap is written
//! here into one blob and read back in the child before the guest resumes.
//! Each module saves and restores its own part (`save`/`restore` next to
//! its state), in the order of [`save`]. What the state names but the child
//! does not inherit (kqueues, the timerfd thread) is then rebuilt by the
//! module's `after_fork_child`.
//!
//! The encoding is private to one `linux-run` binary talking to itself:
//! little-endian integers and length-prefixed bytes, no versioning.

use std::path::PathBuf;

use crate::sys::{
    copies, cred, fdtab, mem, memfd, misc, process, procfs, pstate, selinuxfs, signal, thread, wait,
};

#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }

    pub fn u32(&mut self, v: u32) {
        self.u64(v as u64);
    }

    pub fn i32(&mut self, v: i32) {
        self.i64(v as i64);
    }

    pub fn bool(&mut self, v: bool) {
        self.0.push(v as u8);
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
    }

    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    pub fn path(&mut self, v: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt;
        self.bytes(v.as_os_str().as_bytes());
    }

    pub fn opt<T>(&mut self, v: Option<T>, f: impl FnOnce(&mut Self, T)) {
        self.bool(v.is_some());
        if let Some(v) = v {
            f(self, v);
        }
    }

    pub fn seq<T>(&mut self, v: impl ExactSizeIterator<Item = T>, mut f: impl FnMut(&mut Self, T)) {
        self.u64(v.len() as u64);
        for x in v {
            f(self, x);
        }
    }
}

/// Reads what [`Writer`] wrote. A short or malformed blob reads as zeros
/// and empties from the point of damage on, and [`Reader::ok`] turns false.
pub struct Reader<'a> {
    buf: &'a [u8],
    bad: bool,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, bad: false }
    }

    /// Whether everything read so far was there, and all of it was read.
    pub fn ok(&self) -> bool {
        !self.bad && self.buf.is_empty()
    }

    /// Whether everything read so far was there.
    pub fn intact(&self) -> bool {
        !self.bad
    }

    fn take(&mut self, n: usize) -> &'a [u8] {
        if self.bad || self.buf.len() < n {
            self.bad = true;
            return &[];
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        a
    }

    pub fn u64(&mut self) -> u64 {
        self.take(8).try_into().map_or(0, u64::from_le_bytes)
    }

    pub fn i64(&mut self) -> i64 {
        self.u64() as i64
    }

    pub fn u32(&mut self) -> u32 {
        self.u64() as u32
    }

    pub fn i32(&mut self) -> i32 {
        self.i64() as i32
    }

    pub fn bool(&mut self) -> bool {
        self.take(1).first().is_some_and(|&b| b != 0)
    }

    pub fn bytes(&mut self) -> Vec<u8> {
        let n = self.u64();
        if n > self.buf.len() as u64 {
            self.bad = true;
            return Vec::new();
        }
        self.take(n as usize).to_vec()
    }

    pub fn str(&mut self) -> String {
        String::from_utf8(self.bytes()).unwrap_or_else(|_| {
            self.bad = true;
            String::new()
        })
    }

    pub fn path(&mut self) -> PathBuf {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(self.bytes()))
    }

    pub fn opt<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        self.bool().then(|| f(self))
    }

    pub fn seq<T>(&mut self, mut f: impl FnMut(&mut Self) -> T) -> Vec<T> {
        let n = self.u64();
        let mut v = Vec::new();
        for _ in 0..n {
            if self.bad {
                break;
            }
            v.push(f(self));
        }
        v
    }
}

/// Every module's state, in restore order.
pub fn save(w: &mut Writer) {
    crate::xrt::fork_save(w);
    crate::diag::fork_save(w);
    crate::patch::fork_save(w);
    cred::fork_save(w);
    process::fork_save(w);
    procfs::fork_save(w);
    pstate::fork_save(w);
    misc::fork_save(w);
    mem::fork_save(w);
    copies::fork_save(w);
    memfd::fork_save(w);
    selinuxfs::fork_save(w);
    wait::fork_save(w);
    fdtab::fork_save(w);
    signal::fork_save(w);
    thread::fork_save(w);
    w.bytes(&darwin_host_gpu::fork_state());
}

/// Restore what [`save`] wrote, then rebuild the host objects the state
/// names; false when the blob is damaged.
pub fn restore(r: &mut Reader) -> bool {
    crate::xrt::fork_restore(r);
    crate::diag::fork_restore(r);
    crate::patch::fork_restore(r);
    cred::fork_restore(r);
    process::fork_restore(r);
    procfs::fork_restore(r);
    pstate::fork_restore(r);
    misc::fork_restore(r);
    mem::fork_restore(r);
    copies::fork_restore(r);
    memfd::fork_restore(r);
    selinuxfs::fork_restore(r);
    wait::fork_restore(r);
    fdtab::fork_restore(r);
    signal::fork_restore(r);
    thread::fork_restore(r);
    darwin_host_gpu::restore_fork_state(&r.bytes());
    if !r.intact() {
        return false;
    }
    wait::after_fork_child();
    fdtab::after_fork_child();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_damage_is_noticed() {
        let mut w = Writer::default();
        w.u64(7);
        w.i32(-3);
        w.bool(true);
        w.str("abc");
        w.opt(Some(5u64), |w, v| w.u64(v));
        w.seq([1u64, 2].into_iter(), |w, v| w.u64(v));
        let b = w.into_bytes();
        let mut r = Reader::new(&b);
        assert_eq!(
            (r.u64(), r.i32(), r.bool(), r.str()),
            (7, -3, true, "abc".into())
        );
        assert_eq!(r.opt(|r| r.u64()), Some(5));
        assert_eq!(r.seq(|r| r.u64()), vec![1, 2]);
        assert!(r.ok());
        let mut short = Reader::new(&b[..10]);
        short.u64();
        short.str();
        assert!(!short.ok());
    }
}
