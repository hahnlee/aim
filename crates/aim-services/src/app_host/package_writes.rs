//! PackageManager's enabled settings in the app host
//! (`PackageManagerService.setEnabledSettings` at `android-16.0.0_r1`):
//! each setting decided by the native write model
//! (`package::write::enabled`), the model's state replaced with the
//! change, user 0's `package-restrictions.xml` written by the native owner
//! (`package::owner::Store`), and `ACTION_PACKAGE_CHANGED` sent as
//! `BroadcastHelper.sendPackageChangedBroadcast` sends it, to registered
//! receivers and package monitors (`PackageMonitorCallbackHelper`), at
//! once unless `DONT_KILL_APP` defers it (`mPendingBroadcasts`). The
//! restrictions are written when the setting changes, not on the
//! original's write delay.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, LocalService, Strong};
use aim_binder_host::parcel::{
    Binder, DEAD_OBJECT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, StatusCode,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_iremotecallback as remote_callback;

use super::AppHost;
use super::kept::RawIntent;
use crate::bundle::{self, Value};
use crate::package::apps_filter::{NotModelled, SYSTEM_UID, app_id, uid, user_id};
use crate::package::info::user_state;
use crate::package::model::{PackageState, State};
use crate::package::owner::Store;
use crate::package::query::Query;
use crate::package::uri::Uri;
use crate::package::write::Enabled;
use crate::package::write::enabled::{Policy, Setting, decide_with_policy};
use crate::package::write::mutation::{Change, Plan};

/// `PackageManager.DONT_KILL_APP`.
const DONT_KILL_APP: i32 = 0x1;
/// `PackageManagerService.BROADCAST_DELAY` and
/// `BROADCAST_DELAY_DURING_STARTUP`, the latter in the first minute
/// (`mServiceStartWithDelay`).
const BROADCAST_DELAY: Duration = Duration::from_secs(1);
const BROADCAST_DELAY_DURING_STARTUP: Duration = Duration::from_secs(10);
const STARTUP: Duration = Duration::from_secs(60);
const ACTION_PACKAGE_CHANGED: &str = "android.intent.action.PACKAGE_CHANGED";
const EXTRA_CHANGED_COMPONENT_NAME: &str = "android.intent.extra.changed_component_name";
const EXTRA_CHANGED_COMPONENT_NAME_LIST: &str = "android.intent.extra.changed_component_name_list";
const EXTRA_DONT_KILL_APP: &str = "android.intent.extra.DONT_KILL_APP";
const EXTRA_UID: &str = "android.intent.extra.UID";
const EXTRA_USER_HANDLE: &str = "android.intent.extra.user_handle";
const EXTRA_VISIBILITY_ALLOW_LIST: &str = "android.intent.extra.VISIBILITY_ALLOW_LIST";
const EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT: &str =
    "android.content.pm.extra.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT";
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
const ACCESS_INSTANT_APPS: &str = "android.permission.ACCESS_INSTANT_APPS";
/// The aconfig flag `BroadcastHelper` follows.
const REDUCE_BROADCASTS: &str = "android.content.pm.reduce_broadcasts_for_component_state_changes";
const PLATFORM_PACKAGE: &str = "android";

type Job = Box<dyn FnOnce() + Send>;
/// `mPendingBroadcasts`: by user and package, the changed components.
type Pending = BTreeMap<(i32, String), Vec<String>>;
/// Each changed package's state, and the settings that change one.
type Decided = (BTreeMap<String, Enabled>, Vec<usize>);

/// A registered `IRemoteCallback` of a package monitor, with its user and
/// uid (`PackageMonitorCallbackHelper.RegisterUser`).
struct Monitor {
    binder: Binder,
    callback: Arc<Callback>,
    user: i32,
    uid: i32,
}

/// A monitor's callback: an app's, linked to its death, or one of the app
/// host's own services' (`RemoteCallbackList` takes either).
enum Callback {
    Remote(Arc<Strong>, u64),
    Local(LocalService),
}

impl Callback {
    fn transact(&self, code: u32, data: &Parcel) -> Result<(), StatusCode> {
        match self {
            Callback::Remote(strong, _) => strong.transact(code, data, true).map(drop),
            Callback::Local(service) => service.transact(code, data, true).map(drop),
        }
    }
}

pub(super) struct Writes {
    host: Arc<AppHost>,
    /// The pending broadcasts, and whether their sending is scheduled.
    pending: Mutex<(Pending, bool)>,
    started: Instant,
    /// `mHandler`, which sends the broadcasts in order.
    handler: Mutex<mpsc::Sender<Job>>,
    monitors: Mutex<Vec<Monitor>>,
}

impl Writes {
    pub(super) fn new(host: Arc<AppHost>) -> Arc<Writes> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("package-handler".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .expect("the package handler's thread");
        Arc::new(Writes {
            host,
            pending: Mutex::default(),
            started: Instant::now(),
            handler: Mutex::new(sender),
            monitors: Mutex::default(),
        })
    }

    fn post(&self, job: impl FnOnce() + Send + 'static) {
        if self.handler.lock().unwrap().send(Box::new(job)).is_err() {
            eprintln!("app host: the package handler stopped");
        }
    }

    /// `setEnabledSettings` of `settings` (of one user) by `call`'s caller.
    pub(super) fn set_enabled(
        self: &Arc<Self>,
        call: &Call<'_>,
        settings: &[Setting],
    ) -> Result<(), Exception> {
        let mut store = self.host.package_store.lock().unwrap();
        let writer = self.host.model_writer.lock().unwrap();
        let (before, resolution) = self.host.packages()?;
        let query = Query {
            state: &before,
            filter: &resolution.apps_filter,
            calling_uid: call.sender_euid as i32,
        };
        let (after, changed) = match decide(&query, settings, call.sender_pid, &|package| {
            self.protected(package)
        }) {
            Err(NotModelled(reason)) => {
                return Err(self
                    .host
                    .unimplemented_named("package", &format!("{reason} (#1425)")));
            }
            Ok(decided) => match decided? {
                Some(decided) => decided,
                // `UserManager.exists`: no such user, nothing to do.
                None => return Ok(()),
            },
        };
        if changed.is_empty() {
            return Ok(());
        }
        let user = settings[changed[0]].user;
        if store.is_none() {
            let state =
                |e: String| Exception::new(EX_ILLEGAL_STATE, format!("package restrictions: {e}"));
            *store = Some(
                Store::open(self.host.data()?, &[user as u32])
                    .map_err(state)?
                    .ok_or_else(|| state("no package state".into()))?,
            );
        }
        let (written, error) = persist(store.as_mut().expect("opened"), user, &after);
        self.host
            .user_states_changed(&before, Arc::new(applied(&before, user, &written)))?;
        drop(writer);
        drop(store);
        if let Some(e) = error {
            return Err(e);
        }
        let send_now = {
            let mut pending = self.pending.lock().unwrap();
            let (send_now, deferred) = partition(settings, &changed, &mut pending.0);
            if deferred && !pending.1 {
                pending.1 = true;
                let delay = if self.started.elapsed() > STARTUP {
                    BROADCAST_DELAY
                } else {
                    BROADCAST_DELAY_DURING_STARTUP
                };
                let me = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    me.send_pending();
                });
            }
            send_now
        };
        for (package, components) in send_now {
            let app_id = before.packages[&package].app_id;
            self.package_changed(&package, false, components, uid(user, app_id));
        }
        Ok(())
    }

    /// `ProtectedPackages.isPackageStateProtected` in the app host: the
    /// provisioning package; the packages of a device or profile owner are
    /// not modelled.
    fn protected(&self, package: &str) -> Result<Option<bool>, Exception> {
        if self.host.device_policies()?.owned() {
            return Ok(None);
        }
        let provisioning = self
            .host
            .inputs()?
            .packages
            .framework_string("config_deviceProvisioningPackage");
        Ok(Some(provisioning.as_deref() == Some(package)))
    }

    /// `InstallPackageHelper.sendPendingBroadcasts`: the deferred changes,
    /// each package's sent as not killing it.
    fn send_pending(self: &Arc<Self>) {
        let pending = {
            let mut pending = self.pending.lock().unwrap();
            pending.1 = false;
            std::mem::take(&mut pending.0)
        };
        let state = match self.host.packages() {
            Ok((state, _)) => state,
            Err(e) => {
                eprintln!("app host: pending package changes: {}", e.message);
                return;
            }
        };
        for ((user, package), components) in pending {
            let Some(ps) = state.packages.get(&package) else {
                continue;
            };
            self.package_changed(&package, true, components, uid(user, ps.app_id));
        }
    }

    /// `BroadcastHelper.sendPackageChangedBroadcast`: to the receivers of
    /// the package's user, and its package monitors.
    fn package_changed(
        self: &Arc<Self>,
        package: &str,
        dont_kill: bool,
        components: Vec<String>,
        package_uid: i32,
    ) {
        let (state, resolution) = match self.host.packages() {
            Ok(packages) => packages,
            Err(e) => {
                eprintln!("app host: {package} changed: {}", e.message);
                return;
            }
        };
        let Some(ps) = state.packages.get(package).filter(|ps| ps.pkg.is_some()) else {
            return;
        };
        let user = user_id(package_uid);
        let instant = user_state(ps, user).instant_app;
        let allow_list = if instant {
            None
        } else {
            resolution
                .apps_filter
                .visibility_allow_list(&state, ps, user)
        };
        let shared: Vec<String> = ps
            .shared_user
            .as_ref()
            .and_then(|name| state.shared_users.get(name))
            .map(|su| {
                su.packages
                    .iter()
                    .filter(|p| {
                        state
                            .packages
                            .get(*p)
                            .is_some_and(|ps| user_state(ps, user).installed)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let reduce = state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == REDUCE_BROADCASTS && *on);
        let not_exported = not_exported(ps, &components);
        let change = Changed {
            package: package.into(),
            dont_kill,
            components,
            uid: package_uid,
            user,
            instant,
            allow_list,
        };
        let me = self.clone();
        self.post(move || {
            for (components, target) in change.broadcasts(reduce, &not_exported, &shared) {
                me.send(&change, &components, target.as_deref());
            }
            me.notify_monitors(&change);
        });
    }

    /// `sendPackageChangedBroadcastWithPermissions` of `components`, to
    /// `target` or every package, and what `broadcastIntentLocked` does on
    /// it: the package's processes killed unless it says not to.
    fn send(&self, change: &Changed, components: &[String], target: Option<&str>) {
        let (flags, extras) = change.broadcast(components, target);
        let data = Uri::preferred_opaque("package", &change.package);
        let intent = RawIntent::with_data(
            Some(ACTION_PACKAGE_CHANGED),
            Some(&data),
            target,
            flags,
            &extras,
        );
        if !change.dont_kill {
            self.host
                .kill_package_processes(&change.package, change.uid as u32);
        }
        let required: Vec<String> = if change.instant {
            vec![ACCESS_INSTANT_APPS.into()]
        } else {
            Vec::new()
        };
        let allow_list = if change.instant {
            None
        } else {
            change.allow_list.as_deref()
        };
        self.host.broadcasts.send_from_package_manager(
            &self.host,
            intent,
            change.user,
            allow_list,
            &required,
        );
    }

    /// `PackageMonitorCallbackHelper.notifyPackageChanged`: each monitor
    /// of the user that may see the package is sent the intent.
    fn notify_monitors(&self, change: &Changed) {
        let mut request = Parcel::new();
        remote_callback::SendResult {
            data: Some(BundleOf(&change.monitor_result())),
        }
        .write(&mut request);
        let targets: Vec<(Binder, Arc<Callback>)> =
            self.monitors
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m.user == -1 || m.user == change.user)
                .filter(|m| {
                    change.allow_list.as_ref().is_none_or(|allowed| {
                        app_id(m.uid) == SYSTEM_UID || allowed.contains(&m.uid)
                    })
                })
                .map(|m| (m.binder, m.callback.clone()))
                .collect();
        for (binder, callback) in targets {
            if callback
                .transact(remote_callback::SEND_RESULT, &request)
                .err()
                == Some(DEAD_OBJECT)
            {
                self.unregister_monitor(Some(binder));
            }
        }
    }

    /// `registerPackageMonitorCallback`: the callback is kept for the
    /// user `handleIncomingUser` resolves, until it dies.
    pub(super) fn register_monitor(
        self: &Arc<Self>,
        call: &Call<'_>,
        callback: Option<Binder>,
        user: i32,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        let user = self.host.incoming_user(uid, user, false)?;
        let Some(binder) = callback else {
            return Err(Exception::new(EX_NULL_POINTER, "callback"));
        };
        let mut monitors = self.monitors.lock().unwrap();
        if monitors.iter().any(|m| m.binder == binder) {
            return Ok(());
        }
        let callback = match binder {
            Binder::Handle(handle) => {
                let strong = Arc::new(self.host.process.strong(handle));
                let me = Arc::downgrade(self);
                let death = self.host.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        if let Some(me) = me.upgrade() {
                            me.monitors.lock().unwrap().retain(|m| m.binder != binder);
                        }
                    }),
                );
                Callback::Remote(strong, death)
            }
            Binder::Local(ptr) => match self.host.process.local_service(ptr) {
                Some(service) => Callback::Local(service),
                None => return Err(Exception::new(EX_NULL_POINTER, "callback")),
            },
        };
        monitors.push(Monitor {
            binder,
            callback: Arc::new(callback),
            user,
            uid,
        });
        Ok(())
    }

    /// `unregisterPackageMonitorCallback`.
    pub(super) fn unregister_monitor(&self, callback: Option<Binder>) {
        let Some(binder) = callback else { return };
        let mut monitors = self.monitors.lock().unwrap();
        if let Some(at) = monitors.iter().position(|m| m.binder == binder)
            && let Callback::Remote(strong, death) = &*monitors.remove(at).callback
        {
            self.host.process.clear_death(strong, *death);
        }
    }
}

/// The checks and state changes of `settings` (of one user), each
/// package's state as the settings before it leave it: each changed
/// package's state and the settings that change one; `None` when the
/// user does not exist. `protected` is `isPackageStateProtected`, `None`
/// where not modelled.
fn decide(
    query: &Query<'_>,
    settings: &[Setting],
    calling_pid: i32,
    protected: &dyn Fn(&str) -> Result<Option<bool>, Exception>,
) -> Result<Result<Option<Decided>, Exception>, NotModelled> {
    let mut after: BTreeMap<String, Enabled> = BTreeMap::new();
    let mut changed = Vec::new();
    for (i, setting) in settings.iter().enumerate() {
        let policy = Policy {
            protected: match protected(&setting.package) {
                Ok(protected) => protected,
                Err(e) => return Ok(Err(e)),
            },
            shell_restricted: None,
            compressed_enabled: None,
            suspension_cleanup: false,
        };
        let current = |ps: &PackageState, user: i32| {
            after
                .get(&ps.name)
                .cloned()
                .unwrap_or_else(|| Enabled::of(ps, user))
        };
        let was = query
            .state
            .packages
            .get(&setting.package)
            .map(|ps| current(ps, setting.user));
        let enabled = match decide_with_policy(query, setting, calling_pid, &current, Some(policy))?
        {
            Ok(Some(enabled)) => enabled,
            Ok(None) => return Ok(Ok(None)),
            Err(e) => return Ok(Err(e)),
        };
        if was.as_ref() != Some(&enabled) {
            after.insert(setting.package.clone(), enabled);
            changed.push(i);
        }
    }
    Ok(Ok(Some((after, changed))))
}

/// Writes `after`, the packages' changed states in `user`, to its
/// `package-restrictions.xml` as `Settings.writePackageRestrictionsLPr`
/// writes them: the states written (a failed write may have written its
/// file), and the error.
fn persist(
    store: &mut Store,
    user: i32,
    after: &BTreeMap<String, Enabled>,
) -> (Vec<(String, Enabled)>, Option<Exception>) {
    let mut written = Vec::new();
    for (package, enabled) in after {
        if let Err(e) = store.commit_enabled(package, user as u32, enabled) {
            if e.committed {
                written.push((package.clone(), enabled.clone()));
            }
            return (
                written,
                Some(Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package restrictions: {}", e.message),
                )),
            );
        }
        written.push((package.clone(), enabled.clone()));
    }
    (written, None)
}

/// `before` with `written`, packages' states in `user`.
fn applied(before: &State, user: i32, written: &[(String, Enabled)]) -> State {
    let mut state = before.clone();
    for (package, enabled) in written {
        Plan {
            package: package.clone(),
            user: Some(user),
            change: Change::Enabled(enabled.clone()),
        }
        .apply(&mut state);
    }
    state
}

/// The changes of the settings `changed` to send now, by package, and
/// whether the others were added to `pending` (`mPendingBroadcasts`): a
/// component's class, or the package's name for its own state.
fn partition(
    settings: &[Setting],
    changed: &[usize],
    pending: &mut Pending,
) -> (Vec<(String, Vec<String>)>, bool) {
    let mut send_now: Vec<(String, Vec<String>)> = Vec::new();
    let mut deferred = false;
    for setting in changed.iter().map(|&i| &settings[i]) {
        let name = setting
            .class
            .clone()
            .unwrap_or_else(|| setting.package.clone());
        let key = (setting.user, setting.package.clone());
        if setting.flags & DONT_KILL_APP == 0 {
            let at = match send_now.iter().position(|(p, _)| *p == setting.package) {
                Some(at) => at,
                None => {
                    send_now.push((setting.package.clone(), Vec::new()));
                    send_now.len() - 1
                }
            };
            add(&mut send_now[at].1, name);
            // A pending one is purged: this one is sent now.
            pending.remove(&key);
        } else {
            add(pending.entry(key).or_default(), name);
            deferred = true;
        }
    }
    (send_now, deferred)
}

/// `ArrayList.add` unless `list.contains(name)`.
fn add(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

/// `BroadcastHelper.getAllNotExportedComponents`: those of `components`
/// that are receivers, providers, services or activities not exported.
fn not_exported(ps: &PackageState, components: &[String]) -> Vec<String> {
    let Some(pkg) = ps.pkg.as_deref() else {
        return Vec::new();
    };
    let mains = pkg
        .receivers
        .iter()
        .map(|c| &c.main)
        .chain(pkg.providers.iter().map(|c| &c.main))
        .chain(pkg.services.iter().map(|c| &c.main))
        .chain(pkg.activities.iter().map(|c| &c.main));
    let mut out = Vec::new();
    let mut remaining = components.len();
    for main in mains {
        if components.contains(&main.component.name) {
            if !main.exported {
                out.push(main.component.name.clone());
            }
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
    }
    out
}

/// A package's change, as `sendPackageChangedBroadcastInternal` takes it.
struct Changed {
    package: String,
    dont_kill: bool,
    components: Vec<String>,
    uid: i32,
    user: i32,
    /// An instant app's changes go to those that may see instant apps.
    instant: bool,
    /// The app ids that may see the package; none when all may.
    allow_list: Option<Vec<i32>>,
}

impl Changed {
    /// The components each broadcast names and its target package: the
    /// whole set to all, or, with `reduce` and only components changed,
    /// the not exported ones to the system, the package and its shared
    /// user's packages, the exported ones to all.
    fn broadcasts(
        &self,
        reduce: bool,
        not_exported: &[String],
        shared: &[String],
    ) -> Vec<(Vec<String>, Option<String>)> {
        if self.components.contains(&self.package) || !reduce {
            return vec![(self.components.clone(), None)];
        }
        let mut out = Vec::new();
        if !not_exported.is_empty() {
            if self.package != PLATFORM_PACKAGE {
                out.push((not_exported.to_vec(), Some(PLATFORM_PACKAGE.to_string())));
            }
            out.push((not_exported.to_vec(), Some(self.package.clone())));
            for p in shared.iter().filter(|p| **p != self.package) {
                out.push((not_exported.to_vec(), Some(p.clone())));
            }
        }
        let exported: Vec<String> = self
            .components
            .iter()
            .filter(|c| !not_exported.contains(c))
            .cloned()
            .collect();
        if !exported.is_empty() {
            out.push((exported, None));
        }
        out
    }

    /// The flags and extras of the broadcast naming `components` to
    /// `target` (`sendPackageChangedBroadcastWithPermissions`, then
    /// `doSendBroadcast`): only registered receivers unless the package's
    /// own state changed.
    fn broadcast(
        &self,
        components: &[String],
        target: Option<&str>,
    ) -> (i32, Vec<(&'static str, Value)>) {
        let mut flags = FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT;
        if !components.contains(&self.package) {
            flags |= FLAG_RECEIVER_REGISTERED_ONLY;
        }
        let mut extras = self.extras(components);
        if let (Some(allowed), Some(PLATFORM_PACKAGE)) = (&self.allow_list, target) {
            extras.push((
                EXTRA_VISIBILITY_ALLOW_LIST,
                Value::IntArray(allowed.clone()),
            ));
        }
        extras.push((EXTRA_USER_HANDLE, Value::Int(self.user)));
        (flags, extras)
    }

    /// `PackageMonitorCallbackHelper.invokeCallback`'s bundle: the intent
    /// of the change in its user.
    fn monitor_result(&self) -> [(&'static str, Value); 1] {
        let mut extras = self.extras(&self.components);
        extras.push((EXTRA_USER_HANDLE, Value::Int(self.user)));
        let data = Uri::preferred_opaque("package", &self.package);
        let intent =
            RawIntent::with_data(Some(ACTION_PACKAGE_CHANGED), Some(&data), None, 0, &extras);
        let mut bytes = Parcel::new();
        intent.write_to(&mut bytes);
        [(
            EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT,
            Value::Parcelable {
                class: "android.content.Intent".into(),
                data: bytes.data().to_vec(),
            },
        )]
    }

    /// The change's extras naming `components`.
    fn extras(&self, components: &[String]) -> Vec<(&'static str, Value)> {
        vec![
            (
                EXTRA_CHANGED_COMPONENT_NAME,
                Value::String(components.first().cloned()),
            ),
            (
                EXTRA_CHANGED_COMPONENT_NAME_LIST,
                Value::StringArray(components.iter().cloned().map(Some).collect()),
            ),
            (EXTRA_DONT_KILL_APP, Value::Bool(self.dont_kill)),
            (EXTRA_UID, Value::Int(self.uid)),
        ]
    }
}

/// A bundle of `entries`, as `Bundle.writeToParcel` writes it.
struct BundleOf<'a>(&'a [(&'a str, Value)]);

impl WriteParcelable for BundleOf<'_> {
    fn write_to(&self, p: &mut Parcel) {
        bundle::write(p, self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use aim_binder_host::parcel::{EX_SECURITY, Reader};

    use super::*;
    use crate::package::apps_filter::{AppsFilter, Config};
    use crate::package::model::{PackageUserState, User};
    use crate::package::pkg::{Activity, AndroidPackage};

    const APP: &str = "org.example.app";
    const OTHER: &str = "org.example.other";
    const MAIN: &str = "org.example.app.Main";
    const RECEIVER: &str = "org.example.app.Receiver";

    fn component(name: &str, exported: bool) -> Activity {
        let mut a = Activity::default();
        a.main.component.name = name.into();
        a.main.component.package_name = APP.into();
        a.main.exported = exported;
        a.main.enabled = true;
        a
    }

    fn package(name: &str, app_id: i32) -> PackageState {
        let mut ps = PackageState {
            name: name.into(),
            app_id,
            target_sdk_version: 35,
            ..PackageState::default()
        };
        ps.pkg = Some(Arc::new(AndroidPackage {
            package_name: name.into(),
            target_sdk_version: 35,
            activities: vec![component(MAIN, true)],
            receivers: vec![component(RECEIVER, false)],
            ..AndroidPackage::default()
        }));
        ps.users.insert(
            0,
            PackageUserState {
                installed: true,
                ..PackageUserState::default()
            },
        );
        ps
    }

    fn state() -> State {
        State {
            packages: [package(APP, 10100), package(OTHER, 10101)]
                .into_iter()
                .map(|p| (p.name.clone(), p))
                .collect(),
            users: BTreeMap::from([(
                0,
                User {
                    id: 0,
                    unlocking_or_unlocked: true,
                    ..User::default()
                },
            )]),
            ..State::default()
        }
    }

    fn setting(class: Option<&str>, new_state: i32, flags: i32) -> Setting {
        Setting {
            package: APP.into(),
            class: class.map(Into::into),
            new_state,
            flags,
            user: 0,
            calling_package: APP.into(),
        }
    }

    fn decided(
        state: &State,
        uid: i32,
        settings: &[Setting],
        protected: Option<bool>,
    ) -> Result<Result<Option<Decided>, Exception>, NotModelled> {
        let filter = AppsFilter::new(state, &Config::default()).unwrap();
        let query = Query {
            state,
            filter: &filter,
            calling_uid: uid,
        };
        decide(&query, settings, 77, &|_| Ok(protected))
    }

    /// `setEnabledSettings`: an app changes its own components; another
    /// without CHANGE_COMPONENT_ENABLED_STATE may not, and one with it may
    /// not change a protected package.
    #[test]
    fn an_app_changes_its_own_components_only() {
        let state = state();
        let settings = [
            setting(Some(RECEIVER), 2, DONT_KILL_APP),
            setting(Some(MAIN), 0, 0),
        ];
        let (after, changed) = decided(&state, 10100, &settings, None)
            .unwrap()
            .unwrap()
            .unwrap();
        // Restoring the default of a component in neither list changes nothing.
        assert_eq!(changed, [0]);
        assert_eq!(
            after[APP].disabled_components,
            [RECEIVER.to_string()].into()
        );
        let e = decided(&state, 10101, &settings, Some(false))
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert_eq!(
            e.message,
            format!(
                "Attempt to change component state; pid=77, uid=10101, component=ComponentInfo{{{APP}/{RECEIVER}}}"
            )
        );
        // One that holds the permission and sees the package (`<queries>`).
        let mut granted = state.clone();
        let other = granted.packages.get_mut(OTHER).unwrap();
        other.users.get_mut(&0).unwrap().granted_permissions =
            vec!["android.permission.CHANGE_COMPONENT_ENABLED_STATE".into()];
        Arc::make_mut(other.pkg.as_mut().unwrap()).queries_packages = vec![APP.into()];
        let e = decided(&granted, 10101, &settings, Some(true))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            e.message,
            format!("Cannot disable a protected package: {APP}")
        );
        assert!(decided(&granted, 10101, &settings, None).is_err());
        assert_eq!(
            decided(&granted, 10101, &settings, Some(false))
                .unwrap()
                .unwrap()
                .unwrap()
                .1,
            [0]
        );
        // A user that does not exist: nothing to do.
        let mut absent = setting(Some(RECEIVER), 2, 0);
        absent.user = 10;
        assert!(
            decided(&state, 10100, &[absent], None)
                .unwrap()
                .unwrap()
                .is_none()
        );
    }

    /// A disposable `/data` with the package's settings and user 0's
    /// restrictions.
    struct Data(PathBuf);

    impl Data {
        fn new() -> Data {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-package-writes-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(path.join("system/users/0")).unwrap();
            std::fs::write(
                path.join("system/packages.xml"),
                format!("<packages><package name='{APP}' codePath='/data/app/example' userId='10100' it='12' /></packages>"),
            )
            .unwrap();
            std::fs::write(
                path.join("system/users/0/package-restrictions.xml"),
                format!("<package-restrictions><pkg name='{APP}' stopped='true' inst='true' /></package-restrictions>"),
            )
            .unwrap();
            Data(path)
        }
    }

    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// The change is written to `package-restrictions.xml` (ABX) as the
    /// original writes it, and the model's state takes it.
    #[test]
    fn a_change_is_persisted_and_applied() {
        let data = Data::new();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        let enabled = Enabled {
            disabled_components: [RECEIVER.to_string()].into(),
            ..Enabled::default()
        };
        let after = BTreeMap::from([(APP.to_string(), enabled.clone())]);
        let (written, error) = persist(&mut store, 0, &after);
        assert!(error.is_none());
        assert_eq!(written, [(APP.to_string(), enabled)]);
        let path = data.0.join("system/users/0/package-restrictions.xml");
        assert!(
            std::fs::read(&path)
                .unwrap()
                .starts_with(aim_android_xml::abx::MAGIC)
        );
        let reread = crate::package::State::read(&data.0, &[0]).unwrap().unwrap();
        let user = &reread.users[0].1.restrictions.packages[0].1;
        assert!(user.stopped && user.installed);
        assert_eq!(
            user.disabled_components.as_deref(),
            Some([RECEIVER.to_string()].as_slice())
        );
        let before = state();
        let applied = applied(&before, 0, &written);
        assert_eq!(
            user_state(&applied.packages[APP], 0).disabled_components,
            [RECEIVER.to_string()]
        );
        assert_eq!(applied.packages[OTHER], before.packages[OTHER]);
    }

    /// `DONT_KILL_APP` defers a change; one without it is sent now and
    /// purges the package's deferred one.
    #[test]
    fn dont_kill_app_defers_the_broadcast() {
        let settings = [
            setting(Some(RECEIVER), 2, DONT_KILL_APP),
            setting(Some(MAIN), 2, 0),
            setting(None, 2, 0),
        ];
        let mut pending = BTreeMap::new();
        let (now, deferred) = partition(&settings, &[0], &mut pending);
        assert!(now.is_empty() && deferred);
        assert_eq!(pending[&(0, APP.to_string())], [RECEIVER]);
        let (now, deferred) = partition(&settings, &[1, 2], &mut pending);
        assert!(!deferred && pending.is_empty());
        assert_eq!(
            now,
            [(APP.to_string(), vec![MAIN.to_string(), APP.to_string()])]
        );
    }

    fn changed(components: &[&str], allow_list: Option<Vec<i32>>) -> Changed {
        Changed {
            package: APP.into(),
            dont_kill: true,
            components: components.iter().map(|c| c.to_string()).collect(),
            uid: 10100,
            user: 0,
            instant: false,
            allow_list,
        }
    }

    /// `sendPackageChangedBroadcastInternal`: a whole package's change to
    /// all; with the reduced broadcasts, not exported components to the
    /// system, the package and its shared user's, exported ones to all.
    #[test]
    fn changes_go_to_whom_they_concern() {
        let c = changed(&[RECEIVER, MAIN], Some(vec![10100]));
        let not_exported = not_exported(&state().packages[APP], &c.components);
        assert_eq!(not_exported, [RECEIVER]);
        let shared = [APP.to_string(), "org.example.shared".to_string()];
        assert_eq!(
            c.broadcasts(false, &not_exported, &shared),
            [(c.components.clone(), None)]
        );
        let to = |p: &str| Some(p.to_string());
        assert_eq!(
            c.broadcasts(true, &not_exported, &shared),
            [
                (vec![RECEIVER.to_string()], to("android")),
                (vec![RECEIVER.to_string()], to(APP)),
                (vec![RECEIVER.to_string()], to("org.example.shared")),
                (vec![MAIN.to_string()], None),
            ]
        );
        let whole = changed(&[APP], None);
        assert_eq!(
            whole.broadcasts(true, &[], &[]),
            [(vec![APP.to_string()], None)]
        );
    }

    /// `sendPackageChangedBroadcastWithPermissions`' extras, the visibility
    /// allow list only to the system, and the flags.
    #[test]
    fn a_change_carries_the_original_extras() {
        let c = changed(&[RECEIVER, MAIN], Some(vec![10100]));
        let (flags, extras) = c.broadcast(&c.components, Some("android"));
        assert_eq!(
            flags,
            FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT
        );
        assert_eq!(
            extras,
            [
                (
                    EXTRA_CHANGED_COMPONENT_NAME,
                    Value::String(Some(RECEIVER.into()))
                ),
                (
                    EXTRA_CHANGED_COMPONENT_NAME_LIST,
                    Value::StringArray(vec![Some(RECEIVER.into()), Some(MAIN.into())])
                ),
                (EXTRA_DONT_KILL_APP, Value::Bool(true)),
                (EXTRA_UID, Value::Int(10100)),
                (EXTRA_VISIBILITY_ALLOW_LIST, Value::IntArray(vec![10100])),
                (EXTRA_USER_HANDLE, Value::Int(0)),
            ]
        );
        let (_, extras) = c.broadcast(&c.components, None);
        assert!(
            !extras
                .iter()
                .any(|(k, _)| *k == EXTRA_VISIBILITY_ALLOW_LIST)
        );
        let whole = changed(&[APP], None);
        assert_eq!(
            whole.broadcast(&whole.components, None).0,
            FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT
        );
    }

    /// A package monitor is sent a bundle holding the change's intent.
    #[test]
    fn a_monitor_is_sent_the_intent() {
        let c = changed(&[MAIN], None);
        let mut p = Parcel::new();
        bundle::write(&mut p, &c.monitor_result());
        let read = bundle::read(&mut Reader::new(p.data(), &[]))
            .unwrap()
            .unwrap();
        let Some(Value::Lazy {
            kind: 4,
            start,
            len,
        }) = read.get(EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT).cloned()
        else {
            panic!("no parcelable");
        };
        let mut r = Reader::new(&p.data()[start..start + len], &[]);
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.content.Intent")
        );
        let intent =
            crate::package::intent::Intent::read(&mut r, &mut crate::package::intent_filter::Plain)
                .unwrap();
        assert_eq!(intent.action.as_deref(), Some(ACTION_PACKAGE_CHANGED));
        assert_eq!(
            intent.data.as_ref().map(|d| d.as_str()),
            Some("package:org.example.app")
        );
        // The bundle says it holds an intent.
        assert_eq!(p.data()[p.data().len() - 4..], 1i32.to_le_bytes());
    }

    /// `getVisibilityAllowList`: the app ids that see the package.
    #[test]
    fn the_allow_list_is_who_sees_the_package() {
        let state = state();
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        assert_eq!(
            filter.visibility_allow_list(&state, &state.packages[APP], 0),
            Some(vec![10100])
        );
    }
}
