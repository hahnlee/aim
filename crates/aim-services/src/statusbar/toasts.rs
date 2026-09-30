//! Text toasts (#498), as SystemUI's ToastUI and the framework's
//! ToastPresenter handle them: one on screen at a time, a new one hiding
//! the one shown; `hideToast` only for the one shown. When the Mac has it
//! on screen, the toast is announced to accessibility
//! (`TYPE_NOTIFICATION_STATE_CHANGED`, as `trySendAccessibilityEvent`) and
//! the app's callback hears `onToastShown`; when the Mac has closed it,
//! NotificationManagerService gets its token back (`finishToken`) and the
//! callback hears `onToastHidden`.

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;
use aim_host_display::shell::Message;
use aim_service_aidl::{
    WriteParcelable, android_app_inotificationmanager as nm,
    android_app_itransientnotificationcallback as callback,
    android_view_accessibility_iaccessibilitymanager as a11y,
    com_android_internal_statusbar_istatusbar as bar,
};

use super::StatusBar;
use super::parcels::Text;
use crate::clip::write_char_sequence;

/// `AccessibilityEvent.TYPE_NOTIFICATION_STATE_CHANGED`.
const NOTIFICATION_STATE_CHANGED: i32 = 1 << 6;
/// The class a toast's event names (`Toast.class.getName()`).
const TOAST_CLASS: &str = "android.widget.Toast";
/// `Parcel.VAL_STRING`.
const VAL_STRING: i32 = 0;

/// A toast sent to the Mac, until the Mac has closed it.
pub struct Sent {
    pub package: String,
    user: i32,
    text: String,
    token: Option<Strong>,
    callback: Option<Strong>,
}

/// The `AccessibilityEvent` ToastPresenter sends: the toast's text, from
/// its package, of no window or node.
struct ToastEvent<'a> {
    package: &'a str,
    text: &'a str,
    time_ms: i64,
}

impl WriteParcelable for ToastEvent<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0); // sealed
        p.write_i32(NOTIFICATION_STATE_CHANGED);
        for _ in 0..5 {
            p.write_i32(0); // granularity, action, content/window/speech changes
        }
        write_char_sequence(p, Some(self.package));
        p.write_i64(self.time_ms);
        p.write_i32(-1); // connection
        // The record: no properties, indices undefined, no scroll.
        p.write_i32(0);
        for v in [-1, -1, -1, -1, 0, 0, -1, -1, 0, 0, -1, -1] {
            p.write_i32(v);
        }
        write_char_sequence(p, Some(TOAST_CLASS));
        write_char_sequence(p, None); // content description
        write_char_sequence(p, None); // before text
        p.write_string16(None); // parcelable data
        p.write_i32(1); // the text, one CharSequence
        p.write_i32(VAL_STRING);
        p.write_string16(Some(self.text));
        p.write_i32(-1); // AccessibilityWindowInfo.UNDEFINED_WINDOW_ID
        p.write_i64(0x7fff_ffff_7fff_ffff); // AccessibilityNodeInfo.UNDEFINED_NODE_ID
        p.write_i32(-1); // Display.INVALID_DISPLAY
        p.write_i32(0); // sealed
        p.write_i32(0); // no more records
    }
}

impl StatusBar {
    /// `showToast`: a toast on screen is hidden first, and this one shown
    /// after it.
    pub(super) fn show_toast(&self, args: bar::ShowToast<Text>) {
        let package = args.package_name.unwrap_or_default();
        let text = args.text.and_then(|t| t.0).unwrap_or_default();
        let (id, previous) = {
            let mut state = self.state.lock().unwrap();
            let id = state.next_toast;
            state.next_toast = id.wrapping_add(1).max(1);
            state.toasts.insert(
                id,
                Sent {
                    package: package.clone(),
                    // UserHandle.getUserId.
                    user: args.uid / 100_000,
                    text: text.clone(),
                    token: self.strong(args.token),
                    callback: self.strong(args.callback),
                },
            );
            let previous = state.toast.replace(id).and_then(|p| {
                let package = state.toasts.get(&p)?.package.clone();
                Some((p, package))
            });
            (id, previous)
        };
        if let Some((id, package)) = previous {
            self.send(&Message::HideToast { id, package });
        }
        self.send(&Message::Toast { id, package, text });
    }

    /// `hideToast`: only of the toast on screen.
    pub(super) fn hide_toast(&self, args: bar::HideToast) {
        let package = args.package_name.unwrap_or_default();
        let hidden = {
            let mut state = self.state.lock().unwrap();
            let shown = state.toast.filter(|id| {
                state.toasts.get(id).is_some_and(|t| {
                    t.package == package && t.token.as_ref().map(Strong::binder) == args.token
                })
            });
            if shown.is_some() {
                state.toast = None;
            }
            shown
        };
        match hidden {
            Some(id) => self.send(&Message::HideToast { id, package }),
            None => eprintln!("guest-init: statusbar: {package}: not the toast shown"),
        }
    }

    pub(super) fn toast_shown(&self, id: u32) {
        let shown = {
            let state = self.state.lock().unwrap();
            state.toasts.get(&id).map(|t| {
                (
                    t.package.clone(),
                    t.user,
                    t.text.clone(),
                    t.callback.as_ref().map(Strong::binder),
                )
            })
        };
        let Some((package, user, text, callback)) = shown else {
            return;
        };
        if let Some(service) = self.find("accessibility") {
            let mut data = Parcel::new();
            a11y::SendAccessibilityEvent {
                ui_event: Some(ToastEvent {
                    package: &package,
                    text: &text,
                    // SystemClock.uptimeMillis.
                    time_ms: aim_host_display::monotonic_ns() / 1_000_000,
                }),
                user_id: user,
            }
            .write(&mut data);
            if let Err(s) = service.transact(a11y::SEND_ACCESSIBILITY_EVENT, &data, true) {
                eprintln!("guest-init: statusbar: accessibility event: status {s}");
            }
        }
        if let Some(c) = callback.and_then(|b| self.strong(Some(b))) {
            self.tell(&c, callback::ON_TOAST_SHOWN, |p| {
                callback::OnToastShown {}.write(p)
            });
        }
    }

    /// The Mac closed toast `id`; `shown` if it had been on screen.
    pub(super) fn toast_hidden(&self, id: u32, shown: bool) {
        let Some(t) = self.state.lock().unwrap().toasts.remove(&id) else {
            return;
        };
        if !shown {
            return;
        }
        let token = t.token.as_ref().map(Strong::binder);
        if let Err(e) = self.call(
            "notification",
            nm::FINISH_TOKEN,
            |p| {
                nm::FinishToken {
                    pkg: Some(t.package.clone()),
                    token,
                }
                .write(p)
            },
            nm::read_finish_token_reply,
        ) {
            eprintln!("guest-init: statusbar: finishToken: {e}");
        }
        if let Some(c) = &t.callback {
            self.tell(c, callback::ON_TOAST_HIDDEN, |p| {
                callback::OnToastHidden {}.write(p)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;

    #[test]
    fn writes_the_event_toast_presenter_sends() {
        let mut p = Parcel::new();
        ToastEvent {
            package: "com.example",
            text: "Saved",
            time_ms: 42,
        }
        .write_to(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), NOTIFICATION_STATE_CHANGED);
        for _ in 0..5 {
            r.read_i32().unwrap();
        }
        assert_eq!(
            crate::clip::char_sequence(&mut r).unwrap().as_deref(),
            Some("com.example")
        );
        assert_eq!(r.read_i64().unwrap(), 42);
        r.read_i32().unwrap();
        for _ in 0..13 {
            r.read_i32().unwrap();
        }
        assert_eq!(
            crate::clip::char_sequence(&mut r).unwrap().as_deref(),
            Some(TOAST_CLASS)
        );
        assert_eq!(crate::clip::char_sequence(&mut r).unwrap(), None);
        assert_eq!(crate::clip::char_sequence(&mut r).unwrap(), None);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i32().unwrap(), VAL_STRING);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("Saved"));
        assert_eq!(r.read_i32().unwrap(), -1);
        r.read_i64().unwrap();
        assert_eq!(r.read_i32().unwrap(), -1);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
    }
}
