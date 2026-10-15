//! The media bridge (`docs/media.md`): the media session the Mac's media
//! keys control as the Mac's Now Playing, and the Mac's consent to an
//! app's screen capture, while the original MediaSessionService and
//! MediaProjectionManagerService keep running.
//!
//! **Now Playing.** A native `IOnMediaKeyEventSessionChangedListener` of
//! the system uid registers with the original MediaSessionService
//! (`ISessionManager.addOnMediaKeyEventSessionChangedListener`, a
//! `MediaSessionManager` listener) and follows the session media keys go
//! to: the one that played last (`MediaSessionStack.getMediaButtonSession`).
//! Through the session's `ISessionController` it reads the metadata and
//! playback state, and hears of their changes as a controller does
//! (`registerCallback`). It sends them ([`aim_host_display::media`]) to the
//! display server, which passes them to the session's app's shim; the
//! Mac's transport commands (its media keys, the menu bar's and Control
//! Center's Now Playing) come back as the controller's transport controls.
//!
//! **Screen capture consent** ([`consent`]): the activity that
//! `config_mediaProjectionPermissionDialogComponent` names in place of
//! SystemUI's asks the bridge, which asks the Mac on the app's window and
//! creates the projection as SystemUI does.
//!
//! In window mode the shims show both; in device mode the display server
//! itself.

mod consent;
mod parcels;

use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_driver::{Credentials, Device, Driver};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, UNKNOWN_TRANSACTION};
use aim_host_display::media::{self, Command, Message, NowPlaying, State};
use aim_host_display::wire;
use aim_service_aidl::{
    Returned, android_media_session_ionmediakeyeventsessionchangedlistener as listener,
    android_media_session_isessioncontroller as controller,
    android_media_session_isessioncontrollercallback as callback,
    android_media_session_isessionmanager as sm_media, android_os_iservicemanager as sm,
};

use crate::notifications::ProcessFiles;
use parcels::{
    ACTION_PAUSE, ACTION_PLAY, ACTION_PLAY_PAUSE, ACTION_SEEK_TO, ACTION_SKIP_TO_NEXT,
    ACTION_SKIP_TO_PREVIOUS, ACTION_STOP, Metadata, PlaybackState, STATE_ERROR, STATE_NONE,
    STATE_PAUSED, STATE_STOPPED, Token,
};

/// `Process.SYSTEM_UID` and system_server's context, as the other native
/// services.
const SYSTEM_UID: u32 = 1000;
const SYSTEM_SERVER_CONTEXT: &str = "u:r:system_server:s0";
/// The package the bridge calls as: the system uid's.
const PACKAGE: &str = "android";
/// How often the bridge looks for MediaSessionService while it is not
/// there.
const RETRY: Duration = Duration::from_secs(1);

/// The session followed.
struct Session {
    controller: Arc<Strong>,
    package: String,
    metadata: Option<Metadata>,
    playback: Option<PlaybackState>,
}

struct Sessions {
    /// MediaSessionService, while registered with it.
    manager: Option<Arc<Strong>>,
    session: Option<Session>,
}

pub struct Bridge {
    process: Arc<LocalProcess>,
    listener: Binder,
    callback: Binder,
    link: Mutex<UnixStream>,
    sessions: Mutex<Sessions>,
    /// Serializes changes of the session followed.
    following: Mutex<()>,
    /// Signalled when MediaSessionService dies.
    gone: Condvar,
    consents: consent::Consents,
}

/// The listener and controller callback nodes.
struct Listener(Weak<Bridge>);
struct Callback(Weak<Bridge>);

impl Bridge {
    /// Starts the bridge in the background: it connects to the display
    /// server at `display`, serves the consent activity, and follows
    /// MediaSessionService once it is published (again after
    /// system_server restarts).
    pub fn start(driver: &Arc<Driver>, display: &Path) {
        Self::start_for_namespace(driver,display,std::process::id() as i32)
    }
    pub fn start_for_namespace(driver:&Arc<Driver>,display:&Path,guest_pid:i32){
        let driver = driver.clone();
        let display = display.to_owned();
        let _ = std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                if let Err(e) = Bridge::run(&driver, &display,guest_pid) {
                    eprintln!("guest-init: media: {e}");
                }
            });
    }

    fn run(driver: &Arc<Driver>, display: &Path,guest_pid:i32) -> Result<(), String> {
        let link = connect(display)?;
        let process = LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid: guest_pid,
                euid: SYSTEM_UID,
                security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
            },
        );
        let reader = link.try_clone().map_err(|e| e.to_string())?;
        let bridge = Arc::new_cyclic(|this: &Weak<Bridge>| Bridge {
            listener: process.add_service(Arc::new(Listener(this.clone()))),
            callback: process.add_service(Arc::new(Callback(this.clone()))),
            consents: consent::Consents::new(&process, this.clone()),
            process: process.clone(),
            link: Mutex::new(link),
            sessions: Mutex::new(Sessions {
                manager: None,
                session: None,
            }),
            following: Mutex::new(()),
            gone: Condvar::new(),
        });
        process.start();
        let this = bridge.clone();
        std::thread::Builder::new()
            .name("media-mac".into())
            .spawn(move || this.serve_mac(reader))
            .map_err(|e| e.to_string())?;
        bridge.consents.register();
        bridge.follow_manager();
        Ok(())
    }

    /// Registers with MediaSessionService whenever it is published, until
    /// the process ends.
    fn follow_manager(self: &Arc<Self>) {
        loop {
            let Some(manager) = self.find("media_session") else {
                std::thread::sleep(RETRY);
                continue;
            };
            let manager = Arc::new(manager);
            let this = Arc::downgrade(self);
            self.process.link_to_death(
                &manager,
                Box::new(move || {
                    if let Some(b) = this.upgrade() {
                        b.manager_died();
                    }
                }),
            );
            let registered = call(
                &manager,
                sm_media::ADD_ON_MEDIA_KEY_EVENT_SESSION_CHANGED_LISTENER,
                |p| {
                    sm_media::AddOnMediaKeyEventSessionChangedListener {
                        listener: Some(self.listener),
                        package_name: Some(PACKAGE.into()),
                    }
                    .write(p)
                },
                sm_media::read_add_on_media_key_event_session_changed_listener_reply,
            );
            if let Err(e) = registered {
                eprintln!("guest-init: media: addOnMediaKeyEventSessionChangedListener: {e}");
                std::thread::sleep(RETRY);
                continue;
            }
            self.sessions.lock().unwrap().manager = Some(manager);
            // The listener hears of changes only.
            self.refresh_session();
            let mut sessions = self.sessions.lock().unwrap();
            while sessions.manager.is_some() {
                sessions = self.gone.wait(sessions).unwrap();
            }
        }
    }

    /// MediaSessionService died with system_server: its sessions are gone.
    fn manager_died(&self) {
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.manager = None;
            sessions.session.take()
        };
        if let Some(s) = session {
            self.send(&Message::Gone { package: s.package });
        }
        self.gone.notify_all();
    }

    /// `name` from servicemanager.
    fn find(&self, name: &str) -> Option<Strong> {
        let mut data = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::CHECK_SERVICE, &data, false)
            .ok()?;
        let Ok(Ok(Some(Binder::Handle(h)))) = sm::read_check_service_reply(&mut reply.reader())
        else {
            return None;
        };
        Some(self.process.strong(h))
    }

    fn send(&self, m: &Message) {
        let mut link = self.link.lock().unwrap();
        if let Err(e) = link.write_all(&m.frame()) {
            eprintln!("guest-init: media: display server: {e}");
        }
    }

    /// Follows the session media keys go to now.
    fn refresh_session(&self) {
        let Some(manager) = self.sessions.lock().unwrap().manager.clone() else {
            return;
        };
        let token = call(
            &manager,
            sm_media::GET_MEDIA_KEY_EVENT_SESSION,
            |p| {
                sm_media::GetMediaKeyEventSession {
                    package_name: Some(PACKAGE.into()),
                }
                .write(p)
            },
            // The controller's reference, taken while the reply holds it.
            |r| {
                Ok(
                    sm_media::read_get_media_key_event_session_reply::<Token>(r)?
                        .map(|t| t.and_then(|t| hold(&self.process, t.controller))),
                )
            },
        );
        match token {
            Ok(controller) => self.follow(controller),
            Err(e) => eprintln!("guest-init: media: getMediaKeyEventSession: {e}"),
        }
    }

    /// Follows the session of `controller` (none: no session has the media
    /// keys), leaving the one followed.
    fn follow(&self, controller: Option<Strong>) {
        let _following = self.following.lock().unwrap();
        let old = {
            let mut sessions = self.sessions.lock().unwrap();
            let same = sessions
                .session
                .as_ref()
                .zip(controller.as_ref())
                .is_some_and(|(s, c)| s.controller.binder() == c.binder());
            if same {
                return;
            }
            sessions.session.take()
        };
        if let Some(old) = old {
            let _ = call(
                &old.controller,
                controller::UNREGISTER_CALLBACK,
                |p| {
                    controller::UnregisterCallback {
                        cb: Some(self.callback),
                    }
                    .write(p)
                },
                controller::read_unregister_callback_reply,
            );
            self.send(&Message::Gone {
                package: old.package,
            });
        }
        let Some(session) = controller else { return };
        let registered = call(
            &session,
            controller::REGISTER_CALLBACK,
            |p| {
                controller::RegisterCallback {
                    package_name: Some(PACKAGE.into()),
                    cb: Some(self.callback),
                }
                .write(p)
            },
            controller::read_register_callback_reply,
        );
        let package = call(
            &session,
            controller::GET_PACKAGE_NAME,
            |p| controller::GetPackageName {}.write(p),
            controller::read_get_package_name_reply,
        );
        let package = match (registered, package) {
            (Ok(()), Ok(Some(package))) => package,
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("guest-init: media: session: {e}");
                return;
            }
            (Ok(()), Ok(None)) => return,
        };
        let metadata = self.metadata(&session);
        let playback = self.playback(&session);
        self.sessions.lock().unwrap().session = Some(Session {
            controller: Arc::new(session),
            package,
            metadata,
            playback,
        });
        self.publish();
    }

    fn metadata(&self, session: &Strong) -> Option<Metadata> {
        let mut data = Parcel::new();
        controller::GetMetadata {}.write(&mut data);
        let reply = session
            .transact(controller::GET_METADATA, &data, false)
            .ok()?;
        let mut r = reply.reader();
        if r.read_exception().ok()?.is_err() || r.read_i32().ok()? == 0 {
            return None;
        }
        Metadata::read(&mut r, &ProcessFiles(&self.process))
            .inspect_err(|s| eprintln!("guest-init: media: metadata: status {s}"))
            .ok()
    }

    fn playback(&self, session: &Strong) -> Option<PlaybackState> {
        call(
            session,
            controller::GET_PLAYBACK_STATE,
            |p| controller::GetPlaybackState {}.write(p),
            controller::read_get_playback_state_reply::<PlaybackState>,
        )
        .inspect_err(|e| eprintln!("guest-init: media: playback state: {e}"))
        .ok()
        .flatten()
    }

    /// The session's metadata or playback state changed (`metadata`: which
    /// one).
    fn changed(&self, metadata: bool) {
        let Some(strong) = self
            .sessions
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(|s| s.controller.clone())
        else {
            return;
        };
        let session = strong.binder();
        let (m, p) = if metadata {
            (Some(self.metadata(&strong)), None)
        } else {
            (None, Some(self.playback(&strong)))
        };
        {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions
                .session
                .as_mut()
                .filter(|s| s.controller.binder() == session)
            else {
                return;
            };
            if let Some(m) = m {
                s.metadata = m;
            }
            if let Some(p) = p {
                s.playback = p;
            }
        }
        self.publish();
    }

    /// Sends what Now Playing shows of the session followed.
    fn publish(&self) {
        let now = aim_hostcall::clock::boottime_ns() / 1_000_000;
        let sessions = self.sessions.lock().unwrap();
        let Some(s) = &sessions.session else { return };
        let n = now_playing(&s.package, s.metadata.as_ref(), s.playback.as_ref(), now);
        drop(sessions);
        self.send(&Message::NowPlaying(n));
    }

    /// The display server's messages: the user's commands and answers.
    fn serve_mac(self: Arc<Self>, mut reader: UnixStream) {
        loop {
            match Message::read(&mut reader) {
                Ok(Some(Message::Command { package, command })) => {
                    if let Err(e) = self.command(&package, command) {
                        eprintln!("guest-init: media: {package}: {command:?}: {e}");
                    }
                }
                Ok(Some(Message::Consented { id, allowed })) => self.consents.answered(id, allowed),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    eprintln!("guest-init: media: display server: {e}");
                    break;
                }
            }
        }
        // Nobody is left to answer.
        self.consents.cancel_all();
    }

    /// `command` for `package`'s session, as its transport controls.
    fn command(&self, package: &str, command: Command) -> Result<(), String> {
        let (session, playing) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(s) = sessions.session.as_ref().filter(|s| s.package == package) else {
                return Ok(());
            };
            let playing = s.playback.is_some_and(|p| p.advancing());
            (s.controller.clone(), playing)
        };
        let caller = Some(PACKAGE.to_string());
        let simple = |code: u32| {
            call(
                &session,
                code,
                |p| {
                    p.write_interface_token(controller::DESCRIPTOR);
                    p.write_string16(Some(PACKAGE));
                },
                |r| r.read_exception(),
            )
        };
        match command {
            Command::Play => simple(controller::PLAY),
            Command::Pause => simple(controller::PAUSE),
            Command::TogglePlayPause if playing => simple(controller::PAUSE),
            Command::TogglePlayPause => simple(controller::PLAY),
            Command::Stop => simple(controller::STOP),
            Command::Next => simple(controller::NEXT),
            Command::Previous => simple(controller::PREVIOUS),
            Command::Seek(ms) => call(
                &session,
                controller::SEEK_TO,
                |p| {
                    controller::SeekTo {
                        package_name: caller,
                        pos: ms as i64,
                    }
                    .write(p)
                },
                controller::read_seek_to_reply,
            ),
        }
    }
}

/// What Now Playing shows of `package`'s session, of metadata `m` and
/// playback state `p`, at `now` (of `elapsedRealtime`).
fn now_playing(
    package: &str,
    m: Option<&Metadata>,
    p: Option<&PlaybackState>,
    now: i64,
) -> NowPlaying {
    let m = m.cloned().unwrap_or_default();
    let state = match p.map(|p| p.state) {
        None => State::Unknown,
        Some(STATE_PAUSED) => State::Paused,
        _ if p.is_some_and(PlaybackState::advancing) => State::Playing,
        Some(STATE_NONE | STATE_STOPPED | STATE_ERROR) => State::Stopped,
        // Buffering, connecting, skipping.
        Some(_) => State::Interrupted,
    };
    let p = p.copied().unwrap_or_default();
    let mut commands = 0;
    for (actions, command) in [
        (ACTION_PLAY | ACTION_PLAY_PAUSE, media::commands::PLAY),
        (ACTION_PAUSE | ACTION_PLAY_PAUSE, media::commands::PAUSE),
        (ACTION_STOP, media::commands::STOP),
        (ACTION_SKIP_TO_NEXT, media::commands::NEXT),
        (ACTION_SKIP_TO_PREVIOUS, media::commands::PREVIOUS),
        (ACTION_SEEK_TO, media::commands::SEEK),
    ] {
        if p.actions & actions != 0 {
            commands |= command;
        }
    }
    NowPlaying {
        package: package.to_string(),
        title: m.title().unwrap_or_default().to_string(),
        artist: m.artist().unwrap_or_default().to_string(),
        album: m.album.clone().unwrap_or_default(),
        duration_ms: m.duration.filter(|&d| d > 0).map(|d| d as u64),
        position_ms: p.position_at(now, m.duration) as u64,
        rate: if p.advancing() { p.speed } else { 0.0 },
        state,
        commands,
        artwork: m.artwork().cloned(),
    }
}

/// A reference to a binder received, which must be taken while the call
/// or reply that carries it is alive.
fn hold(process: &LocalProcess, binder: Option<Binder>) -> Option<Strong> {
    match binder {
        Some(Binder::Handle(h)) => Some(process.strong(h)),
        _ => None,
    }
}

/// Calls `code` of `target` and reads the reply.
fn call<T>(
    target: &Strong,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(&mut Reader<'_>) -> aim_binder_host::parcel::Result<Returned<T>>,
) -> Result<T, String> {
    let mut data = Parcel::new();
    write(&mut data);
    let reply = target
        .transact(code, &data, false)
        .map_err(|s| format!("status {s}"))?;
    read(&mut reply.reader())
        .map_err(|s| format!("status {s}"))?
        .map_err(|e: Exception| e.message)
}

/// The display server's media connection.
fn connect(display: &Path) -> Result<UnixStream, String> {
    let link = UnixStream::connect(display).map_err(|e| format!("{}: {e}", display.display()))?;
    let hello = wire::Request {
        op: wire::OP_MEDIA,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    // Its mode: the server shows media in both.
    wire::recv_record::<u32>(link.as_fd()).map_err(|e| e.to_string())?;
    // SO_NOSIGPIPE: a write after the server has gone fails instead.
    let on: libc::c_int = 1;
    // SAFETY: setsockopt on our socket with a local int.
    unsafe {
        libc::setsockopt(
            link.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    Ok(link)
}

impl Service for Listener {
    fn descriptor(&self) -> &str {
        listener::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bridge) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        if call.code != listener::ON_MEDIA_KEY_EVENT_SESSION_CHANGED {
            return Err(UNKNOWN_TRANSACTION);
        }
        let args = listener::OnMediaKeyEventSessionChanged::<Token>::read(&mut call.data)?;
        let controller = args
            .media_key_event_session_token
            .and_then(|t| t.controller);
        bridge.follow(hold(&bridge.process, controller));
        Ok(Parcel::new())
    }
}

impl Service for Callback {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // Bitmaps' ashmem in the metadata.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bridge) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // The payload may be of a session no longer followed: the current
        // one is asked instead.
        match call.code {
            callback::ON_PLAYBACK_STATE_CHANGED => bridge.changed(false),
            callback::ON_METADATA_CHANGED => bridge.changed(true),
            callback::ON_SESSION_DESTROYED => bridge.refresh_session(),
            // Events, the queue, extras and volume: nothing Now Playing
            // shows.
            _ => {}
        }
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parcels::STATE_PLAYING;

    #[test]
    fn shows_the_session_as_now_playing() {
        let m = Metadata {
            title: Some("Song".into()),
            album_artist: Some("Band".into()),
            duration: Some(60_000),
            ..Default::default()
        };
        let p = PlaybackState {
            state: STATE_PLAYING,
            position: 1_000,
            speed: 1.0,
            update_time: 5_000,
            actions: ACTION_PLAY_PAUSE | ACTION_SKIP_TO_NEXT | ACTION_SEEK_TO,
        };
        let n = now_playing("com.example", Some(&m), Some(&p), 7_000);
        assert_eq!((n.title.as_str(), n.artist.as_str()), ("Song", "Band"));
        assert_eq!(
            (n.duration_ms, n.position_ms, n.rate),
            (Some(60_000), 3_000, 1.0)
        );
        assert_eq!(n.state, State::Playing);
        use media::commands::*;
        assert_eq!(n.commands, PLAY | PAUSE | NEXT | SEEK);
        let paused = PlaybackState {
            state: STATE_PAUSED,
            ..p
        };
        let n = now_playing("com.example", Some(&m), Some(&paused), 7_000);
        assert_eq!(
            (n.state, n.position_ms, n.rate),
            (State::Paused, 1_000, 0.0)
        );
        let buffering = PlaybackState { state: 6, ..p };
        assert_eq!(
            now_playing("p", None, Some(&buffering), 0).state,
            State::Interrupted
        );
        assert_eq!(now_playing("p", None, None, 0).state, State::Unknown);
    }
}
