//! The agent: how other processes reach this one's threads, memory and
//! `/proc` state, which Darwin keeps from them.
//!
//! A process of a pid namespace listens on `<pid>.sock` in the namespace's
//! `by-pid` directory, served by a host thread, once a peer asks for it:
//! the peer rings with SIGINFO, which Linux lacks (so no guest sends it)
//! and which a GCD signal source here takes without a thread of its own
//! ([`start`], [`connect`]). Making the socket costs file system work that
//! every process start and fork would otherwise pay. A caller sends a
//! request and reads its reply, both framed ([`send`], [`recv`]) and
//! encoded as fork state is. A connection that ends ends every trace its
//! peer held here: the tracer exited (or execed, which Darwin cannot run
//! with the connection open).

use std::sync::Mutex;
use std::time::Duration;

use super::tracee::{self, Traced};
use crate::sys::fork_state::{Reader, Writer};
use crate::sys::{fdtab, misc, procfs, smaps, thread};

pub(super) const SEIZE: u32 = 1;
pub(super) const INTERRUPT: u32 = 2;
pub(super) const RESUME: u32 = 3;
pub(super) const OPTIONS: u32 = 4;
pub(super) const REGSET: u32 = 5;
pub(super) const MESSAGE: u32 = 6;
pub(super) const PEEK: u32 = 7;
pub(super) const WAIT: u32 = 8;
pub(super) const NOTIFY: u32 = 9;
pub(super) const VM: u32 = 10;
pub(super) const MAPS: u32 = 11;
pub(super) const FDS: u32 = 12;
pub(super) const FD_LINK: u32 = 13;
pub(super) const NET_SOCKET_METADATA: u32 = 16;
pub(super) const STOPPED: u32 = 14;
pub(super) const SIGNAL: u32 = 15;
pub(super) const SMAPS: u32 = 17;
pub(super) const RLIMIT: u32 = 18;
pub(super) const MEM_OPEN: u32 = 19;
pub(super) const MEM_IO: u32 = 20;
pub(super) const PRIORITY: u32 = 22;
pub(super) const CGROUP_POLICY: u32 = 23;
pub(super) const SCHED: u32 = 24;

const NT_PRSTATUS: u32 = 1;
const NT_PRFPREG: u32 = 2;
const NT_ARM_TLS: u32 = 0x401;
/// The largest frame either side accepts.
const MAX_FRAME: usize = 256 << 20;
/// How often a blocked NOTIFY looks whether its caller left.
const NOTIFY_POLL: Duration = Duration::from_secs(1);
/// How long a caller rings a process whose socket is not there yet.
const RING_FOR: Duration = Duration::from_secs(2);

#[repr(C)]
struct DispatchSourceType {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    static _dispatch_source_type_signal: DispatchSourceType;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut libc::c_void;
    fn dispatch_source_create(
        ty: *const DispatchSourceType,
        handle: usize,
        mask: usize,
        queue: *mut libc::c_void,
    ) -> *mut libc::c_void;
    fn dispatch_source_set_event_handler_f(
        source: *mut libc::c_void,
        handler: extern "C" fn(*mut libc::c_void),
    );
    fn dispatch_resume(object: *mut libc::c_void);
    fn pthread_fchdir_np(fd: i32) -> i32;
    fn mach_vm_read_overwrite(t: libc::mach_port_t, a: u64, s: u64, d: u64, o: *mut u64) -> i32;
    fn mach_vm_write(t: libc::mach_port_t, a: u64, d: usize, n: u32) -> i32;
    static mach_task_self_: libc::mach_port_t;
}

/// The layer's fds of this module: they stay with this process
/// ([`fork_save`]).
static OWN: Mutex<Vec<i32>> = Mutex::new(Vec::new());
/// Whether this process listens.
static LISTENING: Mutex<bool> = Mutex::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn errno() -> i64 {
    crate::errno::last() as i64
}

// ---- framing ------------------------------------------------------------------

fn write_all(fd: i32, mut b: &[u8]) -> bool {
    while !b.is_empty() {
        // SAFETY: writing our buffer.
        let n = unsafe { libc::write(fd, b.as_ptr().cast(), b.len()) };
        if n > 0 {
            b = &b[n as usize..];
        } else if n == 0 || crate::errno::last() != crate::errno::EINTR {
            return false;
        }
    }
    true
}

fn read_exact(fd: i32, b: &mut [u8]) -> bool {
    let mut got = 0;
    while got < b.len() {
        // SAFETY: reading into our buffer.
        let n = unsafe { libc::read(fd, b[got..].as_mut_ptr().cast(), b.len() - got) };
        if n > 0 {
            got += n as usize;
        } else if n == 0 || crate::errno::last() != crate::errno::EINTR {
            return false;
        }
    }
    true
}

pub(super) fn send(fd: i32, body: &[u8]) -> bool {
    let mut b = (body.len() as u32).to_le_bytes().to_vec();
    b.extend_from_slice(body);
    write_all(fd, &b)
}

/// One frame; None when the connection ended.
pub(super) fn recv(fd: i32) -> Option<Vec<u8>> {
    let mut n = [0u8; 4];
    if !read_exact(fd, &mut n) {
        return None;
    }
    let n = u32::from_le_bytes(n) as usize;
    if n > MAX_FRAME {
        return None;
    }
    let mut b = vec![0u8; n];
    read_exact(fd, &mut b).then_some(b)
}

/// A reply: the result, then what `f` writes.
fn reply(result: i64, f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::default();
    w.i64(result);
    f(&mut w);
    w.into_bytes()
}

fn err(e: i32) -> Vec<u8> {
    reply(-(e as i64), |_| {})
}

// ---- sockets ------------------------------------------------------------------

fn sockaddr(name: &str) -> (libc::sockaddr_un, libc::socklen_t) {
    // SAFETY: plain data.
    let mut a: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    a.sun_family = libc::AF_UNIX as u8;
    for (d, &s) in a.sun_path.iter_mut().zip(name.as_bytes()) {
        *d = s as libc::c_char;
    }
    let len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + name.len() + 1;
    a.sun_len = len as u8;
    (a, len as libc::socklen_t)
}

/// `f` on socket `s` and the address of `name` in the `by-pid` directory
/// `d`, named relative to it (a socket path is short): the calling
/// thread works there meanwhile.
fn at(
    d: &std::fs::File,
    s: i32,
    name: &str,
    f: unsafe extern "C" fn(i32, *const libc::sockaddr, libc::socklen_t) -> i32,
) -> i32 {
    let (a, len) = sockaddr(name);
    // SAFETY: a thread-local working directory, restored to the process's.
    unsafe {
        if pthread_fchdir_np(dirfd(d)) != 0 {
            return -1;
        }
        let r = f(s, (&a as *const libc::sockaddr_un).cast(), len);
        pthread_fchdir_np(-1);
        r
    }
}

/// The `by-pid` directory, open.
fn dir() -> Option<std::fs::File> {
    std::fs::File::open(crate::sys::cred::by_pid_dir()?).ok()
}

fn dirfd(d: &std::fs::File) -> i32 {
    std::os::fd::AsRawFd::as_raw_fd(d)
}

fn socket() -> Result<i32, i64> {
    // SAFETY: a fresh socket; SIGPIPE would reach the guest.
    unsafe {
        let s = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if s < 0 {
            return Err(-errno());
        }
        let one: i32 = 1;
        libc::setsockopt(
            s,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&one as *const i32).cast(),
            4,
        );
        Ok(fdtab::hide(s))
    }
}

pub(super) fn close(fd: i32) {
    lock(&OWN).retain(|&f| f != fd);
    fdtab::unhide(fd);
    // SAFETY: one of this module's fds.
    unsafe { libc::close(fd) };
}

/// A connection to process `pid`'s agent, ringing it until it listens.
pub(super) fn connect(pid: i32) -> Result<i32, i64> {
    let esrch = -(crate::errno::ESRCH as i64);
    let d = dir().ok_or(esrch)?;
    let name = format!("{pid}.sock");
    let deadline = std::time::Instant::now() + RING_FOR;
    let mut pause = Duration::from_micros(200);
    loop {
        let s = socket()?;
        if at(&d, s, &name, libc::connect) == 0 {
            lock(&OWN).push(s);
            return Ok(s);
        }
        fdtab::unhide(s);
        // SAFETY: our socket; then ringing a process of the namespace.
        unsafe {
            libc::close(s);
            if !crate::sys::pidns::running(pid) || libc::kill(pid, libc::SIGINFO) != 0 {
                return Err(esrch);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(esrch);
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(20));
    }
}

/// A listening socket named `name` in the `by-pid` directory.
fn listen(d: &std::fs::File, name: &str) -> Option<i32> {
    let s = socket().ok()?;
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: a name in our directory, and our socket.
    let ok = unsafe {
        libc::unlinkat(dirfd(d), c.as_ptr(), 0);
        at(d, s, name, libc::bind) == 0 && libc::listen(s, 64) == 0
    };
    if !ok {
        fdtab::unhide(s);
        // SAFETY: our socket.
        unsafe { libc::close(s) };
        return None;
    }
    Some(s)
}

/// Serve on listener `l` from a host thread of this process.
fn serve(l: i32) {
    lock(&OWN).push(l);
    let started = std::thread::Builder::new()
        .name("linux-abi-ptrace".into())
        .spawn(move || accept_loop(l));
    if started.is_err() {
        crate::diag!("[linux-abi] ptrace: no agent thread");
    }
}

/// Let peers ring this process for its agent, when it belongs to a pid
/// namespace.
pub fn start() {
    if crate::sys::cred::by_pid_dir().is_none() {
        return;
    }
    // SAFETY: a signal source on a global queue, kept for the process's
    // life; its handler is a plain function.
    unsafe {
        let s = dispatch_source_create(
            &_dispatch_source_type_signal,
            libc::SIGINFO as usize,
            0,
            dispatch_get_global_queue(0, 0),
        );
        if s.is_null() {
            return;
        }
        dispatch_source_set_event_handler_f(s, rung);
        dispatch_resume(s);
    }
}

/// A peer rang: listen, once.
extern "C" fn rung(_: *mut libc::c_void) {
    let mut listening = lock(&LISTENING);
    if *listening {
        return;
    }
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    if let Some(d) = dir()
        && let Some(l) = listen(&d, &format!("{pid}.sock"))
    {
        serve(l);
        *listening = true;
    }
}

fn accept_loop(l: i32) {
    loop {
        // SAFETY: accepting on our listener.
        let c = unsafe { libc::accept(l, std::ptr::null_mut(), std::ptr::null_mut()) };
        if c < 0 {
            if crate::errno::last() == crate::errno::EINTR {
                continue;
            }
            return;
        }
        let c = fdtab::hide(c);
        lock(&OWN).push(c);
        let started = std::thread::Builder::new()
            .name("linux-abi-ptrace-conn".into())
            .spawn(move || connection(c));
        if started.is_err() {
            close(c);
        }
    }
}

fn peer_pid(fd: i32) -> i32 {
    let mut pid = 0i32;
    let mut len = 4 as libc::socklen_t;
    // SAFETY: an int option into our local.
    unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut i32).cast(),
            &mut len,
        )
    };
    pid
}

fn connection(fd: i32) {
    let peer = peer_pid(fd);
    while let Some(req) = recv(fd) {
        let mut r = Reader::new(&req);
        let op = r.u32();
        if op == NOTIFY {
            notify(fd, peer, r.i32());
            close(fd);
            return;
        }
        if op == WAIT {
            if !wait_status(fd, peer, r.i32()) {
                break;
            }
            continue;
        }
        if !send(fd, &handle(op, &mut r, peer)) {
            break;
        }
    }
    close(fd);
    tracee::tracer_gone(peer);
}

// ---- requests -----------------------------------------------------------------

fn handle(op: u32, r: &mut Reader, peer: i32) -> Vec<u8> {
    use crate::errno::{EINVAL, EIO, ENOENT, EPERM, ESRCH};
    match op {
        CGROUP_POLICY => {
            let policies = r.seq(|r| {
                (
                    r.i32(),
                    crate::sys::cgroup::Policy {
                        min: r.i32(),
                        max: r.i32(),
                        fair_max: r.i32(),
                        io: r.i32(),
                    },
                )
            });
            if !r.ok()
                || policies.iter().any(|(tid, p)| {
                    thread::owner(*tid) != unsafe { libc::getpid() }
                        || ![0, 9, 17, 21, 25, 33].contains(&p.min)
                        || ![9, 17, 21, 25, 33].contains(&p.max)
                        || ![9, 17, 33].contains(&p.fair_max)
                        || p.min > p.max
                        || ![0, 1, 3, 4].contains(&p.io)
                })
            {
                return err(EINVAL);
            }
            match crate::sys::cgroup::apply_policy(&policies) {
                Ok(before) => reply(0, |w| {
                    w.seq(before.iter(), |w, (tid, p)| {
                        w.i32(*tid);
                        w.i32(p.min);
                        w.i32(p.max);
                        w.i32(p.fair_max);
                        w.i32(p.io);
                    })
                }),
                Err(e) => err(e),
            }
        }
        SEIZE => {
            let (tid, options, privileged) = (r.i32(), r.u64(), r.bool());
            let mut g = tracee::lock();
            if thread::find(tid).is_none() {
                return err(ESRCH);
            }
            if g.traced.contains_key(&tid) || !(misc::dumpable() || privileged) {
                return err(EPERM);
            }
            g.attach(
                tid,
                Traced {
                    tracer: peer,
                    options,
                    trap: false,
                    stopped: None,
                    report: None,
                    message: 0,
                },
            );
            reply(0, |_| {})
        }
        INTERRUPT => {
            let tid = r.i32();
            let mut g = tracee::lock();
            let Some(t) = g.of(tid, peer, false) else {
                return err(ESRCH);
            };
            if t.stopped.is_none() {
                t.trap = true;
                drop(g);
                crate::sys::signal::poke(tid);
            }
            reply(0, |_| {})
        }
        RESUME => {
            let (tid, detach, valid) = (r.i32(), r.bool(), r.bool());
            let mut g = tracee::lock();
            let Some(t) = g.of(tid, peer, true) else {
                return err(ESRCH);
            };
            if !valid {
                return err(EIO);
            }
            // No signal-delivery-stops are made, so a resume signal has
            // no stop to replace the signal of (Linux ignores it after an
            // event stop).
            t.stopped = None;
            t.report = None;
            if detach {
                g.detach(tid);
            }
            tracee::CHANGED.notify_all();
            reply(0, |_| {})
        }
        OPTIONS => {
            let (tid, options, valid) = (r.i32(), r.u64(), r.bool());
            match tracee::lock().of(tid, peer, true) {
                Some(_) if !valid => err(EINVAL),
                Some(t) => {
                    t.options = options;
                    reply(0, |_| {})
                }
                None => err(ESRCH),
            }
        }
        STOPPED => match tracee::lock().of(r.i32(), peer, true) {
            Some(_) => reply(0, |_| {}),
            None => err(ESRCH),
        },
        REGSET => {
            let (tid, nt) = (r.i32(), r.u32());
            let mut g = tracee::lock();
            let Some(regs) = g.of(tid, peer, true).and_then(|t| t.stopped.as_ref()) else {
                return err(ESRCH);
            };
            let set = match nt {
                NT_PRSTATUS => &regs.gp,
                NT_PRFPREG => &regs.fp,
                NT_ARM_TLS => &regs.tls,
                _ => return err(EINVAL),
            };
            reply(0, |w| w.bytes(set))
        }
        MESSAGE => match tracee::lock().of(r.i32(), peer, true) {
            Some(t) => {
                let m = t.message;
                reply(0, |w| w.u64(m))
            }
            None => err(ESRCH),
        },
        PEEK => {
            let (tid, addr) = (r.i32(), r.u64());
            if tracee::lock().of(tid, peer, true).is_none() {
                return err(ESRCH);
            }
            let mut word = [0u8; 8];
            if read_mem(addr, &mut word) {
                reply(0, |w| w.u64(u64::from_le_bytes(word)))
            } else {
                err(EIO)
            }
        }
        MEM_OPEN => {
            if !(misc::dumpable() || r.bool()) {
                return err(crate::errno::EACCES);
            }
            reply(0, |w| w.bytes(&crate::sys::proc_mem::epoch()))
        }
        MEM_IO => {
            let Ok(epoch) = r.bytes().try_into() else {
                return err(EINVAL);
            };
            let (address, count, write, data) = (r.u64(), r.u32() as usize, r.bool(), r.bytes());
            if count > crate::sys::mem::PAGE as usize || write && data.len() != count {
                return err(EINVAL);
            }
            let (n, bytes) = crate::sys::proc_mem::transfer(epoch, address, count, write, &data);
            reply(n, |w| w.bytes(&bytes))
        }
        VM => vm(r),
        SIGNAL => {
            let tid = r.i32();
            let mut info = crate::sys::sigframe::Siginfo::new(r.i32(), r.i32());
            for f in &mut info.fields {
                *f = r.u64();
            }
            reply(crate::sys::signal::from_peer(tid, info), |_| {})
        }
        MAPS => reply(0, |w| w.str(&procfs::maps())),
        SMAPS => reply(0, |w| w.str(&smaps::smaps(r.bool()))),
        PRIORITY => {
            let (tid, nice) = (r.i32(), r.opt(|r| r.i32()));
            if !crate::sys::pidns::contains(peer) {
                return err(ESRCH);
            }
            let Some(caller) = crate::sys::cred::identity_of(peer) else {
                return err(ESRCH);
            };
            reply(crate::sys::cred::own_priority(tid, nice, &caller), |_| {})
        }
        SCHED => {
            let (tid, nr, policy, prio) = (r.i32(), r.u64(), r.i32(), r.i32());
            if !crate::sys::pidns::contains(peer) {
                return err(ESRCH);
            }
            let Some(caller) = crate::sys::cred::identity_of(peer) else {
                return err(ESRCH);
            };
            reply(
                crate::sys::process::own_sched(tid, nr, policy, prio, &caller),
                |_| {},
            )
        }
        RLIMIT => {
            let res = r.u32();
            let new = [r.u64(), r.u64()];
            match crate::sys::cred::own_rlimit(res, Some(new), r.bool()) {
                Ok(old) => reply(0, |w| {
                    w.u64(old[0]);
                    w.u64(old[1]);
                }),
                Err(e) => reply(e, |_| {}),
            }
        }
        FDS => {
            let fds = crate::sys::fd_visibility::visible();
            reply(0, |w| w.seq(fds.into_iter(), |w, fd| w.i32(fd)))
        }
        NET_SOCKET_METADATA => {
            if !crate::sys::pidns::contains(peer) {
                return err(EPERM);
            }
            let sockets = match crate::sys::net::proc_metadata() {
                Ok(sockets) => sockets,
                Err(error) => return err(error),
            };
            reply(0, |w| {
                w.seq(sockets.into_iter(), |w, owner| {
                    w.i32(owner.fd);
                    w.u32(owner.uid);
                    w.u64(owner.inode);
                    w.u64(owner.cookie);
                    w.opt(owner.local, |w, value| w.bytes(&value));
                    w.opt(owner.peer, |w, value| w.bytes(&value));
                    w.bool(owner.port_zero);
                    w.bool(owner.probes_known);
                })
            })
        }
        FD_LINK => match procfs::fd_link(r.i32()) {
            Some(l) => reply(0, |w| w.str(&l)),
            None => err(ENOENT),
        },
        _ => err(EINVAL),
    }
}

/// Deliver before consuming a status: removing the last exit report lets
/// the process exit, taking this socket with it.
fn wait_status(fd: i32, peer: i32, tid: i32) -> bool {
    let mut g = tracee::lock();
    if let Some(t) = g.of(tid, peer, false) {
        let sent = send(
            fd,
            &match t.report {
                Some(st) => reply(1, |w| w.i32(st)),
                None => reply(0, |_| {}),
            },
        );
        if sent {
            t.report = None;
        }
        return sent;
    }
    match g.ended.get(&tid) {
        Some(e) if e.tracer == peer => {
            let sent = send(fd, &reply(1, |w| w.i32(e.status)));
            if sent {
                g.ended.remove(&tid);
                tracee::CHANGED.notify_all();
            }
            sent
        }
        _ => send(fd, &err(crate::errno::ESRCH)),
    }
}

/// Wake once thread `tid` has a status for `peer`. Only WAIT consumes it:
/// an interrupted NOTIFY must leave the exit report available for retry.
fn notify(fd: i32, peer: i32, tid: i32) {
    let mut g = tracee::lock();
    loop {
        let r = match g.traced.get(&tid) {
            Some(t) if t.tracer != peer => err(crate::errno::ESRCH),
            Some(t) if t.report.is_some() => reply(0, |_| {}),
            Some(_) => {
                g = tracee::wait(g, NOTIFY_POLL);
                if caller_left(fd) {
                    return;
                }
                continue;
            }
            None => match g.ended.get(&tid) {
                Some(e) if e.tracer == peer => reply(0, |_| {}),
                _ => err(crate::errno::ESRCH),
            },
        };
        drop(g);
        send(fd, &r);
        return;
    }
}

fn caller_left(fd: i32) -> bool {
    let mut b = 0u8;
    // SAFETY: peeking one byte of our socket without blocking.
    let n = unsafe {
        libc::recv(
            fd,
            (&mut b as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    n == 0 || n < 0 && crate::errno::last() != crate::errno::EAGAIN
}

fn read_mem(addr: u64, buf: &mut [u8]) -> bool {
    let mut got = 0u64;
    // SAFETY: a fault-safe copy from our own task into our buffer.
    unsafe {
        mach_vm_read_overwrite(
            mach_task_self_,
            addr,
            buf.len() as u64,
            buf.as_mut_ptr() as u64,
            &mut got,
        ) == 0
    }
}

/// process_vm_readv/writev on this process: the remote ranges in order,
/// up to the first that fails.
fn vm(r: &mut Reader) -> Vec<u8> {
    let write = r.bool();
    let ranges = r.seq(|r| (r.u64(), r.u64()));
    let data = r.bytes();
    let mut out = Vec::new();
    let mut done = 0usize;
    for (addr, len) in ranges {
        let len = len as usize;
        let ok = if write {
            let Some(src) = data.get(done..done + len) else {
                break;
            };
            // SAFETY: writing our buffer into our own task.
            unsafe { mach_vm_write(mach_task_self_, addr, src.as_ptr() as usize, len as u32) == 0 }
        } else {
            let mut b = vec![0u8; len];
            let ok = read_mem(addr, &mut b);
            if ok {
                out.extend_from_slice(&b);
            }
            ok
        };
        if !ok {
            break;
        }
        done += len;
    }
    reply(done as i64, |w| w.bytes(&out))
}

// ---- fork ---------------------------------------------------------------------

/// Fork: this process's sockets, which the child closes.
pub fn fork_save(w: &mut Writer) {
    let own = lock(&OWN).clone();
    w.seq(own.into_iter(), |w, fd| {
        w.retain_private(fd);
        w.i32(fd)
    });
}

pub fn fork_restore(r: &mut Reader) {
    for fd in r.seq(|r| r.i32()) {
        if let Err(error) = fdtab::close_fork_private(fd) {
            crate::diag!("fork ptrace private close: errno {error}");
            r.invalidate();
            return;
        }
    }
    start();
}

/// The path of process `pid`'s socket, removed with its record.
pub fn socket_path(pid: i32) -> Option<std::path::PathBuf> {
    Some(crate::sys::cred::by_pid_dir()?.join(format!("{pid}.sock")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_fit_and_end_in_nul() {
        let (a, len) = sockaddr("12345.sock");
        assert_eq!(len as usize, 2 + 10 + 1);
        assert_eq!(a.sun_path[10], 0);
    }

    #[test]
    fn exited_thread_keeps_its_route_until_wait_delivers() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc::{self, RecvTimeoutError};

        let peer = unsafe { libc::getpid() };
        let end_thread = |tracer| {
            let reservation = crate::sys::pidns::tids::allocate().unwrap();
            let tid = reservation.tid;
            tracee::lock().attach(
                tid,
                Traced {
                    tracer,
                    options: 0,
                    trap: false,
                    stopped: None,
                    report: None,
                    message: 0,
                },
            );
            tracee::thread_ended(tid, 3 << 8, Some(reservation));
            tid
        };
        let tid = end_thread(peer);
        let (done, exited) = mpsc::channel();
        let ending = std::thread::spawn(move || {
            tracee::process_ending(0);
            done.send(()).unwrap();
        });
        assert_eq!(
            exited.recv_timeout(Duration::from_millis(300)),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(thread::owner(tid), peer);

        // Notification is only a wakeup: a cancelled blocking receive
        // must not consume the exit report.
        let (writer, reader) = UnixStream::pair().unwrap();
        notify(writer.as_raw_fd(), peer, tid);
        assert_eq!(Reader::new(&recv(reader.as_raw_fd()).unwrap()).i64(), 0);
        assert_eq!(thread::owner(tid), peer);

        // A failed status write also leaves it available for another wait.
        writer.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(!wait_status(writer.as_raw_fd(), peer, tid));
        assert!(tracee::lock().ended.contains_key(&tid));
        let (writer, reader) = UnixStream::pair().unwrap();
        assert!(wait_status(writer.as_raw_fd(), peer, tid));
        let bytes = recv(reader.as_raw_fd()).unwrap();
        let mut reply = Reader::new(&bytes);
        assert_eq!(reply.i64(), 1);
        assert_eq!(reply.i32(), 3 << 8);
        exited.recv_timeout(Duration::from_secs(2)).unwrap();
        ending.join().unwrap();
        assert_eq!(thread::owner(tid), 0);

        // An automatic attach may never get a connection. Such a tracer's
        // death must still release the report and allow process teardown.
        let mut tracer = std::process::Command::new("/bin/sleep")
            .arg("0.3")
            .spawn()
            .unwrap();
        let tid = end_thread(tracer.id() as i32);
        tracee::process_ending(0);
        assert!(tracer.wait().unwrap().success());
        assert_eq!(thread::owner(tid), 0);
        assert!(tracee::lock().ended.is_empty());
    }
}
