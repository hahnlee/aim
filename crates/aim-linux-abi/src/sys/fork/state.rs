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
//! Versioned little-endian integers and length-prefixed bytes.

use std::path::PathBuf;

use crate::sys::{
    attrs, copies, cred, fdtab, mem, memfd, misc, process, procfs, pstate, ptrace, selinuxfs,
    signal, sync_file, thread, wait,
};

#[derive(Default)]
pub struct Writer(Vec<u8>,Vec<fdtab::ForkPrivateFd>,Option<crate::errno::Errno>);

impl Writer {
    pub fn error(&mut self,error:crate::errno::Errno){self.2.get_or_insert(error);}
    pub fn retain_private(&mut self,fd:i32){
        if self.1.iter().any(|owner|owner.target()==fd)||self.2.is_some(){return;}
        match fdtab::hold_fork_private(unsafe{std::os::fd::BorrowedFd::borrow_raw(fd)}){Ok(owner)=>self.1.push(owner),Err(error)=>self.2=Some(error)}
    }
    pub fn take_private(&mut self)->Result<Vec<fdtab::ForkPrivateFd>,crate::errno::Errno>{if let Some(error)=self.2.take(){return Err(error);}Ok(std::mem::take(&mut self.1))}
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
    total: usize,
    bad: bool,
    stage: &'static str,
    failure: Option<(&'static str, usize)>,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, total: buf.len(), bad: false, stage: "header", failure: None }
    }

    /// Whether everything read so far was there, and all of it was read.
    pub fn ok(&self) -> bool {
        !self.bad && self.buf.is_empty()
    }

    /// Whether everything read so far was there.
    pub fn intact(&self) -> bool {
        !self.bad
    }
    pub fn stage(&mut self, stage: &'static str) { self.stage = stage; }
    pub fn diagnostic(&self) -> String {
        let (stage, offset) = self.failure.unwrap_or((self.stage, self.total - self.buf.len()));
        format!("stage={stage} offset={offset} total={} remaining={} invalid={}", self.total, self.buf.len(), self.bad)
    }
    pub fn invalidate(&mut self) {
        if self.failure.is_none() { self.failure = Some((self.stage, self.total - self.buf.len())); }
        self.bad = true;
    }

    fn take(&mut self, n: usize) -> &'a [u8] {
        if self.bad || self.buf.len() < n {
            self.invalidate();
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
            self.invalidate();
            return Vec::new();
        }
        self.take(n as usize).to_vec()
    }

    pub fn str(&mut self) -> String {
        String::from_utf8(self.bytes()).unwrap_or_else(|_| {
            self.invalidate();
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
pub fn save(w:&mut Writer,mounts:&str){
    w.u32(0x46444e03);
    crate::xrt::fork_save(w);
    crate::diag::fork_save(w);
    crate::patch::fork_save(w);
    crate::vdso::fork_save(w);
    cred::fork_save(w);
    process::fork_save(w);
    procfs::fork_save(w);
    pstate::fork_save(w);
    misc::fork_save(w);
    mem::fork_save(w);
    copies::fork_save(w);
    memfd::fork_save(w);
    attrs::fork_save(w);
    selinuxfs::fork_save(w);
    wait::fork_save(w);
    w.str(mounts);
    fdtab::fork_save(w);
    ptrace::fork_save(w);
    sync_file::fork_save(w);
    signal::fork_save(w);
    thread::fork_save(w);
    w.bytes(&aim_host_gpu::fork_state());
}

/// Restore what [`save`] wrote, then rebuild the host objects the state
/// names; false when the blob is damaged.
pub fn restore(r: &mut Reader) -> bool {
    r.stage("state-version");
    let version=r.u32();
    if version!=0x46444e03{eprintln!("fork state version: received={version:#x} expected=0x46444e03");r.invalidate();return false;}
    r.stage("xrt");
    crate::xrt::fork_restore(r);
    r.stage("diag");
    crate::diag::fork_restore(r);
    r.stage("patch");
    crate::patch::fork_restore(r);
    r.stage("vdso");
    crate::vdso::fork_restore(r);
    r.stage("cred");
    cred::fork_restore(r);
    r.stage("process");
    process::fork_restore(r);
    r.stage("procfs");
    procfs::fork_restore(r);
    r.stage("pstate");
    pstate::fork_restore(r);
    r.stage("misc");
    misc::fork_restore(r);
    r.stage("mem");
    mem::fork_restore(r);
    r.stage("copies");
    copies::fork_restore(r);
    r.stage("memfd");
    memfd::fork_restore(r);
    r.stage("attrs");
    attrs::fork_restore(r);
    r.stage("selinuxfs");
    selinuxfs::fork_restore(r);
    r.stage("wait");
    wait::fork_restore(r);
    r.stage("mount-namespace");
    if let Err(error)=crate::vfs::load_own_mounts(&r.str()){eprintln!("fork mount namespace restore failed: errno={error}");return false;}
    r.stage("fdtab");
    fdtab::fork_restore(r);
    r.stage("ptrace");
    ptrace::fork_restore(r);
    r.stage("sync_file");
    sync_file::fork_restore(r);
    r.stage("signal");
    signal::fork_restore(r);
    r.stage("thread");
    thread::fork_restore(r);
    r.stage("gpu");
    let gpu = r.bytes();
    if gpu != aim_host_gpu::fork_state() {
        crate::hostcall::mark_used();
    }
    aim_host_gpu::restore_fork_state(&gpu);
    if !r.intact() {
        return false;
    }
    wait::after_fork_child();
    r.stage("descriptor-rebuild");
    if let Err(error)=fdtab::after_fork_child(){eprintln!("fork descriptor rebuild: errno {error}");r.invalidate();return false;}
    super::super::posix_locks::reset_fork();
    r.stage("posix-admission");
    if let Err(error)=super::super::posix_locks::attach_fork(){eprintln!("fork POSIX owner admission: errno {error}");return false;}
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_diagnostics_retain_first_owner_and_exact_byte_boundary() {
        let mut w=Writer::default();w.u64(7);w.str("abc");
        let bytes=w.into_bytes();
        let mut short=Reader::new(&bytes[..18]);short.stage("prefix");assert_eq!(short.u64(),7);
        short.stage("names");assert_eq!(short.str(),"");assert!(!short.intact());
        short.stage("later");short.u64();
        assert_eq!(short.diagnostic(),"stage=names offset=16 total=18 remaining=2 invalid=true");
        let mut invalid=Reader::new(&[1,0,0,0,0,0,0,0,255]);invalid.stage("utf8");assert_eq!(invalid.str(),"");
        assert_eq!(invalid.diagnostic(),"stage=utf8 offset=9 total=9 remaining=0 invalid=true");
        let mut tail=Reader::new(&[0;9]);tail.stage("descriptor-flags");tail.u64();assert!(tail.intact());assert!(!tail.ok());
        assert_eq!(tail.diagnostic(),"stage=descriptor-flags offset=8 total=9 remaining=1 invalid=false");
        let mut version=Reader::new(&[0;8]);assert!(!restore(&mut version));
        assert_eq!(version.diagnostic(),"stage=state-version offset=8 total=8 remaining=0 invalid=true");
    }

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
