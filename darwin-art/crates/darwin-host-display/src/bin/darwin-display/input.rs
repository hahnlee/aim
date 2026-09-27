//! The window's AppKit events, into the input devices
//! (`darwin_host_display::input`, `docs/input.md`).
//!
//! The content view is a small `NSView` subclass that takes the keyboard
//! (first responder) and the mouse, reads each event's plain values and
//! hands them to [`Input`]; the window delegate releases everything when
//! the window stops being key. IME composition (`NSTextInputClient`) is
//! not handled yet (#23): keys go out as physical keys.

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use darwin_host_display::input::server::Devices;
use darwin_host_display::input::translate::{Input, Phase};
use darwin_host_display::input::{device_dir, devices};

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

fn time(event: Id) -> i64 {
    uptime_to_monotonic(send!(event, c"timestamp" => f64))
}

fn pointer(view: Id, event: Id, phase: Phase) {
    let Some(input) = INPUT.get() else { return };
    let at = send!(event, c"locationInWindow" => CGPoint);
    let p =
        send!(view, c"convertPoint:fromView:" => CGPoint, CGPoint = at, Id = std::ptr::null_mut());
    let b = send!(view, c"bounds" => CGRect);
    input.pointer(phase, p.x, p.y, b.size.width, b.size.height, time(event));
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

extern "C" fn scroll_wheel(_: Id, _: Sel, event: Id) {
    if let Some(input) = INPUT.get() {
        let dy = send!(event, c"scrollingDeltaY" => f64);
        let precise = send!(event, c"hasPreciseScrollingDeltas" => bool);
        input.scroll(dy, precise, time(event));
    }
}

fn key(event: Id, down: bool) {
    if let Some(input) = INPUT.get() {
        let code = send!(event, c"keyCode" => u16);
        let repeat = down && send!(event, c"isARepeat" => bool);
        input.key(code, down, repeat, time(event));
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

extern "C" fn resign_key(_: Id, _: Sel, _note: Id) {
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
/// (`v@:@`) and BOOL methods.
fn subclass(
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

/// The content view: `frame` in points. It hosts the display's layer and
/// takes the window's input.
pub fn view(frame: CGRect) -> Id {
    let cls = subclass(
        c"NSView",
        c"DarwinDisplayView",
        &[
            (c"mouseDown:", mouse_down),
            (c"mouseDragged:", mouse_dragged),
            (c"mouseUp:", mouse_up),
            (c"scrollWheel:", scroll_wheel),
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
    let v = send!(cls, c"alloc" => Id);
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
