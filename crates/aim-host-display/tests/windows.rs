//! Android FRONT records reorder native task windows while their host is
//! inactive, without activating it. Uses only disposable display servers;
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
use objc::{Id, class, nsstring, release};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Id;
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

struct Server {
    child: Child,
    socket: std::path::PathBuf,
    log: BufReader<ChildStderr>,
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
        let mut server = Self { child, socket, log };
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
        let _ = std::fs::remove_file(&self.socket);
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

/// On-screen normal windows, front to back; no titles or pixels are read.
fn stacking() -> Vec<u32> {
    // SAFETY: a copied, toll-free bridged CFArray of window dictionaries.
    let list = unsafe { CGWindowListCopyWindowInfo(1 | (1 << 4), 0) };
    assert!(!list.is_null(), "window server list");
    let mut result = Vec::new();
    for i in 0..send!(list, c"count" => usize) {
        let w = send!(list, c"objectAtIndex:" => Id, usize = i);
        let level = send!(w, c"objectForKey:" => Id, Id = nsstring("kCGWindowLayer"));
        if send!(level, c"intValue" => i32) == 0 {
            let n = send!(w, c"objectForKey:" => Id, Id = nsstring("kCGWindowNumber"));
            result.push(send!(n, c"unsignedIntValue" => u32));
        }
    }
    release(list);
    result
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

fn above(first: u32, second: u32) -> bool {
    let stack = stacking();
    match (
        stack.iter().position(|&n| n == first),
        stack.iter().position(|&n| n == second),
    ) {
        (Some(a), Some(b)) => a < b,
        _ => false,
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
    wait("active FRONT", || above(first, second));

    // Another process we own takes keyboard focus, as another shim would.
    let other = Server::new("other", "device");
    wait("other host active", || other.active() && !host.active());
    for (task, front, back) in [(2, second, first), (1, first, second), (2, second, first)] {
        record(&mut bridge, window::FRONT, task);
        wait("inactive FRONT", || {
            stacking().first() == Some(&front) && above(front, back)
        });
        assert!(!host.active(), "FRONT activated its host");
        assert!(other.active(), "FRONT took keyboard focus");
    }

    record(&mut bridge, window::MOVED_TO_BACK, 2);
    wait("task minimized", || !stacking().contains(&second));
    record(&mut bridge, window::FRONT, 2);
    // Removal is a barrier: all prior bridge records ran.
    record(&mut bridge, window::REMOVED, 1);
    wait("other task removed", || !stacking().contains(&first));
    assert!(
        !stacking().contains(&second),
        "FRONT restored a minimized window"
    );
    assert!(!host.active());
    assert!(other.active());
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
}
