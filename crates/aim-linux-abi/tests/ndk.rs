//! NDK C tests of the file, socket, event, memory and procfs syscalls, run
//! under `linux-run` with the image's original bionic (`tests/ndk/*.c`).
//!
//! Each program is built with the pinned NDK and run from a
//! writable `/data/local/tmp` through a path map, like a service under
//! guest-init. Skipped when the NDK or the extracted image is missing.

use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a program may run before it counts as hung and is killed.
const RUN_LIMIT: Duration = Duration::from_secs(300);

fn ndk_clang() -> Option<PathBuf> {
    aim_paths::ndk_clang(35)
}

/// The extracted pinned image (the `image` node of `cargo aim`).
fn image() -> Option<PathBuf> {
    aim_paths::original_image_with("system/build.prop")
}

struct Guest {
    runtime: PathBuf,
    map: PathBuf,
    cache: PathBuf,
}

impl Guest {
    fn new(image: &Path, name: &str) -> Guest {
        let runtime =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("n{}-{name}", std::process::id()));
        let _ = aim_linux_abi::cache::remove_tree(&runtime);
        for d in [
            "data/local/tmp",
            "dev/socket",
            "dev/pts",
            "tmp",
            "kernfs/proc",
            "kernfs/sys",
            "cache",
        ] {
            std::fs::create_dir_all(runtime.join(d)).unwrap();
        }
        // A value init wrote (docs/guest-init-contract.md section 7).
        std::fs::create_dir_all(runtime.join("kernfs/proc/sys/kernel")).unwrap();
        std::fs::write(runtime.join("kernfs/proc/sys/kernel/panic_on_oops"), "1\n").unwrap();
        // The kernel log's ring and /proc/kmsg, as guest-init makes them.
        std::fs::write(runtime.join("dev/kmsg"), b"").unwrap();
        std::fs::write(runtime.join("kernfs/proc/kmsg"), b"").unwrap();
        let map = runtime.join("path-map");
        let r = runtime.display();
        std::fs::write(
            &map,
            format!(
                "# aim-guest-init path map v1\nroot\t/\t{}\nrw\t/data\t{r}/data\nrw\t/dev\t{r}/dev\nrw\t/tmp\t{r}/tmp\nkernfs\t/proc\t{r}/kernfs/proc\nkernfs\t/sys\t{r}/kernfs/sys\n",
                image.display()
            ),
        )
        .unwrap();
        let cache = runtime.join("cache");
        Guest {
            runtime,
            map,
            cache,
        }
    }

    /// Build `tests/ndk/NAME.c`, or `NAME.cpp` with a static libc++.
    fn build(&self, clang: &Path, name: &str) -> String {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ndk");
        let mut src = dir.join(format!("{name}.c"));
        let mut cc = Command::new(clang);
        if !src.exists() {
            src = dir.join(format!("{name}.cpp"));
            let mut cxx = clang.as_os_str().to_owned();
            cxx.push("++");
            cc = Command::new(cxx);
            cc.arg("-static-libstdc++");
        }
        let out = self.runtime.join("data/local/tmp").join(name);
        let st = cc
            .args(["-O1", "-D_GNU_SOURCE", "-Wall", "-Werror", "-o"])
            .arg(&out)
            .arg(&src)
            .status()
            .unwrap();
        assert!(st.success(), "compiling {name}");
        format!("/data/local/tmp/{name}")
    }

    /// Show host file `host` at guest path `guest`.
    fn map(&self, guest: &str, host: &Path) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.map)
            .unwrap();
        writeln!(f, "rw\t{guest}\t{}", host.display()).unwrap();
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        self.run_with(args, Vec::new())
    }

    /// Run with `fds` inherited at 3, 4, ... as guest-init passes sockets.
    fn run_with(&self, args: &[&str], fds: Vec<OwnedFd>) -> (bool, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linux-run"));
        cmd.arg("--path-map")
            .arg(&self.map)
            .arg("--cache")
            .arg(&self.cache)
            .args(args);
        let raw: Vec<i32> = fds.iter().map(|f| f.as_raw_fd()).collect();
        // SAFETY: only async-signal-safe dup2 between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                // Out of the way first, so no target clobbers a source.
                let mut high = Vec::with_capacity(raw.len());
                for fd in &raw {
                    let h = libc::fcntl(*fd, libc::F_DUPFD, 64);
                    if h < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    high.push(h);
                }
                for (i, fd) in high.iter().enumerate() {
                    if libc::dup2(*fd, 3 + i as i32) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::close(*fd);
                }
                Ok(())
            });
        }
        // Its own process group, so a hung program goes with its children.
        cmd.process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        drop(fds);
        let pid = child.id() as i32;
        let read = |mut r: Box<dyn Read + Send>| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                let _ = r.read_to_end(&mut v);
                v
            })
        };
        let out = read(Box::new(child.stdout.take().unwrap()));
        let err = read(Box::new(child.stderr.take().unwrap()));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(child.wait().unwrap()));
        let status = rx.recv_timeout(RUN_LIMIT).ok();
        if status.is_none() {
            // SAFETY: the process group this test started.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let _ = rx.recv();
        }
        let mut text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.join().unwrap()),
            String::from_utf8_lossy(&err.join().unwrap())
        );
        if status.is_none() {
            text += &format!("\n[test] still running after {RUN_LIMIT:?}: killed\n");
        }
        (status.is_some_and(|s| s.success()), text)
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = aim_linux_abi::cache::remove_tree(&self.runtime);
    }
}

fn check(name: &str, args: &[&str]) {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, name);
    let prog = g.build(&clang, name);
    let mut argv = vec![prog.as_str()];
    argv.extend_from_slice(args);
    let (ok, out) = g.run(&argv);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "{name} failed:\n{out}");
}

#[test]
fn events() {
    check("t_event", &[]);
}

#[test]
fn files() {
    check("t_fs", &["/data/local/tmp"]);
}

/// Broker transport follows the last device description, including _exit.
#[test]
fn fuse_transport_lifetime() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_fuse_lifetime");
    let prog = g.build(&clang, "t_fuse_lifetime");
    let registry = g.runtime.join("fuse-sessions");
    let (mut host, guest) = UnixStream::pair().unwrap();
    host.set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let controller = std::thread::spawn(move || {
        let wait_removed = |path: &Path| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while path.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{} leaked",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let session = || {
            let entries = std::fs::read_dir(&registry)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(entries.len(), 1);
            PathBuf::from(String::from_utf8(std::fs::read(entries[0].path()).unwrap()).unwrap())
        };
        let mut command = [0];
        host.read_exact(&mut command).unwrap();
        assert_eq!(command, *b"d");
        let first = session();
        assert!(first.join("c.sock").exists());
        host.write_all(b"d").unwrap();
        host.read_exact(&mut command).unwrap();
        assert_eq!(command, *b"c");
        wait_removed(&first);
        host.write_all(b"c").unwrap();
        host.read_exact(&mut command).unwrap();
        assert_eq!(command, *b"e");
        // The short-lived second session's reaper may still be retiring.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::fs::read_dir(&registry).unwrap().count() != 1 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let exiting = session();
        assert!(exiting.join("c.sock").exists());
        host.write_all(b"e").unwrap();
        wait_removed(&exiting);
    });
    let (ok, out) = g.run_with(&[&prog], vec![guest.into()]);
    controller.join().unwrap();
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_fuse_lifetime failed:\n{out}");
}

#[test]
fn pseudo_terminals() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let guest = Guest::new(&image, "t_pty");
    let program = guest.build(&clang, "t_pty");
    for args in [
        vec![program.as_str()],
        vec![program.as_str(), "--allocate-close"],
    ] {
        let (ok, output) = guest.run(&args);
        println!("{output}");
        assert!(
            ok && output.contains("PASS"),
            "PTY owner process lifecycle failed: {output}"
        );
    }
}

#[test]
fn sockets() {
    check("t_net", &["/data/local/tmp"]);
}

/// Hold the Mac's UDP port 68 (the DHCP client port) for the test run, as
/// the Mac or another guest may: a guest's DHCP socket on `eth0` must not
/// need it. Someone else holding it already is as good.
fn hold_dhcp_client_port() {
    static HELD: std::sync::OnceLock<Option<std::net::UdpSocket>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| std::net::UdpSocket::bind(("0.0.0.0", 68)).ok());
}

#[test]
fn network_devices() {
    hold_dhcp_client_port();
    check("t_netif", &[]);
}

#[test]
fn network_device_statistics() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_netdev");
    let prog = g.build(&clang, "t_netdev");
    let (ok, out) = g.run(&[&prog]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_netdev failed:\n{out}");
    let (ok, out) = g.run(&["/system/bin/toybox", "ifconfig", "-a"]);
    println!("{out}");
    assert!(ok, "ifconfig failed:\n{out}");
    for name in ["lo", "eth0"] {
        assert!(
            out.lines()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "ifconfig did not list {name}:\n{out}"
        );
    }
}

/// IPv6 on eth0 (#267).
#[test]
fn network_ipv6() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_ipv6");
    let hook = g.runtime.join("net/simulate");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "").unwrap();
    g.map("/data/local/tmp/simulate", &hook);
    let prog = g.build(&clang, "t_ipv6");
    // A global IPv6 address the host has, for the guest's to stand for.
    let host = host_ipv6().map(|a| a.to_string()).unwrap_or_default();
    let (ok, out) = g.run(&[&prog, "/data/local/tmp/simulate", &host]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_ipv6 failed:\n{out}");
}

/// A global (not link-local, loopback or multicast) IPv6 address of the
/// host's, if it has one.
fn host_ipv6() -> Option<std::net::Ipv6Addr> {
    let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs into a list we free below.
    if unsafe { libc::getifaddrs(&mut ifa) } != 0 {
        return None;
    }
    let mut found = None;
    let mut p = ifa;
    // SAFETY: walking the list getifaddrs returned.
    unsafe {
        while !p.is_null() && found.is_none() {
            let e = &*p;
            p = e.ifa_next;
            if e.ifa_addr.is_null() || (*e.ifa_addr).sa_family as i32 != libc::AF_INET6 {
                continue;
            }
            let a = std::net::Ipv6Addr::from(
                (*(e.ifa_addr as *const libc::sockaddr_in6))
                    .sin6_addr
                    .s6_addr,
            );
            if !(a.is_loopback() || a.is_multicast() || a.segments()[0] & 0xffc0 == 0xfe80) {
                found = Some(a);
            }
        }
        libc::freeifaddrs(ifa);
    }
    found
}

/// The Mac's network changes, through the boot's test hook
/// (`<runtime>/net/simulate`) shown to the program.
#[test]
fn network_link_changes() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    hold_dhcp_client_port();
    let g = Guest::new(&image, "t_netwatch");
    let hook = g.runtime.join("net/simulate");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "").unwrap();
    g.map("/data/local/tmp/simulate", &hook);
    let prog = g.build(&clang, "t_netwatch");
    let (ok, out) = g.run(&[&prog, "/data/local/tmp/simulate"]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_netwatch failed:\n{out}");
}

/// A socket bound to eth0 follows the Mac's interface behind it, changed
/// through the boot's test hook.
#[test]
fn bound_device_follows_the_mac() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_bindif");
    let hook = g.runtime.join("net/simulate");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "").unwrap();
    g.map("/data/local/tmp/simulate", &hook);
    let prog = g.build(&clang, "t_bindif");
    let (ok, out) = g.run(&[&prog, "/data/local/tmp/simulate"]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_bindif failed:\n{out}");
}

#[test]
fn memory() {
    check("t_mem", &[]);
}

/// RLIMIT_AS and RLIMIT_DATA count the guest's mappings (#270).
#[test]
fn memory_limits() {
    check("t_rlimit", &[]);
}

#[test]
fn procfs() {
    check("t_proc", &["two", "args"]);
}

/// ptrace, process_vm_readv/writev and another process's `/proc`, as
/// debuggerd's crash_dump uses them.
/// Inherited kernel files keep their owners rather than becoming content files.
#[test]
fn kernel_knob_inheritance() {
    check("t_knob", &[]);
}

#[test]
fn ptrace() {
    check("t_ptrace", &[]);
}

#[test]
fn proc_mem() {
    check("t_proc_mem", &[]);
}

#[test]
fn posix_timers() {
    check("t_timer", &[]);
}

#[test]
fn cpu_timers() {
    check("t_cpu_timer", &[]);
}

#[test]
fn interval_timers() {
    check("t_itimer", &[]);
}

/// A process that dies of a fault its crash handler took (debuggerd's,
/// installed by the linker) is reported by the layer: signal, fault
/// address, and pc and lr with their mappings.
#[test]
fn fatal_fault_report() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_crash");
    let prog = g.build(&clang, "t_crash");
    let (ok, out) = g.run(&[prog.as_str()]);
    println!("{out}");
    assert!(!ok && out.contains("crashing"), "{out}");
    assert!(
        out.contains("fatal signal 11 after the guest's handler")
            && out.contains("fault addr 0x104")
            && out.contains("(/data/local/tmp/t_crash+"),
        "no fault report:\n{out}"
    );
}

#[test]
fn clocks() {
    check("t_clock", &[]);
}

#[test]
fn ashmem() {
    check("t_ashmem", &[]);
}

#[test]
fn audio_input_open() {
    check("t_audio", &[]);
}

#[test]
fn jit() {
    check("t_jit", &[]);
}

#[test]
fn memfd() {
    check("t_memfd", &[]);
}

/// vm.memfd_noexec at each scope, as init would write it.
#[test]
fn memfd_noexec() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_memfd_noexec");
    let vm = g.runtime.join("kernfs/proc/sys/vm");
    std::fs::create_dir_all(&vm).unwrap();
    let prog = g.build(&clang, "t_memfd");
    for scope in ["0", "1", "2"] {
        std::fs::write(vm.join("memfd_noexec"), format!("{scope}\n")).unwrap();
        let (ok, out) = g.run(&[&prog, "--noexec", scope]);
        println!("{out}");
        assert!(
            ok && out.contains("PASS"),
            "t_memfd_noexec {scope} failed:\n{out}"
        );
    }
}

/// fs.protected_hardlinks off, the kernel's default, and on (#779).
#[test]
fn protected_hardlinks() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_hardlink");
    let fs = g.runtime.join("kernfs/proc/sys/fs");
    std::fs::create_dir_all(&fs).unwrap();
    let prog = g.build(&clang, "t_hardlink");
    for value in ["0", "1"] {
        std::fs::write(fs.join("protected_hardlinks"), format!("{value}\n")).unwrap();
        let (ok, out) = g.run(&[&prog, "/data/local/tmp", value]);
        println!("{out}");
        assert!(
            ok && out.contains("PASS"),
            "t_hardlink {value} failed:\n{out}"
        );
    }
}

#[test]
fn kernel_log() {
    check("t_kmsg", &[]);
}

/// cgroup v1 hierarchies mounted as guest-init's SetupCgroups mounts them.
#[test]
fn cgroup_v1_hierarchies() {
    use aim_storage::cgroup_v1::{Controller, populate};
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_cgroup");
    let prog = g.build(&clang, "t_cgroup");
    for (controller, guest) in [
        (Controller::Cpu, "/dev/cpuctl"),
        (Controller::Cpuset, "/dev/cpuset"),
    ] {
        let host = g.runtime.join("cgroup-v1").join(controller.name());
        std::fs::create_dir_all(&host).unwrap();
        populate(controller, &host, None).unwrap();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&g.map)
            .unwrap();
        writeln!(f, "{}\t{guest}\t{}", controller.map_kind(), host.display()).unwrap();
    }
    let (ok, out) = g.run(&[&prog]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_cgroup failed:\n{out}");
}

/// Extended attributes' permission rules (xattr_permission, #780).
#[test]
fn extended_attribute_permissions() {
    check("t_xattr", &["/data/local/tmp"]);
}

/// preadv2/pwritev2's offsets and RWF flags (#1205).
#[test]
fn vectored_positional_flags() {
    check("t_rwf", &["/data/local/tmp"]);
}

/// A TCP connection's shutdowns as tcp_poll reports them (#1333).
#[test]
fn tcp_shutdown_readiness() {
    check("t_tcphup", &[]);
}

/// Anonymous mapping names in maps and smaps (#1993).
#[test]
fn anonymous_mapping_names() {
    check("t_vma_name", &[]);
}

#[test]
fn netlink_audit_and_generic() {
    check("t_nlfamilies", &[]);
}

#[test]
fn netlink_user_unicast() {
    check("t_nlunicast", &[]);
}

#[test]
fn ip_receive_options() {
    check("t_ipopts", &[]);
}

/// SO_SNDBUF/SO_RCVBUF and their FORCE forms as socket(7) describes them
/// (#369).
#[test]
fn socket_buffer_sizes() {
    check("t_sockbuf", &[]);
}

/// Path sockets' owners and permissions as unix(7) describes them (#778).
#[test]
fn unix_socket_permissions() {
    check("t_unixperm", &["/data/local/tmp"]);
}

/// FIONREAD on framed AF_UNIX and netlink sockets, as Linux answers it
/// (#371).
#[test]
fn socket_queued_bytes() {
    check("t_fionread", &[]);
}

/// The derived image's lmkd answers ActivityManager's protocol, with the
/// host-call module `memory` as its pressure source.
#[test]
fn lmkd() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let Some(lmkd) = aim_paths::input(aim_paths::daemon_bin("lmkd"), "daemon/lmkd") else {
        return;
    };
    let g = Guest::new(&image, "t_lmkd");
    let prog = g.build(&clang, "t_lmkd");
    std::fs::copy(&lmkd, g.runtime.join("data/local/tmp/lmkd")).unwrap();
    let (ok, out) = g.run(&[&prog, "/data/local/tmp/lmkd"]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_lmkd failed:\n{out}");
}

/// The host priority of the thread of `pid` named `name`.
fn thread_priority(pid: u32, name: &str) -> Option<i32> {
    const PROC_PIDLISTTHREADS: i32 = 6;
    let mut ids = [0u64; 256];
    // SAFETY: proc_pidinfo into local buffers of the sizes given.
    unsafe {
        let n = libc::proc_pidinfo(
            pid as i32,
            PROC_PIDLISTTHREADS,
            0,
            ids.as_mut_ptr().cast(),
            size_of_val(&ids) as i32,
        );
        for &id in &ids[..n.max(0) as usize / 8] {
            let mut ti: libc::proc_threadinfo = std::mem::zeroed();
            let size = size_of::<libc::proc_threadinfo>() as i32;
            if libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTHREADINFO,
                id,
                (&raw mut ti).cast(),
                size,
            ) == size
                && std::ffi::CStr::from_ptr(ti.pth_name.as_ptr()).to_bytes() == name.as_bytes()
            {
                return Some(ti.pth_priority);
            }
        }
    }
    None
}

/// Background scheduling (a nice value of 10 or more, SCHED_IDLE) lowers
/// the host thread's QoS, and the default brings it back. The priorities
/// are the threads' own whatever nice value linux-run starts at (a shell's
/// background job runs at nice 5).
#[test]
fn background_qos() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_qos");
    let prog = g.build(&clang, "t_qos");
    qos_steps(&g, &prog, 0);
    qos_steps(&g, &prog, 5);
}

/// A peer changes both the leader and a worker's nice values; each
/// target applies them to its Darwin thread when it leaves a syscall.
#[test]
fn remote_nice_changes_host_thread_priority() {
    use std::io::{BufRead, Write};
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_remote_nice");
    let prog = g.build(&clang, "t_remote_nice");
    let mut child = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--path-map")
        .arg(&g.map)
        .arg("--cache")
        .arg(&g.cache)
        .arg(&prog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let pid: u32 = line
        .trim()
        .strip_prefix("target ")
        .expect(&line)
        .parse()
        .unwrap();
    let mut differences = Vec::new();
    for name in ["main", "worker"] {
        for (nice, want) in [(19, 4), (10, 20), (-2, 33), (-19, 47), (0, 31)] {
            line.clear();
            out.read_line(&mut line).unwrap();
            let got = thread_priority(pid, &format!("remote-{name}"));
            if line.trim() != format!("{name} {nice}") || got != Some(want) {
                differences.push(format!("{line:?}: host {got:?}, expected {want}"));
            }
            writeln!(stdin).unwrap();
        }
    }
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    assert!(
        child.wait().unwrap().success() && rest.contains("PASS"),
        "{rest}"
    );
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// t_qos's steps with linux-run started `renice` above this process's
/// nice value.
fn qos_steps(g: &Guest, prog: &str, renice: i32) {
    use std::io::{BufRead, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_linux-run"));
    command
        .arg("--path-map")
        .arg(&g.map)
        .arg("--cache")
        .arg(&g.cache)
        .arg(prog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    // SAFETY: getpriority and setpriority are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            let nice = libc::getpriority(libc::PRIO_PROCESS, 0);
            if libc::setpriority(libc::PRIO_PROCESS, 0, nice + renice) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    // Darwin's base priorities of the default, utility and background QoS,
    // then of raised threads: the timeshare priority 31 - nice (within
    // 15-47) and the fixed priority 47 of the real-time policies.
    for (step, want) in [
        ("default", 31),
        ("utility", 20),
        ("background", 4),
        ("default", 31),
        ("background", 4),
        ("foreground", 33),
        ("urgent-audio", 47),
        ("fifo", 47),
        ("default", 31),
        ("fifo", 47),
    ] {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), step);
        assert_eq!(
            thread_priority(child.id(), "qos-worker"),
            Some(want),
            "{step} at nice +{renice}"
        );
        writeln!(stdin).unwrap();
    }
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(
        child.wait().unwrap().success() && rest.contains("PASS"),
        "{rest}"
    );
}

/// A zip whose members are stored, each one's data aligned to `align`
/// with padding in its local header's extra field (as zipalign does).
fn stored_zip(members: &[(&str, &[u8])], align: usize) -> Vec<u8> {
    let (mut out, mut dir) = (Vec::new(), Vec::new());
    for &(name, data) in members {
        let header = out.len() as u32;
        let pad = (align - (out.len() + 30 + name.len()) % align) % align;
        let sizes = [(data.len() as u32).to_le_bytes(); 2].concat();
        out.extend_from_slice(b"PK\x03\x04\x0a\0\0\0\0\0\0\0\0\0\0\0\0\0");
        out.extend_from_slice(&sizes);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(pad as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.resize(out.len() + pad, 0);
        out.extend_from_slice(data);
        dir.extend_from_slice(b"PK\x01\x02\x0a\0\x0a\0\0\0\0\0\0\0\0\0\0\0\0\0");
        dir.extend_from_slice(&sizes);
        dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
        dir.extend_from_slice(&[0; 12]);
        dir.extend_from_slice(&header.to_le_bytes());
        dir.extend_from_slice(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    out.extend_from_slice(&dir);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&[(members.len() as u16).to_le_bytes(); 2].concat());
    out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

/// A native library loaded straight from an APK (stored, page-aligned):
/// its code is rewritten at load time, and the sites found in the first
/// process are kept in the translation cache for the next ones.
#[test]
fn apk_library() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_apklib");
    let prog = g.build(&clang, "t_apklib");
    let so = g.runtime.join("libapk.so");
    let st = Command::new(&clang)
        .args(["-O1", "-shared", "-fPIC", "-Wall", "-Werror", "-o"])
        .arg(&so)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ndk/apk_lib.c"))
        .status()
        .unwrap();
    assert!(st.success(), "compiling apk_lib.c");
    let apk = stored_zip(
        &[
            ("classes.dex", b"dex\n035\0"),
            ("lib/arm64-v8a/libapk.so", &std::fs::read(&so).unwrap()),
        ],
        16384,
    );
    let at = apk.windows(4).position(|w| w == b"\x7fELF").unwrap() as u64;
    let host = g.runtime.join("data/local/tmp/app.apk");
    std::fs::write(&host, &apk).unwrap();
    for run in 0..2 {
        let (ok, out) = g.run(&[&prog, "/data/local/tmp/app.apk!/lib/arm64-v8a/libapk.so"]);
        println!("{out}");
        assert!(
            ok && out.contains("PASS"),
            "t_apklib (run {run}) failed:\n{out}"
        );
        // The first run recorded the library's sites: the svc and the
        // thread pointer read.
        let cache = aim_linux_abi::cache::Cache::new(&g.cache);
        let stat = aim_linux_abi::cache::FileStat::of_path(&host).unwrap();
        let member = aim_linux_abi::cache::member_path(&std::fs::canonicalize(&host).unwrap(), at);
        let sites = cache
            .lookup_sites(&member, &stat)
            .expect("the library's sites");
        assert!(sites.len() >= 2, "{sites:?}");
    }
}

/// The evdev devices of a display server (`linux-run --display`), with
/// KEY_A held on its keyboard.
#[test]
fn evdev() {
    use aim_host_display::input::{KEYBOARD, device_dir, devices, server::Devices};
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_evdev");
    let prog = g.build(&clang, "t_evdev");
    let socket = g.runtime.join("display.sock");
    let devs = Devices::create(&device_dir(&socket), devices(1080, 1920, 254.0, 254.0)).unwrap();
    devs.emit(KEYBOARD, aim_host_display::monotonic_ns(), &[(1, 30, 1)]);
    let (ok, out) = g.run(&["--display", socket.to_str().unwrap(), &prog]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_evdev failed:\n{out}");
}

/// The original EventHub (libinputreader.so) opens and classifies the
/// devices with the derived image's `.idc` files and reads their events.
#[test]
fn eventhub() {
    use aim_host_display::input::{device_dir, devices, server::Devices};
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_eventhub");
    let prog = g.build(&clang, "t_eventhub");
    let idc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../image/vendor/usr/idc");
    for name in ["aim-touchscreen", "aim-keyboard", "aim-mouse"] {
        g.map(
            &format!("/vendor/usr/idc/{name}.idc"),
            &idc.join(format!("{name}.idc")),
        );
    }
    let socket = g.runtime.join("display.sock");
    let _devs = Devices::create(&device_dir(&socket), devices(1080, 1920, 254.0, 254.0)).unwrap();
    let (ok, out) = g.run(&["--display", socket.to_str().unwrap(), &prog]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_eventhub failed:\n{out}");
    for want in [
        "aim-touchscreen\n      Classes: TOUCH | TOUCH_MT\n      Path: /dev/input/event0",
        // keyboard.builtIn, from its .idc; the image's key layout.
        "aim-keyboard (aka device 0 - built-in keyboard)\n      Classes: KEYBOARD | ALPHAKEY",
        "KeyLayoutFile: /system/usr/keylayout/Generic.kl",
        // A pointer device (touch.deviceType = pointer): its events are a
        // mouse's.
        "aim-mouse\n      Classes: TOUCH\n      Path: /dev/input/event2",
        // The sysfs root EventHub finds through /sys/dev/char.
        "SysfsDevicePath: /sys/devices/virtual\n",
        "key: device 0 code 48 value 1",
    ] {
        assert!(out.contains(want), "missing {want:?}");
    }
}

/// uevent sockets, the evdev nodes' `uevent` attributes, and sysfs device
/// trees that ignore a value init wrote for a device that does not exist.
#[test]
fn uevents() {
    use aim_host_display::input::{device_dir, devices, server::Devices};
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_uevent");
    let usb = g.runtime.join("kernfs/sys/class/android_usb/android0");
    std::fs::create_dir_all(&usb).unwrap();
    std::fs::write(usb.join("enable"), "0").unwrap();
    let prog = g.build(&clang, "t_uevent");
    let socket = g.runtime.join("display.sock");
    let _devs = Devices::create(&device_dir(&socket), devices(1080, 1920, 254.0, 254.0)).unwrap();
    let (ok, out) = g.run(&["--display", socket.to_str().unwrap(), &prog]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_uevent failed:\n{out}");
}

#[test]
fn uevents_hotplug() {
    use aim_host_display::input::{device_dir, devices, server::Devices};
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_uevent_hotplug");
    let prog = g.build(&clang, "t_uevent");
    let socket = g.runtime.join("display.sock");
    let dir = device_dir(&socket);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut host, guest) = UnixStream::pair().unwrap();
    host.set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let controller = std::thread::spawn(move || {
        let mut live = None;
        let mut commands = Vec::new();
        let mut command = [0];
        while host.read(&mut command).unwrap() == 1 {
            commands.push(command[0]);
            match command[0] {
                b'a' => {
                    live = Some(Devices::create(&dir, devices(1080, 1920, 254.0, 254.0)).unwrap())
                }
                b'r' => drop(live.take().unwrap()),
                b'q' => {
                    // The lifetime must not be lost if a directory watcher
                    // would see the same empty directory before and after.
                    drop(Devices::create(&dir, devices(1080, 1920, 254.0, 254.0)).unwrap());
                }
                _ => panic!("unexpected hotplug command"),
            }
            host.write_all(&command).unwrap();
        }
        commands
    });
    let (ok, out) = g.run_with(
        &["--display", socket.to_str().unwrap(), &prog, "hotplug"],
        vec![guest.into()],
    );
    assert_eq!(controller.join().unwrap(), b"arq");
    println!("{out}");
    assert!(
        ok && out.contains("PASS"),
        "t_uevent hotplug failed:\n{out}"
    );
}

unsafe extern "C" {
    fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
}

/// An AF_UNIX socket bound at `dir/name` by its short name, as guest-init
/// binds (the host path may exceed `sun_path`).
fn bind_in(dir: &Path, name: &str, ty: i32) -> OwnedFd {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    // SAFETY: a fresh socket, bound with this thread's cwd set to `dir`.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, ty, 0);
        assert!(fd >= 0);
        let dfd = libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY);
        assert_eq!(pthread_fchdir_np(dfd), 0);
        let mut a: libc::sockaddr_un = std::mem::zeroed();
        a.sun_family = libc::AF_UNIX as u8;
        for (i, b) in name.bytes().enumerate() {
            a.sun_path[i] = b as libc::c_char;
        }
        let r = libc::bind(
            fd,
            (&a as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&a) as u32,
        );
        pthread_fchdir_np(-1);
        libc::close(dfd);
        assert_eq!(r, 0, "bind {name}");
        OwnedFd::from_raw_fd(fd)
    }
}

#[test]
fn inherited_init_sockets() {
    let (Some(clang), Some(image)) = (ndk_clang(), image()) else {
        aim_paths::skip("the pinned NDK or the extracted image is missing");
        return;
    };
    let g = Guest::new(&image, "t_inherit");
    let prog = g.build(&clang, "t_inherit");
    // What guest-init's CreateSocket does for logd's socket lines.
    let dir = g.runtime.join("dev/socket");
    let logdr = bind_in(&dir, "logdr", libc::SOCK_STREAM);
    // SAFETY: listen on our socket.
    assert_eq!(unsafe { libc::listen(logdr.as_raw_fd(), 8) }, 0);
    let logdw = bind_in(&dir, "logdw", libc::SOCK_DGRAM);
    std::fs::write(
        g.runtime.join("sockets"),
        "logd\t/dev/socket/logdr\tseqpacket\tstream\t-\tlisten\nlogd\t/dev/socket/logdw\tdgram\tdgram\tpasscred\t-\n",
    )
    .unwrap();
    let (ok, out) = g.run_with(&[&prog], vec![logdr, logdw]);
    println!("{out}");
    assert!(ok && out.contains("PASS"), "t_inherit failed:\n{out}");
}
