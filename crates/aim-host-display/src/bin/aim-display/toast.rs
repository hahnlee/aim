//! An app's text toasts (`docs/m1-shell.md`, #498), in the process that
//! stands for the app: a translucent capsule at the bottom centre of the
//! app's window, a non-activating panel attached to it that takes no
//! clicks, as Android places a toast (`config_toastDefaultGravity`,
//! `toast_y_offset`). Without a window it shows where the app's window
//! last was, else at the bottom of the main screen.
//!
//! Android times it: it stays until the status bar hides it. One is shown
//! at a time; the next waits for the previous to fade out, as ToastUI
//! does. The status bar hears when each is on screen and when it is gone,
//! for the app's callbacks. VoiceOver reads it, as Android announces one.
//! Main thread only.

use std::cell::RefCell;

use aim_host_display::shell::Message;

use crate::objc::{CGRect, CGSize, Id, class, nsstring, on_main_after, release};

/// `NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel`.
const STYLE: usize = 1 << 7;
const BUFFERED: usize = 2;
/// `NSFloatingWindowLevel`.
const FLOATING: isize = 3;
/// `canJoinAllSpaces | transient | ignoresCycle | fullScreenAuxiliary`.
const BEHAVIOR: usize = 1 | 8 | 64 | 256;
/// `NSWindowAbove`.
const ABOVE: isize = 1;
// NSVisualEffectView's HUD material, behind-window blending, active.
const HUD_MATERIAL: isize = 13;
const BEHIND_WINDOW: isize = 0;
const ACTIVE: isize = 1;
/// `NSTextAlignmentCenter` (the iOS value on arm64).
const CENTER: isize = if cfg!(target_arch = "x86_64") { 2 } else { 1 };
/// `NSAccessibilityPriorityHigh`.
const PRIORITY_HIGH: isize = 90;
/// From the window's bottom edge to the toast's, as Android's
/// `toast_y_offset` (48dp) places it, in the Mac's points.
const OFFSET: f64 = 48.0;
/// The text's padding in the capsule, its size and widest line.
const PAD_X: f64 = 16.0;
const PAD_Y: f64 = 9.0;
const FONT: f64 = 13.0;
const MAX_WIDTH: f64 = 360.0;
/// Two lines at most, as ToastUI's text.
const LINES: isize = 2;
/// How long it fades in and out (AppKit's animator default).
const FADE_MS: u64 = 250;

struct Shown {
    id: u32,
    panel: Id,
}

#[derive(Default)]
struct State {
    shown: Option<Shown>,
    /// Fading out: the next waits.
    fading: bool,
    /// The toast that waits for the fade.
    next: Option<(u32, String)>,
    /// Where the app's window last was.
    last: Option<CGRect>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    static NSAccessibilityAnnouncementRequestedNotification: Id;
    static NSAccessibilityAnnouncementKey: Id;
    static NSAccessibilityPriorityKey: Id;
    fn NSAccessibilityPostNotificationWithUserInfo(element: Id, notification: Id, info: Id);
}

/// Show toast `id` with `text`, or after the one fading out.
pub fn show(id: u32, text: String) {
    let wait = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.fading || s.shown.is_some() {
            if let Some((skipped, _)) = s.next.replace((id, text.clone())) {
                // Replaced before it was shown.
                crate::shell::answer(&Message::ToastHidden {
                    id: skipped,
                    shown: false,
                });
            }
            true
        } else {
            false
        }
    });
    if !wait {
        present(id, &text);
    }
}

/// Hide toast `id`: fade it out, or drop it if it still waits.
pub fn hide(id: u32) {
    let panel = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.next.as_ref().is_some_and(|(n, _)| *n == id) {
            s.next = None;
            crate::shell::answer(&Message::ToastHidden { id, shown: false });
            return None;
        }
        if s.shown.as_ref().is_none_or(|t| t.id != id) {
            return None;
        }
        s.fading = true;
        s.shown.take().map(|t| t.panel)
    });
    let Some(panel) = panel else { return };
    let animator = send!(panel, c"animator" => Id);
    send!(animator, c"setAlphaValue:" => (), f64 = 0.0);
    let panel = panel as usize;
    on_main_after(FADE_MS, move || {
        let panel = panel as Id;
        let parent = send!(panel, c"parentWindow" => Id);
        if !parent.is_null() {
            send!(parent, c"removeChildWindow:" => (), Id = panel);
        }
        send!(panel, c"orderOut:" => (), Id = std::ptr::null_mut());
        release(panel);
        crate::shell::answer(&Message::ToastHidden { id, shown: true });
        let next = STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.fading = false;
            s.next.take()
        });
        if let Some((id, text)) = next {
            present(id, &text);
        }
    });
}

fn present(id: u32, text: &str) {
    let _pool = crate::objc::Pool::new();
    let label =
        send!(class(c"NSTextField"), c"wrappingLabelWithString:" => Id, Id = nsstring(text));
    let font = send!(class(c"NSFont"), c"systemFontOfSize:" => Id, f64 = FONT);
    send!(label, c"setFont:" => (), Id = font);
    send!(label, c"setAlignment:" => (), isize = CENTER);
    send!(label, c"setMaximumNumberOfLines:" => (), isize = LINES);
    let white = send!(class(c"NSColor"), c"whiteColor" => Id);
    send!(label, c"setTextColor:" => (), Id = white);
    let parent = crate::sheets::task_window(None);
    let area = if parent.is_null() {
        STATE.with(|s| s.borrow().last).unwrap_or_else(|| {
            let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
            send!(screen, c"visibleFrame" => CGRect)
        })
    } else {
        let frame = send!(parent, c"frame" => CGRect);
        STATE.with(|s| s.borrow_mut().last = Some(frame));
        frame
    };
    let widest = MAX_WIDTH.min(area.size.width * 0.8 - 2.0 * PAD_X).max(FONT);
    send!(label, c"setPreferredMaxLayoutWidth:" => (), f64 = widest);
    let fits = send!(label, c"fittingSize" => CGSize);
    let text_size = CGSize {
        width: fits.width.min(widest).ceil(),
        height: fits.height.ceil(),
    };
    let size = CGSize {
        width: text_size.width + 2.0 * PAD_X,
        height: text_size.height + 2.0 * PAD_Y,
    };
    let frame = CGRect {
        x: (area.x + (area.size.width - size.width) / 2.0).round(),
        y: area.y + OFFSET,
        size,
    };
    let panel = send!(class(c"NSPanel"), c"alloc" => Id);
    let panel = send!(panel, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = frame, usize = STYLE, usize = BUFFERED, bool = false);
    send!(panel, c"setReleasedWhenClosed:" => (), bool = false);
    send!(panel, c"setOpaque:" => (), bool = false);
    let clear = send!(class(c"NSColor"), c"clearColor" => Id);
    send!(panel, c"setBackgroundColor:" => (), Id = clear);
    send!(panel, c"setHasShadow:" => (), bool = true);
    send!(panel, c"setIgnoresMouseEvents:" => (), bool = true);
    send!(panel, c"setLevel:" => (), isize = FLOATING);
    send!(panel, c"setCollectionBehavior:" => (), usize = BEHAVIOR);
    send!(panel, c"setHidesOnDeactivate:" => (), bool = false);
    let hud = send!(class(c"NSVisualEffectView"), c"alloc" => Id);
    let hud = send!(hud, c"initWithFrame:" => Id, CGRect = CGRect {
        size,
        ..Default::default()
    });
    send!(hud, c"setMaterial:" => (), isize = HUD_MATERIAL);
    send!(hud, c"setBlendingMode:" => (), isize = BEHIND_WINDOW);
    send!(hud, c"setState:" => (), isize = ACTIVE);
    // Dark in either appearance, as Android's toast and the Mac's HUDs.
    let dark = send!(class(c"NSAppearance"), c"appearanceNamed:" => Id,
        Id = nsstring("NSAppearanceNameVibrantDark"));
    send!(panel, c"setAppearance:" => (), Id = dark);
    send!(hud, c"setWantsLayer:" => (), bool = true);
    let layer = send!(hud, c"layer" => Id);
    send!(layer, c"setCornerRadius:" => (), f64 = (size.height / 2.0).min(20.0));
    send!(layer, c"setMasksToBounds:" => (), bool = true);
    send!(label, c"setFrame:" => (), CGRect = CGRect {
        x: PAD_X,
        y: PAD_Y,
        size: text_size,
    });
    send!(hud, c"addSubview:" => (), Id = label);
    send!(panel, c"setContentView:" => (), Id = hud);
    release(hud);
    send!(panel, c"setAlphaValue:" => (), f64 = 0.0);
    if parent.is_null() {
        send!(panel, c"orderFrontRegardless" => ());
    } else {
        send!(parent, c"addChildWindow:ordered:" => (), Id = panel, isize = ABOVE);
    }
    let animator = send!(panel, c"animator" => Id);
    send!(animator, c"setAlphaValue:" => (), f64 = 1.0);
    announce(text);
    STATE.with(|s| s.borrow_mut().shown = Some(Shown { id, panel }));
    crate::shell::answer(&Message::ToastShown { id });
}

/// VoiceOver reads `text`, as Android announces a toast.
fn announce(text: &str) {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let priority = send!(class(c"NSNumber"), c"numberWithInteger:" => Id, isize = PRIORITY_HIGH);
    // SAFETY: AppKit's constant strings.
    let (keys, notification) = unsafe {
        (
            [NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
            NSAccessibilityAnnouncementRequestedNotification,
        )
    };
    let objects = [nsstring(text), priority];
    let info = send!(class(c"NSDictionary"),
        c"dictionaryWithObjects:forKeys:count:" => Id,
        *const Id = objects.as_ptr(), *const Id = keys.as_ptr(), usize = 2);
    // SAFETY: the app, an AppKit notification name and a dictionary.
    unsafe { NSAccessibilityPostNotificationWithUserInfo(app, notification, info) };
}
