//! Rewritten guest code reaches the syscall layer with Linux register
//! semantics (lean and full paths), the guest thread pointer survives
//! preemption, and load-time patching is safe for running code.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use darwin_linux_abi::{a64, context, diag, patch};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _t_regs, _t_regs_end",
    "_t_regs:", // x0 = u64[40] out buffer, x1 = syscall number
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
    "mov x8, x1",
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
    "dup v0.2d, x22",
    "dup v8.2d, x19",
    "dup v16.2d, x20",
    "dup v31.2d, x21",
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
    "str q0, [x1, #304]",
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
    ".globl _t_sys, _t_sys_end",
    "_t_sys:", // x0 = nr, x1..x4 = args; returns the syscall result
    "mov x8, x0",
    "mov x0, x1",
    "mov x1, x2",
    "mov x2, x3",
    "mov x3, x4",
    "svc #0",
    "ret",
    "_t_sys_end:",
    ".globl _t_kinds, _t_kinds_end",
    "_t_kinds:", // x0 = value, x1 = u64[2] out; SCS-protected
    "str x30, [x18], #8",
    "msr tpidr_el0, x0",
    "mrs x2, tpidr_el0",
    "str x2, [x1]",
    "mrs x3, ctr_el0",
    "str x3, [x1, #8]",
    "mov x30, #0",
    "ldr x30, [x18, #-8]!",
    "ret",
    "_t_kinds_end:",
    ".globl _t_unmap, _t_unmap_end",
    "_t_unmap:", // x0 = base, x1 = len, x2 = sp inside [base, base+len)
    "stp x19, x30, [sp, #-16]!",
    "mov x19, sp",
    "mov sp, x2",
    "mov x8, #215", // munmap(base, len): the calling thread's own stack
    "svc #0",
    "mov sp, x19",
    "ldp x19, x30, [sp], #16",
    "ret",
    "_t_unmap_end:",
    ".globl _t_spin, _t_spin_end",
    "_t_spin:", // x0 = &stop (u8), x1 = &count (u64); loops reading TPIDR_EL0
    "1: mrs x2, tpidr_el0",
    "ldr x3, [x1]",
    "add x3, x3, #1",
    "str x3, [x1]",
    "ldrb w4, [x0]",
    "cbz w4, 1b",
    "ret",
    "_t_spin_end:",
);

unsafe extern "C" {
    static t_regs: u8;
    static t_regs_end: u8;
    static t_tp: u8;
    static t_tp_end: u8;
    static t_sys: u8;
    static t_sys_end: u8;
    static t_kinds: u8;
    static t_kinds_end: u8;
    static t_unmap: u8;
    static t_unmap_end: u8;
    static t_spin: u8;
    static t_spin_end: u8;
}

/// Copy a snippet into fresh memory, rewrite it, make it executable.
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

fn setup() {
    context::init_thread();
    diag::install_signal_handlers();
}

fn check_regs(islands: bool, nr: u64, expect_x0: u64) {
    setup();
    // SAFETY: symbols from global_asm above.
    let (code, stats) = unsafe { load(&t_regs, &t_regs_end, islands) };
    assert_eq!(stats.svc, 1);
    assert_eq!(stats.brk_fallback, if islands { 0 } else { 1 });
    let mut out = [0u64; 40];
    // SAFETY: the snippet follows the C ABI.
    let f: extern "C" fn(*mut u64, u64) = unsafe { std::mem::transmute(code) };
    f(out.as_mut_ptr(), nr);
    assert_eq!(out[0], expect_x0, "x0 = result");
    assert_eq!(out[1], 1, "x1 preserved across svc");
    for r in (2..=28).filter(|&r| r != 8 && r != 18) {
        assert_eq!(out[r], r as u64, "x{r} preserved across svc");
    }
    assert_eq!(out[8], nr, "x8 preserved");
    assert_eq!(out[30] >> 28, 0b0110, "NZCV preserved (Z, C)");
    assert_eq!(&out[32..34], &[19, 19], "v8");
    assert_eq!(&out[34..36], &[20, 20], "v16");
    assert_eq!(&out[36..38], &[21, 21], "v31");
    assert_eq!(&out[38..40], &[22, 22], "v0");
}

// getpid (172) takes the lean path: integer registers only.
#[test]
fn lean_path_preserves_linux_register_state() {
    check_regs(true, 172, std::process::id() as u64);
}

// sched_getscheduler(<not us>) (120) takes the full path through Rust.
#[test]
fn full_path_preserves_linux_register_state() {
    check_regs(true, 120, -3i64 as u64);
}

#[test]
fn svc_via_brk_fallback_preserves_linux_register_state() {
    check_regs(false, 172, std::process::id() as u64);
}

fn sys(nr: u64, a: [u64; 4]) -> i64 {
    // SAFETY: symbols from global_asm above.
    static CODE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let code = *CODE.get_or_init(|| unsafe { load(&t_sys, &t_sys_end, true).0 });
    let f: extern "C" fn(u64, u64, u64, u64, u64) -> i64 = unsafe { std::mem::transmute(code) };
    f(nr, a[0], a[1], a[2], a[3])
}

#[test]
fn lean_syscalls_follow_linux_semantics() {
    setup();
    let mut fds = [0i32; 2];
    // SAFETY: plain pipe.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (r, w) = (fds[0] as u64, fds[1] as u64);
    let msg = b"lean";
    assert_eq!(sys(64, [w, msg.as_ptr() as u64, 4, 0]), 4, "write");
    let mut buf = [0u8; 8];
    assert_eq!(sys(63, [r, buf.as_mut_ptr() as u64, 8, 0]), 4, "read");
    assert_eq!(&buf[..4], msg);
    assert_eq!(sys(63, [9999, buf.as_mut_ptr() as u64, 8, 0]), -9, "EBADF");
    assert_eq!(sys(62, [r, 0, 0, 0]), -29, "lseek on a pipe: ESPIPE");
    assert_eq!(sys(57, [w, 0, 0, 0]), 0, "close");
    assert_eq!(sys(57, [w, 0, 0, 0]), -9, "close twice: EBADF");
    // EAGAIN is 35 on Darwin, 11 on Linux.
    // SAFETY: making our pipe non-blocking.
    unsafe { libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK) };
    let (a, b) = {
        let mut p = [0i32; 2];
        unsafe { libc::pipe(p.as_mut_ptr()) };
        unsafe { libc::fcntl(p[0], libc::F_SETFL, libc::O_NONBLOCK) };
        (p[0] as u64, p[1])
    };
    assert_eq!(sys(63, [a, buf.as_mut_ptr() as u64, 8, 0]), -11, "EAGAIN");
    // SAFETY: closing our fds.
    unsafe {
        libc::close(fds[0]);
        libc::close(a as i32);
        libc::close(b);
    }
    // The ids are the guest's identity, not the host's.
    use darwin_linux_abi::sys::cred;
    cred::init(
        cred::Identity::parse("uid\t1000\ngid\t1001\negid\t1002\n").unwrap(),
        None,
    );
    assert_eq!(sys(174, [0; 4]), 1000, "getuid");
    assert_eq!(sys(176, [0; 4]), 1001, "getgid");
    assert_eq!(sys(177, [0; 4]), 1002, "getegid");
    // SAFETY: trivial.
    assert_eq!(sys(173, [0; 4]), unsafe { libc::getppid() } as i64);
    assert_eq!(sys(124, [0; 4]), 0, "sched_yield");
    assert_eq!(
        sys(178, [0; 4]),
        darwin_linux_abi::sys::host_tid(),
        "gettid"
    );
    // pread64 with Linux SEEK semantics on a file.
    let path = std::env::temp_dir().join(format!("lean-{}", std::process::id()));
    std::fs::write(&path, b"0123456789").unwrap();
    let f = std::fs::File::open(&path).unwrap();
    use std::os::fd::AsRawFd;
    let fd = f.as_raw_fd() as u64;
    assert_eq!(sys(67, [fd, buf.as_mut_ptr() as u64, 3, 5]), 3, "pread64");
    assert_eq!(&buf[..3], b"567");
    assert_eq!(sys(62, [fd, 4, 0, 0]), 4, "lseek SEEK_SET");
    assert_eq!(sys(62, [fd, 0, 3, 0]), 0, "lseek SEEK_DATA (Linux 3)");
    assert_eq!(sys(62, [fd, 0, 4, 0]), 10, "lseek SEEK_HOLE (Linux 4)");
    drop(f);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn guest_thread_pointer_survives_syscalls_and_preemption() {
    setup();
    // SAFETY: symbols from global_asm above.
    let (code, stats) = unsafe { load(&t_tp, &t_tp_end, true) };
    assert_eq!((stats.svc, stats.mrs_tp, stats.msr_tp), (1, 1, 1));
    let f: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code) };
    let stop = Arc::new(AtomicBool::new(false));
    let spinners: Vec<_> = (0..16)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let mismatches = f(0x7100_0000_1234_5670, 200_000);
    stop.store(true, Ordering::Relaxed);
    spinners.into_iter().for_each(|t| t.join().unwrap());
    assert_eq!(mismatches, 0);
    assert_eq!(context::guest_tp(), 0x7100_0000_1234_5670);
}

fn check_kinds(islands: bool) {
    setup();
    // SAFETY: symbols from global_asm above.
    let (code, stats) = unsafe { load(&t_kinds, &t_kinds_end, islands) };
    assert_eq!(
        (stats.msr_tp, stats.mrs_tp, stats.scs, stats.ctr),
        (1, 1, 2, 1)
    );
    assert_eq!(stats.brk_fallback, if islands { 0 } else { 5 });
    let scs = context::guest_scs();
    let mut out = [0u64; 2];
    let f: extern "C" fn(u64, *mut u64) = unsafe { std::mem::transmute(code) };
    f(0x7200_0000_0000_1230, out.as_mut_ptr());
    assert_eq!(out[0], 0x7200_0000_0000_1230);
    assert_eq!(out[1], a64::host_ctr_el0() as u64);
    assert_eq!(context::guest_scs(), scs, "shadow stack balanced");
}

#[test]
fn tls_scs_and_ctr_via_islands() {
    check_kinds(true);
}

#[test]
fn tls_scs_and_ctr_via_brk_fallback() {
    check_kinds(false);
}

/// bionic's `_exit_with_stack_teardown` unmaps the calling thread's stack
/// before `exit`; the stub frame below sp must stay mapped until then.
#[test]
fn munmap_of_the_calling_stack_is_deferred() {
    setup();
    // SAFETY: symbols from global_asm above; a private stack mapping.
    unsafe {
        let (code, _) = load(&t_unmap, &t_unmap_end, true);
        let len = 64 * 1024;
        let stack = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        ) as u64;
        const MARK: u64 = 0x5eed_5eed_5eed_5eed;
        (stack as *mut u64).write(MARK);
        let f: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(code);
        assert_eq!(f(stack, len as u64, stack + len as u64 - 64), 0);
        // Still mapped: the stub returned through it.
        let ours = || {
            patch::vm::region(stack).is_some_and(|(lo, _, prot, _)| {
                lo <= stack && prot & libc::PROT_READ != 0 && (stack as *const u64).read() == MARK
            })
        };
        assert!(ours());
        assert_eq!(darwin_linux_abi::sys::run_deferred_unmaps(), 1);
        // Gone (another test thread may map the hole again, but not with
        // our contents).
        assert!(!ours(), "deferred munmap ran at thread exit");
    }
}

/// Load-time patching of code another thread is executing: the page keeps
/// execute permission throughout (stubs and site words are written through
/// RW aliases).
#[test]
fn patching_running_code_never_removes_execute_permission() {
    setup();
    // SAFETY: symbols from global_asm above. The copy is left unrewritten
    // (islands on, but only after it runs).
    let code = unsafe {
        let (s, e) = (&t_spin as *const u8, &t_spin_end as *const u8);
        let len = e as usize - s as usize;
        let p = libc::mmap(
            std::ptr::null_mut(),
            16384,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        std::ptr::copy_nonoverlapping(s, p as *mut u8, len);
        libc::mprotect(p, 16384, libc::PROT_READ | libc::PROT_EXEC);
        p as u64
    };
    let stop = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let count = Arc::new(AtomicU64::new(0));
    let runner = {
        let (stop, count) = (stop.clone(), count.clone());
        std::thread::spawn(move || {
            context::init_thread();
            let f: extern "C" fn(*const u8, *const u64) = unsafe { std::mem::transmute(code) };
            f(stop.as_ptr(), count.as_ptr());
        })
    };
    while count.load(Ordering::Relaxed) < 1_000_000 {
        std::hint::spin_loop();
    }
    let stats = patch::rewrite_region(code, 64);
    assert_eq!(stats.mrs_tp, 1);
    // Still executable, and now branching to the stub.
    let (_, _, prot, _) = patch::vm::region(code).unwrap();
    assert!(prot & libc::PROT_EXEC != 0);
    // SAFETY: reading our code page.
    assert!(a64::is_b(unsafe { (code as *const u32).read() }));
    let before = count.load(Ordering::Relaxed);
    while count.load(Ordering::Relaxed) < before + 1_000_000 {
        std::hint::spin_loop();
    }
    stop.store(1, Ordering::Relaxed);
    runner.join().unwrap();
}
