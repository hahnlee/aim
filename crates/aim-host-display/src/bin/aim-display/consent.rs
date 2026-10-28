//! The Mac's consent to an app's screen capture (`docs/media.md`), in the
//! process that stands for the app (its shim; the server in device mode):
//! the question SystemUI's MediaProjection dialog asks, as a sheet on the
//! app's window, answered to the media bridge.

use std::ffi::c_void;

use aim_host_display::media::Message;

use crate::objc::{Id, class, nsstring, on_main, release};

/// `NSAlertFirstButtonReturn`: the first button, "Share screen".
const FIRST_BUTTON: isize = 1000;
/// `NSAlertStyleWarning`.
const WARNING: usize = 0;

unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A completion handler that carries the request's id and the alert it
/// releases: plain data, so the copy the API makes (`Block_copy`) needs no
/// helpers.
#[repr(C)]
struct AnswerBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*const AnswerBlock, isize),
    descriptor: &'static BlockDescriptor,
    id: u32,
    alert: Id,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: size_of::<AnswerBlock>(),
};

extern "C" fn answered(block: *const AnswerBlock, response: isize) {
    // SAFETY: the heap copy of a block `ask` built.
    let (id, alert) = unsafe { ((*block).id, (*block).alert) };
    release(alert);
    crate::media::answer(&Message::Consented {
        id,
        allowed: response == FIRST_BUTTON,
    });
}

/// The window to put the sheet on: the app's key window, else its main
/// window, else a visible one.
fn window() -> Id {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    for w in [
        send!(app, c"keyWindow" => Id),
        send!(app, c"mainWindow" => Id),
    ] {
        if !w.is_null() {
            return w;
        }
    }
    let windows = send!(app, c"windows" => Id);
    (0..send!(windows, c"count" => usize))
        .map(|i| send!(windows, c"objectAtIndex:" => Id, usize = i))
        .find(|&w| send!(w, c"isVisible" => bool))
        .unwrap_or(std::ptr::null_mut())
}

/// Ask with Android's localized title, warning and buttons; the answer goes back as
/// [`Message::Consented`] `id`.
pub fn ask(id: u32, texts: [String; 4]) {
    on_main(move || {
        let _pool = crate::objc::Pool::new();
        let alert = send!(class(c"NSAlert"), c"new" => Id);
        send!(alert, c"setAlertStyle:" => (), usize = WARNING);
        let [title, warning, share, cancel] = texts;
        send!(alert, c"setMessageText:" => (), Id = nsstring(&title));
        send!(alert, c"setInformativeText:" => (), Id = nsstring(&warning));
        send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring(&share));
        send!(alert, c"addButtonWithTitle:" => Id, Id = nsstring(&cancel));
        let window = window();
        if window.is_null() {
            // A shim started in the background has no window: an
            // app-modal alert.
            let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
            send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
            let response = send!(alert, c"runModal" => isize);
            release(alert);
            crate::media::answer(&Message::Consented {
                id,
                allowed: response == FIRST_BUTTON,
            });
        } else {
            // A stack block, which the API copies.
            let block = AnswerBlock {
                isa: (&raw const _NSConcreteStackBlock).cast(),
                flags: 0,
                reserved: 0,
                invoke: answered,
                descriptor: &DESCRIPTOR,
                id,
                alert,
            };
            send!(alert, c"beginSheetModalForWindow:completionHandler:" => (),
                Id = window, *const AnswerBlock = &block);
        }
    });
}
