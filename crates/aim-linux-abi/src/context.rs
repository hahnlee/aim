//! Per-thread guest register state and the Darwin TSD slots that locate it.
//!
//! Translated code reaches host state only through four fixed Darwin pthread
//! keys (`a64::slot`), addressed as `[TPIDRRO_EL0 + key * 8]`:
//! - the context slot holds this thread's [`GuestContext`];
//! - the thread-pointer slot holds the guest `TPIDR_EL0` (XNU stores a
//!   per-CPU value in the real register on every context switch);
//! - the entry slot holds the address of the syscall trampoline;
//! - the shadow-call-stack slot holds the guest shadow stack pointer, which
//!   Linux keeps in x18 (Darwin zeroes x18).
//!
//! The key numbers are baked into translated files, so they are claimed at
//! start-up rather than allocated.

use std::arch::global_asm;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;

use crate::a64::slot;

global_asm!(
    include_str!("trampoline.S"),
    ctx = const slot::CTX,
    pid = sym LINUX_ABI_PID,
    ids = sym crate::sys::cred::IDS,
    trace = sym crate::sys::TRACE,
    errtab = sym crate::errno::DARWIN_TO_LINUX,
    hostcall_hi = const aim_hostcall::SYSCALL_NR >> 16,
    slow = sym crate::sys::fdtab::SLOW,
    budget = sym crate::sys::space::BUDGET,
    in_host = const std::mem::offset_of!(GuestContext, in_host),
    attn = const std::mem::offset_of!(GuestContext, attn),
    orig = const std::mem::offset_of!(GuestContext, orig_x0),
);

unsafe extern "C" {
    fn linux_abi_enter_guest(ctx: *const libc::c_void) -> !;
    fn linux_abi_resume(ctx: *const libc::c_void) -> !;
    pub(crate) fn linux_abi_syscall_entry();
    fn linux_abi_lean_check();
    fn linux_abi_lean_end();
    fn linux_abi_check();
    fn linux_abi_recheck();
    fn linux_abi_tail_end();
    fn linux_abi_resume_trap() -> !;
    fn linux_abi_text_end();
}

/// The process id, answered by the lean syscall path without a Darwin call.
/// Set once in [`init_thread`].
pub static mut LINUX_ABI_PID: u64 = 0;

/// Full guest register state saved at a syscall boundary. The fields up to
/// `v` and the three after it are shared with `trampoline.S`.
#[repr(C, align(16))]
pub struct GuestContext {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub nzcv: u64,
    pub fpcr: u64,
    pub fpsr: u64,
    pub stub_ret: u64,
    pub host_sp: u64,
    /// Linux tid of this thread (read by the lean path).
    pub tid: u64,
    /// Nonzero while the thread runs host code on its behalf (from the
    /// full path's entry to its exit check), so a signal must wait.
    pub in_host: u32,
    _pad: u32,
    pub v: [u128; 32],
    /// The thread's signal attention flag, checked on every syscall exit.
    pub attn: *const AtomicU32,
    /// x0 of a lean-path syscall, to restart it.
    pub orig_x0: u64,
    pub(crate) thread: *const crate::sys::Thread,
}

const _: () = {
    assert!(std::mem::offset_of!(GuestContext, sp) == 248);
    assert!(std::mem::offset_of!(GuestContext, pc) == 256);
    assert!(std::mem::offset_of!(GuestContext, nzcv) == 264);
    assert!(std::mem::offset_of!(GuestContext, fpcr) == 272);
    assert!(std::mem::offset_of!(GuestContext, stub_ret) == 288);
    assert!(std::mem::offset_of!(GuestContext, host_sp) == 296);
    assert!(std::mem::offset_of!(GuestContext, tid) == 304);
    assert!(std::mem::offset_of!(GuestContext, in_host) == 312);
    assert!(std::mem::offset_of!(GuestContext, v) == 320);
};

impl GuestContext {
    fn zeroed() -> Self {
        // SAFETY: all-zero is a valid GuestContext.
        unsafe { std::mem::zeroed() }
    }

    /// Guest pc of the instruction after the redirected `svc`, recovered from
    /// the `b <site+4>` that ends the per-site stub.
    pub fn resume_pc(&self) -> u64 {
        if self.stub_ret == 0 {
            return self.pc;
        }
        let b_addr = self.stub_ret + 8;
        // SAFETY: stub_ret points into a live stub.
        let insn = unsafe { (b_addr as *const u32).read() };
        b_addr.wrapping_add(crate::a64::decode_b(insn) as u64)
    }
}

/// A clone child's context: the parent's registers, fresh host state.
pub fn copy_regs(p: &GuestContext) -> Box<GuestContext> {
    let mut c = Box::new(GuestContext::zeroed());
    c.x = p.x;
    c.sp = p.sp;
    c.pc = p.pc;
    c.nzcv = p.nzcv;
    c.fpcr = p.fpcr;
    c.fpsr = p.fpsr;
    c.stub_ret = p.stub_ret;
    c.v = p.v;
    c.in_host = 1;
    c
}

static CLAIMED: OnceLock<()> = OnceLock::new();

/// Claim the fixed TSD keys. Darwin hands out the lowest free key, so keys
/// are created until the wanted ones come up, and the rest are released.
fn claim_slots() {
    CLAIMED.get_or_init(|| {
        let wanted = [slot::CTX_KEY, slot::TP_KEY, slot::ENTRY_KEY, slot::SCS_KEY];
        let mut spare = Vec::new();
        let mut got = 0;
        while got < wanted.len() {
            let mut k: libc::pthread_key_t = 0;
            // SAFETY: plain key creation.
            if unsafe { libc::pthread_key_create(&mut k, None) } != 0 {
                break;
            }
            if wanted.contains(&(k as usize)) {
                got += 1;
            } else {
                spare.push(k);
            }
        }
        for k in spare {
            // SAFETY: releasing keys we created above.
            unsafe { libc::pthread_key_delete(k) };
        }
        assert_eq!(
            got,
            wanted.len(),
            "Darwin TSD keys {:?} are not free; translated code needs them",
            wanted
        );
    });
}

fn tsd_base() -> *mut u64 {
    let v: u64;
    // SAFETY: reading TPIDRRO_EL0 is always permitted at EL0.
    unsafe { std::arch::asm!("mrs {}, tpidrro_el0", out(reg) v) };
    v as *mut u64
}

fn slot_ptr(off: u32) -> *mut u64 {
    // SAFETY: offsets of claimed keys lie inside the TSD array.
    unsafe { tsd_base().byte_add(off as usize) }
}

/// The calling thread's context, or null if it runs no guest code.
/// Async-signal-safe.
pub fn current_ctx() -> *mut GuestContext {
    if CLAIMED.get().is_none() {
        return std::ptr::null_mut();
    }
    // SAFETY: slot claimed.
    unsafe { slot_ptr(slot::CTX).read() as *mut GuestContext }
}

/// Whether `ctx` is the calling thread's own context (the `brk` fallback
/// dispatches on a temporary one).
pub fn is_live(ctx: &GuestContext) -> bool {
    std::ptr::eq(ctx, current_ctx())
}

/// Guest `TPIDR_EL0` of the current thread.
pub fn guest_tp() -> u64 {
    // SAFETY: slot claimed in `init_thread`.
    unsafe { slot_ptr(slot::TP).read() }
}

/// Guest shadow-call-stack pointer of the current thread.
pub fn guest_scs() -> u64 {
    // SAFETY: slot claimed in `init_thread`.
    unsafe { slot_ptr(slot::SCS).read() }
}

/// Set the guest shadow-call-stack pointer of the current thread (a fork
/// child taking over its parent's shadow stack).
pub fn set_guest_scs(v: u64) {
    // SAFETY: slot claimed in `init_thread`.
    unsafe { slot_ptr(slot::SCS).write(v) }
}

const HOST_STACK_SIZE: usize = 1 << 20;
/// Shadow call stack: guard page, 32 KiB, guard page (bionic uses 16 KiB).
const SCS_GUARD: usize = 16 << 10;
const SCS_SIZE: usize = 32 << 10;
/// The host alternate signal stack, where every host handler runs.
const ALTSTACK_SIZE: usize = 128 << 10;
const STACKS_LEN: usize = SCS_GUARD * 2 + SCS_SIZE + ALTSTACK_SIZE;

fn map_anon(len: usize, prot: i32) -> *mut libc::c_void {
    // SAFETY: fresh anonymous mapping.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            prot,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(p, libc::MAP_FAILED, "guest thread allocation failed");
    p
}

/// A thread's shadow call stack and host signal stack, in one mapping:
/// guard, shadow stack, guard, alternate stack.
pub struct HostStacks {
    base: u64,
}

/// Stacks of exited threads, reused by new ones (thread pools churn).
static SPARE_STACKS: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
const MAX_SPARE_STACKS: usize = 64;

impl HostStacks {
    fn map() -> Self {
        if let Some(base) = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner()).pop() {
            return HostStacks { base };
        }
        let base = map_anon(STACKS_LEN, libc::PROT_READ | libc::PROT_WRITE) as u64;
        // SAFETY: guards inside our fresh mapping.
        unsafe {
            libc::mprotect(base as *mut _, SCS_GUARD, libc::PROT_NONE);
            libc::mprotect(
                (base + (SCS_GUARD + SCS_SIZE) as u64) as *mut _,
                SCS_GUARD,
                libc::PROT_NONE,
            );
        }
        HostStacks { base }
    }

    pub(crate) fn scs(&self) -> u64 {
        self.base + SCS_GUARD as u64
    }

    fn altstack(&self) -> u64 {
        self.base + (SCS_GUARD * 2 + SCS_SIZE) as u64
    }
}

/// Bind the calling host thread to `ctx`: TSD slots (guest thread pointer
/// `tp`), a fresh shadow call stack and the host alternate signal stack.
///
/// # Safety
/// `ctx` must stay valid while bound.
pub unsafe fn bind(ctx: *mut GuestContext, tp: u64) -> HostStacks {
    claim_slots();
    let st = HostStacks::map();
    // SAFETY: slots claimed above; this thread owns its slots.
    unsafe {
        slot_ptr(slot::CTX).write(ctx as u64);
        slot_ptr(slot::TP).write(tp);
        slot_ptr(slot::ENTRY).write(linux_abi_syscall_entry as usize as u64);
        slot_ptr(slot::SCS).write(st.scs());
        let ss = libc::stack_t {
            ss_sp: st.altstack() as *mut _,
            ss_size: ALTSTACK_SIZE,
            ss_flags: 0,
        };
        libc::sigaltstack(&ss, std::ptr::null_mut());
        // The slots must be what libpthread thinks they are.
        assert_eq!(
            libc::pthread_getspecific(slot::CTX_KEY as libc::pthread_key_t) as u64,
            ctx as u64,
            "Darwin TSD layout is not tpidrro_el0[key]"
        );
    }
    st
}

/// Undo [`bind`] on a thread that runs no more guest code. Host signals
/// must be blocked on it.
///
/// # Safety
/// Nothing may use the thread's context or stacks afterwards.
pub unsafe fn unbind(st: Option<HostStacks>) {
    // SAFETY: caller contract.
    unsafe {
        slot_ptr(slot::CTX).write(0);
        let ss = libc::stack_t {
            ss_sp: std::ptr::null_mut(),
            ss_size: 0,
            ss_flags: libc::SS_DISABLE,
        };
        libc::sigaltstack(&ss, std::ptr::null_mut());
        if let Some(st) = st {
            let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
            if spare.len() < MAX_SPARE_STACKS {
                spare.push(st.base);
            } else {
                libc::munmap(st.base as *mut _, STACKS_LEN);
            }
        }
    }
}

/// Prepare the current host thread to run guest code: its context, host
/// stack, shadow call stack, host signal stack and thread record (the first
/// thread is the main thread). Idempotent per thread.
pub fn init_thread() -> *mut GuestContext {
    let existing = current_ctx();
    if !existing.is_null() {
        return existing;
    }
    // SAFETY: the process id is written before any guest code runs.
    unsafe { LINUX_ABI_PID = libc::getpid() as u64 };
    let stack = map_anon(HOST_STACK_SIZE, libc::PROT_READ | libc::PROT_WRITE);
    let mut ctx = Box::new(GuestContext::zeroed());
    ctx.host_sp = stack as u64 + HOST_STACK_SIZE as u64;
    ctx.in_host = 1;
    let ptr = Box::into_raw(ctx);
    // SAFETY: the context lives for the thread's lifetime.
    let st = unsafe { bind(ptr, 0) };
    crate::sys::register_current(ptr, st);
    ptr
}

/// Set the guest `TPIDR_EL0` of the current thread (a fork's CLONE_SETTLS).
pub fn set_guest_tp(tp: u64) {
    // SAFETY: slot claimed in `init_thread`.
    unsafe { slot_ptr(slot::TP).write(tp) }
}

/// Switch the current thread into guest code at `pc` with stack `sp`.
/// Registers other than sp are zero, as the Linux kernel leaves them at exec.
pub fn enter_guest(pc: u64, sp: u64) -> ! {
    let ctx = init_thread();
    // SAFETY: ctx is this thread's context; the guest image is mapped.
    unsafe {
        let c = &mut *ctx;
        c.x = [0; 31];
        c.v = [0; 32];
        c.sp = sp;
        c.pc = pc;
        c.nzcv = 0;
        c.fpcr = 0;
        linux_abi_enter_guest(ctx as *const _)
    }
}

/// Start a bound thread from its context: the trampoline's exit path
/// delivers pending signals, then resumes the guest.
///
/// # Safety
/// `ctx` is the calling thread's bound context with `host_sp` set.
pub unsafe fn resume(ctx: *mut GuestContext) -> ! {
    // SAFETY: caller contract.
    unsafe { linux_abi_resume(ctx as *const _) }
}

/// Resume the live context exactly, through the trap whose handler loads
/// every register (`rt_sigreturn`).
pub fn resume_trap() -> ! {
    // SAFETY: the SIGILL handler recognizes this pc.
    unsafe { linux_abi_resume_trap() }
}

pub fn resume_trap_pc() -> u64 {
    linux_abi_resume_trap as usize as u64
}

/// Where a signal-interrupted pc lies, for deciding how to deliver.
#[derive(PartialEq, Eq)]
pub enum Region {
    /// The lean path's exit check: restart it at [`lean_check`].
    LeanTail,
    /// The full path's exit check and register restore: restart at
    /// [`full_recheck`] on the host stack.
    FullTail,
    /// Elsewhere in the trampoline: an exit check follows.
    Trampoline,
    /// Guest code (or host code, see `GuestContext::in_host`).
    Other,
}

pub fn region(pc: u64) -> Region {
    let a = |f: unsafe extern "C" fn()| f as usize as u64;
    if (a(linux_abi_lean_check)..a(linux_abi_lean_end)).contains(&pc) {
        Region::LeanTail
    } else if (a(linux_abi_check)..a(linux_abi_tail_end)).contains(&pc) {
        Region::FullTail
    } else if (a(linux_abi_syscall_entry)..a(linux_abi_text_end)).contains(&pc) {
        Region::Trampoline
    } else {
        Region::Other
    }
}

pub fn lean_check() -> u64 {
    linux_abi_lean_check as usize as u64
}

pub fn full_recheck() -> u64 {
    linux_abi_recheck as usize as u64
}
