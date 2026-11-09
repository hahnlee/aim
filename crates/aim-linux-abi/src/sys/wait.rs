//! Waiting for children and pidfds.
//!
//! Guest processes are Darwin processes, so the host kernel keeps the
//! parent/child relation, zombies and exit statuses; `wait4` and `waitid`
//! translate options, status words (signal numbers differ), `siginfo_t` and
//! `struct rusage`.
//!
//! A pidfd is a kqueue with an `EVFILT_PROC`/`NOTE_EXIT` filter on the
//! process: it polls readable once the process exits, as a Linux pidfd
//! does. Its state is the open file's ([`PidFd`], in [`fdtab`]), so a dup
//! of it is the same pidfd, as on Linux. The process's start time, recorded at open, tells it apart from a later
//! one with its pid: once no process has both, it was reaped, and the pidfd
//! also reports POLLHUP (`pidfd_poll`). The pidfd stays readable from the
//! exit on, so a wait for the reap alone (an interest in no events, or the
//! reap's edge for an edge-triggered one) is on the pidfd's [`ReapWatch`].
//! Darwin does not pass kqueues to a forked child; [`after_fork_child`]
//! recreates the pidfds there.

use super::fdtab::{self, Kind};
use aim_storage::process_namespace::ProcessIdentity;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::errno::{self, EAGAIN, EBADF, EINVAL, ESRCH};

const LINUX_SIGCHLD: i32 = 17;

// Linux wait options.
const WNOHANG: u64 = 0x1;
const WUNTRACED: u64 = 0x2; // WSTOPPED for waitid
const WEXITED: u64 = 0x4;
const WCONTINUED: u64 = 0x8;
const WNOWAIT: u64 = 0x0100_0000;
const WNOTHREAD: u64 = 0x2000_0000;
const WALL: u64 = 0x4000_0000;
const WCLONE: u64 = 0x8000_0000;

const P_ALL: u64 = 0;
const P_PID: u64 = 1;
const P_PGID: u64 = 2;
const P_PIDFD: u64 = 3;

/// Linux signal number of a Darwin one.
pub fn signal_from_host(host: i32) -> i32 {
    (1..32)
        .find(|&l| super::signal::to_host(l) == host)
        .unwrap_or(host)
}

/// Darwin status word -> Linux (`WIFEXITED`/`WIFSIGNALED`/`WIFSTOPPED`/
/// `WIFCONTINUED` encodings).
pub fn status_from_host(st: i32) -> i32 {
    let low = st & 0x7f;
    if low == 0 {
        return st & 0xff00;
    }
    if low == 0x7f {
        let sig = (st >> 8) & 0xff;
        if sig == libc::SIGCONT {
            return 0xffff;
        }
        return 0x7f | signal_from_host(sig) << 8;
    }
    signal_from_host(low) | (st & 0x80)
}

/// The Linux signal that killed host process `pid`, which exited with
/// `code` because Darwin could not carry the signal (`signal::die`): a
/// process that merely exits 128 + a signal recorded none.
fn fatal_exit(pid: i32, code: i32) -> Option<i32> {
    if code <= 128 {
        return None;
    }
    super::procrec::fatal_of(pid).filter(|&sig| code == 128 + sig)
}

/// Linux status word of host child `pid` that Darwin reports as `st`.
pub(super) fn status_of(pid: i32, st: i32) -> i32 {
    if st & 0x7f == 0
        && let Some(sig) = fatal_exit(pid, st >> 8 & 0xff)
    {
        return sig;
    }
    status_from_host(st)
}

/// Linux `si_code` and `si_status` of a `CLD_*` event that Darwin reports
/// as `code` and `status`, of a child that recorded `fatal`.
fn cld(code: i32, status: i32, fatal: Option<i32>) -> (i32, i32) {
    if code != libc::CLD_EXITED {
        return (code, signal_from_host(status));
    }
    match fatal.filter(|&sig| status == 128 + sig) {
        Some(sig) => (libc::CLD_KILLED, sig),
        None => (code, status),
    }
}

/// [`cld`] of host child `pid`, which still has its record.
fn child_info(pid: i32, code: i32, status: i32) -> (i32, i32) {
    let fatal = (code == libc::CLD_EXITED)
        .then(|| fatal_exit(pid, status))
        .flatten();
    cld(code, status, fatal)
}

/// Children this process reaped, with their real uid and recorded fatal
/// signal read before their entry and record went: a SIGCHLD routed after
/// the reap ([`sigchld_info`]) still reports them, as Linux fills its
/// siginfo at the child's exit (`do_notify_parent`). The latest
/// [`REAPED_KEPT`]; one goes when its SIGCHLD is routed.
static REAPED_CHILDREN: Mutex<VecDeque<(i32, u32, Option<i32>)>> = Mutex::new(VecDeque::new());
const REAPED_KEPT: usize = 32;

/// Host child `pid` was reaped: keep what its SIGCHLD reports, then drop
/// its entry and record.
fn reaped(pid: i32) {
    let child = (
        pid,
        super::signal::sender_uid(pid),
        super::procrec::fatal_of(pid),
    );
    {
        let mut r = REAPED_CHILDREN.lock().unwrap();
        r.retain(|c| c.0 != pid);
        if r.len() == REAPED_KEPT {
            r.pop_front();
        }
        r.push_back(child);
    }
    super::cred::forget(pid);
}

/// The guest's `si_pid`, `si_uid`, `si_code` and `si_status` of a SIGCHLD
/// that Darwin reports from host child `pid` as `code` and `status`.
pub(super) fn sigchld_info(pid: i32, code: i32, status: i32) -> (i32, u32, i32, i32) {
    // The record goes after the entry: read first, it was there while the
    // entry is.
    let fatal = super::procrec::fatal_of(pid);
    let (uid, fatal) = match super::cred::real_uid_of(pid) {
        Some(uid) => (uid, fatal),
        None => {
            let mut r = REAPED_CHILDREN.lock().unwrap();
            match r.iter().position(|c| c.0 == pid) {
                Some(i) => r.remove(i).map_or((0, None), |c| (c.1, c.2)),
                None => (0, None),
            }
        }
    };
    let (code, status) = cld(code, status, fatal);
    // Darwin sends a CLD_* SIGCHLD only from a child, a member of the
    // namespace even once reaped.
    (super::pidns::guest_pid(pid).unwrap_or(0), uid, code, status)
}

/// Linux arm64 `struct rusage` (timevals with 64-bit microseconds, the
/// maximum RSS in KiB).
fn put_rusage(ru: &libc::rusage, out: u64) -> Result<(), crate::errno::Errno> {
    let mut w = [0i64; 18];
    w[0] = ru.ru_utime.tv_sec;
    w[1] = ru.ru_utime.tv_usec as i64;
    w[2] = ru.ru_stime.tv_sec;
    w[3] = ru.ru_stime.tv_usec as i64;
    let rest = [
        ru.ru_maxrss / 1024,
        ru.ru_ixrss,
        ru.ru_idrss,
        ru.ru_isrss,
        ru.ru_minflt,
        ru.ru_majflt,
        ru.ru_nswap,
        ru.ru_inblock,
        ru.ru_oublock,
        ru.ru_msgsnd,
        ru.ru_msgrcv,
        ru.ru_nsignals,
        ru.ru_nvcsw,
        ru.ru_nivcsw,
    ];
    w[4..].copy_from_slice(&rest);
    // SAFETY: guest struct rusage (144 bytes).
    user_write!(out, [i64; 18], w);
    Ok(())
}

pub(super) fn put_rusage_of(who: i32, out: u64) -> i64 {
    // SAFETY: local buffer.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(who, &mut ru) } < 0 {
        return -(errno::last() as i64);
    }
    user_try!(put_rusage(&ru, out));
    0
}

pub fn wait4(a: [u64; 6]) -> i64 {
    let mut a = a;
    let pid = a[0] as i32;
    a[0] = if pid < -1 {
        match super::pidns::syscall_pid(-pid) {
            Ok(group) => (-group) as u64,
            Err(error) => return error,
        }
    } else {
        match super::pidns::syscall_pid(pid) {
            Ok(pid) => pid as u64,
            Err(error) => return error,
        }
    };
    let (pid, status, options, rusage) = (a[0] as i32, a[1], a[2], a[3]);
    if options & !(WNOHANG | WUNTRACED | WCONTINUED | WNOTHREAD | WALL | WCLONE) != 0 {
        return -(EINVAL as i64);
    }
    if pid > 0
        && let Some(r) = super::ptrace::wait(pid, options & WNOHANG != 0)
    {
        return match r {
            Ok(st) => {
                if let Some(st) = st.filter(|_| status != 0) {
                    // SAFETY: guest int.
                    user_write!(status, i32, st);
                }
                if rusage != 0 {
                    // SAFETY: guest struct rusage.
                    user_try!(super::user_memory::write_exact(rusage, &[0; 144]));
                }
                st.map_or(0, |_| pid as i64)
            }
            Err(e) => e,
        };
    }
    let mut hopts = 0;
    if options & WNOHANG != 0 {
        hopts |= libc::WNOHANG;
    }
    if options & WUNTRACED != 0 {
        hopts |= libc::WUNTRACED;
    }
    if options & WCONTINUED != 0 {
        hopts |= libc::WCONTINUED;
    }
    let mut st = 0;
    // SAFETY: local buffers.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::wait4(pid, &mut st, hopts, &mut ru) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    if r > 0 {
        // Read before the record goes with the entry.
        let st = status_of(r, st);
        if st & 0x7f != 0x7f {
            // Exited or killed: reaped.
            reaped(r);
            notice_reaped(r);
        }
        if status != 0 {
            // SAFETY: guest int.
            user_write!(status, i32, st);
        }
        if rusage != 0 {
            user_try!(put_rusage(&ru, rusage));
        }
    }
    if r > 0 {
        super::pidns::guest_pid(r)
            .map(|pid| pid as i64)
            .unwrap_or(-(crate::errno::ESRCH as i64))
    } else {
        r as i64
    }
}

/// The Linux `siginfo_t` fields `waitid` writes: the child's host pid,
/// the Linux `si_code` and `si_status`, and `uid`, the child's real uid in
/// the guest's model (the host one is not it); zeros for no child.
fn put_wait_siginfo(
    out: u64,
    info: Option<(i32, (i32, i32), u32)>,
) -> Result<(), crate::errno::Errno> {
    let (signo, code, pid, uid, status) = match info {
        Some((pid, (code, status), uid)) => (
            LINUX_SIGCHLD,
            code,
            super::pidns::guest_pid(pid).unwrap_or(0),
            uid,
            status,
        ),
        None => (0, 0, 0, 0, 0),
    };
    // SAFETY: guest siginfo_t (128 bytes).
    {
        let p = out as *mut u8;
        user_write!(p, [i32; 3], [signo, 0, code]);
        user_write!(out + 16, [i32; 3], [pid, uid as i32, status]);
    }
    Ok(())
}

pub fn waitid(a: [u64; 6]) -> i64 {
    let (idtype, id, infop, options, rusage) = (a[0], a[1], a[2], a[3], a[4]);
    if options & !(WNOHANG | WUNTRACED | WEXITED | WCONTINUED | WNOWAIT | WNOTHREAD | WALL | WCLONE)
        != 0
        || options & (WUNTRACED | WEXITED | WCONTINUED) == 0
    {
        return -(EINVAL as i64);
    }
    let mut nohang = options & WNOHANG != 0;
    let (htype, hid) = match idtype {
        P_ALL => (libc::P_ALL, 0),
        P_PID if (id as i32) > 0 => (
            libc::P_PID,
            match super::pidns::syscall_pid(id as i32) {
                Ok(pid) => pid as u32,
                Err(error) => return error,
            },
        ),
        P_PGID if (id as i32) >= 0 => (
            libc::P_PGID,
            if id == 0 {
                // SAFETY: trivial.
                unsafe { libc::getpgrp() as u32 }
            } else {
                match super::pidns::syscall_pid(id as i32) {
                    Ok(group) => group as u32,
                    Err(error) => return error,
                }
            },
        ),
        P_PIDFD => match pidfd_of(id as i32) {
            Some(pidfd) => {
                nohang |= pidfd.nonblock;
                // A readable pidfd's process has exited, and Linux has made
                // it a zombie by then. Darwin posts NOTE_EXIT a moment
                // before the zombie can be waited for: wait that moment
                // out rather than find nothing.
                if nohang && options & WEXITED != 0 && pidfd_readable(id as i32) {
                    nohang = false;
                }
                (libc::P_PID, pidfd.pid as u32)
            }
            None => return -(EBADF as i64),
        },
        _ => return -(EINVAL as i64),
    };
    let mut hopts = 0;
    for (l, d) in [
        (WEXITED, libc::WEXITED),
        (WUNTRACED, libc::WSTOPPED),
        (WCONTINUED, libc::WCONTINUED),
    ] {
        if options & l != 0 {
            hopts |= d;
        }
    }
    if nohang {
        hopts |= libc::WNOHANG;
    }
    // Darwin's waitid reports no rusage: peek with WNOWAIT, then reap the
    // child found with wait4, which does.
    let reap_with_wait4 = rusage != 0 && options & WNOWAIT == 0;
    if options & WNOWAIT != 0 || reap_with_wait4 {
        hopts |= libc::WNOWAIT;
    }
    // SAFETY: local siginfo.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::waitid(htype, hid, &mut info, hopts) } < 0 {
        return -(errno::last() as i64);
    }
    if info.si_pid == 0 && idtype == P_PIDFD && options & WNOHANG == 0 {
        // A non-blocking pidfd whose process is still running.
        return -(EAGAIN as i64);
    }
    if reap_with_wait4 && info.si_pid != 0 {
        let mut st = 0;
        // SAFETY: local buffers.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let flags = libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED;
        if unsafe { libc::wait4(info.si_pid, &mut st, flags, &mut ru) } > 0 {
            user_try!(put_rusage(&ru, rusage));
        }
    } else if rusage != 0 {
        // SAFETY: guest struct rusage.
        user_try!(super::user_memory::write_exact(rusage, &[0; 144]));
    }
    let ended = matches!(
        info.si_code,
        libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
    );
    // Read before the entry and the record go.
    let child = (info.si_pid != 0).then(|| {
        (
            info.si_pid,
            child_info(info.si_pid, info.si_code, info.si_status),
            super::signal::sender_uid(info.si_pid),
        )
    });
    if info.si_pid != 0 && ended && options & WNOWAIT == 0 {
        reaped(info.si_pid);
        notice_reaped(info.si_pid);
    }
    if infop != 0 {
        user_try!(put_wait_siginfo(infop, child));
    }
    0
}

// ---- pidfd -----------------------------------------------------------------

const PIDFD_NONBLOCK: u64 = 0o4000;
/// `EVFILT_USER` ident of the "already exited" filter.
const PIDFD_EXITED: usize = 0x7069_6466_ffff_ffff;
/// `EVFILT_USER` ident of a reap watch's "reaped here" filter.
const REAPED: usize = 0;
/// `NOTE_REAP` (`<sys/event.h>`): deprecated, but xnu still posts it to
/// the process's knotes when its parent reaps it.
const NOTE_REAP: u32 = 0x1000_0000;

/// A pidfd's open file, which every fd referring to it shares (dups, and
/// a fork child's copies), as [`fdtab`] keeps an open file's state.
pub struct PidFd {
    pid: i32,
    nonblock: bool,
    process: ProcessIdentity,
    /// Unset only in a fork child until [`after_fork_child`] makes it again.
    reap: OnceLock<ReapWatch>,
}

impl PidFd {
    /// Whether the process was reaped: no process has its pid and start.
    fn reaped(&self) -> bool {
        ProcessIdentity::running_strict(self.pid).map_or(true, |p| p != self.process)
    }

    /// Make the reap watch readable if the process was reaped.
    fn notice_reap(&self) {
        if let Some(reap) = self.reap.get().filter(|_| self.reaped()) {
            apply(
                reap.0,
                &[kev(REAPED, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER)],
            );
        }
    }

    /// Fork: the open file (its kqueues are made again in the child).
    pub(super) fn save(&self, w: &mut super::fork_state::Writer) {
        w.i32(self.pid);
        w.bool(self.nonblock);
        w.u64(self.process.start_seconds);
        w.u64(self.process.start_microseconds);
    }

    pub(super) fn load(r: &mut super::fork_state::Reader) -> Arc<Self> {
        let (pid, nonblock) = (r.i32(), r.bool());
        Arc::new(Self {
            pid,
            nonblock,
            process: ProcessIdentity {
                host_pid: pid,
                start_seconds: r.u64(),
                start_microseconds: r.u64(),
            },
            reap: OnceLock::new(),
        })
    }
}

/// A hidden kqueue that polls readable once a pidfd's process is reaped,
/// as `pidfd_poll`'s EPOLLHUP; the pidfd itself is readable from the exit.
/// Its `NOTE_REAP` filter sees the reap by whichever process does it, but
/// Darwin attaches none to a zombie: a "reaped here" user filter, which
/// this process triggers as it reaps the pid or finds it reaped, covers a
/// process that was a zombie when watched.
struct ReapWatch(i32);

impl ReapWatch {
    fn new(pid: i32) -> Result<Self, i64> {
        // SAFETY: plain kqueue creation.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(-(errno::last() as i64));
        }
        let watch = Self(fdtab::hide(kq));
        fdtab::set_flags(watch.0, false, true);
        let r = apply(
            watch.0,
            &[
                kev(REAPED, libc::EVFILT_USER, libc::EV_ADD, 0),
                kev(pid as usize, libc::EVFILT_PROC, libc::EV_ADD, NOTE_REAP),
            ],
        );
        // A process filter fails for a zombie or a process gone (ESRCH),
        // or one this process may not watch: as for the pidfd's own
        // (`new_pidfd_kqueue`), the pidfd stays, with only the "reaped
        // here" filter.
        match r[..] {
            [0, _] => Ok(watch),
            [e, _] => Err(-(errno::from_darwin(e as i32) as i64)),
            _ => Err(-(EINVAL as i64)),
        }
    }
}

impl Drop for ReapWatch {
    fn drop(&mut self) {
        if let Err(error) = fdtab::close_fork_private(self.0) {
            crate::diag!("pidfd reap watch close: errno {error}");
        }
    }
}

fn kev(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

/// Apply `changes` to `kq`, returning each change's error (0 on success).
fn apply(kq: i32, changes: &[libc::kevent]) -> Vec<i64> {
    let mut changes = changes.to_vec();
    for c in &mut changes {
        c.flags |= libc::EV_RECEIPT;
    }
    let mut out = changes.clone();
    // SAFETY: local arrays of equal length.
    let n = unsafe {
        libc::kevent(
            kq,
            changes.as_ptr(),
            changes.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
            std::ptr::null(),
        )
    };
    if n < 0 {
        return vec![errno::last() as i64; changes.len()];
    }
    out[..n as usize].iter().map(|e| e.data as i64).collect()
}

/// A kqueue that polls readable once `pid` exits; readable from the start
/// unless `watch`.
fn new_pidfd_kqueue(pid: i32, watch: bool) -> Result<i32, i64> {
    // SAFETY: plain kqueue creation.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(-(errno::last() as i64));
    }
    // SAFETY: our new fd.
    unsafe { libc::fcntl(kq, libc::F_SETFD, libc::FD_CLOEXEC) };
    let proc = kev(
        pid as usize,
        libc::EVFILT_PROC,
        libc::EV_ADD,
        libc::NOTE_EXIT,
    );
    if !watch || apply(kq, &[proc]) != [0] {
        // The process is gone or a zombie: it still exists for its parent
        // until reaped, and its pidfd is readable from the start.
        // SAFETY: probing a pid.
        if watch && unsafe { libc::kill(pid, 0) } < 0 && errno::last() == ESRCH {
            unsafe { libc::close(kq) };
            return Err(-(ESRCH as i64));
        }
        let exited = apply(
            kq,
            &[
                kev(PIDFD_EXITED, libc::EVFILT_USER, libc::EV_ADD, 0),
                kev(PIDFD_EXITED, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER),
            ],
        );
        if let Some(&e) = exited.iter().find(|&&e| e != 0) {
            unsafe { libc::close(kq) };
            return Err(-(errno::from_darwin(e as i32) as i64));
        }
    }
    Ok(kq)
}

pub(super) fn open_pidfd(pid: i32, nonblock: bool) -> i64 {
    let process = match ProcessIdentity::running_strict(pid) {
        Ok(process) => process,
        Err(e) => return -(errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)) as i64),
    };
    let reap = match ReapWatch::new(pid) {
        Ok(reap) => reap,
        Err(e) => return e,
    };
    let kq = match new_pidfd_kqueue(pid, true) {
        Ok(kq) => kq,
        Err(e) => return e,
    };
    let pidfd = Arc::new(PidFd {
        pid,
        nonblock,
        process,
        reap: OnceLock::from(reap),
    });
    // A reap since the watch was made, by this process or another.
    pidfd.notice_reap();
    fdtab::insert(kq, Kind::PidFd(pidfd));
    if let Err(error) = fdtab::publish_typed_guest(kq) {
        fdtab::on_close(kq);
        unsafe {
            libc::close(kq);
        }
        return -(error as i64);
    }
    kq as i64
}

/// Whether pidfd `fd` polls readable: its process exited. Polling the
/// kqueue leaves its events for the guest.
fn pidfd_readable(fd: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one local pollfd, no wait.
    unsafe { libc::poll(&mut p, 1, 0) == 1 && p.revents & libc::POLLIN != 0 }
}

/// The open pidfd file behind `fd`, whichever fd referring to it this is.
fn pidfd_of(fd: i32) -> Option<Arc<PidFd>> {
    match fdtab::get(fd)? {
        Kind::PidFd(pidfd) => Some(pidfd),
        _ => None,
    }
}

/// Whether `fd` is a pidfd whose process was reaped, which `pidfd_poll`
/// reports as EPOLLHUP.
pub(super) fn pidfd_reaped(fd: i32) -> bool {
    pidfd_of(fd).is_some_and(|p| p.reaped())
}

/// Pidfd `fd`'s open file, which keeps its reap watch, and the watch's fd.
pub(super) fn reap_watch(fd: i32) -> Option<(Arc<PidFd>, i32)> {
    let pidfd = pidfd_of(fd)?;
    let watch = pidfd.reap.get()?.0;
    Some((pidfd, watch))
}

/// An epoll interest's own fd of a pidfd's reap watch, for the interest's
/// knote on it: as Linux's epitem is per (file, fd), two dups of a pidfd
/// in one epoll each have their own. It keeps the watch's kqueue, not the
/// pidfd, open while the interest lasts.
pub(super) struct ReapAlias {
    pidfd: Weak<PidFd>,
    watch: ReapWatch,
}

impl ReapAlias {
    /// One for pidfd `fd`; None if `fd` is no pidfd.
    pub(super) fn new(fd: i32) -> Option<Result<Self, errno::Errno>> {
        let (pidfd, watch) = reap_watch(fd)?;
        // SAFETY: duplicating the watch's fd.
        let alias = unsafe { libc::dup(watch) };
        if alias < 0 {
            return Some(Err(errno::last()));
        }
        let watch = ReapWatch(fdtab::hide(alias));
        fdtab::set_flags(watch.0, false, true);
        Some(Ok(Self {
            pidfd: Arc::downgrade(&pidfd),
            watch,
        }))
    }

    pub(super) fn fd(&self) -> i32 {
        self.watch.0
    }

    /// Whether `fd` still refers to the pidfd this alias watches.
    pub(super) fn current(&self, fd: i32) -> bool {
        pidfd_of(fd).is_some_and(|p| std::ptr::eq(Arc::as_ptr(&p), self.pidfd.as_ptr()))
    }
}

/// The fds referring to pidfds, with their open files.
fn pidfds() -> Vec<(i32, Arc<PidFd>)> {
    fdtab::fds_where(|k| matches!(k, Kind::PidFd(_)))
        .into_iter()
        .filter_map(|(fd, k)| match k {
            Kind::PidFd(p) => Some((fd, p)),
            _ => None,
        })
        .collect()
}

/// This process reaped `pid`: make its pidfds' reap watches readable,
/// which `NOTE_REAP` does not for a process that was a zombie when
/// watched.
fn notice_reaped(pid: i32) {
    for (_, p) in pidfds().iter().filter(|(_, p)| p.pid == pid) {
        p.notice_reap();
    }
}

pub fn pidfd_open(a: [u64; 6]) -> i64 {
    let mut a = a;
    a[0] = match super::pidns::syscall_pid(a[0] as i32) {
        Ok(pid) => pid as u64,
        Err(error) => return error,
    };
    let (pid, flags) = (a[0] as i32, a[1]);
    if flags & !PIDFD_NONBLOCK != 0 || pid <= 0 {
        return -(EINVAL as i64);
    }
    if let Err(e) = super::pidns::check(pid) {
        return e;
    }
    open_pidfd(pid, flags & PIDFD_NONBLOCK != 0)
}

/// Send Linux signal `sig` to `pid`: through the signal layer for this
/// process, as the host signal for another one. A pid the Mac reused
/// after the process exited is no longer in the namespace.
fn send_signal(pid: i32, sig: i32) -> i64 {
    if pid as i64 == super::process::getpid() {
        return super::signal::kill([pid as u64, sig as u64, 0, 0, 0, 0]);
    }
    if let Err(e) = super::pidns::check(pid) {
        return e;
    }
    let host = if sig == 0 {
        0
    } else {
        match super::signal::to_host(sig) {
            0 => return -(EINVAL as i64),
            h => h,
        }
    };
    super::signal::signal_process(pid, sig, host)
}

pub fn pidfd_send_signal(a: [u64; 6]) -> i64 {
    let (fd, sig, info, flags) = (a[0] as i32, a[1] as i32, a[2], a[3]);
    if flags != 0 || !(0..=64).contains(&sig) {
        return -(EINVAL as i64);
    }
    if info != 0 {
        // SAFETY: guest siginfo_t; si_signo must name the signal sent.
        if user_read!(info, i32) != sig {
            return -(EINVAL as i64);
        }
    }
    let Some(pidfd) = pidfd_of(fd) else {
        return -(EBADF as i64);
    };
    send_signal(pidfd.pid, sig)
}

/// In a forked child: kqueues are not inherited, so each pidfd is made
/// again, once per open file and duplicated onto every fd number that
/// referred to it (held by placeholders until now), as Linux children
/// inherit their parent's pidfds; one whose process was reaped watches no
/// process that took its pid since. The reap watches are made again too;
/// one whose process is a zombie now, not this process's to reap, gets no
/// notice of the reap.
pub(super) fn after_fork_child() -> Result<(), errno::Errno> {
    let mut made: Vec<(i32, Arc<PidFd>)> = Vec::new();
    for (fd, p) in pidfds() {
        let source = made.iter().find(|(_, m)| Arc::ptr_eq(m, &p)).map(|m| m.0);
        let kq = match source {
            Some(source) => source,
            None => {
                let watch = ReapWatch::new(p.pid).map_err(|error| -error as errno::Errno)?;
                p.reap.get_or_init(|| watch);
                p.notice_reap();
                new_pidfd_kqueue(p.pid, !p.reaped()).map_err(|error| -error as errno::Errno)?
            }
        };
        let result = fdtab::restore_kqueue(fd, kq, true);
        if kq != fd && source.is_none() {
            // SAFETY: our temporary kqueue, now on `fd`.
            unsafe { libc::close(kq) };
        }
        result?;
        if source.is_none() {
            made.push((fd, p));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_words_use_linux_signal_numbers() {
        assert_eq!(status_from_host(3 << 8), 3 << 8);
        // Darwin SIGUSR1 (30) -> Linux 10; core dump flag kept.
        assert_eq!(status_from_host(libc::SIGUSR1 | 0x80), 10 | 0x80);
        // Stopped by Darwin SIGTSTP (18) -> Linux SIGTSTP (20).
        assert_eq!(status_from_host(0x7f | libc::SIGTSTP << 8), 0x7f | 20 << 8);
        assert_eq!(status_from_host(0x7f | libc::SIGCONT << 8), 0xffff);
    }
}
