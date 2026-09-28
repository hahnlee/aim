//! The windows' AppKit events, into the input devices
//! (`aim_host_display::input`, `docs/input.md`).
//!
//! The content view is a small `NSView` subclass that takes the keyboard
//! (first responder) and the mouse, reads each event's plain values and
//! hands them to [`Input`] (a window host sends them to the server, which
//! applies them the same way, `hosts::apply`); the window delegate releases
//! everything when the window stops being key. IME composition
//! (`NSTextInputClient`) is not handled yet (#23): keys go out as physical
//! keys.
//!
//! The primary button is a touch; hovering, the other buttons, scrolling
//! and the trackpad's pinch and rotation go where the pointer is. In
//! window mode Cmd+W closes the window (its task) and Cmd+Q quits the app,
//! closing its tasks.

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use aim_host_display::input::server::Devices;
use aim_host_display::input::translate::{COMMAND, Input, Phase};
use aim_host_display::input::{device_dir, devices};
use aim_host_display::wire::{HostInput, input as kind};

use crate::objc::{CGPoint, CGRect, Id, Sel, class, class_addMethod, sel};
use crate::objc::{objc_allocateClassPair, objc_registerClassPair};
use crate::vsync::uptime_to_monotonic;
use crate::window::Window;

static INPUT: OnceLock<Input> = OnceLock::new();

/// Create the devices of the server at `socket` for `win`'s display.
pub fn start(socket: &Path, win: &Window) -> std::io::Result<()> {
    let dir = device_dir(socket);
    let devs = Devices::create(&dir, devices(win.width, win.height, win.dpi_x, win.dpi_y))?;
    let _ = INPUT.set(Input::new(devs, win.width, win.height, win.dpi_y));
    Ok(())
}

pub fn input() -> Option<&'static Input> {
    INPUT.get()
}

pub fn time(event: Id) -> i64 {
    uptime_to_monotonic(send!(event, c"timestamp" => f64))
}

/// An event for the devices: to the server's own, or, in a window host,
/// to the server.
fn deliver(i: HostInput) {
    if crate::shim::is_host() {
        crate::shim::input(i);
    } else {
        crate::hosts::apply(i);
    }
}

/// The event's point in `view`, and the view's bounds.
fn point(view: Id, event: Id) -> (CGPoint, CGRect) {
    let at = send!(event, c"locationInWindow" => CGPoint);
    let p =
        send!(view, c"convertPoint:fromView:" => CGPoint, CGPoint = at, Id = std::ptr::null_mut());
    (p, send!(view, c"bounds" => CGRect))
}

/// Where `event` is on the display: the pixel, the area that bounds it
/// (the task's content in window mode) and display pixels per point.
fn locate(view: Id, event: Id) -> Option<(f64, f64, [i32; 4], f64)> {
    let (p, b) = point(view, event);
    if let Some((_, x, y, area, scale)) = crate::windows::locate(view, p.x, p.y, b.size.height) {
        return Some((x, y, area, scale));
    }
    let input = INPUT.get()?;
    let (x, y) = input.at(p.x, p.y, b.size.width, b.size.height)?;
    let scale = input.pixels_per_point(b.size.width, b.size.height);
    Some((x, y, input.display(), scale))
}

pub fn pointer(view: Id, event: Id, phase: Phase) {
    let (p, b) = point(view, event);
    if crate::windows::pointer(view, phase, p.x, p.y, b.size.height, time(event)) {
        return;
    }
    if let Some(input) = INPUT.get() {
        input.pointer(phase, p.x, p.y, b.size.width, b.size.height, time(event));
    }
}

extern "C" fn mouse_down(this: Id, _: Sel, event: Id) {
    pointer(this, event, Phase::Down);
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event: Id) {
    pointer(this, event, Phase::Drag);
}

extern "C" fn mouse_up(this: Id, _: Sel, event: Id) {
    pointer(this, event, Phase::Up);
}

/// The pointer moved over the view (no button, or one other than the
/// primary held): the mouse follows it.
extern "C" fn mouse_moved(this: Id, _: Sel, event: Id) {
    if let Some((x, y, _, _)) = locate(this, event) {
        deliver(HostInput {
            kind: kind::HOVER,
            x,
            y,
            time_ns: time(event),
            ..Default::default()
        });
    }
}

extern "C" fn mouse_exited(_: Id, _: Sel, event: Id) {
    deliver(HostInput {
        kind: kind::LEAVE,
        time_ns: time(event),
        ..Default::default()
    });
}

/// A button other than the primary one: `code` as `wire::input::BUTTON`.
fn button(view: Id, event: Id, code: u32, down: bool) {
    if let Some((x, y, _, _)) = locate(view, event) {
        deliver(HostInput {
            kind: kind::BUTTON,
            code,
            down: down as u32,
            x,
            y,
            time_ns: time(event),
            ..Default::default()
        });
    }
}

extern "C" fn right_mouse_down(this: Id, _: Sel, event: Id) {
    button(this, event, 0, true);
}

extern "C" fn right_mouse_up(this: Id, _: Sel, event: Id) {
    button(this, event, 0, false);
}

/// `buttonNumber` 2, 3 and 4: middle, back and forward.
fn other_mouse(view: Id, event: Id, down: bool) {
    let n = send!(event, c"buttonNumber" => isize);
    if (2..=4).contains(&n) {
        button(view, event, n as u32 - 1, down);
    }
}

extern "C" fn other_mouse_down(this: Id, _: Sel, event: Id) {
    other_mouse(this, event, true);
}

extern "C" fn other_mouse_up(this: Id, _: Sel, event: Id) {
    other_mouse(this, event, false);
}

// NSEventPhase
const PHASE_BEGAN: usize = 1;
const PHASE_CHANGED: usize = 4;
const PHASE_ENDED: usize = 8;
const PHASE_CANCELLED: usize = 16;

/// A gesture phase as `wire::input` numbers it.
fn phase(event: Id) -> Option<u32> {
    match send!(event, c"phase" => usize) {
        PHASE_BEGAN => Some(0),
        PHASE_CHANGED => Some(1),
        PHASE_ENDED | PHASE_CANCELLED => Some(2),
        _ => None,
    }
}

extern "C" fn scroll_wheel(this: Id, _: Sel, event: Id) {
    let Some((x, y, _, scale)) = locate(this, event) else {
        return;
    };
    let precise = send!(event, c"hasPreciseScrollingDeltas" => bool);
    let momentum = send!(event, c"momentumPhase" => usize) != 0;
    // A trackpad's deltas are points; the devices take pixels.
    let per = if precise { scale } else { 1.0 };
    let (dx, dy) = (
        send!(event, c"scrollingDeltaX" => f64) * per,
        send!(event, c"scrollingDeltaY" => f64) * per,
    );
    // Two fingers on a trackpad may swipe back: the fingers' own
    // direction, whatever the scrolling direction setting.
    let swipes = send!(class(c"NSEvent"), c"isSwipeTrackingFromScrollEventsEnabled" => bool);
    let (mut down, mut sx, mut sy) = (0, 0.0, 0.0);
    if precise
        && swipes
        && !momentum
        && let Some(p) = phase(event)
    {
        let sign = if send!(event, c"isDirectionInvertedFromDevice" => bool) {
            1.0
        } else {
            -1.0
        };
        (down, sx, sy) = (p + 1, sign * dx / per, sign * dy / per);
    }
    deliver(HostInput {
        kind: kind::SCROLL,
        code: precise as u32 | (momentum as u32) << 1,
        down,
        x,
        y,
        dx,
        dy,
        sx,
        sy,
        time_ns: time(event),
        ..Default::default()
    });
}

/// `magnifyWithEvent:` (`code` 0) or `rotateWithEvent:` (1).
fn twist(view: Id, event: Id, code: u32, amount: f64) {
    let (Some((x, y, area, _)), Some(p)) = (locate(view, event), phase(event)) else {
        return;
    };
    deliver(HostInput {
        kind: kind::TWIST,
        code,
        down: p,
        x,
        y,
        area,
        dx: amount,
        time_ns: time(event),
        ..Default::default()
    });
}

extern "C" fn magnify(this: Id, _: Sel, event: Id) {
    twist(this, event, 0, send!(event, c"magnification" => f64));
}

extern "C" fn rotate(this: Id, _: Sel, event: Id) {
    twist(this, event, 1, send!(event, c"rotation" => f32) as f64);
}

// kVK_ANSI_W, kVK_ANSI_Q
const MAC_W: u16 = 0x0d;
const MAC_Q: u16 = 0x0c;

/// Window mode's Cmd+W and Cmd+Q, in `view`'s window: whether it was one.
fn window_shortcut(view: Id, code: u16) -> bool {
    let w = send!(view, c"window" => Id);
    if !crate::windows::is_task_view(view) {
        return false;
    }
    match code {
        MAC_W => send!(w, c"performClose:" => (), Id = std::ptr::null_mut()),
        MAC_Q if crate::shim::is_host() => {
            let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
            send!(app, c"terminate:" => (), Id = std::ptr::null_mut());
        }
        MAC_Q => crate::windows::quit_app(w),
        _ => return false,
    }
    true
}

fn key(view: Id, event: Id, down: bool) {
    let code = send!(event, c"keyCode" => u16);
    let flags = send!(event, c"modifierFlags" => u64);
    let repeat = down && send!(event, c"isARepeat" => bool);
    if down && !repeat && flags & COMMAND != 0 && window_shortcut(view, code) {
        return;
    }
    deliver(HostInput {
        kind: kind::KEY,
        code: code as u32,
        down: if repeat { 2 } else { down as u32 },
        flags,
        time_ns: time(event),
        ..Default::default()
    });
}

extern "C" fn key_down(this: Id, _: Sel, event: Id) {
    key(this, event, true);
}

extern "C" fn key_up(this: Id, _: Sel, event: Id) {
    key(this, event, false);
}

extern "C" fn flags_changed(_: Id, _: Sel, event: Id) {
    deliver(HostInput {
        kind: kind::FLAGS,
        code: send!(event, c"keyCode" => u16) as u32,
        flags: send!(event, c"modifierFlags" => u64),
        time_ns: time(event),
        ..Default::default()
    });
}

pub extern "C" fn resign_key(_: Id, _: Sel, _note: Id) {
    deliver(HostInput {
        kind: kind::RELEASE_ALL,
        time_ns: crate::vsync::monotonic_ns(),
        ..Default::default()
    });
}

/// The devices disappear with the window (hotplug for the guest); a
/// window host's app closes its tasks.
pub extern "C" fn will_terminate(_: Id, _: Sel, _note: Id) {
    if let Some(input) = INPUT.get() {
        input.close();
    }
    if crate::shim::is_host() {
        crate::shim::close_all();
    }
}

unsafe extern "C" {
    static _dispatch_source_type_signal: c_void;
    static _dispatch_main_q: c_void;
    fn dispatch_source_create(
        ty: *const c_void,
        handle: usize,
        mask: usize,
        q: *const c_void,
    ) -> *mut c_void;
    fn dispatch_source_set_event_handler_f(
        source: *mut c_void,
        handler: extern "C" fn(*mut c_void),
    );
    fn dispatch_resume(object: *mut c_void);
}

extern "C" fn terminate(_: *mut c_void) {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"terminate:" => (), Id = std::ptr::null_mut());
}

/// SIGTERM and SIGINT quit the application normally, on the main thread,
/// so the devices are removed as when the window closes.
pub fn quit_on_signals() {
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the signal is ignored so only the dispatch source sees
        // it; the source lives for the process.
        unsafe {
            libc::signal(sig, libc::SIG_IGN);
            let s = dispatch_source_create(
                &raw const _dispatch_source_type_signal,
                sig as usize,
                0,
                &raw const _dispatch_main_q,
            );
            dispatch_source_set_event_handler_f(s, terminate);
            dispatch_resume(s);
        }
    }
}

extern "C" fn yes(_: Id, _: Sel) -> bool {
    true
}

extern "C" fn yes_for(_: Id, _: Sel, _: Id) -> bool {
    true
}

/// Register `name`, a subclass of `superclass`, with event methods
/// (`v@:@`).
pub fn subclass(
    superclass: &std::ffi::CStr,
    name: &std::ffi::CStr,
    events: &[(&std::ffi::CStr, extern "C" fn(Id, Sel, Id))],
) -> Id {
    // SAFETY: registers a new class with methods of the stated encodings.
    unsafe {
        let cls = objc_allocateClassPair(class(superclass), name.as_ptr(), 0);
        for (s, f) in events {
            class_addMethod(cls, sel(s), *f as *const c_void, c"v@:@".as_ptr());
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// The view class, registered once.
fn view_class() -> Id {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let cls = subclass(
            c"NSView",
            c"DarwinDisplayView",
            &[
                (c"mouseDown:", mouse_down),
                (c"mouseDragged:", mouse_dragged),
                (c"mouseUp:", mouse_up),
                (c"mouseMoved:", mouse_moved),
                (c"mouseEntered:", mouse_moved),
                (c"mouseExited:", mouse_exited),
                (c"rightMouseDown:", right_mouse_down),
                (c"rightMouseDragged:", mouse_moved),
                (c"rightMouseUp:", right_mouse_up),
                (c"otherMouseDown:", other_mouse_down),
                (c"otherMouseDragged:", mouse_moved),
                (c"otherMouseUp:", other_mouse_up),
                (c"scrollWheel:", scroll_wheel),
                (c"magnifyWithEvent:", magnify),
                (c"rotateWithEvent:", rotate),
                (c"keyDown:", key_down),
                (c"keyUp:", key_up),
                (c"flagsChanged:", flags_changed),
            ],
        );
        // SAFETY: adds BOOL methods to the class registered above.
        unsafe {
            class_addMethod(
                cls,
                sel(c"acceptsFirstResponder"),
                yes as *const c_void,
                c"B@:".as_ptr(),
            );
            // A click that activates the window is also a touch.
            class_addMethod(
                cls,
                sel(c"acceptsFirstMouse:"),
                yes_for as *const c_void,
                c"B@:@".as_ptr(),
            );
        }
        cls as usize
    }) as Id
}

// NSTrackingAreaOptions: entered and exited, moved, whether or not the app
// is active, over the view's visible rectangle as it changes.
const TRACKING: usize = 0x01 | 0x02 | 0x80 | 0x200;

/// A content view: `frame` in points. It hosts a layer of the display and
/// takes its window's input, the pointer's moves included.
pub fn view(frame: CGRect) -> Id {
    let v = send!(view_class(), c"alloc" => Id);
    let v = send!(v, c"initWithFrame:" => Id, CGRect = frame);
    let area = send!(class(c"NSTrackingArea"), c"alloc" => Id);
    let area = send!(area, c"initWithRect:options:owner:userInfo:" => Id,
        CGRect = CGRect::default(), usize = TRACKING, Id = v, Id = std::ptr::null_mut());
    send!(v, c"addTrackingArea:" => (), Id = area);
    send!(area, c"release" => ());
    v
}

/// The window's delegate: nothing stays pressed in the guest once the
/// window loses the keyboard.
pub fn window_delegate() -> Id {
    let cls = subclass(
        c"NSObject",
        c"DarwinDisplayWindowDelegate",
        &[(c"windowDidResignKey:", resign_key)],
    );
    send!(cls, c"new" => Id)
}
