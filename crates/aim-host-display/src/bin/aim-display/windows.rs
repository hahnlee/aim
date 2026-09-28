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
//! - closing the window removes the task, and a removed task's window
//!   closes.
//!
//! A press in a window whose task is not the top one focuses the task first
//! and holds the touch until Android reports the task in front (or
//! [`HOLD_MS`] pass), so it lands on that task and not on one above it.
//!
//! The window chrome is the Mac's, with no button of Android's: the first
//! window says once how to go back ([`back_hint`]), until dismissed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use aim_host_display::input::translate::Phase;
use aim_host_display::windows::{Frame, Screen, bounds, content, view_to_display};
use aim_hostcall::display::{Window as Record, window};

use crate::metal::Target;
use crate::objc::{
    CGPoint, CGRect, CGSize, Id, Sel, class, class_addMethod, nsstring, on_main, on_main_after,
    release, sel,
};
use crate::window::{NS_BACKING_STORE_BUFFERED, NS_WINDOW_STYLE};

/// The longest a press waits for its task to come to the front.
const HOLD_MS: u64 = 250;
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

struct TaskWindow {
    window: Id,
    layer: Id,
    view: Id,
    /// The task's bounds and caption, in display pixels.
    bounds: [i32; 4],
    caption: i32,
    package: String,
    title: String,
    visible: bool,
    closed: bool,
    /// Bounds asked for and not answered yet.
    outstanding: u32,
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

struct State {
    screen: Screen,
    device: Id,
    tasks: HashMap<i32, TaskWindow>,
    /// Package and title reported before a task's first bounds.
    early: HashMap<i32, (String, String)>,
    front: Option<i32>,
    held: Option<Held>,
    generation: u64,
    /// Set while the server changes windows itself, so the delegate does
    /// not report the change back.
    applying: bool,
}

thread_local! {
    /// Window mode's state, on the main thread.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The task bridge's connection, for requests.
static BRIDGE: Mutex<Option<UnixStream>> = Mutex::new(None);

/// Run `f` on the state (main thread, window mode). AppKit calls that may
/// call the delegate back must be made outside `f`.
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn send(r: Record) {
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
            early: HashMap::new(),
            front: None,
            held: None,
            generation: 0,
            applying: false,
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
        on_main(move || on_record(&r));
    }
    *BRIDGE.lock().unwrap() = None;
    on_main(|| {
        let tasks: Vec<i32> = with(|s| s.tasks.keys().copied().collect()).unwrap_or_default();
        for task in tasks {
            remove(task);
        }
    });
}

fn on_record(r: &Record) {
    match r.op {
        window::TASK => task(r.task, r.bounds, r.caption),
        window::PACKAGE | window::TITLE => {
            let text = r.text().to_string();
            let is_title = r.op == window::TITLE;
            let shown = with(|s| {
                if let Some(t) = s.tasks.get_mut(&r.task) {
                    if is_title {
                        t.title = text;
                    } else {
                        t.package = text;
                    }
                    return Some((t.window, title_of(t)));
                }
                let e = s.early.entry(r.task).or_default();
                if is_title {
                    e.1 = text;
                } else {
                    e.0 = text;
                }
                None
            })
            .flatten();
            if let Some((w, title)) = shown {
                send!(w, c"setTitle:" => (), Id = nsstring(&title));
            }
        }
        window::FRONT => front(r.task),
        window::REMOVED => remove(r.task),
        window::MOVED_TO_BACK => {
            if let Some(Some(w)) = with(|s| s.tasks.get(&r.task).map(|t| t.window)) {
                send!(w, c"miniaturize:" => (), Id = std::ptr::null_mut());
            }
        }
        op => eprintln!("aim-display: task bridge sent {op}"),
    }
}

/// The window title: the task's label, else its package.
fn title_of(t: &TaskWindow) -> String {
    if !t.title.is_empty() {
        t.title.clone()
    } else {
        t.package.clone()
    }
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
    // Apply Android's bounds to the window only when they are not the
    // answer to a move or resize still going on.
    let apply = with(|s| {
        let t = s.tasks.get_mut(&task)?;
        t.outstanding = t.outstanding.saturating_sub(1);
        t.bounds = b;
        t.caption = caption;
        let c = t.content();
        send!(t.layer, c"setDrawableSize:" => (), CGSize = CGSize {
            width: (c[2] - c[0]).max(1) as f64,
            height: (c[3] - c[1]).max(1) as f64,
        });
        let live = send!(t.window, c"inLiveResize" => bool);
        let differs = window_content(&s.screen, t.window) != c;
        let apply = t.outstanding == 0 && !live && differs && !t.closed;
        s.applying |= apply;
        Some((t.window, s.screen, c, apply))
    })
    .flatten();
    if let Some((w, screen, c, true)) = apply {
        place(&screen, w, c);
        with(|s| s.applying = false);
    }
    update_targets(Some(task));
}

/// A new task window over the task's content.
fn create(task: i32, b: [i32; 4], caption: i32) {
    let Some((screen, device, (package, title))) = with(|s| {
        s.applying = true;
        (
            s.screen,
            s.device,
            s.early.remove(&task).unwrap_or_default(),
        )
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
        package,
        title,
        visible: true,
        closed: false,
        outstanding: 0,
    };
    send!(w, c"setTitle:" => (), Id = nsstring(&title_of(&t)));
    // The screen may not take the window where the task is (above the
    // menu bar): the task follows the window.
    let placed = window_content(&screen, w);
    with(|s| {
        s.tasks.insert(task, t);
    });
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(w, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
    send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
    with(|s| s.applying = false);
    // For `screencapture -l`.
    eprintln!(
        "aim-display: task {task} window {}",
        send!(w, c"windowNumber" => isize)
    );
    if placed != c {
        set_bounds(task, bounds(placed, caption));
    }
    update_targets(Some(task));
}

fn set_bounds(task: i32, b: [i32; 4]) {
    with(|s| {
        if let Some(t) = s.tasks.get_mut(&task) {
            t.outstanding += 1;
        }
    });
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
    let Some(Some(t)) = with(|s| {
        s.early.remove(&task);
        if s.front == Some(task) {
            s.front = None;
        }
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

/// Window mode's handling of a pointer event in `view`; false when `view`
/// is not a task window's.
pub fn pointer(view: Id, phase: Phase, x: f64, y: f64, view_height: f64, t: i64) -> bool {
    // None: not a task window's view; Some(None): held.
    let touch = with(|s| {
        let (&task, tw) = s.tasks.iter().find(|(_, tw)| tw.view == view)?;
        let c = tw.content();
        let (px, py) = view_to_display(x, y, view_height, [c[0], c[1]], s.screen.scale);
        let touch = (phase, px, py, c, t);
        if let Some(h) = &mut s.held {
            h.touches.push(touch);
            return Some(None);
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
            return Some(None);
        }
        Some(Some(touch))
    })
    .flatten();
    let Some(touch) = touch else {
        return false;
    };
    if let Some((phase, px, py, area, t)) = touch
        && let Some(input) = crate::input::input()
    {
        input.touch(phase, px, py, area, t);
    }
    true
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
        let t = s.tasks.get(&task)?;
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
    moved(note);
}

extern "C" fn did_become_key(_: Id, _: Sel, note: Id) {
    if let Some((task, _)) = task_of(note)
        && with(|s| s.front != Some(task)) == Some(true)
    {
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

/// A minimized window's task goes behind the visible ones: focus those
/// from the back to the front, as the screen stacks them.
extern "C" fn did_miniaturize(_: Id, _: Sel, _note: Id) {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let ordered = send!(app, c"orderedWindows" => Id);
    let n = send!(ordered, c"count" => usize);
    let windows: Vec<Id> = (0..n)
        .map(|i| send!(ordered, c"objectAtIndex:" => Id, usize = i))
        .filter(|&w| send!(w, c"isVisible" => bool) && !send!(w, c"isMiniaturized" => bool))
        .collect();
    let tasks: Vec<i32> = with(|s| {
        windows
            .iter()
            .filter_map(|&w| {
                s.tasks
                    .iter()
                    .find(|(_, t)| t.window == w)
                    .map(|(&task, _)| task)
            })
            .collect()
    })
    .unwrap_or_default();
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
