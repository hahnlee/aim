//! `darwin-display --socket PATH [--size WxH] [--title TEXT] [--capture FILE]`
//!
//! The display server (`docs/composer.md`): one macOS window showing the
//! guest's display. It owns what must live on the host's main thread
//! (AppKit) and outlive a composer restart: the window, its Metal layer and
//! the display link. The composer HAL reaches it through the host-call
//! module `display` of its `linux-run --display PATH`; the protocol is
//! `darwin_host_display::wire`.
//!
//! The window's input is the guest's evdev devices, listening sockets in
//! `PATH.input` (`docs/input.md`), removed when the server quits (window
//! closed, SIGTERM, SIGINT).
//!
//! On start it prints one line with the window number (for
//! `screencapture -l`), the display mode and the refresh period. Every
//! 5 seconds it prints vsync and present statistics to stderr. SIGUSR1
//! writes the last presented buffer to the `--capture` file (BMP).

#[macro_use]
mod objc;
mod input;
mod metal;
mod stats;
mod vsync;
mod window;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use darwin_host_display::wire::{self, Request};
use darwin_hostcall::display::{Connect, Event, event};

use metal::{Renderer, Texture};
use stats::Stats;

const USAGE: &str =
    "usage: darwin-display --socket PATH [--size WxH] [--title TEXT] [--capture FILE]";

struct Client {
    /// Events go out on the connection the guest reads.
    sock: OwnedFd,
    vsync: AtomicBool,
}

struct Display {
    renderer: Renderer,
    info: Connect,
    clients: Mutex<Vec<Arc<Client>>>,
    /// Serializes presents from all clients.
    last: Mutex<Option<Arc<Texture>>>,
    stats: Stats,
    capture: Option<PathBuf>,
}

static CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigusr1(_sig: i32) {
    CAPTURE_REQUESTED.store(true, Ordering::Relaxed);
}

impl Display {
    fn on_vsync(&self, tick: vsync::Tick) {
        self.stats.vsync(tick);
        let e = Event {
            kind: event::VSYNC,
            timestamp_ns: tick.timestamp_ns,
            period_ns: tick.period_ns,
            sent_ns: vsync::monotonic_ns(),
            ..Default::default()
        };
        for c in self.clients.lock().unwrap().iter() {
            if c.vsync.load(Ordering::Relaxed) {
                // A reader that falls behind loses vsyncs rather than
                // stalling the link.
                // SAFETY: a send on our socket from a local record.
                unsafe {
                    libc::send(
                        c.sock.as_raw_fd(),
                        (&e as *const Event).cast(),
                        size_of::<Event>(),
                        libc::MSG_DONTWAIT,
                    )
                };
            }
        }
        if CAPTURE_REQUESTED.swap(false, Ordering::Relaxed) {
            self.capture();
        }
    }

    fn capture(&self) {
        let (Some(path), Some(t)) = (&self.capture, self.last.lock().unwrap().clone()) else {
            eprintln!("darwin-display: nothing to capture");
            return;
        };
        match write_bmp(path, &t) {
            Ok(()) => eprintln!("darwin-display: captured {}", path.display()),
            Err(e) => eprintln!("darwin-display: capture {}: {e}", path.display()),
        }
    }

    fn present(&self, t: &Arc<Texture>) {
        let mut last = self.last.lock().unwrap();
        if let Some(frame) = self.renderer.present(Some(t)) {
            self.stats.present(frame);
        }
        *last = Some(t.clone());
    }

    /// Serve one client until it disconnects.
    fn serve(&self, sock: OwnedFd) {
        let Ok(events) = sock.try_clone() else {
            return;
        };
        let client = Arc::new(Client {
            sock: events,
            vsync: AtomicBool::new(false),
        });
        let mut textures: HashMap<u64, Arc<Texture>> = HashMap::new();
        loop {
            let mut r = Request::default();
            let mut fd = None;
            // SAFETY: `Request` is plain old data.
            let buf = unsafe {
                std::slice::from_raw_parts_mut(
                    (&mut r as *mut Request).cast(),
                    size_of::<Request>(),
                )
            };
            match wire::recv(sock.as_fd(), buf, &mut fd) {
                Ok(true) => {}
                _ => break,
            }
            match r.op {
                wire::OP_HELLO if r.id == wire::VERSION && r.flag == 0 => {
                    if wire::send(sock.as_fd(), wire::bytes(&self.info), None).is_err() {
                        break;
                    }
                    self.clients.lock().unwrap().push(client.clone());
                }
                wire::OP_IMPORT => {
                    let Some(fd) = fd else { break };
                    match self.renderer.import(fd, &r.import) {
                        Ok(t) => {
                            textures.insert(r.id, Arc::new(t));
                        }
                        Err(e) => eprintln!("darwin-display: import {:#x}: {e}", r.id),
                    }
                }
                wire::OP_PRESENT => match textures.get(&r.id) {
                    Some(t) => self.present(t),
                    None => eprintln!("darwin-display: present of unknown buffer {:#x}", r.id),
                },
                wire::OP_RELEASE => {
                    textures.remove(&r.id);
                }
                wire::OP_SET_VSYNC => client.vsync.store(r.flag != 0, Ordering::Relaxed),
                _ => break,
            }
        }
        self.clients
            .lock()
            .unwrap()
            .retain(|c| !Arc::ptr_eq(c, &client));
    }
}

/// A 32-bit BMP of an RGBA or RGBX buffer, the top row first.
fn write_bmp(path: &std::path::Path, t: &Texture) -> std::io::Result<()> {
    let i = &t.import;
    if !matches!(i.format, 0x1 | 0x2) {
        return Err(std::io::Error::other(format!("format {:#x}", i.format)));
    }
    let (w, h) = (i.width as usize, i.height as usize);
    let mut out = Vec::with_capacity(54 + w * h * 4);
    let size = (54 + w * h * 4) as u32;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    // Negative height: rows top to bottom.
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    let pixels = t.pixels();
    for y in 0..h {
        let row = &pixels[y * i.stride_bytes as usize..][..w * 4];
        for p in row.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    std::fs::write(path, out)
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut socket, mut size, mut capture) = (None, None, None);
    let mut title = "Android".to_string();
    while let Some(a) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(value())),
            "--size" => {
                size = Some(parse_size(&value()).unwrap_or_else(|| {
                    eprintln!("{USAGE}");
                    std::process::exit(2)
                }))
            }
            "--title" => title = value(),
            "--capture" => capture = Some(PathBuf::from(value())),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2)
            }
        }
    }
    let Some(socket) = socket else {
        eprintln!("{USAGE}");
        std::process::exit(2)
    };
    // SAFETY: plain signal dispositions; the handler only stores an atomic.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGUSR1, on_sigusr1 as usize);
    }

    let _pool = objc::Pool::new();
    let device = metal::device();
    if device.is_null() {
        eprintln!("darwin-display: no Metal device");
        std::process::exit(1);
    }
    let win = window::create(size, &title, device);
    let renderer = Renderer::new(device, win.layer).unwrap_or_else(|e| {
        eprintln!("darwin-display: shader: {e}");
        std::process::exit(1)
    });
    // The display is black until the first frame.
    renderer.present(None);

    static DISPLAY: OnceLock<Display> = OnceLock::new();
    let period = vsync::start(Box::new(|tick| {
        if let Some(d) = DISPLAY.get() {
            d.on_vsync(tick)
        }
    }))
    .unwrap_or_else(|e| {
        eprintln!("darwin-display: {e}");
        std::process::exit(1)
    });
    let display = DISPLAY.get_or_init(|| Display {
        renderer,
        info: Connect {
            display: 0,
            width: win.width,
            height: win.height,
            dpi_x_milli: (win.dpi_x * 1000.0) as u32,
            dpi_y_milli: (win.dpi_y * 1000.0) as u32,
            _reserved: 0,
            vsync_period_ns: period as u64,
        },
        clients: Mutex::new(Vec::new()),
        last: Mutex::new(None),
        stats: Stats::new(),
        capture,
    });

    if let Err(e) = input::start(&socket, &win) {
        eprintln!(
            "darwin-display: input devices in {}: {e}",
            darwin_host_display::input::device_dir(&socket).display()
        );
        std::process::exit(1);
    }
    input::quit_on_signals();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap_or_else(|e| {
        eprintln!("darwin-display: {}: {e}", socket.display());
        std::process::exit(1)
    });
    println!(
        "darwin-display: window {} {}x{} pixels, {:.0}x{:.0} dpi, vsync {} ns, socket {}",
        win.number,
        win.width,
        win.height,
        win.dpi_x,
        win.dpi_y,
        period,
        socket.display()
    );
    let _ = std::io::stdout().flush();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || display.serve(conn.into()));
        }
    });
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if let Some(line) = DISPLAY.get().and_then(|d| d.stats.take()) {
                eprintln!("darwin-display: {line}");
            }
        }
    });
    window::run()
}
