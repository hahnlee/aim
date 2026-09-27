//! Evdev input devices (docs/input.md): the original `getevent` of the
//! pinned image lists the display server's devices with their capabilities
//! and state (`getevent -lp`) and reads the events the window's input
//! becomes (`getevent -l`). The host side is driven through
//! `darwin_host_display::input::translate::Input`, the path `darwin-display`
//! feeds from AppKit, so no window is needed.
//!
//! Skipped when the extracted image is absent.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use darwin_host_display::input::server::Devices;
use darwin_host_display::input::translate::{Input, Phase};
use darwin_host_display::input::{KEYBOARD, TOUCHSCREEN, WHEEL, device_dir, devices};
use darwin_host_display::monotonic_ns;

const IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);

fn image() -> Option<&'static Path> {
    let p = Path::new(IMAGE);
    if p.join("system/bin/toolbox").exists() {
        Some(p)
    } else {
        eprintln!("skipped: extracted image not found at {IMAGE}");
        None
    }
}

/// A display server's input side, in a short directory: device sockets
/// must fit `sun_path`.
struct Server {
    dir: PathBuf,
    socket: PathBuf,
    cache: PathBuf,
    input: Input,
}

impl Server {
    fn new(name: &str) -> Server {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("d.sock");
        let devs = Devices::create(&device_dir(&socket), devices(1080, 1920, 254.0, 254.0))
            .expect("devices");
        let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-cache"));
        std::fs::create_dir_all(&cache).unwrap();
        Server {
            dir,
            socket,
            cache,
            input: Input::new(devs, 1080, 1920),
        }
    }

    fn getevent(&self, image: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_linux-run"));
        c.arg("--cache")
            .arg(&self.cache)
            .arg("--root")
            .arg(image)
            .arg("--display")
            .arg(&self.socket)
            .arg("/system/bin/getevent")
            .args(args);
        c
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.input.close();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Output with runs of spaces collapsed, for comparing getevent's columns.
fn squeeze(s: &str) -> String {
    s.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait(child: &mut Child, limit: Duration) -> (bool, String) {
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            let mut out = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut out)
                .unwrap();
            let mut err = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut err)
                .unwrap();
            return (st.success(), format!("{out}{err}"));
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return (false, "timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn getevent_lists_devices_and_reads_window_input() {
    let Some(image) = image() else { return };
    let s = Server::new("evdev");
    let t = monotonic_ns;

    // Hold A and touch the middle of the window (a 540x960 point view of
    // the 1080x1920 display): -lp shows the key and the touch as state.
    s.input.key(0x00, true, false, t());
    s.input
        .pointer(Phase::Down, 270.0, 480.0, 540.0, 960.0, t());
    let out = s.getevent(image, &["-l", "-p", "-i"]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("{text}");
    assert!(out.status.success(), "{text}");
    let lp = squeeze(&text);
    for want in [
        // Devices are listed in directory order, as on Linux.
        ": /dev/input/event0\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"darwin-touchscreen\"",
        "id: \"\"\nversion: 1.0.1",
        "KEY (0001): BTN_TOUCH*",
        "ABS (0003): ABS_MT_SLOT : value 0, min 0, max 9, fuzz 0, flat 0, resolution 0",
        "ABS_MT_POSITION_X : value 0, min 0, max 1079, fuzz 0, flat 0, resolution 10",
        "ABS_MT_POSITION_Y : value 0, min 0, max 1919, fuzz 0, flat 0, resolution 10",
        "ABS_MT_TRACKING_ID : value 0, min 0, max 65535, fuzz 0, flat 0, resolution 0",
        "input props:\nINPUT_PROP_DIRECT",
        ": /dev/input/event1\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"darwin-keyboard\"",
        "KEY_A*",
        ": /dev/input/event2\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"darwin-wheel\"",
        "REL (0002): REL_WHEEL REL_WHEEL_HI_RES",
    ] {
        assert!(lp.contains(want), "missing {want:?} in:\n{lp}");
    }
    s.input.release_all(t());

    // Events, as the window reports them.
    let mut child = s
        .getevent(image, &["-l", "-c", "23"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while [TOUCHSCREEN, KEYBOARD, WHEEL]
        .iter()
        .any(|&i| s.input.devices().clients(i) == 0)
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "getevent never opened the devices"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    s.input
        .pointer(Phase::Down, 0.25, 959.75, 540.0, 960.0, t());
    s.input
        .pointer(Phase::Drag, 100.0, 900.0, 540.0, 960.0, t());
    // Out of the window: clamped to the edge.
    s.input.pointer(Phase::Drag, 600.0, -5.0, 540.0, 960.0, t());
    s.input.pointer(Phase::Up, 600.0, -5.0, 540.0, 960.0, t());
    s.input.key(0x00, true, false, t());
    s.input.key(0x00, true, true, t()); // AppKit's repeat: Android repeats
    s.input.key(0x00, false, false, t());
    // Right Shift, as flagsChanged reports it.
    s.input.flags_changed(0x3c, (1 << 17) | 0x4, t());
    s.input.scroll(1.0, false, t());
    let (ok, text) = wait(&mut child, Duration::from_secs(60));
    println!("{text}");
    assert!(ok, "{text}");
    // getevent reads one event per ready device per poll: compare each
    // device's own sequence.
    let lines = squeeze(&text);
    let of = |dev: &str| -> Vec<String> {
        lines
            .lines()
            .filter_map(|l| l.strip_prefix(&format!("/dev/input/{dev}: ")))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(
        of("event0"),
        [
            "EV_ABS ABS_MT_TRACKING_ID 00000001",
            "EV_ABS ABS_MT_POSITION_X 00000000",
            "EV_ABS ABS_MT_POSITION_Y 00000000",
            "EV_KEY BTN_TOUCH DOWN",
            "EV_SYN SYN_REPORT 00000000",
            "EV_ABS ABS_MT_POSITION_X 000000c8",
            "EV_ABS ABS_MT_POSITION_Y 00000078",
            "EV_SYN SYN_REPORT 00000000",
            "EV_ABS ABS_MT_POSITION_X 00000437",
            "EV_ABS ABS_MT_POSITION_Y 0000077f",
            "EV_SYN SYN_REPORT 00000000",
            "EV_ABS ABS_MT_TRACKING_ID ffffffff",
            "EV_KEY BTN_TOUCH UP",
            "EV_SYN SYN_REPORT 00000000",
        ]
    );
    assert_eq!(
        of("event1"),
        [
            "EV_KEY KEY_A DOWN",
            "EV_SYN SYN_REPORT 00000000",
            "EV_KEY KEY_A UP",
            "EV_SYN SYN_REPORT 00000000",
            "EV_KEY KEY_RIGHTSHIFT DOWN",
            "EV_SYN SYN_REPORT 00000000",
        ]
    );
    assert_eq!(
        of("event2"),
        [
            "EV_REL REL_WHEEL 00000001",
            "EV_REL REL_WHEEL_HI_RES 00000078",
            "EV_SYN SYN_REPORT 00000000",
        ]
    );
}

/// Devices that appear while getevent watches `/dev/input` (inotify) are
/// opened; their events carry `CLOCK_MONOTONIC` times (`EVIOCSCLOCKID`).
#[test]
fn getevent_sees_hotplugged_devices() {
    let Some(image) = image() else { return };
    let s = Server::new("evhot");
    // The devices go; only their directory stays.
    s.input.close();
    let mut child = s
        .getevent(image, &["-l", "-t", "-c", "2"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // getevent starts watching well within this.
    std::thread::sleep(Duration::from_secs(3));
    let devs = Devices::create(&device_dir(&s.socket), devices(1080, 1920, 254.0, 254.0))
        .expect("devices");
    let start = Instant::now();
    while devs.clients(KEYBOARD) == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the new keyboard was not opened"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let now = monotonic_ns();
    devs.emit(KEYBOARD, now, &[(1, 30, 1)]);
    let (ok, text) = wait(&mut child, Duration::from_secs(30));
    println!("{text}");
    assert!(ok, "{text}");
    assert!(text.contains("add device"), "{text}");
    // "[  sec.usec] /dev/input/event1: EV_KEY KEY_A DOWN"
    let line = text
        .lines()
        .find(|l| l.contains("KEY_A"))
        .unwrap_or_else(|| panic!("no key event in:\n{text}"));
    assert!(line.contains("/dev/input/event1: EV_KEY"), "{line}");
    let stamp: f64 = line
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        (stamp - now as f64 / 1e9).abs() < 1e-5,
        "time {stamp} is not the event's {now} ns"
    );
}
