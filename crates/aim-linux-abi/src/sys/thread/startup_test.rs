//! Host-only startup workload: no Android image or guest execution.
use super::*;
use crate::sys::{cgroup, cred};
use aim_storage::cgroup_v1::{self, Controller, Membership};
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

#[test]
#[ignore = "host timing workload; run explicitly with --ignored --nocapture"]
fn startup_parallel() {
    const WORKER: &str = "sys::thread::startup_test::startup_worker";
    let root = std::env::temp_dir().join(format!("aim-2389-{}", pid()));
    std::fs::create_dir_all(root.join("cpu/startup")).unwrap();
    cgroup_v1::populate(Controller::Cpu, &root.join("cpu"), None).unwrap();
    cgroup_v1::populate(
        Controller::Cpu,
        &root.join("cpu/startup"),
        Some(&root.join("cpu")),
    )
    .unwrap();
    std::fs::write(
        root.join("map"),
        format!(
            "root\t/\t{}\ncgroup:cpu\t/dev/cpuctl\t{}/cpu\n",
            root.display(),
            root.display()
        ),
    )
    .unwrap();
    struct Children(Vec<std::process::Child>);
    impl Drop for Children {
        fn drop(&mut self) {
            for child in &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    std::fs::create_dir_all(root.join("by-pid")).unwrap();
    let mut children = Children(Vec::new());
    for _ in 0..4 {
        children.0.push(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", WORKER, "--ignored", "--nocapture"])
                .current_dir(&root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut entries = Vec::new();
    let mut readers = Vec::new();
    for child in &mut children.0 {
        let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "worker exited before ready"
            );
            if line.starts_with("ready ") {
                break;
            }
        }
        let p =
            aim_storage::process_namespace::ProcessIdentity::running(child.id() as i32).unwrap();
        entries.push((
            p.host_pid,
            (p.start_seconds, p.start_microseconds),
            "/startup".into(),
        ));
        readers.push(reader);
    }
    // Model libprocessgroup moving each app's main task alone before clone.
    std::fs::write(
        cgroup_v1::state_path(&root.join("cpu")),
        Membership {
            processes: Vec::new(),
            threads: entries,
        }
        .text(),
    )
    .unwrap();
    let start = Instant::now();
    for child in &mut children.0 {
        child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    }
    for child in &mut children.0 {
        assert!(child.wait().unwrap().success());
    }
    let mut births_ms = 0.0_f64;
    for mut reader in readers {
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        let ms: f64 = output
            .lines()
            .find_map(|line| line.strip_prefix("births_ms "))
            .expect("worker birth timing")
            .parse()
            .unwrap();
        births_ms = births_ms.max(ms);
    }
    eprintln!("birth phase (slowest process): {births_ms:.3} ms");
    eprintln!(
        "startup: 4 processes x 8 threads x 8 births, 16384 mmaps, 8192 setpriority: {:.3} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    children.0.clear();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "subprocess of startup_parallel"]
fn startup_worker() {
    let root = std::env::current_dir().unwrap();
    crate::vfs::init(&root, Some(&root.join("map"))).unwrap();
    cred::init(cred::Identity::default(), Some(root.join("by-pid")));
    super::super::procrec::init(&super::super::procfs::StackInfo::default());
    with_table_mut(|t| {
        t.insert(
            pid(),
            Arc::new(Thread::new(pid(), std::ptr::null_mut(), 0, None)),
        );
    });
    super::super::procrec::thread(pid(), 0, &[0; 16]);
    println!("ready {}", pid());
    std::io::stdout().flush().unwrap();
    let mut go = String::new();
    std::io::stdin().read_line(&mut go).unwrap();
    let start = Instant::now();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                let mut maps = Vec::new();
                for _ in 0..512 {
                    let p = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            16384,
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_PRIVATE | libc::MAP_ANON,
                            -1,
                            0,
                        )
                    };
                    assert_ne!(p, libc::MAP_FAILED);
                    maps.push(p);
                }
                barrier.wait();
                let mut born = Vec::new();
                for _ in 0..8 {
                    let th = with_table_mut(|t| {
                        let r = alloc_tid(t).unwrap();
                        let th = Arc::new(Thread::new(r.tid, std::ptr::null_mut(), 0, Some(r)));
                        t.insert(th.tid, th.clone());
                        super::super::procrec::thread(th.tid, 0, &[0; 16]);
                        th
                    });
                    cgroup::thread_created(pid(), th.tid).unwrap();
                    assert!(cgroup::proc_cgroup(pid(), th.tid).contains(":cpu:/startup\n"));
                    cgroup::refresh_own().unwrap();
                    for _ in 0..32 {
                        assert_eq!(cred::setpriority([0, th.tid as u64, 0, 0, 0, 0]), 0);
                    }
                    born.push(th);
                }
                if barrier.wait().is_leader() {
                    println!("births_ms {:.3}", start.elapsed().as_secs_f64() * 1000.0);
                }
                for th in born {
                    cgroup::thread_gone(th.tid);
                    with_table_mut(|t| {
                        super::super::procrec::gone(th.tid);
                        t.remove(&th.tid);
                    });
                    cred::forget_nice(th.tid);
                }
                for p in maps {
                    assert_eq!(unsafe { libc::munmap(p, 16384) }, 0);
                }
            });
        }
    });
}
