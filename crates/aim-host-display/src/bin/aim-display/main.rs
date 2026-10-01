//! `aim-display --socket PATH [--mode device|windows] [--size WxH] [--title TEXT] [--capture FILE] [--apps DIR]`
//!
//! The display server (`docs/composer.md`): macOS windows showing the
//! guest's display. It owns what must live on the host's main thread
//! (AppKit) and outlive a composer restart: the windows, their Metal
//! layers and the display link. The composer HAL reaches it through the
//! host-call module `display` of its `linux-run --display PATH`; the
//! protocol is `aim_host_display::wire`.
//!
//! - **Device mode** (the default): one window showing the whole display.
//! - **Window mode** (`--mode windows`, `docs/windows.md`): one window per
//!   Android task, which the guest's task bridge reports; the display is
//!   the Mac's main screen. Each window shows its task's layers
//!   (`docs/layers.md`), and system panels the layers of no task. The app
//!   shims in `--apps DIR` show the guest's notifications
//!   (`docs/notifications.md`) and name the server's own windows.
//!
//! The windows' input is the guest's evdev devices, listening sockets in
//! `PATH.input` (`docs/input.md`), removed when the server quits (device
//! window closed, SIGTERM, SIGINT).
//!
//! On start it prints one line with the window number (for
//! `screencapture -l`), the display mode and the refresh period. Every
//! 5 seconds in which vsync ran or frames were presented it prints their
//! statistics to stderr. SIGUSR1
//! writes the last presented buffer to the `--capture` file (BMP).

#[macro_use]
mod objc;
mod apps;
mod consent;
mod cursor;
mod hosts;
mod input;
mod media;
mod metal;
mod notifications;
mod nowplaying;
mod panels;
mod sheets;
mod shell;
mod shim;
mod stats;
mod status;
mod system_icons;
mod toast;
mod un;
mod vsync;
mod window;
mod windows;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::layers::{self, Attribution, Task};
use aim_host_display::wire::{self, Request};
use aim_hostcall::display::{Connect, Event, Windows, event, mode};
use aim_sync_file::Writer;

use metal::{Fence, LayerFrame, Renderer, Target, Texture};
use stats::Stats;

/// How long a present waits for its buffer's acquire fence.
const ACQUIRE_TIMEOUT_MS: i32 = 3000;

const USAGE: &str = "usage: aim-display --socket PATH [--mode device|windows] [--size WxH] [--title TEXT] [--capture FILE] [--apps DIR]";

struct Client {
    /// Events go out on the connection the guest reads.
    sock: OwnedFd,
    vsync: AtomicBool,
}

/// What the last present showed.
#[derive(Clone)]
enum Shown {
    /// A buffer: the client target of client composition.
    Buffer(Arc<Texture>),
    /// A frame of layers (window mode).
    Layers(Arc<LayerFrame>),
}

struct Display {
    renderer: Renderer,
    info: Connect,
    /// One of `aim_hostcall::display::mode`.
    mode: u32,
    clients: Mutex<Vec<Arc<Client>>>,
    /// Serializes presents from all clients.
    last: Mutex<Option<Shown>>,
    /// The layers presents go to: the device window's, or the task
    /// windows' that are visible.
    targets: Mutex<Vec<Target>>,
    /// The system panels' layers (in the server).
    panels: Mutex<Vec<Target>>,
    /// The tasks with windows and the front one, which the layers of a
    /// frame are given to (in the server).
    tasks: Mutex<(Vec<Task>, Option<i32>)>,
    owners: Mutex<Attribution>,
    /// A later attribution of the last frame is due.
    again: AtomicBool,
    stats: Stats,
    capture: Option<PathBuf>,
}

static DISPLAY: OnceLock<Display> = OnceLock::new();

impl Display {
    fn new(renderer: Renderer, info: Connect, mode: u32, capture: Option<PathBuf>) -> Display {
        Display {
            renderer,
            info,
            mode,
            clients: Mutex::new(Vec::new()),
            last: Mutex::new(None),
            targets: Mutex::new(Vec::new()),
            panels: Mutex::new(Vec::new()),
            tasks: Mutex::new((Vec::new(), None)),
            owners: Mutex::new(Attribution::default()),
            again: AtomicBool::new(false),
            stats: Stats::new(),
            capture,
        }
    }
}

/// The write end of the pipe SIGUSR1 wakes the capture thread through.
static CAPTURE_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigusr1(_sig: i32) {
    // SAFETY: write(2) is async-signal-safe.
    unsafe {
        libc::write(
            CAPTURE_PIPE.load(Ordering::Relaxed),
            [0u8].as_ptr().cast(),
            1,
        )
    };
}

/// Write a capture for each SIGUSR1.
fn capture_on_signal() {
    let mut fds = [0; 2];
    // SAFETY: fills `fds` with a new pipe.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        eprintln!("aim-display: pipe: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    CAPTURE_PIPE.store(fds[1], Ordering::Relaxed);
    std::thread::spawn(move || {
        let mut b = [0u8; 16];
        // SAFETY: reads into a local buffer from our pipe.
        while unsafe { libc::read(fds[0], b.as_mut_ptr().cast(), b.len()) } > 0 {
            if let Some(d) = DISPLAY.get() {
                d.capture();
            }
        }
    });
    // SAFETY: the handler only writes to the pipe.
    unsafe { libc::signal(libc::SIGUSR1, on_sigusr1 as usize) };
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
    }

    /// Run the display link while a client wants vsync.
    fn update_vsync(&self) {
        vsync::update(|| {
            self.clients
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.vsync.load(Ordering::Relaxed))
        });
    }

    fn capture(&self) {
        let last = self.last.lock().unwrap().clone();
        let (Some(path), Some(Shown::Buffer(t))) = (&self.capture, last) else {
            eprintln!("aim-display: nothing to capture");
            return;
        };
        match write_bmp(path, &t) {
            Ok(()) => eprintln!("aim-display: captured {}", path.display()),
            Err(e) => eprintln!("aim-display: capture {}: {e}", path.display()),
        }
    }

    /// Show `t` once its content is ready (`acquire`), in this process's
    /// windows and the window hosts'; `fence` signals when it is on screen.
    fn present(&self, t: &Arc<Texture>, acquire: Option<OwnedFd>, fence: Option<Writer>) {
        if let Some(a) = acquire
            && !aim_sync_file::wait(a.as_fd(), ACQUIRE_TIMEOUT_MS)
        {
            eprintln!(
                "aim-display: acquire fence of a present not signaled in {ACQUIRE_TIMEOUT_MS} ms"
            );
        }
        let fence = fence.map(Fence::new);
        let mut last = self.last.lock().unwrap();
        let targets = self.targets.lock().unwrap().clone();
        let hosts = hosts::present(t, fence.as_ref());
        if let Some(frame) = self.renderer.present(Some(t), &targets, fence.as_ref()) {
            self.stats.present(frame);
        }
        hosts.wait();
        *last = Some(Shown::Buffer(t.clone()));
        if let Some(f) = fence {
            f.done();
        }
    }

    /// Show a frame of layers once its content is ready (`acquire`), each
    /// window its task's layers, in this process's windows and panels and
    /// the window hosts'; `fence` signals when it is on screen.
    fn present_layers(&self, mut f: LayerFrame, acquire: Option<OwnedFd>, fence: Option<Writer>) {
        if let Some(a) = acquire
            && !aim_sync_file::wait(a.as_fd(), ACQUIRE_TIMEOUT_MS)
        {
            eprintln!(
                "aim-display: acquire fence of a present not signaled in {ACQUIRE_TIMEOUT_MS} ms"
            );
        }
        let fence = fence.map(Fence::new);
        let mut last = self.last.lock().unwrap();
        let again = self.attribute(&mut f);
        let f = Arc::new(f);
        self.show_layers(&f, fence.as_ref());
        *last = Some(Shown::Layers(f.clone()));
        drop(last);
        if let Some(fence) = fence {
            fence.done();
        }
        panels::update(&f);
        self.attribute_again(again);
    }

    /// Show a frame of layers the server sent (in a window host).
    fn show_host_layers(&self, f: LayerFrame) {
        let mut last = self.last.lock().unwrap();
        let f = Arc::new(f);
        self.show_layers(&f, None);
        *last = Some(Shown::Layers(f));
    }

    /// Draw `f` into this process's targets and the window hosts'.
    fn show_layers(&self, f: &Arc<LayerFrame>, fence: Option<&Fence>) {
        let mut targets = self.targets.lock().unwrap().clone();
        targets.extend(self.panels.lock().unwrap().iter().cloned());
        let hosts = hosts::present_layers(f, fence);
        if let Some(frame) = self.renderer.compose(f, &targets, fence) {
            self.stats.present(frame);
        }
        hosts.wait();
    }

    /// Give `f`'s layers their owners; when a layer waits for a task, the
    /// time (ms) at which to do it again.
    fn attribute(&self, f: &mut LayerFrame) -> Option<i64> {
        let (tasks, front) = self.tasks.lock().unwrap().clone();
        let display = [0, 0, self.info.width as i32, self.info.height as i32];
        let now = vsync::monotonic_ns() / 1_000_000;
        self.owners
            .lock()
            .unwrap()
            .attribute(&mut f.layers, &tasks, display, front, now)
    }

    /// At `at` (ms), give the last frame's layers their owners again,
    /// unless that is due already (a layer waiting since earlier).
    fn attribute_again(&self, at: Option<i64>) {
        if let Some(at) = at
            && !self.again.swap(true, Ordering::Relaxed)
        {
            let wait = (at - vsync::monotonic_ns() / 1_000_000).max(0) as u64;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                if let Some(d) = DISPLAY.get() {
                    d.again.store(false, Ordering::Relaxed);
                    d.reattribute();
                }
            });
        }
    }

    /// Give the last frame's layers their owners again (the tasks changed,
    /// or a layer stopped waiting for one), and show it again if that
    /// changed any.
    fn reattribute(&self) {
        let mut last = self.last.lock().unwrap();
        let Some(Shown::Layers(old)) = last.clone() else {
            return;
        };
        let mut f = LayerFrame::clone(&old);
        let again = self.attribute(&mut f);
        if f.layers != old.layers {
            let f = Arc::new(f);
            self.show_layers(&f, None);
            *last = Some(Shown::Layers(f.clone()));
            drop(last);
            panels::update(&f);
        }
        self.attribute_again(again);
    }

    /// The tasks with windows, and the front one (the server's task
    /// records).
    fn set_tasks(&'static self, mut tasks: Vec<Task>, front: Option<i32>) {
        tasks.sort_by_key(|t| t.id);
        let mut t = self.tasks.lock().unwrap();
        if *t != (tasks.clone(), front) {
            *t = (tasks, front);
            std::thread::spawn(|| self.reattribute());
        }
    }

    /// Show the last frame again in the layers of `targets` (a window that
    /// appeared or changed), without a fence.
    fn refresh(&self, targets: &[Target]) {
        match self.last.lock().unwrap().as_ref() {
            Some(Shown::Buffer(t)) => {
                self.renderer.present(Some(t), targets, None);
            }
            Some(Shown::Layers(f)) => {
                self.renderer.compose(f, targets, None);
            }
            None => {}
        }
    }

    fn set_targets(&self, targets: Vec<Target>) {
        *self.targets.lock().unwrap() = targets;
    }

    /// The system panels' layers.
    fn set_panels(&self, targets: Vec<Target>) {
        *self.panels.lock().unwrap() = targets;
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
            let mut fds = Vec::new();
            // SAFETY: `Request` is plain old data.
            let buf = unsafe {
                std::slice::from_raw_parts_mut(
                    (&mut r as *mut Request).cast(),
                    size_of::<Request>(),
                )
            };
            match wire::recv(sock.as_fd(), buf, &mut fds) {
                Ok(true) => {}
                _ => break,
            }
            match r.op {
                wire::OP_HELLO if r.id == wire::VERSION && r.flag == 0 => {
                    if wire::send(sock.as_fd(), wire::bytes(&self.info), None).is_err() {
                        break;
                    }
                    self.clients.lock().unwrap().push(client.clone());
                    // A new composer names its layers afresh.
                    self.owners.lock().unwrap().reset();
                }
                wire::OP_IMPORT => {
                    let Some(fd) = fds.pop() else { break };
                    match self.renderer.import(fd, &r.import) {
                        Ok(t) => {
                            textures.insert(r.id, Arc::new(t));
                        }
                        Err(e) => eprintln!("aim-display: import {:#x}: {e}", r.id),
                    }
                }
                wire::OP_PRESENT => {
                    let mut fds = fds.into_iter();
                    let fence = fds.next().map(Writer::from);
                    let acquire = (r.flag & wire::PRESENT_ACQUIRE != 0)
                        .then(|| fds.next())
                        .flatten();
                    match textures.get(&r.id) {
                        Some(t) => self.present(t, acquire, fence),
                        None => eprintln!("aim-display: present of unknown buffer {:#x}", r.id),
                    }
                }
                wire::OP_LAYERS => {
                    let Ok((layers, rects)) = layers::read(sock.as_fd()) else {
                        break;
                    };
                    let mut fds = fds.into_iter();
                    let fence = fds.next().map(Writer::from);
                    let acquire = (r.flag & wire::PRESENT_ACQUIRE != 0)
                        .then(|| fds.next())
                        .flatten();
                    let f = LayerFrame::new(layers, rects, r.id, &textures);
                    self.present_layers(f, acquire, fence);
                }
                wire::OP_RELEASE => {
                    textures.remove(&r.id);
                    hosts::release(r.id);
                }
                wire::OP_CURSOR => {
                    let acquire = (r.flag & wire::CURSOR_ACQUIRE != 0)
                        .then(|| fds.pop())
                        .flatten();
                    let changed = r.flag & wire::CURSOR_CHANGED != 0;
                    cursor::serve(r.id, textures.get(&r.id), changed, acquire, r.x, r.y);
                }
                wire::OP_SET_VSYNC => {
                    client.vsync.store(r.flag != 0, Ordering::Relaxed);
                    self.update_vsync();
                }
                wire::OP_WINDOWS if r.id == wire::VERSION => {
                    let answer = Windows {
                        mode: self.mode,
                        ..Default::default()
                    };
                    if wire::send(sock.as_fd(), wire::bytes(&answer), None).is_ok()
                        && self.mode == mode::WINDOWS
                    {
                        windows::serve_bridge(sock);
                    }
                    return;
                }
                wire::OP_HOST if r.id == wire::VERSION && self.mode == mode::WINDOWS => {
                    return hosts::serve(sock);
                }
                wire::OP_NOTIFICATIONS if r.id == wire::VERSION => {
                    // Only window mode's shims show them; in device mode
                    // SystemUI does.
                    if wire::send(sock.as_fd(), wire::bytes(&self.mode), None).is_ok()
                        && self.mode == mode::WINDOWS
                    {
                        notifications::serve_bridge(sock.into());
                    }
                    return;
                }
                wire::OP_MEDIA if r.id == wire::VERSION => {
                    // Window mode's shims show media; in device mode, this
                    // process.
                    if wire::send(sock.as_fd(), wire::bytes(&self.mode), None).is_ok() {
                        media::serve_bridge(sock.into(), self.mode == mode::WINDOWS);
                    }
                    return;
                }
                wire::OP_SHELL if r.id == wire::VERSION => {
                    // Window mode's shims show it; in device mode, this
                    // process.
                    if wire::send(sock.as_fd(), wire::bytes(&self.mode), None).is_ok() {
                        shell::serve_bar(sock.into(), self.mode == mode::WINDOWS);
                    }
                    return;
                }
                _ => break,
            }
        }
        self.clients
            .lock()
            .unwrap()
            .retain(|c| !Arc::ptr_eq(c, &client));
        self.update_vsync();
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
    // An app's shim bundle runs this binary as that app's window host.
    if let Some(package) = shim::info("AIMPackage") {
        let activity = shim::info("AIMActivity").unwrap_or_default();
        let socket = shim::info("AIMDisplaySocket").unwrap_or_default();
        shim::run(package, activity, std::path::Path::new(&socket));
    }
    let mut args = std::env::args().skip(1);
    let (mut socket, mut size, mut capture) = (None, None, None);
    let mut title = "Android".to_string();
    let mut display_mode = mode::DEVICE;
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
            "--mode" => {
                display_mode = match value().as_str() {
                    "device" => mode::DEVICE,
                    "windows" => mode::WINDOWS,
                    _ => {
                        eprintln!("{USAGE}");
                        std::process::exit(2)
                    }
                }
            }
            "--capture" => capture = Some(PathBuf::from(value())),
            "--apps" => apps::set_dir(std::path::Path::new(&value())),
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
    if display_mode == mode::WINDOWS && size.is_some() {
        eprintln!(
            "aim-display: window mode's display is the main screen; --size is for device mode"
        );
        std::process::exit(2)
    }
    // SAFETY: a plain signal disposition.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    capture_on_signal();

    let _pool = objc::Pool::new();
    let device = metal::device();
    if device.is_null() {
        eprintln!("aim-display: no Metal device");
        std::process::exit(1);
    }
    window::app(display_mode, true);
    let win = if display_mode == mode::WINDOWS {
        windows::start(device)
    } else {
        window::create(size, &title, device)
    };
    let renderer = Renderer::new(device).unwrap_or_else(|e| {
        eprintln!("aim-display: shader: {e}");
        std::process::exit(1)
    });
    let targets: Vec<Target> = win
        .layer
        .map(|l| Target::new(l, None))
        .into_iter()
        .collect();
    // The display is black until the first frame.
    renderer.present(None, &targets, None);

    let period = vsync::create(Box::new(|tick| {
        if let Some(d) = DISPLAY.get() {
            d.on_vsync(tick)
        }
    }))
    .unwrap_or_else(|e| {
        eprintln!("aim-display: {e}");
        std::process::exit(1)
    });
    let info = Connect {
        display: 0,
        width: win.width,
        height: win.height,
        dpi_x_milli: (win.dpi_x * 1000.0) as u32,
        dpi_y_milli: (win.dpi_y * 1000.0) as u32,
        mode: display_mode,
        vsync_period_ns: period as u64,
    };
    let display = DISPLAY.get_or_init(|| Display::new(renderer, info, display_mode, capture));
    display.set_targets(targets);

    if let Err(e) = input::start(&socket, &win) {
        eprintln!(
            "aim-display: input devices in {}: {e}",
            aim_host_display::input::device_dir(&socket).display()
        );
        std::process::exit(1);
    }
    input::quit_on_signals();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap_or_else(|e| {
        eprintln!("aim-display: {}: {e}", socket.display());
        std::process::exit(1)
    });
    println!(
        "aim-display: {} {}x{} pixels, {:.0}x{:.0} dpi, vsync {} ns, socket {}",
        win.number
            .map_or("window mode".to_string(), |n| format!("window {n}")),
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
                eprintln!("aim-display: {line}");
            }
        }
    });
    window::run()
}
