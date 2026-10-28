//! Exercise scheduling setup and the real agent protocol without a guest boot.

use super::*;
use crate::sys::{cgroup, cred, process};
use std::io::{BufRead, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::time::Instant;

const TEST: &str = "sys::ptrace::agent::signal_tests::scheduling_setup_survives_signal_storm";
const ISOLATED: &str = "scheduling-signal-isolated";
const TARGET: &str = "scheduling-signal-target";
static SIGNALS: AtomicUsize = AtomicUsize::new(0);

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn spawn(marker: &str) -> OwnedChild {
    OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--skip", marker])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

struct Storm {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Storm {
    fn start() -> Self {
        extern "C" fn handler(_: i32) {
            SIGNALS.fetch_add(1, SeqCst);
        }
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            // Deliberately omit SA_RESTART: blocked host calls must see EINTR.
            assert_eq!(
                libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()),
                0
            );
            let mut mask = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGUSR1);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut()),
                0
            );
        }
        let target = unsafe { libc::pthread_self() } as usize;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(SeqCst) {
                assert_eq!(
                    unsafe { libc::pthread_kill(target as libc::pthread_t, libc::SIGUSR1) },
                    0
                );
                std::thread::sleep(Duration::from_micros(100));
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Storm {
    fn drop(&mut self) {
        self.stop.store(true, SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn setup(root: &Path) {
    crate::vfs::init(&root.join("root"), Some(&root.join("run/path-map"))).unwrap();
    cred::init(cred::Identity::default(), Some(root.join("by-pid")));
    crate::context::init_thread();
}

fn hold(path: &Path) -> std::fs::File {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
    file
}

fn contended(path: &Path, f: impl FnOnce()) {
    let held = hold(path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        drop(held);
    });
    f();
    release.join().unwrap();
}

#[test]
fn scheduling_setup_survives_signal_storm() {
    if std::env::args().any(|arg| arg == TARGET) {
        let mut root = String::new();
        std::io::stdin().read_line(&mut root).unwrap();
        let root = Path::new(root.trim());
        setup(root);
        let _storm = Storm::start();
        let d = dir().unwrap();
        let listener = listen(&d, &format!("{}.sock", std::process::id())).unwrap();
        println!("agent-ready");
        std::io::stdout().flush().unwrap();
        // Like the real agent, serve on a separate host thread: changing
        // the guest's priority must not background the control channel.
        std::thread::spawn(move || {
            let _storm = Storm::start();
            let client = crate::errno::retry_host(|| unsafe {
                libc::accept(listener, std::ptr::null_mut(), std::ptr::null_mut())
            })
            .unwrap();
            let client = fdtab::hide(client);
            lock(&OWN).push(client);
            connection(client);
            close(listener);
        })
        .join()
        .unwrap();
        process::apply_stale_sched(thread::current().unwrap()).unwrap();
        assert!(SIGNALS.load(SeqCst) > 0);
        return;
    }
    if !std::env::args().any(|arg| arg == ISOLATED) {
        let mut child = spawn(ISOLATED);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                let mut output = String::new();
                std::io::Read::read_to_string(child.0.stdout.as_mut().unwrap(), &mut output)
                    .unwrap();
                assert!(status.success(), "{status}: {output}");
                return;
            }
            assert!(Instant::now() < deadline, "signal-storm test timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    let root = std::env::temp_dir().join(format!("aim-scheduling-signals-{}", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir_all(root.join("root")).unwrap();
    let _cleanup = Cleanup(root.clone());
    for name in ["by-pid", "run", "cpu", "blkio"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for c in [
        aim_storage::cgroup_v1::Controller::Cpu,
        aim_storage::cgroup_v1::Controller::Blkio,
    ] {
        aim_storage::cgroup_v1::populate(c, &root.join(c.name()), None).unwrap();
    }
    std::fs::write(
        root.join("run/path-map"),
        format!(
            "cgroup:cpu\t/dev/cpuctl\t{}\ncgroup:blkio\t/dev/blkio\t{}\n",
            root.join("cpu").display(),
            root.join("blkio").display()
        ),
    )
    .unwrap();
    setup(&root);
    let _storm = Storm::start();
    let state = aim_storage::cgroup_v1::state_path(&root.join("cpu"));

    // Control: prove the storm interrupts a contended, unwrapped flock.
    let held = hold(&state);
    let reader = std::fs::File::open(&state).unwrap();
    assert_eq!(
        unsafe { libc::flock(reader.as_raw_fd(), libc::LOCK_SH) },
        -1
    );
    assert_eq!(crate::errno::last(), crate::errno::EINTR);
    drop(held);

    contended(&state, || cgroup::refresh_own().unwrap());
    contended(
        &aim_storage::cgroup_v1::state_path(&root.join("blkio")),
        || cgroup::refresh_own().unwrap(),
    );
    for (nice, policy) in [(0, 0), (10, 0), (-10, 0), (0, 1), (19, 0), (0, 0)] {
        process::apply_host_sched(nice, policy).unwrap();
    }

    // Transactions use both the policy lock and the membership lock.
    let group = root.join("cpu/group");
    std::fs::create_dir(&group).unwrap();
    aim_storage::cgroup_v1::populate(
        aim_storage::cgroup_v1::Controller::Cpu,
        &group,
        Some(&root.join("cpu")),
    )
    .unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(group.join("cpu.shares"))
        .unwrap();
    cgroup::opened(
        file.as_raw_fd(),
        &group.join("cpu.shares"),
        Some(aim_storage::cgroup_v1::Controller::Cpu),
        root.join("cpu"),
    );
    let Some(fdtab::Kind::Cgroup(open)) = fdtab::get(file.as_raw_fd()) else {
        panic!("missing cgroup fd")
    };
    let value = b"1024\n";
    let iov = [libc::iovec {
        iov_base: value.as_ptr().cast_mut().cast(),
        iov_len: value.len(),
    }];
    for path in [
        root.join("cpu").with_extension("policy-lock"),
        state.clone(),
    ] {
        contended(&path, || {
            assert_eq!(cgroup::write(&open, &iov, Some(0)), value.len() as i64)
        });
    }

    // Backpressure interrupts writes too; framing must retain partial progress.
    let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
    let body = vec![0x5a; 2 << 20];
    let reading = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        recv(reader.as_raw_fd()).unwrap()
    });
    assert!(send(writer.as_raw_fd(), &body));
    assert_eq!(reading.join().unwrap(), body);

    let mut target = spawn(TARGET);
    writeln!(target.0.stdin.take().unwrap(), "{}", root.display()).unwrap();
    let mut output = std::io::BufReader::new(target.0.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "agent exited before ready"
        );
        if line.trim() == "agent-ready" {
            break;
        }
    }
    let pid = target.0.id() as i32;
    contended(&state, || {
        assert_eq!(super::super::remote_sched(pid, pid, 119, 2, 1), 0)
    });
    for _ in 0..100 {
        for (policy, prio) in [(0, 0), (1, 1), (2, 2), (3, 0)] {
            assert_eq!(super::super::remote_sched(pid, pid, 119, policy, prio), 0);
            assert_eq!(
                super::super::remote_sched(pid, pid, 120, 0, 0),
                policy as i64
            );
            assert_eq!(super::super::remote_sched(pid, pid, 121, 0, 0), prio as i64);
        }
        assert_eq!(
            super::super::remote_sched(pid, pid, 119, 2, 0),
            -(libc::EINVAL as i64)
        );
        assert_eq!(super::super::remote_priority(pid, pid, Some(10)), 0);
        assert_eq!(super::super::remote_priority(pid, pid, None), 10);
        super::super::remote_cgroup_policy(pid, &[(pid, cgroup::Policy::default())]).unwrap();
    }
    // Close only this test's connection so the target exits and is reaped.
    let fd = super::super::CONNS.lock().unwrap().remove(&pid).unwrap();
    close(fd);
    assert!(target.0.wait().unwrap().success());
    assert!(SIGNALS.load(SeqCst) > 0);

    // Preserve real errors instead of treating them as successful setup.
    assert_eq!(
        crate::errno::retry_host(|| unsafe { libc::flock(-1, libc::LOCK_SH) }),
        Err(crate::errno::EBADF)
    );
    std::fs::remove_file(&state).unwrap();
    std::fs::create_dir(&state).unwrap();
    assert!(cgroup::refresh_own().is_err());
}
