//! Notifications, in the display server (`docs/notifications.md`): the
//! bridge's notifications go to the posting app's window host (its shim),
//! which shows them under the app's own identity; what the user does with
//! them comes back to the bridge.
//!
//! An app without a running host gets its shim launched in the background
//! (`--notifications`: no window until the app shows one). A package
//! without a shim (the shell, system components) is shown by the system
//! shim, `android`. The server keeps the notifications shown, so a host
//! that connects (again) gets its package's at once.
//!
//! A new notification with a full-screen intent (`Post::full_screen`)
//! arriving while the Mac's screen is locked or its display asleep wakes
//! the display, and the bridge launches the intent: the app's activity is
//! in its window when the user unlocks, as on a locked phone. Otherwise
//! it is an ordinary alerting notification (macOS's time-sensitive level
//! needs an entitlement ad hoc signed shims cannot have).
//!
//! An app's request for POST_NOTIFICATIONS goes to the host of the same
//! shim, launched if need be, which asks the Mac and answers; a shim's
//! own setting goes to the bridge (#470).
//!
//! An app's indicators (the microphone, camera or location in use) go to
//! the same shim, launched if need be, as long as they are on.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_host_display::notify::{Indicator, Message, Post};

use crate::hosts::Host;

/// The package whose shim shows the notifications of packages without one.
const SYSTEM: &str = "android";
/// How long a launched shim may take to connect before it is launched
/// again.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

struct State {
    /// The bridge's connection, to write to.
    bridge: Option<UnixStream>,
    /// What is shown, by key.
    shown: BTreeMap<String, Post>,
    /// Shims launched and not yet connected.
    launching: BTreeMap<String, Instant>,
    /// Packages whose authorization the bridge asked for, until answered.
    authorizing: BTreeSet<String>,
    /// The indicators on, by package.
    indicators: BTreeSet<(String, Indicator)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    bridge: None,
    shown: BTreeMap::new(),
    launching: BTreeMap::new(),
    authorizing: BTreeSet::new(),
    indicators: BTreeSet::new(),
});

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> bool;
    fn CFRelease(obj: *const c_void);
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionDeclareUserActivity(name: *const c_void, kind: u32, id: *mut u32) -> i32;
}

const UTF8: u32 = 0x0800_0100;

/// A CFString of `s`, released by the caller.
fn cf_string(s: &std::ffi::CStr) -> *const c_void {
    // SAFETY: a NUL-terminated string.
    unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) }
}

/// Whether the Mac's screen is locked or its main display asleep.
fn mac_locked() -> bool {
    // SAFETY: plain queries; the session dictionary and key are released.
    unsafe {
        if CGDisplayIsAsleep(CGMainDisplayID()) != 0 {
            return true;
        }
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return false;
        }
        let key = cf_string(c"CGSSessionScreenIsLocked");
        let v = CFDictionaryGetValue(session, key);
        let locked = !v.is_null() && CFBooleanGetValue(v);
        CFRelease(key);
        CFRelease(session);
        locked
    }
}

/// Wakes the display as the user's activity would (it stays locked).
fn wake_display() {
    static ASSERTION: AtomicU32 = AtomicU32::new(0);
    let mut id = ASSERTION.load(Ordering::Relaxed);
    let name = cf_string(c"Android full-screen notification");
    // SAFETY: a CFString name and an assertion id we keep; kIOPMUserActiveLocal.
    unsafe {
        IOPMAssertionDeclareUserActivity(name, 0, &mut id);
        CFRelease(name);
    }
    ASSERTION.store(id, Ordering::Relaxed);
}

/// The package whose shim shows `package`'s notifications.
fn owner(package: &str) -> Option<String> {
    [package, SYSTEM]
        .into_iter()
        .find(|p| crate::apps::bundle(p).is_some())
        .map(str::to_string)
}

/// Pass `m` to the host of `owner`, launching its shim if none runs.
fn deliver(s: &mut State, owner: &str, m: &Message) {
    if let Some(h) = crate::hosts::of(owner) {
        h.send_notify(m);
        return;
    }
    if !matches!(
        m,
        Message::Post(_) | Message::Authorize { .. } | Message::Indicator { on: true, .. }
    ) {
        return;
    }
    if s.launching
        .get(owner)
        .is_some_and(|t| t.elapsed() < LAUNCH_WAIT)
    {
        return;
    }
    let Some(bundle) = crate::apps::bundle(owner) else {
        return;
    };
    s.launching.insert(owner.to_string(), Instant::now());
    // In the background and hidden; it shows its notifications once it
    // connects (`host_connected`).
    std::thread::spawn(move || {
        let launched = std::process::Command::new("/usr/bin/open")
            .args(["-g", "-j"])
            .arg(&bundle)
            .args(["--args", "--notifications"])
            .status();
        if !launched.is_ok_and(|s| s.success()) {
            eprintln!("aim-display: could not open {}", bundle.display());
        }
    });
}

/// Write `m` to the bridge.
fn to_bridge(s: &mut State, m: &Message) {
    if let Some(b) = s.bridge.as_mut()
        && let Err(e) = b.write_all(&m.frame())
    {
        eprintln!("aim-display: notification bridge: {e}");
    }
}

/// Serve the notification bridge's connection until it closes; its
/// notifications go with it.
pub fn serve_bridge(sock: UnixStream) {
    let Ok(writer) = sock.try_clone() else { return };
    STATE.lock().unwrap().bridge = Some(writer);
    let mut reader = sock;
    while let Ok(Some(m)) = Message::read(&mut reader) {
        let mut s = STATE.lock().unwrap();
        let package = match &m {
            Message::Post(p) => {
                let new = s.shown.insert(p.key.clone(), p.clone()).is_none();
                if new && p.full_screen && mac_locked() {
                    wake_display();
                    to_bridge(&mut s, &Message::FullScreen { key: p.key.clone() });
                }
                p.package.clone()
            }
            Message::Authorize { package } => {
                if owner(package).is_none() {
                    // No shim to ask: the permission stays as it is.
                    let answer = Message::Authorization {
                        package: package.clone(),
                        allowed: None,
                        answer: true,
                    };
                    to_bridge(&mut s, &answer);
                    continue;
                }
                s.authorizing.insert(package.clone());
                package.clone()
            }
            Message::Remove { key, package } => {
                s.shown.remove(key);
                package.clone()
            }
            Message::Indicator {
                package,
                indicator,
                on,
            } => {
                let entry = (package.clone(), *indicator);
                if *on {
                    s.indicators.insert(entry);
                } else {
                    s.indicators.remove(&entry);
                }
                package.clone()
            }
            _ => continue,
        };
        if let Some(owner) = owner(&package) {
            deliver(&mut s, &owner, &m);
        }
    }
    let mut s = STATE.lock().unwrap();
    s.bridge = None;
    s.authorizing.clear();
    for (package, indicator) in std::mem::take(&mut s.indicators) {
        if let Some(owner) = owner(&package) {
            let off = Message::Indicator {
                package,
                indicator,
                on: false,
            };
            deliver(&mut s, &owner, &off);
        }
    }
    let shown: Vec<Post> = std::mem::take(&mut s.shown).into_values().collect();
    for p in shown {
        if let Some(owner) = owner(&p.package) {
            deliver(
                &mut s,
                &owner,
                &Message::Remove {
                    key: p.key,
                    package: p.package,
                },
            );
        }
    }
}

/// A window host connected: it shows its package's notifications when its
/// shim stands for the package (`apps::stands_for`).
pub fn host_connected(h: &Arc<Host>) {
    if !crate::apps::stands_for(&h.package, &h.activity) {
        return;
    }
    let mut s = STATE.lock().unwrap();
    s.launching.remove(&h.package);
    let posts: Vec<Post> = s.shown.values().cloned().collect();
    for p in posts {
        if owner(&p.package).as_deref() == Some(h.package.as_str()) {
            h.send_notify(&Message::Post(p));
        }
    }
    for package in &s.authorizing {
        if owner(package).as_deref() == Some(h.package.as_str()) {
            h.send_notify(&Message::Authorize {
                package: package.clone(),
            });
        }
    }
    for (package, indicator) in &s.indicators {
        if owner(package).as_deref() == Some(h.package.as_str()) {
            h.send_notify(&Message::Indicator {
                package: package.clone(),
                indicator: *indicator,
                on: true,
            });
        }
    }
}

/// What the user did with a notification, or the Mac's setting, from
/// window host `h`.
pub fn from_host(h: &Host, m: &Message) {
    if let Message::Shown { key, shown, error } = m {
        let state = if *shown { "shown" } else { "not shown" };
        match error {
            Some(e) => eprintln!("aim-display: notification {key}: {state}: {e}"),
            None => eprintln!("aim-display: notification {key}: {state}"),
        }
        return;
    }
    let mut s = STATE.lock().unwrap();
    if let Message::Authorization {
        package, answer, ..
    } = m
    {
        if *answer {
            s.authorizing.remove(package);
        } else if *package != h.package || h.activity.is_empty() {
            // A shim mirrors its own app's setting; the system shim
            // stands for no app.
            return;
        }
    }
    to_bridge(&mut s, m);
}
