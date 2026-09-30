//! A window host: aim-display run from an app's shim bundle
//! (`docs/windows.md`, "App shims"). Its `Info.plist` names the Android
//! package (`AIMPackage`), its launcher activity (`AIMActivity`) and the
//! display server (`AIMDisplaySocket`).
//!
//! It shows that package's task windows under the bundle's name and Dock
//! icon. The server sends it the package's task records, the buffers (as
//! memfds, mapped here as in the server) and every present, which it draws
//! into its windows and answers once its GPU pass has read the buffer, and
//! the pointer's image for its windows' cursor. Its
//! windows' requests and input go back to the server.
//!
//! Launching the shim, or clicking it in the Dock, starts the app (its
//! launcher activity: Android brings a running task to the front). Quitting
//! it closes the app's tasks. The server launches it with
//! `--notifications` to show the app's notifications (`un.rs`) while the
//! app has no window: it then starts nothing, and has no Dock icon until a
//! window opens. The system shim (package `android`) has no activity.

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::input::translate::Phase;
use aim_host_display::wire::{self, Host as Rec, HostInput, Request, host, input};
use aim_hostcall::display::{Connect, Window as Record, mode, window};

use crate::cursor::Image;
use crate::metal::{Renderer, Texture};
use crate::objc::{class, nsstring, on_main, text};

struct Link {
    /// The launcher activity, `package/class`; none for the system shim.
    activity: Option<String>,
    writer: Mutex<UnixStream>,
}

/// Started to show notifications (`--notifications`): no Dock icon until
/// the app shows a window.
static BACKGROUND: AtomicBool = AtomicBool::new(false);

static LINK: OnceLock<Link> = OnceLock::new();

/// Whether this process is a window host.
pub fn is_host() -> bool {
    LINK.get().is_some()
}

fn send(r: &Rec) {
    if let Some(l) = LINK.get() {
        let w = l.writer.lock().unwrap();
        let _ = wire::send(w.as_fd(), wire::bytes(r), None);
    }
}

/// A request for the task bridge, through the server.
pub fn send_window(r: Record) {
    send(&Rec {
        op: host::WINDOW,
        window: r,
        ..Default::default()
    });
}

/// Report task `task`'s window number, for the server's stacking.
pub fn window_number(task: i32, number: isize) {
    send(&Rec {
        op: host::NUMBER,
        flag: task as u32,
        id: number as u64,
        ..Default::default()
    });
}

/// A window was minimized: the server restacks the tasks.
pub fn restack() {
    send(&Rec {
        op: host::RESTACK,
        ..Default::default()
    });
}

/// An input event of this host's windows, for the server's devices.
pub fn input(i: HostInput) {
    send(&Rec {
        op: host::INPUT,
        input: i,
        ..Default::default()
    });
}

pub fn touch(task: i32, phase: Phase, x: f64, y: f64, area: [i32; 4], t: i64) {
    let code = match phase {
        Phase::Down => 0,
        Phase::Drag => 1,
        Phase::Up => 2,
    };
    input(HostInput {
        kind: input::TOUCH,
        task,
        code,
        x,
        y,
        area,
        time_ns: t,
        ..Default::default()
    });
}

/// A notification message for the server.
pub fn notify(m: &aim_host_display::notify::Message) {
    if let Some(l) = LINK.get() {
        let w = l.writer.lock().unwrap();
        let r = Rec {
            op: host::NOTIFY,
            ..Default::default()
        };
        if wire::send(w.as_fd(), wire::bytes(&r), None).is_ok() {
            let _ = wire::send(w.as_fd(), &m.frame(), None);
        }
    }
}

/// Start the app (or bring its task to the front).
pub fn launch() {
    if let Some(activity) = LINK.get().and_then(|l| l.activity.as_ref()) {
        send_window(Record::with_text(window::LAUNCH, 0, activity));
    }
}

/// The app quits: its tasks close with it.
pub fn close_all() {
    for (task, _) in crate::windows::window_numbers() {
        send_window(Record {
            op: window::CLOSE,
            task,
            ..Default::default()
        });
    }
}

/// `NSApplicationActivationPolicy`.
const REGULAR: isize = 0;
const ACCESSORY: isize = 1;

fn set_policy(policy: isize) {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"setActivationPolicy:" => bool, isize = policy);
}

/// `key` of the main bundle's `Info.plist`, if this process runs from a
/// bundle that has it.
pub fn info(key: &str) -> Option<String> {
    let bundle = send!(class(c"NSBundle"), c"mainBundle" => Id);
    let v = send!(bundle, c"objectForInfoDictionaryKey:" => Id, Id = nsstring(key));
    (!v.is_null()).then(|| text(v))
}

/// The app's name, in a window host.
pub fn app_name() -> Option<String> {
    is_host().then(|| info("CFBundleDisplayName")).flatten()
}

/// Tell the user the app cannot open, and quit.
fn fail(message: &str) -> ! {
    eprintln!("aim-display: {message}");
    let alert = send!(class(c"NSAlert"), c"new" => Id);
    send!(alert, c"setMessageText:" => (), Id = nsstring(message));
    send!(alert, c"runModal" => isize);
    std::process::exit(1)
}

fn read(sock: &mut UnixStream, fds: &mut Vec<std::os::fd::OwnedFd>) -> Option<Rec> {
    let mut r = Rec::default();
    // SAFETY: `Rec` is plain old data; every byte pattern is a value.
    let buf = unsafe {
        std::slice::from_raw_parts_mut((&mut r as *mut Rec).cast::<u8>(), size_of::<Rec>())
    };
    match wire::recv(sock.as_fd(), buf, fds) {
        Ok(true) => Some(r),
        _ => None,
    }
}

/// Run as the window host of `package`, whose launcher activity is
/// `activity`, served by the display server at `socket`. Never returns.
pub fn run(package: String, activity: String, socket: &Path) -> ! {
    let _pool = crate::objc::Pool::new();
    crate::window::app(mode::WINDOWS);
    if std::env::args().any(|a| a == "--notifications") {
        BACKGROUND.store(true, Ordering::Relaxed);
        set_policy(ACCESSORY);
    }
    crate::un::start();
    let device = crate::metal::device();
    if device.is_null() {
        fail("no Metal device");
    }
    crate::windows::start(device);
    let renderer = Renderer::new(device).unwrap_or_else(|e| fail(&format!("shader: {e}")));
    let Ok(mut sock) = UnixStream::connect(socket) else {
        fail(&format!(
            "{package} cannot open: Android (aim) is not running"
        ));
    };
    let hello = Request {
        op: wire::OP_HOST,
        id: wire::VERSION,
        ..Default::default()
    };
    let named = Rec {
        op: host::HELLO,
        id: wire::VERSION,
        window: Record::with_text(0, 0, &package),
        ..Default::default()
    };
    let (Ok(writer), Ok(()), Ok(())) = (
        sock.try_clone(),
        wire::send(sock.as_fd(), wire::bytes(&hello), None),
        wire::send(sock.as_fd(), wire::bytes(&named), None),
    ) else {
        fail("the display server did not answer");
    };
    let _ = LINK.set(Link {
        activity: (!activity.is_empty()).then(|| format!("{package}/{activity}")),
        writer: Mutex::new(writer),
    });
    let _ = crate::DISPLAY.set(crate::Display::new(
        renderer,
        Connect::default(),
        mode::WINDOWS,
        None,
    ));
    std::thread::spawn(move || serve(&mut sock));
    // SIGTERM and SIGINT quit the app normally, closing its tasks.
    crate::input::quit_on_signals();
    if !BACKGROUND.load(Ordering::Relaxed) {
        launch();
    }
    crate::window::run()
}

/// The server's records, until it goes away.
fn serve(sock: &mut UnixStream) {
    let mut textures: HashMap<u64, Arc<Texture>> = HashMap::new();
    let mut fds = Vec::new();
    while let Some(r) = read(sock, &mut fds) {
        match r.op {
            host::IMPORT => {
                let Some(fd) = fds.pop() else { break };
                let Some(d) = crate::DISPLAY.get() else { break };
                match d.renderer.import(fd, &r.import) {
                    Ok(t) => {
                        textures.insert(r.id, Arc::new(t));
                    }
                    Err(e) => eprintln!("aim-display: import {:#x}: {e}", r.id),
                }
            }
            host::RELEASE => {
                textures.remove(&r.id);
            }
            host::PRESENT => {
                if let (Some(t), Some(d)) = (textures.get(&r.id), crate::DISPLAY.get()) {
                    d.present(t, None, None);
                }
                send(&Rec {
                    op: host::SAMPLED,
                    flag: r.flag,
                    id: crate::vsync::monotonic_ns() as u64,
                    ..Default::default()
                });
            }
            host::WINDOW => {
                let w = r.window;
                if w.op == window::TASK && BACKGROUND.swap(false, Ordering::Relaxed) {
                    on_main(|| set_policy(REGULAR));
                }
                on_main(move || crate::windows::on_host_record(&w));
            }
            host::CURSOR => {
                let i = r.import;
                let mut pixels = vec![0; r.id as usize];
                if pixels.len() != i.width as usize * i.height as usize * 4
                    || !wire::recv(sock.as_fd(), &mut pixels, &mut fds).unwrap_or(false)
                {
                    break;
                }
                let image = (!pixels.is_empty()).then(|| {
                    Arc::new(Image {
                        width: i.width,
                        height: i.height,
                        pixels,
                        hot: (r.input.x as i32, r.input.y as i32),
                    })
                });
                on_main(move || crate::cursor::show(image));
            }
            host::NOTIFY => match aim_host_display::notify::Message::read(sock) {
                Ok(Some(m)) => crate::un::handle(m),
                _ => break,
            },
            _ => break,
        }
        fds.clear();
    }
    // The server is gone, and with it the app's windows.
    on_main(|| {
        let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
        send!(app, c"terminate:" => (), Id = std::ptr::null_mut());
    });
}
