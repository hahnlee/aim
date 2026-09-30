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

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_host_display::notify::{Message, Post};

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
}

static STATE: Mutex<State> = Mutex::new(State {
    bridge: None,
    shown: BTreeMap::new(),
    launching: BTreeMap::new(),
});

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
    if !matches!(m, Message::Post(_)) {
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
                s.shown.insert(p.key.clone(), p.clone());
                p.package.clone()
            }
            Message::Remove { key, package } => {
                s.shown.remove(key);
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
}

/// What the user did with a notification, from a window host.
pub fn from_host(m: &Message) {
    use std::io::Write;
    if let Message::Shown { key, shown, error } = m {
        let state = if *shown { "shown" } else { "not shown" };
        match error {
            Some(e) => eprintln!("aim-display: notification {key}: {state}: {e}"),
            None => eprintln!("aim-display: notification {key}: {state}"),
        }
        return;
    }
    let mut s = STATE.lock().unwrap();
    if let Some(b) = s.bridge.as_mut()
        && let Err(e) = b.write_all(&m.frame())
    {
        eprintln!("aim-display: notification bridge: {e}");
    }
}
