//! Host scheduling regressions and an opt-in startup timing workload.

use super::*;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const BENCH: &str = "sys::process::sched_tests::parallel_setpriority_startup";
const ISOLATED: &str = "setpriority-startup-isolated";

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn isolated(test: &str) -> bool {
    if !std::env::args().any(|a| a == ISOLATED) {
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    test,
                    "--include-ignored",
                    "--nocapture",
                    "--skip",
                    ISOLATED,
                ])
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return false;
            }
            if Instant::now() >= deadline {
                panic!("scheduling test timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    super::super::cred::init(super::super::cred::Identity::default(), None);
    crate::context::init_thread();
    true
}

#[test]
fn renice_retains_linux_state_and_observes_native_qos() {
    use super::super::cred;
    use libc::qos_class_t::*;
    if !isolated("sys::process::sched_tests::renice_retains_linux_state_and_observes_native_qos") {
        return;
    }
    let th = thread::current().unwrap();
    let caller = cred::current();
    // setpriority(2): per-thread values are clamped, readable synchronously,
    // and checked for permission even when the host class stays the same.
    for (nice, want) in [
        (0, QOS_CLASS_DEFAULT),
        (1, QOS_CLASS_DEFAULT),
        (9, QOS_CLASS_DEFAULT),
        (10, QOS_CLASS_UTILITY),
        (11, QOS_CLASS_UTILITY),
        (18, QOS_CLASS_UTILITY),
        (19, QOS_CLASS_BACKGROUND),
        (20, QOS_CLASS_BACKGROUND),
        (19, QOS_CLASS_BACKGROUND),
        (10, QOS_CLASS_UTILITY),
        (0, QOS_CLASS_DEFAULT),
    ] {
        assert_eq!(cred::setpriority([0, 0, nice, 0, 0, 0]), 0);
        assert_eq!(cred::getpriority([0; 6]), 20 - nice.min(19) as i64);
        let mut qos = QOS_CLASS_UNSPECIFIED;
        let mut relative = 1;
        assert_eq!(
            unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut qos, &mut relative)
            },
            0
        );
        assert_eq!(qos as i32, want as i32);
        assert_eq!(relative, 0);
    }
    let foreign = cred::Identity {
        uid: [10050; 4],
        cap_eff: 0,
        ..caller.clone()
    };
    assert_eq!(
        cred::own_priority(th.tid, Some(0), &foreign),
        -(EPERM as i64)
    );
    assert_eq!(cred::own_priority(th.tid, None, &caller), 20);

    // A native library changed this pthread between syscalls. A remembered
    // Linux nice/class alone would incorrectly skip the necessary update.
    for (qos, relative) in [(QOS_CLASS_UTILITY, 0), (QOS_CLASS_DEFAULT, -3)] {
        assert_eq!(
            unsafe { libc::pthread_set_qos_class_self_np(qos, relative) },
            0
        );
        assert_eq!(cred::setpriority([0; 6]), 0);
        let mut qos = QOS_CLASS_UNSPECIFIED;
        let mut relative = 1;
        assert_eq!(
            unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut qos, &mut relative)
            },
            0
        );
        assert_eq!((qos as i32, relative), (QOS_CLASS_DEFAULT as i32, 0));
    }

    // A cgroup change must be evaluated even with an unchanged nice value.
    th.sched
        .cgroup_fair_max
        .store(QOS_CLASS_BACKGROUND as i32, SeqCst);
    th.sched.host_stale.store(true, SeqCst);
    apply_stale_sched(th).unwrap();
    let mut qos = QOS_CLASS_UNSPECIFIED;
    assert_eq!(
        unsafe {
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut qos, std::ptr::null_mut())
        },
        0
    );
    assert_eq!(qos as i32, QOS_CLASS_BACKGROUND as i32);
    assert_eq!(cred::getpriority([0; 6]), 20);
    th.sched
        .cgroup_fair_max
        .store(QOS_CLASS_USER_INTERACTIVE as i32, SeqCst);
    th.sched.host_stale.store(true, SeqCst);
    apply_stale_sched(th).unwrap();
    assert_eq!(
        unsafe {
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut qos, std::ptr::null_mut())
        },
        0
    );
    assert_eq!(qos as i32, QOS_CLASS_DEFAULT as i32);
}

#[test]
#[ignore = "host timing benchmark"]
fn parallel_setpriority_startup() {
    if !isolated(BENCH) {
        return;
    }
    // Keep thousands of distinct VM entries live, and mutate mappings while
    // every worker renices itself. Alternating protections prevent coalescing.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let mut maps = Vec::new();
    for i in 0..4096 {
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page,
                if i % 2 == 0 {
                    libc::PROT_READ
                } else {
                    libc::PROT_NONE
                },
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        assert_ne!(p, libc::MAP_FAILED);
        maps.push(p);
    }
    const THREADS: usize = 32;
    const CALLS: usize = 1024;
    let barrier = Arc::new(Barrier::new(THREADS));
    let start = Instant::now();
    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                crate::context::init_thread();
                barrier.wait();
                let mut times = Vec::with_capacity(CALLS);
                for i in 0..CALLS {
                    let p = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            page,
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_ANON | libc::MAP_PRIVATE,
                            -1,
                            0,
                        )
                    };
                    assert_ne!(p, libc::MAP_FAILED);
                    // Pools repeat the same priority; different Linux nice values
                    // can also map to the same host class. Include real transitions.
                    let nice = [0, 1, 10, 11, 19, 19, 10, 0][(i / 16) % 8];
                    let before = Instant::now();
                    assert_eq!(super::super::cred::setpriority([0, 0, nice, 0, 0, 0]), 0);
                    times.push(before.elapsed().as_nanos());
                    assert_eq!(super::super::cred::getpriority([0; 6]), 20 - nice as i64);
                    assert_eq!(unsafe { libc::munmap(p, page) }, 0);
                }
                times
            })
        })
        .collect();
    let mut times: Vec<_> = workers
        .into_iter()
        .flat_map(|w| w.join().unwrap())
        .collect();
    let wall = start.elapsed();
    times.sort_unstable();
    let n = times.len();
    println!(
        "{THREADS} threads, 4096 live mmaps, {n} setpriority + mmap/munmap pairs: wall {:.3} ms; setpriority mean {:.3} us p50 {:.3} us p95 {:.3} us p99 {:.3} us max {:.3} us",
        wall.as_secs_f64() * 1e3,
        times.iter().sum::<u128>() as f64 / n as f64 / 1e3,
        times[n / 2] as f64 / 1e3,
        times[n * 95 / 100] as f64 / 1e3,
        times[n * 99 / 100] as f64 / 1e3,
        times[n - 1] as f64 / 1e3
    );
    for p in maps {
        assert_eq!(unsafe { libc::munmap(p, page) }, 0);
    }
    // Registered contexts live until process exit; this isolated host test
    // never resumes guest code on a worker after its Rust thread joins.
}
