//! Window hosts, in the display server (`docs/windows.md`, "App shims"):
//! the processes that show one package's task windows under the app's own
//! Dock icon (`shim.rs`).
//!
//! A host gets the task records of its package and the frames to show: the
//! buffers once each, as their memfds, and every present. A present waits
//! (at most [`SAMPLE_MS`]) until each host has read its buffer, so the
//! composer may reuse it, and the present fence waits for them too. The
//! host's requests go to the task bridge, and its input to the server's
//! devices, where touches wait for their task to come to the front like
//! the server's own.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use aim_host_display::input::translate::{Button, Gesture, Phase, Scroll, Twist};
use aim_host_display::wire::{self, Host as Rec, HostInput, host, input};
use aim_hostcall::display::{Import, Window as Record, window};

use crate::cursor::Image;
use crate::metal::{Fence, Texture};
use crate::objc::on_main;

/// The longest a present waits for a host to read its buffer.
const SAMPLE_MS: u64 = 250;

pub struct Host {
    pub package: String,
    writer: Mutex<UnixStream>,
    /// Buffers the host has.
    imported: Mutex<HashSet<u64>>,
    /// The last present it read, when, and whether it is gone.
    sampled: Mutex<(u32, i64, bool)>,
    cond: Condvar,
    /// Its windows' numbers, by task.
    numbers: Mutex<HashMap<i32, isize>>,
}

static HOSTS: Mutex<Vec<Arc<Host>>> = Mutex::new(Vec::new());
static SEQ: AtomicU32 = AtomicU32::new(0);

/// The host of `package`.
pub fn of(package: &str) -> Option<Arc<Host>> {
    HOSTS
        .lock()
        .unwrap()
        .iter()
        .find(|h| h.package == package)
        .cloned()
}

/// The task a host's window `number` shows.
pub fn task_of_window(number: isize) -> Option<i32> {
    HOSTS.lock().unwrap().iter().find_map(|h| {
        h.numbers
            .lock()
            .unwrap()
            .iter()
            .find(|&(_, &n)| n == number)
            .map(|(&t, _)| t)
    })
}

impl Host {
    fn send(&self, r: &Rec, fd: Option<i32>) -> bool {
        let w = self.writer.lock().unwrap();
        wire::send(std::os::fd::AsFd::as_fd(&*w), wire::bytes(r), fd).is_ok()
    }

    /// Pass a task record on.
    pub fn send_window(&self, r: &Record) {
        self.send(
            &Rec {
                op: host::WINDOW,
                window: *r,
                ..Default::default()
            },
            None,
        );
    }
}

/// Waits for the hosts' reading of one present.
pub struct Waiter {
    hosts: Vec<Arc<Host>>,
    seq: u32,
    fence: Option<Fence>,
}

/// Ask every host to show `t`; the waiter collects their answers.
pub fn present(t: &Texture, fence: Option<&Fence>) -> Waiter {
    let hosts = HOSTS.lock().unwrap().clone();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let id = t.import.id;
    let mut asked = Vec::new();
    for h in hosts {
        if h.imported.lock().unwrap().insert(id) {
            let r = Rec {
                op: host::IMPORT,
                id,
                import: t.import,
                ..Default::default()
            };
            if !h.send(&r, Some(t.fd().as_raw_fd())) {
                continue;
            }
        }
        let r = Rec {
            op: host::PRESENT,
            flag: seq,
            id,
            ..Default::default()
        };
        if h.send(&r, None) {
            if let Some(f) = fence {
                f.expect();
            }
            asked.push(h);
        }
    }
    Waiter {
        hosts: asked,
        seq,
        fence: fence.cloned(),
    }
}

impl Waiter {
    /// Wait until each host read the buffer (or gave up), and count it
    /// shown for the fence.
    pub fn wait(self) {
        let deadline = Instant::now() + Duration::from_millis(SAMPLE_MS);
        for h in &self.hosts {
            let mut s = h.sampled.lock().unwrap();
            while s.0 != self.seq && !s.2 {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                s = h.cond.wait_timeout(s, left).unwrap().0;
            }
            let at = if s.0 == self.seq { s.1 } else { 0 };
            drop(s);
            if let Some(f) = &self.fence {
                f.shown(at);
            }
        }
    }
}

/// The pointer's image (None: the arrow), for every host's views.
pub fn cursor(image: Option<&Image>) {
    let (width, height, hot, pixels) = image.map_or((0, 0, (0, 0), &[][..]), |i| {
        (i.width, i.height, i.hot, &i.pixels[..])
    });
    let r = Rec {
        op: host::CURSOR,
        id: pixels.len() as u64,
        import: Import {
            width,
            height,
            stride_bytes: width * 4,
            ..Default::default()
        },
        input: HostInput {
            x: hot.0 as f64,
            y: hot.1 as f64,
            ..Default::default()
        },
        ..Default::default()
    };
    for h in HOSTS.lock().unwrap().iter() {
        let mut w = h.writer.lock().unwrap();
        let _ = wire::send(std::os::fd::AsFd::as_fd(&*w), wire::bytes(&r), None)
            .and_then(|()| w.write_all(pixels));
    }
}

/// The composer released buffer `id`: the hosts that have it forget it.
pub fn release(id: u64) {
    for h in HOSTS.lock().unwrap().iter() {
        if h.imported.lock().unwrap().remove(&id) {
            h.send(
                &Rec {
                    op: host::RELEASE,
                    id,
                    ..Default::default()
                },
                None,
            );
        }
    }
}

fn read(sock: &mut UnixStream) -> Option<Rec> {
    let mut r = Rec::default();
    // SAFETY: `Rec` is plain old data; every byte pattern is a value.
    let buf = unsafe {
        std::slice::from_raw_parts_mut((&mut r as *mut Rec).cast::<u8>(), size_of::<Rec>())
    };
    sock.read_exact(buf).ok().map(|()| r)
}

/// Serve a window host's connection ([`wire::OP_HOST`]) until it closes.
pub fn serve(sock: OwnedFd) {
    let mut sock = UnixStream::from(sock);
    let Some(hello) = read(&mut sock) else { return };
    let Ok(writer) = sock.try_clone() else { return };
    let package = hello.window.text().to_string();
    if hello.op != host::HELLO || hello.id != wire::VERSION || package.is_empty() {
        return;
    }
    let h = Arc::new(Host {
        package: package.clone(),
        writer: Mutex::new(writer),
        imported: Mutex::new(HashSet::new()),
        sampled: Mutex::new((0, 0, false)),
        cond: Condvar::new(),
        numbers: Mutex::new(HashMap::new()),
    });
    {
        let mut hosts = HOSTS.lock().unwrap();
        // One host per package: a second shim of it is turned away.
        if hosts.iter().any(|o| o.package == package) {
            return;
        }
        hosts.push(h.clone());
    }
    eprintln!("aim-display: window host for {package}");
    let adopted = h.clone();
    on_main(move || crate::windows::adopt(&adopted.package, &adopted));
    while let Some(r) = read(&mut sock) {
        match r.op {
            host::SAMPLED => {
                *h.sampled.lock().unwrap() = (r.flag, r.id as i64, false);
                h.cond.notify_all();
            }
            host::WINDOW => request(r.window),
            host::INPUT => apply(r.input),
            host::NUMBER => {
                h.numbers
                    .lock()
                    .unwrap()
                    .insert(r.flag as i32, r.id as isize);
            }
            host::RESTACK => on_main(crate::windows::restack),
            _ => break,
        }
    }
    HOSTS.lock().unwrap().retain(|o| !Arc::ptr_eq(o, &h));
    h.sampled.lock().unwrap().2 = true;
    h.cond.notify_all();
    eprintln!("aim-display: window host for {package} left");
    on_main(move || crate::windows::disown(&package));
}

/// A host's request, for the task bridge.
fn request(r: Record) {
    match r.op {
        window::SET_BOUNDS | window::LAUNCH => crate::windows::send(r),
        window::FOCUS => on_main(move || crate::windows::focus(r.task)),
        window::CLOSE => {
            crate::windows::send(r);
            on_main(move || crate::windows::closing(r.task));
        }
        op => eprintln!("aim-display: window host sent {op}"),
    }
}

/// An input event, into the server's devices: a window host's, or one of
/// the server's own windows'. Touches wait for their task
/// (`windows::touch`), on the main thread.
pub fn apply(i: HostInput) {
    let Some(input) = crate::input::input() else {
        return;
    };
    let t = i.time_ns;
    let down = i.down != 0;
    let gesture = |g: u32| match g {
        0 => Gesture::Began,
        1 => Gesture::Changed,
        _ => Gesture::Ended,
    };
    match i.kind {
        input::TOUCH => {
            let phase = match i.code {
                0 => Phase::Down,
                1 => Phase::Drag,
                _ => Phase::Up,
            };
            on_main(move || crate::windows::touch(i.task, phase, i.x, i.y, i.area, t));
        }
        input::KEY => input.key(i.code as u16, down, i.down == 2, i.flags, t),
        input::HOVER => input.hover(i.x, i.y, t),
        input::FLAGS => input.flags_changed(i.code as u16, i.flags, t),
        input::SCROLL => input.scroll(
            Scroll {
                x: i.x,
                y: i.y,
                dx: i.dx,
                dy: i.dy,
                precise: i.code & 1 != 0,
                momentum: i.code & 2 != 0,
                swipe: (i.down > 0).then(|| (gesture(i.down - 1), i.sx, i.sy)),
            },
            t,
        ),
        input::LEAVE => input.leave(t),
        input::BUTTON => {
            let buttons = [Button::Right, Button::Middle, Button::Back, Button::Forward];
            if let Some(&b) = buttons.get(i.code as usize) {
                input.button(b, down, i.x, i.y, t);
            }
        }
        input::RELEASE_ALL => input.release_all(t),
        input::TWIST => {
            let twist = if i.code == 0 {
                Twist::Magnify(i.dx)
            } else {
                Twist::Rotate(i.dx)
            };
            input.twist(twist, gesture(i.down), i.x, i.y, i.area, t);
        }
        _ => {}
    }
}
