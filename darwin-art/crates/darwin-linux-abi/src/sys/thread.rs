//! Guest threads: `clone` with CLONE_THREAD and the `exit` of a non-main
//! thread, as experiments/p0/02-clone-threads settled.
//!
//! Each guest thread is a detached Darwin pthread. It gets its own
//! [`GuestContext`] (host stack, shadow call stack), copies the caller's
//! registers with x0 = 0 and sp = the child stack, and resumes after the
//! `svc`. Processes (`fork`) and namespaces are not handled here.

use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex};

use crate::context::{self, GuestContext};
use crate::errno::{EAGAIN, EINVAL, ENOSYS};

const CLONE_VM: u64 = 0x100;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_SETTLS: u64 = 0x80000;
const CLONE_PARENT_SETTID: u64 = 0x100000;
const CLONE_CHILD_CLEARTID: u64 = 0x200000;
const CLONE_CHILD_SETTID: u64 = 0x1000000;
const CLONE_THREAD: u64 = 0x10000;

/// Host stack for the pthread itself; guest syscalls run on the context's
/// own host stack.
const PTHREAD_STACK: usize = 256 << 10;

thread_local! {
    /// CLONE_CHILD_CLEARTID / set_tid_address of this thread.
    static CLEAR_TID: Cell<u64> = const { Cell::new(0) };
}

pub fn set_clear_tid(addr: u64) {
    CLEAR_TID.with(|c| c.set(addr));
}

struct Start {
    regs: Box<GuestContext>,
    tls: Option<u64>,
    clear_tid: u64,
    set_tid: u64,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

/// The start handshake: the child publishes its Linux tid, and runs guest
/// code only after the parent stored it (CLONE_PARENT_SETTID is visible
/// before the child runs, as on Linux).
enum Gate {
    Created,
    Started(i64),
    Go,
}

extern "C" fn child_main(arg: *mut libc::c_void) -> *mut libc::c_void {
    // SAFETY: `arg` is the Box<Start> leaked by `clone`.
    let start = unsafe { Box::from_raw(arg as *mut Start) };
    let ctx = context::init_thread();
    let tid = super::host_tid();
    {
        // Publish the tid, then wait until the parent has stored *ptid.
        let (lock, cv) = &*start.gate;
        let mut g = lock.lock().unwrap();
        *g = Gate::Started(tid);
        cv.notify_all();
        while !matches!(*g, Gate::Go) {
            g = cv.wait(g).unwrap();
        }
    }
    // SAFETY: `ctx` is this thread's fresh context; the parent's registers
    // were copied into `regs` while the parent was stopped in the syscall.
    unsafe {
        let c = &mut *ctx;
        c.x = start.regs.x;
        c.v = start.regs.v;
        c.sp = start.regs.sp;
        c.pc = start.regs.pc;
        c.nzcv = start.regs.nzcv;
        c.fpcr = start.regs.fpcr;
        c.fpsr = start.regs.fpsr;
        c.stub_ret = 0;
        if let Some(tp) = start.tls {
            context::set_guest_tp(tp);
        }
        if start.set_tid != 0 {
            (start.set_tid as *mut u32).write_volatile(tid as u32);
        }
    }
    set_clear_tid(start.clear_tid);
    drop(start);
    context::resume_guest()
}

pub fn clone(ctx: &GuestContext, a: [u64; 6]) -> i64 {
    let (flags, stack, ptid, tls, ctid) = (a[0], a[1], a[2], a[3], a[4]);
    let thread = CLONE_VM | CLONE_SIGHAND | CLONE_THREAD;
    if flags & thread != thread {
        eprintln!("[linux-abi] clone flags {flags:#x}: only threads are implemented");
        return -(ENOSYS as i64);
    }
    if stack == 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: an all-zero context is valid; the copy is filled below.
    let mut regs: Box<GuestContext> = Box::new(unsafe { std::mem::zeroed() });
    regs.x = ctx.x;
    regs.x[0] = 0;
    regs.v = ctx.v;
    regs.sp = stack;
    regs.pc = ctx.resume_pc();
    regs.nzcv = ctx.nzcv;
    regs.fpcr = ctx.fpcr;
    regs.fpsr = ctx.fpsr;
    let gate = Arc::new((Mutex::new(Gate::Created), Condvar::new()));
    let start = Box::new(Start {
        regs,
        tls: (flags & CLONE_SETTLS != 0).then_some(tls),
        clear_tid: if flags & CLONE_CHILD_CLEARTID != 0 {
            ctid
        } else {
            0
        },
        set_tid: if flags & CLONE_CHILD_SETTID != 0 {
            ctid
        } else {
            0
        },
        gate: gate.clone(),
    });
    // SAFETY: pthread attributes on our stack, started detached.
    let r = unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        libc::pthread_attr_init(&mut attr);
        libc::pthread_attr_setstacksize(&mut attr, PTHREAD_STACK);
        libc::pthread_attr_setdetachstate(&mut attr, libc::PTHREAD_CREATE_DETACHED);
        let mut t: libc::pthread_t = std::mem::zeroed();
        let raw = Box::into_raw(start);
        let r = libc::pthread_create(&mut t, &attr, child_main, raw.cast());
        libc::pthread_attr_destroy(&mut attr);
        if r != 0 {
            drop(Box::from_raw(raw));
        }
        r
    };
    if r != 0 {
        return -(EAGAIN as i64);
    }
    let (lock, cv) = &*gate;
    let mut g = lock.lock().unwrap();
    let tid = loop {
        if let Gate::Started(tid) = *g {
            break tid;
        }
        g = cv.wait(g).unwrap();
    };
    if flags & CLONE_PARENT_SETTID != 0 {
        // SAFETY: guest pointer to the new thread's tid field.
        unsafe { (ptid as *mut u32).write_volatile(tid as u32) };
    }
    *g = Gate::Go;
    cv.notify_all();
    tid
}

/// `exit` of the calling thread: CLONE_CHILD_CLEARTID, then the Darwin
/// thread ends. The main thread's exit ends the process.
pub fn exit(a: [u64; 6]) -> i64 {
    // Now off the guest stack: its deferred munmap (bionic's
    // _exit_with_stack_teardown) can run. bionic cleared the tid address of
    // such a thread first.
    super::run_deferred_unmaps();
    // SAFETY: trivial.
    if unsafe { libc::pthread_main_np() } == 1 {
        return super::process::exit_group(a);
    }
    let clear = CLEAR_TID.with(|c| c.get());
    if clear != 0 {
        // SAFETY: the guest registered this word for exactly this store.
        unsafe { (clear as *mut u32).write_volatile(0) };
        super::futex::wake_all(clear);
    }
    // SAFETY: ending this Darwin thread; the guest never runs on it again.
    unsafe { libc::pthread_exit(std::ptr::null_mut()) }
}
