//! The lightweight shell's status bar, in the display server
//! (`docs/m1-shell.md`): what the service host's native `IStatusBar` asks
//! to show goes to the window host (the shim) that stands for the app:
//! toasts ([`crate::toast`]), the credential and pinning sheets
//! ([`crate::sheets`]) and system status icons (the app's menu bar item);
//! what the Mac showed and what the user answered comes back.
//!
//! No shim is launched for any of them. A toast or sheet of an app whose
//! shim does not run is this process's: the tasks no shim shows are its
//! windows, and in device mode it is the one Mac app. A pinning request
//! goes to the host that shows the task. An icon goes to the package's
//! shim, else the system shim, `android`, as notifications do; the server
//! keeps the icons, so a host that connects (again) gets its package's.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use aim_host_display::shell::Message;

use crate::hosts::Host;
use crate::objc::on_main;

/// The package whose shim stands for packages without one.
const SYSTEM: &str = "android";

struct State {
    /// The status bar's connection, to write to.
    bar: Option<UnixStream>,
    /// Window mode: the shims show it; device mode: this process.
    windows: bool,
    /// The icons shown, by package and slot.
    icons: BTreeMap<(String, String), Message>,
}

static STATE: Mutex<State> = Mutex::new(State {
    bar: None,
    windows: false,
    icons: BTreeMap::new(),
});

/// The package whose shim shows `package`'s.
fn owner(package: &str) -> Option<String> {
    [package, SYSTEM]
        .into_iter()
        .find(|p| crate::apps::bundle(p).is_some())
        .map(str::to_string)
}

fn to_bar(s: &mut State, m: &Message) {
    if let Some(b) = s.bar.as_mut()
        && let Err(e) = b.write_all(&m.frame())
    {
        eprintln!("aim-display: status bar: {e}");
    }
}

/// Pass `m` to `host` if there is one (in window mode), else show it here.
/// What ends a toast or sheet is handled here as well: it may have been
/// shown here before the host connected (an unknown id changes nothing).
fn deliver(s: &State, host: Option<Arc<Host>>, m: Message) {
    match host.filter(|_| s.windows) {
        Some(h) => {
            h.send_shell(&m);
            if matches!(m, Message::HideToast { .. } | Message::Dismiss { .. }) {
                on_main(move || handle(m));
            }
        }
        None => on_main(move || handle(m)),
    }
}

/// Serve the status bar's connection until it closes; in window mode
/// (`windows`) the shims show what it sends.
pub fn serve_bar(sock: UnixStream, windows: bool) {
    let Ok(writer) = sock.try_clone() else { return };
    {
        let mut s = STATE.lock().unwrap();
        s.bar = Some(writer);
        s.windows = windows;
    }
    let mut reader = sock;
    while let Ok(Some(m)) = Message::read(&mut reader) {
        let mut s = STATE.lock().unwrap();
        let host = match &m {
            Message::Toast { package, .. }
            | Message::HideToast { package, .. }
            | Message::Authenticate(aim_host_display::shell::Authenticate { package, .. })
            | Message::Retry { package, .. }
            | Message::Dismiss { package, .. } => crate::hosts::of(package),
            Message::Icon {
                package,
                slot,
                icon,
            } => {
                let key = (package.clone(), slot.clone());
                if icon.is_some() {
                    s.icons.insert(key, m.clone());
                } else {
                    s.icons.remove(&key);
                }
                // Icons are the shims' menu bar items: none without one.
                match owner(package).and_then(|o| crate::hosts::of(&o)) {
                    Some(h) if s.windows => h.send_shell(&m),
                    _ => {}
                }
                continue;
            }
            Message::Pin { task } => crate::hosts::showing(*task),
            Message::ToastShown { .. }
            | Message::ToastHidden { .. }
            | Message::Secret { .. }
            | Message::Cancel { .. }
            | Message::Pinned { .. } => continue,
        };
        deliver(&s, host, m);
    }
    let mut s = STATE.lock().unwrap();
    s.bar = None;
    let icons = std::mem::take(&mut s.icons);
    for ((package, slot), _) in icons {
        let gone = Message::Icon {
            package: package.clone(),
            slot,
            icon: None,
        };
        if let Some(h) = owner(&package).and_then(|o| crate::hosts::of(&o)) {
            h.send_shell(&gone);
        }
    }
}

/// A window host connected: it shows the icons of the packages its shim
/// stands for.
pub fn host_connected(h: &Arc<Host>) {
    if !crate::apps::stands_for(&h.package, &h.activity) {
        return;
    }
    let s = STATE.lock().unwrap();
    for ((package, _), m) in &s.icons {
        if owner(package).as_deref() == Some(h.package.as_str()) {
            h.send_shell(m);
        }
    }
}

/// What the Mac showed or the user answered, from a window host or this
/// process: to the status bar.
pub fn from_host(m: &Message) {
    if matches!(
        m,
        Message::ToastShown { .. }
            | Message::ToastHidden { .. }
            | Message::Secret { .. }
            | Message::Cancel { .. }
            | Message::Pinned { .. }
    ) {
        to_bar(&mut STATE.lock().unwrap(), m);
    }
}

/// A message for this process to show: a window host's, or the server's.
/// Main thread.
pub fn handle(m: Message) {
    match m {
        Message::Toast { id, text, .. } => crate::toast::show(id, text),
        Message::HideToast { id, .. } => crate::toast::hide(id),
        Message::Authenticate(a) => crate::sheets::authenticate(a),
        Message::Retry { id, lockout_ms, .. } => crate::sheets::retry(id, lockout_ms),
        Message::Dismiss { id, .. } => crate::sheets::dismiss(id),
        Message::Pin { task } => crate::sheets::pin(task),
        // The app's menu bar item (`status.rs`).
        Message::Icon {
            package,
            slot,
            icon,
        } => crate::system_icons::system_icon(&package, &slot, icon.as_ref()),
        Message::ToastShown { .. }
        | Message::ToastHidden { .. }
        | Message::Secret { .. }
        | Message::Cancel { .. }
        | Message::Pinned { .. } => {}
    }
}

/// What this process showed or was answered: to the server if it is a
/// window host, else to the status bar.
pub fn answer(m: &Message) {
    if crate::shim::is_host() {
        crate::shim::shell(m);
    } else {
        from_host(m);
    }
}
