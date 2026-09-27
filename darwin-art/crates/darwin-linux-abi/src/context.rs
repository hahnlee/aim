//! Per-thread guest register state and the Darwin TSD slots that locate it.
//!
//! Two Darwin pthread keys are reserved for every guest thread:
//! - the context key holds a pointer to this thread's [`GuestContext`], read by
//!   the syscall trampoline through `TPIDRRO_EL0` without any scratch state;
//! - the thread-pointer key holds the guest's `TPIDR_EL0` value. XNU does not
//!   preserve a user-written `TPIDR_EL0` across context switches (it stores a
//!   per-CPU value there), so guest `mrs`/`msr tpidr_el0` instructions are
//!   rewritten at load time to use this slot instead (see `patch`).

use std::arch::global_asm;
use std::sync::OnceLock;

global_asm!(include_str!("trampoline.S"));

unsafe extern "C" {
    fn linux_abi_enter_guest(ctx: *const GuestContext) -> !;
    pub(crate) fn linux_abi_syscall_entry();
}

/// Byte offset of the context slot from `TPIDRRO_EL0`; read by the trampoline.
#[unsafe(no_mangle)]
pub static mut LINUX_ABI_CTX_TSD_OFFSET: u64 = 0;

/// Full guest register state saved at a syscall boundary. Layout is shared
/// with `trampoline.S`.
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
    _pad: [u64; 2],
    pub v: [u128; 32],
}

const _: () = {
    assert!(std::mem::offset_of!(GuestContext, sp) == 248);
    assert!(std::mem::offset_of!(GuestContext, pc) == 256);
    assert!(std::mem::offset_of!(GuestContext, nzcv) == 264);
    assert!(std::mem::offset_of!(GuestContext, stub_ret) == 288);
    assert!(std::mem::offset_of!(GuestContext, host_sp) == 296);
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
        // SAFETY: stub_ret points into a live trampoline island.
        let insn = unsafe { (b_addr as *const u32).read() };
        b_addr.wrapping_add(crate::patch::decode_b(insn) as u64)
    }
}

struct Keys {
    ctx: libc::pthread_key_t,
    tp: libc::pthread_key_t,
}

static KEYS: OnceLock<Keys> = OnceLock::new();

fn keys() -> &'static Keys {
    KEYS.get_or_init(|| {
        let mut ctx = 0;
        let mut tp = 0;
        // SAFETY: plain pthread key creation.
        unsafe {
            assert_eq!(libc::pthread_key_create(&mut ctx, None), 0);
            assert_eq!(libc::pthread_key_create(&mut tp, None), 0);
        }
        let k = Keys { ctx, tp };
        // SAFETY: written once before any guest code or trampoline runs.
        unsafe { LINUX_ABI_CTX_TSD_OFFSET = tsd_offset(k.ctx) as u64 };
        k
    })
}

/// Offset of a pthread key's slot from `TPIDRRO_EL0`. Darwin's
/// `_pthread_getspecific_direct` reads `((void **)tpidrro_el0)[key]`.
fn tsd_offset(key: libc::pthread_key_t) -> usize {
    key as usize * 8
}

fn tsd_base() -> *mut u64 {
    let v: u64;
    // SAFETY: reading TPIDRRO_EL0 is always permitted at EL0.
    unsafe { std::arch::asm!("mrs {}, tpidrro_el0", out(reg) v) };
    v as *mut u64
}

/// Byte offset from `TPIDRRO_EL0` of the slot that holds the guest `TPIDR_EL0`.
pub fn guest_tp_tsd_offset() -> u32 {
    tsd_offset(keys().tp) as u32
}

/// Guest `TPIDR_EL0` of the current thread.
pub fn guest_tp() -> u64 {
    // SAFETY: slot index validated in `init_thread`.
    unsafe { tsd_base().byte_add(guest_tp_tsd_offset() as usize).read() }
}

const HOST_STACK_SIZE: usize = 1 << 20;

/// Prepare the current host thread to run guest code: allocate its context
/// and host stack, and register both TSD slots. Idempotent per thread.
pub fn init_thread() -> *mut GuestContext {
    let k = keys();
    // SAFETY: pthread key APIs with keys created above.
    unsafe {
        let existing = libc::pthread_getspecific(k.ctx) as *mut GuestContext;
        if !existing.is_null() {
            return existing;
        }
        let stack = libc::mmap(
            std::ptr::null_mut(),
            HOST_STACK_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        assert_ne!(stack, libc::MAP_FAILED, "host stack allocation failed");
        let mut ctx = Box::new(GuestContext::zeroed());
        ctx.host_sp = stack as u64 + HOST_STACK_SIZE as u64;
        let ptr = Box::into_raw(ctx);
        assert_eq!(libc::pthread_setspecific(k.ctx, ptr as *const _), 0);
        assert_eq!(libc::pthread_setspecific(k.tp, std::ptr::null()), 0);
        // The trampoline and the rewritten TLS instructions address these
        // slots directly; make sure that matches libpthread's own view.
        let direct = tsd_base().byte_add(tsd_offset(k.ctx)).read();
        assert_eq!(
            direct, ptr as u64,
            "Darwin TSD layout is not tpidrro_el0[key]"
        );
        ptr
    }
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
        linux_abi_enter_guest(ctx)
    }
}
