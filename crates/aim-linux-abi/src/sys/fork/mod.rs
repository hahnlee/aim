//! fork and vfork: `clone` without CLONE_VM, and CLONE_VFORK.
//!
//! A guest process is a Darwin process, and a guest fork makes a new one
//! by spawning a fresh `linux-run` that takes over a copy-on-write snapshot
//! of the guest memory and the layer's state ([`spawn`], [`state`];
//! `docs/fork.md`). A Darwin `fork()` is never used: its child cannot reach
//! XPC services, which Metal's shader compiler is. The child resumes the
//! forking thread's registers with 0 in x0, as its main thread
//! ([`child_started`] applies the clone flags there).
//!
//! `vfork` (CLONE_VM | CLONE_VFORK) is a fork whose parent waits until the
//! child execs or exits, which is all POSIX lets a vfork child do. The
//! wait is a close-on-exec pipe the child holds.

pub(crate) mod spawn;
pub(crate) mod state;

use crate::context::GuestContext;
use crate::errno::{self, EINVAL};

const CSIGNAL: u64 = 0xff;
const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_SYSVSEM: u64 = 0x40000;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_PIDFD: u64 = 0x1000;
const CLONE_VFORK: u64 = 0x4000;
const CLONE_PARENT: u64 = 0x8000;
const CLONE_THREAD: u64 = 0x10000;
const CLONE_NEWNS: u64 = 0x20000;
const CLONE_SETTLS: u64 = 0x80000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
const CLONE_NEWCGROUP: u64 = 0x200_0000;
const CLONE_NEWUTS: u64 = 0x400_0000;
const CLONE_NEWIPC: u64 = 0x800_0000;
const CLONE_NEWUSER: u64 = 0x1000_0000;
const CLONE_NEWPID: u64 = 0x2000_0000;
const CLONE_NEWNET: u64 = 0x4000_0000;
const CLONE_NEWTIME: u64 = 0x80;

/// Sharing a process cannot do with another Darwin process, and namespaces
/// (answered unsupported, ADR 0012).
const UNSUPPORTED: u64 = CLONE_FS
    | CLONE_FILES
    | CLONE_SIGHAND
    | CLONE_THREAD
    | CLONE_PARENT
    | CLONE_NEWNS
    | CLONE_NEWCGROUP
    | CLONE_NEWUTS
    | CLONE_NEWIPC
    | CLONE_NEWUSER
    | CLONE_NEWPID
    | CLONE_NEWNET;

/// Whether a `clone` with these flags creates a process (handled here)
/// rather than a thread.
pub fn is_fork(flags: u64) -> bool {
    flags & CLONE_VM == 0
        || flags & (CLONE_VFORK | CLONE_THREAD) == CLONE_VFORK
        || is_own_files_thread(flags)
}

/// A thread with its own file table (CLONE_THREAD without CLONE_FILES):
/// bionic's debuggerd handler runs its dispatch "pseudothread" so, and
/// the pseudothread closes every fd it has. Darwin threads share one
/// table, so it runs as a forked process instead: its file table is its
/// own, its writes to memory stay its own (the handler reads none back),
/// and the parent sees the tid handshake the handler waits on
/// (CLONE_CHILD_SETTID, then CLONE_CHILD_CLEARTID with a futex wake when
/// it ends).
fn is_own_files_thread(flags: u64) -> bool {
    flags & (CLONE_THREAD | CLONE_VM | CLONE_FILES) == CLONE_THREAD | CLONE_VM
}

/// The parent's side of an own-files thread's tid handshake: `tid` at
/// `child_tid` now, and 0 plus a futex wake once the process `pid` ends.
/// The layer reaps it; the guest never forked it.
fn own_files_thread_started(pid: i32, r: &Request) {
    if r.child_tid == 0 {
        return;
    }
    // SAFETY: guest word named by CLONE_CHILD_SETTID/CLEARTID.
    unsafe {
        if r.flags & CLONE_CHILD_SETTID != 0 {
            (r.child_tid as *mut i32).write_volatile(pid);
        }
    }
    if r.flags & CLONE_CHILD_CLEARTID == 0 {
        return;
    }
    let addr = r.child_tid;
    std::thread::spawn(move || {
        let mut status = 0;
        // SAFETY: reaping the process that stands for the thread.
        while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 && errno::last() == errno::EINTR {}
        // SAFETY: as above; the guest waits on this word.
        unsafe { (addr as *mut i32).write_volatile(0) };
        super::futex::wake_one(addr);
    });
}

/// `struct clone_args` flags of a `clone3` call, for [`is_fork`].
pub fn clone3_flags(a: [u64; 6]) -> u64 {
    if a[0] == 0 || a[1] < 64 {
        return 0;
    }
    // SAFETY: guest struct clone_args of at least 64 bytes.
    unsafe { (a[0] as *const u64).read_unaligned() }
}

struct Request {
    flags: u64,
    stack: u64,
    parent_tid: u64,
    child_tid: u64,
    tls: u64,
    /// Where CLONE_PIDFD stores the pidfd.
    pidfd: u64,
}

/// clone(flags, stack, parent_tid, tls, child_tid) (arm64 argument order).
pub fn clone(ctx: &mut GuestContext, a: [u64; 6]) -> i64 {
    let flags = a[0];
    // clone() reports the pidfd through parent_tid, so both cannot be used.
    if flags & CLONE_PIDFD != 0 && flags & CLONE_PARENT_SETTID != 0 {
        return -(EINVAL as i64);
    }
    fork(
        ctx,
        Request {
            flags,
            stack: a[1],
            parent_tid: a[2],
            tls: a[3],
            child_tid: a[4],
            pidfd: a[2],
        },
    )
}

/// clone3(args, size): struct clone_args { flags, pidfd, child_tid,
/// parent_tid, exit_signal, stack, stack_size, tls, ... }.
pub fn clone3(ctx: &mut GuestContext, a: [u64; 6]) -> i64 {
    if a[0] == 0 || a[1] < 64 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest struct clone_args of at least 64 bytes.
    let f = unsafe { (a[0] as *const [u64; 8]).read_unaligned() };
    if f[0] & CSIGNAL != 0 || f[4] > 64 {
        return -(EINVAL as i64);
    }
    fork(
        ctx,
        Request {
            flags: f[0] | f[4],
            stack: if f[5] != 0 { f[5] + f[6] } else { 0 },
            parent_tid: f[3],
            child_tid: f[2],
            tls: f[7],
            pidfd: f[1],
        },
    )
}

fn fork(ctx: &mut GuestContext, mut r: Request) -> i64 {
    let own_files_thread = is_own_files_thread(r.flags);
    if own_files_thread {
        r.flags &= !(CLONE_THREAD | CLONE_SIGHAND | CLONE_VM | CLONE_FS | CLONE_SYSVSEM);
    }
    // The exit signal (CSIGNAL) is always SIGCHLD on Darwin.
    if r.flags & (UNSUPPORTED | CLONE_NEWTIME) != 0 {
        return -(EINVAL as i64);
    }
    let Some(runtime) = super::exec::launch_args() else {
        return -(libc::ENOEXEC as i64);
    };
    let vfork = r.flags & CLONE_VFORK != 0;
    let mut done = [-1i32; 2];
    // SAFETY: plain pipe creation; both ends close on exec.
    if vfork && unsafe { libc::pipe(done.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    for fd in done.into_iter().filter(|&fd| fd >= 0) {
        // SAFETY: our pipe.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let setup = spawn::ChildSetup {
        stack: r.stack,
        tls: (r.flags & CLONE_SETTLS != 0).then_some(r.tls),
        set_tid: if r.flags & CLONE_CHILD_SETTID != 0 {
            r.child_tid
        } else {
            0
        },
        clear_tid: if r.flags & CLONE_CHILD_CLEARTID != 0 {
            r.child_tid
        } else {
            0
        },
        vfork: vfork.then_some((done[0], done[1])),
    };
    let pid = match spawn::fork(ctx, &setup, runtime) {
        Ok(pid) => pid,
        Err(e) => {
            for fd in done.into_iter().filter(|&fd| fd >= 0) {
                // SAFETY: our pipe.
                unsafe { libc::close(fd) };
            }
            return e;
        }
    };
    super::cred::note_child(pid);
    if own_files_thread {
        own_files_thread_started(pid, &r);
    }
    // SAFETY: guest pointers named by the caller's flags.
    unsafe {
        if r.flags & CLONE_PARENT_SETTID != 0 && r.parent_tid != 0 {
            (r.parent_tid as *mut i32).write_unaligned(pid);
        }
        if r.flags & CLONE_PIDFD != 0 && r.pidfd != 0 {
            let fd = super::wait::open_pidfd(pid, false);
            (r.pidfd as *mut i32).write_unaligned(fd as i32);
        }
    }
    if vfork {
        // SAFETY: our pipe: read until the child's end closes.
        unsafe {
            libc::close(done[1]);
            let mut b = 0u8;
            while libc::read(done[0], (&mut b as *mut u8).cast(), 1) < 0
                && errno::last() == errno::EINTR
            {}
            libc::close(done[0]);
        }
    }
    pid as i64
}

/// In the child, on its main thread before it resumes: what the clone
/// flags ask of a new process. It returns 0.
fn child_started(ctx: &mut GuestContext, stack: u64, set_tid: u64, clear_tid: u64) {
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    // A new process has no clear_child_tid unless it asked for one.
    super::thread::set_tid_address([clear_tid, 0, 0, 0, 0, 0]);
    if set_tid != 0 {
        // SAFETY: guest pointer in the child's copy of memory.
        unsafe { (set_tid as *mut i32).write_unaligned(pid) };
    }
    if stack != 0 {
        ctx.sp = stack;
    }
    ctx.x[0] = 0;
}
