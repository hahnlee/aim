//! Window mode (`docs/windows.md`): one macOS window per Android task.
//!
//! The display is the Mac's main screen ([`aim_host_display::windows`]),
//! and the guest's task bridge reports the freeform tasks on it. Each gets
//! an `NSWindow` exactly over its content, whose layer shows that part of
//! every presented frame, one pixel per pixel. Window and task follow each
//! other:
//!
//! - moving the window moves the task, resizing it resizes the task once
//!   the live resize ends, and the window takes the bounds the task got;
//! - the key window's task is the focused one, and a task Android brings to
//!   the front brings its window;
//! - a minimized window's task goes behind the visible windows' tasks, so
//!   the display's stacking stays the screen's, and a task Android moves to
//!   the back (Back on its root activity) minimizes its window;
//! - an app that goes home (a HOME intent) hides, as with Cmd+H;
//! - an app its shim launches shows a splash, its icon on its splash
//!   screen's background (else the window background), at once and until
//!   Android reports its first frame.
//! - closing the window removes the task, and a removed task's window
//!   closes;
//! - an orientation an activity asks for turns the window to landscape or
//!   portrait proportions around its centre, within the screen, and a
//!   request for none turns it back to the user's size.
//!
//! A press in a window whose task is not the top one focuses the task first
//! and holds the touch until Android reports the task in front (or
//! [`HOLD_MS`] pass), so it lands on that task and not on one above it.
//!
//! The window chrome is the Mac's, with no button of Android's: the first
//! window says once how to go back ([`back_hint`]), until dismissed.
//!
//! The same windows run in the display server and in a window host, an
//! app's shim (`shim.rs`): the server hands each task to the host of its
//! activity or package ([`crate::hosts::for_task`]) and shows the tasks no
//! host takes. A host
//! sends its requests and input through the server, which alone knows
//! which task is in front.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_host_display::input::translate::Phase;
use aim_host_display::windows::{Frame, Screen, bounds, content, fit, turn, view_to_display};
use aim_hostcall::display::{Window as Record, orientation, window};

use crate::hosts::Host;
use crate::metal::Target;
use crate::objc::{
    CGPoint, CGRect, CGSize, Id, Sel, class, class_addMethod, nsstring, on_main, on_main_after,
    release, sel, text,
};
use crate::window::{NS_BACKING_STORE_BUFFERED, NS_WINDOW_STYLE};

/// The longest a press waits for its task to come to the front.
const HOLD_MS: u64 = 250;
/// How long after the user moved or resized a window Android's bounds for
/// it are taken as answers to that, not as moves of Android's own.
const SETTLE_MS: u64 = 500;
/// `NSWindowCollectionBehaviorFullScreenNone`: the green button zooms.
const FULL_SCREEN_NONE: usize = 1 << 9;
/// `NSWindowToolbarStyleUnified`.
const TOOLBAR_UNIFIED: isize = 3;
/// `NSWindowOcclusionStateVisible`.
const OCCLUSION_VISIBLE: usize = 1 << 1;
/// `NSViewWidthSizable | NSViewHeightSizable`.
const SIZABLE: usize = 2 | 16;
/// `NSViewMinXMargin | NSViewMaxXMargin | NSViewMaxYMargin`: stays at the
/// bottom center.
const BOTTOM_CENTER: usize = 1 | 4 | 32;
/// Where the dismissed back hint is remembered: the `dev.aim` defaults.
const DEFAULTS: &str = "dev.aim";
const HINT_DISMISSED: &str = "BackHintDismissed";
// NSVisualEffectView's material, blending mode and state.
const HUD_MATERIAL: isize = 13;
const WITHIN_WINDOW: isize = 1;
const ACTIVE: isize = 1;
const HINT: &str = "Swipe with two fingers or press \u{2318}[ to go back";
/// A splash window's content before the task has bounds, in points:
/// Android's default size of a freeform task (`LaunchParamsUtil`, 412 x
/// 732 dp), as on a 2x screen.
const SPLASH: CGSize = CGSize {
    width: 412.0,
    height: 732.0,
};
/// The app icon's size on a splash, in points.
const SPLASH_ICON: f64 = 128.0;
/// `NSViewMinXMargin | NSViewMaxXMargin | NSViewMinYMargin |
/// NSViewMaxYMargin`: stays centered.
const CENTERED: usize = 1 | 4 | 8 | 32;

struct TaskWindow {
    window: Id,
    layer: Id,
    view: Id,
    /// The task's bounds and caption, in display pixels.
    bounds: [i32; 4],
    caption: i32,
    visible: bool,
    closed: bool,
    /// When the user last moved or resized the window.
    moved: Option<Instant>,
    /// The content's size (points) the user gave the window, while it is
    /// turned for an orientation an activity asked for.
    user: Option<CGSize>,
}

impl TaskWindow {
    fn content(&self) -> [i32; 4] {
        content(self.bounds, self.caption)
    }
}

/// Touches held for a task to come to the front.
struct Held {
    task: i32,
    generation: u64,
    touches: Vec<(Phase, f64, f64, [i32; 4], i64)>,
}

/// What the bridge said of a task.
#[derive(Clone, Default)]
struct Info {
    package: String,
    /// The activity it was started with, `package/class`.
    activity: String,
    title: String,
    /// Its bounds and caption, once it has a window.
    bounds: Option<([i32; 4], i32)>,
    /// The window host that shows it (in the server).
    host: Option<Arc<Host>>,
    /// Its window asked to close it.
    closing: bool,
    /// The orientation an activity of it asked for.
    orientation: u32,
}

impl Info {
    /// The window title: the task's label, else as a launcher names it:
    /// the window host's name, or (in the server) its launcher activity's
    /// or app's label from the shims; else its package.
    fn title(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        crate::shim::app_name()
            .or_else(|| crate::apps::title(&self.package, &self.activity))
            .unwrap_or_else(|| self.package.clone())
    }
}

struct State {
    screen: Screen,
    device: Id,
    /// The windows this process shows.
    tasks: HashMap<i32, TaskWindow>,
    infos: HashMap<i32, Info>,
    front: Option<i32>,
    held: Option<Held>,
    generation: u64,
    /// Set while the server changes windows itself, so the delegate does
    /// not report the change back.
    applying: bool,
    /// The splash of a launch not yet drawn (in a window host).
    splash: Option<Splash>,
}

/// A launch's splash: a window of its own until the task has one, then a
/// view over the task in the task's windows.
struct Splash {
    window: Option<Id>,
    views: Vec<Id>,
}

thread_local! {
    /// Window mode's state, on the main thread.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The task bridge's connection, for requests (the server's).
static BRIDGE: Mutex<Option<UnixStream>> = Mutex::new(None);

/// Run `f` on the state (main thread, window mode). AppKit calls that may
/// call the delegate back must be made outside `f`.
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Send `r` to the task bridge: this process's, or the server's for a
/// window host.
pub fn send(r: Record) {
    if crate::shim::is_host() {
        return crate::shim::send_window(r);
    }
    let mut bridge = BRIDGE.lock().unwrap();
    let Some(sock) = bridge.as_mut() else { return };
    // SAFETY: `Record` is plain old data.
    let bytes = unsafe {
        std::slice::from_raw_parts((&r as *const Record).cast::<u8>(), size_of::<Record>())
    };
    if let Err(e) = sock.write_all(bytes) {
        eprintln!("aim-display: task bridge: {e}");
    }
}

fn request(op: u32, task: i32, bounds: [i32; 4]) {
    send(Record {
        op,
        task,
        bounds,
        ..Default::default()
    });
}

/// Set up window mode: the display is the main screen, no window yet.
pub fn start(device: Id) -> crate::window::Window {
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let frame = send!(screen, c"frame" => CGRect);
    let screen = Screen {
        width: frame.size.width,
        height: frame.size.height,
        scale: send!(screen, c"backingScaleFactor" => f64),
    };
    let (width, height) = screen.display_size();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            screen,
            device,
            tasks: HashMap::new(),
            infos: HashMap::new(),
            front: None,
            held: None,
            generation: 0,
            applying: false,
            splash: None,
        })
    });
    let (dpi_x, dpi_y) = crate::window::main_display_dpi();
    crate::window::Window {
        layer: None,
        width,
        height,
        dpi_x,
        dpi_y,
        number: None,
    }
}

/// Serve the task bridge's connection until it closes: its records go to
/// the main thread.
pub fn serve_bridge(sock: OwnedFd) {
    let mut sock = UnixStream::from(sock);
    match sock.try_clone() {
        Ok(writer) => *BRIDGE.lock().unwrap() = Some(writer),
        Err(_) => return,
    }
    loop {
        let mut r = Record::default();
        // SAFETY: `Record` is plain old data; every byte pattern is a value.
        let buf = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut r as *mut Record).cast::<u8>(),
                size_of::<Record>(),
            )
        };
        if sock.read_exact(buf).is_err() {
            break;
        }
        on_main(move || on_bridge_record(&r));
    }
    *BRIDGE.lock().unwrap() = None;
    on_main(|| {
        let tasks: Vec<i32> = with(|s| s.infos.keys().copied().collect()).unwrap_or_default();
        for task in tasks {
            on_bridge_record(&Record {
                op: window::REMOVED,
                task,
                ..Default::default()
            });
        }
    });
}

/// Keep what `r` says of its task.
fn note(s: &mut State, r: &Record) -> Info {
    let info = s.infos.entry(r.task).or_default();
    match r.op {
        window::PACKAGE => info.package = r.text().to_string(),
        window::ACTIVITY => info.activity = r.text().to_string(),
        window::TITLE => info.title = r.text().to_string(),
        window::TASK => info.bounds = Some((r.bounds, r.caption)),
        window::ORIENTATION => info.orientation = r.orientation,
        _ => {}
    }
    info.clone()
}

/// A record of the task bridge (in the server): a window host's when one
/// shows the task, else shown here.
fn on_bridge_record(r: &Record) {
    if r.op == window::DRAWN {
        // The launch a window host asked for.
        if let Some((package, _)) = r.text().split_once('/')
            && let Some(h) = crate::hosts::for_task(package, r.text())
        {
            h.send_window(r);
        }
        return;
    }
    with(|s| {
        note(s, r);
        if r.op == window::FRONT {
            s.front = Some(r.task);
        }
    });
    if matches!(r.op, window::PACKAGE | window::ACTIVITY) && route(r.task) {
        // The new owner got the task as it is now, this record included.
        return;
    }
    let host = with(|s| s.infos.get(&r.task).and_then(|i| i.host.clone())).flatten();
    match host {
        Some(h) => {
            h.send_window(r);
            if r.op == window::REMOVED {
                with(|s| s.infos.remove(&r.task));
            }
            if r.op == window::FRONT {
                release_held(None);
            }
        }
        None => apply(r),
    }
}

/// A record the server passed on (in a window host).
pub fn on_host_record(r: &Record) {
    if r.op == window::DRAWN {
        return drawn();
    }
    with(|s| note(s, r));
    apply(r);
}

/// Show what `r` says in this process's windows.
fn apply(r: &Record) {
    match r.op {
        window::TASK => task(r.task, r.bounds, r.caption),
        window::PACKAGE | window::ACTIVITY | window::TITLE => {
            let shown = with(|s| {
                let w = s.tasks.get(&r.task)?.window;
                Some((w, s.infos.get(&r.task)?.title()))
            })
            .flatten();
            if let Some((w, title)) = shown {
                send!(w, c"setTitle:" => (), Id = nsstring(&title));
            }
        }
        window::FRONT => front(r.task),
        window::ORIENTATION => orient(r.task, r.orientation),
        window::HIDE => hide(r.task),
        window::REMOVED => remove(r.task),
        window::MOVED_TO_BACK => {
            if let Some(Some(w)) = with(|s| s.tasks.get(&r.task).map(|t| t.window)) {
                send!(w, c"miniaturize:" => (), Id = std::ptr::null_mut());
            }
        }
        op => eprintln!("aim-display: task record {op}"),
    }
}

/// Give `task` to the window host that should show it now
/// ([`crate::hosts::for_task`]), or to the server: the one that showed it
/// lets it go and the new one gets what is known of it. Whether it moved.
fn route(task: i32) -> bool {
    let Some(Some((old, new, info, front))) = with(|s| {
        let i = s.infos.get_mut(&task)?;
        let new = crate::hosts::for_task(&i.package, &i.activity);
        let same = match (&i.host, &new) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        // A window closed from its host stays closed.
        if same || i.closing {
            return None;
        }
        let old = std::mem::replace(&mut i.host, new.clone());
        Some((old, new, i.clone(), s.front == Some(task)))
    }) else {
        return false;
    };
    match old {
        Some(h) => h.send_window(&Record {
            op: window::REMOVED,
            task,
            ..Default::default()
        }),
        None => close_window(task),
    }
    match new {
        Some(h) => {
            let text = |op, text: &str| Record::with_text(op, task, text);
            h.send_window(&text(window::PACKAGE, &info.package));
            h.send_window(&text(window::ACTIVITY, &info.activity));
            h.send_window(&text(window::TITLE, &info.title));
            if let Some((bounds, caption)) = info.bounds {
                h.send_window(&Record {
                    op: window::TASK,
                    task,
                    bounds,
                    caption,
                    ..Default::default()
                });
            }
            if info.orientation != orientation::ANY {
                h.send_window(&Record {
                    op: window::ORIENTATION,
                    task,
                    orientation: info.orientation,
                    ..Default::default()
                });
            }
            if front {
                h.send_window(&Record {
                    op: window::FRONT,
                    task,
                    ..Default::default()
                });
            }
        }
        None => {
            if let Some((b, caption)) = info.bounds {
                create(task, b, caption);
            }
        }
    }
    true
}

/// A window host of `package` came or went (in the server): its tasks
/// move to the host that should show them now.
pub fn route_package(package: &str) {
    let tasks: Vec<i32> = with(|s| {
        s.infos
            .iter()
            .filter(|(_, i)| i.package == package)
            .map(|(&t, _)| t)
            .collect()
    })
    .unwrap_or_default();
    for task in tasks {
        route(task);
    }
}

/// A window host asked to close `task` (in the server): its window does
/// not come back here when the host goes.
pub fn closing(task: i32) {
    with(|s| {
        if let Some(i) = s.infos.get_mut(&task) {
            i.closing = true;
        }
    });
}

/// The task windows of this process, as (task, window number), for a
/// window host to report.
pub fn window_numbers() -> Vec<(i32, isize)> {
    let windows: Vec<(i32, Id)> =
        with(|s| s.tasks.iter().map(|(&t, w)| (t, w.window)).collect()).unwrap_or_default();
    windows
        .into_iter()
        .map(|(t, w)| (t, send!(w, c"windowNumber" => isize)))
        .collect()
}

fn cg(f: Frame) -> CGRect {
    CGRect {
        x: f.x,
        y: f.y,
        size: CGSize {
            width: f.width,
            height: f.height,
        },
    }
}

fn frame(r: CGRect) -> Frame {
    Frame {
        x: r.x,
        y: r.y,
        width: r.size.width,
        height: r.size.height,
    }
}

/// The window's content, in display pixels.
fn window_content(screen: &Screen, w: Id) -> [i32; 4] {
    let f = send!(w, c"frame" => CGRect);
    screen.to_pixels(frame(
        send!(w, c"contentRectForFrameRect:" => CGRect, CGRect = f),
    ))
}

/// Place `w` so its content covers display pixels `c`.
fn place(screen: &Screen, w: Id, c: [i32; 4]) {
    let f = send!(w, c"frameRectForContentRect:" => CGRect, CGRect = cg(screen.to_frame(c)));
    send!(w, c"setFrame:display:" => (), CGRect = f, bool = true);
}

/// A task's bounds, new or changed.
fn task(task: i32, b: [i32; 4], caption: i32) {
    let Some(known) = with(|s| s.tasks.contains_key(&task)) else {
        return;
    };
    if !known {
        return create(task, b, caption);
    }
    with(|s| {
        if let Some(t) = s.tasks.get_mut(&task) {
            t.bounds = b;
            t.caption = caption;
            let c = t.content();
            send!(t.layer, c"setDrawableSize:" => (), CGSize = CGSize {
                width: (c[2] - c[0]).max(1) as f64,
                height: (c[3] - c[1]).max(1) as f64,
            });
        }
    });
    follow(task);
    update_targets(Some(task));
}

/// Put the window where its task is, unless the user is moving or
/// resizing it: then Android's bounds answer the user's, and the window
/// follows only once the user has stopped (a minimum size Android kept).
fn follow(task: i32) {
    let Some(Some((w, screen, c, recent))) = with(|s| {
        let t = s.tasks.get(&task)?;
        let c = t.content();
        let live = send!(t.window, c"inLiveResize" => bool);
        if t.closed || live || window_content(&s.screen, t.window) == c {
            return None;
        }
        let recent = t
            .moved
            .is_some_and(|m| m.elapsed() < Duration::from_millis(SETTLE_MS));
        s.applying |= !recent;
        Some((t.window, s.screen, c, recent))
    }) else {
        return;
    };
    if recent {
        on_main_after(SETTLE_MS, move || follow(task));
        return;
    }
    place(&screen, w, c);
    with(|s| s.applying = false);
}

/// A new task window over the task's content.
fn create(task: i32, b: [i32; 4], caption: i32) {
    let Some((screen, device, title)) = with(|s| {
        s.applying = true;
        let title = s.infos.get(&task).map(Info::title).unwrap_or_default();
        (s.screen, s.device, title)
    }) else {
        return;
    };
    let c = content(b, caption);
    let f = screen.to_frame(c);
    let w = send!(class(c"NSWindow"), c"alloc" => Id);
    let w = send!(w, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = cg(f), usize = NS_WINDOW_STYLE, usize = NS_BACKING_STORE_BUFFERED, bool = false);
    send!(w, c"setReleasedWhenClosed:" => (), bool = false);
    send!(w, c"setCollectionBehavior:" => (), usize = FULL_SCREEN_NONE);
    // The caption lies under the title bar, so the bar must be as tall.
    let whole = send!(w, c"frame" => CGRect);
    let inner = send!(w, c"contentRectForFrameRect:" => CGRect, CGRect = whole);
    if (whole.size.height - inner.size.height) * screen.scale < caption as f64 {
        let toolbar = send!(class(c"NSToolbar"), c"alloc" => Id);
        let toolbar = send!(toolbar, c"initWithIdentifier:" => Id, Id = nsstring("dev.aim.window"));
        send!(w, c"setToolbar:" => (), Id = toolbar);
        release(toolbar);
        send!(w, c"setToolbarStyle:" => (), isize = TOOLBAR_UNIFIED);
    }
    let (cw, ch) = ((c[2] - c[0]).max(1) as u32, (c[3] - c[1]).max(1) as u32);
    let bounds_pt = CGRect {
        size: CGSize {
            width: f.width,
            height: f.height,
        },
        ..Default::default()
    };
    // The content view holds the view that shows the task and takes its
    // input (a layer-hosting view, which may have no subviews of its own)
    // and, above it, the back hint.
    let container = send!(class(c"NSView"), c"alloc" => Id);
    let container = send!(container, c"initWithFrame:" => Id, CGRect = bounds_pt);
    let view = crate::input::view(bounds_pt);
    send!(view, c"setAutoresizingMask:" => (), usize = SIZABLE);
    // One pixel per pixel from the top left: while the window and the task
    // differ (a live resize), the content is not stretched.
    let layer = crate::window::metal_layer(device, cw, ch, screen.scale, "topLeft");
    send!(view, c"setLayer:" => (), Id = layer);
    send!(view, c"setWantsLayer:" => (), bool = true);
    send!(container, c"addSubview:" => (), Id = view);
    // A launch not yet drawn shows its splash over the task.
    if with(|s| s.splash.is_some()) == Some(true) {
        let splash = splash_view(bounds_pt);
        send!(container, c"addSubview:" => (), Id = splash);
        with(|s| s.splash.as_mut().map(|sp| sp.views.push(splash)));
    }
    send!(w, c"setContentView:" => (), Id = container);
    release(container);
    send!(w, c"makeFirstResponder:" => bool, Id = view);
    back_hint(container);
    send!(w, c"setDelegate:" => (), Id = delegate());
    place(&screen, w, c);
    let t = TaskWindow {
        window: w,
        layer,
        view,
        bounds: b,
        caption,
        visible: true,
        closed: false,
        moved: None,
        user: None,
    };
    send!(w, c"setTitle:" => (), Id = nsstring(&title));
    with(|s| {
        s.tasks.insert(task, t);
    });
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(w, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
    send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
    // The splash moved into the task's window.
    if let Some(Some(sw)) = with(|s| s.splash.as_mut().and_then(|sp| sp.window.take())) {
        send!(sw, c"close" => ());
        release(sw);
    }
    // The screen may not take the window where the task is (above the menu
    // bar, over the Dock): showing it moves it, and the task follows.
    let placed = window_content(&screen, w);
    with(|s| s.applying = false);
    // For `screencapture -l`, and for the server's stacking.
    let number = send!(w, c"windowNumber" => isize);
    eprintln!("aim-display: task {task} window {number}");
    crate::shim::window_number(task, number);
    if placed != c {
        set_bounds(task, bounds(placed, caption));
    }
    update_targets(Some(task));
}

/// The app's launch starts (in a window host): until it has drawn its
/// first frame, a window shows the splash at once, and then the task's
/// window over the task (docs/m1-shell.md, D4). Android draws no starting
/// window without WMShell.
pub fn splash() {
    if with(|s| s.tasks.is_empty() && s.splash.is_none()) != Some(true) {
        return;
    }
    // Centered where Android places a new freeform task.
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let area = frame(send!(screen, c"visibleFrame" => CGRect));
    let c = fit(area, SPLASH.width, SPLASH.height, area);
    let w = send!(class(c"NSWindow"), c"alloc" => Id);
    let w = send!(w, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = cg(c), usize = NS_WINDOW_STYLE, usize = NS_BACKING_STORE_BUFFERED, bool = false);
    send!(w, c"setReleasedWhenClosed:" => (), bool = false);
    send!(w, c"setCollectionBehavior:" => (), usize = FULL_SCREEN_NONE);
    let title = crate::shim::info("CFBundleDisplayName").unwrap_or_default();
    send!(w, c"setTitle:" => (), Id = nsstring(&title));
    let view = splash_view(CGRect {
        size: CGSize {
            width: c.width,
            height: c.height,
        },
        ..Default::default()
    });
    send!(w, c"setContentView:" => (), Id = view);
    release(view);
    send!(w, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
    // On screen now, not once the run loop turns: the host may not run it
    // yet.
    send!(class(c"CATransaction"), c"flush" => ());
    with(|s| {
        s.splash = Some(Splash {
            window: Some(w),
            views: Vec::new(),
        })
    });
}

/// The app's splash screen background in the Mac's appearance, as its
/// shim names it (`AIMSplashBackground`, `AIMSplashBackgroundDark`), else
/// the window background.
fn splash_background() -> Id {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let appearance = send!(app, c"effectiveAppearance" => Id);
    let names = [
        nsstring("NSAppearanceNameAqua"),
        nsstring("NSAppearanceNameDarkAqua"),
    ];
    let names = send!(class(c"NSArray"), c"arrayWithObjects:count:" => Id,
        *const Id = names.as_ptr(), usize = names.len());
    let best = send!(appearance, c"bestMatchFromAppearancesWithNames:" => Id, Id = names);
    let key = if !best.is_null() && text(best) == "NSAppearanceNameDarkAqua" {
        "AIMSplashBackgroundDark"
    } else {
        "AIMSplashBackground"
    };
    let argb =
        crate::shim::info(key).and_then(|v| u32::from_str_radix(v.strip_prefix('#')?, 16).ok());
    let Some(argb) = argb else {
        return send!(class(c"NSColor"), c"windowBackgroundColor" => Id);
    };
    let [a, r, g, b] = argb.to_be_bytes().map(|c| f64::from(c) / 255.0);
    send!(class(c"NSColor"), c"colorWithSRGBRed:green:blue:alpha:" => Id,
        f64 = r, f64 = g, f64 = b, f64 = a)
}

/// The app's icon, centered on its splash background, filling `frame`.
fn splash_view(frame: CGRect) -> Id {
    let v = send!(class(c"NSView"), c"alloc" => Id);
    let v = send!(v, c"initWithFrame:" => Id, CGRect = frame);
    send!(v, c"setAutoresizingMask:" => (), usize = SIZABLE);
    send!(v, c"setWantsLayer:" => (), bool = true);
    let background = send!(splash_background(), c"CGColor" => Id);
    let layer = send!(v, c"layer" => Id);
    send!(layer, c"setBackgroundColor:" => (), Id = background);
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let image = send!(app, c"applicationIconImage" => Id);
    let icon = send!(class(c"NSImageView"), c"imageViewWithImage:" => Id, Id = image);
    send!(icon, c"setFrame:" => (), CGRect = CGRect {
        x: (frame.size.width - SPLASH_ICON) / 2.0,
        y: (frame.size.height - SPLASH_ICON) / 2.0,
        size: CGSize {
            width: SPLASH_ICON,
            height: SPLASH_ICON,
        },
    });
    send!(icon, c"setAutoresizingMask:" => (), usize = CENTERED);
    send!(v, c"addSubview:" => (), Id = icon);
    v
}

/// The launch has drawn its first frame, or ended without one: the
/// splash goes.
fn drawn() {
    let Some(Some(splash)) = with(|s| s.splash.take()) else {
        return;
    };
    for v in splash.views {
        send!(v, c"removeFromSuperview" => ());
        release(v);
    }
    if let Some(w) = splash.window {
        send!(w, c"close" => ());
        release(w);
    }
}

/// An activity of `task` asked for orientation `o`: the window's content
/// turns to its proportions around its centre, within the screen's
/// visible part, or (for none) back to the size the user gave it. The
/// window's resize resizes the task, as the user's does.
fn orient(task: i32, o: u32) {
    let Some(Some((w, user))) = with(|s| {
        let t = s.tasks.get(&task).filter(|t| !t.closed)?;
        Some((t.window, t.user))
    }) else {
        return;
    };
    if send!(w, c"inLiveResize" => bool) {
        return;
    }
    let whole = send!(w, c"frame" => CGRect);
    let c = frame(send!(w, c"contentRectForFrameRect:" => CGRect, CGRect = whole));
    // Below the menu bar and beside the Dock, with room for the title bar.
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let mut area = frame(send!(screen, c"visibleFrame" => CGRect));
    area.height -= whole.size.height - c.height;
    let turned = match o {
        orientation::LANDSCAPE | orientation::PORTRAIT => {
            turn(c, o == orientation::LANDSCAPE, area).map(|f| {
                (
                    f,
                    Some(user.unwrap_or(CGSize {
                        width: c.width,
                        height: c.height,
                    })),
                )
            })
        }
        _ => user.map(|u| (fit(c, u.width, u.height, area), None)),
    };
    let Some((f, user)) = turned else {
        return;
    };
    with(|s| {
        if let Some(t) = s.tasks.get_mut(&task) {
            t.user = user;
        }
    });
    let f = send!(w, c"frameRectForContentRect:" => CGRect, CGRect = cg(f));
    send!(w, c"setFrame:display:" => (), CGRect = f, bool = true);
}

fn set_bounds(task: i32, b: [i32; 4]) {
    request(window::SET_BOUNDS, task, b);
}

/// Android brought `task` to the front.
fn front(task: i32) {
    let Some(w) = with(|s| {
        s.front = Some(task);
        let t = s.tasks.get(&task).filter(|t| !t.closed)?;
        s.applying = true;
        Some(t.window)
    }) else {
        return;
    };
    release_held(None);
    if let Some(w) = w {
        let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
        let key = send!(w, c"isKeyWindow" => bool);
        let minimized = send!(w, c"isMiniaturized" => bool);
        if send!(app, c"isActive" => bool) && !key && !minimized {
            send!(w, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
        }
        with(|s| s.applying = false);
    }
}

/// The task is gone: its window closes.
fn remove(task: i32) {
    with(|s| {
        s.infos.remove(&task);
        if s.front == Some(task) {
            s.front = None;
        }
    });
    close_window(task);
}

/// Close the task's window here.
fn close_window(task: i32) {
    let Some(Some(t)) = with(|s| {
        let t = s.tasks.remove(&task);
        s.applying = true;
        t
    }) else {
        with(|s| s.applying = false);
        return;
    };
    if !t.closed {
        send!(t.window, c"close" => ());
    }
    with(|s| s.applying = false);
    update_targets(None);
    send!(t.window, c"setDelegate:" => (), Id = std::ptr::null_mut());
    release(t.layer);
    release(t.view);
    release(t.window);
}

/// Present into the visible windows; show the last frame at once in
/// `changed`'s.
fn update_targets(changed: Option<i32>) {
    let Some((all, fresh)) = with(|s| {
        let mut all = Vec::new();
        let mut fresh = Vec::new();
        for (&task, t) in &s.tasks {
            if !t.visible || t.closed {
                continue;
            }
            let target = Target::new(t.layer, Some(t.content()));
            if changed == Some(task) {
                fresh.push(target.clone());
            }
            all.push(target);
        }
        (all, fresh)
    }) else {
        return;
    };
    if let Some(d) = crate::DISPLAY.get() {
        d.set_targets(all);
        if !fresh.is_empty() {
            d.refresh(&fresh);
        }
    }
}

/// Where view point (`x`, `y`) of `view` (`view_height` points tall) is,
/// when `view` is a task window's: the task, the display pixel, the task's
/// content and the display pixels per point.
pub fn locate(
    view: Id,
    x: f64,
    y: f64,
    view_height: f64,
) -> Option<(i32, f64, f64, [i32; 4], f64)> {
    with(|s| {
        let (&task, tw) = s.tasks.iter().find(|(_, tw)| tw.view == view)?;
        let c = tw.content();
        let (px, py) = view_to_display(x, y, view_height, [c[0], c[1]], s.screen.scale);
        Some((task, px, py, c, s.screen.scale))
    })
    .flatten()
}

/// Whether `view` is a task window's.
pub fn is_task_view(view: Id) -> bool {
    with(|s| s.tasks.values().any(|tw| tw.view == view)) == Some(true)
}

/// Window mode's handling of a pointer event in `view`; false when `view`
/// is not a task window's.
pub fn pointer(view: Id, phase: Phase, x: f64, y: f64, view_height: f64, t: i64) -> bool {
    let Some((task, px, py, area, _)) = locate(view, x, y, view_height) else {
        return false;
    };
    if crate::shim::is_host() {
        crate::shim::touch(task, phase, px, py, area, t);
    } else {
        touch(task, phase, px, py, area, t);
    }
    true
}

/// The windows this process shows of the app (package) of `task`.
fn app_windows(s: &State, task: i32) -> Vec<Id> {
    let Some(package) = s.infos.get(&task).map(|i| &i.package) else {
        return Vec::new();
    };
    s.tasks
        .iter()
        .filter(|(t, _)| s.infos.get(t).is_some_and(|i| i.package == *package))
        .map(|(_, tw)| tw.window)
        .collect()
}

/// Cmd+Q in the server's task window `w`: its app quits, as a window
/// host's does: every window of its package closes.
pub fn quit_app(w: Id) {
    let windows = with(|s| {
        let (&task, _) = s.tasks.iter().find(|(_, t)| t.window == w)?;
        Some(app_windows(s, task))
    })
    .flatten()
    .unwrap_or_default();
    for w in windows {
        send!(w, c"performClose:" => (), Id = std::ptr::null_mut());
    }
}

/// `task`'s app went home: a window host hides, as Cmd+H hides it, and
/// its Dock icon brings it back; the server, which shows several apps,
/// minimizes that app's windows.
fn hide(task: i32) {
    if crate::shim::is_host() {
        let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
        send!(app, c"hide:" => (), Id = std::ptr::null_mut());
        return;
    }
    for w in with(|s| app_windows(s, task)).unwrap_or_default() {
        send!(w, c"miniaturize:" => (), Id = std::ptr::null_mut());
    }
}

/// A touch at display pixel (`x`, `y`) of `task`'s window `area` (in the
/// server): held until the task is in front.
pub fn touch(task: i32, phase: Phase, x: f64, y: f64, area: [i32; 4], t: i64) {
    let now = with(|s| {
        let touch = (phase, x, y, area, t);
        if let Some(h) = &mut s.held {
            h.touches.push(touch);
            return None;
        }
        if phase == Phase::Down && s.front != Some(task) {
            s.generation += 1;
            let generation = s.generation;
            s.held = Some(Held {
                task,
                generation,
                touches: vec![touch],
            });
            request(window::FOCUS, task, [0; 4]);
            on_main_after(HOLD_MS, move || release_held(Some(generation)));
            return None;
        }
        Some(touch)
    })
    .flatten();
    if let Some((phase, x, y, area, t)) = now
        && let Some(input) = crate::input::input()
    {
        input.touch(phase, x, y, area, t);
    }
}

/// Send the held touches: when their task came to the front (`None`), or
/// when hold `generation` timed out.
fn release_held(generation: Option<u64>) {
    let Some(Some(held)) = with(|s| {
        let h = s.held.as_ref()?;
        let due = match generation {
            Some(g) => h.generation == g,
            None => s.front == Some(h.task),
        };
        if due { s.held.take() } else { None }
    }) else {
        return;
    };
    if let Some(input) = crate::input::input() {
        for (phase, px, py, area, t) in held.touches {
            input.touch(phase, px, py, area, t);
        }
    }
}

/// The task of the window posting `note`, unless the server is changing
/// windows itself.
fn task_of(note: Id) -> Option<(i32, Id)> {
    let w = send!(note, c"object" => Id);
    with(|s| {
        if s.applying {
            return None;
        }
        s.tasks
            .iter()
            .find(|(_, t)| t.window == w)
            .map(|(&task, _)| (task, w))
    })
    .flatten()
}

/// The window moved or was resized: the task follows.
fn moved(note: Id) {
    let Some((task, w)) = task_of(note) else {
        return;
    };
    let Some(Some(b)) = with(|s| {
        let t = s.tasks.get_mut(&task)?;
        t.moved = Some(Instant::now());
        let b = bounds(window_content(&s.screen, w), t.caption);
        (b != t.bounds).then_some(b)
    }) else {
        return;
    };
    set_bounds(task, b);
}

extern "C" fn did_move(_: Id, _: Sel, note: Id) {
    moved(note);
}

extern "C" fn did_resize(_: Id, _: Sel, note: Id) {
    let w = send!(note, c"object" => Id);
    // A live resize resizes the task once, at its end.
    if !send!(w, c"inLiveResize" => bool) {
        moved(note);
    }
}

extern "C" fn did_end_live_resize(_: Id, _: Sel, note: Id) {
    // The user's own size, which a request for no orientation keeps.
    if let Some((task, _)) = task_of(note) {
        with(|s| s.tasks.get_mut(&task).map(|t| t.user = None));
    }
    moved(note);
}

extern "C" fn did_become_key(_: Id, _: Sel, note: Id) {
    if let Some((task, _)) = task_of(note) {
        focus(task);
    }
}

/// Make `task` the focused one, unless it is (in the server, which alone
/// knows which task is in front; a window host asks it).
pub fn focus(task: i32) {
    if crate::shim::is_host() || with(|s| s.front != Some(task)) == Some(true) {
        request(window::FOCUS, task, [0; 4]);
    }
}

extern "C" fn should_close(_: Id, _: Sel, w: Id) -> bool {
    let task = with(|s| {
        s.tasks
            .iter_mut()
            .find(|(_, t)| t.window == w)
            .map(|(&task, t)| {
                t.closed = true;
                task
            })
    })
    .flatten();
    if let Some(task) = task {
        request(window::CLOSE, task, [0; 4]);
        update_targets(None);
    }
    true
}

/// A minimized window's task goes behind the visible ones.
extern "C" fn did_miniaturize(_: Id, _: Sel, _note: Id) {
    if crate::shim::is_host() {
        crate::shim::restack();
    } else {
        restack();
    }
}

/// Stack the tasks as the screen stacks their windows (in the server):
/// focus the visible ones from the back to the front. The windows of all
/// processes count, by their window numbers.
pub fn restack() {
    let order = crate::window::on_screen_windows();
    let local = window_numbers();
    let tasks: Vec<i32> = order
        .iter()
        .filter_map(|&n| {
            local
                .iter()
                .find(|&&(_, w)| w == n as isize)
                .map(|&(t, _)| t)
                .or_else(|| crate::hosts::task_of_window(n as isize))
        })
        .collect();
    for &task in tasks.iter().rev() {
        request(window::FOCUS, task, [0; 4]);
    }
}

extern "C" fn did_change_occlusion(_: Id, _: Sel, note: Id) {
    let w = send!(note, c"object" => Id);
    let visible = send!(w, c"occlusionState" => usize) & OCCLUSION_VISIBLE != 0;
    let changed = with(|s| {
        let (&task, t) = s.tasks.iter_mut().find(|(_, t)| t.window == w)?;
        (t.visible != visible).then(|| {
            t.visible = visible;
            task
        })
    })
    .flatten();
    if let Some(task) = changed {
        update_targets(visible.then_some(task));
    }
}

/// The defaults the back hint's dismissal is kept in.
fn defaults() -> Id {
    let d = send!(class(c"NSUserDefaults"), c"alloc" => Id);
    send!(d, c"initWithSuiteName:" => Id, Id = nsstring(DEFAULTS))
}

/// Tell how to go back, at the bottom of the window's `content`, once:
/// until the hint is dismissed, in the first window of each run.
fn back_hint(content: Id) {
    static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let d = defaults();
    let dismissed = send!(d, c"boolForKey:" => bool, Id = nsstring(HINT_DISMISSED));
    release(d);
    if dismissed || SHOWN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let button = send!(class(c"NSButton"),
        c"buttonWithTitle:target:action:" => Id,
        Id = nsstring(&format!("{HINT}   \u{2715}")), Id = hint_target(), Sel = sel(c"dismiss:"));
    send!(button, c"setBordered:" => (), bool = false);
    send!(button, c"sizeToFit" => ());
    let size = send!(button, c"frame" => CGRect).size;
    // A dark, rounded HUD behind it, legible over any content.
    let pad = 8.0;
    let plate = CGSize {
        width: size.width + 2.0 * pad,
        height: size.height + 2.0 * pad,
    };
    let area = send!(content, c"bounds" => CGRect).size;
    let hud = send!(class(c"NSVisualEffectView"), c"alloc" => Id);
    let hud = send!(hud, c"initWithFrame:" => Id, CGRect = CGRect {
        x: ((area.width - plate.width) / 2.0).max(0.0),
        y: 16.0,
        size: plate,
    });
    send!(hud, c"setMaterial:" => (), isize = HUD_MATERIAL);
    send!(hud, c"setBlendingMode:" => (), isize = WITHIN_WINDOW);
    send!(hud, c"setState:" => (), isize = ACTIVE);
    let dark = send!(class(c"NSAppearance"), c"appearanceNamed:" => Id,
        Id = nsstring("NSAppearanceNameVibrantDark"));
    send!(hud, c"setAppearance:" => (), Id = dark);
    send!(hud, c"setWantsLayer:" => (), bool = true);
    let layer = send!(hud, c"layer" => Id);
    send!(layer, c"setCornerRadius:" => (), f64 = plate.height / 2.0);
    send!(layer, c"setMasksToBounds:" => (), bool = true);
    send!(button, c"setFrameOrigin:" => (), CGPoint = CGPoint { x: pad, y: pad });
    send!(hud, c"addSubview:" => (), Id = button);
    send!(hud, c"setAutoresizingMask:" => (), usize = BOTTOM_CENTER);
    send!(content, c"addSubview:" => (), Id = hud);
    release(hud);
}

extern "C" fn dismiss_hint(_: Id, _: Sel, button: Id) {
    let hud = send!(button, c"superview" => Id);
    send!(hud, c"removeFromSuperview" => ());
    let d = defaults();
    send!(d, c"setBool:forKey:" => (), bool = true, Id = nsstring(HINT_DISMISSED));
    release(d);
}

/// The back hint's button target, registered once.
fn hint_target() -> Id {
    static TARGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *TARGET.get_or_init(|| {
        let cls = crate::input::subclass(
            c"NSObject",
            c"DarwinBackHint",
            &[(c"dismiss:", dismiss_hint)],
        );
        send!(cls, c"new" => Id) as usize
    }) as Id
}

/// The task windows' delegate, registered once.
fn delegate() -> Id {
    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DELEGATE.get_or_init(|| {
        let cls = crate::input::subclass(
            c"NSObject",
            c"DarwinTaskWindowDelegate",
            &[
                (c"windowDidMove:", did_move),
                (c"windowDidResize:", did_resize),
                (c"windowDidEndLiveResize:", did_end_live_resize),
                (c"windowDidBecomeKey:", did_become_key),
                (c"windowDidResignKey:", crate::input::resign_key),
                (c"windowDidMiniaturize:", did_miniaturize),
                (c"windowDidChangeOcclusionState:", did_change_occlusion),
            ],
        );
        // SAFETY: adds a BOOL method to the class registered above.
        unsafe {
            class_addMethod(
                cls,
                sel(c"windowShouldClose:"),
                should_close as *const c_void,
                c"B@:@".as_ptr(),
            );
        }
        send!(cls, c"new" => Id) as usize
    }) as Id
}
