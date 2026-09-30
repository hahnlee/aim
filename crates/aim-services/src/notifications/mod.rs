//! The notification bridge (`docs/notifications.md`): Android's
//! notifications as Mac notifications, while the original
//! NotificationManagerService (NMS) and SystemUI keep running.
//!
//! A native `INotificationListener` of the system uid registers with the
//! original NMS (`INotificationManager.registerListener`, as SystemUI's
//! listener does) and gets every posted, updated and removed notification
//! with its `StatusBarNotification`. It sends what the Mac shows of each
//! ([`aim_host_display::notify::Post`]) to the display server, which
//! passes it to the posting app's shim (its own Mac identity). What the
//! user does there comes back: a click sends the notification's content
//! `PendingIntent` (`IActivityManager.sendIntentSender`, with the
//! notification's allowlist token, as SystemUI sends it) and reports the
//! click (`IStatusBarService.onNotificationClick`, which auto-cancels);
//! an action sends its `PendingIntent`, with a `RemoteInput` reply as its
//! fill-in intent; a dismissal is `cancelNotificationsFromListener`.
//! When the Mac is locked or asleep, the server has the bridge launch a
//! notification's full-screen intent, as SystemUI does when the device is.
//! An app's request for POST_NOTIFICATIONS is answered by its shim's
//! authorization prompt, and the Mac's per-app setting is mirrored into
//! the permission ([`permission`], #470). The app ops SystemUI's status
//! bar indicators watch go to the apps' menu bar items too
//! ([`indicators`]).
//!
//! Only in window mode: in device mode SystemUI's shade shows them.

mod icons;
mod indicators;
pub(crate) mod parcels;
mod permission;

pub use icons::GuestFiles;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::Duration;

use aim_apps::apk::Apk;
use aim_binder_driver::{Credentials, Device, Driver};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel, Reader, UNKNOWN_TRANSACTION};
use aim_host_display::notify::{self, Clock, Image, Live, Message, Post, Progress};
use aim_host_display::wire;
use aim_hostcall::display::mode;
use aim_service_aidl::{
    Returned, android_app_iactivitymanager as am, android_app_inotificationmanager as nm,
    android_os_iservicemanager as sm,
    android_service_notification_inotificationlistener as listener,
    android_service_notification_istatusbarnotificationholder as holder,
    com_android_internal_statusbar_istatusbarservice as statusbar,
};

use parcels::{
    Channel, ComponentName, FLAG_FOREGROUND_SERVICE, FLAG_GROUP_SUMMARY, FLAG_NO_CLEAR,
    FLAG_ONGOING_EVENT, FLAG_ONLY_ALERT_ONCE, FLAG_PROMOTED_ONGOING, Files, NotificationStats,
    RankingUpdate, ReplyIntent, SendOptions, StatusBarNotification, Visibility,
};

/// `Process.SYSTEM_UID` and system_server's context, as the other native
/// services.
const SYSTEM_UID: u32 = 1000;
const SYSTEM_SERVER_CONTEXT: &str = "u:r:system_server:s0";
/// `UserHandle.USER_ALL`: every user's notifications.
const USER_ALL: i32 = -1;
/// The listener's component, as NMS records it (a system listener is not
/// bound, so no such class exists).
const COMPONENT: ComponentName<'static> = ComponentName("android", "aim.notifications.MacBridge");
/// `NotificationManager.IMPORTANCE_LOW`: shown without interrupting.
const IMPORTANCE_LOW: i32 = 2;
/// `NotificationManager.IMPORTANCE_HIGH`: may take the screen.
const IMPORTANCE_HIGH: i32 = 4;
/// `Notification.GROUP_ALERT_SUMMARY`: a group's children do not alert.
const GROUP_ALERT_SUMMARY: i32 = 1;
/// `Notification.BubbleMetadata.FLAG_SUPPRESS_NOTIFICATION`.
const BUBBLE_SUPPRESS_NOTIFICATION: i32 = 0x2;
/// How often the bridge looks for NMS while it is not there.
const RETRY: Duration = Duration::from_secs(1);
/// The largest blob it maps.
const MAX_BLOB: usize = 64 << 20;
/// `UserHandle.USER_SYSTEM`: the user the Mac's setting is mirrored into.
const USER_SYSTEM: i32 = 0;
/// `Notification.CallStyle.CALL_TYPE_ONGOING`.
const CALL_TYPE_ONGOING: i32 = 2;
const CALL_STYLE: &str = "android.app.Notification$CallStyle";
/// The styles `Notification.hasPromotableStyle` accepts besides none.
const PROMOTABLE_STYLES: [&str; 3] = [
    "android.app.Notification$BigTextStyle",
    CALL_STYLE,
    "android.app.Notification$ProgressStyle",
];

/// Called once with the permission's state after the user's answer.
pub type Answer = Box<dyn FnOnce(bool) + Send>;

/// The bridge while it shows notifications on the Mac, for the service
/// host.
static MAC: Mutex<Weak<Bridge>> = Mutex::new(Weak::new());

/// A notification the Mac shows, and the binders its actions need.
struct Entry {
    package: String,
    notification: parcels::Notification,
    /// Its small icon as the menu bar shows it, if it is live.
    live_icon: Option<Image>,
    /// References to its binders, held as long as it is shown.
    _binders: Vec<Strong>,
}

struct State {
    /// NMS, while registered with it.
    manager: Option<Arc<Strong>>,
    entries: HashMap<String, Entry>,
}

pub struct Bridge {
    process: Arc<LocalProcess>,
    /// The guest's files, for icons.
    files: Arc<GuestFiles>,
    /// framework-res, once an icon needed it.
    framework: OnceLock<Option<Apk>>,
    listener: Binder,
    link: Mutex<UnixStream>,
    state: Mutex<State>,
    /// Signalled when NMS dies.
    gone: Condvar,
    /// Requests for POST_NOTIFICATIONS waiting for the Mac's answer: the
    /// user of each, by package.
    requests: Mutex<HashMap<String, Vec<(i32, Answer)>>>,
    /// The app ops active that the indicators show.
    active: Mutex<indicators::Active>,
}

/// The listener node.
struct Listener(Weak<Bridge>);

/// What a click or action sends: the `IIntentSender`, the allowlist token
/// and the key of its `RemoteInput`'s result.
type Target = (Binder, Option<Binder>, Option<String>);

impl Bridge {
    /// Starts the bridge in the background: it connects to the display
    /// server at `display` and, in window mode, registers with NMS once it
    /// is published (again after system_server restarts). Icons given as
    /// a resource or a `file:` URI are read from `files`.
    pub fn start(driver: &Arc<Driver>, display: &Path, files: Arc<GuestFiles>) {
        let driver = driver.clone();
        let display = display.to_owned();
        let _ = std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                if let Err(e) = Bridge::run(&driver, &display, files) {
                    eprintln!("guest-init: notifications: {e}");
                }
            });
    }

    fn run(driver: &Arc<Driver>, display: &Path, files: Arc<GuestFiles>) -> Result<(), String> {
        let Some(link) = connect(display)? else {
            return Ok(());
        };
        let process = LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
            },
        );
        let reader = link.try_clone().map_err(|e| e.to_string())?;
        let bridge = Arc::new_cyclic(|this: &Weak<Bridge>| Bridge {
            listener: process.add_service(Arc::new(Listener(this.clone()))),
            process: process.clone(),
            files,
            framework: OnceLock::new(),
            link: Mutex::new(link),
            state: Mutex::new(State {
                manager: None,
                entries: HashMap::new(),
            }),
            gone: Condvar::new(),
            requests: Mutex::new(HashMap::new()),
            active: Mutex::default(),
        });
        process.start();
        *MAC.lock().unwrap() = Arc::downgrade(&bridge);
        let this = bridge.clone();
        std::thread::Builder::new()
            .name("notifications-mac".into())
            .spawn(move || this.serve_mac(reader))
            .map_err(|e| e.to_string())?;
        bridge.follow_manager();
        Ok(())
    }

    /// The bridge, if it shows notifications on the Mac (window mode).
    pub fn mac() -> Option<Arc<Bridge>> {
        MAC.lock().unwrap().upgrade()
    }

    /// Asks the Mac for `package`'s notification authorization (its
    /// shim's prompt, the first time), grants or revokes POST_NOTIFICATIONS
    /// of `user` by the answer, and tells `answer` the permission's state.
    pub fn request_permission(&self, package: String, user: i32, answer: Answer) {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            let waiting = requests.entry(package.clone()).or_default();
            waiting.push((user, answer));
            waiting.len() == 1
        };
        if first {
            self.send(&Message::Authorize { package });
        }
    }

    /// The Mac's setting for `package`: applied to the users waiting for
    /// it (an answer), else mirrored into the system user.
    fn authorization(&self, package: &str, allowed: Option<bool>, answer: bool) {
        let waiting = if answer {
            self.requests
                .lock()
                .unwrap()
                .remove(package)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut users: Vec<i32> = if answer {
            waiting.iter().map(|(user, _)| *user).collect()
        } else {
            vec![USER_SYSTEM]
        };
        users.sort_unstable();
        users.dedup();
        if let Some(allow) = allowed {
            for &user in &users {
                if let Err(e) = self.set_notifications(package, user, allow) {
                    eprintln!("guest-init: notifications: {package}: permission: {e}");
                }
            }
        }
        for (user, answer) in waiting {
            answer(
                self.notifications_granted(package, user)
                    .unwrap_or_else(|e| {
                        eprintln!("guest-init: notifications: {package}: permission: {e}");
                        false
                    }),
            );
        }
    }

    /// Registers with NMS whenever it is published, until the process
    /// ends.
    fn follow_manager(self: &Arc<Self>) {
        loop {
            let Some(manager) = self.find("notification") else {
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
            let mut data = Parcel::new();
            nm::RegisterListener {
                listener: Some(self.listener),
                component: Some(COMPONENT),
                userid: USER_ALL,
            }
            .write(&mut data);
            match manager.transact(nm::REGISTER_LISTENER, &data, false) {
                Ok(reply) => match nm::read_register_listener_reply(&mut reply.reader()) {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        eprintln!("guest-init: notifications: registerListener: {}", e.message);
                        std::thread::sleep(RETRY);
                        continue;
                    }
                    Err(s) => {
                        eprintln!("guest-init: notifications: registerListener: status {s}");
                        std::thread::sleep(RETRY);
                        continue;
                    }
                },
                Err(s) => {
                    eprintln!("guest-init: notifications: registerListener: status {s}");
                    std::thread::sleep(RETRY);
                    continue;
                }
            }
            self.state.lock().unwrap().manager = Some(manager);
            self.sync();
            self.watch_ops();
            let mut state = self.state.lock().unwrap();
            while state.manager.is_some() {
                state = self.gone.wait(state).unwrap();
            }
        }
    }

    /// NMS died with system_server: its notifications are gone.
    fn manager_died(&self) {
        let entries = {
            let mut state = self.state.lock().unwrap();
            state.manager = None;
            std::mem::take(&mut state.entries)
        };
        for (key, e) in entries {
            self.send(&Message::Remove {
                key,
                package: e.package,
            });
        }
        self.ops_gone();
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

    fn manager(&self) -> Option<Arc<Strong>> {
        self.state.lock().unwrap().manager.clone()
    }

    fn send(&self, m: &Message) {
        let mut link = self.link.lock().unwrap();
        if let Err(e) = link.write_all(&m.frame()) {
            eprintln!("guest-init: notifications: display server: {e}");
        }
    }

    /// Calls `code` of service `name` (found each time: these calls are
    /// the user's clicks) and reads the reply.
    fn call<T>(
        &self,
        name: &str,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> aim_binder_host::parcel::Result<Returned<T>>,
    ) -> Result<T, String> {
        let service = self.find(name).ok_or(format!("no {name} service"))?;
        let mut data = Parcel::new();
        write(&mut data);
        let reply = service
            .transact(code, &data, false)
            .map_err(|s| format!("{name}: status {s}"))?;
        read(&mut reply.reader())
            .map_err(|s| format!("{name}: status {s}"))?
            .map_err(|e: Exception| format!("{name}: {}", e.message))
    }

    /// A notification was posted or updated.
    fn posted(&self, sbn: StatusBarNotification) {
        let key = sbn.key();
        let n = &sbn.notification;
        if !n.complete {
            eprintln!("guest-init: notifications: {key}: read in part");
        }
        let binders = n
            .binders()
            .filter_map(|b| match b {
                Binder::Handle(h) => Some(self.process.strong(h)),
                Binder::Local(_) => None,
            })
            .collect();
        let importance = n.channel_id.as_deref().and_then(|channel| {
            let args = nm::GetNotificationChannelForPackage {
                pkg: Some(sbn.package.clone()),
                uid: sbn.uid,
                channel_id: Some(channel.into()),
                conversation_id: n.shortcut_id.clone(),
                include_deleted: false,
            };
            let manager = self.manager()?;
            let mut data = Parcel::new();
            args.write(&mut data);
            let reply = manager
                .transact(nm::GET_NOTIFICATION_CHANNEL_FOR_PACKAGE, &data, false)
                .ok()?;
            let channel =
                nm::read_get_notification_channel_for_package_reply::<Channel>(&mut reply.reader());
            Some(channel.ok()?.ok()??.importance)
        });
        let updated = self.state.lock().unwrap().entries.contains_key(&key);
        // UserHandle.getUserId: the posting app's user.
        let user = sbn.uid / 100_000;
        let e = &n.extras;
        let image = [&e.picture, &e.large_icon, &n.large_icon]
            .into_iter()
            .flatten()
            .find_map(|i| self.image(i, user));
        let live = is_live(n).then(|| {
            // An update with the same small icon (a progress step) keeps
            // the image drawn.
            let kept = self
                .state
                .lock()
                .unwrap()
                .entries
                .get(&key)
                .filter(|old| old.notification.small_icon == n.small_icon)
                .and_then(|old| old.live_icon.clone());
            let icon = kept.or_else(|| n.small_icon.as_ref().and_then(|i| self.image(i, user)));
            Box::new(to_live(n, Now::read(), icon))
        });
        let live_icon = live.as_ref().and_then(|l| l.icon.clone());
        let post = to_post(&sbn, &key, importance, updated, image).map(|p| Post { live, ..p });
        self.state.lock().unwrap().entries.insert(
            key,
            Entry {
                package: sbn.package.clone(),
                notification: sbn.notification.clone(),
                live_icon,
                _binders: binders,
            },
        );
        if let Some(post) = post {
            self.send(&Message::Post(post));
        }
    }

    fn removed(&self, key: String) {
        if let Some(e) = self.state.lock().unwrap().entries.remove(&key) {
            self.send(&Message::Remove {
                key,
                package: e.package,
            });
        }
    }

    /// Calls `f` with the notification a `IStatusBarNotificationHolder`
    /// holds, while the reply it came in (and its binders) is alive.
    fn fetch(&self, holder: Option<Binder>, f: impl FnOnce(StatusBarNotification)) {
        let Some(Binder::Handle(h)) = holder else {
            return;
        };
        let mut data = Parcel::new();
        holder::Get {}.write(&mut data);
        let reply = match self.process.transact(h, holder::GET, &data, false) {
            Ok(reply) => reply,
            Err(s) => {
                eprintln!("guest-init: notifications: holder: status {s}");
                return;
            }
        };
        let mut r = reply.reader();
        let read = (|| -> aim_binder_host::parcel::Result<Option<StatusBarNotification>> {
            if r.read_exception()?.is_err() || r.read_i32()? == 0 {
                return Ok(None);
            }
            StatusBarNotification::read(&mut r, &ProcessFiles(&self.process)).map(Some)
        })();
        if let Ok(Some(sbn)) = read {
            f(sbn);
        }
    }

    /// What NMS already holds when the bridge registers.
    fn sync(&self) {
        let Some(manager) = self.manager() else {
            return;
        };
        let mut data = Parcel::new();
        nm::GetActiveNotificationsFromListener {
            token: Some(self.listener),
            keys: None,
            trim: 0,
        }
        .write(&mut data);
        let Ok(reply) = manager.transact(nm::GET_ACTIVE_NOTIFICATIONS_FROM_LISTENER, &data, false)
        else {
            return;
        };
        let mut r = reply.reader();
        let files = ProcessFiles(&self.process);
        let mut posted = Vec::new();
        let _ = (|| -> aim_binder_host::parcel::Result<()> {
            if r.read_exception()?.is_err() || r.read_i32()? == 0 {
                return Ok(());
            }
            // ParceledListSlice: the count, the class, then items inline
            // while they fit, then a binder for the rest.
            let count = r.read_i32()?;
            if count <= 0 {
                return Ok(());
            }
            r.read_string16()?;
            while posted.len() < count as usize {
                if r.read_i32()? == 0 {
                    break;
                }
                let sbn = StatusBarNotification::read(&mut r, &files)?;
                let complete = sbn.notification.complete;
                posted.push(sbn);
                if !complete {
                    // What follows it cannot be found.
                    return Ok(());
                }
            }
            Ok(())
        })();
        for sbn in posted {
            self.posted(sbn);
        }
    }

    /// The display server's messages: what the user did on the Mac.
    fn serve_mac(self: Arc<Self>, mut reader: UnixStream) {
        loop {
            match Message::read(&mut reader) {
                Ok(Some(m)) => {
                    if let Err(e) = self.user(&m) {
                        eprintln!("guest-init: notifications: {}: {e}", m.key());
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    eprintln!("guest-init: notifications: display server: {e}");
                    return;
                }
            }
        }
    }

    fn user(&self, m: &Message) -> Result<(), String> {
        let key = m.key().to_string();
        let entry = |f: &dyn Fn(&Entry) -> Option<Target>| {
            let state = self.state.lock().unwrap();
            state.entries.get(&key).and_then(f)
        };
        match m {
            Message::Click { .. } => {
                let Some((target, token, _)) = entry(&|e| {
                    Some((
                        e.notification.content_intent?,
                        e.notification.allowlist_token,
                        None,
                    ))
                }) else {
                    return Ok(());
                };
                self.send_intent(target, token, None)?;
                self.call(
                    "statusbar",
                    statusbar::ON_NOTIFICATION_CLICK,
                    |p| {
                        statusbar::OnNotificationClick {
                            key: Some(key.clone()),
                            nv: Some(Visibility(&key)),
                        }
                        .write(p)
                    },
                    statusbar::read_on_notification_click_reply,
                )
            }
            Message::Action { index, reply, .. } => {
                let Some((target, token, result_key)) = entry(&|e| {
                    let a = e.notification.actions.get(*index as usize)?;
                    Some((
                        a.intent?,
                        e.notification.allowlist_token,
                        a.inputs.first().map(|i| i.result_key.clone()),
                    ))
                }) else {
                    return Ok(());
                };
                let fill_in = match (reply, &result_key) {
                    (Some(text), Some(result_key)) => Some(ReplyIntent { result_key, text }),
                    _ => None,
                };
                let replied = fill_in.is_some();
                self.send_intent(target, token, fill_in)?;
                if !replied {
                    return Ok(());
                }
                self.call(
                    "statusbar",
                    statusbar::ON_NOTIFICATION_DIRECT_REPLIED,
                    |p| {
                        statusbar::OnNotificationDirectReplied {
                            key: Some(key.clone()),
                        }
                        .write(p)
                    },
                    statusbar::read_on_notification_direct_replied_reply,
                )
            }
            Message::Dismiss { .. } => {
                let manager = self.manager().ok_or("not registered")?;
                let mut data = Parcel::new();
                nm::CancelNotificationsFromListener {
                    token: Some(self.listener),
                    keys: Some(vec![Some(key.clone())]),
                }
                .write(&mut data);
                let reply = manager
                    .transact(nm::CANCEL_NOTIFICATIONS_FROM_LISTENER, &data, false)
                    .map_err(|s| format!("cancel: status {s}"))?;
                nm::read_cancel_notifications_from_listener_reply(&mut reply.reader())
                    .map_err(|s| format!("cancel: status {s}"))?
                    .map_err(|e| format!("cancel: {}", e.message))
            }
            Message::FullScreen { .. } => {
                let Some((target, token, _)) = entry(&|e| {
                    Some((
                        e.notification.full_screen_intent?,
                        e.notification.allowlist_token,
                        None,
                    ))
                }) else {
                    return Ok(());
                };
                self.send_intent(target, token, None)
            }
            Message::Authorization {
                package,
                allowed,
                answer,
            } => {
                self.authorization(package, *allowed, *answer);
                Ok(())
            }
            Message::Post(_)
            | Message::Remove { .. }
            | Message::Shown { .. }
            | Message::Authorize { .. }
            | Message::Indicator { .. } => Err("not from the Mac".into()),
        }
    }

    /// `PendingIntent.send` as SystemUI does it for a notification.
    fn send_intent(
        &self,
        target: Binder,
        token: Option<Binder>,
        fill_in: Option<ReplyIntent<'_>>,
    ) -> Result<(), String> {
        let code = self.call(
            "activity",
            am::SEND_INTENT_SENDER,
            |p| {
                am::SendIntentSender {
                    caller: None,
                    target: Some(target),
                    whitelist_token: token,
                    code: 0,
                    intent: fill_in,
                    resolved_type: None,
                    finished_receiver: None,
                    required_permission: None,
                    options: Some(SendOptions),
                }
                .write(p)
            },
            am::read_send_intent_sender_reply,
        )?;
        // ActivityManager.START_CANCELED and other errors are negative.
        if code < 0 {
            return Err(format!("sendIntentSender: {code}"));
        }
        Ok(())
    }
}

/// The display server, if it shows notifications (window mode).
fn connect(display: &Path) -> Result<Option<UnixStream>, String> {
    let link = UnixStream::connect(display).map_err(|e| format!("{}: {e}", display.display()))?;
    let hello = wire::Request {
        op: wire::OP_NOTIFICATIONS,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    let answer = wire::recv_record::<u32>(link.as_fd()).map_err(|e| e.to_string())?;
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
    Ok((answer == Some(mode::WINDOWS)).then_some(link))
}

/// What the Mac shows of `sbn`, with `image` drawn from its icons: none
/// for a group's summary (the Mac groups by thread itself).
fn to_post(
    sbn: &StatusBarNotification,
    key: &str,
    importance: Option<i32>,
    updated: bool,
    image: Option<Image>,
) -> Option<Post> {
    let n = &sbn.notification;
    if n.flags & FLAG_GROUP_SUMMARY != 0 {
        return None;
    }
    let e = &n.extras;
    let title = e
        .conversation_title
        .clone()
        .or(e.title.clone())
        .unwrap_or_default();
    let body = match e.messages.last() {
        Some(m) => {
            let text = m.text.clone().unwrap_or_default();
            match &m.sender {
                Some(s) if *s != title => format!("{s}: {text}"),
                _ => text,
            }
        }
        None => e
            .big_text
            .clone()
            .or(e.text.clone())
            .or_else(|| (!e.lines.is_empty()).then(|| e.lines.join("\n")))
            .or(n.ticker.clone())
            .unwrap_or_default(),
    };
    let passive = importance.is_some_and(|i| i <= IMPORTANCE_LOW)
        || (updated && n.flags & FLAG_ONLY_ALERT_ONCE != 0);
    // What SystemUI asks of a full-screen intent before the device's
    // state (FullScreenIntentDecisionProvider): important enough, alerting,
    // not a silenced group child, not shown as a bubble instead.
    let full_screen = n.full_screen_intent.is_some()
        && importance.is_some_and(|i| i >= IMPORTANCE_HIGH)
        && !passive
        && !(n.group.is_some() && n.group_alert_behavior == GROUP_ALERT_SUMMARY)
        && n.bubble_flags & BUBBLE_SUPPRESS_NOTIFICATION == 0;
    let ongoing = n.flags & (FLAG_ONGOING_EVENT | FLAG_NO_CLEAR | FLAG_FOREGROUND_SERVICE) != 0;
    Some(Post {
        key: key.to_string(),
        package: sbn.package.clone(),
        title,
        subtitle: e.sub_text.clone().unwrap_or_default(),
        body,
        thread: sbn
            .override_group_key
            .clone()
            .or(n.group.clone())
            .unwrap_or_default(),
        passive,
        badge: !ongoing,
        full_screen,
        actions: n
            .actions
            .iter()
            .map(|a| notify::Action {
                title: a.title.clone().unwrap_or_default(),
                input: a
                    .inputs
                    .first()
                    .map(|i| i.label.clone().unwrap_or_default()),
            })
            .collect(),
        image,
        live: None,
    })
}

/// Whether the menu bar shows `n` as an ongoing activity: an ongoing
/// notification NMS promoted (`FLAG_PROMOTED_ONGOING`), or would promote
/// but for the framework's Live Updates UI, which is off in the image
/// (`ui_rich_ongoing`, so apps may not be promoted by default): the
/// characteristics of `Notification.hasPromotableCharacteristics`; and a
/// notification whose time runs in a chronometer (a timer, a stopwatch, a
/// call, a recording), ongoing or not (Clock's timers are not).
fn is_live(n: &parcels::Notification) -> bool {
    if n.flags & FLAG_GROUP_SUMMARY != 0 {
        return false;
    }
    let e = &n.extras;
    let style = e.template.as_deref();
    let promotable = !n.views.custom
        && e.title.as_deref().is_some_and(|t| !t.is_empty())
        && ((style == Some(CALL_STYLE) && e.call_type == CALL_TYPE_ONGOING)
            || (e.colorized && style.is_none_or(|s| PROMOTABLE_STYLES.contains(&s))));
    let ongoing = n.flags & FLAG_ONGOING_EVENT != 0
        && (n.flags & FLAG_PROMOTED_ONGOING != 0 || promotable);
    let chronometer = if n.views.custom {
        n.views.chronometer.is_some()
    } else {
        e.show_chronometer
    };
    ongoing || chronometer
}

/// The clocks a chronometer is read against, in milliseconds.
#[derive(Clone, Copy)]
struct Now {
    /// `SystemClock.elapsedRealtime()`.
    boot_ms: i64,
    /// `System.currentTimeMillis()`.
    wall_ms: i64,
}

impl Now {
    fn read() -> Now {
        Now {
            boot_ms: aim_hostcall::clock::boottime_ns() / 1_000_000,
            wall_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
        }
    }
}

/// What the menu bar shows of live notification `n`: its custom views'
/// chronometer if it has custom views, else the template's.
fn to_live(n: &parcels::Notification, now: Now, icon: Option<Image>) -> Live {
    let e = &n.extras;
    let clock = if n.views.custom {
        n.views
            .chronometer
            .map(|c| match (c.started, c.count_down) {
                (true, down) => Clock::Counting {
                    base_ms: c.base,
                    down,
                },
                (false, true) => Clock::Stopped {
                    shown_ms: c.base - now.boot_ms,
                },
                (false, false) => Clock::Stopped {
                    shown_ms: now.boot_ms - c.base,
                },
            })
    } else {
        e.show_chronometer.then_some(Clock::Counting {
            base_ms: n.when - now.wall_ms + now.boot_ms,
            down: e.chronometer_count_down,
        })
    };
    // A bar shows when it has a maximum or is indeterminate
    // (`Notification.Builder.hasProgress`).
    let max = e.progress_max.unwrap_or(0).max(0) as u32;
    let progress = (max > 0 || e.progress_indeterminate).then(|| Progress {
        value: (e.progress.unwrap_or(0).max(0) as u32).min(max),
        max,
        indeterminate: e.progress_indeterminate,
    });
    Live {
        text: e.short_critical_text.clone().unwrap_or_default(),
        clock,
        progress,
        icon,
    }
}

/// Blobs of a call or reply, read from the files the guest sent.
pub(crate) struct ProcessFiles<'a>(pub(crate) &'a LocalProcess);

impl Files for ProcessFiles<'_> {
    fn read(&self, fd: u32, len: usize) -> Option<Vec<u8>> {
        if len == 0 || len > MAX_BLOB {
            return None;
        }
        let file = self.0.file(fd)?;
        let fd = aim_binder_host::server::file_fd(&file)?;
        // SAFETY: a read-only shared mapping of a file we hold open,
        // copied and unmapped here.
        unsafe {
            let p = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if p == libc::MAP_FAILED {
                return None;
            }
            let bytes = std::slice::from_raw_parts(p.cast::<u8>(), len).to_vec();
            libc::munmap(p, len);
            Some(bytes)
        }
    }
}

impl Service for Listener {
    fn descriptor(&self) -> &str {
        listener::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // The ranking update's shared memory, and bitmaps' ashmem.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bridge) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        let files = ProcessFiles(&bridge.process);
        let r = &mut call.data;
        match call.code {
            listener::ON_NOTIFICATION_POSTED => {
                let args = listener::OnNotificationPosted::<RankingUpdate>::read(r)?;
                bridge.fetch(args.notification_holder, |sbn| bridge.posted(sbn));
            }
            listener::ON_NOTIFICATION_POSTED_FULL => {
                r.enforce_interface(listener::DESCRIPTOR)?;
                if r.read_i32()? != 0 {
                    // Its binders live while this call is served.
                    bridge.posted(StatusBarNotification::read(r, &files)?);
                }
            }
            listener::ON_NOTIFICATION_REMOVED => {
                let args =
                    listener::OnNotificationRemoved::<RankingUpdate, NotificationStats>::read(r)?;
                bridge.fetch(args.notification_holder, |sbn| bridge.removed(sbn.key()));
            }
            listener::ON_NOTIFICATION_REMOVED_FULL => {
                r.enforce_interface(listener::DESCRIPTOR)?;
                if r.read_i32()? == 0 {
                    return Err(BAD_VALUE);
                }
                bridge.removed(StatusBarNotification::read(r, &files)?.key());
            }
            // Connected (the bridge reads what NMS holds once
            // registerListener returns), rankings, hints, filters, and the
            // assistants' calls: nothing the Mac shows.
            _ => {}
        }
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_full_screen_intent() -> StatusBarNotification {
        let mut sbn = StatusBarNotification {
            package: "com.example".into(),
            uid: 10_123,
            ..Default::default()
        };
        sbn.notification.full_screen_intent = Some(Binder::Local(0x10));
        sbn
    }

    #[test]
    fn takes_the_screen_as_systemui_would() {
        let full_screen = |sbn: &StatusBarNotification, importance, updated| {
            to_post(sbn, "k", importance, updated, None)
                .unwrap()
                .full_screen
        };
        let mut sbn = with_full_screen_intent();
        assert!(full_screen(&sbn, Some(IMPORTANCE_HIGH), false));
        assert!(!full_screen(&sbn, Some(IMPORTANCE_HIGH - 1), false));
        assert!(!full_screen(&sbn, None, false));
        sbn.notification.flags = FLAG_ONLY_ALERT_ONCE;
        assert!(full_screen(&sbn, Some(IMPORTANCE_HIGH), false));
        assert!(!full_screen(&sbn, Some(IMPORTANCE_HIGH), true));
        let mut child = with_full_screen_intent();
        child.notification.group = Some("g".into());
        child.notification.group_alert_behavior = GROUP_ALERT_SUMMARY;
        assert!(!full_screen(&child, Some(IMPORTANCE_HIGH), false));
        let mut bubble = with_full_screen_intent();
        bubble.notification.bubble_flags = BUBBLE_SUPPRESS_NOTIFICATION;
        assert!(!full_screen(&bubble, Some(IMPORTANCE_HIGH), false));
        let mut none = with_full_screen_intent();
        none.notification.full_screen_intent = None;
        assert!(!full_screen(&none, Some(IMPORTANCE_HIGH), false));
    }

    fn ongoing() -> parcels::Notification {
        let mut n = parcels::Notification {
            flags: FLAG_ONGOING_EVENT,
            ..Default::default()
        };
        n.extras.title = Some("Download".into());
        n
    }

    #[test]
    fn lives_in_the_menu_bar_as_it_would_be_promoted() {
        let mut n = ongoing();
        assert!(!is_live(&n));
        n.extras.colorized = true;
        assert!(is_live(&n));
        n.extras.template = Some("android.app.Notification$ProgressStyle".into());
        assert!(is_live(&n));
        n.extras.template = Some("android.app.Notification$InboxStyle".into());
        assert!(!is_live(&n));
        let mut call = ongoing();
        call.extras.template = Some(CALL_STYLE.into());
        call.extras.call_type = CALL_TYPE_ONGOING;
        assert!(is_live(&call));
        call.flags = 0;
        assert!(!is_live(&call));
        call.flags = FLAG_PROMOTED_ONGOING;
        assert!(!is_live(&call));
        let mut promoted = ongoing();
        promoted.flags |= FLAG_PROMOTED_ONGOING;
        assert!(is_live(&promoted));
        promoted.flags |= FLAG_GROUP_SUMMARY;
        assert!(!is_live(&promoted));
        let mut untitled = ongoing();
        untitled.extras.colorized = true;
        untitled.extras.title = None;
        assert!(!is_live(&untitled));
    }

    #[test]
    fn a_running_chronometer_is_live() {
        let mut n = ongoing();
        n.extras.show_chronometer = true;
        n.extras.chronometer_count_down = true;
        n.when = 1_000_000;
        assert!(is_live(&n));
        let now = Now {
            boot_ms: 5_000,
            wall_ms: 900_000,
        };
        assert_eq!(
            to_live(&n, now, None).clock,
            Some(Clock::Counting {
                base_ms: 105_000,
                down: true
            })
        );
        // A custom view's chronometer, paused: it shows what it showed.
        // Not ongoing, as Clock's timers.
        let mut timer = parcels::Notification::default();
        timer.extras.title = None;
        timer.views.custom = true;
        assert!(!is_live(&timer));
        timer.views.chronometer = Some(parcels::ViewChronometer {
            base: 65_000,
            count_down: true,
            started: false,
        });
        assert!(is_live(&timer));
        assert_eq!(
            to_live(&timer, now, None).clock,
            Some(Clock::Stopped { shown_ms: 60_000 })
        );
    }

    #[test]
    fn shows_progress_as_the_template_does() {
        let mut n = ongoing();
        n.extras.progress = Some(30);
        let now = Now {
            boot_ms: 0,
            wall_ms: 0,
        };
        assert_eq!(to_live(&n, now, None).progress, None);
        n.extras.progress_max = Some(20);
        assert_eq!(
            to_live(&n, now, None).progress,
            Some(Progress {
                value: 20,
                max: 20,
                indeterminate: false
            })
        );
        n.extras.short_critical_text = Some("5 km".into());
        assert_eq!(to_live(&n, now, None).text, "5 km");
    }
}
