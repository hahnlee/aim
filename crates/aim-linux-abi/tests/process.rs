//! Process lifecycle on the syscall layer: fork, vfork, execve (ELF, `#!`
//! scripts, argv[0] and AT_EXECFN), wait4/waitid, pidfds, identity and
//! death by signal, checked by an NDK-built guest program
//! (`tests/guest/process.c`) and the image's own mksh.
//!
//! The guest root is a small copy of the pinned full image (linker64, the
//! runtime APEX and mksh) plus the test program. Skipped when the image or
//! the NDK is not installed.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const PROGRAM: &str = "/data/local/tmp/process";

fn ndk_clang() -> Option<PathBuf> {
    aim_paths::ndk_clang(35)
}

/// Copy a guest path of the image into `dst`, keeping symlinks (absolute in
/// the image's namespace) and following them.
fn mirror(dst: &Path, guest: &str) {
    let rel = guest.trim_start_matches('/');
    let (from, to) = (aim_paths::original_image().join(rel), dst.join(rel));
    if to.symlink_metadata().is_ok() {
        return;
    }
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let meta = from.symlink_metadata().unwrap();
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&from).unwrap();
        std::os::unix::fs::symlink(&target, &to).unwrap();
        let t = target.to_string_lossy().into_owned();
        let next = if t.starts_with('/') {
            t
        } else {
            format!("{}/{t}", Path::new(guest).parent().unwrap().display())
        };
        mirror(dst, &next);
    } else if meta.is_dir() {
        for e in std::fs::read_dir(&from).unwrap().flatten() {
            mirror(dst, &format!("{guest}/{}", e.file_name().to_string_lossy()));
        }
    } else {
        std::fs::copy(&from, &to).unwrap();
    }
}

/// The guest root, built once per test run.
fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let Some(clang) = ndk_clang() else {
            aim_paths::skip("the pinned NDK is not installed");
            return None;
        };
        aim_paths::input(aim_paths::original_image().join("system/bin/sh"), "image")?;
        // One copy, replaced by each run.
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("process-root");
        let _ = aim_linux_abi::cache::remove_tree(&root);
        for g in [
            "/apex/com.android.runtime",
            "/system/bin/linker64",
            "/system/bin/sh",
            "/system/lib64/libc.so",
            "/system/lib64/libm.so",
            "/system/lib64/libdl.so",
        ] {
            mirror(&root, g);
        }
        std::fs::create_dir_all(root.join("data/local/tmp/id/by-pid")).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guest/process.c");
        let out = Command::new(clang)
            .args(["-O1", "-Wall", "-Werror", "-o"])
            .arg(root.join(PROGRAM.trim_start_matches('/')))
            .arg(src)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(root)
    })
    .as_deref()
}

/// Run linux-run with `args`, killing it after a minute.
fn linux_run(root: &Path, args: &[&str], setup: impl FnOnce(&mut Command)) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_linux-run"));
    c.arg("--no-cache")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    setup(&mut c);
    let mut child = c.spawn().unwrap();
    let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out = std::thread::spawn(move || {
        let mut v = Vec::new();
        so.read_to_end(&mut v).unwrap();
        v
    });
    let err = std::thread::spawn(move || {
        let mut v = Vec::new();
        se.read_to_end(&mut v).unwrap();
        v
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn check(name: &str) {
    let Some(root) = root() else { return };
    let out = linux_run(root, &[PROGRAM, name], |_| {});
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        out.status.success() && so.contains(&format!("ok {name}\n")),
        "{name}: {:?}\nstdout:\n{so}\nstderr:\n{se}",
        out.status
    );
}

#[test]
fn fork_returns_twice_and_wait4_reports_the_exit() {
    check("fork_wait");
}

#[test]
fn a_forked_child_runs_code_it_makes_executable() {
    check("fork_new_code");
}

#[test]
fn mount_namespaces_bind_and_tmpfs() {
    check("mount_ns");
}

#[test]
fn xattrs_and_selinux_labels() {
    check("xattrs");
}

#[test]
fn a_pf_key_socket_opens_and_closes() {
    check("pf_key");
}

#[test]
fn an_empty_scm_rights_passes_no_control_message() {
    check("empty_rights");
}

#[test]
fn a_thread_with_its_own_file_table_keeps_the_spawners_fds() {
    check("own_files_thread");
}

/// `--stdio-null`: the guest's stdio is /dev/null as init gives services,
/// while the layer's own messages (here the syscall trace) still reach the
/// original stderr, also after the guest execs.
#[test]
fn stdio_null_keeps_the_layer_log() {
    let Some(root) = root() else { return };
    let out = linux_run(
        root,
        &[
            "--stdio-null",
            "--trace",
            "/system/bin/sh",
            "-c",
            "echo visible; echo visible >&2; exec /system/bin/sh -c 'echo again'",
        ],
        |_| {},
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(out.stdout.is_empty());
    assert!(!err.contains("visible"), "{err}");
    assert!(err.contains("[linux] exit_group"), "{err}");
    assert!(err.matches("/system/bin/sh loaded").count() >= 2, "{err}");
}

#[test]
fn a_pipe_connects_parent_and_child() {
    check("pipe_echo");
}

#[test]
fn vfork_execs_an_image_binary() {
    check("exec_image");
}

#[test]
fn execve_keeps_argv0_and_reports_execfn() {
    check("exec_argv");
}

#[test]
fn execve_runs_scripts_through_their_interpreter() {
    check("exec_script");
}

#[test]
fn waitid_reports_stops_continues_and_kills() {
    check("waitid_variants");
}

#[test]
fn pidfds_poll_readable_when_the_process_exits() {
    check("pidfd_poll");
}

#[test]
fn epoll_fds_survive_fork() {
    check("epoll_fork");
}

#[test]
fn fork_copies_private_memory_and_shares_shared_memory() {
    check("fork_memory");
}

#[test]
fn a_parent_that_exits_right_after_fork_leaves_a_running_child() {
    check("fork_then_exit");
}

#[test]
fn a_fatal_signal_ends_the_host_process_with_it() {
    check("death_by_signal");
}

#[test]
fn seccomp_filters_are_accepted() {
    check("seccomp_filter");
}

#[test]
fn credentials_follow_linux_rules_across_fork_and_exec() {
    check("identity");
}

#[test]
fn identity_files_inherited_env_and_fds_reach_the_guest() {
    let Some(root) = root() else { return };
    let id = root.join("data/local/tmp/id/svc");
    std::fs::write(
        &id,
        "# aim-guest-init identity v1\nservice\tsvc\nuid\t1000\ngid\t1001\ngroups\t3003 1065\n\
         cap_effective\t0x30c0\ncap_permitted\t0x30c0\ncap_inheritable\t0x30c0\n\
         cap_ambient\t0x30c0\ncap_bounding\t0x30c0\nseclabel\t\npriority\t10\n\
         oom_score_adj\t-1000\nrlimit\t13\t40\t40\n",
    )
    .unwrap();
    let mut fds = [0; 2];
    // SAFETY: a pipe whose read end the guest inherits as fd 3.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let id_arg = id.to_str().unwrap().to_string();
    let out = linux_run(
        root,
        &[
            "--identity",
            &id_arg,
            "--inherit-env",
            PROGRAM,
            "identity_file",
        ],
        |c| {
            c.env_clear().env("ANDROID_SOCKET_test", "3");
            let fd = fds[0];
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                c.pre_exec(move || {
                    if libc::dup2(fd, 3) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
            };
        },
    );
    // SAFETY: closing our pipe.
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        out.status.success() && so.contains("ok identity_file\n"),
        "{:?}\nstdout:\n{so}\nstderr:\n{se}",
        out.status
    );
}

/// A guest with a process table (`--identity`) lives in a pid namespace:
/// given the pid of a host process, every pid-taking call fails with
/// ESRCH, and the host process survives `kill`, `kill(-pgrp)` and
/// `kill(-1)`.
#[test]
fn a_guest_cannot_reach_host_processes() {
    let Some(root) = root() else { return };
    // Its own table, so no other test's processes are in it.
    let dir = root.join("data/local/tmp/pidns");
    std::fs::create_dir_all(dir.join("by-pid")).unwrap();
    let id = dir.join("guest");
    std::fs::write(
        &id,
        "# aim-guest-init identity v1\nservice\tguest\nuid\t10050\ngid\t10050\n",
    )
    .unwrap();
    host_process_unreachable(root, &["--identity", id.to_str().unwrap()]);
}

/// A guest started without a table (a test, a debugging shell) is alone in
/// a private namespace, as unable to reach host processes; its descendants
/// die with it and its table goes.
#[test]
fn a_standalone_guest_has_a_private_namespace() {
    let Some(root) = root() else { return };
    host_process_unreachable(root, &[]);

    let out = linux_run(root, &[PROGRAM, "ns_init_exit"], |_| {});
    let so = String::from_utf8_lossy(&out.stdout);
    let pids: Vec<i32> = so
        .strip_prefix("pids ")
        .map(|l| {
            l.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let [init, child] = pids[..] else {
        panic!("{so}\n{}", String::from_utf8_lossy(&out.stderr))
    };
    assert!(out.status.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    // SAFETY: probing a pid with signal 0.
    while unsafe { libc::kill(child, 0) } == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: as above.
    assert_ne!(
        unsafe { libc::kill(child, 0) },
        0,
        "the child died with init"
    );
    let table = std::env::temp_dir().join(format!("aim-pidns-{init}"));
    assert!(!table.exists(), "{} removed", table.display());
}

/// `pid_namespace` of `tests/guest/process.c` with a host process's pid.
fn host_process_unreachable(root: &Path, args: &[&str]) {
    let mut host = Command::new("/bin/sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let host_pid = host.id().to_string();
    let mut argv = args.to_vec();
    argv.extend([PROGRAM, "pid_namespace", &host_pid]);
    let out = linux_run(root, &argv, |_| {});
    let alive = host.try_wait().unwrap().is_none();
    let _ = host.kill();
    let _ = host.wait();
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        out.status.success() && so.contains("ok pid_namespace\n"),
        "{:?}\nstdout:\n{so}\nstderr:\n{se}",
        out.status
    );
    assert!(alive, "the host process survived");
}

/// The image's mksh forks, pipes and execs itself; its parent waits for
/// SIGCHLD in `sigsuspend`.
#[test]
fn mksh_runs_a_pipeline() {
    let Some(root) = root() else { return };
    let out = linux_run(
        root,
        &[
            "/system/bin/sh",
            "-c",
            "echo x | /system/bin/sh -c 'read a; echo got $a'; echo done",
        ],
        |_| {},
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "got x\ndone\n");
}

/// A `#!` script as linux-run's program runs its interpreter, as init's
/// execv of a script (otapreopt_slot) does on Linux.
#[test]
fn a_script_runs_as_the_program() {
    let Some(root) = root() else { return };
    let script = root.join("data/local/tmp/program.sh");
    std::fs::write(&script, "#!/system/bin/sh -e\necho \"$0|$1\"\n").unwrap();
    let exec = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&script, exec).unwrap();
    let out = linux_run(root, &["/data/local/tmp/program.sh", "arg"], |_| {});
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "/data/local/tmp/program.sh|arg\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// fork+exit+wait and fork+exec+exit+wait latency (printed, not asserted).
#[test]
fn process_latency() {
    let Some(root) = root() else { return };
    let out = linux_run(root, &[PROGRAM, "bench"], |_| {});
    eprintln!("{}", String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
}

/// fork+exit+wait of a process with zygote's shape: thousands of mappings
/// and hundreds of MiB of touched memory (printed, not asserted).
#[test]
fn fork_latency_with_a_large_address_space() {
    let Some(root) = root() else { return };
    let out = linux_run(root, &[PROGRAM, "bench_mappings", "4000", "300"], |_| {});
    eprintln!("{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
