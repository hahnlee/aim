//! Waiting for children and pidfds.
//!
//! Guest processes are Darwin processes, so the host kernel keeps the
//! parent/child relation, zombies and exit statuses; `wait4` and `waitid`
//! translate options, status words (signal numbers differ), `siginfo_t` and
//! `struct rusage`.
//!
//! A pidfd is a kqueue with an `EVFILT_PROC`/`NOTE_EXIT` filter on the
//! process: it polls readable once the process exits, as a Linux pidfd
//! does. A disabled `EVFILT_USER` filter whose ident encodes the pid tags
//! it, so a table entry is only trusted while the fd still is that kqueue.
//! Darwin does not pass kqueues to a forked child; [`after_fork_child`]
//! recreates the pidfds there.

use super::fdtab;
use std::sync::Mutex;

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

/// Linux arm64 `struct rusage` (timevals with 64-bit microseconds, the
/// maximum RSS in KiB).
fn put_rusage(ru: &libc::rusage, out: u64) {
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
    unsafe { (out as *mut [i64; 18]).write_unaligned(w) };
}

pub(super) fn put_rusage_of(who: i32, out: u64) -> i64 {
    // SAFETY: local buffer.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(who, &mut ru) } < 0 {
        return -(errno::last() as i64);
    }
    put_rusage(&ru, out);
    0
}

pub fn wait4(a: [u64; 6]) -> i64 {
    let mut a=a;
    let pid=a[0] as i32;
    a[0]=if pid < -1 {match super::pidns::syscall_pid(-pid){Ok(group)=>(-group) as u64,Err(error)=>return error}}else{match super::pidns::syscall_pid(pid){Ok(pid)=>pid as u64,Err(error)=>return error}};
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
                    unsafe { (status as *mut i32).write_unaligned(st) };
                }
                if rusage != 0 {
                    // SAFETY: guest struct rusage.
                    unsafe { std::ptr::write_bytes(rusage as *mut u8, 0, 144) };
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
        if st & 0x7f != 0x7f {
            // Exited or killed: reaped.
            super::cred::forget(r);
        }
        if status != 0 {
            // SAFETY: guest int.
            unsafe { (status as *mut i32).write_unaligned(status_from_host(st)) };
        }
        if rusage != 0 {
            put_rusage(&ru, rusage);
        }
    }
    if r>0 {super::pidns::guest_pid(r).map(|pid|pid as i64).unwrap_or(-(crate::errno::ESRCH as i64))} else {r as i64}
}

/// The Linux `siginfo_t` fields `waitid` writes.
/// `uid` is the child's real uid in the guest's model (the host one is not
/// it).
fn put_wait_siginfo(out: u64, info: Option<(&libc::siginfo_t, u32)>) {
    let (signo, code, pid, uid, status) = match info {
        Some((i, uid)) if i.si_pid != 0 => {
            let status = if i.si_code == libc::CLD_EXITED {
                i.si_status
            } else {
                signal_from_host(i.si_status)
            };
            (LINUX_SIGCHLD, i.si_code, super::pidns::guest_pid(i.si_pid).unwrap_or(0), uid, status)
        }
        _ => (0, 0, 0, 0, 0),
    };
    // SAFETY: guest siginfo_t (128 bytes).
    unsafe {
        let p = out as *mut u8;
        (p as *mut [i32; 3]).write_unaligned([signo, 0, code]);
        (p.add(16) as *mut [i32; 3]).write_unaligned([pid, uid as i32, status]);
    }
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
        P_PID if (id as i32) > 0 => (libc::P_PID,match super::pidns::syscall_pid(id as i32){Ok(pid)=>pid as u32,Err(error)=>return error}),
        P_PGID if (id as i32) >= 0 => (
            libc::P_PGID,
            if id == 0 {
                // SAFETY: trivial.
                unsafe { libc::getpgrp() as u32 }
            } else {
                match super::pidns::syscall_pid(id as i32){Ok(group)=>group as u32,Err(error)=>return error}
            },
        ),
        P_PIDFD => match pidfd_lookup(id as i32) {
            Some((pid, nonblock)) => {
                nohang |= nonblock;
                (libc::P_PID, pid as u32)
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
            put_rusage(&ru, rusage);
        }
    } else if rusage != 0 {
        // SAFETY: guest struct rusage.
        unsafe { std::ptr::write_bytes(rusage as *mut u8, 0, 144) };
    }
    let ended = matches!(
        info.si_code,
        libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
    );
    let uid = super::signal::sender_uid(info.si_pid);
    if info.si_pid != 0 && ended && options & WNOWAIT == 0 {
        super::cred::forget(info.si_pid);
    }
    if infop != 0 {
        put_wait_siginfo(infop, Some((&info, uid)));
    }
    0
}

// ---- pidfd -----------------------------------------------------------------

const PIDFD_NONBLOCK: u64 = 0o4000;
/// `EVFILT_USER` ident of the tag filter: this prefix | pid.
const PIDFD_TAG: usize = 0x7069_6466_0000_0000;
/// `EVFILT_USER` ident of the "already exited" filter.
const PIDFD_EXITED: usize = 0x7069_6466_ffff_ffff;

struct PidFd {
    fd: i32,
    pid: i32,
    nonblock: bool,
}

static PIDFDS: Mutex<Vec<PidFd>> = Mutex::new(Vec::new());

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

/// A kqueue that polls readable once `pid` exits, tagged with the pid.
fn new_pidfd_kqueue(pid: i32) -> Result<i32, i64> {
    // SAFETY: plain kqueue creation.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(-(errno::last() as i64));
    }
    // SAFETY: our new fd.
    unsafe { libc::fcntl(kq, libc::F_SETFD, libc::FD_CLOEXEC) };
    let tag = kev(
        PIDFD_TAG | pid as u32 as usize,
        libc::EVFILT_USER,
        libc::EV_ADD | libc::EV_DISABLE,
        0,
    );
    let proc = kev(
        pid as usize,
        libc::EVFILT_PROC,
        libc::EV_ADD,
        libc::NOTE_EXIT,
    );
    let r = apply(kq, &[tag, proc]);
    if r.first() != Some(&0) {
        // SAFETY: closing our fd.
        unsafe { libc::close(kq) };
        return Err(-(EINVAL as i64));
    }
    if r.get(1) != Some(&0) {
        // The process is gone or a zombie: it still exists for its parent
        // until reaped, and its pidfd is readable from the start.
        // SAFETY: probing a pid.
        if unsafe { libc::kill(pid, 0) } < 0 && errno::last() == ESRCH {
            unsafe { libc::close(kq) };
            return Err(-(ESRCH as i64));
        }
        apply(
            kq,
            &[
                kev(PIDFD_EXITED, libc::EVFILT_USER, libc::EV_ADD, 0),
                kev(PIDFD_EXITED, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER),
            ],
        );
    }
    Ok(kq)
}

/// Whether `fd` is still the pidfd kqueue of `pid`.
fn has_tag(fd: i32, pid: i32) -> bool {
    let probe = kev(
        PIDFD_TAG | pid as u32 as usize,
        libc::EVFILT_USER,
        libc::EV_DISABLE,
        0,
    );
    apply(fd, &[probe]) == [0]
}

pub(super) fn open_pidfd(pid: i32, nonblock: bool) -> i64 {
    let kq = match new_pidfd_kqueue(pid) {
        Ok(kq) => kq,
        Err(e) => return e,
    };
    let mut t = PIDFDS.lock().unwrap();
    t.retain(|p| p.fd != kq);
    t.push(PidFd {
        fd: kq,
        pid,
        nonblock,
    });
    drop(t);
    if let Err(error)=fdtab::publish_typed_guest(kq){
        PIDFDS.lock().unwrap().retain(|entry|entry.fd!=kq);unsafe{libc::close(kq);}return -(error as i64);
    }
    kq as i64
}

/// The pid and O_NONBLOCK flag behind a pidfd.
fn pidfd_lookup(fd: i32) -> Option<(i32, bool)> {
    let t = PIDFDS.lock().unwrap();
    let p = t.iter().find(|p| p.fd == fd)?;
    has_tag(fd, p.pid).then_some((p.pid, p.nonblock))
}

pub fn pidfd_open(a: [u64; 6]) -> i64 {
    let mut a=a;
    a[0]=match super::pidns::syscall_pid(a[0] as i32){Ok(pid)=>pid as u64,Err(error)=>return error};
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
        if unsafe { (info as *const i32).read_unaligned() } != sig {
            return -(EINVAL as i64);
        }
    }
    let Some((pid, _)) = pidfd_lookup(fd) else {
        return -(EBADF as i64);
    };
    send_signal(pid, sig)
}

/// Before fork: forget pidfds the guest closed or replaced.
pub(super) fn prune_pidfds() {
    PIDFDS.lock().unwrap().retain(|p| has_tag(p.fd, p.pid));
}

/// Fork: the pidfds (their kqueues are made again in the child).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    prune_pidfds();
    let t = PIDFDS.lock().unwrap();
    w.seq(t.iter(), |w, p| {
        w.i32(p.fd);
        w.i32(p.pid);
        w.bool(p.nonblock);
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let v = r.seq(|r| PidFd {
        fd: r.i32(),
        pid: r.i32(),
        nonblock: r.bool(),
    });
    *PIDFDS.lock().unwrap() = v;
}

/// The fds of this process's pidfds.
pub(super) fn pidfd_fds() -> Vec<i32> {
    prune_pidfds();
    PIDFDS.lock().unwrap().iter().map(|p| p.fd).collect()
}

/// In a forked child: kqueues are not inherited, so each pidfd is
/// recreated at its number (held by a placeholder until now), as Linux
/// children inherit their parent's pidfds.
pub(super) fn after_fork_child() {
    let mut t = PIDFDS.lock().unwrap();
    t.retain(|p| {
        // SAFETY: filling the fd number the kqueue had.
        unsafe {
            let Ok(kq) = new_pidfd_kqueue(p.pid) else {
                return false;
            };
            if kq == p.fd {
                return true;
            }
            let ok = libc::dup2(kq, p.fd) == p.fd;
            libc::close(kq);
            if ok {
                libc::fcntl(p.fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            ok
        }
    });
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
