//! The original system services a native one consults, as their client:
//! found through servicemanager, called with the pinned AIDL's codes.
//!
//! These are the binder equivalents of what the original service asks
//! other services in its process; where the original uses a
//! system_server-internal API that has none, the method says what it
//! stands in for. What a focused app's access reads (package ownership,
//! app-op modes, focus, whether the device is locked) is mirrored
//! ([`crate::mirror`]), each fed by its owner's listener; noting an app op,
//! which only records the access, is sent in the background, and an op
//! noted for the caller is sent back with the reply
//! ([`aim_binder_host::appops`]). Permission
//! checks are kept as apps' `PermissionManager` keeps them, by the
//! `package_info_cache` nonce the system_server bridge hands over
//! ([`crate::nonces`]); an instrumentation target's app ops are asked each
//! time (docs/system-services.md, "Mirrored state").

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, LocalService, Received, Reply, Service, Strong};
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

/// An app op to note: `(op, uid, package, attribution tag)`, and the
/// message for the app's async noted-op callback when collected for it.
type Note = (i32, i32, String, Option<String>, Option<String>);

/// `AppOpsManager.NotedOpCollectionMode`, `DONT_COLLECT` including
/// `COLLECT_SELF`: this process has no op-noted callback of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Collection {
    None,
    /// Sent back with the reply to the caller the op is noted for.
    Sync,
    /// Told to the app's async noted-op callback.
    Async,
}

enum ServiceOwner {
    Remote(Arc<Strong>),
    Local(LocalService),
}
impl ServiceOwner {
    fn transact(
        &self,
        code: u32,
        data: &Parcel,
        oneway: bool,
    ) -> std::result::Result<Received, aim_binder_host::parcel::StatusCode> {
        match self {
            Self::Remote(service) => service.transact(code, data, oneway),
            Self::Local(service) => service.transact(code, data, oneway),
        }
    }
}

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
    /// `AppOpsManager.sAppOpsToNote`: whether an op's notes are collected.
    collected_ops: Mutex<HashMap<i32, bool>>,
    /// The nonces in system_server's shared memory, from the bridge.
    nonces: Mutex<Option<Arc<NonceSource>>>,
    permissions: Mutex<Permissions>,
    /// Told of each bridge system_server hands over (a handle held for
    /// the call).
    bridge_listeners: Mutex<Vec<BridgeListener>>,
    package_bootstrap: Mutex<PackageBootstrapState>,
    package_install_lock: Mutex<()>,
    package_signing: Mutex<Option<Arc<crate::package::sign::Overrides>>>,
}

#[derive(Default)]
struct PackageBootstrapState {
    current: Option<PackageBootstrap>,
    version: u64,
}

struct PackageBootstrap {
    bridge: Arc<crate::package::bootstrap::Bridge>,
    snapshots: Option<crate::package::scan_snapshot::Store>,
    queries: Option<Arc<crate::package::scan_snapshot::query_state::Capture>>,
}

/// Told of a bridge attached, with its handle.
type BridgeListener = Box<dyn Fn(u32) + Send + Sync>;

/// One system_server generation owns both its Binder and nonce mapping.
struct NonceSource {
    bridge: Arc<Strong>,
    nonces: Nonces,
}

/// Permission checks kept with the nonce they were asked at, as
/// `PermissionManager`'s `sPermissionCache` (by permission and uid; the
/// pid does not decide) and `sPackageNamePermissionCache` keep them.
#[derive(Default)]
struct Permissions {
    source: Weak<NonceSource>,
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
                collected_ops: Mutex::new(HashMap::new()),
                nonces: Mutex::new(None),
                permissions: Mutex::new(Permissions::default()),
                bridge_listeners: Mutex::new(Vec::new()),
                package_bootstrap: Mutex::new(PackageBootstrapState::default()),
                package_install_lock: Mutex::new(()),
                package_signing: Mutex::new(None),
            }
        });
        let this = Arc::downgrade(&system);
        let _ = std::thread::Builder::new()
            .name("appops-notes".into())
            .spawn(move || {
                for (op, uid, package, tag, message) in noted {
                    let Some(system) = this.upgrade() else { break };
                    if let Err(e) = system.note_op(op, uid, &package, tag.as_deref(), message) {
                        eprintln!(
                            "services: noteOperation({op}, {uid}, {package}): {}",
                            e.message
                        );
                    }
                }
            });
        system
    }

    /// The binder process the services and the bridge's handles live in.
    pub(crate) fn process(&self) -> Arc<LocalProcess> {
        self.process.clone()
    }

    /// `name` from servicemanager, kept until it dies.
    fn service(self: &Arc<Self>, name: &'static str) -> Result<ServiceOwner> {
        if let Some(s) = self.services.lock().unwrap().get(name) {
            return Ok(ServiceOwner::Remote(s.clone()));
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
        if let Some(Binder::Local(ptr)) = binder {
            return self
                .process
                .local_service(ptr)
                .map(ServiceOwner::Local)
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("unknown local {name} service"),
                    )
                });
        }
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
        Ok(ServiceOwner::Remote(strong))
    }

    /// Calls `code` of service `name` and reads its reply.
    pub(crate) fn call<T>(
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
        if let ServiceOwner::Remote(service) = service {
            self.process.link_to_death(
                &service,
                Box::new(move || {
                    if let Some(system) = this.upgrade() {
                        unwatch(&system);
                    }
                }),
            );
        }
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
        let Some(source) = nonces else {
            return ask();
        };
        let Some(nonce) = source.nonces.get(PACKAGE_INFO_NONCE) else {
            return ask();
        };
        {
            let mut kept = self.permissions.lock().unwrap();
            if kept.nonce != nonce || !kept.source.ptr_eq(&Arc::downgrade(&source)) {
                *kept = Permissions {
                    source: Arc::downgrade(&source),
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
        if kept.nonce == nonce && kept.source.ptr_eq(&Arc::downgrade(&source)) {
            table(&mut kept).insert(key, granted);
        }
        Ok(granted)
    }

    pub(crate) fn package_install_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.package_install_lock.lock().unwrap()
    }

    /// SDK filesystem work shares the package owner's install lock with code cleanup.
    pub fn reconcile_package_sdk_data(
        self: &Arc<Self>,
        args: crate::package::owner::sdk_data::SdkData,
    ) -> Result<()> {
        use aim_service_aidl::android_os_iinstalld as installd;
        let _install = self.package_install_guard();
        let request = installd::ReconcileSdkData { args: Some(args) };
        self.call(
            "installd",
            installd::RECONCILE_SDK_DATA,
            |p| request.write(p),
            |r| {
                let result = installd::read_reconcile_sdk_data_reply(r)?;
                if r.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                Ok(result)
            },
        )
    }

    /// Synchronous early package-owner attachment, independent of late listeners.
    pub fn attach_package_bootstrap(self: &Arc<Self>, handle: u32) -> Result<()> {
        let bridge = Arc::new(crate::package::bootstrap::Bridge::new(
            self.process.strong(handle),
        )?);
        let mut signing = self.package_signing.lock().unwrap();
        match signing.as_ref() {
            Some(owner) if owner.is_debuggable() != bridge.signing_debuggable => {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package signing build policy changed",
                ));
            }
            None => {
                *signing = Some(Arc::new(crate::package::sign::Overrides::new(
                    bridge.signing_debuggable,
                )))
            }
            _ => {}
        }
        let this = Arc::downgrade(self);
        let attached = Arc::downgrade(&bridge);
        let mut current = self.package_bootstrap.lock().unwrap();
        current.current = Some(PackageBootstrap {
            bridge: bridge.clone(),
            snapshots: None,
            queries: None,
        });
        self.process.link_to_death(
            &bridge.owner,
            Box::new(move || {
                if let (Some(system), Some(attached)) = (this.upgrade(), attached.upgrade()) {
                    let mut current = system.package_bootstrap.lock().unwrap();
                    if current
                        .current
                        .as_ref()
                        .is_some_and(|owner| Arc::ptr_eq(&owner.bridge, &attached))
                    {
                        current.current.take();
                    }
                }
            }),
        );
        Ok(())
    }

    pub fn package_signing_overrides(&self) -> Result<Arc<crate::package::sign::Overrides>> {
        self.package_bootstrap()?;
        self.package_signing.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package signing owner is unavailable",
            )
        })
    }

    pub(crate) fn mutate_package_signing(
        &self,
        edit: impl FnOnce(&crate::package::sign::Overrides) -> Result<i64>,
    ) -> Result<i64> {
        let owner = self.package_signing_overrides()?;
        if !owner.is_debuggable() {
            return Err(Exception::security(
                "This test API is only available on debuggable builds",
            ));
        }
        edit(&owner)
    }

    pub fn package_bootstrap(&self) -> Result<Arc<crate::package::bootstrap::Bridge>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .map(|owner| owner.bridge.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package bootstrap bridge is unavailable",
                )
            })
    }

    pub(crate) fn check_package_bootstrap(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<()> {
        if !Arc::ptr_eq(bridge, &self.package_bootstrap()?) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package scan bootstrap owner changed",
            ));
        }
        Ok(())
    }

    fn check_package_boot_scan(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        apks: &crate::package::write::Apks,
    ) -> std::result::Result<(), crate::package::bootstrap::BootError> {
        use crate::package::bootstrap::{BootError, OwnerError};
        let result = (|| {
            self.check_package_bootstrap(bridge)?;
            let expected = self.package_signing_overrides()?;
            if !apks
                .signing_overrides
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &expected))
            {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package scan has a different signing owner",
                ));
            }
            Ok(())
        })();
        result.map_err(|error| BootError::Owner(OwnerError::Owner(error)))
    }

    /// Publish only against the original policy owner used to complete this scan.
    pub fn publish_package_scan(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
    ) -> Result<Arc<crate::package::scan_snapshot::Snapshot>> {
        use crate::package::scan_snapshot::Store;
        let fail =
            |message: &str| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        let mut state = self.package_bootstrap.lock().unwrap();
        let previous_version = state.version;
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| fail("package scan bootstrap owner changed"))?;
        let result = match (&current.snapshots, base) {
            (Some(store), Some(base)) => store.publish(base, owner, usage),
            (None, None) => previous_version
                .checked_add(1)
                .ok_or(crate::package::scan_snapshot::Error::VersionExhausted)
                .and_then(|version| Store::new_replica_at_version(owner, usage, version))
                .map(|store| {
                    let capture = store.capture();
                    current.snapshots = Some(store);
                    capture
                }),
            _ => return Err(fail("package scan publication base differs")),
        };
        let snapshot =
            result.map_err(|error| fail(&format!("package scan publication failed: {error:?}")))?;
        current.queries = None;
        state.version = snapshot.version();
        Ok(snapshot)
    }

    /// Publish the complete package and query owners as one native generation.
    pub fn publish_package_scan_with_queries(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        context: crate::package::scan_snapshot::query_state::Context,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        use crate::package::scan_snapshot::{Store, query_state::Capture};
        let fail =
            |message: String| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        let mut state = self.package_bootstrap.lock().unwrap();
        let version = state
            .version
            .checked_add(1)
            .ok_or_else(|| fail("package query version exhausted".into()))?;
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| fail("package query bootstrap owner changed".into()))?;
        match (&current.snapshots, base) {
            (Some(store), Some(base)) if Arc::ptr_eq(&store.capture(), base) => {}
            (None, None) => {}
            _ => return Err(fail("package query publication base differs".into())),
        }
        let store = Store::new_replica_at_version(owner, usage, version)
            .map_err(|error| fail(format!("package query scan validation failed: {error:?}")))?;
        let capture = Capture::new(store.capture(), context).map_err(fail)?;
        current.snapshots = Some(store);
        current.queries = Some(capture.clone());
        state.version = version;
        Ok(capture)
    }

    pub fn publish_package_queries(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        scan: &Arc<crate::package::scan_snapshot::Snapshot>,
        context: crate::package::scan_snapshot::query_state::Context,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        let fail =
            |message: String| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        let capture =
            crate::package::scan_snapshot::query_state::Capture::new(scan.clone(), context)
                .map_err(fail)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| fail("package query bootstrap owner changed".into()))?;
        if !current
            .snapshots
            .as_ref()
            .is_some_and(|store| Arc::ptr_eq(&store.capture(), scan))
        {
            return Err(fail("package query scan owner changed".into()));
        }
        if current.queries.is_some() {
            return Err(fail("package query owner already published".into()));
        }
        current.queries = Some(capture.clone());
        Ok(capture)
    }

    pub fn capture_package_queries(
        &self,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.queries.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native package query owners are unavailable",
                )
            })
    }

    /// Finish native boot dependencies/runtime before publication.
    pub fn complete_package_scan(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        retained: std::collections::BTreeMap<(String, bool), crate::package::scan::OriginalRuntime>,
    ) -> Result<Arc<crate::package::scan_snapshot::Snapshot>> {
        let owner = self.complete_package_owner(bridge, owner, &usage, retained)?;
        self.publish_package_scan(bridge, base, owner, usage)
    }

    pub fn complete_package_scan_with_queries(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        retained: std::collections::BTreeMap<(String, bool), crate::package::scan::OriginalRuntime>,
        context: crate::package::scan_snapshot::query_state::Context,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        let owner = self.complete_package_owner(bridge, owner, &usage, retained)?;
        let context = bridge
            .resolve_query_context(&owner, context)
            .map_err(|error| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package query owners failed: {error:?}"),
                )
            })?;
        self.check_package_bootstrap(bridge)?;
        self.publish_package_scan_with_queries(bridge, base, owner, usage, context)
    }

    /// Complete and retain boot domains in the same atomic scan/query capture.
    pub fn complete_package_scan_with_domains(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        retained: std::collections::BTreeMap<(String, bool), crate::package::scan::OriginalRuntime>,
        context: crate::package::scan_snapshot::query_state::Context,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        let owner = self.complete_package_owner(bridge, owner, &usage, retained)?;
        let fail = |error| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package domain/query owners failed: {error:?}"),
            )
        };
        let context = bridge
            .resolve_boot_domain_query_context(&owner, context, config)
            .map_err(fail)?;
        let context = bridge
            .resolve_query_context(&owner, context)
            .map_err(fail)?;
        self.check_package_bootstrap(bridge)?;
        self.publish_package_scan_with_queries(bridge, base, owner, usage, context)
    }

    pub fn capture_package_domains(
        &self,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::NativeDomains>> {
        self.capture_package_queries()?
            .domains()
            .cloned()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native package domain owner is unavailable",
                )
            })
    }
    pub(crate) fn check_package_domain_capture(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        if Arc::ptr_eq(capture, &self.capture_package_queries()?) {
            Ok(())
        } else {
            Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "domain authorization capture changed",
            ))
        }
    }

    /// Original permission/proxy calls run outside the publication lock.
    pub fn authorize_package_domain(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        pid: i32,
        uid: i32,
        operation: crate::package::domain_verification::enforcer::Operation<'_>,
    ) -> Result<bool> {
        use crate::package::domain_verification::enforcer::{self, Captured, Error};
        let check = || self.check_package_domain_capture(bridge, capture);
        check()?;
        if uid < 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT,
                "invalid domain caller UID",
            ));
        }
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state()).map_err(|e| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("domain visibility owner: {e:?}"),
            )
        })?;
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let permission = |uid, permission: &str| {
            self.check_permission(permission, pid, uid)
                .map_err(|e| format!("{e:?}"))
        };
        let verifier = |uid| {
            bridge
                .is_domain_verifier_uid(uid)
                .map_err(|e| format!("{e:?}"))
        };
        let owners = Captured {
            query: &query,
            permissions: &permission,
            verifier: &verifier,
        };
        let result = enforcer::authorize(
            &owners,
            uid,
            crate::package::apps_filter::user_id(uid),
            operation,
        );
        check()?;
        result.map_err(|error| match error {
            Error::Security => Exception::security("domain caller is not authorized"),
            Error::Owner(message) => {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
            }
        })
    }
    /// Persist a prevalidated domain replacement under the same generation
    /// lock as publication. A committed reserve failure still publishes it.
    pub fn commit_package_domains(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        update: crate::package::scan_snapshot::query_state::DomainUpdate,
        persistence: &mut crate::package::owner::Store,
    ) -> std::result::Result<
        Arc<crate::package::scan_snapshot::query_state::Capture>,
        crate::package::owner::WriteError,
    > {
        self.commit_package_domains_if_current(bridge, update, persistence)?
            .ok_or_else(|| crate::package::owner::WriteError {
                committed: false,
                message: "domain publication base changed".into(),
            })
    }
    pub(crate) fn commit_package_domains_if_current(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        update: crate::package::scan_snapshot::query_state::DomainUpdate,
        persistence: &mut crate::package::owner::Store,
    ) -> std::result::Result<
        Option<Arc<crate::package::scan_snapshot::query_state::Capture>>,
        crate::package::owner::WriteError,
    > {
        use crate::package::owner::WriteError;
        let before = |message: &str| WriteError {
            committed: false,
            message: message.into(),
        };
        let mut state = self.package_bootstrap.lock().unwrap();
        let version = state.version;
        let current = state
            .current
            .as_mut()
            .filter(|c| Arc::ptr_eq(&c.bridge, bridge))
            .ok_or_else(|| before("domain bootstrap owner changed"))?;
        if !current
            .queries
            .as_ref()
            .is_some_and(|q| Arc::ptr_eq(q, &update.base))
        {
            return Ok(None);
        }
        if update.capture.scan().version()
            != version
                .checked_add(1)
                .ok_or_else(|| before("domain generation exhausted"))?
        {
            return Err(before("domain generation differs"));
        }
        // Bind the disk writer to this exact package inventory, including signers.
        let identities = |settings: &crate::package::settings::Settings| {
            settings
                .packages
                .iter()
                .map(|s| {
                    (
                        s.name.clone(),
                        s.app_id,
                        s.code_path.clone(),
                        s.version_code,
                        s.domain_set_id.clone(),
                        s.signatures.as_ref().map(|v| v.signatures.clone()),
                    )
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        if identities(&persistence.state().settings)
            != identities(&update.base.scan().owner().settings)
        {
            return Err(before("domain persistence package owner differs"));
        }
        let canonical = |mut value: crate::package::domain_verification::State| {
            value.active.sort_by(|a, b| a.name.cmp(&b.name));
            value.restored.sort_by(|a, b| a.name.cmp(&b.name));
            value
        };
        if canonical(persistence.state().settings.domain_verification.clone())
            != canonical(
                update
                    .base
                    .scan()
                    .owner()
                    .settings
                    .domain_verification
                    .clone(),
            )
        {
            return Err(before("domain persistence base differs"));
        }
        let result = persistence.commit_domains(
            &update
                .capture
                .domains()
                .ok_or_else(|| before("missing replacement domain owner"))?
                .owner()
                .persisted(),
        );
        if result.is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            current.snapshots = Some(update.store);
            current.queries = Some(update.capture.clone());
            state.version = update.capture.scan().version();
        }
        let committed = result.is_ok() || result.as_ref().is_err_and(|e| e.committed);
        drop(state);
        if committed {
            // Original callbacks cannot run under the publication lock.
            if let Err(error) = self
                .check_package_bootstrap(bridge)
                .map_err(|e| format!("{e:?}"))
                .and_then(|_| {
                    bridge
                        .invalidate_package_info_cache()
                        .map_err(|e| format!("{e:?}"))
                })
            {
                return Err(WriteError {
                    committed: true,
                    message: format!(
                        "committed domain cache invalidation failed: {error}; persistence: {result:?}"
                    ),
                });
            }
        }
        result.map(|_| Some(update.capture))
    }
    // Original policy calls run without holding the publication lock.
    fn complete_package_owner(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        mut owner: crate::package::scan::SigningScan,
        usage: &crate::package::owner::usage::Usage,
        retained: std::collections::BTreeMap<(String, bool), crate::package::scan::OriginalRuntime>,
    ) -> Result<crate::package::scan::SigningScan> {
        let fail =
            |message: String| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        self.check_package_bootstrap(bridge)?;
        let policies = owner
            .loaded_packages()
            .iter()
            .map(|(name, code)| {
                bridge
                    .library_policy(name, code.package.target_sdk_version)
                    .map(|policy| (name.clone(), policy))
                    .map_err(|error| {
                        fail(format!("boot library policy failed for {name}: {error:?}"))
                    })
            })
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        owner
            .complete_library_dependencies(&|name, _| {
                policies
                    .get(name)
                    .map(|policy| crate::package::libraries::Policy {
                        enforce_native_dependencies: policy.enforce_native_dependencies,
                        sdk_library_independence: policy.sdk_library_independence,
                    })
                    .ok_or(crate::package::libraries::ResolveError::Incomplete(
                        "boot library policy owner",
                    ))
            })
            .map_err(|error| fail(format!("boot library completion failed: {error:?}")))?;
        owner
            .complete_runtime_at_boot(usage, retained)
            .map_err(|error| fail(format!("boot runtime completion failed: {error}")))?;
        Ok(owner)
    }

    pub fn capture_package_scan(&self) -> Result<Arc<crate::package::scan_snapshot::Snapshot>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|owner| owner.snapshots.as_ref())
            .map(|store| store.capture())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "complete package scan is unavailable",
                )
            })
    }

    /// Prepare native first-boot code using the caller's captured early owner.
    /// Carry that same bridge through saved phases and completion/publication.
    /// The result is unpublished until reconciliation and snapshot gates finish.
    pub fn scan_package_first_boot(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        apks: &crate::package::write::Apks,
        config: &crate::package::system_config::SystemConfig,
        properties: &dyn Fn(&str) -> Option<String>,
        policy: crate::package::bootstrap::ScanPolicy<'_>,
    ) -> std::result::Result<
        crate::package::scan::SystemImageScan,
        crate::package::bootstrap::BootError,
    > {
        self.check_package_boot_scan(bridge, apks)?;
        let scan = bridge
            .resolve_boot(config, properties)?
            .scan_first_boot(apks, policy)?;
        self.check_package_boot_scan(bridge, apks)?;
        Ok(scan)
    }

    /// Scan restored system settings before data APK reconciliation. Mutations
    /// remain with the supplied owner; this does not persist or publish it.
    pub fn scan_package_saved_system(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        owner: &mut crate::package::scan::SigningScan,
        apks: &crate::package::write::Apks,
        config: &crate::package::system_config::SystemConfig,
        properties: &dyn Fn(&str) -> Option<String>,
        policy: crate::package::bootstrap::ScanPolicy<'_>,
        saved: crate::package::scan::SavedSystemScanInputs<'_>,
    ) -> std::result::Result<
        crate::package::bootstrap::SavedSystemPhase,
        crate::package::bootstrap::BootError,
    > {
        self.check_package_boot_scan(bridge, apks)?;
        let scan = bridge
            .resolve_boot(config, properties)?
            .scan_saved_system(owner, apks, policy, saved)?;
        self.check_package_boot_scan(bridge, apks)?;
        Ok(scan)
    }

    /// Run restored system and data phases against one captured original owner.
    /// The caller retains mutations on failure; persistence/publication follows.
    pub fn scan_package_saved_boot(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        owner: &mut crate::package::scan::SigningScan,
        apks: &crate::package::write::Apks,
        config: &crate::package::system_config::SystemConfig,
        properties: &dyn Fn(&str) -> Option<String>,
        policy: crate::package::bootstrap::ScanPolicy<'_>,
        saved: crate::package::scan::SavedSystemScanInputs<'_>,
        volumes: &[String],
        expecting_better: &std::collections::BTreeSet<String>,
        is_incremental: &dyn Fn(&str) -> std::result::Result<bool, String>,
        destinations: &std::collections::BTreeMap<
            String,
            crate::package::scan::NativeLibraryDestination<'_>,
        >,
    ) -> std::result::Result<
        crate::package::bootstrap::SavedBootScan,
        crate::package::bootstrap::BootError,
    > {
        use crate::package::bootstrap::{BootError, DataBootInputs, SavedBootScan};
        self.check_package_boot_scan(bridge, apks)?;
        let boot = bridge.resolve_boot(config, properties)?;
        let data_users = saved.users;
        let first_boot_or_upgrade = saved.first_boot_or_upgrade;
        let old_stub_packages = saved.old_stub_packages;
        let resources = saved.resources;
        let system = boot.scan_saved_system(owner, apks, policy, saved)?;
        let platform = owner
            .loaded_packages()
            .get("android")
            .ok_or_else(|| {
                BootError::Scan(crate::package::scan::SigningError::Fatal(
                    crate::package::scan::Error {
                        package: "android".into(),
                        path: String::new(),
                        phase: "platform",
                        message: "data admission requires the scanned platform signing owner"
                            .into(),
                    },
                ))
            })?
            .collected_signing
            .clone();
        let data = boot.scan_data(
            owner,
            apks,
            policy,
            DataBootInputs {
                factories: &system.system,
                platform: &platform,
                volumes,
                users: data_users,
                first_boot_or_upgrade,
                old_stub_packages,
                expecting_better,
                is_incremental,
                destinations,
                resources,
            },
        )?;
        self.check_package_boot_scan(bridge, apks)?;
        Ok(SavedBootScan { system, data })
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
        let mut reader = reply.reader();
        let fd =
            bridge::read_get_application_shared_memory_reply::<ParcelFileDescriptor>(&mut reader)
                .map_err(failed)??
                .ok_or_else(|| failed(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(failed(aim_binder_host::parcel::BAD_VALUE));
        }
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
        let mut current = self.nonces.lock().unwrap();
        // Binder accepts one death registration per node reference. Reattaching
        // that endpoint reuses its retained identity and existing registration.
        let retained = current
            .as_ref()
            .filter(|source| source.bridge.handle == handle)
            .map(|source| source.bridge.clone());
        let newly_attached = retained.is_none();
        let owner = retained.unwrap_or_else(|| Arc::new(strong));
        *current = Some(Arc::new(NonceSource {
            bridge: owner.clone(),
            nonces,
        }));
        drop(current);
        if newly_attached {
            let attached = Arc::downgrade(&owner);
            let this = Arc::downgrade(self);
            self.process.link_to_death(
                &owner,
                Box::new(move || {
                    if let (Some(system), Some(attached)) = (this.upgrade(), attached.upgrade()) {
                        let mut current = system.nonces.lock().unwrap();
                        if current
                            .as_ref()
                            .is_some_and(|source| Arc::ptr_eq(&source.bridge, &attached))
                        {
                            current.take();
                        }
                    }
                }),
            );
        }
        for listener in self.bridge_listeners.lock().unwrap().iter() {
            listener(handle);
        }
        Ok(())
    }

    /// The `package_info_cache` nonce now; `None` without the bridge or
    /// while it is unset.
    pub(crate) fn package_info_nonce(&self) -> Option<i64> {
        let nonces = self.nonces.lock().unwrap().clone();
        nonces.and_then(|n| n.nonces.get(PACKAGE_INFO_NONCE))
    }

    /// Tells `listener` of each bridge attached from now on, with its
    /// handle, valid while it is called.
    pub fn add_bridge_listener(&self, listener: BridgeListener) {
        self.bridge_listeners.lock().unwrap().push(listener);
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
    /// the background, collected as `noteOp` collects it. An async note's
    /// message is this thread's stack, as `noteOp` without one sends its
    /// own (`getFormattedStackTrace`).
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
            let collection = self.collection(op, uid, package);
            if collection == Collection::Sync && mode == MODE_ALLOWED {
                aim_binder_host::appops::collect_sync(op, attribution_tag);
            }
            let message = (collection == Collection::Async)
                .then(|| std::backtrace::Backtrace::force_capture().to_string());
            let note = (
                op,
                uid,
                package.to_string(),
                attribution_tag.map(Into::into),
                message,
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

    /// `IAppOpsService.noteOperation`, with the message of a note
    /// collected for the app's async noted-op callback.
    fn note_op(
        self: &Arc<Self>,
        op: i32,
        uid: i32,
        package: &str,
        attribution_tag: Option<&str>,
        async_message: Option<String>,
    ) -> Result<()> {
        let args = appops::NoteOperation {
            code: op,
            uid,
            package_name: Some(package.into()),
            attribution_tag: attribution_tag.map(Into::into),
            should_collect_async_noted_op: async_message.is_some(),
            message: async_message,
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

    /// `AppOpsManager.noteOpNoThrow(op, uid, package, tag, message)` of
    /// system_server for an app: noted now, its mode the answer, collected
    /// as `getNotedOpCollectionMode` decides.
    pub(crate) fn note_op_now(
        self: &Arc<Self>,
        op: i32,
        uid: i32,
        package: &str,
        attribution_tag: Option<&str>,
        message: &str,
    ) -> Result<i32> {
        let collection = self.collection(op, uid, package);
        let args = appops::NoteOperation {
            code: op,
            uid,
            package_name: Some(package.into()),
            attribution_tag: attribution_tag.map(Into::into),
            should_collect_async_noted_op: collection == Collection::Async,
            message: Some(message.into()),
            should_collect_message: true,
        };
        let Some(noted) = self.call(
            "appops",
            appops::NOTE_OPERATION,
            |p| args.write(p),
            appops::read_note_operation_reply::<NotedOp>,
        )?
        else {
            return Ok(MODE_ERRORED);
        };
        if collection == Collection::Sync && noted.mode == MODE_ALLOWED {
            aim_binder_host::appops::collect_sync(op, noted.attribution_tag.as_deref());
        }
        Ok(noted.mode)
    }

    /// `AppOpsManager.getNotedOpCollectionMode` of system_server
    /// (`currentOpPackageName` "android"): a note for the caller of the
    /// call this thread serves, when it collects, goes back with the reply;
    /// one for another app to its async callback.
    fn collection(self: &Arc<Self>, op: i32, uid: i32, package: &str) -> Collection {
        let known = self.collected_ops.lock().unwrap().get(&op).copied();
        let collected = match known {
            Some(collected) => collected,
            None => {
                let args = appops::ShouldCollectNotes { op_code: op };
                let Ok(collected) = self.call(
                    "appops",
                    appops::SHOULD_COLLECT_NOTES,
                    |p| args.write(p),
                    appops::read_should_collect_notes_reply,
                ) else {
                    return Collection::None;
                };
                self.collected_ops.lock().unwrap().insert(op, collected);
                collected
            }
        };
        if !collected || (uid == crate::SYSTEM_UID as i32 && package == "android") {
            Collection::None
        } else if aim_binder_host::appops::collecting_uid() == Some(uid as u32) {
            Collection::Sync
        } else {
            Collection::Async
        }
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

    /// `UserManager.isUserUnlockingOrUnlocked`.
    pub fn user_unlocking_or_unlocked(self: &Arc<Self>, user_id: i32) -> Result<bool> {
        let args = um::IsUserUnlockingOrUnlocked { user_id };
        self.call(
            "user",
            um::IS_USER_UNLOCKING_OR_UNLOCKED,
            |p| args.write(p),
            um::read_is_user_unlocking_or_unlocked_reply,
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

/// `SyncNotedAppOp`'s mode and the attribution tag it was noted under.
struct NotedOp {
    mode: i32,
    attribution_tag: Option<String>,
}

impl ReadParcelable for NotedOp {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // Which of the nullable fields follow.
        let fields = r.read_i32()?;
        let mode = r.read_i32()?;
        r.read_i32()?; // the op
        let attribution_tag = if fields & 0x4 != 0 {
            r.read_string16()?
        } else {
            None
        };
        Ok(Self {
            mode,
            attribution_tag,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_cache_is_bound_to_nonce_mapping_identity() {
        use aim_binder_driver::{Credentials, Device, Driver};
        use std::os::fd::AsFd;
        let driver = Driver::new();
        let process = LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid: 97001,
                euid: 1000,
                security_context: None,
            },
        );
        let system = System::new(process.clone(), &[]);
        let mapped = || {
            let file = crate::nonces::tests::nonce_file(42);
            let retained = aim_binder_host::server::file_from_fd(file.as_fd()).unwrap();
            Arc::new(NonceSource {
                bridge: Arc::new(process.strong(0)),
                nonces: Nonces::map(&retained).unwrap(),
            })
        };
        let table: fn(&mut Permissions) -> &mut HashMap<(String, i32), bool> = |p| &mut p.uids;
        let key = ("permission".to_string(), 19001);
        *system.nonces.lock().unwrap() = Some(mapped());
        assert!(system.cached(table, key.clone(), || Ok(true)).unwrap());
        assert!(
            system
                .cached(table, key.clone(), || panic!(
                    "unchanged owner must hit cache"
                ))
                .unwrap()
        );
        // Two system_server generations may carry equal numeric nonces.
        *system.nonces.lock().unwrap() = Some(mapped());
        assert!(!system.cached(table, key.clone(), || Ok(false)).unwrap());
        assert!(
            !system
                .cached(table, key.clone(), || panic!(
                    "replacement result must be cached"
                ))
                .unwrap()
        );

        let old = mapped();
        *system.nonces.lock().unwrap() = Some(old.clone());
        let (started, waiting) = mpsc::channel();
        let (finish, released) = mpsc::channel();
        let querying = system.clone();
        let old_key = key.clone();
        let in_flight = std::thread::spawn(move || {
            querying.cached(table, old_key, || {
                started.send(()).unwrap();
                released.recv().unwrap();
                Ok(true)
            })
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        *system.nonces.lock().unwrap() = Some(mapped());
        assert!(!system.cached(table, key.clone(), || Ok(false)).unwrap());
        finish.send(()).unwrap();
        assert!(in_flight.join().unwrap().unwrap());
        assert!(
            !system
                .cached(table, key.clone(), || panic!(
                    "old reply polluted replacement cache"
                ))
                .unwrap()
        );
        system.nonces.lock().unwrap().take();
        assert!(system.cached(table, key.clone(), || Ok(true)).unwrap());
        assert!(!system.cached(table, key, || Ok(false)).unwrap());
        driver.release(process.proc_handle());
    }

    /// `SyncNotedAppOp.writeToParcel`: the null fields' flags, mode, op,
    /// then the tag and package present.
    fn sync_noted(tag: Option<&str>) -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(if tag.is_some() { 0x4 } else { 0 } | 0x8);
        p.write_i32(MODE_ALLOWED);
        p.write_i32(1);
        if tag.is_some() {
            p.write_string16(tag);
        }
        p.write_string16(Some("com.example"));
        p
    }

    #[test]
    fn a_noted_op_keeps_the_tag_it_was_noted_under() {
        for tag in [Some("tag"), None] {
            let p = sync_noted(tag);
            let noted = NotedOp::read_from(&mut Reader::new(p.data(), &[])).unwrap();
            assert_eq!(noted.mode, MODE_ALLOWED);
            assert_eq!(noted.attribution_tag.as_deref(), tag);
        }
    }
}
