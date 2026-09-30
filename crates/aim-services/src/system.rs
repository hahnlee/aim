//! The original system services a native one consults, as their client:
//! found through servicemanager, called with the pinned AIDL's codes.
//!
//! These are the binder equivalents of what the original service asks
//! other services in its process; where the original uses a
//! system_server-internal API that has none, the method says what it
//! stands in for. What a focused app's access reads (package ownership,
//! app-op modes, focus, whether the device is locked) is mirrored
//! ([`crate::mirror`]), each fed by its owner's listener; noting an app op,
//! which only records the access, is sent in the background. Permission
//! checks are kept as apps' `PermissionManager` keeps them, by the
//! `package_info_cache` nonce the system_server bridge hands over
//! ([`crate::nonces`]); an instrumentation target's app ops are asked each
//! time (docs/system-services.md, "Mirrored state").

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{
    ReadParcelable, Returned, WriteParcelable, android_app_iactivitymanager as am,
    android_app_iactivitytaskmanager as atm, android_app_itaskstacklistener as task_listener,
    android_app_iurigrantsmanager as ugm, android_app_trust_itrustmanager as trust,
    android_content_pm_ipackagemanager as package, android_os_iremotecallback as remote_callback,
    android_os_iservicemanager as sm, android_os_iusermanager as um,
    android_permission_ipermissionmanager as pm,
    com_android_internal_app_iappopscallback as ops_callback,
    com_android_internal_app_iappopsservice as appops,
    com_android_internal_policy_idevicelockedstatelistener as lock_listener,
    dev_aim_server_ibridge as bridge,
};

use crate::mirror::Mirror;
use crate::nonces::Nonces;

/// The nonce of `PermissionManager`'s caches (`getPackageInfoCacheKey`
/// with `pic_separate_permission_notifications`, on in this image).
const PACKAGE_INFO_NONCE: &str = "package_info_cache";
/// `PackageManager.PERMISSION_GRANTED`.
const PERMISSION_GRANTED: i32 = 0;
/// `VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT`.
const PERSISTENT_DEVICE_ID_DEFAULT: &str = "default:0";
/// `AppOpsManager.MODE_*`.
pub const MODE_ALLOWED: i32 = 0;
pub const MODE_ERRORED: i32 = 2;
/// `AppOpsManager.WATCH_FOREGROUND_CHANGES`: also told when a uid's
/// foreground state changes an op in `MODE_FOREGROUND`.
const WATCH_FOREGROUND_CHANGES: i32 = 1;
/// `UserHandle.USER_ALL`, `USER_SYSTEM`.
const USER_ALL: i32 = -1;
const USER_SYSTEM: i32 = 0;
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
/// `UserHandle.PER_USER_RANGE`; `Process.FIRST_APPLICATION_UID`,
/// `FIRST_SDK_SANDBOX_UID`, `LAST_SDK_SANDBOX_UID`.
const PER_USER_RANGE: i32 = 100_000;
const FIRST_APPLICATION_UID: i32 = 10_000;
const FIRST_SDK_SANDBOX_UID: i32 = 20_000;
const LAST_SDK_SANDBOX_UID: i32 = 29_999;
/// `ActivityManagerService.STOCK_PM_FLAGS`, what `startInstrumentation`
/// looks instrumentations up with.
const STOCK_PM_FLAGS: i32 = 1 << 10;

/// A failure to reach a service, as the exception a Java caller would
/// see once system_server rethrew it.
fn unreachable_service(what: &str, status: i32) -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        format!("{what}: binder status {status}"),
    )
}

/// An app op to note: `(op, uid, package, attribution tag)`.
type Note = (i32, i32, String, Option<String>);

pub struct System {
    process: Arc<LocalProcess>,
    services: Mutex<HashMap<&'static str, Arc<Strong>>>,
    listeners: Listeners,
    /// `checkPackage`'s mode by (uid, package).
    packages: Mirror<(i32, String), i32>,
    /// `checkOperation`'s mode by (op, uid, package).
    modes: Mirror<(i32, i32, String), i32>,
    /// Whether an installed instrumentation targets the uid.
    instrumented: Mirror<i32, bool>,
    /// The focused root task's effective uid.
    focus: Mirror<(), Option<i32>>,
    /// Whether the system user's device is locked.
    locked: Mirror<(), bool>,
    notes: Sender<Note>,
    /// The nonces in system_server's shared memory, from the bridge.
    nonces: Mutex<Option<Arc<Nonces>>>,
    permissions: Mutex<Permissions>,
}

/// Permission checks kept with the nonce they were asked at, as
/// `PermissionManager`'s `sPermissionCache` (by permission and uid; the
/// pid does not decide) and `sPackageNamePermissionCache` keep them.
#[derive(Default)]
struct Permissions {
    nonce: i64,
    uids: HashMap<(String, i32), bool>,
    packages: HashMap<(String, String), bool>,
}

/// The listener nodes the mirrors are fed by.
struct Listeners {
    task_stack: Binder,
    /// One per watched op: a callback is registered for one op.
    ops: Vec<(i32, Binder)>,
    packages: Binder,
    locked: Binder,
}

type Result<T> = std::result::Result<T, Exception>;

impl System {
    /// `ops` are the app ops the service decides by.
    pub fn new(process: Arc<LocalProcess>, ops: &[i32]) -> Arc<Self> {
        let (notes, noted) = mpsc::channel::<Note>();
        let system = Arc::new_cyclic(|this: &Weak<Self>| {
            let node = |descriptor, fds, changed: fn(&System)| {
                process.add_service(Arc::new(Listener {
                    descriptor,
                    fds,
                    system: this.clone(),
                    changed,
                }))
            };
            let listeners = Listeners {
                // Its task snapshots carry a buffer's file descriptors.
                task_stack: node(task_listener::DESCRIPTOR, true, |s| s.focus.invalidate()),
                ops: ops
                    .iter()
                    .map(|&op| {
                        let binder =
                            node(ops_callback::DESCRIPTOR, false, |s| s.modes.invalidate());
                        (op, binder)
                    })
                    .collect(),
                // A package added, removed or changed: its uid, modes and
                // instrumentations with it.
                packages: node(remote_callback::DESCRIPTOR, false, |s| {
                    s.packages.invalidate();
                    s.modes.invalidate();
                    s.instrumented.invalidate();
                }),
                locked: node(lock_listener::DESCRIPTOR, false, |s| s.locked.invalidate()),
            };
            Self {
                process: process.clone(),
                services: Mutex::new(HashMap::new()),
                listeners,
                packages: Mirror::new(),
                modes: Mirror::new(),
                instrumented: Mirror::new(),
                focus: Mirror::new(),
                locked: Mirror::new(),
                notes,
                nonces: Mutex::new(None),
                permissions: Mutex::new(Permissions::default()),
            }
        });
        let this = Arc::downgrade(&system);
        let _ = std::thread::Builder::new()
            .name("appops-notes".into())
            .spawn(move || {
                for (op, uid, package, tag) in noted {
                    let Some(system) = this.upgrade() else { break };
                    if let Err(e) = system.note_op(op, uid, &package, tag.as_deref()) {
                        eprintln!(
                            "services: noteOperation({op}, {uid}, {package}): {}",
                            e.message
                        );
                    }
                }
            });
        system
    }

    /// `name` from servicemanager, kept until it dies.
    fn service(self: &Arc<Self>, name: &'static str) -> Result<Arc<Strong>> {
        if let Some(s) = self.services.lock().unwrap().get(name) {
            return Ok(s.clone());
        }
        let mut data = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::CHECK_SERVICE, &data, false)
            .map_err(|s| unreachable_service(name, s))?;
        let binder = sm::read_check_service_reply(&mut reply.reader())
            .map_err(|s| unreachable_service(name, s))??;
        let Some(aim_binder_host::parcel::Binder::Handle(handle)) = binder else {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("no {name} service"),
            ));
        };
        let strong = Arc::new(self.process.strong(handle));
        drop(reply);
        let this = Arc::downgrade(self);
        self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(system) = this.upgrade() {
                    system.services.lock().unwrap().remove(name);
                }
            }),
        );
        self.services.lock().unwrap().insert(name, strong.clone());
        Ok(strong)
    }

    /// Calls `code` of service `name` and reads its reply.
    fn call<T>(
        self: &Arc<Self>,
        name: &'static str,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Returned<T>>,
    ) -> Result<T> {
        let service = self.service(name)?;
        let mut data = Parcel::new();
        write(&mut data);
        let reply = service
            .transact(code, &data, false)
            .map_err(|s| unreachable_service(name, s))?;
        read(&mut reply.reader()).map_err(|s| unreachable_service(name, s))?
    }

    /// Registers a listener with service `name` (a call without result);
    /// `unwatch` runs when the service, and the registration with it, dies.
    fn watch(
        self: &Arc<Self>,
        name: &'static str,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Returned<()>>,
        unwatch: fn(&System),
    ) -> Result<()> {
        let service = self.service(name)?;
        let mut data = Parcel::new();
        write(&mut data);
        let reply = service
            .transact(code, &data, false)
            .map_err(|s| unreachable_service(name, s))?;
        read(&mut reply.reader()).map_err(|s| unreachable_service(name, s))??;
        // The instance registered with; one already dead is reported at once.
        let this = Arc::downgrade(self);
        self.process.link_to_death(
            &service,
            Box::new(move || {
                if let Some(system) = this.upgrade() {
                    unwatch(&system);
                }
            }),
        );
        Ok(())
    }

    /// Registers `binder` with servicemanager as `name`, as
    /// `SystemService.publishBinderService` does.
    pub fn add_service(&self, name: &str, binder: aim_binder_host::parcel::Binder) -> Result<()> {
        let mut data = Parcel::new();
        sm::AddService {
            name: Some(name.into()),
            service: Some(binder),
            allow_isolated: false,
            // IServiceManager.DUMP_FLAG_PRIORITY_DEFAULT
            dump_priority: 1 << 3,
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::ADD_SERVICE, &data, false)
            .map_err(|s| unreachable_service("servicemanager", s))?;
        sm::read_add_service_reply(&mut reply.reader())
            .map_err(|s| unreachable_service("servicemanager", s))?
    }

    /// `Context.checkPermission(permission, pid, uid)`.
    pub fn check_permission(
        self: &Arc<Self>,
        permission: &str,
        pid: i32,
        uid: i32,
    ) -> Result<bool> {
        let key = (permission.to_string(), uid);
        self.cached(
            |p| &mut p.uids,
            key,
            || {
                let args = am::CheckPermission {
                    permission: Some(permission.into()),
                    pid,
                    uid,
                };
                self.call(
                    "activity",
                    am::CHECK_PERMISSION,
                    |p| args.write(p),
                    am::read_check_permission_reply,
                )
                .map(|r| r == PERMISSION_GRANTED)
            },
        )
    }

    /// A permission check's answer: the one kept at the current
    /// `package_info_cache` nonce, else `ask`'s, kept with the nonce read
    /// before asking (`PropertyInvalidatedCache.query`). system_server
    /// bumps the nonce after a change, so a kept answer is never older
    /// than the last change. Without the bridge, or while the nonce is
    /// unset, every check is asked.
    fn cached<K: Eq + std::hash::Hash>(
        &self,
        table: fn(&mut Permissions) -> &mut HashMap<K, bool>,
        key: K,
        ask: impl FnOnce() -> Result<bool>,
    ) -> Result<bool> {
        let nonces = self.nonces.lock().unwrap().clone();
        let Some(nonce) = nonces.and_then(|n| n.get(PACKAGE_INFO_NONCE)) else {
            return ask();
        };
        {
            let mut kept = self.permissions.lock().unwrap();
            if kept.nonce != nonce {
                *kept = Permissions {
                    nonce,
                    ..Permissions::default()
                };
            }
            if let Some(&granted) = table(&mut kept).get(&key) {
                return Ok(granted);
            }
        }
        let granted = ask()?;
        let mut kept = self.permissions.lock().unwrap();
        if kept.nonce == nonce {
            table(&mut kept).insert(key, granted);
        }
        Ok(granted)
    }

    /// Takes system_server's bridge (#430): maps the shared memory of its
    /// cache nonces, dropped again when system_server dies.
    pub fn attach_bridge(self: &Arc<Self>, handle: u32) -> Result<()> {
        let strong = self.process.strong(handle);
        let mut data = Parcel::new();
        bridge::GetApplicationSharedMemory {}.write(&mut data);
        let failed = |s| unreachable_service("bridge", s);
        let reply = strong
            .transact(bridge::GET_APPLICATION_SHARED_MEMORY, &data, false)
            .map_err(failed)?;
        let fd = bridge::read_get_application_shared_memory_reply::<ParcelFileDescriptor>(
            &mut reply.reader(),
        )
        .map_err(failed)??
        .ok_or_else(|| failed(aim_binder_host::parcel::BAD_VALUE))?;
        let nonces = self
            .process
            .file(fd.0)
            .and_then(|file| Nonces::map(&file))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "the bridge's shared memory is not a nonce store",
                )
            })?;
        drop(reply);
        *self.nonces.lock().unwrap() = Some(Arc::new(nonces));
        let this = Arc::downgrade(self);
        self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(system) = this.upgrade() {
                    system.nonces.lock().unwrap().take();
                }
            }),
        );
        Ok(())
    }

    /// `PermissionEnforcer.enforcePermission(permission, pid, uid)`, what
    /// `@EnforcePermission` generates, for a permission without an app op.
    pub fn enforce_permission(
        self: &Arc<Self>,
        permission: &str,
        pid: i32,
        uid: i32,
    ) -> Result<()> {
        if self.check_permission(permission, pid, uid)? {
            Ok(())
        } else {
            Err(Exception::security(format!(
                "Access denied, requires: {permission}"
            )))
        }
    }

    /// `ActivityManagerInternal.handleIncomingUser`.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_incoming_user(
        self: &Arc<Self>,
        calling_pid: i32,
        calling_uid: i32,
        user_id: i32,
        require_full: bool,
        name: &str,
        caller_package: Option<&str>,
    ) -> Result<i32> {
        let args = am::HandleIncomingUser {
            calling_pid,
            calling_uid,
            user_id,
            allow_all: false,
            require_full,
            name: Some(name.into()),
            caller_package: caller_package.map(Into::into),
        };
        self.call(
            "activity",
            am::HANDLE_INCOMING_USER,
            |p| args.write(p),
            am::read_handle_incoming_user_reply,
        )
    }

    /// `IActivityManager.getContentProviderExternal(name, user, token,
    /// tag)`: the provider, held for `token`. Its `ProviderInfo` carries
    /// no binder, so the holder's first binder is the provider's.
    pub fn content_provider_external(
        self: &Arc<Self>,
        name: &str,
        user_id: i32,
        token: Binder,
        tag: &str,
    ) -> Result<Option<Strong>> {
        let service = self.service("activity")?;
        let mut data = Parcel::new();
        am::GetContentProviderExternal {
            name: Some(name.into()),
            user_id,
            token: Some(token),
            tag: Some(tag.into()),
        }
        .write(&mut data);
        let reply = service
            .transact(am::GET_CONTENT_PROVIDER_EXTERNAL, &data, false)
            .map_err(|s| unreachable_service("activity", s))?;
        let holder = am::read_get_content_provider_external_reply::<ContentProviderHolder>(
            &mut reply.reader(),
        )
        .map_err(|s| unreachable_service("activity", s))??;
        Ok(holder
            .and_then(|h| h.provider)
            .map(|handle| self.process.strong(handle)))
    }

    /// `UriGrantsManagerInternal.checkGrantUriPermission(uid, null, uri,
    /// modeFlags, userId)`, which throws unless `uid` may grant `uri`;
    /// its binder form answers system callers the same way.
    pub fn check_grant_uri_permission(
        self: &Arc<Self>,
        uid: i32,
        uri: &str,
        mode_flags: i32,
        user_id: i32,
    ) -> Result<()> {
        let args = ugm::CheckGrantUriPermissionIgnoreNonSystem {
            source_uid: uid,
            target_pkg: None,
            uri: Some(StringUri(uri)),
            mode_flags,
            user_id,
        };
        self.call(
            "uri_grants",
            ugm::CHECK_GRANT_URI_PERMISSION_IGNORE_NON_SYSTEM,
            |p| args.write(p),
            ugm::read_check_grant_uri_permission_ignore_non_system_reply,
        )
        .map(drop)
    }

    /// `PackageManager.checkPermission(permission, package)` in
    /// system_server's context (user 0, the default device).
    pub fn package_has_permission(
        self: &Arc<Self>,
        permission: &str,
        package: &str,
    ) -> Result<bool> {
        let key = (permission.to_string(), package.to_string());
        self.cached(
            |p| &mut p.packages,
            key,
            || {
                let args = pm::CheckPermission {
                    package_name: Some(package.into()),
                    permission_name: Some(permission.into()),
                    persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
                    user_id: 0,
                };
                self.call(
                    "permissionmgr",
                    pm::CHECK_PERMISSION,
                    |p| args.write(p),
                    pm::read_check_permission_reply,
                )
                .map(|r| r == PERMISSION_GRANTED)
            },
        )
    }

    /// Tells the package, mode and instrumentation mirrors of every
    /// package change (`PackageMonitor`'s callback).
    fn watch_packages(self: &Arc<Self>) -> Result<()> {
        let callback = self.listeners.packages;
        self.watch(
            "package",
            package::REGISTER_PACKAGE_MONITOR_CALLBACK,
            |p| {
                package::RegisterPackageMonitorCallback {
                    callback: Some(callback),
                    user_id: USER_ALL,
                }
                .write(p)
            },
            package::read_register_package_monitor_callback_reply,
            |s| {
                s.packages.unwatch();
                s.modes.unwatch();
                s.instrumented.unwatch();
            },
        )
    }

    /// `AppOpsManager.checkPackage`: throws unless `package` is `uid`'s.
    pub fn check_package(self: &Arc<Self>, uid: i32, package: &str) -> Result<()> {
        let mode = self.packages.get(
            (uid, package.to_string()),
            || self.watch_packages(),
            || {
                let args = appops::CheckPackage {
                    uid,
                    package_name: Some(package.into()),
                };
                self.call(
                    "appops",
                    appops::CHECK_PACKAGE,
                    |p| args.write(p),
                    appops::read_check_package_reply,
                )
            },
        )?;
        if mode != MODE_ALLOWED {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(())
    }

    /// `AppOpsManager.noteOp` (`note`) or `checkOp`, on the default device;
    /// both throw on `MODE_ERRORED`. The mode is `checkOp`'s, the one
    /// `noteOp` decides by; the note, which records the access, is sent in
    /// the background.
    pub fn app_op(
        self: &Arc<Self>,
        note: bool,
        op: i32,
        uid: i32,
        package: &str,
        attribution_tag: Option<&str>,
    ) -> Result<i32> {
        let fetch = || {
            let args = appops::CheckOperationForDevice {
                code: op,
                uid,
                package_name: Some(package.into()),
                attribution_tag: None,
                virtual_device_id: 0,
            };
            self.call(
                "appops",
                appops::CHECK_OPERATION_FOR_DEVICE,
                |p| args.write(p),
                appops::read_check_operation_for_device_reply,
            )
        };
        // Shell permission delegation decides the ops of an
        // instrumentation's target as shell's and tells no mode watcher
        // (`AccessCheckDelegate`, #467), so a target's modes are asked.
        let mode = if self.instrumented(uid)? {
            fetch()?
        } else {
            self.modes.get(
                (op, uid, package.to_string()),
                || {
                    for &(op, callback) in &self.listeners.ops {
                        self.watch(
                            "appops",
                            appops::START_WATCHING_MODE_WITH_FLAGS,
                            |p| {
                                appops::StartWatchingModeWithFlags {
                                    op,
                                    package_name: None,
                                    flags: WATCH_FOREGROUND_CHANGES,
                                    callback: Some(callback),
                                }
                                .write(p)
                            },
                            appops::read_start_watching_mode_with_flags_reply,
                            |s| s.modes.unwatch(),
                        )?;
                    }
                    self.watch_packages()
                },
                fetch,
            )?
        };
        if note {
            let note = (
                op,
                uid,
                package.to_string(),
                attribution_tag.map(Into::into),
            );
            let _ = self.notes.send(note);
        }
        if mode == MODE_ERRORED {
            return Err(Exception::security(format!(
                "uid {uid} does not have app op {op} for {package}"
            )));
        }
        Ok(mode)
    }

    /// Whether an installed instrumentation targets `uid`, the only uid
    /// `startDelegateShellPermissionIdentity` can delegate to: its target
    /// package is one of the uid's (`startInstrumentation`), or, for an SDK
    /// sandbox's uid, of its client's.
    fn instrumented(self: &Arc<Self>, uid: i32) -> Result<bool> {
        self.instrumented.get(
            uid,
            || self.watch_packages(),
            || {
                let app = uid % PER_USER_RANGE;
                let target = if (FIRST_SDK_SANDBOX_UID..=LAST_SDK_SANDBOX_UID).contains(&app) {
                    uid - (FIRST_SDK_SANDBOX_UID - FIRST_APPLICATION_UID)
                } else {
                    uid
                };
                let args = package::GetPackagesForUid { uid: target };
                let packages = self.call(
                    "package",
                    package::GET_PACKAGES_FOR_UID,
                    |p| args.write(p),
                    package::read_get_packages_for_uid_reply,
                )?;
                for name in packages.into_iter().flatten().flatten() {
                    let args = package::QueryInstrumentationAsUser {
                        target_package: Some(name),
                        flags: STOCK_PM_FLAGS,
                        user_id: target / PER_USER_RANGE,
                    };
                    let found = self.call(
                        "package",
                        package::QUERY_INSTRUMENTATION_AS_USER,
                        |p| args.write(p),
                        package::read_query_instrumentation_as_user_reply::<ParceledListSlice>,
                    )?;
                    if found.is_some_and(|list| list.len > 0) {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
        )
    }

    /// `IAppOpsService.noteOperation`.
    fn note_op(
        self: &Arc<Self>,
        op: i32,
        uid: i32,
        package: &str,
        attribution_tag: Option<&str>,
    ) -> Result<()> {
        let args = appops::NoteOperation {
            code: op,
            uid,
            package_name: Some(package.into()),
            attribution_tag: attribution_tag.map(Into::into),
            should_collect_async_noted_op: false,
            message: None,
            should_collect_message: true,
        };
        self.call(
            "appops",
            appops::NOTE_OPERATION,
            |p| args.write(p),
            appops::read_note_operation_reply::<SyncNotedAppOp>,
        )
        .map(drop)
    }

    pub fn user_running(self: &Arc<Self>, user_id: i32) -> Result<bool> {
        let args = um::IsUserRunning { user_id };
        self.call(
            "user",
            um::IS_USER_RUNNING,
            |p| args.write(p),
            um::read_is_user_running_reply,
        )
    }

    /// `UserManager.getProfiles(user, true)`, as ids.
    pub fn profile_ids(self: &Arc<Self>, user_id: i32) -> Result<Vec<i32>> {
        let args = um::GetProfileIds {
            user_id,
            enabled_only: true,
        };
        self.call(
            "user",
            um::GET_PROFILE_IDS,
            |p| args.write(p),
            um::read_get_profile_ids_reply,
        )
        .map(Option::unwrap_or_default)
    }

    pub fn has_user_restriction(self: &Arc<Self>, key: &str, user_id: i32) -> Result<bool> {
        let args = um::HasUserRestriction {
            restriction_key: Some(key.into()),
            user_id,
        };
        self.call(
            "user",
            um::HAS_USER_RESTRICTION,
            |p| args.write(p),
            um::read_has_user_restriction_reply,
        )
    }

    /// `KeyguardManager.isDeviceLocked(user, device)`.
    /// The listener reports the system user's state, the one this
    /// process's user; other users are asked each time.
    pub fn device_locked(self: &Arc<Self>, user_id: i32, device_id: i32) -> Result<bool> {
        if device_id != DEVICE_ID_DEFAULT {
            // TrustManagerService: "Virtual devices are considered insecure."
            return Ok(false);
        }
        let fetch = || {
            let args = trust::IsDeviceLocked { user_id, device_id };
            self.call(
                "trust",
                trust::IS_DEVICE_LOCKED,
                |p| args.write(p),
                trust::read_is_device_locked_reply,
            )
        };
        if user_id != USER_SYSTEM {
            return fetch();
        }
        self.locked.get(
            (),
            || {
                let listener = self.listeners.locked;
                self.watch(
                    "trust",
                    trust::REGISTER_DEVICE_LOCKED_STATE_LISTENER,
                    |p| {
                        trust::RegisterDeviceLockedStateListener {
                            listener: Some(listener),
                            device_id: DEVICE_ID_DEFAULT,
                        }
                        .write(p)
                    },
                    trust::read_register_device_locked_state_listener_reply,
                    |s| s.locked.unwatch(),
                )
            },
            fetch,
        )
    }

    /// Stands in for `WindowManagerInternal.isUidFocused`, which has no
    /// binder form: whether `uid` owns the focused root task (#430).
    /// Any change of the tasks may change it (`ITaskStackListener`).
    pub fn uid_focused(self: &Arc<Self>, uid: i32) -> Result<bool> {
        let focused = self.focus.get(
            (),
            || {
                let listener = self.listeners.task_stack;
                self.watch(
                    "activity_task",
                    atm::REGISTER_TASK_STACK_LISTENER,
                    |p| {
                        atm::RegisterTaskStackListener {
                            listener: Some(listener),
                        }
                        .write(p)
                    },
                    atm::read_register_task_stack_listener_reply,
                    |s| s.focus.unwatch(),
                )
            },
            || {
                let task = self.call(
                    "activity_task",
                    atm::GET_FOCUSED_ROOT_TASK_INFO,
                    |p| atm::GetFocusedRootTaskInfo {}.write(p),
                    atm::read_get_focused_root_task_info_reply::<RootTaskInfo>,
                )?;
                Ok(task.map(|task| task.effective_uid))
            },
        )?;
        Ok(focused == Some(uid))
    }
}

/// `ContentProviderHolder`, up to its provider: the first binder past
/// its `ProviderInfo`.
struct ContentProviderHolder {
    provider: Option<u32>,
}

impl ReadParcelable for ContentProviderHolder {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let Some(at) = r.next_object() else {
            return Ok(Self { provider: None });
        };
        r.set_position(at);
        Ok(Self {
            provider: match r.read_binder()? {
                Some(Binder::Handle(h)) => Some(h),
                _ => None,
            },
        })
    }
}

/// `Uri.StringUri` (`Uri.writeToParcel`: its type and its string).
struct StringUri<'a>(&'a str);

impl WriteParcelable for StringUri<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1);
        p.write_string8(Some(self.0));
    }
}

/// `ParcelFileDescriptor` (`writeToParcel` without a comm channel: 0, the
/// fd), as the fd's index in this process.
struct ParcelFileDescriptor(u32);

impl ReadParcelable for ParcelFileDescriptor {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        if r.read_i32()? != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(Self(r.read_fd()?))
    }
}

/// `SyncNotedAppOp`, not read: the note's mode is the one already decided.
struct SyncNotedAppOp;

impl ReadParcelable for SyncNotedAppOp {
    fn read_from(_: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self)
    }
}

/// `ParceledListSlice`, up to its length.
struct ParceledListSlice {
    len: i32,
}

impl ReadParcelable for ParceledListSlice {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self { len: r.read_i32()? })
    }
}

/// `ActivityTaskManager.RootTaskInfo`, up to `TaskInfo.effectiveUid`.
struct RootTaskInfo {
    effective_uid: i32,
}

impl ReadParcelable for RootTaskInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let rect = |r: &mut Reader<'_>| -> ParcelResult<()> {
            if r.read_i32()? != 0 {
                (0..4).try_for_each(|_| r.read_i32().map(drop))?;
            }
            Ok(())
        };
        rect(r)?; // bounds
        aim_service_aidl::read_int_array(r)?; // child task ids
        aim_service_aidl::read_string_list(r)?; // child task names
        for _ in 0..r.read_i32()?.max(0) {
            rect(r)?; // child task bounds
        }
        aim_service_aidl::read_int_array(r)?; // child task user ids
        r.read_i32()?; // visible
        r.read_i32()?; // position
        r.read_i32()?; // TaskInfo.userId
        r.read_i32()?; // taskId
        Ok(Self {
            effective_uid: r.read_i32()?,
        })
    }
}

/// A listener node: every call it gets is a change of what it watches.
struct Listener {
    descriptor: &'static str,
    fds: bool,
    system: Weak<System>,
    changed: fn(&System),
}

impl Service for Listener {
    fn descriptor(&self) -> &str {
        self.descriptor
    }

    fn transact(&self, _: &mut Call<'_>) -> Reply {
        if let Some(system) = self.system.upgrade() {
            (self.changed)(&system);
        }
        Ok(Parcel::new())
    }

    fn accepts_fds(&self) -> bool {
        self.fds
    }
}
