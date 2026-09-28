//! NDK C tests of the file, socket, event, memory and procfs syscalls, run
//! under `linux-run` with the image's original bionic (`tests/ndk/*.c`).
//!
//! Each program is built with the pinned NDK and run from a
//! writable `/data/local/tmp` through a path map, like a service under
//! guest-init. Skipped when the NDK or the extracted image is missing.

use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let _ = std::fs::remove_dir_all(&runtime);
        for d in [
            "data/local/tmp",
            "dev/socket",
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
        let out = cmd.output().unwrap();
        drop(fds);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.runtime);
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

#[test]
fn sockets() {
    check("t_net", &["/data/local/tmp"]);
}

#[test]
fn network_devices() {
    check("t_netif", &[]);
}

#[test]
fn memory() {
    check("t_mem", &[]);
}

#[test]
fn procfs() {
    check("t_proc", &["two", "args"]);
}

#[test]
fn posix_timers() {
    check("t_timer", &[]);
}

#[test]
fn ashmem() {
    check("t_ashmem", &[]);
}

#[test]
fn memfd() {
    check("t_memfd", &[]);
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
    for name in ["aim-touchscreen", "aim-keyboard", "aim-wheel"] {
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
        "aim-wheel\n      Classes: ROTARY_ENCODER",
        // The sysfs root EventHub finds through /sys/dev/char.
        "SysfsDevicePath: /sys/devices/virtual\n",
        "key: device 0 code 48 value 1",
    ] {
        assert!(out.contains(want), "missing {want:?}");
    }
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
