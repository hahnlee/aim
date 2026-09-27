//! Rewritten guest code reaches the syscall layer with Linux register
//! semantics, and the guest thread pointer survives preemption.

use darwin_linux_abi::{context, diag, patch};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _t_regs, _t_regs_end",
    "_t_regs:", // x0 = u64[40] out buffer
    "sub sp, sp, #160",
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
    "str x0, [sp, #-16]!",
    "mov x1, #1",
    "mov x2, #2",
    "mov x3, #3",
    "mov x4, #4",
    "mov x5, #5",
    "mov x6, #6",
    "mov x7, #7",
    "mov x9, #9",
    "mov x10, #10",
    "mov x11, #11",
    "mov x12, #12",
    "mov x13, #13",
    "mov x14, #14",
    "mov x15, #15",
    "mov x16, #16",
    "mov x17, #17",
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
    "dup v8.2d, x19",
    "dup v16.2d, x20",
    "dup v31.2d, x21",
    "mov x8, #172",
    "cmp x1, x1", // Z and C set
    "svc #0",
    "mrs x30, nzcv",
    "stp x0, x1, [sp, #-16]!",
    "ldr x1, [sp, #16]",
    "stp x2, x3, [x1, #16]",
    "stp x4, x5, [x1, #32]",
    "stp x6, x7, [x1, #48]",
    "stp x8, x9, [x1, #64]",
    "stp x10, x11, [x1, #80]",
    "stp x12, x13, [x1, #96]",
    "stp x14, x15, [x1, #112]",
    "stp x16, x17, [x1, #128]",
    "stp x18, x19, [x1, #144]",
    "stp x20, x21, [x1, #160]",
    "stp x22, x23, [x1, #176]",
    "stp x24, x25, [x1, #192]",
    "stp x26, x27, [x1, #208]",
    "str x28, [x1, #224]",
    "str x30, [x1, #240]",
    "str q8, [x1, #256]",
    "str q16, [x1, #272]",
    "str q31, [x1, #288]",
    "ldp x2, x3, [sp], #16",
    "stp x2, x3, [x1]",
    "add sp, sp, #16",
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
    "add sp, sp, #160",
    "ret",
    "_t_regs_end:",
    ".globl _t_tp, _t_tp_end",
    "_t_tp:", // x0 = value to set, x1 = iterations; returns mismatches
    "stp x29, x30, [sp, #-16]!",
    "msr tpidr_el0, x0",
    "mov x9, x0",
    "mov x10, #0",
    "1: mov x8, #124", // sched_yield
    "svc #0",
    "mrs x11, tpidr_el0",
    "cmp x11, x9",
    "cinc x10, x10, ne",
    "subs x1, x1, #1",
    "b.ne 1b",
    "mov x0, x10",
    "ldp x29, x30, [sp], #16",
    "ret",
    "_t_tp_end:",
);

unsafe extern "C" {
    static t_regs: u8;
    static t_regs_end: u8;
    static t_tp: u8;
    static t_tp_end: u8;
}

fn load(start: &u8, end: &u8, islands: bool) -> (u64, patch::PatchStats) {
    let (s, e) = (start as *const u8, end as *const u8);
    let len = e as usize - s as usize;
    // SAFETY: fresh RW page; snippet copied, rewritten, then protected RX.
    unsafe {
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
        assert_eq!(
            libc::mprotect(p, 16384, libc::PROT_READ | libc::PROT_EXEC),
            0
        );
        (p as u64, stats)
    }
}

fn check_regs(islands: bool) {
    context::init_thread();
    diag::install_signal_handlers();
    // SAFETY: symbols from global_asm above.
    let (code, stats) = unsafe { load(&t_regs, &t_regs_end, islands) };
    assert_eq!(stats.svc, 1);
    assert_eq!(stats.brk_fallback, if islands { 0 } else { 1 });
    let mut out = [0u64; 40];
    // SAFETY: the snippet follows the C ABI.
    let f: extern "C" fn(*mut u64) = unsafe { std::mem::transmute(code) };
    f(out.as_mut_ptr());
    assert_eq!(out[0], unsafe { libc::getpid() } as u64, "x0 = getpid()");
    for r in (1..=28).filter(|&r| r != 8 && r != 18) {
        assert_eq!(out[r], r as u64, "x{r} preserved across svc");
    }
    assert_eq!(out[8], 172, "x8 preserved");
    assert_eq!(out[30] >> 28, 0b0110, "NZCV preserved (Z, C)");
    assert_eq!(&out[32..34], &[19, 19], "v8");
    assert_eq!(&out[34..36], &[20, 20], "v16");
    assert_eq!(&out[36..38], &[21, 21], "v31");
}

#[test]
fn svc_via_island_preserves_linux_register_state() {
    check_regs(true);
}

#[test]
fn svc_via_brk_fallback_preserves_linux_register_state() {
    check_regs(false);
}

#[test]
fn guest_thread_pointer_survives_syscalls_and_preemption() {
    context::init_thread();
    // SAFETY: symbols from global_asm above.
    let (code, stats) = unsafe { load(&t_tp, &t_tp_end, true) };
    assert_eq!((stats.svc, stats.mrs_tp, stats.msr_tp), (1, 1, 1));
    let f: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code) };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spinners: Vec<_> = (0..16)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let mismatches = f(0x7100_0000_1234_5670, 200_000);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    spinners.into_iter().for_each(|t| t.join().unwrap());
    assert_eq!(mismatches, 0);
    assert_eq!(context::guest_tp(), 0x7100_0000_1234_5670);
}
