//! Android FRONT records reorder native task windows while their host is
//! inactive, below another Mac app and without activating it. Uses disposable hosts;
//! runs on the main thread so NSRunningApplication's properties update.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::time::{Duration, Instant};

use aim_host_display::wire::{self, Request};
use aim_hostcall::display::{Window, Windows, mode, window};

#[macro_use]
#[allow(dead_code)]
#[path = "../src/bin/aim-display/objc.rs"]
mod objc;
use objc::{CGPoint, CGRect, CGSize, Id, class, release};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

struct Server {
    child: Child,
    socket: std::path::PathBuf,
    log: BufReader<ChildStderr>,
    bundle: Option<std::path::PathBuf>,
}

impl Server {
    fn new(name: &str, mode: &str) -> Self {
        let socket = std::env::temp_dir().join(format!("dw-{}-{name}.sock", std::process::id()));
        let mut command = Command::new(env!("CARGO_BIN_EXE_aim-display"));
        command.args(["--mode", mode]);
        if mode == "device" {
            command.args(["--size", "64x48"]);
        }
        let mut child = command
            .arg("--socket")
            .arg(&socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let log = BufReader::new(child.stderr.take().unwrap());
        let mut server = Self {
            child,
            socket,
            log,
            bundle: None,
        };
        let mut banner = [0; 32];
        if let Err(e) = server
            .child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut banner)
        {
            let mut log = String::new();
            server.log.read_to_string(&mut log).unwrap();
            panic!("display server: {e}: {log}");
        }
        server
    }

    fn shim(server: &Server) -> Self {
        let bundle = std::env::temp_dir().join(format!("dw-{}.app", std::process::id()));
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        let exe = contents.join("MacOS/aim-display");
        std::fs::copy(env!("CARGO_BIN_EXE_aim-display"), &exe).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>aim-display</string>
<key>CFBundleIdentifier</key><string>dev.aim.test.windows</string>
<key>AIMPackage</key><string>dev.aim.test</string>
<key>AIMActivity</key><string>Main</string>
<key>AIMDisplaySocket</key><string>{}</string>
</dict></plist>"#,
                server.socket.display()
            ),
        )
        .unwrap();
        let mut child = Command::new(exe)
            .arg("--notifications")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let log = BufReader::new(child.stderr.take().unwrap());
        Self {
            child,
            socket: server.socket.clone(),
            log,
            bundle: Some(bundle),
        }
    }

    fn app(&self) -> Id {
        send!(class(c"NSRunningApplication"), c"runningApplicationWithProcessIdentifier:" => Id,
            i32 = self.child.id() as i32)
    }

    fn active(&self) -> bool {
        send!(self.app(), c"isActive" => bool)
    }

    fn bridge(&self) -> UnixStream {
        let mut sock = UnixStream::connect(&self.socket).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        wire::send(
            sock.as_fd(),
            wire::bytes(&Request {
                op: wire::OP_WINDOWS,
                id: wire::VERSION,
                ..Default::default()
            }),
            None,
        )
        .unwrap();
        let mut answer = [0; size_of::<Windows>()];
        sock.read_exact(&mut answer).unwrap();
        assert_eq!(
            u32::from_ne_bytes(answer[..4].try_into().unwrap()),
            mode::WINDOWS
        );
        sock
    }

    fn task(&mut self, bridge: &mut UnixStream, task: i32) -> u32 {
        record(bridge, window::TASK, task);
        let prefix = format!("aim-display: task {task} window ");
        loop {
            let mut line = String::new();
            assert!(self.log.read_line(&mut line).unwrap() > 0, "server gone");
            if let Some(number) = line.strip_prefix(&prefix) {
                return number.trim().parse().unwrap();
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(bundle) = &self.bundle {
            let _ = std::fs::remove_dir_all(bundle);
        } else {
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

fn record(sock: &mut UnixStream, op: u32, task: i32) {
    sock.write_all(wire::bytes(&Window {
        op,
        task,
        bounds: [200, 300, 600, 700],
        ..Default::default()
    }))
    .unwrap();
}

/// Hit-test only the shared point of the disposable test windows. No
/// desktop window list, titles or pixels are read.
fn at_point(below: u32) -> u32 {
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let frame = send!(screen, c"frame" => CGRect);
    let scale = send!(screen, c"backingScaleFactor" => f64);
    let point = CGPoint {
        x: 400.0 / scale,
        y: frame.size.height - 500.0 / scale,
    };
    send!(class(c"NSWindow"), c"windowNumberAtPoint:belowWindowWithWindowNumber:" => isize,
        CGPoint = point, isize = below as isize) as u32
}

fn wait(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "{what}");
        let until = send!(class(c"NSDate"), c"dateWithTimeIntervalSinceNow:" => Id, f64 = 0.02);
        let run_loop = send!(class(c"NSRunLoop"), c"mainRunLoop" => Id);
        send!(run_loop, c"runUntilDate:" => (), Id = until);
    }
}

fn main() {
    let _pool = objc::Pool::new();
    // SAFETY: returns an owned Metal device, or nil when none is available.
    let device = unsafe { MTLCreateSystemDefaultDevice() };
    if device.is_null() {
        aim_paths::skip("no Metal device for the window hosts");
        return;
    }
    release(device);
    if let Some(reason) = aim_host_display::screen::hidden() {
        aim_paths::skip(reason);
        return;
    }
    let mut host = Server::new("tasks", "windows");
    let mut bridge = host.bridge();
    let first = host.task(&mut bridge, 1);
    let second = host.task(&mut bridge, 2);
    wait("task host active", || host.active());
    record(&mut bridge, window::FRONT, 1);
    wait("active FRONT", || at_point(0) == first);

    // A cooperating host reports its own NSWindow number to the server.
    let mut peer = Server::shim(&host);
    bridge
        .write_all(wire::bytes(&Window::with_text(
            window::PACKAGE,
            3,
            "dev.aim.test",
        )))
        .unwrap();
    let third = peer.task(&mut bridge, 3);
    wait("shim active", || peer.active() && at_point(0) == third);

    // This test process is a separate Mac app, outside the AIM handoff.
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"setActivationPolicy:" => bool, isize = 0);
    send!(app, c"finishLaunching" => ());
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let frame = send!(screen, c"frame" => CGRect);
    let scale = send!(screen, c"backingScaleFactor" => f64);
    let cover = send!(class(c"NSWindow"), c"alloc" => Id);
    let cover = send!(cover, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = CGRect { x: 200.0 / scale, y: frame.size.height - 700.0 / scale,
            size: CGSize { width: 400.0 / scale, height: 400.0 / scale } },
        usize = 1 | 2 | 4 | 8, usize = 2, bool = false);
    send!(cover, c"setReleasedWhenClosed:" => (), bool = false);
    send!(cover, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
    send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
    let cover_number = send!(cover, c"windowNumber" => isize) as u32;
    wait("non-AIM app active", || {
        send!(app, c"isActive" => bool) && at_point(0) == cover_number
    });
    for (task, front) in [(2, second), (3, third), (1, first), (3, third), (2, second)] {
        record(&mut bridge, window::FRONT, task);
        wait("inactive FRONT below non-AIM window", || {
            at_point(cover_number) == front
        });
        assert_eq!(at_point(0), cover_number, "FRONT covered another Mac app");
        assert!(
            !host.active() && !peer.active(),
            "FRONT activated an AIM host"
        );
        assert!(send!(app, c"isActive" => bool), "FRONT took keyboard focus");
    }

    record(&mut bridge, window::MOVED_TO_BACK, 2);
    wait("task minimized", || at_point(cover_number) != second);
    record(&mut bridge, window::FRONT, 1);
    wait("front after minimization", || {
        at_point(cover_number) == first
    });
    record(&mut bridge, window::FRONT, 2);
    // Removal is a barrier: all prior records on this host ran.
    record(&mut bridge, window::REMOVED, 1);
    wait("other task removed", || at_point(cover_number) != first);
    assert_eq!(
        at_point(cover_number),
        third,
        "FRONT restored a minimized window"
    );
    assert_eq!(at_point(0), cover_number);
    assert!(!host.active() && !peer.active());
    assert!(send!(app, c"isActive" => bool));
    send!(cover, c"close" => ());
    release(cover);
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
}
