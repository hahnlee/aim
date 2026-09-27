//! fork and vfork: `clone` without CLONE_VM, and CLONE_VFORK.
//!
//! A guest process is a Darwin process, so a fork is a Darwin `fork()` made
//! from inside the syscall handler. The child is a copy of the calling
//! thread with the whole address space: guest memory, the translation
//! cache mappings (file-backed and shared, so still shared), this thread's
//! `GuestContext`, host stack and TSD slots. It returns from the same
//! syscall on the same guest context with 0 in x0. Only the layer's own
//! state that names threads or host objects needs fixing up in the child
//! ([`child_fixups`]): the pid and tid, the by-pid identity entry, and the
//! kqueue-backed fds (pidfds, epoll) and binder connection, which are host
//! objects Darwin does not inherit.
//!
//! `vfork` (CLONE_VM | CLONE_VFORK) is a fork whose parent waits until the
//! child execs or exits, which is all POSIX lets a vfork child do. The
//! wait is a close-on-exec pipe the child holds.

use crate::context::{self, GuestContext};
use crate::errno::{self, EINVAL};

const CSIGNAL: u64 = 0xff;
const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
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
    flags & CLONE_VM == 0 || flags & (CLONE_VFORK | CLONE_THREAD) == CLONE_VFORK
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

fn fork(ctx: &mut GuestContext, r: Request) -> i64 {
    // The exit signal (CSIGNAL) is always SIGCHLD on Darwin.
    if r.flags & (UNSUPPORTED | CLONE_NEWTIME) != 0 {
        return -(EINVAL as i64);
    }
    let vfork = r.flags & CLONE_VFORK != 0;
    super::wait::prune_pidfds();
    super::events::prune_epolls();
    let mut done = [-1i32; 2];
    // SAFETY: plain pipe creation; both ends close on exec.
    if vfork && unsafe { libc::pipe(done.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    for fd in done.into_iter().filter(|&fd| fd >= 0) {
        // SAFETY: our pipe.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // Hold stderr's lock across the fork so the child never inherits it
    // held by a thread that does not exist there. Darwin's libc takes its
    // own locks (malloc and the like) in its fork handlers.
    let stderr = std::io::stderr().lock();
    // SAFETY: fork; the child continues on this thread only.
    let pid = unsafe { libc::fork() };
    drop(stderr);
    if pid < 0 {
        let e = errno::last();
        for fd in done.into_iter().filter(|&fd| fd >= 0) {
            // SAFETY: our pipe.
            unsafe { libc::close(fd) };
        }
        return -(e as i64);
    }
    if pid == 0 {
        if vfork {
            // SAFETY: our pipe's read end; the write end closes on exec
            // or exit, which releases the parent.
            unsafe { libc::close(done[0]) };
        }
        child_fixups(ctx, &r);
        return 0;
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

/// The forked child's layer state: its pid, the one surviving thread (whose
/// Linux tid is the pid) and the host objects Darwin does not inherit.
fn child_fixups(ctx: &mut GuestContext, r: &Request) {
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    // SAFETY: only this thread runs in the child.
    unsafe { context::LINUX_ABI_PID = pid as u64 };
    ctx.tid = pid as u64;
    // A new process has no clear_child_tid unless it asked for one.
    let clear = if r.flags & CLONE_CHILD_CLEARTID != 0 {
        r.child_tid
    } else {
        0
    };
    super::process::set_tid_address([clear, 0, 0, 0, 0, 0]);
    if r.flags & CLONE_CHILD_SETTID != 0 && r.child_tid != 0 {
        // SAFETY: guest pointer in the child's copy of memory.
        unsafe { (r.child_tid as *mut i32).write_unaligned(pid) };
    }
    if r.flags & CLONE_SETTLS != 0 {
        context::set_guest_tp(r.tls);
    }
    if r.stack != 0 {
        ctx.sp = r.stack;
    }
    super::cred::after_fork_child();
    super::wait::after_fork_child();
    super::events::after_fork_child();
    super::binder::after_fork_child();
}
