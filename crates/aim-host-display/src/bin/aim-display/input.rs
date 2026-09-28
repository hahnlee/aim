//! The window's AppKit events, into the input devices
//! (`aim_host_display::input`, `docs/input.md`).
//!
//! The content view is a small `NSView` subclass that takes the keyboard
//! (first responder) and the mouse, reads each event's plain values and
//! hands them to [`Input`]; the window delegate releases everything when
//! the window stops being key. IME composition (`NSTextInputClient`) is
//! not handled yet (#23): keys go out as physical keys.
//!
//! Back is the Mac's: Esc (a key like any other), Cmd+[, the mouse's back
//! button, and a two-finger swipe to the right when the system's "swipe
//! between pages" is on.

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use aim_host_display::input::server::Devices;
use aim_host_display::input::translate::{Gesture, Input, Phase};
use aim_host_display::input::{device_dir, devices};

use crate::objc::{CGPoint, CGRect, Id, Sel, class, class_addMethod, sel};
use crate::objc::{objc_allocateClassPair, objc_registerClassPair};
use crate::vsync::uptime_to_monotonic;
use crate::window::Window;

static INPUT: OnceLock<Input> = OnceLock::new();

/// Create the devices of the server at `socket` for `win`'s display.
pub fn start(socket: &Path, win: &Window) -> std::io::Result<()> {
    let dir = device_dir(socket);
    let devs = Devices::create(&dir, devices(win.width, win.height, win.dpi_x, win.dpi_y))?;
    let _ = INPUT.set(Input::new(devs, win.width, win.height));
    Ok(())
}

pub fn input() -> Option<&'static Input> {
    INPUT.get()
}

pub fn time(event: Id) -> i64 {
    uptime_to_monotonic(send!(event, c"timestamp" => f64))
}

pub fn pointer(view: Id, event: Id, phase: Phase) {
    let Some(input) = INPUT.get() else { return };
    let at = send!(event, c"locationInWindow" => CGPoint);
    let p =
        send!(view, c"convertPoint:fromView:" => CGPoint, CGPoint = at, Id = std::ptr::null_mut());
    let b = send!(view, c"bounds" => CGRect);
    if !crate::windows::pointer(view, phase, p.x, p.y, b.size.height, time(event)) {
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

// NSEventPhase
const PHASE_BEGAN: usize = 1;
const PHASE_CHANGED: usize = 4;
const PHASE_ENDED: usize = 8;
const PHASE_CANCELLED: usize = 16;
/// NSEventModifierFlagCommand
const COMMAND: u64 = 1 << 20;
/// The mouse's back button (`buttonNumber` of the fourth button).
const BACK_BUTTON: isize = 3;

extern "C" fn scroll_wheel(_: Id, _: Sel, event: Id) {
    let Some(input) = INPUT.get() else { return };
    let dx = send!(event, c"scrollingDeltaX" => f64);
    let dy = send!(event, c"scrollingDeltaY" => f64);
    let precise = send!(event, c"hasPreciseScrollingDeltas" => bool);
    input.scroll(dy, precise, time(event));
    // Two fingers on a trackpad: the fingers' own direction, whatever the
    // scrolling direction setting.
    let swipes = send!(class(c"NSEvent"), c"isSwipeTrackingFromScrollEventsEnabled" => bool);
    if !precise || !swipes || send!(event, c"momentumPhase" => usize) != 0 {
        return;
    }
    let sign = if send!(event, c"isDirectionInvertedFromDevice" => bool) {
        1.0
    } else {
        -1.0
    };
    let gesture = match send!(event, c"phase" => usize) {
        PHASE_BEGAN => Gesture::Began,
        PHASE_CHANGED => Gesture::Changed,
        PHASE_ENDED | PHASE_CANCELLED => Gesture::Ended,
        _ => return,
    };
    input.swipe(gesture, sign * dx, sign * dy, time(event));
}

extern "C" fn other_mouse(_: Id, _: Sel, event: Id, down: bool) {
    if let Some(input) = INPUT.get()
        && send!(event, c"buttonNumber" => isize) == BACK_BUTTON
    {
        input.back(down, time(event));
    }
}

extern "C" fn other_mouse_down(this: Id, s: Sel, event: Id) {
    other_mouse(this, s, event, true);
}

extern "C" fn other_mouse_up(this: Id, s: Sel, event: Id) {
    other_mouse(this, s, event, false);
}

fn key(event: Id, down: bool) {
    let Some(input) = INPUT.get() else { return };
    // Android repeats keys itself.
    if down && send!(event, c"isARepeat" => bool) {
        return;
    }
    let code = send!(event, c"keyCode" => u16);
    let command = send!(event, c"modifierFlags" => u64) & COMMAND != 0;
    if !input.back_shortcut(code, down, command, time(event)) {
        input.key(code, down, false, time(event));
    }
}

extern "C" fn key_down(_: Id, _: Sel, event: Id) {
    key(event, true);
}

extern "C" fn key_up(_: Id, _: Sel, event: Id) {
    key(event, false);
}

extern "C" fn flags_changed(_: Id, _: Sel, event: Id) {
    if let Some(input) = INPUT.get() {
        let code = send!(event, c"keyCode" => u16);
        let flags = send!(event, c"modifierFlags" => u64);
        input.flags_changed(code, flags, time(event));
    }
}

pub extern "C" fn resign_key(_: Id, _: Sel, _note: Id) {
    if let Some(input) = INPUT.get() {
        input.release_all(crate::vsync::monotonic_ns());
    }
}

/// The devices disappear with the window (hotplug for the guest).
pub extern "C" fn will_terminate(_: Id, _: Sel, _note: Id) {
    if let Some(input) = INPUT.get() {
        input.close();
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
                (c"scrollWheel:", scroll_wheel),
                (c"otherMouseDown:", other_mouse_down),
                (c"otherMouseUp:", other_mouse_up),
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

/// A content view: `frame` in points. It hosts a layer of the display and
/// takes its window's input.
pub fn view(frame: CGRect) -> Id {
    let v = send!(view_class(), c"alloc" => Id);
    send!(v, c"initWithFrame:" => Id, CGRect = frame)
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
