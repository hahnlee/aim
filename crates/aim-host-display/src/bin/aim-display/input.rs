//! The windows' AppKit events, into the input devices
//! (`aim_host_display::input`, `docs/input.md`).
//!
//! The content view is a small `NSView` subclass that takes the keyboard
//! (first responder) and the mouse, reads each event's plain values and
//! hands them to [`Input`] (a window host sends them to the server, which
//! applies them the same way, `hosts::apply`); the window delegate releases
//! everything when the window stops being key. Keys go out as physical
//! keys, except in a window whose task has an editor the app host serves
//! (`text_input`): there the view is the Mac's input method's client
//! (`NSTextInputClient`), and its marked and committed text and editing
//! commands go to the app's editor.
//!
//! The views' cursor is the pointer's sprite (`cursor.rs`). The primary
//! button is a touch; hovering, the other buttons, scrolling
//! and the trackpad's pinch, rotation and smart zoom go where the pointer
//! is. In
//! window mode Cmd+W closes the window (its task) and Cmd+Q quits the app,
//! closing its tasks.

use std::ffi::c_void;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use aim_host_display::input::server::Devices;
use aim_host_display::input::translate::{COMMAND, Input, Phase, double_tap};
use aim_host_display::input::{KEYBOARD, device_dir, devices, keymap};
use aim_host_display::wire::{HostInput, input as kind};

use aim_host_display::text_input::Message as Text;

use crate::objc::{CGPoint, CGRect, Id, Sel, class, class_addMethod, sel};
use crate::objc::{objc_allocateClassPair, objc_registerClassPair};
use crate::vsync::uptime_to_monotonic;
use crate::window::Window;

static INPUT: OnceLock<Input> = OnceLock::new();

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn LMGetKbdType() -> u8;
    fn KBGetLayoutType(keyboard_type: i16) -> u32;
}

/// `kKeyboardISO`.
const KEYBOARD_ISO: u32 = u32::from_be_bytes(*b"ISO ");

/// Create the devices of the server at `socket` for `win`'s display. The
/// Mac's keyboard (the one last typed on) decides the keymap's ISO keys.
pub fn start(socket: &Path, win: &Window) -> std::io::Result<()> {
    let dir = device_dir(socket);
    let devs = Devices::create(&dir, devices(win.width, win.height, win.dpi_x, win.dpi_y))?;
    // SAFETY: plain HIToolbox queries, on the main thread.
    if unsafe { KBGetLayoutType(LMGetKbdType() as i16) } == KEYBOARD_ISO {
        for (usage, key) in keymap::ISO {
            devs.remap(KEYBOARD, usage, key);
        }
    }
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
    if let Some(at) = crate::panels::locate(view, p.x, p.y, b.size.height) {
        return Some(at);
    }
    let input = INPUT.get()?;
    let (x, y) = input.at(p.x, p.y, b.size.width, b.size.height)?;
    let scale = input.pixels_per_point(b.size.width, b.size.height);
    Some((x, y, input.display(), scale))
}

/// The primary button at `event`'s point, at `t`.
fn pointer(view: Id, event: Id, phase: Phase, t: i64) {
    let (p, b) = point(view, event);
    if crate::windows::pointer(view, phase, p.x, p.y, b.size.height, t) {
        return;
    }
    let Some(input) = INPUT.get() else { return };
    // A system panel's press belongs to no task.
    match crate::panels::locate(view, p.x, p.y, b.size.height) {
        Some((x, y, area, _)) => input.touch(phase, x, y, area, t),
        None => input.pointer(phase, p.x, p.y, b.size.width, b.size.height, t),
    }
}

extern "C" fn mouse_down(this: Id, _: Sel, event: Id) {
    // A touch may move the caret.
    forget_typed();
    pointer(this, event, Phase::Down, time(event));
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event: Id) {
    pointer(this, event, Phase::Drag, time(event));
}

extern "C" fn mouse_up(this: Id, _: Sel, event: Id) {
    pointer(this, event, Phase::Up, time(event));
}

/// `smartMagnifyWithEvent:`, a two-finger double tap: a double tap where
/// the pointer is, which zooms as on a phone.
extern "C" fn smart_magnify(this: Id, _: Sel, event: Id) {
    for (phase, t) in double_tap(time(event)) {
        pointer(this, event, phase, t);
    }
}

/// The pointer moved over the view (no button, or one other than the
/// primary held): the mouse follows it.
extern "C" fn mouse_moved(this: Id, _: Sel, event: Id) {
    if let Some((x, y, _, scale)) = locate(this, event) {
        crate::cursor::entered(scale);
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
    crate::cursor::exited();
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
    // Typing into an editor: the Mac's input method takes the key.
    if down && editing(view).is_some() && flags & (COMMAND | CONTROL) == 0 {
        // The view's input context is made at its first use: it is the
        // current one before the input method sees the key.
        let context = send!(view, c"inputContext" => Id);
        if !context.is_null()
            && send!(class(c"NSTextInputContext"), c"currentInputContext" => Id) != context
        {
            send!(context, c"activate" => ());
        }
        let events = send!(class(c"NSArray"), c"arrayWithObject:" => Id, Id = event);
        send!(view, c"interpretKeyEvents:" => (), Id = events);
        return;
    }
    // A release goes where its press went.
    {
        let mut pressed = PRESSED.lock().unwrap();
        if down {
            if !pressed.contains(&code) {
                pressed.push(code);
            }
        } else if let Some(at) = pressed.iter().position(|&c| c == code) {
            pressed.remove(at);
        } else {
            return;
        }
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

/// `NSEventModifierFlagControl`.
const CONTROL: u64 = 1 << 18;
/// `NSNotFound`.
const NOT_FOUND: usize = isize::MAX as usize;
/// `EditorInfo.IME_MASK_ACTION`, `IME_ACTION_NONE`, `IME_FLAG_NO_ENTER_ACTION`;
/// the Android key codes of the editing commands.
const IME_MASK_ACTION: i32 = 0xff;
const IME_ACTION_NONE: i32 = 1;
const IME_FLAG_NO_ENTER_ACTION: i32 = 0x4000_0000;
const KEYCODE_ENTER: i32 = 66;
const KEYCODE_TAB: i32 = 61;
const KEYCODE_ESCAPE: i32 = 111;
const KEYCODE_DPAD_UP: i32 = 19;
const KEYCODE_DPAD_DOWN: i32 = 20;
const KEYCODE_DPAD_LEFT: i32 = 21;
const KEYCODE_DPAD_RIGHT: i32 = 22;

/// The editor this process's views type into: its task (-1: the
/// display, in device mode) and its `EditorInfo.imeOptions`.
static EDITOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);
/// The input method's marked text.
static MARKED: Mutex<String> = Mutex::new(String::new());
/// The text the input method put before the caret since the editor
/// started, the caret last moved or the view was touched: what the input
/// method reads back and replaces (Korean input composes a syllable by
/// replacing the one before the caret). The editor's other text is the
/// app's.
static TYPED: Mutex<String> = Mutex::new(String::new());
/// The keys whose press went to the keyboard device.
static PRESSED: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// The app host's editor, or its end (`text_input::Message::Editor`).
pub fn editor(m: &Text) {
    let Text::Editor {
        task,
        active,
        ime_options,
        ..
    } = *m
    else {
        return;
    };
    let mut editor = EDITOR.lock().unwrap();
    // A restart of the same task's input keeps what the input method is
    // composing; another editor, or none, ends it.
    let same = editor.is_some_and(|(t, _)| t == task);
    if active {
        *editor = Some((task, ime_options));
    } else if same {
        *editor = None;
    }
    if !(active && same) {
        MARKED.lock().unwrap().clear();
        TYPED.lock().unwrap().clear();
    }
}

/// The caret moved where the input method does not know: what it typed
/// is no longer before it.
fn forget_typed() {
    TYPED.lock().unwrap().clear();
}

/// The typed text and the marked text after it, in UTF-16 units.
fn document() -> Vec<u16> {
    let mut d: Vec<u16> = TYPED.lock().unwrap().encode_utf16().collect();
    d.extend(MARKED.lock().unwrap().encode_utf16());
    d
}

/// A replacement of the typed text from `range` on: the app's text
/// before the caret goes as far back (`deleteSurroundingText`), and so
/// does the typed text.
fn replace(range: NsRange) {
    if range.location == NOT_FOUND {
        return;
    }
    let mut typed = TYPED.lock().unwrap();
    let units: Vec<u16> = typed.encode_utf16().collect();
    let from = range.location.min(units.len());
    if from < units.len() {
        give(Text::Delete {
            before: (units.len() - from) as i32,
            after: 0,
        });
        *typed = String::from_utf16_lossy(&units[..from]);
    }
}

/// The editor's `imeOptions`, if `view` shows its task.
fn editing(view: Id) -> Option<i32> {
    let (task, options) = (*EDITOR.lock().unwrap())?;
    (crate::windows::task_of_view(view).unwrap_or(-1) == task).then_some(options)
}

/// What the input method gave: to the app host, through the server if
/// this is a window host.
fn give(m: Text) {
    if crate::shim::is_host() {
        crate::shim::text_input(&m);
    } else {
        crate::text_input::to_app_host(&m);
    }
}

/// `NSRange`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: usize,
    length: usize,
}

/// The text of an `NSString` or `NSAttributedString`.
fn string_of(s: Id) -> String {
    if send!(s, c"isKindOfClass:" => bool, Id = class(c"NSAttributedString")) {
        crate::objc::text(send!(s, c"string" => Id))
    } else {
        crate::objc::text(s)
    }
}

extern "C" fn insert_text(_: Id, _: Sel, s: Id, replacement: NsRange) {
    replace(replacement);
    let text = string_of(s);
    MARKED.lock().unwrap().clear();
    TYPED.lock().unwrap().push_str(&text);
    give(Text::Commit { text });
}

extern "C" fn set_marked_text(_: Id, _: Sel, s: Id, _: NsRange, replacement: NsRange) {
    replace(replacement);
    let text = string_of(s);
    *MARKED.lock().unwrap() = text.clone();
    // The cursor after the marked text, as the input method keeps it.
    give(Text::Compose { text, cursor: 1 });
}

/// The marked text is taken as it is.
extern "C" fn unmark_text(_: Id, _: Sel) {
    let text = std::mem::take(&mut *MARKED.lock().unwrap());
    if !text.is_empty() {
        TYPED.lock().unwrap().push_str(&text);
        give(Text::Commit { text });
    }
}

extern "C" fn has_marked_text(_: Id, _: Sel) -> bool {
    !MARKED.lock().unwrap().is_empty()
}

extern "C" fn marked_range(_: Id, _: Sel) -> NsRange {
    let length = MARKED.lock().unwrap().encode_utf16().count();
    let location = TYPED.lock().unwrap().encode_utf16().count();
    if length == 0 {
        NsRange {
            location: NOT_FOUND,
            length: 0,
        }
    } else {
        NsRange { location, length }
    }
}

/// The caret: after the typed and the marked text.
extern "C" fn selected_range(_: Id, _: Sel) -> NsRange {
    NsRange {
        location: document().len(),
        length: 0,
    }
}

/// The marked text's attributes the app shows: its underline and
/// clauses (`NSUnderlineStyleAttributeName`,
/// `NSMarkedClauseSegmentAttributeName`), as the composing span does.
extern "C" fn valid_attributes(_: Id, _: Sel) -> Id {
    let names = [
        crate::objc::nsstring("NSUnderline"),
        crate::objc::nsstring("NSMarkedClauseSegment"),
    ];
    send!(class(c"NSArray"), c"arrayWithObjects:count:" => Id, *const Id = names.as_ptr(), usize = names.len())
}

/// The part of the typed and marked text in `range`: the input method
/// reads what is before the caret back to go on composing it.
extern "C" fn attributed_substring(_: Id, _: Sel, range: NsRange, actual: *mut NsRange) -> Id {
    let marked = document();
    let start = range.location.min(marked.len());
    let end = range
        .location
        .saturating_add(range.length)
        .min(marked.len());
    if range.location == NOT_FOUND || start >= end {
        return std::ptr::null_mut();
    }
    if !actual.is_null() {
        // SAFETY: AppKit's out parameter for the range given.
        unsafe {
            *actual = NsRange {
                location: start,
                length: end - start,
            }
        };
    }
    let text = String::from_utf16_lossy(&marked[start..end]);
    let s = send!(class(c"NSAttributedString"), c"alloc" => Id);
    let s = send!(s, c"initWithString:" => Id, Id = crate::objc::nsstring(&text));
    send!(s, c"autorelease" => Id)
}

/// Where the input method shows its candidates: the window's content's
/// bottom left, on the screen (the caret's place is not reported yet,
/// #1458).
extern "C" fn first_rect(view: Id, _: Sel, _: NsRange, _: *mut NsRange) -> CGRect {
    let window = send!(view, c"window" => Id);
    if window.is_null() {
        return CGRect::default();
    }
    let bounds = send!(view, c"bounds" => CGRect);
    let in_window =
        send!(view, c"convertRect:toView:" => CGRect, CGRect = bounds, Id = std::ptr::null_mut());
    let on_screen = send!(window, c"convertRectToScreen:" => CGRect, CGRect = in_window);
    CGRect {
        x: on_screen.x,
        y: on_screen.y,
        size: crate::objc::CGSize {
            width: 0.0,
            height: 20.0,
        },
    }
}

extern "C" fn character_index(_: Id, _: Sel, _: CGPoint) -> usize {
    NOT_FOUND
}

/// The input method's editing commands, as an Android input method sends
/// them: Return as `InputMethodService.sendDefaultEditorAction` does,
/// deletions as `deleteSurroundingText`, the rest as keys.
extern "C" fn do_command(view: Id, _: Sel, command: Sel) {
    // SAFETY: a selector's NUL-terminated name.
    let name = unsafe { std::ffi::CStr::from_ptr(crate::objc::sel_getName(command)) }.to_bytes();
    let options = editing(view).unwrap_or(0);
    let m = match name {
        b"deleteBackward:" | b"deleteBackwardByDecomposingPreviousCharacter:" => {
            let mut typed = TYPED.lock().unwrap();
            let mut units: Vec<u16> = typed.encode_utf16().collect();
            units.pop();
            *typed = String::from_utf16_lossy(&units);
            Text::Delete {
                before: 1,
                after: 0,
            }
        }
        b"deleteForward:" => Text::Delete {
            before: 0,
            after: 1,
        },
        b"insertNewline:" | b"insertLineBreak:" => {
            let action = options & IME_MASK_ACTION;
            if options & IME_FLAG_NO_ENTER_ACTION == 0 && action != IME_ACTION_NONE {
                Text::Action { code: action }
            } else {
                Text::Key {
                    code: KEYCODE_ENTER,
                }
            }
        }
        b"insertTab:" => Text::Key { code: KEYCODE_TAB },
        b"cancelOperation:" => Text::Key {
            code: KEYCODE_ESCAPE,
        },
        b"moveUp:" => Text::Key {
            code: KEYCODE_DPAD_UP,
        },
        b"moveDown:" => Text::Key {
            code: KEYCODE_DPAD_DOWN,
        },
        b"moveLeft:" => Text::Key {
            code: KEYCODE_DPAD_LEFT,
        },
        b"moveRight:" => Text::Key {
            code: KEYCODE_DPAD_RIGHT,
        },
        _ => return,
    };
    if !matches!(m, Text::Delete { .. }) {
        forget_typed();
    }
    give(m);
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
    // terminate: does nothing while an app-modal alert runs (#631): end
    // its loop, and quit once it has returned.
    if !send!(app, c"modalWindow" => Id).is_null() {
        send!(app, c"abortModal" => ());
        crate::objc::on_main(|| terminate(std::ptr::null_mut()));
        return;
    }
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
                (c"smartMagnifyWithEvent:", smart_magnify),
                (c"keyDown:", key_down),
                (c"keyUp:", key_up),
                (c"flagsChanged:", flags_changed),
                (c"cursorUpdate:", crate::cursor::cursor_update),
            ],
        );
        // SAFETY: adds methods of the stated encodings, and the protocol
        // they make, to the class registered above.
        unsafe {
            let range = "{_NSRange=QQ}";
            let rect = "{CGRect={CGPoint=dd}{CGSize=dd}}";
            for (name, imp, types) in [
                (
                    c"insertText:replacementRange:",
                    insert_text as *const c_void,
                    format!("v@:@{range}"),
                ),
                (
                    c"setMarkedText:selectedRange:replacementRange:",
                    set_marked_text as *const c_void,
                    format!("v@:@{range}{range}"),
                ),
                (c"unmarkText", unmark_text as *const c_void, "v@:".into()),
                (
                    c"hasMarkedText",
                    has_marked_text as *const c_void,
                    "B@:".into(),
                ),
                (
                    c"markedRange",
                    marked_range as *const c_void,
                    format!("{range}@:"),
                ),
                (
                    c"selectedRange",
                    selected_range as *const c_void,
                    format!("{range}@:"),
                ),
                (
                    c"validAttributesForMarkedText",
                    valid_attributes as *const c_void,
                    "@@:".into(),
                ),
                (
                    c"attributedSubstringForProposedRange:actualRange:",
                    attributed_substring as *const c_void,
                    format!("@@:{range}^{range}"),
                ),
                (
                    c"firstRectForCharacterRange:actualRange:",
                    first_rect as *const c_void,
                    format!("{rect}@:{range}^{range}"),
                ),
                (
                    c"characterIndexForPoint:",
                    character_index as *const c_void,
                    "Q@:{CGPoint=dd}".into(),
                ),
                (
                    c"doCommandBySelector:",
                    do_command as *const c_void,
                    "v@::".into(),
                ),
            ] {
                let types = std::ffi::CString::new(types).unwrap();
                class_addMethod(cls, sel(name), imp, types.as_ptr());
            }
            crate::objc::class_addProtocol(
                cls,
                crate::objc::objc_getProtocol(c"NSTextInputClient".as_ptr()),
            );
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

// NSTrackingAreaOptions: entered and exited, moved, cursor updates,
// whether or not the app is active, over the view's visible rectangle as
// it changes.
const TRACKING: usize = 0x01 | 0x02 | 0x04 | 0x80 | 0x200;

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
