//! P0 measurements for ADR 0012 (not part of the runtime):
//! - cost of a redirected Linux `getpid` against Darwin's own `getpid`;
//! - cost of a rewritten `mrs xN, tpidr_el0`;
//! - whether a raw user write to TPIDR_EL0 survives syscalls and preemption.
//!
//! `cargo run --release --example boundary_probe`

use std::time::Instant;

use darwin_linux_abi::{context, diag, patch};

// Guest-style code, copied into fresh pages and rewritten like loaded code.
std::arch::global_asm!(
    ".p2align 2",
    ".globl _probe_getpid_loop, _probe_getpid_loop_end",
    "_probe_getpid_loop:", // x0 = iterations, x1 = Linux syscall number
    "mov x9, x0",
    "mov x10, x1",
    "1: mov x8, x10",
    "svc #0",
    "subs x9, x9, #1",
    "b.ne 1b",
    "ret",
    "_probe_getpid_loop_end:",
    ".globl _probe_mrs_loop, _probe_mrs_loop_end",
    "_probe_mrs_loop:", // x0 = iterations; returns the last value read
    "mov x9, x0",
    "1: mrs x0, tpidr_el0",
    "subs x9, x9, #1",
    "b.ne 1b",
    "ret",
    "_probe_mrs_loop_end:",
    ".globl _probe_msr, _probe_msr_end",
    "_probe_msr:",
    "msr tpidr_el0, x0",
    "ret",
    "_probe_msr_end:",
);

unsafe extern "C" {
    static probe_getpid_loop: u8;
    static probe_getpid_loop_end: u8;
    static probe_mrs_loop: u8;
    static probe_mrs_loop_end: u8;
    static probe_msr: u8;
    static probe_msr_end: u8;
}

/// Copy a code snippet into its own page, rewrite it, and make it executable.
fn load(start: &u8, end: &u8, rewrite: Option<bool>) -> extern "C" fn(u64, u64) -> u64 {
    let (s, e) = (start as *const u8, end as *const u8);
    let len = e as usize - s as usize;
    // SAFETY: fresh RW page; snippet copied then protected RX.
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
        if let Some(islands) = rewrite {
            patch::rewrite_region_with(p as u64, len as u64, islands);
        }
        assert_eq!(
            libc::mprotect(p, 16384, libc::PROT_READ | libc::PROT_EXEC),
            0
        );
        std::mem::transmute::<*mut libc::c_void, extern "C" fn(u64, u64) -> u64>(p)
    }
}

fn ns_per(iters: u64, f: impl FnOnce()) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_nanos() as f64 / iters as f64
}

fn darwin_svc_getpid() -> u64 {
    let r: u64;
    // SAFETY: Darwin BSD syscall 20 (getpid) through svc #0x80.
    unsafe {
        std::arch::asm!("svc #0x80", inlateout("x16") 20u64 => _, lateout("x0") r, lateout("x1") _)
    };
    r
}

fn raw_tpidr_read() -> u64 {
    let v: u64;
    // SAFETY: TPIDR_EL0 is readable at EL0.
    unsafe { std::arch::asm!("mrs {}, tpidr_el0", out(reg) v) };
    v
}

fn raw_tpidr_write(v: u64) {
    // SAFETY: TPIDR_EL0 is writable at EL0; restored by the caller.
    unsafe { std::arch::asm!("msr tpidr_el0, {}", in(reg) v) };
}

fn tpidr_survival() {
    const V: u64 = 0x1234_5678_9abc_def0;
    let saved = raw_tpidr_read();
    println!("raw TPIDR_EL0 at start: {saved:#x}");
    let count = |n: u64, op: &dyn Fn()| {
        raw_tpidr_write(V);
        let (mut bad, mut seen) = (0u64, 0u64);
        for _ in 0..n {
            op();
            let r = raw_tpidr_read();
            if r != V {
                bad += 1;
                seen = r;
                raw_tpidr_write(V);
            }
        }
        raw_tpidr_write(saved);
        (bad, seen)
    };
    let (b, s) = count(1_000_000, &|| {
        darwin_svc_getpid();
    });
    println!(
        "raw TPIDR_EL0 across 1e6 Darwin getpid syscalls: clobbered {b} times (last value {s:#x})"
    );
    let (b, s) = count(100_000, &|| unsafe {
        libc::sched_yield();
    });
    println!("raw TPIDR_EL0 across 1e5 sched_yield: clobbered {b} times (last value {s:#x})");
    let (b, s) = count(2_000, &|| unsafe {
        libc::usleep(100);
    });
    println!(
        "raw TPIDR_EL0 across 2000 usleep(100) (blocking): clobbered {b} times (last value {s:#x})"
    );
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spinners: Vec<_> = (0..2 * std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8))
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let (b, s) = count(1_000_000_000, &|| {});
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    spinners.into_iter().for_each(|t| t.join().unwrap());
    println!(
        "raw TPIDR_EL0 over 1e9 reads with no syscalls, CPU oversubscribed 2x: clobbered {b} times (last value {s:#x})"
    );
}

fn main() {
    context::init_thread();
    diag::install_signal_handlers();
    const N: u64 = 2_000_000;

    // SAFETY: symbols from the global_asm block above.
    let (gl, gle, ml, mle, ms, mse) = unsafe {
        (
            &probe_getpid_loop,
            &probe_getpid_loop_end,
            &probe_mrs_loop,
            &probe_mrs_loop_end,
            &probe_msr,
            &probe_msr_end,
        )
    };
    let getpid_island = load(gl, gle, Some(true));
    let getpid_brk = load(gl, gle, Some(false));
    let mrs_island = load(ml, mle, Some(true));
    let mrs_native = load(ml, mle, None);
    let msr_island = load(ms, mse, Some(true));

    const GETPID: u64 = 172;
    const GETPPID: u64 = 173;
    getpid_island(1000, GETPID);
    let t_island = ns_per(N, || {
        getpid_island(N, GETPID);
    });
    let t_island_ppid = ns_per(N, || {
        getpid_island(N, GETPPID);
    });
    let t_darwin_ppid = ns_per(N, || {
        for _ in 0..N {
            std::hint::black_box(unsafe { libc::getppid() });
        }
    });
    let t_darwin_libc = ns_per(N, || {
        for _ in 0..N {
            std::hint::black_box(unsafe { libc::getpid() });
        }
    });
    let t_darwin_svc = ns_per(N, || {
        for _ in 0..N {
            std::hint::black_box(darwin_svc_getpid());
        }
    });
    let nb = N / 20;
    let t_brk = ns_per(nb, || {
        getpid_brk(nb, GETPID);
    });
    println!(
        "redirected Linux getpid (svc -> island stub -> trampoline -> Rust -> libc getpid): {t_island:.1} ns"
    );
    println!("native Darwin getpid (libc, pid cached in userspace): {t_darwin_libc:.1} ns");
    println!("native Darwin getpid (svc #0x80, enters XNU): {t_darwin_svc:.1} ns");
    println!(
        "  => boundary round trip alone: {:.1} ns",
        t_island - t_darwin_libc
    );
    println!(
        "redirected Linux getppid (enters XNU): {t_island_ppid:.1} ns; native Darwin getppid: {t_darwin_ppid:.1} ns"
    );
    println!(
        "  => overhead on a kernel-entering syscall: {:.1} ns",
        t_island_ppid - t_darwin_ppid
    );
    println!("redirected Linux getpid via brk + SIGTRAP fallback: {t_brk:.1} ns");

    msr_island(0xfeed_f00d, 0);
    let n = 200_000_000;
    let t_mrs = ns_per(n, || {
        assert_eq!(mrs_island(n, 0), 0xfeed_f00d);
    });
    let t_mrs_native = ns_per(n, || {
        mrs_native(n, 0);
    });
    println!("rewritten mrs xN, tpidr_el0: {t_mrs:.2} ns; native mrs: {t_mrs_native:.2} ns");
    println!("guest tp slot after sched_yield: {:#x}", {
        unsafe { libc::sched_yield() };
        mrs_island(1, 0)
    });
    tpidr_survival();
}
