//! ptrace, and what other processes read of a process: its memory
//! (`process_vm_readv`) and its `/proc/<pid>/{maps,fd}`.
//!
//! A guest process is a Darwin process, which no other process may stop,
//! read or write, nor send a real-time signal or a signal Darwin would
//! take for a fault (`signal::signal_process`). Each process runs an agent (`agent`) that does it for
//! its peers on the process's behalf, and the process's threads stop at
//! the points where the layer delivers signals (`tracee`). The tracer's
//! side is here: requests go to the agent of the process a tid belongs
//! to, and its `wait4` for a tracee asks there too.
//!
//! Supported (ptrace(2)): PTRACE_SEIZE, PTRACE_INTERRUPT, PTRACE_CONT,
//! PTRACE_DETACH, PTRACE_SETOPTIONS, PTRACE_GETREGSET (NT_PRSTATUS,
//! NT_PRFPREG, NT_ARM_TLS), PTRACE_GETEVENTMSG and PTRACE_PEEKTEXT/DATA;
//! the options PTRACE_O_TRACEFORK, _TRACEVFORK, _TRACECLONE, _TRACESYSGOOD
//! and _EXITKILL; `wait4` for a tracee by tid. Other requests fail with
//! EIO as unknown ones do on Linux, other options with EINVAL. No
//! signal-delivery-stops are made: a traced thread takes its signals as
//! an untraced one does (#594).

mod agent;
mod tracee;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex};

use crate::errno::{EINTR, EINVAL, EIO, EPERM, ESRCH};
use crate::sys::fork_state::{Reader, Writer};
use crate::sys::{cred, pidns, thread};

pub use agent::{fork_restore, fork_save, socket_path, start};
pub use tracee::{
    born_traced, clone_event, clone_stop, forget, process_ending, thread_ended, tracer_of,
    trap_pending, trap_stop,
};

const PTRACE_PEEKTEXT: u64 = 1;
const PTRACE_PEEKDATA: u64 = 2;
const PTRACE_CONT: u64 = 7;
const PTRACE_DETACH: u64 = 17;
const PTRACE_SETOPTIONS: u64 = 0x4200;
const PTRACE_GETEVENTMSG: u64 = 0x4201;
const PTRACE_GETREGSET: u64 = 0x4204;
const PTRACE_SEIZE: u64 = 0x4206;
const PTRACE_INTERRUPT: u64 = 0x4207;

const O_TRACESYSGOOD: u64 = 1;
const O_TRACEFORK: u64 = 2;
const O_TRACEVFORK: u64 = 4;
const O_TRACECLONE: u64 = 8;
pub(super) const O_EXITKILL: u64 = 0x10_0000;
const OPTIONS: u64 = O_TRACESYSGOOD | O_TRACEFORK | O_TRACEVFORK | O_TRACECLONE | O_EXITKILL;
const CAP_SYS_PTRACE: u32 = 19;

/// Connections to other processes' agents, by pid.
static CONNS: LazyLock<Mutex<HashMap<i32, i32>>> = LazyLock::new(Default::default);
/// Set once this process traces: its `wait4`s may be for tracees.
static TRACER: AtomicBool = AtomicBool::new(false);

fn request(op: u32, f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(op);
    f(&mut w);
    w.into_bytes()
}

/// Send a request to process `pid`'s agent and read its reply: the result
/// and the rest of the reply. The agent answers at once.
fn call(pid: i32, req: &[u8]) -> Result<(i64, Vec<u8>), i64> {
    let mut conns = CONNS.lock().unwrap_or_else(|e| e.into_inner());
    let fd = match conns.get(&pid) {
        Some(&fd) => fd,
        None => {
            let fd = agent::connect(pid)?;
            conns.insert(pid, fd);
            fd
        }
    };
    let got = if agent::send(fd, req) {
        agent::recv(fd)
    } else {
        None
    };
    let Some(b) = got else {
        // The process is gone (a new one of its pid has a new socket).
        conns.remove(&pid);
        agent::close(fd);
        return Err(-(ESRCH as i64));
    };
    let mut r = Reader::new(&b);
    let result = r.i64();
    Ok((result, b[8.min(b.len())..].to_vec()))
}

/// A call whose failure is its result.
fn call_result(pid: i32, req: &[u8]) -> Result<Vec<u8>, i64> {
    match call(pid, req)? {
        (r, _) if r < 0 => Err(r),
        (_, rest) => Ok(rest),
    }
}

/// The process of thread `tid` for a request of this process, which
/// cannot trace its own threads.
fn tracee_process(tid: i32, seize: bool) -> Result<i32, i64> {
    let pid = thread::owner(tid);
    // SAFETY: trivial.
    if tid <= 0 || pid == unsafe { libc::getpid() } {
        return Err(-(if seize && tid > 0 { EPERM } else { ESRCH } as i64));
    }
    pidns::check(pid)
}

/// ptrace(request, pid, addr, data).
pub fn ptrace(a: [u64; 6]) -> i64 {
    let mut a=a;
    a[1]=match pidns::syscall_pid(a[1] as i32){Ok(pid)=>pid as u64,Err(error)=>return error};
    let (req, tid, addr, data) = (a[0], a[1] as i32, a[2], a[3]);
    let pid = match tracee_process(tid, req == PTRACE_SEIZE) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let r = match req {
        PTRACE_SEIZE => seize(pid, tid, addr, data),
        PTRACE_INTERRUPT => call_result(pid, &request(agent::INTERRUPT, |w| w.i32(tid))),
        // The tracee checks the attach first, then the argument (EIO for
        // a bad signal, EINVAL for bad options), as `ptrace_check_attach`
        // precedes the request on Linux.
        PTRACE_CONT | PTRACE_DETACH => call_result(
            pid,
            &request(agent::RESUME, |w| {
                w.i32(tid);
                w.bool(req == PTRACE_DETACH);
                w.bool(data <= 64);
            }),
        ),
        PTRACE_SETOPTIONS => call_result(
            pid,
            &request(agent::OPTIONS, |w| {
                w.i32(tid);
                w.u64(data);
                w.bool(data & !OPTIONS == 0);
            }),
        ),
        PTRACE_GETREGSET => getregset(pid, tid, addr as u32, data),
        PTRACE_GETEVENTMSG => {
            call_result(pid, &request(agent::MESSAGE, |w| w.i32(tid))).map(|b| {
                // SAFETY: the guest's unsigned long.
                unsafe { (data as *mut u64).write_unaligned(Reader::new(&b).u64()) };
                Vec::new()
            })
        }
        PTRACE_PEEKTEXT | PTRACE_PEEKDATA => call_result(
            pid,
            &request(agent::PEEK, |w| {
                w.i32(tid);
                w.u64(addr);
            }),
        )
        .map(|b| {
            // SAFETY: the guest's word (the raw syscall stores the word
            // through `data`).
            unsafe { (data as *mut u64).write_unaligned(Reader::new(&b).u64()) };
            Vec::new()
        }),
        _ => call_result(pid, &request(agent::STOPPED, |w| w.i32(tid))).and(Err(-(EIO as i64))),
    };
    r.map_or_else(|e| e, |_| 0)
}

/// `ptrace_attach` for PTRACE_SEIZE: the options, then `__ptrace_may_access`
/// with the caller's real ids (the tracee checks that it is dumpable).
fn seize(pid: i32, tid: i32, addr: u64, options: u64) -> Result<Vec<u8>, i64> {
    if addr != 0 {
        return Err(-(EIO as i64));
    }
    if options & !OPTIONS != 0 {
        return Err(-(EINVAL as i64));
    }
    if !cred::may_ptrace(pid, false) {
        return Err(-(EPERM as i64));
    }
    TRACER.store(true, Relaxed);
    call_result(
        pid,
        &request(agent::SEIZE, |w| {
            w.i32(tid);
            w.u64(options);
            w.bool(cred::capable(CAP_SYS_PTRACE));
        }),
    )
}

/// PTRACE_GETREGSET(nt, iov): at most `iov_len` bytes of the regset, in
/// whole elements, and the length written back.
fn getregset(pid: i32, tid: i32, nt: u32, iov: u64) -> Result<Vec<u8>, i64> {
    let set = call_result(
        pid,
        &request(agent::REGSET, |w| {
            w.i32(tid);
            w.u32(nt);
        }),
    )?;
    let set = Reader::new(&set).bytes();
    // Element sizes of the regsets (NT_PRFPREG's is a u32).
    let unit = if nt == 2 { 4 } else { 8 };
    // SAFETY: the guest's struct iovec.
    let [base, len] = unsafe { (iov as *const [u64; 2]).read_unaligned() };
    if len % unit != 0 {
        return Err(-(EINVAL as i64));
    }
    let n = (len as usize).min(set.len());
    // SAFETY: the guest's buffer of `len` bytes and its iovec.
    unsafe {
        std::ptr::copy_nonoverlapping(set.as_ptr(), base as *mut u8, n);
        (iov as *mut [u64; 2]).write_unaligned([base, n as u64]);
    }
    Ok(Vec::new())
}

/// `wait4` for thread `tid` by its tracer: the status (None with `nohang`
/// when there is none yet). None when this process does not trace `tid`,
/// or `tid` ended as this process's child, which `wait4` then reaps.
pub fn wait(tid: i32, nohang: bool) -> Option<Result<Option<i32>, i64>> {
    if !TRACER.load(Relaxed) {
        return None;
    }
    let pid = tracee_process(tid, false).ok()?;
    let st = loop {
        let (r, rest) = call(pid, &request(agent::WAIT, |w| w.i32(tid))).ok()?;
        match r {
            1 => break Reader::new(&rest).i32(),
            0 if nohang => return Some(Ok(None)),
            0 => match notify(pid, tid) {
                Ok(Some(st)) => break st,
                Ok(None) => {}
                Err(e) if e == -(EINTR as i64) => return Some(Err(e)),
                Err(_) => return None,
            },
            _ => return None,
        }
    };
    // A child's end: reaped by `wait4` itself, with its rusage.
    if tid == pid && pidns::is_child(pid) && st & 0x7f != 0x7f {
        return None;
    }
    Some(Ok(Some(st)))
}

/// Block until thread `tid` has a status: None for a stop (for WAIT to
/// take), or its end's status; EINTR when a signal comes first.
fn notify(pid: i32, tid: i32) -> Result<Option<i32>, i64> {
    let fd = agent::connect(pid)?;
    let got = if agent::send(fd, &request(agent::NOTIFY, |w| w.i32(tid))) {
        let wake = move || {
            // SAFETY: our socket: the blocked read returns.
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
        };
        crate::sys::signal::interruptible(&wake, || agent::recv(fd))
    } else {
        Some(None)
    };
    agent::close(fd);
    let interrupted = || thread::current().is_some_and(crate::sys::signal::interrupted);
    match got {
        None => Err(-(EINTR as i64)),
        Some(None) if interrupted() => Err(-(EINTR as i64)),
        Some(None) => Err(-(ESRCH as i64)),
        Some(Some(b)) => {
            let mut r = Reader::new(&b);
            match r.i64() {
                0 => Ok(None),
                1 => Ok(Some(r.i32())),
                e => Err(e),
            }
        }
    }
}

/// `process_vm_readv`/`writev` of another process `pid`: its ranges
/// `remote`, and our buffers `local`. The count of bytes moved.
pub fn remote_vm(write: bool, pid: i32, local: &[(u64, u64)], remote: &[(u64, u64)]) -> i64 {
    let pid=match pidns::syscall_pid(pid){Ok(pid)=>pid,Err(error)=>return error};
    if let Err(e) = pidns::check(pid) {
        return e;
    }
    // PTRACE_MODE_ATTACH_REALCREDS.
    if !cred::may_ptrace(pid, false) {
        return -(EPERM as i64);
    }
    // As much of `remote` as `local` holds.
    let mut room = local.iter().map(|l| l.1).sum::<u64>();
    let ranges: Vec<(u64, u64)> = remote
        .iter()
        .map(|&(a, n)| {
            let n = n.min(room);
            room -= n;
            (a, n)
        })
        .filter(|r| r.1 > 0)
        .collect();
    let data = if write { gather(local) } else { Vec::new() };
    let req = request(agent::VM, |w| {
        w.bool(write);
        w.seq(ranges.iter(), |w, &(a, n)| {
            w.u64(a);
            w.u64(n);
        });
        w.bytes(&data);
    });
    let (done, rest) = match call(pid, &req) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if done == 0 && !ranges.is_empty() {
        return -(libc::EFAULT as i64);
    }
    if !write {
        scatter(&Reader::new(&rest).bytes(), local);
    }
    done
}

fn gather(local: &[(u64, u64)]) -> Vec<u8> {
    let mut v = Vec::new();
    for &(a, n) in local {
        // SAFETY: the guest's buffers.
        v.extend_from_slice(unsafe { std::slice::from_raw_parts(a as *const u8, n as usize) });
    }
    v
}

fn scatter(mut data: &[u8], local: &[(u64, u64)]) {
    for &(a, n) in local {
        let k = (n as usize).min(data.len());
        // SAFETY: the guest's buffer of `n` bytes.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), a as *mut u8, k) };
        data = &data[k..];
    }
}

/// Queue `info` for thread `tid` (0: the process) of another process `pid`,
/// whose permission the caller checked.
pub fn remote_signal(pid: i32, tid: i32, info: &crate::sys::sigframe::Siginfo) -> i64 {
    let req = request(agent::SIGNAL, |w| {
        w.i32(tid);
        w.i32(info.signo);
        w.i32(info.code);
        for &f in &info.fields {
            w.u64(f);
        }
    });
    match call(pid, &req) {
        Ok((r, _)) => r,
        Err(e) => e,
    }
}

/// Another process's `/proc/<pid>/maps`, for a caller that may read it
/// (PTRACE_MODE_READ_FSCREDS).
pub fn remote_maps(pid: i32) -> Option<String> {
    if !cred::may_ptrace(pid, true) {
        return None;
    }
    let b = call_result(pid, &request(agent::MAPS, |_| {})).ok()?;
    Some(Reader::new(&b).str())
}

/// Another process's open fds, as `/proc/<pid>/fd` lists them.
pub fn remote_fds(pid: i32) -> Option<Vec<i32>> {
    if !cred::may_ptrace(pid, true) {
        return None;
    }
    let b = call_result(pid, &request(agent::FDS, |_| {})).ok()?;
    Some(Reader::new(&b).seq(|r| r.i32()))
}

/// Public netns metadata, not process memory or remote descriptor access.
pub(crate) fn remote_net_sockets(pid: i32) -> Result<Vec<super::net::SocketMetadata>, crate::errno::Errno> {
    if !pidns::contains(pid) { return Err(crate::errno::ESRCH); }
    let bytes = call_result(pid, &request(agent::NET_SOCKET_METADATA, |_| {}))
        .map_err(|error| (-error) as crate::errno::Errno)?;
    Ok(Reader::new(&bytes).seq(|r| super::net::SocketMetadata {
        fd: r.i32(), uid: r.u32(), inode: r.u64(), cookie: r.u64(),
        local: r.opt(|r| r.bytes().try_into().expect("net local address record")),
        peer: r.opt(|r| r.bytes().try_into().expect("net peer address record")), port_zero: r.bool(), probes_known: r.bool(),
    }))
}

/// The target of another process's `/proc/<pid>/fd/<fd>`.
pub fn remote_fd_link(pid: i32, fd: i32) -> Option<String> {
    if !cred::may_ptrace(pid, true) {
        return None;
    }
    let b = call_result(pid, &request(agent::FD_LINK, |w| w.i32(fd))).ok()?;
    Some(Reader::new(&b).str())
}
