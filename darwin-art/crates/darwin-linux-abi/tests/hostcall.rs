//! Rewritten guest code reaches host modules through the host-call syscall
//! with AAPCS64 call semantics, on the lean entry, the traced full path and
//! the `brk` fallback.

use darwin_hostcall::{ABI_VERSION, SYSCALL_NR, errno, health, module};
use darwin_linux_abi::{context, diag, patch, sys};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _t_hc, _t_hc_end",
    // x0 = u64[24] out, x1 = module, x2 = func, x3 = args, x4 = len.
    // Sets every callee-saved register, makes the host call and records
    // x0, x19..x29, sp, d8..d15.
    "_t_hc:",
    "sub sp, sp, #176",
    "stp x19, x20, [sp]",
    "stp x21, x22, [sp, #16]",
    "stp x23, x24, [sp, #32]",
    "stp x25, x26, [sp, #48]",
    "stp x27, x28, [sp, #64]",
    "stp x29, x30, [sp, #80]",
    "stp d8, d9, [sp, #96]",
    "stp d10, d11, [sp, #112]",
    "stp d12, d13, [sp, #128]",
    "stp d14, d15, [sp, #144]",
    "str x0, [sp, #160]",
    "mov x19, #19",
    "mov x20, #20",
    "mov x21, #21",
    "mov x22, #22",
    "mov x23, #23",
    "mov x24, #24",
    "mov x25, #25",
    "mov x26, #26",
    "mov x27, #27",
    "mov x28, #28",
    "mov x29, #29",
    "fmov d8, x19",
    "fmov d9, x20",
    "fmov d10, x21",
    "fmov d11, x22",
    "fmov d12, x23",
    "fmov d13, x24",
    "fmov d14, x25",
    "fmov d15, x26",
    "mov x0, x1",
    "mov x1, x2",
    "mov x2, x3",
    "mov x3, x4",
    "movz x8, #0x4843, lsl #16",
    "svc #0",
    "ldr x9, [sp, #160]",
    "str x0, [x9]",
    "stp x19, x20, [x9, #8]",
    "stp x21, x22, [x9, #24]",
    "stp x23, x24, [x9, #40]",
    "stp x25, x26, [x9, #56]",
    "stp x27, x28, [x9, #72]",
    "str x29, [x9, #88]",
    "mov x10, sp",
    "str x10, [x9, #96]",
    "stp d8, d9, [x9, #104]",
    "stp d10, d11, [x9, #120]",
    "stp d12, d13, [x9, #136]",
    "stp d14, d15, [x9, #152]",
    "ldp x19, x20, [sp]",
    "ldp x21, x22, [sp, #16]",
    "ldp x23, x24, [sp, #32]",
    "ldp x25, x26, [sp, #48]",
    "ldp x27, x28, [sp, #64]",
    "ldp x29, x30, [sp, #80]",
    "ldp d8, d9, [sp, #96]",
    "ldp d10, d11, [sp, #112]",
    "ldp d12, d13, [sp, #128]",
    "ldp d14, d15, [sp, #144]",
    "add sp, sp, #176",
    "ret",
    "_t_hc_end:",
);

unsafe extern "C" {
    static t_hc: u8;
    static t_hc_end: u8;
}

const _: () = assert!(SYSCALL_NR == 0x4843 << 16);

/// Copy the snippet into fresh memory, rewrite it, make it executable.
fn load(islands: bool) -> u64 {
    // SAFETY: symbols from global_asm above; fresh RW page, copied,
    // rewritten, then protected RX.
    unsafe {
        let (s, e) = (&t_hc as *const u8, &t_hc_end as *const u8);
        let len = e as usize - s as usize;
        let p = libc::mmap(
            std::ptr::null_mut(),
            16384,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        assert_ne!(p, libc::MAP_FAILED);
        std::ptr::copy_nonoverlapping(s, p as *mut u8, len);
        let stats = patch::rewrite_region_with(p as u64, len as u64, islands);
        assert_eq!(stats.svc, 1);
        assert_eq!(stats.brk_fallback, usize::from(!islands));
        assert_eq!(
            libc::mprotect(p, 16384, libc::PROT_READ | libc::PROT_EXEC),
            0
        );
        p as u64
    }
}

/// Call through the snippet; returns x0 after checking the callee-saved
/// registers and sp.
fn hostcall(code: u64, module: u32, func: u32, args: *mut u8, len: usize) -> i64 {
    let mut out = [0u64; 24];
    // SAFETY: the snippet follows the C ABI.
    let f: extern "C" fn(*mut u64, u64, u64, u64, u64) = unsafe { std::mem::transmute(code) };
    f(
        out.as_mut_ptr(),
        module as u64,
        func as u64,
        args as u64,
        len as u64,
    );
    for r in 19..=29 {
        assert_eq!(out[r - 18], r as u64, "x{r} preserved");
    }
    for d in 8..=15 {
        assert_eq!(out[13 + d - 8], d as u64 + 11, "d{d} preserved");
    }
    assert_ne!(out[12], 0, "sp");
    out[0] as i64
}

fn check(islands: bool) {
    context::init_thread();
    diag::install_signal_handlers();
    let code = load(islands);
    let null = std::ptr::null_mut();
    assert_eq!(hostcall(code, module::CORE, 0, null, 0), ABI_VERSION as i64);
    assert_eq!(
        hostcall(code, module::HEALTH, 0, null, 0),
        health::VERSION as i64
    );
    assert_eq!(
        hostcall(code, 77, 0, null, 0),
        -(errno::ENOSYS as i64),
        "unknown module"
    );
    let mut b = health::Battery::default();
    let p = (&mut b as *mut health::Battery).cast();
    assert_eq!(
        hostcall(code, module::HEALTH, health::FN_BATTERY, p, 8),
        -(errno::EINVAL as i64),
        "short argument block"
    );
    assert_eq!(hostcall(code, module::HEALTH, health::FN_BATTERY, p, 64), 0);
    assert_eq!(b.present, darwin_host_health::battery().present);
    assert_ne!(b.status, 0, "the host filled the block");
}

#[test]
fn host_call_on_the_lean_entry() {
    check(true);
}

#[test]
fn host_call_through_the_brk_fallback() {
    check(false);
}

#[test]
fn host_call_on_the_traced_full_path() {
    sys::set_trace(true);
    check(true);
    sys::set_trace(false);
}
