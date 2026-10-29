//! Host-only startup workload; no image or guest boot required.
use super::*;
use crate::sys::{cred, fdtab, fs, fsops, mem, memfd};
use std::sync::Barrier;
use std::time::{Duration, Instant};

/// Run with `cargo test -p aim-linux-abi --release --lib startup_memory_cost
/// -- --ignored --nocapture --test-threads=1`. Keep fragmented mappings below
/// an orphaned ART-style RW/RX memfd while threads map/unmap and renice.
#[test]
#[ignore]
fn startup_memory_cost() {
    let _view = crate::vfs::test_view();
    const PAGE: u64 = 16384;
    const THREADS: usize = 16;
    const ROUNDS: usize = 128;
    let crowded: Vec<_> = (0..2048)
        .map(|i| {
            let a = mem::mmap([0, PAGE, if i % 2 == 0 { 1 } else { 3 }, 0x22, u64::MAX, 0]);
            assert!(a > 0);
            a as u64
        })
        .collect();
    let fd = memfd::memfd_create([c"startup-jit".as_ptr() as u64, 0, 0, 0, 0, 0]);
    assert!(fd >= 0);
    assert_eq!(fsops::ftruncate([fd as u64, PAGE * 4, 0, 0, 0, 0]), 0);
    let rw = mem::mmap([0, PAGE * 4, 3, 1, fd as u64, 0]);
    let rx = mem::mmap([0, PAGE * 4, 5, 1, fd as u64, 0]);
    assert!(rw > 0 && rx > 0);
    assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    unsafe { (rw as *mut u8).write(73) };
    // Reserve the leader record before workers register, so each has a TID.
    let mut leader: GuestContext = unsafe { std::mem::zeroed() };
    let stacks = unsafe { context::bind(&raw mut leader, 0) };
    register_current(&raw mut leader, stacks);
    let ready = Barrier::new(THREADS + 1);
    let start = Instant::now();
    let (wall, totals) = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                let ready = &ready;
                scope.spawn(move || {
                    let mut ctx: GuestContext = unsafe { std::mem::zeroed() };
                    let stacks = unsafe { context::bind(&raw mut ctx, 0) };
                    register_current(&raw mut ctx, stacks);
                    // Warm host scheduling before timing it alongside memory.
                    assert_eq!(cred::setpriority([0; 6]), 0);
                    ready.wait();
                    let mut times = [Duration::ZERO; 3];
                    for _ in 0..ROUNDS {
                        let now = Instant::now();
                        assert_eq!(cred::setpriority([0; 6]), 0);
                        times[2] += now.elapsed();
                        let now = Instant::now();
                        let a = mem::mmap([0, PAGE, 3, 0x22, u64::MAX, 0]);
                        times[0] += now.elapsed();
                        assert!(a > 0);
                        let now = Instant::now();
                        assert_eq!(cred::setpriority([0; 6]), 0);
                        times[2] += now.elapsed();
                        let now = Instant::now();
                        assert_eq!(mem::munmap(&ctx, [a as u64, PAGE, 0, 0, 0, 0]), 0);
                        times[1] += now.elapsed();
                    }
                    unregister(&mut ctx);
                    times
                })
            })
            .collect();
        ready.wait();
        let begin = Instant::now();
        let mut totals = [Duration::ZERO; 3];
        for worker in workers {
            let times = worker.join().unwrap();
            for (total, time) in totals.iter_mut().zip(times) {
                *total += time;
            }
        }
        (begin.elapsed(), totals)
    });
    let pairs = (THREADS * ROUNDS) as f64;
    eprintln!(
        "startup: {THREADS} threads, 2048 resident VMAs, {} pairs: wall {:.3} ms; mean mmap {:.3} us, munmap {:.3} us, setpriority {:.3} us (includes contention); setup+run {:.3} ms",
        pairs,
        wall.as_secs_f64() * 1e3,
        totals[0].as_secs_f64() * 1e6 / pairs,
        totals[1].as_secs_f64() * 1e6 / pairs,
        totals[2].as_secs_f64() * 1e6 / (pairs * 2.0),
        start.elapsed().as_secs_f64() * 1e3,
    );
    assert_eq!(unsafe { (rx as *const u8).read() }, 73);
    unregister(&mut leader);
    for a in crowded.into_iter().chain([rw as u64, rx as u64]) {
        let len = if a == rw as u64 || a == rx as u64 {
            PAGE * 4
        } else {
            PAGE
        };
        assert_eq!(mem::munmap(&leader, [a, len, 0, 0, 0, 0]), 0);
    }
    assert!(fdtab::fds_where(|k| matches!(k, fdtab::Kind::Memfd(_))).is_empty());
}

fn unregister(ctx: &mut GuestContext) {
    let th = unsafe { &*ctx.thread };
    with_table_mut(|t| {
        super::super::procrec::gone(th.tid);
        t.remove(&th.tid);
        cred::forget_nice(th.tid);
    });
    let stacks = th.stacks.lock().unwrap().take();
    unsafe {
        context::unbind(stacks);
        if th.tid == pid() {
            MAIN_THREAD.store(0, SeqCst);
            MAIN_PTHREAD.store(0, SeqCst);
        }
        drop(Arc::from_raw(ctx.thread));
        ctx.thread = std::ptr::null();
    }
}
