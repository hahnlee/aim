//! Evdev input devices (docs/input.md): the original `getevent` of the
//! pinned image lists the display server's devices with their capabilities
//! and state (`getevent -lp`) and reads the events the window's input
//! becomes (`getevent -l`). The host side is driven through
//! `aim_host_display::input::translate::Input`, the path `aim-display`
//! feeds from AppKit, so no window is needed.
//!
//! Skipped when the extracted image is absent.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use aim_host_display::input::server::Devices;
use aim_host_display::input::translate::{Button, Input, Phase, Scroll};
use aim_host_display::input::{KEYBOARD, MOUSE, TOUCHSCREEN, device_dir, devices};
use aim_host_display::monotonic_ns;

/// The extracted pinned image (the `image` node of `cargo aim`).
fn image() -> Option<PathBuf> {
    aim_paths::original_image_with("system/bin/toolbox")
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
        let _ = aim_linux_abi::cache::remove_tree(&dir);
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
            input: Input::new(devs, 1080, 1920, 254.0),
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
        let _ = aim_linux_abi::cache::remove_tree(&self.dir);
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
            // Unless the caller reads it.
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                stdout.read_to_string(&mut out).unwrap();
            }
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

/// getevent's output as it comes: it disables stdout's buffer.
fn stream(child: &mut Child) -> Receiver<String> {
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = stdout.read(&mut buf) {
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
        }
    });
    rx
}

/// Reads getevent's output into `text` until it has reported each of
/// `devices` added: opened, with its clock chosen (`EVIOCSCLOCKID`). The
/// clock flushes what was queued, so an event sent before is gone, as on
/// Linux.
fn until_added(rx: &Receiver<String>, text: &mut String, devices: &[u32]) {
    let start = Instant::now();
    for d in devices {
        let node = format!(": /dev/input/event{d}\n");
        while !text
            .split_inclusive('\n')
            .any(|l| l.starts_with("add device") && l.ends_with(&node))
        {
            let left = Duration::from_secs(60).saturating_sub(start.elapsed());
            match rx.recv_timeout(left) {
                Ok(chunk) => *text += &chunk,
                Err(_) => panic!("getevent did not open event{d}:\n{text}"),
            }
        }
    }
}

#[test]
fn getevent_lists_devices_and_reads_window_input() {
    let Some(ref image) = image() else { return };
    let s = Server::new("evdev");
    let t = monotonic_ns;

    // Hold A and touch the middle of the window (a 540x960 point view of
    // the 1080x1920 display): -lp shows the key and the touch as state.
    s.input.key(0x00, true, false, 0, t());
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
        ": /dev/input/event0\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"aim-touchscreen\"",
        "id: \"\"\nversion: 1.0.1",
        "KEY (0001): BTN_TOUCH*",
        "ABS (0003): ABS_MT_SLOT : value 0, min 0, max 9, fuzz 0, flat 0, resolution 0",
        "ABS_MT_POSITION_X : value 0, min 0, max 1079, fuzz 0, flat 0, resolution 10",
        "ABS_MT_POSITION_Y : value 0, min 0, max 1919, fuzz 0, flat 0, resolution 10",
        "ABS_MT_TRACKING_ID : value 0, min 0, max 65535, fuzz 0, flat 0, resolution 0",
        "input props:\nINPUT_PROP_DIRECT",
        ": /dev/input/event1\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"aim-keyboard\"",
        "KEY_A*",
        ": /dev/input/event2\nbus: 0006\nvendor 0000\nproduct 0000\nversion 0001\nname: \"aim-mouse\"",
        "REL (0002): REL_HWHEEL REL_WHEEL REL_WHEEL_HI_RES REL_HWHEEL_HI_RES",
        "ABS (0003): ABS_X : value 0, min 0, max 1079, fuzz 0, flat 0, resolution 10",
        "input props:\nINPUT_PROP_POINTER",
    ] {
        assert!(lp.contains(want), "missing {want:?} in:\n{lp}");
    }
    s.input.release_all(t());

    // Events, as the window reports them.
    let mut child = s
        .getevent(image, &["-l", "-c", "34"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = stream(&mut child);
    let mut text = String::new();
    until_added(&rx, &mut text, &[TOUCHSCREEN, KEYBOARD, MOUSE]);
    s.input
        .pointer(Phase::Down, 0.25, 959.75, 540.0, 960.0, t());
    s.input
        .pointer(Phase::Drag, 100.0, 900.0, 540.0, 960.0, t());
    // Out of the window: clamped to the edge.
    s.input.pointer(Phase::Drag, 600.0, -5.0, 540.0, 960.0, t());
    s.input.pointer(Phase::Up, 600.0, -5.0, 540.0, 960.0, t());
    s.input.key(0x00, true, false, 0, t());
    s.input.key(0x00, true, true, 0, t()); // AppKit's repeat: Android repeats
    s.input.key(0x00, false, false, 0, t());
    // Right Shift, as flagsChanged reports it.
    s.input.flags_changed(0x3c, (1 << 17) | 0x4, t());
    // The mouse: hover in the middle, a right click, a wheel line and a
    // trackpad's 128 pixels to the left (content moving left: scrolling
    // right).
    s.input.hover(540.0, 960.0, t());
    s.input.button(Button::Right, true, 540.0, 960.0, t());
    s.input.button(Button::Right, false, 540.0, 960.0, t());
    let at = Scroll {
        x: 540.0,
        y: 960.0,
        ..Default::default()
    };
    s.input.scroll(Scroll { dy: 1.0, ..at }, t());
    s.input.scroll(
        Scroll {
            dx: -128.0,
            precise: true,
            ..at
        },
        t(),
    );
    let (ok, err) = wait(&mut child, Duration::from_secs(60));
    text.extend(rx);
    let text = text + &err;
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
            "EV_KEY BTN_TOOL_MOUSE DOWN",
            "EV_ABS ABS_X 0000021c",
            "EV_ABS ABS_Y 000003c0",
            "EV_SYN SYN_REPORT 00000000",
            "EV_KEY BTN_RIGHT DOWN",
            "EV_SYN SYN_REPORT 00000000",
            "EV_KEY BTN_RIGHT UP",
            "EV_SYN SYN_REPORT 00000000",
            "EV_REL REL_WHEEL 00000001",
            "EV_REL REL_WHEEL_HI_RES 00000078",
            "EV_SYN SYN_REPORT 00000000",
            "EV_REL REL_HWHEEL 00000001",
            "EV_REL REL_HWHEEL_HI_RES 00000078",
            "EV_SYN SYN_REPORT 00000000",
        ]
    );
}

/// The nodes of a display server that died without removing them (its
/// device directory is not locked) are removed when a client finds them.
#[test]
fn stale_nodes_are_removed() {
    let Some(ref image) = image() else { return };
    let s = Server::new("evstale");
    let dir = device_dir(&s.socket);
    s.input.close();
    // What a SIGKILLed server leaves: a bound socket nobody listens on.
    let stale = std::os::unix::net::UnixListener::bind(dir.join("event0")).unwrap();
    drop(stale);
    let out = s.getevent(image, &["-l", "-p"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    println!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(!text.contains("event0"), "{text}");
    assert!(!dir.join("event0").exists());
}

/// Devices that appear while getevent watches `/dev/input` (inotify) are
/// opened; their events carry `CLOCK_MONOTONIC` times (`EVIOCSCLOCKID`).
#[test]
fn getevent_sees_hotplugged_devices() {
    let Some(ref image) = image() else { return };
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
    let rx = stream(&mut child);
    let mut text = String::new();
    let devs = Devices::create(&device_dir(&s.socket), devices(1080, 1920, 254.0, 254.0))
        .expect("devices");
    until_added(&rx, &mut text, &[KEYBOARD]);
    let now = monotonic_ns();
    devs.emit(KEYBOARD, now, &[(1, 30, 1)]);
    let (ok, err) = wait(&mut child, Duration::from_secs(30));
    // The rest, to the end of its output.
    text.extend(rx);
    let text = text + &err;
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
