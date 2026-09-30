//! Media, in the display server (`docs/media.md`): the media bridge's
//! Now Playing goes to the window host (the shim) of the session's app,
//! which publishes it as the Mac's Now Playing under the app's identity;
//! the Mac's transport commands and media keys reach that host and come
//! back to the bridge. As with notifications, a package without a shim
//! goes to the system shim, `android`, and an app whose shim is not
//! running (a player whose window was closed) gets it launched in the
//! background.
//!
//! An app's request to capture the screen is asked on the app's window:
//! in its shim, or in the server when the server shows the app's task (no
//! shim of the app runs). In device mode the server is the one Mac app:
//! it shows Now Playing and asks itself.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_host_display::media::{Message, NowPlaying};

use crate::hosts::Host;

/// The package whose shim stands for packages without one.
const SYSTEM: &str = "android";
/// How long a launched shim may take to connect before it is launched
/// again.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

struct State {
    /// The bridge's connection, to write to.
    bridge: Option<UnixStream>,
    /// Window mode: the shims show media; device mode: this process.
    windows: bool,
    /// What Now Playing shows.
    playing: Option<NowPlaying>,
    /// The package whose shim shows it (window mode).
    shown_by: Option<String>,
    /// Consent requests not yet answered: their packages, by id.
    consents: BTreeMap<u32, String>,
    /// Shims launched and not yet connected.
    launching: BTreeMap<String, Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    bridge: None,
    windows: false,
    playing: None,
    shown_by: None,
    consents: BTreeMap::new(),
    launching: BTreeMap::new(),
});

/// The package whose shim shows `package`'s media.
fn owner(package: &str) -> Option<String> {
    [package, SYSTEM]
        .into_iter()
        .find(|p| crate::apps::bundle(p).is_some())
        .map(str::to_string)
}

/// Write `m` to the bridge.
fn to_bridge(s: &mut State, m: &Message) {
    if let Some(b) = s.bridge.as_mut()
        && let Err(e) = b.write_all(&m.frame())
    {
        eprintln!("aim-display: media bridge: {e}");
    }
}

/// Pass `m`, about `package`, to whoever shows it: this process in device
/// mode; else a consent request to the app's own host, if it runs, and
/// the rest to the host of the package's shim, launched if none runs.
fn deliver(s: &mut State, package: &str, m: &Message) {
    if !s.windows {
        handle(m.clone());
        return;
    }
    if let Message::Consent { id, .. } = m {
        match crate::hosts::of(package) {
            Some(h) => h.send_media(m),
            // The server's window shows the app's task.
            None => handle(m.clone()),
        }
        eprintln!("aim-display: consent {id} of {package} asked");
        return;
    }
    let Some(owner) = owner(package) else {
        return;
    };
    if let Some(h) = crate::hosts::of(&owner) {
        h.send_media(m);
        return;
    }
    if s.launching
        .get(&owner)
        .is_some_and(|t| t.elapsed() < LAUNCH_WAIT)
    {
        return;
    }
    let Some(bundle) = crate::apps::bundle(&owner) else {
        return;
    };
    s.launching.insert(owner, Instant::now());
    // In the background and hidden; it gets the message once it connects
    // (`host_connected`).
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

/// Now Playing no longer shows `package`'s session: the shim that shows
/// it clears it.
fn gone(s: &mut State, package: &str) {
    let m = Message::Gone {
        package: package.to_string(),
    };
    if !s.windows {
        handle(m);
    } else if let Some(h) = s.shown_by.take().and_then(|o| crate::hosts::of(&o)) {
        h.send_media(&m);
    }
}

/// Serve the media bridge's connection until it closes; in window mode
/// (`windows`) the shims show what it sends.
pub fn serve_bridge(sock: UnixStream, windows: bool) {
    let Ok(writer) = sock.try_clone() else { return };
    {
        let mut s = STATE.lock().unwrap();
        s.bridge = Some(writer);
        s.windows = windows;
    }
    let mut reader = sock;
    while let Ok(Some(m)) = Message::read(&mut reader) {
        let mut s = STATE.lock().unwrap();
        match &m {
            Message::NowPlaying(n) => {
                let by = owner(&n.package);
                let old = s.playing.take();
                if s.shown_by.is_some()
                    && s.shown_by != by
                    && let Some(old) = &old
                {
                    gone(&mut s, &old.package);
                }
                if old.is_none_or(|o| (&o.package, o.state) != (&n.package, n.state)) {
                    eprintln!("aim-display: now playing {} ({:?})", n.package, n.state);
                }
                s.shown_by = by;
                s.playing = Some(n.clone());
                deliver(&mut s, &n.package, &m);
            }
            Message::Gone { package } => {
                if s.playing.as_ref().is_some_and(|n| n.package == *package) {
                    s.playing = None;
                    eprintln!("aim-display: now playing nothing");
                    gone(&mut s, package);
                }
            }
            Message::Consent { id, package, .. } => {
                s.consents.insert(*id, package.clone());
                deliver(&mut s, package, &m);
            }
            Message::Command { .. } | Message::Consented { .. } => {}
        }
    }
    let mut s = STATE.lock().unwrap();
    s.bridge = None;
    s.consents.clear();
    if let Some(n) = s.playing.take() {
        gone(&mut s, &n.package);
    }
}

/// A window host connected: it shows Now Playing of the packages its
/// shim stands for, taking it from the system shim if that showed it
/// while the app had no shim.
pub fn host_connected(h: &Arc<Host>) {
    if !crate::apps::stands_for(&h.package, &h.activity) {
        return;
    }
    let mut s = STATE.lock().unwrap();
    s.launching.remove(&h.package);
    let Some(n) = s
        .playing
        .clone()
        .filter(|n| owner(&n.package).as_deref() == Some(h.package.as_str()))
    else {
        return;
    };
    if s.shown_by.as_deref() != Some(h.package.as_str()) {
        gone(&mut s, &n.package);
        s.shown_by = Some(h.package.clone());
    }
    h.send_media(&Message::NowPlaying(n));
}

/// What the user did, from window host `h`, or from this process in
/// device mode (`None`).
pub fn from_host(h: Option<&Host>, m: &Message) {
    let mut s = STATE.lock().unwrap();
    let known = match m {
        // Only the host that shows the session commands it.
        Message::Command { package, .. } => {
            s.playing.as_ref().is_some_and(|n| n.package == *package)
                && h.is_none_or(|h| s.shown_by.as_deref() == Some(h.package.as_str()))
        }
        // Only the app's own host answers for it.
        Message::Consented { id, .. } => s
            .consents
            .remove(id)
            .is_some_and(|package| h.is_none_or(|h| h.package == package)),
        Message::NowPlaying(_) | Message::Gone { .. } | Message::Consent { .. } => false,
    };
    if let Message::Consented { id, allowed } = m {
        eprintln!("aim-display: consent {id}: allowed {allowed}, known {known}");
    }
    if known {
        to_bridge(&mut s, m);
    }
}

/// A message for this process's own Now Playing and sheets: a window
/// host's, or the server's in device mode.
pub fn handle(m: Message) {
    match m {
        Message::NowPlaying(n) => crate::nowplaying::show(n),
        Message::Gone { package } => crate::nowplaying::clear(package),
        Message::Consent { id, label, .. } => crate::consent::ask(id, label),
        Message::Command { .. } | Message::Consented { .. } => {}
    }
}

/// The user's command or answer, from this process: to the server if it
/// is a window host, else to the bridge.
pub fn answer(m: &Message) {
    if crate::shim::is_host() {
        crate::shim::media(m);
    } else {
        from_host(None, m);
    }
}
