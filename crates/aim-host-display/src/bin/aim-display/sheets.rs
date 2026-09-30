//! The status bar's questions, in the process that stands for the app
//! (`docs/m1-shell.md`): `BiometricPrompt`'s device credential and the
//! screen pinning request, each as a sheet on the app's window (an
//! app-modal alert while it has none), answered to the status bar. The
//! credential is verified there; a wrong one asks again, after the
//! lockout Android imposes. Main thread only.

use std::cell::RefCell;
use std::ffi::c_void;

use aim_host_display::shell::{Authenticate, Credential, Message};

use crate::objc::{CGRect, CGSize, Id, class, nsstring, on_main_after, release, text};

/// `NSAlertFirstButtonReturn`.
const FIRST_BUTTON: isize = 1000;
/// `NSAlertStyleInformational`, `NSAlertStyleWarning`.
const INFORMATIONAL: usize = 1;
const WARNING: usize = 0;
/// The credential field's size, in points.
const FIELD: CGSize = CGSize {
    width: 240.0,
    height: 22.0,
};
/// The line under it for what went wrong.
const ERROR_HEIGHT: f64 = 32.0;

unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A completion handler carrying the sheet's kind and id: plain data, so
/// the copy the API makes (`Block_copy`) needs no helpers.
#[repr(C)]
struct AnswerBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*const AnswerBlock, isize),
    descriptor: &'static BlockDescriptor,
    pin: bool,
    id: u64,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: size_of::<AnswerBlock>(),
};

/// The credential request being asked.
struct Asking {
    request: Authenticate,
    /// The alert while it is shown, and its field and error line.
    alert: Id,
    field: Id,
    error: Id,
    /// When the lockout ends (`on_main_after` ticks count it down).
    lockout_s: u32,
}

thread_local! {
    static ASKING: RefCell<Option<Asking>> = const { RefCell::new(None) };
}

extern "C" fn answered(block: *const AnswerBlock, response: isize) {
    // SAFETY: the heap copy of a block `present` built.
    let (pin, id) = unsafe { ((*block).pin, (*block).id) };
    if pin {
        crate::shell::answer(&Message::Pinned {
            task: id as i32,
            accepted: response == FIRST_BUTTON,
        });
        return;
    }
    // Only an answer to the sheet shown: one closed by the status bar is
    // no longer asked.
    let secret = ASKING.with(|a| {
        let mut a = a.borrow_mut();
        let asking = a
            .as_mut()
            .filter(|a| a.request.id == id && !a.alert.is_null())?;
        let secret = text(send!(asking.field, c"stringValue" => Id));
        release(asking.alert);
        asking.alert = std::ptr::null_mut();
        if response == FIRST_BUTTON {
            Some(Some(secret))
        } else {
            *a = None;
            Some(None)
        }
    });
    match secret {
        Some(Some(secret)) => crate::shell::answer(&Message::Secret { id, secret }),
        Some(None) => crate::shell::answer(&Message::Cancel { id, shown: true }),
        None => {}
    }
}

/// The task window to attach to: task `task`'s if this process shows it,
/// else the key one, else the frontmost visible one; only task windows,
/// never a menu bar item's or a panel.
pub fn task_window(task: Option<i32>) -> Id {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let windows: Vec<(i32, Id)> = crate::windows::window_numbers()
        .into_iter()
        .map(|(t, n)| (t, send!(app, c"windowWithWindowNumber:" => Id, isize = n)))
        .filter(|&(_, w)| !w.is_null() && send!(w, c"isVisible" => bool))
        .collect();
    let key = send!(app, c"keyWindow" => Id);
    windows
        .iter()
        .find(|&&(t, _)| Some(t) == task)
        .or_else(|| windows.iter().find(|&&(_, w)| w == key))
        .or_else(|| {
            windows
                .iter()
                .max_by_key(|&&(_, w)| -send!(w, c"orderedIndex" => isize))
        })
        .map_or(std::ptr::null_mut(), |&(_, w)| w)
}

/// Show `alert` on `window`, or app-modal without one; the answer goes to
/// [`answered`].
fn present(alert: Id, window: Id, pin: bool, id: u64) {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    if window.is_null() {
        send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
        let response = send!(alert, c"runModal" => isize);
        let block = block(pin, id);
        answered(&block, response);
    } else {
        // A stack block, which the API copies.
        let block = block(pin, id);
        send!(alert, c"beginSheetModalForWindow:completionHandler:" => (),
            Id = window, *const AnswerBlock = &block);
    }
}

fn block(pin: bool, id: u64) -> AnswerBlock {
    AnswerBlock {
        isa: (&raw const _NSConcreteStackBlock).cast(),
        flags: 0,
        reserved: 0,
        invoke: answered,
        descriptor: &DESCRIPTOR,
        pin,
        id,
    }
}

/// Ask whether to pin task `task`, as SystemUI's ScreenPinningRequest.
pub fn pin(task: i32) {
    let _pool = crate::objc::Pool::new();
    let alert = send!(class(c"NSAlert"), c"new" => Id);
    send!(alert, c"setAlertStyle:" => (), usize = WARNING);
    send!(alert, c"setMessageText:" => (), Id = nsstring("Pin this app?"));
    send!(alert, c"setInformativeText:" => (), Id = nsstring(
        "A pinned app stays in front, and other Android apps can\u{2019}t be opened \
         from it, until it unpins itself."));
    send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring("Pin"));
    send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring("No thanks"));
    present(alert, task_window(Some(task)), true, task as u32 as u64);
    release(alert);
}

/// Ask for the device credential of `request`, in place of a sheet
/// already asking.
pub fn authenticate(request: Authenticate) {
    close();
    ASKING.with(|a| {
        *a.borrow_mut() = Some(Asking {
            request,
            alert: std::ptr::null_mut(),
            field: std::ptr::null_mut(),
            error: std::ptr::null_mut(),
            lockout_s: 0,
        })
    });
    ask("");
}

/// The secret of sheet `id` was wrong: ask again, once `lockout_ms` has
/// passed.
pub fn retry(id: u64, lockout_ms: u32) {
    let asking = ASKING.with(|a| {
        let mut a = a.borrow_mut();
        let asking = a.as_mut().filter(|a| a.request.id == id)?;
        asking.lockout_s = lockout_ms.div_ceil(1000);
        Some(asking.request.credential)
    });
    let Some(credential) = asking else { return };
    let wrong = match credential {
        Credential::Pin => "Wrong PIN",
        Credential::Password => "Wrong password",
        Credential::Pattern => "Wrong pattern",
    };
    ask(wrong);
    if lockout_ms > 0 {
        tick(id);
    }
}

/// Close sheet `id`: the status bar is done with it.
pub fn dismiss(id: u64) {
    if ASKING.with(|a| a.borrow().as_ref().is_some_and(|a| a.request.id == id)) {
        close();
    }
}

/// End the sheet shown, without an answer.
fn close() {
    let Some(asking) = ASKING.with(|a| a.borrow_mut().take()) else {
        return;
    };
    if asking.alert.is_null() {
        return;
    }
    let sheet = send!(asking.alert, c"window" => Id);
    let parent = send!(sheet, c"sheetParent" => Id);
    if parent.is_null() {
        let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
        send!(app, c"abortModal" => ());
    } else {
        send!(parent, c"endSheet:" => (), Id = sheet);
    }
    release(asking.alert);
}

/// Show the credential sheet, with `error` under the field.
fn ask(error: &str) {
    let _pool = crate::objc::Pool::new();
    let Some((request, lockout_s)) = ASKING.with(|a| {
        a.borrow()
            .as_ref()
            .map(|a| (a.request.clone(), a.lockout_s))
    }) else {
        return;
    };
    let what = match request.credential {
        Credential::Pin => "PIN",
        Credential::Password => "password",
        Credential::Pattern => "pattern",
    };
    let alert = send!(class(c"NSAlert"), c"new" => Id);
    send!(alert, c"setAlertStyle:" => (), usize = INFORMATIONAL);
    let title = if request.title.is_empty() {
        format!("Enter your {what}")
    } else {
        request.title.clone()
    };
    send!(alert, c"setMessageText:" => (), Id = nsstring(&title));
    let mut info: Vec<&str> = [&request.subtitle, &request.description]
        .into_iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    if request.credential == Credential::Pattern {
        info.push("Enter your pattern as the numbers of its dots, 1 to 9, row by row.");
    }
    send!(alert, c"setInformativeText:" => (), Id = nsstring(&info.join("\n\n")));
    send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring("OK"));
    send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring("Cancel"));
    let accessory = send!(class(c"NSView"), c"alloc" => Id);
    let accessory = send!(accessory, c"initWithFrame:" => Id, CGRect = CGRect {
        size: CGSize {
            width: FIELD.width,
            height: FIELD.height + ERROR_HEIGHT,
        },
        ..Default::default()
    });
    let field = send!(class(c"NSSecureTextField"), c"alloc" => Id);
    let field = send!(field, c"initWithFrame:" => Id, CGRect = CGRect {
        y: ERROR_HEIGHT,
        size: FIELD,
        ..Default::default()
    });
    send!(field, c"setPlaceholderString:" => (), Id = nsstring(&format!("Android {what}")));
    send!(accessory, c"addSubview:" => (), Id = field);
    release(field);
    let label =
        send!(class(c"NSTextField"), c"wrappingLabelWithString:" => Id, Id = nsstring(error));
    send!(label, c"setFrame:" => (), CGRect = CGRect {
        size: CGSize {
            width: FIELD.width,
            height: ERROR_HEIGHT - 4.0,
        },
        ..Default::default()
    });
    let red = send!(class(c"NSColor"), c"systemRedColor" => Id);
    send!(label, c"setTextColor:" => (), Id = red);
    send!(accessory, c"addSubview:" => (), Id = label);
    send!(alert, c"setAccessoryView:" => (), Id = accessory);
    release(accessory);
    send!(alert, c"layout" => ());
    let sheet = send!(alert, c"window" => Id);
    send!(sheet, c"setInitialFirstResponder:" => (), Id = field);
    ASKING.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            a.alert = alert;
            a.field = field;
            a.error = label;
        }
    });
    if lockout_s > 0 {
        lock(alert, field, true);
    }
    present(alert, task_window(None), false, request.id);
}

/// While locked out, the field and OK take nothing.
fn lock(alert: Id, field: Id, locked: bool) {
    send!(field, c"setEnabled:" => (), bool = !locked);
    let buttons = send!(alert, c"buttons" => Id);
    let ok = send!(buttons, c"objectAtIndex:" => Id, usize = 0);
    send!(ok, c"setEnabled:" => (), bool = !locked);
}

/// Count sheet `id`'s lockout down, a second at a time, as SystemUI's
/// credential view does.
fn tick(id: u64) {
    let state = ASKING.with(|a| {
        let mut a = a.borrow_mut();
        let asking = a
            .as_mut()
            .filter(|a| a.request.id == id && !a.alert.is_null())?;
        let left = asking.lockout_s;
        asking.lockout_s = left.saturating_sub(1);
        Some((asking.alert, asking.field, asking.error, left))
    });
    let Some((alert, field, error, left)) = state else {
        return;
    };
    if left == 0 {
        lock(alert, field, false);
        send!(error, c"setStringValue:" => (), Id = nsstring(""));
        let sheet = send!(alert, c"window" => Id);
        send!(sheet, c"makeFirstResponder:" => bool, Id = field);
        return;
    }
    let _pool = crate::objc::Pool::new();
    let message = format!("Too many attempts. Try again in {left} seconds.");
    send!(error, c"setStringValue:" => (), Id = nsstring(&message));
    on_main_after(1000, move || tick(id));
}
