//! BackgroundInstallControlService and BackgroundInstallControlCallbackHelper,
//! adapted from platform/frameworks/base at android-16.0.0_r1 (Apache-2.0).
//! PackageList and UsageStats messages share a FIFO; callbacks have their own
//! FIFO, as the original's two handlers do. State uses background_install_control.proto.
//! Copyright (C) 2022, 2024 The Android Open Source Project.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak, mpsc};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_pm_ibackgroundinstallcontrolservice as bic;
use aim_service_aidl::android_os_iremotecallback as callback;

use super::exit_info::{ProtoWriter, fields};
use super::package_observers::PackageListObserver;
use super::usage_database::{atomic_read, atomic_write};
use super::usage_events::{ACTIVITY_PAUSED, ACTIVITY_RESUMED, ACTIVITY_STOPPED, Event};
use super::{AppHost, PER_USER_RANGE, SYSTEM_UID, reply};
use crate::package::info::{PackageInfo, java_hash};
use crate::package::list_slice::Slice;
use crate::package::query::Query;

const GET_PACKAGES: &str = "android.permission.GET_BACKGROUND_INSTALLED_PACKAGES";
const INSTALL_PACKAGES: &str = "android.permission.INSTALL_PACKAGES";
const STATES: &str = "system/bic/states";

impl AppHost {
    pub(super) fn background_install_control(self: &Arc<Self>) -> Arc<BackgroundInstallControl> {
        self.background_install_control
            .get_or_init(|| BackgroundInstallControl::new(self))
            .clone()
    }
}

fn error(message: impl std::fmt::Display) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!("background_install_control: {message}"),
    )
}

#[derive(Default)]
struct State {
    packages: Option<BTreeMap<i32, Vec<String>>>,
    frames: BTreeMap<(i32, String), Vec<Frame>>,
}

struct Frame {
    start: i64,
    end: i64,
    activities: HashSet<i32>,
}

impl State {
    fn usage(&mut self, user: i32, event: &Event) {
        let frames = self
            .frames
            .entry((user, event.package.clone()))
            .or_default();
        if frames.last().is_none_or(|f| f.end != 0) {
            if event.event_type != ACTIVITY_RESUMED {
                return;
            }
            let at = frames.partition_point(|f| f.start <= event.time_stamp);
            frames.insert(
                at,
                Frame {
                    start: event.time_stamp,
                    end: 0,
                    activities: HashSet::new(),
                },
            );
        }
        let frame = frames.last_mut().unwrap();
        match event.event_type {
            ACTIVITY_RESUMED => {
                frame.activities.insert(event.instance_id);
            }
            ACTIVITY_PAUSED | ACTIVITY_STOPPED => {
                if frame.activities.remove(&event.instance_id) && frame.activities.is_empty() {
                    frame.end = event.time_stamp;
                }
            }
            _ => unreachable!(),
        }
        if frames.len() > 10 {
            frames.remove(0);
        }
    }

    fn foreground(&self, installer: &str, user: i32, timestamp: i64) -> bool {
        self.frames
            .get(&(user, installer.into()))
            .is_some_and(|frames| {
                frames
                    .iter()
                    .any(|f| f.start <= timestamp && (f.end == 0 || timestamp <= f.end))
            })
    }

    fn load(&mut self, file: &Path) -> Result<&mut BTreeMap<i32, Vec<String>>, Exception> {
        if self.packages.is_none() {
            let mut packages = BTreeMap::new();
            // The original tests the base file before AtomicFile.openRead.
            if file.exists() {
                match atomic_read(file) {
                    Ok(bytes) => {
                        if let Err(e) = fields(&bytes, |number, value| {
                            if number == 1 {
                                let (mut name, mut user) = (None, None);
                                fields(value.bytes()?, |number, value| {
                                    match number {
                                        1 => name = Some(value.string()?),
                                        2 => user = Some(value.int()?.wrapping_sub(1)),
                                        _ => {}
                                    }
                                    Ok(())
                                })?;
                                if let (Some(name), Some(user)) = (name, user) {
                                    if user != -10000 {
                                        insert(&mut packages, user, name);
                                    }
                                } else {
                                    eprintln!(
                                        "background_install_control: missing package or user in states"
                                    );
                                }
                            }
                            Ok(())
                        }) {
                            eprintln!("background_install_control: error reading states: {e}");
                        }
                    }
                    Err(e) => eprintln!("background_install_control: error reading states: {e}"),
                }
            }
            self.packages = Some(packages);
        }
        Ok(self.packages.as_mut().unwrap())
    }

    fn persist(&self, file: &Path) -> Result<(), Exception> {
        let mut out = ProtoWriter::default();
        for (&user, names) in self.packages.as_ref().expect("loaded") {
            for name in names {
                let mut entry = ProtoWriter::default();
                entry.string(1, Some(name));
                entry.varint(2, user.wrapping_add(1).into());
                out.message(1, &entry.0);
            }
        }
        let dir = file.parent().unwrap();
        if !dir.exists() {
            std::fs::create_dir_all(dir).map_err(error)?;
            // AtomicFile.startWrite sets a newly created parent to 0771.
            aim_storage::guest_inode::record(
                dir,
                aim_storage::guest_inode::GuestInode {
                    uid: Some(1000),
                    gid: Some(1000),
                    mode: Some(0o771),
                },
            )
            .map_err(error)?;
        }
        atomic_write(file, &out.0).map_err(|e| {
            // AtomicFile.failWrite discards only the unfinished new file.
            let new = file.with_extension("new");
            if let Err(cleanup) = std::fs::remove_file(new)
                && cleanup.kind() != std::io::ErrorKind::NotFound
            {
                return error(format!("{e}; failWrite: {cleanup}"));
            }
            error(e)
        })
    }
}

/// ArraySet<String> orders by Java hash, preserving insertion among collisions.
fn insert(packages: &mut BTreeMap<i32, Vec<String>>, user: i32, name: String) {
    let names = packages.entry(user).or_default();
    if !names.contains(&name) {
        let at = names.partition_point(|n| java_hash(n) <= java_hash(&name));
        names.insert(at, name);
    }
}

enum Message {
    Usage(i32, Event),
    Added(i32, String),
    Removed(i32, String),
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
}

pub(super) struct BackgroundInstallControl {
    host: Weak<AppHost>,
    state: Mutex<State>,
    handler: mpsc::Sender<Message>,
    notifications: mpsc::Sender<InstallEvent>,
    callbacks: Mutex<BTreeMap<u32, (Arc<Strong>, u64)>>,
}

struct InstallEvent {
    user: i32,
    package: String,
    kind: i32,
}

impl WriteParcelable for InstallEvent {
    fn write_to(&self, p: &mut Parcel) {
        // Bundle.putCharSequence stores this String with VAL_STRING.
        crate::bundle::write(
            p,
            &[
                (
                    "packageName",
                    crate::bundle::Value::String(Some(self.package.clone())),
                ),
                ("userId", crate::bundle::Value::Int(self.user)),
                ("installEventType", crate::bundle::Value::Int(self.kind)),
            ],
        );
    }
}

impl BackgroundInstallControl {
    pub(super) fn new(host: &Arc<AppHost>) -> Arc<Self> {
        let (handler, messages) = mpsc::channel();
        let (notifications, events) = mpsc::channel();
        let me = Arc::new(Self {
            host: Arc::downgrade(host),
            state: Mutex::default(),
            handler,
            notifications,
            callbacks: Mutex::default(),
        });
        let weak = Arc::downgrade(&me);
        std::thread::Builder::new()
            .name("background_install_control".into())
            .spawn(move || {
                for message in messages {
                    let Some(me) = weak.upgrade() else { break };
                    if let Err(e) = me.handle(message) {
                        eprintln!("{}", e.message);
                    }
                }
            })
            .expect("background install handler");
        let weak = Arc::downgrade(&me);
        std::thread::Builder::new()
            .name("background_install_callbacks".into())
            .spawn(move || {
                for event in events {
                    let Some(me) = weak.upgrade() else { break };
                    let callbacks: Vec<_> = me
                        .callbacks
                        .lock()
                        .unwrap()
                        .values()
                        .map(|(s, _)| s.clone())
                        .collect();
                    let mut p = Parcel::new();
                    callback::SendResult { data: Some(event) }.write(&mut p);
                    for callback in callbacks {
                        if let Err(e) = callback.transact(callback::SEND_RESULT, &p, true) {
                            eprintln!("background_install_control: callback failed: {e}");
                        }
                    }
                }
            })
            .expect("background install callback handler");
        me
    }

    fn post(&self, message: Message) -> Result<(), Exception> {
        self.handler.send(message).map_err(error)
    }

    pub(super) fn usage(&self, user: i32, event: Event) -> Result<(), Exception> {
        self.post(Message::Usage(user, event))
    }

    fn notify(&self, user: i32, package: &str, kind: i32) -> Result<(), Exception> {
        self.notifications
            .send(InstallEvent {
                user,
                package: package.into(),
                kind,
            })
            .map_err(error)
    }

    fn is_installer(&self, host: &AppHost, package: &str, user: i32) -> Result<bool, Exception> {
        host.permission_manager
            .get()
            .ok_or_else(|| error("no permissionmgr"))?
            .package_holds(package, INSTALL_PACKAGES, user)
    }

    fn handle(&self, message: Message) -> Result<(), Exception> {
        let host = self.host.upgrade().ok_or_else(|| error("app host gone"))?;
        match message {
            Message::Usage(user, event) => {
                if !matches!(
                    event.event_type,
                    ACTIVITY_RESUMED | ACTIVITY_PAUSED | ACTIVITY_STOPPED
                ) {
                    return Ok(());
                }
                let known = self
                    .state
                    .lock()
                    .unwrap()
                    .frames
                    .contains_key(&(user, event.package.clone()));
                if known || self.is_installer(&host, &event.package, user)? {
                    self.state.lock().unwrap().usage(user, &event);
                }
                Ok(())
            }
            Message::Added(user, name) => self.added(&host, user, &name),
            Message::Removed(user, name) => {
                let file = host.data()?.join(STATES);
                let mut state = self.state.lock().unwrap();
                let packages = state.load(&file)?;
                if let Some(names) = packages.get_mut(&user) {
                    if names.contains(&name) {
                        self.notify(user, &name, 2)?;
                    }
                    names.retain(|n| n != &name);
                    if names.is_empty() {
                        packages.remove(&user);
                    }
                }
                state.persist(&file)
            }
            #[cfg(test)]
            Message::Barrier(done) => done.send(()).map_err(error),
        }
    }

    fn added(&self, host: &AppHost, user: i32, name: &str) -> Result<(), Exception> {
        let (state, resolution) = host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID as i32,
        };
        if query
            .application_info(name, 0, user)
            .map_err(|e| error(e.0))??
            .is_none()
        {
            eprintln!("background_install_control: application not found: {name}");
            return Ok(());
        }
        // getInstallSourceInfo uses system_server's user (0), as in the original.
        let Some(source) = query
            .install_source_info(name, 0)
            .map_err(|e| error(e.0))??
        else {
            eprintln!("background_install_control: install source not found: {name}");
            return Ok(());
        };
        let Some(installer) = source.installing_package_name else {
            return Ok(());
        };
        if !self.is_installer(host, &installer, user)? {
            return Ok(());
        }
        // ApplicationInfo has no createTimestamp owner yet: its query writes 0.
        // Keep that absent-data fallback, rather than invent an install time.
        let start = match host.package_system()?.internal_get_historical_sessions(
            user,
            SYSTEM_UID as i32,
            0,
        ) {
            Ok(sessions) => sessions
                .iter()
                .filter(|s| s.app_package_name.as_deref() == Some(name))
                .map(|s| s.created_millis)
                .max()
                .unwrap_or_else(|| {
                    eprintln!("background_install_control: no historical session for {name}, using createTimestamp");
                    0
                }),
            Err(e) => {
                eprintln!(
                    "background_install_control: historical sessions failed, using createTimestamp: {}",
                    e.message
                );
                0
            }
        };
        let timestamp =
            super::usage_stats::now_millis() - (super::clock_ms(libc::CLOCK_UPTIME_RAW) - start);
        let adb = host
            .property("debug.gts.transparency.bg-install-apps")
            .is_none_or(|s| s.is_empty())
            && source
                .initiating_package_name
                .as_deref()
                .is_none_or(|s| s == "com.android.shell");
        let mut state = self.state.lock().unwrap();
        if adb || state.foreground(&installer, user, timestamp) {
            return Ok(());
        }
        let file = host.data()?.join(STATES);
        insert(state.load(&file)?, user, name.into());
        self.notify(user, name, 1)?;
        state.persist(&file)
    }

    fn packages(
        &self,
        host: &AppHost,
        uid: i32,
        flags: i64,
        user: i32,
    ) -> Result<Vec<PackageInfo>, Exception> {
        if host.flag("android.app.bic_client")?
            && host.check_permission(Some(GET_PACKAGES), uid)? != 0
        {
            return Err(Exception::security(format!(
                "User is not permitted to call service: {GET_PACKAGES}"
            )));
        }
        let (state, resolution) = host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID as i32,
        };
        let mock = host
            .property("debug.transparency.bg-install-apps")
            .unwrap_or_default();
        if !mock.is_empty() && host.property("ro.debuggable").as_deref() == Some("1") {
            let mut out = Vec::new();
            for name in mock.split(',') {
                match query
                    .package_info(name, -1, 0x0002_0000 /* MATCH_ALL */, 0)
                    .map_err(|e| error(e.0))??
                {
                    Some(info) => out.push(info),
                    None => eprintln!("background_install_control: mock package not found: {name}"),
                }
            }
            return Ok(out);
        }
        let mut packages = query
            .installed_packages(flags, user)
            .map_err(|e| error(e.0))??;
        let mut state = self.state.lock().unwrap();
        let tracked = state.load(&host.data()?.join(STATES))?.get(&user);
        packages.retain(|p| {
            p.package_name
                .as_ref()
                .is_some_and(|n| tracked.is_some_and(|v| v.contains(n)))
        });
        Ok(packages)
    }

    fn unregister(&self, host: &AppHost, handle: u32) {
        if let Some((strong, cookie)) = self.callbacks.lock().unwrap().remove(&handle) {
            host.process.clear_death(&strong, cookie);
        }
    }

    fn register(&self, host: &Arc<AppHost>, handle: u32) {
        let strong = Arc::new(host.process.strong(handle));
        let weak = Arc::downgrade(host);
        let cookie = host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    if let Some(owner) = host.background_install_control.get() {
                        owner.unregister(&host, handle);
                    }
                }
            }),
        );
        let old = self
            .callbacks
            .lock()
            .unwrap()
            .insert(handle, (strong, cookie));
        if let Some((strong, cookie)) = old {
            host.process.clear_death(&strong, cookie);
        }
    }
}

impl PackageListObserver for BackgroundInstallControl {
    fn package_added(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.post(Message::Added(
            uid / PER_USER_RANGE as i32,
            name.ok_or_else(|| Exception::illegal_argument("package name absent"))?
                .into(),
        ))
    }
    fn package_changed(&self, _name: Option<&str>, _uid: i32) -> Result<(), Exception> {
        Ok(())
    }
    fn package_removed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.post(Message::Removed(
            uid / PER_USER_RANGE as i32,
            name.ok_or_else(|| Exception::illegal_argument("package name absent"))?
                .into(),
        ))
    }
}

impl Service for BackgroundInstallControl {
    fn descriptor(&self) -> &str {
        bic::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(host) = self.host.upgrade() else {
            return reply(Err(error("app host gone")));
        };
        match call.code {
            bic::GET_BACKGROUND_INSTALLED_PACKAGES => {
                let a = bic::GetBackgroundInstalledPackages::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.packages(&host, call.sender_euid as i32, a.flags, a.user_id)
                        .map(|items| {
                            let mut p = Parcel::new();
                            bic::write_get_background_installed_packages_reply(
                                &mut p,
                                Some(&Slice::new(
                                    host.process.clone(),
                                    "android.content.pm.PackageInfo",
                                    items,
                                )),
                            );
                            p
                        }),
                )
            }
            bic::REGISTER_BACKGROUND_INSTALL_CALLBACK
            | bic::UNREGISTER_BACKGROUND_INSTALL_CALLBACK => {
                let binder = if call.code == bic::REGISTER_BACKGROUND_INSTALL_CALLBACK {
                    bic::RegisterBackgroundInstallCallback::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .callback
                } else {
                    bic::UnregisterBackgroundInstallCallback::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .callback
                };
                let handle = match binder {
                    Some(Binder::Handle(handle)) => handle,
                    None => return reply(Err(Exception::new(EX_NULL_POINTER, "callback"))),
                    _ => return Err(BAD_VALUE),
                };
                if call.code == bic::REGISTER_BACKGROUND_INSTALL_CALLBACK {
                    self.register(&host, handle);
                } else {
                    self.unregister(&host, handle);
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests;
