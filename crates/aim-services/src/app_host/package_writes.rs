//! PackageManager's enabled effects in the app host
//! (`PackageManagerService.setEnabledSettings` at `android-16.0.0_r1`):
//! settings committed and published by the native session, then
//! `ACTION_PACKAGE_CHANGED` sent as
//! `BroadcastHelper.sendPackageChangedBroadcast` sends it, to registered
//! receivers and package monitors (`PackageMonitorCallbackHelper`), at
//! once unless `DONT_KILL_APP` defers it (`mPendingBroadcasts`). The
//! restrictions are written when the setting changes, not on the
//! original's write delay. The app host's own services' package monitors
//! (`PackageMonitor.register`, which the system server's services call)
//! get the change before the apps' callbacks, as `doHandlePackageEvent`
//! decodes it. A package's data cleared (`clear_data`) is sent as
//! `sendPackageRestartedBroadcast` and `sendPackageDataClearedBroadcast`
//! send it, its `ACTION_PACKAGE_DATA_CLEARED` also to the services that
//! receive it in the system server. The removal leaf also sends the package
//! removal intents and notifies the native discovery monitors once.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::kept::RawIntent;
use super::{AppHost, PER_USER_RANGE, USER_ALL};
use crate::bundle::{self, Value};
use crate::package::apps_filter::{app_id, uid, user_id};
use crate::package::info::user_state;
use crate::package::model::PackageState;
use crate::package::uri::Uri;

/// `PackageManagerService.BROADCAST_DELAY` and
/// `BROADCAST_DELAY_DURING_STARTUP`, the latter in the first minute
/// (`mServiceStartWithDelay`).
const BROADCAST_DELAY: Duration = Duration::from_secs(1);
const BROADCAST_DELAY_DURING_STARTUP: Duration = Duration::from_secs(10);
const STARTUP: Duration = Duration::from_secs(60);
const ACTION_PACKAGE_CHANGED: &str = "android.intent.action.PACKAGE_CHANGED";
const ACTION_PACKAGE_RESTARTED: &str = "android.intent.action.PACKAGE_RESTARTED";
const ACTION_PACKAGE_DATA_CLEARED: &str = "android.intent.action.PACKAGE_DATA_CLEARED";
const EXTRA_CHANGED_COMPONENT_NAME: &str = "android.intent.extra.changed_component_name";
const EXTRA_CHANGED_COMPONENT_NAME_LIST: &str = "android.intent.extra.changed_component_name_list";
const EXTRA_DONT_KILL_APP: &str = "android.intent.extra.DONT_KILL_APP";
const EXTRA_UID: &str = "android.intent.extra.UID";
const EXTRA_TIME: &str = "android.intent.extra.TIME";
const EXTRA_IS_RESTORE: &str = "android.intent.extra.IS_RESTORE";
const EXTRA_PACKAGE_NAME: &str = "android.intent.extra.PACKAGE_NAME";
const EXTRA_USER_HANDLE: &str = "android.intent.extra.user_handle";
const EXTRA_VISIBILITY_ALLOW_LIST: &str = "android.intent.extra.VISIBILITY_ALLOW_LIST";
const EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT: &str =
    "android.content.pm.extra.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT";
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
const ACCESS_INSTANT_APPS: &str = "android.permission.ACCESS_INSTANT_APPS";
/// The aconfig flags `BroadcastHelper` and `sendPackageRestartedBroadcast`
/// follow.
const REDUCE_BROADCASTS: &str = "android.content.pm.reduce_broadcasts_for_component_state_changes";
const STAY_STOPPED: &str = "android.content.pm.stay_stopped";
const PLATFORM_PACKAGE: &str = "android";

/// `mPendingBroadcasts`: by user and package, the changed components.
type Pending = BTreeMap<(i32, String), Vec<String>>;

/// `ACTION_PACKAGE_CHANGED` as `PackageMonitor.doHandlePackageEvent`
/// reads it: the package, the changing user and the changed components
/// (`EXTRA_CHANGED_COMPONENT_NAME_LIST`).
pub(super) struct PackageChanged<'a> {
    pub package: &'a str,
    pub user: i32,
    pub components: &'a [String],
    pub removed: bool,
}

impl PackageChanged<'_> {
    /// `PackageMonitor.onPackageChanged`'s default, which decides
    /// `onSomePackagesChanged`: the package itself is among the changed
    /// components.
    pub fn some_packages_changed(&self) -> bool {
        self.components.iter().any(|c| c == self.package)
    }
}

/// `ACTION_PACKAGE_DATA_CLEARED` as its receivers read it: the package,
/// its uid (`EXTRA_UID`, -1 for a package with no application) and its
/// user (`EXTRA_USER_HANDLE`).
pub(super) struct DataCleared<'a> {
    pub package: &'a str,
    pub uid: i32,
    pub user: i32,
}

/// A package monitor of one of the app host's own services, or its
/// receiver of the package broadcasts (`registerReceiverAsUser` for all
/// users): an event it does not take is ignored, as `PackageMonitor`'s
/// callbacks do nothing by default.
pub(super) trait PackageMonitor: Send + Sync {
    fn package_changed(&self, _change: &PackageChanged<'_>) -> Result<(), Exception> {
        Ok(())
    }
    fn package_data_cleared(&self, _cleared: &DataCleared<'_>) {}
}

/// The app host's services' package monitors, each of a user or of all
/// (`USER_ALL`), in registration order (`PackageMonitorCallbackHelper`'s
/// `RemoteCallbackList`).
#[derive(Default)]
pub(super) struct NativeMonitors(Mutex<Vec<(i32, Weak<dyn PackageMonitor>)>>);

impl NativeMonitors {
    /// `PackageMonitor.register(context, null, user, true)`.
    pub(super) fn register(&self, user: i32, monitor: Weak<dyn PackageMonitor>) {
        self.0.lock().unwrap().push((user, monitor));
    }

    /// The monitors of `user`; those of dropped services are forgotten.
    fn of(&self, user: i32) -> Vec<Arc<dyn PackageMonitor>> {
        let mut monitors = self.0.lock().unwrap();
        monitors.retain(|(_, m)| m.strong_count() > 0);
        monitors
            .iter()
            .filter(|(u, _)| *u == USER_ALL || *u == user)
            .filter_map(|(_, m)| m.upgrade())
            .collect()
    }
}

pub(super) struct Writes {
    host: Arc<AppHost>,
    /// The pending broadcasts, and whether their sending is scheduled.
    pending: Mutex<(Pending, bool)>,
    started: Instant,
}

impl Writes {
    pub(super) fn new(host: Arc<AppHost>) -> Arc<Writes> {
        Arc::new(Writes {
            host,
            pending: Mutex::default(),
            started: Instant::now(),
        })
    }

    /// `InstallerRemovalBridge.sendRemovedBroadcast`: native discovery
    /// monitors and guest package broadcasts share this leaf.
    pub(super) fn removed(
        &self,
        package: &str,
        app_id: i32,
        user: i32,
        data_removed: bool,
        fully_removed: bool,
        uid_removed: bool,
    ) -> Result<(), Exception> {
        let uid = uid(user, app_id);
        let mut failure = self
            .host
            .notify_package_discoveries(&PackageChanged {
                package,
                user,
                components: &[package.to_owned()],
                removed: true,
            })
            .err();
        for intent in removed_intents(package, uid, data_removed, fully_removed, uid_removed) {
            if let Err(e) =
                self.host
                    .broadcasts
                    .try_send_from_package_manager(&self.host, intent.clone(), user)
            {
                failure.get_or_insert(e);
            }
            if let Err(e) = self.invoke_callbacks(&intent, user, None) {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// `mHandler.post`.
    pub(super) fn post(&self, job: impl FnOnce() + Send + 'static) -> Result<(), Exception> {
        self.host
            .package_system()?
            .package_events()?
            .post(false, Box::new(job))
    }

    /// `sendMessageDelayed(SEND_PENDING_BROADCAST, delay)` unless one is
    /// scheduled.
    fn schedule_pending(self: &Arc<Self>, pending: &mut (Pending, bool), delay: Duration) {
        if pending.1 {
            return;
        }
        pending.1 = true;
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            me.send_pending();
        });
    }

    /// The native writer has committed before admitting this effect.
    pub(super) fn changed(
        self: &Arc<Self>,
        package: &str,
        uid: i32,
        dont_kill: bool,
        components: Vec<String>,
        delayed: bool,
        reason: Option<&str>,
    ) -> Result<(), Exception> {
        self.host.package_system()?.package_events()?;
        let mut pending = self.pending.lock().unwrap();
        if delayed {
            let names = pending.0.entry((user_id(uid), package.into())).or_default();
            for name in components {
                add(names, name);
            }
            let delay = if reason == Some("reset_component_state_changed")
                || self.started.elapsed() > STARTUP
            {
                BROADCAST_DELAY
            } else {
                BROADCAST_DELAY_DURING_STARTUP
            };
            self.schedule_pending(&mut pending, delay);
        } else {
            pending.0.remove(&(user_id(uid), package.into()));
            drop(pending);
            self.package_changed(package, dont_kill, components, uid)?;
        }
        Ok(())
    }

    /// `sendPackageRestartedBroadcast` of `package`, whose processes of
    /// `uid` were force-stopped: to the receivers of the uid's user and the
    /// package monitors, from the handler as `stay_stopped` has it.
    pub(super) fn package_restarted(self: &Arc<Self>, package: &str, uid: i32) {
        let (state, resolution) = match self.host.packages() {
            Ok(packages) => packages,
            Err(e) => {
                eprintln!("app host: {package} restarted: {}", e.message);
                return;
            }
        };
        let user = user_id(uid);
        let allow_list = state
            .packages
            .get(package)
            .filter(|ps| ps.pkg.is_some())
            .and_then(|ps| {
                resolution
                    .apps_filter
                    .visibility_allow_list(&state, ps, user)
            });
        let stay_stopped = state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == STAY_STOPPED && *on);
        // `mProcessesReady`: the app host runs once the processes are.
        let intent = restarted_intent(package, uid, 0, stay_stopped);
        let me = self.clone();
        let monitor_intent = intent.clone();
        let monitor_allow_list = allow_list.clone();
        let send = move || {
            me.host.broadcasts.send_from_package_manager(
                &me.host,
                intent,
                user,
                allow_list.as_deref(),
                &[],
            );
        };
        if stay_stopped {
            if let Err(error) = self.post(send) {
                eprintln!("app host: package restarted: {}", error.message);
            }
        } else {
            send();
        }
        if let Err(error) =
            self.invoke_callbacks(&monitor_intent, user, monitor_allow_list.as_deref())
        {
            eprintln!("app host: package restarted monitors: {}", error.message);
        }
    }

    /// `sendPackageDataClearedBroadcast` of `package` (of `uid`, -1 for
    /// none) in `user`: to the receivers of the user, those of the system's
    /// services among them, and the package monitors.
    pub(super) fn package_data_cleared(
        self: &Arc<Self>,
        package: &str,
        uid: i32,
        user: i32,
        is_restore: bool,
        instant: bool,
    ) {
        let (state, resolution) = match self.host.packages() {
            Ok(packages) => packages,
            Err(e) => {
                eprintln!("app host: {package}'s data cleared: {}", e.message);
                return;
            }
        };
        let allow_list = state
            .packages
            .get(package)
            .filter(|ps| ps.pkg.is_some())
            .and_then(|ps| {
                resolution
                    .apps_filter
                    .visibility_allow_list(&state, ps, user)
            });
        let intent = data_cleared_intent(package, uid, user, is_restore, instant);
        let required: Vec<String> = if instant {
            vec![ACCESS_INSTANT_APPS.into()]
        } else {
            Vec::new()
        };
        self.host.broadcasts.send_from_package_manager(
            &self.host,
            intent.clone(),
            user,
            allow_list.as_deref(),
            &required,
        );
        let cleared = DataCleared { package, uid, user };
        for monitor in self.host.package_monitors.of(user) {
            monitor.package_data_cleared(&cleared);
        }
        if let Err(error) = self.invoke_callbacks(&intent, user, allow_list.as_deref()) {
            eprintln!("app host: data cleared monitors: {}", error.message);
        }
    }

    /// `ProtectedPackages.isPackageStateProtected` in the app host: the
    /// provisioning package; the packages of a device or profile owner are
    /// not modelled.
    pub(super) fn protected(&self, package: &str) -> Result<Option<bool>, Exception> {
        if self.host.device_policies()?.owned() {
            return Ok(None);
        }
        let provisioning = self
            .host
            .framework()?
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
            if let Err(error) =
                self.package_changed(&package, true, components, uid(user, ps.app_id))
            {
                eprintln!("app host: pending package change: {}", error.message);
            }
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
    ) -> Result<(), Exception> {
        let (state, resolution) = self.host.packages()?;
        let Some(ps) = state.packages.get(package).filter(|ps| ps.pkg.is_some()) else {
            return Ok(());
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
            if let Err(error) = me.notify_monitors(&change) {
                eprintln!("app host: package change monitors: {}", error.message);
            }
        })
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
    fn notify_monitors(&self, change: &Changed) -> Result<(), Exception> {
        // The system's own monitors may see every package.
        let changed = PackageChanged {
            package: &change.package,
            user: change.user,
            components: &change.components,
            removed: false,
        };
        let failure = self.host.notify_package_discoveries(&changed).err();
        self.invoke_callbacks(
            &change.monitor_intent(),
            change.user,
            change.allow_list.as_deref(),
        )?;
        failure.map_or(Ok(()), Err)
    }

    /// All public handles register with the session's native event registry.
    fn invoke_callbacks(
        &self,
        intent: &RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
    ) -> Result<(), Exception> {
        let bytes = callback_bundle(intent);
        let action = intent
            .intent
            .action
            .as_deref()
            .expect("package event action");
        self.host.package_system()?.package_events()?.notify(
            action,
            user,
            allow_list.map(<[i32]>::to_vec),
            Arc::new(move |_| {
                let mut payload = Parcel::new();
                payload.write_raw(&bytes, &[]);
                Ok(Some(payload))
            }),
        )
    }
}

/// The Java removal bridge's actions and extras, without PMS broadcast additions.
fn removed_intents(
    package: &str,
    uid: i32,
    data_removed: bool,
    fully_removed: bool,
    uid_removed: bool,
) -> Vec<RawIntent> {
    let data = Uri::preferred_opaque("package", package);
    let mut intents = vec![RawIntent::with_data(
        Some("android.intent.action.PACKAGE_REMOVED"),
        Some(&data),
        None,
        0,
        &[
            (EXTRA_UID, Value::Int(uid)),
            (
                "android.intent.extra.DATA_REMOVED",
                Value::Bool(data_removed),
            ),
            (EXTRA_DONT_KILL_APP, Value::Bool(false)),
            ("android.intent.extra.USER_INITIATED", Value::Bool(true)),
        ],
    )];
    if fully_removed && data_removed {
        intents.push(RawIntent::with_data(
            Some("android.intent.action.PACKAGE_FULLY_REMOVED"),
            Some(&data),
            None,
            0,
            &[
                (EXTRA_UID, Value::Int(uid)),
                ("android.intent.extra.DATA_REMOVED", Value::Bool(true)),
            ],
        ));
    }
    if uid_removed {
        intents.push(RawIntent::new(
            "android.intent.action.UID_REMOVED",
            0,
            &[(EXTRA_UID, Value::Int(uid))],
        ));
    }
    intents
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

    /// The intent the package monitors' callbacks get: the change in its
    /// user.
    fn monitor_intent(&self) -> RawIntent {
        let mut extras = self.extras(&self.components);
        extras.push((EXTRA_USER_HANDLE, Value::Int(self.user)));
        let data = Uri::preferred_opaque("package", &self.package);
        RawIntent::with_data(Some(ACTION_PACKAGE_CHANGED), Some(&data), None, 0, &extras)
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

/// `PackageMonitorCallbackHelper.invokeCallback`'s bundle: `intent`.
fn callback_result(intent: &RawIntent) -> [(&'static str, Value); 1] {
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

/// `NativePackageMonitorPayload.serialize`: the callback's Intent, with
/// its uid translated to the receiving user, in the callback Bundle.
pub(super) fn monitor_result(
    action: Option<&str>,
    package: Option<&str>,
    user: i32,
    package_uid: i32,
    replacing: bool,
) -> Vec<u8> {
    let package_uid = if package_uid >= 0 && user_id(package_uid) != user {
        user.wrapping_mul(PER_USER_RANGE as i32)
            .wrapping_add(app_id(package_uid))
    } else {
        package_uid
    };
    let data = package.map(|p| Uri::preferred_opaque("package", p));
    let intent = RawIntent::with_data(
        action,
        data.as_ref(),
        None,
        0,
        &[
            (EXTRA_UID, Value::Int(package_uid)),
            ("android.intent.extra.REPLACING", Value::Bool(replacing)),
            (EXTRA_USER_HANDLE, Value::Int(user)),
        ],
    );
    callback_bundle(&intent)
}

pub(super) fn callback_bundle(intent: &RawIntent) -> Vec<u8> {
    let mut p = Parcel::new();
    bundle::write(&mut p, &callback_result(intent));
    p.data().to_vec()
}

/// `PackageInternalEventsBridge.prepareRestarted` and
/// `PackageManagerService.sendPackageRestartedBroadcast`'s Intent.
pub(super) fn restarted_intent(
    package: &str,
    uid: i32,
    flags: i32,
    stay_stopped: bool,
) -> RawIntent {
    let mut extras = vec![
        (EXTRA_UID, Value::Int(uid)),
        (EXTRA_USER_HANDLE, Value::Int(user_id(uid))),
    ];
    if stay_stopped {
        extras.push((EXTRA_TIME, Value::Long(super::alarm_service::elapsed_ms())));
    }
    RawIntent::with_data(
        Some(ACTION_PACKAGE_RESTARTED),
        Some(&Uri::preferred_opaque("package", package)),
        None,
        flags | FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT,
        &extras,
    )
}

/// `PackageInternalEventsBridge.prepareDataCleared` and
/// `PackageManagerService.sendPackageDataClearedBroadcast`'s Intent.
pub(super) fn data_cleared_intent(
    package: &str,
    uid: i32,
    user: i32,
    restore: bool,
    instant: bool,
) -> RawIntent {
    let mut extras = vec![
        (EXTRA_UID, Value::Int(uid)),
        (EXTRA_USER_HANDLE, Value::Int(user)),
    ];
    if restore {
        extras.push((EXTRA_IS_RESTORE, Value::Bool(true)));
    }
    if instant {
        extras.push((EXTRA_PACKAGE_NAME, Value::String(Some(package.into()))));
    }
    RawIntent::with_data(
        Some(ACTION_PACKAGE_DATA_CLEARED),
        Some(&Uri::preferred_opaque("package", package)),
        None,
        FLAG_RECEIVER_INCLUDE_BACKGROUND | FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT,
        &extras,
    )
}

#[cfg(test)]
mod tests {
    use aim_binder_host::parcel::Reader;

    use super::*;
    use crate::package::apps_filter::{AppsFilter, Config};
    use crate::package::model::{PackageUserState, State, User};
    use crate::package::pkg::{Activity, AndroidPackage};

    const APP: &str = "org.example.app";
    const OTHER: &str = "org.example.other";
    const MAIN: &str = "org.example.app.Main";
    const RECEIVER: &str = "org.example.app.Receiver";

    #[test]
    fn removal_intents_follow_the_java_actions_data_and_extras() {
        let intents = removed_intents(APP, 10100, true, true, true);
        assert_eq!(
            intents
                .iter()
                .map(|i| i.intent.action.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [
                "android.intent.action.PACKAGE_REMOVED",
                "android.intent.action.PACKAGE_FULLY_REMOVED",
                "android.intent.action.UID_REMOVED"
            ]
        );
        assert_eq!(
            intents[0].intent.data.as_ref().unwrap().as_str(),
            "package:org.example.app"
        );
        assert_eq!(intents[1].intent.data, intents[0].intent.data);
        assert!(intents[2].intent.data.is_none());
        let extras = intents[0].test_extras();
        assert_eq!(extras.len(), 4);
        assert_eq!(extras[EXTRA_UID], Value::Int(10100));
        assert_eq!(
            extras["android.intent.extra.DATA_REMOVED"],
            Value::Bool(true)
        );
        assert_eq!(extras[EXTRA_DONT_KILL_APP], Value::Bool(false));
        assert_eq!(
            extras["android.intent.extra.USER_INITIATED"],
            Value::Bool(true)
        );
        assert_eq!(intents[1].test_extras().len(), 2);
        assert_eq!(intents[2].test_extras().len(), 1);
        assert_eq!(removed_intents(APP, 10100, false, true, false).len(), 1);
        assert_eq!(removed_intents(APP, 10100, true, false, false).len(), 1);
    }

    struct RemovalSeen(Mutex<Vec<(String, i32, bool)>>);
    impl PackageMonitor for RemovalSeen {
        fn package_changed(&self, change: &PackageChanged<'_>) -> Result<(), Exception> {
            self.0
                .lock()
                .unwrap()
                .push((change.package.into(), change.user, change.removed));
            Ok(())
        }
    }
    struct RemovalCallback(std::sync::mpsc::Sender<RawIntent>);
    impl aim_binder_host::local::Service for RemovalCallback {
        fn descriptor(&self) -> &str {
            "android.os.IRemoteCallback"
        }
        fn transact(
            &self,
            call: &mut aim_binder_host::local::Call<'_>,
        ) -> aim_binder_host::local::Reply {
            call.data.enforce_interface(self.descriptor())?;
            assert_eq!(call.data.read_i32()?, 1);
            let values = bundle::read(&mut call.data)?.unwrap();
            let Value::Lazy { start, .. } = values[EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT] else {
                panic!("intent callback");
            };
            call.data.set_position(start);
            assert_eq!(
                call.data.read_string16()?.as_deref(),
                Some("android.content.Intent")
            );
            self.0
                .send(aim_service_aidl::ReadParcelable::read_from(&mut call.data)?)
                .unwrap();
            Ok(Parcel::new())
        }
    }

    #[test]
    fn removal_notifies_native_and_callback_monitors_once() {
        crate::system::with_discovery_session(|host, _, _| {
            let native = Arc::new(RemovalSeen(Mutex::default()));
            host.package_monitors.register(
                0,
                Arc::downgrade(&(native.clone() as Arc<dyn PackageMonitor>)),
            );
            let (send, receive) = std::sync::mpsc::channel();
            let callback = host.process.add_service(Arc::new(RemovalCallback(send)));
            let events = host.package_system().unwrap().package_events().unwrap();
            events.register(Some(callback), 0, 1000).unwrap();
            Writes::new(host.clone())
                .removed("p", 10100, 0, true, true, true)
                .unwrap();
            assert!(events.wait(2000, false).unwrap());
            assert_eq!(*native.0.lock().unwrap(), [("p".into(), 0, true)]);
            let actions: Vec<_> = receive
                .try_iter()
                .map(|i| i.intent.action.unwrap())
                .collect();
            assert_eq!(
                actions,
                [
                    "android.intent.action.PACKAGE_REMOVED",
                    "android.intent.action.UID_REMOVED"
                ]
            );
        });
    }

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
        bundle::write(&mut p, &callback_result(&c.monitor_intent()));
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

    struct Seen(Mutex<Vec<(String, i32, bool)>>);

    impl PackageMonitor for Seen {
        fn package_changed(&self, change: &PackageChanged<'_>) -> Result<(), Exception> {
            self.0.lock().unwrap().push((
                change.package.into(),
                change.user,
                change.some_packages_changed(),
            ));
            Ok(())
        }
    }

    /// The services' monitors of the changing user or of all, in
    /// registration order; a dropped service's is forgotten.
    #[test]
    fn native_monitors_of_the_user_get_the_change() {
        let monitors = NativeMonitors::default();
        let all = Arc::new(Seen(Mutex::default()));
        let ten = Arc::new(Seen(Mutex::default()));
        let gone = Arc::new(Seen(Mutex::default()));
        monitors.register(
            USER_ALL,
            Arc::downgrade(&(all.clone() as Arc<dyn PackageMonitor>)),
        );
        monitors.register(
            10,
            Arc::downgrade(&(ten.clone() as Arc<dyn PackageMonitor>)),
        );
        monitors.register(
            USER_ALL,
            Arc::downgrade(&(gone.clone() as Arc<dyn PackageMonitor>)),
        );
        drop(gone);
        let components = vec!["org.example.app".to_string()];
        let changed = PackageChanged {
            package: "org.example.app",
            user: 0,
            components: &components,
            removed: false,
        };
        let of_user_0 = monitors.of(0);
        assert_eq!(of_user_0.len(), 1);
        for m in of_user_0 {
            m.package_changed(&changed).unwrap();
        }
        assert_eq!(
            *all.0.lock().unwrap(),
            [("org.example.app".to_string(), 0, true)]
        );
        assert!(ten.0.lock().unwrap().is_empty());
        assert_eq!(monitors.of(10).len(), 2);
        assert_eq!(monitors.0.lock().unwrap().len(), 2);
    }

    /// `onPackageChanged`'s default: only the whole package's change is
    /// "some packages changed".
    #[test]
    fn a_component_change_is_not_the_package_changing() {
        let components = vec!["org.example.app.Service".to_string()];
        let changed = PackageChanged {
            package: "org.example.app",
            user: 0,
            components: &components,
            removed: false,
        };
        assert!(!changed.some_packages_changed());
    }
}

impl AppHost {
    pub(super) fn notify_package_discoveries(
        &self,
        change: &PackageChanged<'_>,
    ) -> Result<(), Exception> {
        let mut failure = self.refresh_audio_permissions().err();
        for monitor in self.package_monitors.of(change.user) {
            if let Err(error) = monitor.package_changed(change) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
