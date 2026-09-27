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

use crate::a64::slot;

global_asm!(
    include_str!("trampoline.S"),
    ctx = const slot::CTX,
    pid = sym LINUX_ABI_PID,
    trace = sym crate::sys::TRACE,
    errtab = sym crate::errno::DARWIN_TO_LINUX,
    hostcall_hi = const darwin_hostcall::SYSCALL_NR >> 16,
);

unsafe extern "C" {
    fn linux_abi_enter_guest(ctx: *const GuestContext) -> !;
    pub(crate) fn linux_abi_syscall_entry();
}

/// The process id, answered by the lean syscall path without a Darwin call.
/// Set once in [`init_thread`]; a future fork must refresh it.
pub static mut LINUX_ABI_PID: u64 = 0;

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
    /// Linux tid of this thread (read by the lean path).
    pub tid: u64,
    _pad: u64,
    pub v: [u128; 32],
}

const _: () = {
    assert!(std::mem::offset_of!(GuestContext, sp) == 248);
    assert!(std::mem::offset_of!(GuestContext, pc) == 256);
    assert!(std::mem::offset_of!(GuestContext, nzcv) == 264);
    assert!(std::mem::offset_of!(GuestContext, fpcr) == 272);
    assert!(std::mem::offset_of!(GuestContext, stub_ret) == 288);
    assert!(std::mem::offset_of!(GuestContext, host_sp) == 296);
    assert!(std::mem::offset_of!(GuestContext, tid) == 304);
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

const HOST_STACK_SIZE: usize = 1 << 20;
/// Shadow call stack: guard page, 32 KiB, guard page (bionic uses 16 KiB).
const SCS_GUARD: usize = 16 << 10;
const SCS_SIZE: usize = 32 << 10;

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

/// Prepare the current host thread to run guest code: allocate its context,
/// host stack and shadow call stack, and fill the TSD slots. Idempotent per
/// thread.
pub fn init_thread() -> *mut GuestContext {
    claim_slots();
    // SAFETY: the slots were claimed above; this thread owns its slots.
    unsafe {
        let existing = slot_ptr(slot::CTX).read() as *mut GuestContext;
        if !existing.is_null() {
            return existing;
        }
        LINUX_ABI_PID = libc::getpid() as u64;
        let stack = map_anon(HOST_STACK_SIZE, libc::PROT_READ | libc::PROT_WRITE);
        let scs = map_anon(SCS_GUARD * 2 + SCS_SIZE, libc::PROT_NONE);
        let scs_base = scs as u64 + SCS_GUARD as u64;
        assert_eq!(
            libc::mprotect(
                scs_base as *mut _,
                SCS_SIZE,
                libc::PROT_READ | libc::PROT_WRITE
            ),
            0
        );
        let mut ctx = Box::new(GuestContext::zeroed());
        ctx.host_sp = stack as u64 + HOST_STACK_SIZE as u64;
        ctx.tid = crate::sys::host_tid() as u64;
        let ptr = Box::into_raw(ctx);
        slot_ptr(slot::CTX).write(ptr as u64);
        slot_ptr(slot::TP).write(0);
        slot_ptr(slot::ENTRY).write(linux_abi_syscall_entry as usize as u64);
        slot_ptr(slot::SCS).write(scs_base);
        // The slots must be what libpthread thinks they are.
        assert_eq!(
            libc::pthread_getspecific(slot::CTX_KEY as libc::pthread_key_t) as u64,
            ptr as u64,
            "Darwin TSD layout is not tpidrro_el0[key]"
        );
        ptr
    }
}

/// Set the guest `TPIDR_EL0` of the current thread (CLONE_SETTLS).
pub fn set_guest_tp(tp: u64) {
    // SAFETY: slot claimed in `init_thread`.
    unsafe { slot_ptr(slot::TP).write(tp) }
}

/// Resume guest code with the full register state already in the current
/// thread's context (set up by [`init_thread`]).
pub fn resume_guest() -> ! {
    // SAFETY: the slot holds this thread's live context.
    unsafe { linux_abi_enter_guest(slot_ptr(slot::CTX).read() as *const GuestContext) }
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
