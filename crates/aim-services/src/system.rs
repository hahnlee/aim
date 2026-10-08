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

#[path = "system_staging.rs"]
mod staging;
#[path = "system_permission_mutation.rs"]
mod permission_mutation;
#[path = "system_preferred.rs"]
mod preferred;
#[path = "system_existing.rs"]
mod existing;
#[path = "system_events.rs"]
mod events;
#[path = "system_package_public_services.rs"]
mod package_public_services;
#[path = "system_package_runtime_creation.rs"]
mod package_runtime_creation;
#[cfg(test)]
#[path = "system_package_internal_usage_tests.rs"]
mod internal_usage_tests;
#[path = "system_verification.rs"]
mod verification;
#[path = "system_installer_confirmation.rs"]
mod installer_confirmation;
#[path = "system_package_lifecycle.rs"]
pub mod package_lifecycle;
#[path = "system_removal_factory.rs"]
mod removal_factory;
#[path = "system_installer_archive.rs"]
mod installer_archive;
#[path = "system_package_launch.rs"]
mod package_launch;
#[path = "system_package_metadata.rs"]
mod package_metadata;
#[path = "system_package_verification_init.rs"]
mod package_verification_init;
#[path = "system_package_preferred_init.rs"]
mod package_preferred_init;
#[path = "system_package_maintenance_init.rs"]
mod package_maintenance_init;
#[path = "system_package_internal_installs.rs"]
mod internal_installs;
#[path = "system_package_internal_suspension.rs"]
mod internal_suspension;
#[path = "system_package_internal_persistence.rs"]
mod internal_persistence;
#[path = "system_package_internal_storage.rs"]
mod internal_storage;
#[path = "system_package_internal_events.rs"]
mod internal_events;
#[path = "system_package_internal_implicit.rs"]
mod internal_implicit;
#[path = "system_package_internal_archive.rs"]
mod internal_archive;
#[path = "system_package_observers.rs"]
mod package_observers;
#[path = "system_package_runtime.rs"]
pub mod package_runtime;
#[path = "system_package_web_policy.rs"]
mod package_web_policy;
#[path = "system_package_boot_scan.rs"]
pub mod package_boot_scan;
#[path = "system_package_boot_inputs.rs"]
pub mod package_boot_inputs;
#[path = "system_package_environment_init.rs"]
mod package_environment_init;
#[path = "system_package_boot_lifecycle.rs"]
mod package_boot_lifecycle;
#[path = "system_package_dexopt_completion.rs"]
mod package_dexopt_completion;
#[path = "system_package_original_domains.rs"]
pub mod original_domains;
mod legacy_domain;

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
    diagnostic_self: Weak<Self>,
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
    package_public_services: Mutex<Option<package_public_services::Pair>>,
    package_install_lock: Arc<Mutex<()>>,
    package_signing: Mutex<Option<Arc<crate::package::sign::Overrides>>>,
    package_boot_image: Mutex<Option<Arc<crate::package::boot_image::Owner>>>,
    package_boot_image_configuration: Mutex<Option<crate::package::boot_image::Configuration>>,
    package_requested: std::sync::atomic::AtomicBool,
    package_property_area: Mutex<Option<std::path::PathBuf>>,
}

#[derive(Default)]
struct PackageBootstrapState {
    current: Option<PackageBootstrap>,
    version: u64,
}

/// IO completes before callback shutdown; retain outside publication locks.
pub struct InstallerWorkers {
    _events: Option<crate::package::events::Workers>,
    _commit:Option<crate::package::installer::commit::Worker>,
    _approval:Option<crate::package::installer::preapproval::Worker>,
    _constraints:Option<crate::package::installer::constraints::Worker>,
    _io: Option<crate::package::installer::file_bridge::IoWorkerGuard>,
    _callbacks: crate::package::installer::callbacks::CallbackWorker,
}
impl InstallerWorkers {
    pub fn is_finished(&self) -> bool {
        self._commit.as_ref().is_none_or(|worker|worker.is_finished()) && self._approval.as_ref().is_none_or(|worker|worker.is_finished()) && self._constraints.as_ref().is_none_or(|worker|worker.is_finished()) && self._io.as_ref().is_none_or(|io| io.is_finished()) && self._callbacks.is_finished()
    }
}

#[derive(Default)]
struct BootConfiguration {
    values:Mutex<BootConfigurationValues>,
}
#[derive(Default)]
struct BootConfigurationValues {
    policy:Option<(package_boot_inputs::Cli,crate::package::parse::resources::Config)>,
    persistence:Option<crate::system_package_persistence_init::Inputs>,
    facts:Option<package_runtime::BootFacts>,
    early:Option<Arc<crate::package::boot_configuration::Early>>,
}

struct PackageBootstrap {
    boot_configuration:Arc<BootConfiguration>,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    snapshots: Option<Arc<crate::package::scan_snapshot::Store>>,
    queries: Option<Arc<crate::package::scan_snapshot::query_state::Capture>>,
    key_set_tokens: Option<Arc<crate::package::keysets::Tokens>>,
    installer_files: Option<Arc<crate::package::installer::hardlink::Files>>,
    installer_external: Option<Arc<crate::package::installer::preapproval::BridgeOwner>>,
    install_environment: Option<Arc<crate::package::installer::environment::Config>>,
    factory_library_settings: Option<Arc<crate::package::installer::environment_producers::LibrarySettings>>,
    internal_host: Option<Binder>,
    dexopt_completion: Option<Binder>,
    package_observers: Option<Arc<crate::package::observers::Owner>>,
    boot_lifecycle: Option<Arc<package_boot_lifecycle::Runtime>>,
    web_policy: Option<Arc<package_web_policy::Runtime>>,
    boot_session: Option<Weak<crate::package::boot_session::Session>>,
    boot_session_starting: bool,
    mutation_reservations: Vec<Weak<crate::package::mutation_reservation::Reservation>>,
    internal_installs: Option<Arc<crate::package::installer::internal_installs::Owner>>,
    internal_persistence: Option<Arc<internal_persistence::Runtime>>,
    original_domains: Option<Weak<original_domains::Runtime>>,
    existing_installer: Option<Arc<crate::package::installer::existing::Owner>>,
    policies: Option<Arc<crate::package::policies::Owner>>,
    maintenance: Option<Arc<crate::package::diagnostics::Runtime>>,
    effects: Option<Arc<crate::package::effects::Owner>>,
    mutations: Option<Arc<crate::package::mutation_dispatch::Dependencies>>,
    changes: Arc<crate::package::changes::Owner>,
    customization: Option<Arc<crate::package::customization::Owner>>,
    dirty_restrictions: std::collections::BTreeSet<i32>,
    events: Option<Arc<crate::package::events::Owner>>,
    moves: Option<Arc<crate::package::moves::Owner>>,
    relocation: Option<Arc<crate::package::move_package::Owner>>,
    application_data: Option<Arc<crate::package::application_data::Owner>>,
    verification_agent: Option<Arc<crate::package::domain_verification::agent::Runtime>>,
    pending_verification: Option<Arc<crate::package::install_verification::Owner>>,
    legacy_domain_verification: Option<Arc<crate::package::legacy_domain_mutation::Runtime>>,
    public_removal: Option<Arc<crate::package::query::removal::Owner>>,
    user_policy: Arc<crate::package::user_policy::Owner>,
    uri_access: Arc<crate::package::uri_access::Owner>,
    instant_registry: Option<Arc<crate::package::instant::Owner>>,
    staging: Option<Arc<crate::package::staging::Owner>>,
    hold_locks: Arc<crate::package::hold_lock::Owner>,
    preferred_registry: Option<Arc<crate::package::preferred::registry::Registry>>,
    preferred_commit: Option<crate::package::preferred::registry::Commit>,
    preferred_actions: Option<Arc<dyn crate::package::preferred::registry::Actions>>,
    runtime_metadata: Option<Arc<Mutex<crate::package::owner::runtime_metadata::State>>>,
    runtime_worker: Option<crate::package::owner::runtime_metadata::worker::StopHandle>,
    persistence: Option<Arc<Mutex<crate::package::owner::Store>>>,
    version_page: Option<crate::package::scan_snapshot::version_page::VersionPage>,
    installer: Option<(
        Arc<crate::package::installer::native::NativeOwners>,
        aim_binder_host::parcel::Binder,
    )>,
}

impl Drop for PackageBootstrap {
    fn drop(&mut self) {
        if let Some(owner) = &self.boot_lifecycle { owner.stop(); }
        if let Some(owner) = &self.package_observers { owner.stop(); }
        for reservation in self.mutation_reservations.drain(..).filter_map(|reservation| reservation.upgrade()) {
            reservation.close();
        }
        if let Some(owner) = &self.internal_installs { owner.stop(); }
        if let Some(owner) = &self.internal_persistence { owner.stop(); }
        if let Some(external) = &self.installer_external {
            external.revoke();
        }
        if let Some(maintenance) = &self.maintenance {
            maintenance.stop();
        }
        if let Some(effects) = &self.effects {
            effects.revoke();
        }
        if let Some(events) = &self.events {
            events.close();
        }
        if let Some(moves) = &self.moves {
            moves.close();
        }
        if let Some(relocation) = &self.relocation {
            relocation.close();
        }
        if let Some(data) = &self.application_data {
            data.stop();
        }
        if let Some(verification) = &self.pending_verification {
            verification.stop();
        }
        if let Some(instant) = &self.instant_registry {
            instant.stop();
        }
        if let Some(existing)=&self.existing_installer {existing.restores.shutdown();}
        self.hold_locks.close();
        if let Some(files) = &self.installer_files {
            files.close();
        }
        if let Some(staging) = &self.staging {
            staging.close();
        }
        if let Some(preferred) = &self.preferred_registry {
            preferred.revoke();
        }
        if let Some((installer, _)) = &self.installer {
            installer.shutdown_callbacks();
        }
        if let Some(worker) = &self.runtime_worker {
            worker.stop();
        }
    }
}

impl PackageBootstrap {
    #[track_caller]
    fn publish_snapshot(&mut self, candidate: impl Into<Arc<crate::package::scan_snapshot::Store>>) {
        let candidate = candidate.into();
        match &self.snapshots {
            Some(current) => {
                let before = current.capture();
                if self.queries.as_ref().is_some_and(|query| !Arc::ptr_eq(query.scan(), &before)) {
                    eprintln!("Canonical/query publication lag at {}: canonical={} query={:?} candidate={}: {}",
                        std::panic::Location::caller(), before.version(),
                        self.queries.as_ref().map(|query| query.scan().version()), candidate.capture().version(),
                        crate::package::scan_snapshot::install_context::install_identity_delta(&before, &candidate.capture()));
                }
                current.publish_validated_store(&candidate);
            }
            None => self.snapshots = Some(candidate),
        }
    }
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
                diagnostic_self: this.clone(),
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
                package_public_services: Mutex::new(None),
                package_install_lock: Arc::new(Mutex::new(())),
                package_signing: Mutex::new(None),
                package_boot_image: Mutex::new(None),
                package_boot_image_configuration: Mutex::new(None),
                package_requested: std::sync::atomic::AtomicBool::new(false),
                package_property_area: Mutex::new(None),
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
        drop(signing);
        let this = Arc::downgrade(self);
        let attached = Arc::downgrade(&bridge);
        let policy_system = this.clone();
        let policy_bridge = bridge.clone();
        let parent_system = this.clone(); let parent_bridge = bridge.clone();
        let linking_system = this.clone(); let linking_bridge = bridge.clone();
        let roles_system = this.clone(); let roles_bridge = bridge.clone();
        let grants_system = this.clone();
        let grants_bridge = bridge.clone();
        let clone_system = this.clone();
        let clone_bridge = bridge.clone();
        let uri_access = Arc::new(crate::package::uri_access::Owner::new(
            Box::new(move |uid, provider, user, _check_user| {
                let system = grants_system.upgrade().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "provider grants system stopped"))?;
                system.check_package_bootstrap(&grants_bridge)?;
                let granted = grants_bridge.provider_authority_grants(uid, provider, user).map_err(|error| match error {
                    crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                    error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("provider URI owner: {error:?}")),
                })?;
                system.check_package_bootstrap(&grants_bridge)?;
                Ok(granted)
            }),
            Box::new(move |authority, uid, user| {
                let system = clone_system.upgrade().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "provider clone system stopped"))?;
                system.check_package_bootstrap(&clone_bridge)?;
                let redirected = clone_bridge.provider_clone_redirected(authority, uid, user).map_err(|error| match error {
                    crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                    error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("provider clone owner: {error:?}")),
                })?;
                system.check_package_bootstrap(&clone_bridge)?;
                Ok(redirected)
            }),
        ));
        let user_policy = Arc::new(crate::package::user_policy::Owner::new(Box::new(move |user| {
            let system = policy_system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "package user policy system stopped"))?;
            system.check_package_bootstrap(&policy_bridge)?;
            let restricted = policy_bridge.shell_debugging_restricted(user).map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package shell policy owner: {error:?}")),
            })?;
            system.check_package_bootstrap(&policy_bridge)?;
            Ok(restricted)
        })).with_cross_profile(
            Box::new(move |user| {
                let system = parent_system.upgrade().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "profile parent system stopped"))?;
                system.read_package_live_owner(&parent_bridge, |bridge| bridge.package_profile_parent(user))
            }),
            Box::new(move |user| {
                let system = linking_system.upgrade().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "profile linking system stopped"))?;
                system.read_package_live_owner(&linking_bridge, |bridge| bridge.parent_profile_app_linking(user))
            }),
        ).with_role_holders(Box::new(move |role, user| {
            let system = roles_system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "role system stopped"))?;
            system.read_package_live_owner(&roles_bridge, |bridge| bridge.package_role_holders(role, user))
        })));
        let mut current = self.package_bootstrap.lock().unwrap();
        let previous = current.current.replace(PackageBootstrap {
            bridge: bridge.clone(),
            boot_configuration:Arc::new(BootConfiguration::default()),
            snapshots: None,
            queries: None,
            key_set_tokens: None,
            installer_files: None,
            installer_external: None,
            install_environment: None,
            factory_library_settings: None,
            internal_host: None,
            dexopt_completion: None,
            package_observers: None,
            boot_lifecycle: None,
            web_policy: None,
            boot_session: None,
            boot_session_starting: false,
            mutation_reservations: Vec::new(),
            internal_installs: None,
            internal_persistence: None,
            original_domains: None,
            existing_installer: None,
            policies: None,
            maintenance: None,
            effects: None,
            mutations: None,
            changes: Arc::new(crate::package::changes::Owner::default()),
            customization: None,
            dirty_restrictions: std::collections::BTreeSet::new(),
            events: None,
            moves: None,
            relocation: None,
            application_data: None,
            verification_agent: None,
            pending_verification: None,
            legacy_domain_verification: None,
            public_removal: None,
            user_policy,
            uri_access,
            instant_registry: None,
            staging: None,
            hold_locks: crate::package::hold_lock::Owner::new(self.process.clone(),bridge.signing_debuggable),
            preferred_registry: None,
            preferred_commit: None,
            preferred_actions: None,
            runtime_metadata: None,
            runtime_worker: None,
            persistence: None,
            version_page: None,
            installer: None,
        });
        drop(current);
        if let Some(previous) = previous {
            if let Some((installer, _)) = &previous.installer {
                if let Err(error) = installer.close_external() {
                    eprintln!("Installer production owners close failed: {}", error.message);
                }
            } else if let Some(external) = &previous.installer_external {
                if let Err(error) = external.close() {
                    eprintln!("Installer external owner close failed: {}", error.message);
                }
            }
            drop(previous);
        }
        self.process.link_to_death(
            &bridge.owner,
            Box::new(move || {
                if let (Some(system), Some(attached)) = (this.upgrade(), attached.upgrade()) {
                    let session=system.package_bootstrap.lock().unwrap().current.as_ref()
                        .filter(|owner|Arc::ptr_eq(&owner.bridge,&attached))
                        .and_then(|owner|owner.boot_session.as_ref()).and_then(Weak::upgrade);
                    if let Some(session)=session {
                        if let Err(error)=session.close(){eprintln!("Package session death cleanup failed: {}",error.message);}
                    }
                    else if let Err(error)=system.detach_package_bootstrap(&attached){
                        eprintln!("Package bootstrap death cleanup failed: {}",error.message);
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
                    current.publish_snapshot(store);
                    capture
                }),
            _ => return Err(fail("package scan publication base differs")),
        };
        let snapshot =
            result.map_err(|error| fail(&format!("package scan publication failed: {error:?}")))?;
        current.queries = None;
        if let Some(page) = &current.version_page {
            page.publish(snapshot.version());
        }
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
        self.publish_package_scan_with_query_roles(bridge, base, owner, usage, context, None)
    }

    /// The native constructor supplies actual framework configuration before publication.
    pub fn publish_package_scan_with_configured_roles(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        context: crate::package::scan_snapshot::query_state::Context,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
        properties: &dyn Fn(&str) -> Option<String>,
        system_config: &crate::package::system_config::SystemConfig,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        let debuggable = match properties("ro.debuggable").as_deref() {
            Some("0") => false,
            Some("1") => true,
            _ => return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "original ro.debuggable property owner unavailable")),
        };
        if debuggable != bridge.signing_debuggable {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package build property and original bootstrap disagree"));
        }
        let eng_build = match properties("ro.build.type").as_deref() {
            Some("eng") => true,
            Some("user" | "userdebug") => false,
            _ => return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "original build type property owner unavailable")),
        };
        self.publish_package_scan_with_query_roles(
            bridge,
            base,
            owner,
            usage,
            context,
            Some((platform, config, debuggable, eng_build, system_config)),
        )
    }

    fn publish_package_scan_with_query_roles(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: Option<&Arc<crate::package::scan_snapshot::Snapshot>>,
        owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
        context: crate::package::scan_snapshot::query_state::Context,
        roles: Option<(
            &crate::package::parse::Platform,
            crate::package::parse::resources::Config,
            bool,
            bool,
            &crate::package::system_config::SystemConfig,
        )>,
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
        let mut context = context;
        if base.is_none() { context.scan_version = version; }
        if let Some(tokens) = &context.system.key_set_tokens {
            if !current
                .key_set_tokens
                .as_ref()
                .is_some_and(|installed| Arc::ptr_eq(installed, tokens))
            {
                return Err(fail("keyset bootstrap owner differs".into()));
            }
        }
        context.system.diagnostic_dates = Some(Arc::new(crate::package::dump::Dates::new(bridge.clone(), self.diagnostic_self.clone())));
        context.system.key_set_tokens = current.key_set_tokens.clone();
        context.system.user_policy = Some(current.user_policy.clone());
        context.system.uri_access = Some(current.uri_access.clone());
        let store = Store::new_replica_at_version(owner, usage, version)
            .map_err(|error| fail(format!("package query scan validation failed: {error:?}")))?;
        let capture = Capture::new(store.capture(), context).map_err(|error|fail(format!("initial publication: {error}")))?;
        let capture = if let Some((platform, config, debuggable, eng_build, system_config)) = roles {
            let capture = capture
                .select_configured_roles(platform, config)
                .map_err(&fail)?;
            let capture=capture.select_permission_controller().map_err(&fail)?;
            let capture=capture.prepare_known_packages(platform,config,system_config.overlay_config_signature_package().map_err(&fail)?).map_err(&fail)?;
            let capture=capture.bind_default_browser_source(bridge.clone()).map_err(&fail)?;
            let capture = capture.prepare_instant_components(platform, config, debuggable, eng_build)
                .map_err(&fail)?;
            let capture = capture
                .load_page_size_compat_resources(platform, config)
                .map_err(&fail)?;
            if capture.state().system.key_set_tokens.is_some() {
                capture
            } else {
                capture
                    .prepare_keysets(self.process.clone())
                    .map_err(&fail)?
            }
        } else {
            capture
        };
        current.key_set_tokens = capture.state().system.key_set_tokens.clone();
        current.publish_snapshot(store);
        current.queries = Some(capture.clone());
        if let Some(page) = &current.version_page {
            page.publish(version);
        }
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
        let mut context = context;
        context.system.user_policy = Some(self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .map(|current| current.user_policy.clone())
            .ok_or_else(|| fail("package user policy bootstrap changed".into()))?);
        context.system.uri_access = Some(self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .map(|current| current.uri_access.clone())
            .ok_or_else(|| fail("package URI policy bootstrap changed".into()))?);
        let capture =
            crate::package::scan_snapshot::query_state::Capture::new(scan.clone(), context)
                .map_err(fail)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| fail("package query bootstrap owner changed".into()))?;
        if let Some(tokens) = &capture.state().system.key_set_tokens {
            if !current
                .key_set_tokens
                .as_ref()
                .is_some_and(|installed| Arc::ptr_eq(installed, tokens))
            {
                return Err(fail("keyset bootstrap owner differs".into()));
            }
        }
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

    pub(crate) fn package_state_version_page(&self) -> Result<aim_binder_driver::File> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package bootstrap unavailable",
            )
        })?;
        let version = current
            .queries
            .as_ref()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package queries unavailable",
                )
            })?
            .scan()
            .version();
        if current.version_page.is_none() {
            current.version_page = Some(
                crate::package::scan_snapshot::version_page::VersionPage::new(version).map_err(
                    |error| {
                        Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
                    },
                )?,
            );
        }
        current
            .version_page
            .as_ref()
            .unwrap()
            .file()
            .map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
            })
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
        let resolution = capture.resolution().map_err(|e| {
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
            self
                .is_native_domain_verifier_uid(uid)
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
    pub fn publish_runtime_package_domains(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        update: crate::package::scan_snapshot::query_state::RuntimeDomainUpdate,
    ) -> Result<Option<Arc<crate::package::scan_snapshot::query_state::Capture>>> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let version = state.version;
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime domain bootstrap owner changed",
                )
            })?;
        if !current
            .queries
            .as_ref()
            .is_some_and(|capture| Arc::ptr_eq(capture, &update.base))
        {
            return Ok(None);
        }
        if Some(update.capture.scan().version()) != version.checked_add(1) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime domain generation differs",
            ));
        }
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page {
            page.publish(update.capture.scan().version());
        }
        state.version = update.capture.scan().version();
        Ok(Some(update.capture))
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
        // Bind to the actual Settings writer/parser projection. Native query
        // metadata (including UID-less APEX code) remains in its scan owner.
        let persistent_base = persistence.persistent_scan_settings(update.base.scan().owner())?;
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
            != identities(&persistent_base)
        {
            let stored = identities(&persistence.state().settings);
            let captured = identities(&persistent_base);
            return Err(before(&format!("domain persistence package owner differs: stored-only={:?}; captured-only={:?}",
                stored.difference(&captured).take(4).map(|(name, uid, path, version, domain, _)|
                    (name, uid, path, version, domain)).collect::<Vec<_>>(),
                captured.difference(&stored).take(4).map(|(name, uid, path, version, domain, _)|
                    (name, uid, path, version, domain)).collect::<Vec<_>>())));
        }
        let canonical = |mut value: crate::package::domain_verification::State| {
            value.active.sort_by(|a, b| a.name.cmp(&b.name));
            value.restored.sort_by(|a, b| a.name.cmp(&b.name));
            value
        };
        if canonical(persistence.state().settings.domain_verification.clone())
            != canonical(persistent_base.domain_verification.clone())
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
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page {
                page.publish(update.capture.scan().version());
            }
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

    pub(crate) fn capture_package_scan_and_queries(
        &self,
    ) -> Result<(
        Arc<crate::package::scan_snapshot::Snapshot>,
        Option<Arc<crate::package::scan_snapshot::query_state::Capture>>,
    )> {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_ref().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package bootstrap unavailable",
            )
        })?;
        let snapshot = current
            .snapshots
            .as_ref()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "complete package scan unavailable",
                )
            })?
            .capture();
        Ok((snapshot, current.queries.clone()))
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

    /// Recover settings under the same retained early owner used by native scan.
    /// Missing/replaced bridges abort before selection or frontend publication.
    /// The caller supplies complete record owners and keeps partial mutations.
    pub(crate) fn recover_package_settings(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        data: &std::path::Path,
        users: &[u32],
        settings: &mut crate::package::settings::Settings,
        mut read: impl FnMut(
            &[u8],
            &mut crate::package::settings::Settings,
        ) -> std::result::Result<
            Option<aim_android_xml::Element>,
            crate::package::owner::recovery::ReadError,
        >,
    ) -> std::result::Result<
        (
            crate::package::owner::Store,
            crate::package::owner::recovery::Report,
        ),
        crate::package::owner::recovery::Error,
    > {
        use crate::package::owner::recovery::{Error, Plan, ReadError};
        let check = || {
            self.check_package_bootstrap(bridge)
                .map_err(|error| format!("settings bootstrap owner: {error:?}"))
        };
        check().map_err(|message| Error {
            events: Vec::new(),
            message,
        })?;
        let current_version = bridge.current_package_version().map_err(|error| Error {
            events: Vec::new(),
            message: format!("current settings build owner: {error:?}"),
        })?;
        check().map_err(|message| Error {
            events: Vec::new(),
            message,
        })?;
        let recovered = Plan::inspect(data)?.recover_boot(
            users,
            settings,
            &current_version,
            |bytes, settings| {
                check().map_err(ReadError::Owner)?;
                let document = read(bytes, settings)?;
                check().map_err(ReadError::Owner)?;
                Ok(document)
            },
        )?;
        check().map_err(|message| Error {
            events: recovered.1.events.clone(),
            message,
        })?;
        Ok(recovered)
    }

    /// Run the exclusive frontend through readLPw completion before user files.
    /// The same retained bridge protects file reads and pending/group completion.
    pub fn recover_package_settings_frontend(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        data: &std::path::Path,
        users: &[u32],
        settings: &mut crate::package::settings::Settings,
        mut frontend: impl FnMut(
            crate::package::owner::recovery::ReadStage<'_>,
            &mut crate::package::settings::Settings,
        ) -> std::result::Result<
            Option<aim_android_xml::Element>,
            crate::package::settings::ReadError,
        >,
    ) -> std::result::Result<
        (
            crate::package::owner::Store,
            crate::package::owner::recovery::Report,
        ),
        crate::package::owner::recovery::Error,
    > {
        use crate::package::owner::recovery::{Error, Plan, ReadError};
        let check = || {
            self.check_package_bootstrap(bridge)
                .map_err(|error| format!("settings bootstrap owner: {error:?}"))
        };
        check().map_err(|message| Error {
            events: Vec::new(),
            message,
        })?;
        let current = bridge.current_package_version().map_err(|error| Error {
            events: Vec::new(),
            message: format!("current settings build owner: {error:?}"),
        })?;
        check().map_err(|message| Error {
            events: Vec::new(),
            message,
        })?;
        let result = Plan::inspect(data)?.recover_boot_frontend(
            users,
            settings,
            &current,
            |stage, settings| {
                check().map_err(ReadError::Owner)?;
                let root = frontend(stage, settings)?;
                check().map_err(ReadError::Owner)?;
                Ok(root)
            },
        )?;
        check().map_err(|message| Error {
            events: result.1.events.clone(),
            message,
        })?;
        Ok(result)
    }

    /// Generate packages.list from native captured metadata and active-user GIDs.
    pub fn commit_package_list_from_scan(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        active_users: &[i32],
    ) -> std::result::Result<(), crate::package::owner::WriteError> {
        use crate::package::owner::WriteError;
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: false,
                message: format!("package list bootstrap owner: {error:?}"),
            })?;
        store.validate_committed_scan(capture.scan().owner())?;
        let mut entries =
            crate::package::list::metadata_from_capture(capture).map_err(|message| WriteError {
                committed: false,
                message,
            })?;
        for entry in &mut entries {
            entry.gids = bridge
                .permission_gids(entry.uid as i32, active_users)
                .map_err(|error| WriteError {
                    committed: false,
                    message: format!("package list permission owner: {error:?}"),
                })?;
        }
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: false,
                message: format!("package list bootstrap owner before commit: {error:?}"),
            })?;
        store.commit_package_list(&entries)?;
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: true,
                message: format!("package list bootstrap owner after commit: {error:?}"),
            })
    }

    /// Caller retains the joining guard outside bootstrap publication locks.
    pub fn install_package_installer(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        owner: Arc<crate::package::installer::native::NativeOwners>,
    ) -> Result<InstallerWorkers> {
        owner.configure_writer(self.package_installer_write_mode_source(bridge)?,
            self.package_installer_allocator(bridge)?)?;
        owner.configure_domain_policy(self.package_installer_domain_policy(bridge)?)?;
        let events = self.configure_package_events(bridge)?;
        let existing=self.existing_package_owner()?;
        if !Arc::ptr_eq(&existing.bridge,bridge){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"existing installer bootstrap differs"));}
        match owner.existing_owner() {
            Ok(installed) if Arc::ptr_eq(&installed,&existing)=>{},
            Ok(_)=>return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"foreign existing installer owner")),
            Err(_)=>owner.set_existing_owner(existing)?,
        }
        owner.configure_staging(self.configure_package_staging(bridge)?)?;
        owner.configure_files(self.package_installer_files(bridge)?)?;
        owner.configure_external(self.package_installer_external(bridge)?,owner.confirmation_source())?;
        if !owner.commit_configured() {
            owner.configure_commit(self.package_installer_commit_policy(bridge)?)?;
        }
        let worker = owner.take_callback_worker().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "installer callback guard already taken",
            )
        })?;
        let io = owner.take_optional_io_worker_guard()?;
        let commit=owner.take_commit_worker();let approval=owner.take_approval_worker();let constraints=owner.take_constraint_worker();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| {
                Arc::ptr_eq(&current.bridge, bridge)
                    && current
                        .queries
                        .as_ref()
                        .is_some_and(|query| Arc::ptr_eq(query, capture))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "installer bootstrap generation changed",
                )
            })?;
        if current.installer.is_some() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "native installer already installed",
            ));
        }
        let endpoint = crate::package::installer::endpoint::Endpoint {
            sessions: owner.sessions.clone(),
            owners: owner.clone(),
        };
        let binder = self.process.add_service(Arc::new(endpoint));
        current.installer = Some((owner, binder));
        Ok(InstallerWorkers {
            _events: events,
            _commit:commit,_approval:approval,_constraints:constraints,
            _io: io,
            _callbacks: worker,
        })
    }

    pub(crate) fn package_installer_native_owner(&self) -> Result<Arc<crate::package::installer::native::NativeOwners>> {
        self.package_bootstrap.lock().unwrap().current.as_ref()
            .and_then(|current| current.installer.as_ref().map(|(owner,_)| owner.clone()))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native installer owner unavailable"))
    }

    pub(crate) fn package_installer(&self) -> Result<aim_binder_host::parcel::Binder> {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_ref().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "installer bootstrap unavailable",
            )
        })?;
        current
            .installer
            .as_ref()
            .map(|(_, binder)| *binder)
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native package installer unavailable",
                )
            })
    }

    pub fn package_installer_policy_source(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: crate::package::installer::native::PolicySource,
    ) -> Result<crate::package::installer::native::PolicySource> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move |uid, user| {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "installer system owner stopped",
                )
            })?;
            system.check_package_bootstrap(&bridge)?;
            let mut policy = base(uid, user)?;
            match bridge.installer_user_policy(user).map_err(|error| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("installer user policy owner: {error:?}"),
                )
            })? {
                Some(user_policy) => {
                    policy.users.insert(user, user_policy);
                }
                None => {
                    policy.users.remove(&user);
                }
            }
            system.check_package_bootstrap(&bridge)?;
            Ok(policy)
        }))
    }

    pub fn package_installer_labeler(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<crate::package::installer::storage::Labeler> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move |_, guest| {
            let fail = |message: String| crate::package::installer::storage::Error {
                committed: false,
                message,
            };
            let system = system
                .upgrade()
                .ok_or_else(|| fail("installer system owner stopped".into()))?;
            system
                .check_package_bootstrap(&bridge)
                .map_err(|error| fail(format!("installer label bootstrap: {error:?}")))?;
            bridge
                .restore_installer_context(guest)
                .map_err(|error| fail(format!("installer label owner: {error:?}")))?;
            system
                .check_package_bootstrap(&bridge)
                .map_err(|error| fail(format!("installer label owner changed: {error:?}")))
        }))
    }

    /// Retain the exclusive disk owner for native Binder mutations.
    pub fn install_package_persistence(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        persistence: Arc<Mutex<crate::package::owner::Store>>,
    ) -> Result<()> {
        // Disk owner precedes bootstrap in the lock order, as in persistence.
        let disk = persistence.lock().unwrap();
        disk.validate_committed_scan(capture.scan().owner())
            .map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
            })?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|owner| {
                Arc::ptr_eq(&owner.bridge, bridge)
                    && owner
                        .queries
                        .as_ref()
                        .is_some_and(|q| Arc::ptr_eq(q, capture))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package persistence generation changed",
                )
            })?;
        if current.persistence.is_some() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package persistence owner already installed",
            ));
        }
        let blocks = crate::package::mutations::UninstallBlocks::from_state(disk.state());
        if capture.state().system.uninstall_blocks.as_deref() != Some(&blocks) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package uninstall-block capture differs from restored disk owner"));
        }
        current.persistence = Some(persistence.clone());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn package_persistence_owner(
        &self,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Result<(
        Arc<crate::package::bootstrap::Bridge>,
        Arc<Mutex<crate::package::owner::Store>>,
    )> {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_ref()
            .filter(|owner| {
                owner
                    .queries
                    .as_ref()
                    .is_some_and(|q| Arc::ptr_eq(q, capture))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package persistence generation changed",
                )
            })?;
        let persistence = current.persistence.clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "native package persistence is unavailable",
            )
        })?;
        Ok((current.bridge.clone(), persistence))
    }

    pub(crate) fn commit_package_uninstall_block(
        &self,
        request: &crate::package::mutations::BlockUninstall,
        resolver: &crate::package::resolve::Resolver,
        uid: i32,
    ) -> std::result::Result<std::result::Result<bool, Exception>, crate::package::resolve::QueryError> {
        use crate::package::{apps_filter::NotModelled, resolve::QueryError};
        let (bridge, persistence) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_ref().ok_or(QueryError::NotModelled(NotModelled("native uninstall-block bootstrap unavailable")))?;
            (current.bridge.clone(), current.persistence.clone().ok_or(QueryError::NotModelled(NotModelled("native uninstall-block persistence unavailable")))?)
        };
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge)
            && current.persistence.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &persistence)))
            .ok_or(QueryError::NotModelled(NotModelled("uninstall-block bootstrap owner changed")))?;
        let capture = current.queries.clone().ok_or(QueryError::NotModelled(NotModelled("native uninstall-block capture unavailable")))?;
        let resolution = match resolver.resolution(capture.state()) {
            Ok(resolution) => resolution,
            Err(error) => {
                let reply = error.reply().map_err(QueryError::Transport)?;
                return Ok(Reader::new(reply.data(), reply.objects()).read_exception()
                    .map_err(QueryError::Transport)?.map(|()| false));
            }
        };
        let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid };
        match request.decide(&query).map_err(QueryError::NotModelled)? {
            Err(error) => return Ok(Err(error)),
            Ok(false) => return Ok(Ok(false)),
            Ok(true) => {}
        }
        let mut blocks = capture.state().system.uninstall_blocks.as_deref()
            .ok_or(QueryError::NotModelled(NotModelled("native uninstall-block owner unavailable")))?.clone();
        blocks.set(request);
        let update = match capture.prepare_uninstall_blocks_update(blocks.clone()) {
            Ok(update) => update,
            Err(error) => return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))),
        };
        // The original commits Settings first and schedules disk writes afterward.
        // Serialization or unknown-user failures cannot undo the live mutation.
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        let persisted = u32::try_from(request.user).map_err(|error| error.to_string())
            .and_then(|user| disk.commit_uninstall_blocks(user, &blocks).map_err(|error| error.to_string()));
        drop(disk);
        if let Err(error) = persisted {
            eprintln!("uninstall-block restrictions write failed after runtime publication: {error}");
        }
        if let Err(error) = self.check_package_bootstrap(&bridge) {
            return Ok(Err(error));
        }
        if let Err(error) = bridge.invalidate_package_info_cache() {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("uninstall-block cache invalidation failed after publication: {error:?}"))));
        }
        Ok(Ok(true))
    }

    pub(crate) fn set_native_installer_attribution(
        &self, request: crate::package::installer_attribution::Request, uid: i32,
    ) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        let effects = self.package_effects_owner(&bridge)?;
        let resolver = crate::package::resolve::Resolver::default();
        let mut retained: Option<crate::package::installer_attribution::Plan> = None;
        loop {
            self.check_package_bootstrap(&bridge)?;
            let capture = self.capture_package_queries()?;
            let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("installer attribution visibility: {error:?}")))?;
            let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid };
            let plan = match &retained {
                Some(plan) => plan.reauthorize(&query, &effects)?,
                None => request.prepare(&query, &effects)?,
            };
            let Some(plan) = plan else { return Ok(()); };
            retained = Some(plan.clone());
            let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, &bridge)).and_then(|current| current.persistence.clone())
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "installer attribution disk unavailable"))?;
            let mut disk = persistence.lock().unwrap();
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "installer attribution bootstrap changed"))?;
            if !current.queries.as_ref().is_some_and(|current| Arc::ptr_eq(current, &capture)) {
                drop(state); drop(disk); continue;
            }
            disk.validate_committed_scan(capture.scan().owner()).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
            let mut owner = capture.scan().owner().clone();
            plan.apply_scan(&mut owner).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
            let update = capture.prepare_package_update(owner.clone()).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
            let written = plan.persist(&mut disk, &owner);
            if written.is_ok() || written.as_ref().is_err_and(|error| error.committed) {
                current.publish_snapshot(update.store);
                current.queries = Some(update.capture.clone());
                if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
                state.version = update.capture.scan().version();
            }
            let committed = written.is_ok() || written.as_ref().is_err_and(|error| error.committed);
            drop(state); drop(disk);
            let invalidated = if committed { plan.finish_publication(&bridge) } else { Ok(()) };
            written.map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
            return invalidated;
        }
    }

    pub(crate) fn set_package_page_size_override(&self, name: Option<&str>, enabled: bool, uid: i32) -> Result<()> {
        if !matches!(crate::package::apps_filter::app_id(uid), 0 | 1000) {
            return Err(Exception::security("Caller must be the system or root."));
        }
        let bridge = self.package_bootstrap()?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .and_then(|current| current.persistence.clone()).ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "page size settings owner unavailable"))?;
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "page size bootstrap changed"))?;
        let base = current.queries.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "page size capture unavailable"))?;
        let mut owner = base.scan().owner().clone();
        let package = owner.settings.packages.iter_mut().find(|package| Some(package.name.as_str()) == name)
            .ok_or_else(|| Exception::illegal_argument(format!("Unknown package: {}", name.unwrap_or("null"))))?;
        package.set_page_size_compat(if enabled { 32 } else { 64 }).map_err(Exception::illegal_argument)?;
        let update = base.prepare_package_update(owner.clone()).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        let persisted = disk.commit_scan_settings_owner(&owner);
        drop(disk);
        if let Err(error) = persisted { eprintln!("page size settings write: {error}"); }
        bridge.invalidate_package_info_cache().map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("page size cache owner: {error:?}")))
    }

    pub(crate) fn flush_native_package_restrictions(
        &self, query: &crate::package::query::Query<'_>, user: i32,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .and_then(|current| current.persistence.clone()).ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "restriction disk owner unavailable"))?;
        let mut disk = persistence.lock().unwrap();
        self.check_package_bootstrap(&bridge)?;
        crate::package::customization::flush(query, &mut disk, base.scan().owner(), user, |user| {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge)
                && current.queries.as_ref().is_some_and(|current| Arc::ptr_eq(current, base)))
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "restriction flush generation changed"))?;
            current.dirty_restrictions.remove(&user);
            Ok(())
        })
    }

    pub fn install_package_app_metadata_files(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, data: &std::path::Path,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let owner = Arc::new(crate::package::app_metadata::Owner::new(data));
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app metadata bootstrap changed"))?;
        let capture = current.queries.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "app metadata capture unavailable"))?;
        if capture.state().system.app_metadata_files.is_some() {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app metadata owner already installed"));
        }
        let update = capture.prepare_app_metadata_files(owner).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }

    fn read_package_live_owner<T>(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        read: impl FnOnce(&crate::package::bootstrap::Bridge) -> std::result::Result<T, crate::package::bootstrap::OwnerError>,
    ) -> Result<T> {
        self.check_package_bootstrap(bridge)?;
        let value = read(bridge).map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package live owner: {error:?}")),
        })?;
        self.check_package_bootstrap(bridge)?;
        Ok(value)
    }

    pub fn install_package_customization(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let owner = crate::package::customization::Owner::load(platform, config)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "customization bootstrap changed"))?;
        if current.customization.is_some() {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "customization owner already installed"));
        }
        current.customization = Some(owner);
        Ok(())
    }

    pub(crate) fn set_native_keep_uninstalled_packages(
        &self, query: &crate::package::query::Query<'_>, packages: Option<Vec<Option<String>>>,
    ) -> Result<()> {
        let customization = self.package_customization()?;
        let removal = self.public_package_removal()?;
        customization.set_keep(query, packages, |package| {
            let package = package.to_owned();
            let controller = removal.controller.clone();
            removal.events.post(false, Box::new(move || {
                match controller.delete_x(&package, -1, 0,
                    crate::package::installer::removal::ALL_USERS, true) {
                    Ok(_) => {},
                    Err(error) => eprintln!("Unused package removal failed: {}", error.message),
                }
            }))
        })
    }

    pub(crate) fn package_customization(&self) -> Result<Arc<crate::package::customization::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.customization.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native customization owner unavailable"))
    }

    pub(crate) fn apply_package_label_override(
        &self, query: &crate::package::query::Query<'_>, request: crate::package::customization::Label,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        let effects = self.package_effects_owner(&bridge)?;
        let plan = self.package_customization()?.prepare_label(query, request)?;
        if !plan.changed { return Ok(()); }
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge)
            && current.queries.as_ref().is_some_and(|current| Arc::ptr_eq(current, base)))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "label override generation changed"))?;
        let mut owner = base.scan().owner().clone();
        plan.apply_scan(&mut owner).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let update = base.prepare_package_update(owner).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        plan.finish(&effects, query.calling_uid)?;
        bridge.invalidate_package_info_cache().map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("label cache owner: {error:?}")))
    }

    pub fn install_package_verification(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<crate::package::install_verification::Worker> {
        self.check_package_bootstrap(bridge)?;
        let (owner, worker) = crate::package::install_verification::Owner::start();
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)) {
                Some(current) if current.pending_verification.is_none() => {
                    current.pending_verification = Some(owner.clone()); true
                }
                _ => false,
            }
        };
        if !installed {
            owner.stop(); drop(worker);
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "verification bootstrap changed or already installed"));
        }
        Ok(worker)
    }

    pub(crate) fn pending_package_verification(&self) -> Result<Arc<crate::package::install_verification::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.pending_verification.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native verification handler unavailable"))
    }

    pub fn install_public_package_removal(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        controller: Arc<crate::package::installer::removal::Controller>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let owner = crate::package::query::removal::Owner::new(controller, self.process.clone(), self.package_events()?);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "public removal bootstrap changed"))?;
        if current.public_removal.is_some() {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "public removal owner already installed"));
        }
        current.public_removal = Some(owner);
        Ok(())
    }

    pub(crate) fn public_package_removal(&self) -> Result<Arc<crate::package::query::removal::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.public_removal.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native public removal unavailable"))
    }

    pub fn install_package_application_data(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        commit: crate::package::application_data::Commit,
    ) -> Result<crate::package::application_data::Worker> {
        self.check_package_bootstrap(bridge)?;
        let transport = bridge.application_data().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("application data bridge: {error:?}")),
        })?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let current = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "application data system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            system.capture_package_queries()
        });
        let (owner, worker) = crate::package::application_data::Owner::start(transport,
            self.process.clone(), current, commit, self.package_install_lock.clone());
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)) {
                Some(current) if current.application_data.is_none() => { current.application_data = Some(owner.clone()); true }
                _ => false,
            }
        };
        if !installed {
            owner.stop(); drop(worker);
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "application data bootstrap changed or already installed"));
        }
        Ok(worker)
    }

    pub(crate) fn package_application_data(&self) -> Result<Arc<crate::package::application_data::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.application_data.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native application data owner unavailable"))
    }

    pub fn install_package_relocation(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        files: crate::package::move_package::Files,
        installer: Arc<dyn crate::package::move_package::InstallOwner>,
    ) -> Result<crate::package::move_package::Workers> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let lifecycle = capture.state().system.lifecycle.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "relocation lifecycle unavailable"))?;
        let effects = self.package_effects_owner(bridge)?;
        let callbacks = self.package_moves()?;
        let transport = bridge.package_relocation().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("relocation bridge: {error:?}")),
        })?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let source = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "relocation system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            Ok(system.capture_package_queries()?.state().clone())
        });
        let (owner, workers) = crate::package::move_package::Owner::new(source, files,
            crate::package::move_package::StorageOwner::new(transport), installer, effects,
            lifecycle, callbacks, self.package_install_lock.clone());
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)) {
                Some(current) if current.relocation.is_none() => { current.relocation = Some(owner.clone()); true }
                _ => false,
            }
        };
        if !installed {
            owner.close(); drop(workers);
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "relocation bootstrap changed or already installed"));
        }
        Ok(workers)
    }

    pub(crate) fn package_relocation_owner(&self) -> Result<Arc<crate::package::move_package::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.relocation.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native relocation owner unavailable"))
    }

    pub fn install_native_package_mutations(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        pipeline: Arc<crate::package::installer::pipeline::Native>,
    ) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as aidl;
        self.check_package_bootstrap(bridge)?;
        let deletion = self.public_package_removal()?;
        let existing = self.existing_package_owner()?;
        let effects = self.package_effects_owner(bridge)?;
        let preferred = self.package_preferred_registry(bridge)?;
        let lifecycle = self.capture_package_queries()?.state().system.lifecycle.clone().ok_or_else(||
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation lifecycle unavailable"))?;
        let changes = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge)).map(|current| current.changes.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation bootstrap changed"))?;
        let mut request = Parcel::new(); aidl::GetPackageEnableBridge {}.write(&mut request);
        let reply = bridge.owner.transact(aidl::GET_PACKAGE_ENABLE_BRIDGE, &request, false)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("enable owner transport: {status}")))?;
        let mut reader = reply.reader();
        let binder = aidl::read_get_package_enable_bridge_reply(&mut reader)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("enable owner reply: {status}")))??
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "enable owner unavailable"))?;
        if reader.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "enable owner trailing bytes")); }
        let transport = reply.retain_remote_binder(binder).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("enable owner lifetime: {status}")))?;
        let enable = crate::package::mutation_producers::Bridge::new(transport);
        let quarantine_enabled = enable.quarantine_enabled()?;
        let system = Arc::downgrade(self); let retained = bridge.clone();
        let source = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            Ok(system.capture_package_queries()?.state().clone())
        });
        let producer = crate::package::mutation_producers::Owner::new(pipeline,
            deletion.controller.store.clone(), source, existing, deletion, effects.clone(),
            lifecycle, enable, self.process.clone(), self.package_install_lock.clone());
        let compressed = producer.clone();
        let system = Arc::downgrade(self); let retained = bridge.clone();
        let preferred_system = system.clone(); let preferred_bridge = bridge.clone(); let registry = preferred.clone();
        self.install_package_mutations(bridge, Arc::new(crate::package::mutation_dispatch::Dependencies {
            effects, changes, preferred,
            shell_restricted: Box::new(move |user| {
                let system = system.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation shell owner stopped"))?;
                system.check_package_bootstrap(&retained)?;
                system.package_shell_debugging_policy(user)
            }),
            enable_compressed: Box::new(move |name, user| compressed.enable_compressed(name, user)),
            system_install_state: Box::new(move |name, installed, user| producer.system_install_state(name, installed, user)),
            commit_preferred: Box::new(move |stage| {
                let system = preferred_system.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation preferred owner stopped"))?;
                system.commit_package_preferred_stage(&preferred_bridge, &registry, stage, false)
            }),
            quarantine_enabled,
        }))
    }

    fn package_bootstrap_binder_leaf(&self, bridge: &Arc<crate::package::bootstrap::Bridge>, code: u32) -> Result<Strong> {
        self.package_bootstrap_binder_leaf_with(bridge, code, |_| {})
    }
    fn package_bootstrap_binder_leaf_with(&self, bridge: &Arc<crate::package::bootstrap::Bridge>, code: u32,
        write: impl FnOnce(&mut Parcel),
    ) -> Result<Strong> {
        self.check_package_bootstrap(bridge)?;
        let mut request = Parcel::new();
        request.write_interface_token(aim_service_aidl::dev_aim_server_ipackagebootstrapbridge::DESCRIPTOR);
        write(&mut request);
        let reply = bridge.owner.transact(code, &request, false).map_err(|status| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package owner transport: {status}")))?;
        let mut reader = reply.reader();
        reader.read_exception().map_err(|status| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package owner exception: {status}")))??;
        let binder = reader.read_binder().map_err(|status| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package owner binder: {status}")))?
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "package owner unavailable"))?;
        if reader.remaining() != 0 { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "package owner reply trailing bytes")); }
        let owner = reply.retain_remote_binder(binder).map_err(|status| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package owner lifetime: {status}")))?;
        self.check_package_bootstrap(bridge)?;
        Ok(owner)
    }

    pub fn package_diagnostic_owners(self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>, data: &std::path::Path,
        messages: crate::package::diagnostic_inputs::SettingsMessages,
    ) -> Result<(Arc<crate::package::diagnostic_inputs::SettingsMessages>, Arc<crate::package::diagnostic_inputs::Maintenance>)> {
        use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap, dev_aim_server_ipackagediagnosticinputs as diagnostic};
        let leaf = self.package_bootstrap_binder_leaf(bridge, bootstrap::GET_PACKAGE_DIAGNOSTIC_INPUTS)?;
        let system = Arc::downgrade(self); let retained = bridge.clone();
        let art = Arc::new(move |package: Option<&str>| {
            let system = system.upgrade().ok_or("ART diagnostic system stopped")?;
            system.check_package_bootstrap(&retained).map_err(|error| format!("{error:?}"))?;
            let mut request = Parcel::new();
            diagnostic::DumpDexopt { package_name: package.map(str::to_owned) }.write(&mut request);
            let reply = leaf.transact(diagnostic::DUMP_DEXOPT, &request, false).map_err(|status| format!("ART diagnostic transport: {status}"))?;
            let mut reader = reply.reader();
            let text = diagnostic::read_dump_dexopt_reply(&mut reader).map_err(|status| format!("ART diagnostic reply: {status}"))?
                .map_err(|error| format!("ART diagnostic owner: {error:?}"))?.ok_or("ART diagnostic text absent")?;
            if reader.remaining() != 0 { return Err("ART diagnostic trailing bytes".into()); }
            system.check_package_bootstrap(&retained).map_err(|error| format!("{error:?}"))?;
            Ok(text)
        });
        let compiler = crate::package::diagnostic_inputs::CompilerStats::read(data).map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        Ok((Arc::new(messages), Arc::new(crate::package::diagnostic_inputs::Maintenance { compiler, art })))
    }

    pub fn initialize_package_internal_owners(self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        kernel: Arc<crate::package::kernel_mappings::Owner>,
    ) -> Result<crate::package::installer::internal_installs::Worker> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
        let permission = self.package_bootstrap_binder_leaf(bridge, bootstrap::GET_PERMISSION_PERSISTENCE_BRIDGE)?;
        let persistence = internal_persistence::Runtime::start(self, bridge.clone(), permission, kernel)?;
        self.install_internal_persistence(persistence)?;
        let apex = self.package_bootstrap_binder_leaf(bridge, bootstrap::GET_PACKAGE_APEX_UNINSTALL_BRIDGE)?;
        self.initialize_package_internal_installs(bridge, apex)
    }

    pub(crate) fn publish_package_user_transition(&self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>, scan: crate::package::scan::SigningScan,
        user: i32, created: bool, record: Option<crate::package::user_operations::UserRecord>,
    ) -> Result<Arc<crate::package::scan_snapshot::query_state::Capture>> {
        self.check_package_bootstrap(bridge)?;
        let update = base.prepare_user_transition(scan, user, created, record, bridge).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.queries.as_ref().is_some_and(|capture| Arc::ptr_eq(capture, base)))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "user transition base changed"))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(update.capture)
    }

    pub(crate) fn native_package_has_launcher(&self, package: &str, user: i32, flags: i64) -> Result<bool> {
        let capture = self.capture_package_queries()?;
        let resolution = capture.resolution().map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("new-user launcher resolution: {error:?}")))?;
        let intent = crate::package::intent::Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec!["android.intent.category.LAUNCHER".into()]),
            package: Some(package.into()), ..Default::default()
        };
        resolution.query_intent_activities(&intent, None, flags, user, 1000)
            .map(|activities| !activities.is_empty()).map_err(|error| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("new-user launcher query: {error:?}")))
    }

    pub(crate) fn apply_native_default_preferred_apps(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, user: i32,
    ) -> Result<aim_android_xml::Element> {
        self.default_preferred_for_new_user(bridge, user)
    }

    pub(crate) fn commit_native_package_preferred_stage(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, stage: crate::package::preferred::registry::Stage,
    ) -> Result<()> {
        let registry = self.package_preferred_registry(bridge)?;
        self.commit_package_preferred_stage(bridge, &registry, stage, false)
    }

    pub(crate) fn add_native_default_cross_profile_filter(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, filter: crate::package::intent_filter::IntentFilter,
        owner: Option<String>, source: i32, target: i32, flags: i32,
    ) -> Result<()> {
        use crate::package::query::preferred::Owner;
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let handle = capture.state().system.preferred_owner.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "cross-profile preferred owner unavailable"))?;
        let level = handle.actions.cross_access(1000, source, target, true)
            .map_err(|error| match error {
                crate::package::preferred::registry::ActionError::Exception(error) => error,
                crate::package::preferred::registry::ActionError::Unavailable(error) => Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0),
            })?;
        let owner = owner.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,
            "null cross-profile owner package"))?;
        let snapshot = handle.snapshot(source).map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?.ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "cross-profile source user unavailable"))?;
        handle.actions.commit_mutation(source, snapshot.generation,
            &crate::package::preferred::Mutation::CrossAdd(crate::package::preferred::CrossProfileIntentFilter {
                filter, owner_package: owner, target_user_id: target, flags, access_control: level,
            }), 1000).map(|_| ()).map_err(|error| match error {
                crate::package::preferred::registry::ActionError::Exception(error) => error,
                crate::package::preferred::registry::ActionError::Unavailable(error) => Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0),
            })
    }

    pub(crate) fn remove_native_unused_packages_for_user(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, capture: &Arc<crate::package::scan_snapshot::query_state::Capture>, user: i32,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let keep = self.package_customization()?;
        let removal = self.public_package_removal()?;
        for package in capture.state().packages.values() {
            if package.users.iter().any(|(id, state)| *id != user && state.installed)
                || keep.should_keep(Some(&package.name)) { continue; }
            let name = package.name.clone();
            let controller = removal.controller.clone();
            removal.events.post(false, Box::new(move || {
                if let Err(error) = controller.delete_x(&name, -1, 0,
                    crate::package::installer::removal::ALL_USERS, true) {
                    eprintln!("Removed user unused package deletion failed: {}", error.message);
                }
            }))?;
        }
        Ok(())
    }

    pub(crate) fn binder_process(&self) -> Arc<LocalProcess> { self.process.clone() }

    pub(crate) fn publish_internal_package_mutation(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        version: i64, bytes: &[u8],
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        if i64::try_from(base.scan().version()).ok() != Some(version) { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal mutation expected version differs")); }
        let record = crate::package::internal_mutation_record::Record::read(bytes).map_err(|status| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT, format!("internal mutation record: {status}")))?;
        let gate=self.package_install_guard();
        loop {
        let latest=self.capture_package_queries()?;
        self.check_package_bootstrap(bridge)?;
        let applied = crate::package::internal_mutation_apply::apply_rebased(base.scan(),latest.scan(), &record).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let update = latest.prepare_internal_mutation(applied.scan.clone(), applied.usage).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let disk = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge)).and_then(|current| current.persistence.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal mutation persistence unavailable"))?;
        let disk_owner=disk.clone();
        let mut disk = disk.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.persistence.as_ref().is_some_and(|owner| Arc::ptr_eq(owner,&disk_owner)))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal mutation bootstrap or persistence retired"))?;
        if !current.queries.as_ref().is_some_and(|capture| Arc::ptr_eq(capture, &latest)) {
            // Usage/context publication intentionally does not take install gate.
            // Nothing has been written: discard this prepared candidate and
            // rebase the same verified field operation on the newest owner.
            // Do not replay the original Java mutator callback or hold the
            // publication coordinator across full graph/RPC preparation.
            drop(state); drop(disk); continue;
        }
        disk.validate_committed_scan(latest.scan().owner()).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
        let mut written = Ok(());
        if applied.settings_changed { written = disk.commit_scan_settings_owner(&applied.scan); }
        let mut persisted = applied.settings_changed && (written.is_ok() || written.as_ref().is_err_and(|error| error.committed));
        let mut committed = written.is_ok() || written.as_ref().is_err_and(|error| error.committed);
        if written.is_ok() {
            for user in &applied.changed_users {
                let result = disk.commit_updated_scan_restrictions(&applied.scan, *user as u32, false);
                match result {
                    Ok(()) => persisted = true,
                    Err(mut error) => {
                        error.committed |= persisted;
                        committed = error.committed;
                        written = Err(error);
                        break;
                    }
                }
            }
        }
        if committed {
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
            state.version = update.capture.scan().version();
        }
        drop(state); drop(disk); drop(gate);
        let invalidated = if committed { bridge.invalidate_package_info_cache().map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("internal mutation cache owner: {error:?}"))) } else { Ok(()) };
        written.map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
        return invalidated;
        }
    }

    pub(crate) fn reserve_package_mutation(self: &Arc<Self>) -> Result<Binder> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation bootstrap unavailable"))?;
        let capture = current.queries.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation capture unavailable"))?;
        let reservation = Arc::new(crate::package::mutation_reservation::Reservation::new(
            self, current.bridge.clone(), capture, current.changes.sequence()));
        current.mutation_reservations.retain(|reservation| reservation.strong_count() != 0);
        current.mutation_reservations.push(Arc::downgrade(&reservation));
        Ok(self.process.add_service(reservation))
    }

    pub(crate) fn detach_package_bootstrap(&self, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        let detached = {
            let mut state = self.package_bootstrap.lock().unwrap();
            if state.current.as_ref().is_some_and(|current| Arc::ptr_eq(&current.bridge, bridge)) {
                state.current.take()
            } else { None }
        };
        let Some(detached) = detached else { return Ok(()); };
        let closed = match &detached.installer {
            Some((installer, _)) => installer.close_external(),
            None => match &detached.installer_external { Some(external) => external.close(), None => Ok(()) },
        };
        let observers = match &detached.package_observers { Some(owner) => owner.close(), None => Ok(()) };
        let web = match &detached.web_policy { Some(owner) => owner.close(), None => Ok(()) };
        drop(detached);
        closed.and(observers).and(web)
    }

    pub(crate) fn package_internal_host(self: &Arc<Self>) -> Result<Binder> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal package bootstrap unavailable"))?;
        if let Some(binder) = current.internal_host { return Ok(binder); }
        let endpoint = crate::package::internal_host::Endpoint::new(Arc::downgrade(self), current.bridge.clone());
        let binder = self.process.add_service(Arc::new(endpoint));
        current.internal_host = Some(binder);
        Ok(binder)
    }

    pub(crate) fn package_installer_publisher(&self) -> Result<crate::package::installer::native::Publisher> {
        let process = self.process.clone();
        Ok(Arc::new(move |service| Ok(process.add_service(service))))
    }

    pub(crate) fn install_archive_query_owner(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, owner: Arc<crate::package::archive::Owner>,
    ) -> Result<()> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "archive bootstrap changed"))?;
        let base = current.queries.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "archive capture unavailable"))?;
        if base.state().system.archive_owner.is_some() { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "archive query owner already installed")); }
        let update = base.prepare_archive_owner(owner).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }

    pub fn install_package_launch_owner(self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        let owner = self.package_launch_owner(bridge)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "launch bootstrap changed"))?;
        let base = current.queries.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "launch capture unavailable"))?;
        if base.state().system.launch_sender.is_some() { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "launch owner already installed")); }
        let update = base.prepare_launch_owner(owner).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }

    pub fn install_package_mutations(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        dependencies: Arc<crate::package::mutation_dispatch::Dependencies>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation bootstrap changed"))?;
        if current.mutations.is_some() {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation owner already installed"));
        }
        if !current.effects.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &dependencies.effects))
            || !Arc::ptr_eq(&current.changes, &dependencies.changes)
            || !current.preferred_registry.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &dependencies.preferred)) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "mutation producer differs from bootstrap owners"));
        }
        current.mutations = Some(dependencies);
        Ok(())
    }

    pub(crate) fn dispatch_package_mutation(
        &self, call: &mut Call<'_>, query: &crate::package::query::Query<'_>,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Option<Result<Parcel>> {
        if !crate::package::mutation_dispatch::handles(call.code) { return None; }
        let retained = {
            let state = self.package_bootstrap.lock().unwrap();
            state.current.as_ref().and_then(|current| current.mutations.as_ref()
                .map(|dependencies| (current.bridge.clone(), dependencies.clone())))
        };
        let (bridge, dependencies) = retained?;
        let install = self.package_install_guard();
        let capture = match self.capture_package_queries() { Ok(capture)=>capture, Err(error)=>return Some(Err(error)) };
        let resolution = match capture.resolution() { Ok(value)=>value, Err(error)=>return Some(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,format!("mutation current resolution: {error:?}")))) };
        let current_query = crate::package::query::Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:query.calling_uid};
        // Classify the actual typed request, not broad transaction families.
        // Component setters never enter the compressed installation owner.
        if crate::package::mutation_dispatch::owns_install_side_effect(call.code,&mut call.data,&current_query) {
            drop(install);
            let prepared=crate::package::mutation_dispatch::prepare(call.code,&mut call.data,&current_query,call.sender_pid,&dependencies)?;
            return Some(prepared.and_then(|prepared|self.publish_prepared_package_mutation(&bridge,&capture,prepared,&dependencies,call.sender_euid as i32)));
        }
        let prepared=crate::package::mutation_dispatch::prepare(call.code,&mut call.data,&current_query,call.sender_pid,&dependencies)?;
        Some(prepared.and_then(|prepared|self.publish_prepared_package_mutation_guarded(&bridge,&capture,prepared,&dependencies,call.sender_euid as i32,install)))
    }

    pub fn publish_prepared_package_mutation(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        prepared: crate::package::mutation_dispatch::Prepared,
        dependencies: &crate::package::mutation_dispatch::Dependencies, caller: i32,
    ) -> Result<Parcel> {
        let install = self.package_install_guard();
        self.publish_prepared_package_mutation_guarded(bridge, base, prepared, dependencies, caller, install)
    }

    /// Callers needing capture+prepare atomicity acquire the same native install
    /// gate first and transfer it here. Original callbacks run after its release.
    pub(crate) fn publish_prepared_package_mutation_guarded(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        prepared: crate::package::mutation_dispatch::Prepared,
        dependencies: &crate::package::mutation_dispatch::Dependencies, caller: i32,
        install: std::sync::MutexGuard<'_, ()>,
    ) -> Result<Parcel> {
        self.check_package_bootstrap(bridge)?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "mutation disk owner unavailable"))?;
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.persistence.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &persistence)))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "prepared mutation generation changed"))?;
        // Select latest while holding the publication coordinator, after the
        // disk lock. Freezer/other context publications cannot race this rebase.
        let latest = current.queries.clone().ok_or_else(||Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"prepared mutation current capture unavailable"))?;
        if !Arc::ptr_eq(&latest,base) { prepared.validate_rebase(base.scan().owner(),latest.scan().owner()).map_err(|error|Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,error))?; }
        let base = &latest;
        let before = base.state().clone();
        let mut written = Ok(());
        let after = if prepared.changes_state() {
            disk.validate_committed_scan(base.scan().owner()).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
            let mut owner = base.scan().owner().clone();
            prepared.apply_scan(&mut owner).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
            let update = base.prepare_package_update(owner.clone()).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
            written = prepared.persist(&mut disk, &owner);
            if written.as_ref().is_err_and(|error| !error.committed) {
                return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    written.unwrap_err().to_string()));
            }
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
            state.version = update.capture.scan().version();
            update.capture.state().clone()
        } else { before.clone() };
        drop(state);
        drop(disk);
        drop(install);
        // Original AM/UM/permission/broadcast owners can reenter native PM here.
        let finished = prepared.finish(dependencies, &before, &after, caller);
        let invalidated = if prepared.changes_state() {
            bridge.invalidate_package_info_cache().map_err(|error| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("mutation cache owner: {error:?}")))
        } else { Ok(()) };
        written.map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
        finished?;
        invalidated?;
        Ok(prepared.reply)
    }

    pub fn package_effects_owner(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::effects::Owner>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if let Some(owner) = state.current.as_ref().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.effects.clone()) { return Ok(owner); }
        }
        let strong = bridge.package_effects().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("package effects owner: {error:?}")),
        })?;
        let effects = crate::package::effects::Owner::new(strong)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "package effects bootstrap changed"))?;
        Ok(current.effects.get_or_insert(effects).clone())
    }

    pub fn package_changes_owner(&self) -> Result<Arc<crate::package::changes::Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().map(|current| current.changes.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "changed packages owner unavailable"))
    }

    pub fn install_package_instant_registry(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, data: &std::path::Path,
    ) -> Result<crate::package::instant::Worker> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let density = bridge.instant_icon_density().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("instant configuration owner: {error:?}")),
        })?;
        let retained=bridge.clone();
        let limit:crate::package::instant::CookieLimitSource=Arc::new(move||retained.instant_cookie_limit()
            .map_err(|error|format!("instant cookie policy owner: {error:?}")));
        let (owner, worker) = crate::package::instant::Owner::open_with_cookie_limit(data, capture.state(), limit, density)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let prepared = capture.prepare_instant_registry_update(owner.clone());
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.queries.as_ref().is_some_and(|current| Arc::ptr_eq(current, &capture))
            && current.instant_registry.is_none());
        let result = match (current, prepared) {
            (Some(current), Ok(update)) => {
                current.instant_registry = Some(owner.clone());
                current.publish_snapshot(update.store);
                current.queries = Some(update.capture.clone());
                if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
                state.version = update.capture.scan().version();
                Ok(())
            }
            (_, Err(error)) => Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)),
            _ => Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "instant publication owner changed or already installed")),
        };
        drop(state);
        if let Err(error) = result {
            owner.stop(); drop(worker); return Err(error);
        }
        Ok(worker)
    }

    pub fn publish_package_instant_access(&self, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "instant bootstrap changed"))?;
        let owner = current.instant_registry.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "instant registry unavailable"))?;
        let capture = current.queries.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "instant capture unavailable"))?;
        let update = capture.prepare_instant_registry_update(owner).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }

    pub fn install_package_maintenance(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        effects: Arc<crate::package::effects::Owner>,
        removal: Arc<crate::package::installer::removal::Controller>,
        usage: crate::package::diagnostics::UsagePublication,
    ) -> Result<crate::package::diagnostics::Worker> {
        self.check_package_bootstrap(bridge)?;
        let installer = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.installer.as_ref().map(|(owner, _)| owner.clone()))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "maintenance installer owner unavailable"))?;
        let maintenance = bridge.package_maintenance().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("maintenance bridge owner: {error:?}")),
        })?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let current = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "maintenance system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            system.capture_package_queries()
        });
        let (runtime, worker) = crate::package::diagnostics::Runtime::attach(crate::package::diagnostics::Inputs {
            process: self.process.clone(), maintenance: Arc::new(maintenance), effects,
            install_serial: self.package_install_lock.clone(), current, removal, installer, usage,
        })?;
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)) {
                Some(current) if current.maintenance.is_none() => { current.maintenance = Some(runtime.clone()); true }
                _ => false,
            }
        };
        if !installed {
            runtime.stop(); drop(worker);
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "maintenance publication owner changed or already installed"));
        }
        Ok(worker)
    }

    pub fn package_maintenance_owner(&self) -> Result<Arc<crate::package::diagnostics::Runtime>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.maintenance.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native maintenance owner unavailable"))
    }

    pub fn package_installer_external(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::installer::preapproval::BridgeOwner>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if let Some(owner) = state.current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.installer_external.clone()) {
                return Ok(owner);
            }
        }
        let strong = bridge.installer_external().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("installer external owner: {error:?}")),
        })?;
        let owner = crate::package::installer::preapproval::BridgeOwner::new(strong);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "installer external bootstrap changed"))?;
        Ok(current.installer_external.get_or_insert(owner).clone())
    }

    pub fn package_policy_owner(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::policies::Owner>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if let Some(owner) = state.current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.policies.clone()) {
                return Ok(owner);
            }
        }
        let owner = Arc::new(crate::package::policies::Owner::capture(&bridge.owner)?);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package policy bootstrap changed"))?;
        Ok(current.policies.get_or_insert(owner).clone())
    }

    pub fn package_installer_files(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::installer::hardlink::Files>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if let Some(files) = state.current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.installer_files.clone()) {
                return Ok(files);
            }
        }
        let strong = bridge.installer_files().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("installer file owner: {error:?}")),
        })?;
        let files = crate::package::installer::hardlink::Files::new(strong);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "installer file bootstrap changed"))?;
        Ok(current.installer_files.get_or_insert(files).clone())
    }

    pub fn package_preferred_identity_provider(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<dyn crate::package::preferred::registry::IdentityProvider>> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        Ok(Arc::new(crate::package::preferred::records::BridgeProducer::new(
            bridge.clone(), Arc::new(move || {
                let system = system.upgrade().ok_or("preferred identity system stopped")?;
                system.check_package_bootstrap(&retained).map_err(|error| format!("{error:?}"))
            }),
        )))
    }

    pub fn package_installer_lite_policy(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        platform: Arc<crate::package::parse::Platform>,
        properties: Arc<dyn Fn() -> Result<std::collections::BTreeMap<String, String>> + Send + Sync>,
    ) -> Result<crate::package::installer::native::LitePolicySource> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "installer system owner stopped"))?;
            system.check_package_bootstrap(&bridge)?;
            let properties = properties()?;
            let enabled = bridge.installer_art_service_v3_enabled().map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("installer ART filter owner: {error:?}")),
            })?;
            system.check_package_bootstrap(&bridge)?;
            Ok(crate::package::installer::native::LitePolicy {
                environment: crate::package::parse::lite::Environment {
                    sdk: platform.sdk,
                    codenames: platform.codenames.clone(),
                    properties,
                },
                art_managed_extensions: if enabled { vec![".dm".into(), ".prof".into(), ".sdm".into()] }
                    else { vec![".dm".into()] },
            })
        }))
    }

    pub fn configure_native_installer_confirmation(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        installer: &Arc<crate::package::installer::native::NativeOwners>,
        apks: Arc<crate::package::write::Apks>, dependency_installer_enabled: bool,
    ) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as aidl;
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let resolver = Arc::new(crate::package::resolve::Resolver::default());
        let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("confirmation installer selection: {error:?}")))?;
        let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: 1000 };
        let roles = capture.state().system.roles.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation KnownPackages unavailable"))?;
        let names = roles.known_packages(&query, 2, 0).map_err(crate::package::installer::policy::unknown)?;
        let name = names.into_iter().flatten().next().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "required installer unavailable"))?;
        let mut request = Parcel::new();
        aidl::GetInstallerConfirmationBridge { dependency_installer_enabled }.write(&mut request);
        let reply = bridge.owner.transact(aidl::GET_INSTALLER_CONFIRMATION_BRIDGE, &request, false)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("confirmation leaf transport: {status}")))?;
        let mut reader = reply.reader();
        let binder = aidl::read_get_installer_confirmation_bridge_reply(&mut reader)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("confirmation leaf reply: {status}")))??
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation leaf unavailable"))?;
        if reader.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation leaf trailing bytes")); }
        let leaf = reply.retain_remote_binder(binder).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("confirmation leaf lifetime: {status}")))?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let source = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            Ok(system.capture_package_queries()?.state().clone())
        });
        let sessions = Arc::downgrade(installer);
        let stages = Box::new(move |session: &crate::package::installer::Session, record: &crate::package::installer::Record| {
            let sessions = sessions.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation sessions stopped"))?;
            let guest = sessions.confirmation_stage_path(session, record)?;
            (apks.files)(&guest).ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "confirmation stage VFS unavailable"))
        });
        let policy = installer_confirmation::Source::new(leaf, source, resolver, stages, name)?;
        self.check_package_bootstrap(bridge)?;
        installer.configure_confirmation(policy.policy_source())
    }

    pub fn install_native_package_relocation(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        pipeline: Arc<crate::package::installer::pipeline::Native>,
        claims: std::path::PathBuf,
    ) -> Result<crate::package::move_package::Workers> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let source = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "move install system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            Ok(system.capture_package_queries()?.state().clone())
        });
        let adapter = crate::package::installer::move_pipeline::Adapter::new(pipeline.clone(), source, claims);
        let apks = pipeline.apks.clone();
        self.install_package_relocation(bridge, Arc::new(move |path| (apks.files)(path)), adapter)
    }

    pub fn configure_native_install_pipeline(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        installer: &Arc<crate::package::installer::native::NativeOwners>,
        config: crate::package::installer::environment::Config,
    ) -> Result<Arc<crate::package::installer::pipeline::Native>> {
        self.check_package_bootstrap(bridge)?;
        let (snapshots, disk) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_ref().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "install pipeline bootstrap changed"))?;
            (current.snapshots.clone().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "install scan owner unavailable"))?,
             current.persistence.clone().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "install disk owner unavailable"))?)
        };
        if !Arc::ptr_eq(&snapshots, &config.snapshots)
            || !Arc::ptr_eq(&self.package_effects_owner(bridge)?, &config.effects) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "install pipeline uses foreign scan or effects owners"));
        }
        let library = Arc::new(self.package_installer_external(bridge)?.native_install_environment()?.library);
        let apks = config.apks.clone();
        let environment = crate::package::installer::environment::Owner::new(config)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let environment_config = environment.config();
        let environment = self.package_install_event_environment(bridge, environment)?;
        let pipeline = Arc::new(crate::package::installer::pipeline::Native {
            snapshots, disk, apks: apks.clone(), environment,
        });
        self.check_package_bootstrap(bridge)?;
        installer.configure_installation(apks, pipeline.clone())?;
        self.configure_package_install_preparation(bridge, installer, pipeline.clone())?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "install environment bootstrap changed"))?;
        if current.install_environment.is_some() { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "install environment already attached")); }
        current.install_environment = Some(environment_config);
        current.factory_library_settings = Some(library);
        Ok(pipeline)
    }

    fn configure_package_install_preparation(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        installer: &Arc<crate::package::installer::native::NativeOwners>,
        pipeline: Arc<crate::package::installer::pipeline::Native>,
    ) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as aidl;
        self.check_package_bootstrap(bridge)?;
        let mut request = Parcel::new(); aidl::GetInstallerPreparationBridge {}.write(&mut request);
        let reply = bridge.owner.transact(aidl::GET_INSTALLER_PREPARATION_BRIDGE, &request, false)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("install preparation transport: {status}")))?;
        let mut reader = reply.reader();
        let binder = aidl::read_get_installer_preparation_bridge_reply(&mut reader)
            .map_err(|status| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("install preparation reply: {status}")))??
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "install preparation owner unavailable"))?;
        if reader.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "install preparation trailing bytes")); }
        let staged = reply.retain_remote_binder(binder).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("staged owner lifetime: {status}")))?;
        let stream = reply.retain_remote_binder(binder).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("stream owner lifetime: {status}")))?;
        self.check_package_bootstrap(bridge)?;
        installer.configure_production_preparation(staged, stream, pipeline)
    }

    pub fn package_install_query_publication(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        snapshots: Arc<crate::package::scan_snapshot::Store>,
        context: crate::package::installer::permission_capture::ContextSource,
    ) -> Result<crate::package::installer::environment::QueryPublication> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move |scan| {
            let system = system.upgrade().ok_or("install publication system stopped")?;
            loop {
                system.check_package_bootstrap(&bridge).map_err(|error| format!("{error:?}"))?;
                let base = system.capture_package_queries().map_err(|error| format!("{error:?}"))?;
                let latest = snapshots.capture();
                let mut retained = scan.owner().clone();
                if !crate::package::scan_snapshot::install_context::rebase_install_users(
                    scan, &latest, &mut retained, &std::collections::BTreeSet::new())? {
                    return Err(format!("install publication canonical identity changed: {}",
                        crate::package::scan_snapshot::install_context::install_identity_delta(scan, &latest)));
                }
                let capture = crate::package::scan_snapshot::query_state::Capture::new(
                    latest.clone(), context(&latest)?)?;
                let mut state = system.package_bootstrap.lock().unwrap();
                let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                    .ok_or("install publication bootstrap changed")?;
                if !current.snapshots.as_ref().is_some_and(|store| Arc::ptr_eq(store, &snapshots)) {
                    return Err("install publication canonical store replaced".into());
                }
                if !Arc::ptr_eq(&snapshots.capture(), &latest)
                    || !current.queries.as_ref().is_some_and(|query| Arc::ptr_eq(query, &base)) {
                    drop(state);
                    continue;
                }
                current.queries = Some(capture);
                if let Some(page) = &current.version_page { page.publish(latest.version()); }
                state.version = latest.version();
                break;
            }
            bridge.invalidate_package_info_cache().map_err(|error|
                format!("installed package cache invalidation: {error:?}"))
        }))
    }

    pub fn install_package_freezer_publication(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let lifecycle = self.capture_package_queries()?.state().system.lifecycle.clone()
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "freezer lifecycle owner unavailable"))?;
        let owner = lifecycle.clone();
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        lifecycle.install_frozen_publisher(Arc::new(move |_revision, frozen| {
            let system = system.upgrade().ok_or("freezer system stopped")?;
            loop {
            let mut state = system.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or("freezer bootstrap changed")?;
            let base = current.queries.as_ref().ok_or("freezer capture unavailable")?;
            if !base.state().system.lifecycle.as_ref().is_some_and(|lifecycle| Arc::ptr_eq(lifecycle, &owner)) {
                return Err("freezer lifecycle changed".into());
            }
            let canonical = current.snapshots.as_ref().ok_or("freezer canonical scan unavailable")?.capture();
            if !Arc::ptr_eq(base.scan(), &canonical) {
                // An installation has persisted its scan and is completing its
                // query projection. Publish only the retained freezer view;
                // the install owner will project the real latest lifecycle map.
                current.queries = Some(base.with_frozen_view(frozen.clone())?);
                return Ok(());
            }
            let update = base.prepare_package_update(base.scan().owner().clone())?;
            let capture = base.with_frozen_packages(update.capture.scan().clone(), frozen.clone())?;
            let snapshots = current.snapshots.as_ref().ok_or("freezer canonical scan unavailable")?;
            match snapshots.publish_validated_store_after(&canonical, &update.store) {
                Err(crate::package::scan_snapshot::Error::Stale) => { drop(state); continue; }
                Err(error) => return Err(format!("freezer canonical publication: {error:?}")),
                Ok(_) => {}
            }
            current.queries = Some(capture.clone());
            if let Some(page) = &current.version_page { page.publish(capture.scan().version()); }
            state.version = capture.scan().version();
            return Ok(());
            }
        })).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))
    }

    pub fn package_installer_commit_policy(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<crate::package::installer::commit::PolicySource> {
        self.check_package_bootstrap(bridge)?;
        let external = self.package_installer_external(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move |uid, _receiver| {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "installer system owner stopped"))?;
            system.check_package_bootstrap(&bridge)?;
            let uid = i32::try_from(uid).map_err(|_| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT, "installer uid exceeds Android uid range"))?;
            let require_mutable_receiver = external.commit_mutable_receiver_enforced(uid)?;
            let secure_frp = external.secure_frp_active()?;
            let secure_frp_install_allowed = if secure_frp {
                let capture = system.capture_package_queries()?;
                let resolution = capture.resolution().map_err(|error|
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("FRP package resolution: {error:?}")))?;
                let query = crate::package::query::Query {
                    state: capture.state(), filter: &resolution.apps_filter, calling_uid: 1000,
                };
                let roles = capture.state().system.roles.as_ref().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "FRP KnownPackages owner unavailable"))?;
                let installers = roles.known_packages(&query, 2, 0)
                    .map_err(crate::package::installer::policy::unknown)?;
                let calling_package = query.packages_for_uid(uid)
                    .map_err(crate::package::installer::policy::unknown)?
                    .into_iter().flatten().flatten().find(|name|
                        capture.state().packages.get(name).is_some_and(|package| package.pkg.is_some()));
                let system_installer = calling_package.as_ref().is_some_and(|name|
                    installers.iter().any(|installer| installer.as_ref() == Some(name)));
                let allowed = !system_installer && system.check_permission("android.permission.INSTALL_PACKAGES", -1, uid)?;
                if !Arc::ptr_eq(&capture, &system.capture_package_queries()?) {
                    return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "FRP package selection changed"));
                }
                allowed
            } else { false };
            system.check_package_bootstrap(&bridge)?;
            Ok(crate::package::installer::commit::Policy {
                require_mutable_receiver, secure_frp, secure_frp_install_allowed,
            })
        }))
    }

    pub fn package_installer_domain_policy(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<crate::package::installer::native::DomainPolicySource> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "installer system owner stopped"))?;
            system.check_package_bootstrap(&bridge)?;
            let capture = system.capture_package_queries()?;
            let owner = capture.state().system.instant_components.as_ref().ok_or_else(||
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "instant app installer selection owner unavailable"))?;
            let package = owner.installer_package();
            let (count, length) = bridge.installer_domain_limits().map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                error => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("installer domain limits owner: {error:?}")),
            })?;
            system.check_package_bootstrap(&bridge)?;
            if !Arc::ptr_eq(&capture, &system.capture_package_queries()?) {
                return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "installer domain selection changed"));
            }
            Ok((count, length, package))
        }))
    }

    pub fn package_installer_allocator(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<crate::package::installer::native::AllocationOwner> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move |file, length, flags| {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "installer system owner stopped",
                )
            })?;
            system.check_package_bootstrap(&bridge)?;
            bridge
                .allocate_installer_bytes(file, length, flags)
                .map_err(|error| match error {
                    crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                    error => Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("installer allocation owner: {error:?}"),
                    ),
                })?;
            system.check_package_bootstrap(&bridge)
        }))
    }

    pub fn package_installer_write_mode_source(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<dyn Fn() -> Result<bool> + Send + Sync>> {
        self.check_package_bootstrap(bridge)?;
        let system = Arc::downgrade(self);
        let bridge = bridge.clone();
        Ok(Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "installer system owner stopped",
                )
            })?;
            system.check_package_bootstrap(&bridge)?;
            let mode = bridge.installer_revocable_fd_enabled().map_err(|error| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("installer write mode owner: {error:?}"),
                )
            })?;
            system.check_package_bootstrap(&bridge)?;
            Ok(mode)
        }))
    }

    pub(crate) fn package_shell_debugging_policy(&self, user: i32) -> Result<bool> {
        let bridge = self.package_bootstrap()?;
        let restricted = bridge.shell_debugging_restricted(user).map_err(|error| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("shell user policy owner: {error:?}"),
            )
        })?;
        self.check_package_bootstrap(&bridge)?;
        Ok(restricted)
    }

    pub(crate) fn commit_package_mutation(
        &self,
        request: &crate::package::write::mutation::Request,
        resolver: &crate::package::resolve::Resolver,
        uid: i32,
        pid: i32,
    ) -> std::result::Result<std::result::Result<(), Exception>, crate::package::resolve::QueryError>
    {
        use crate::package::{owner::WriteError, resolve::QueryError, write::mutation::Change};
        let (bridge, persistence) = {
            let state = self.package_bootstrap.lock().unwrap();
            let Some(current) = state.current.as_ref() else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native mutation bootstrap unavailable",
                )));
            };
            let Some(persistence) = current.persistence.clone() else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native package persistence is unavailable",
                )));
            };
            (current.bridge.clone(), persistence)
        };
        let mut disk = persistence.lock().unwrap();
        let restricted = match request {
            crate::package::write::mutation::Request::HarmfulWarning { user, .. }
                if uid == 2000 && *user >= 0 =>
            {
                if let Err(error) = self.check_package_bootstrap(&bridge) {
                    return Ok(Err(error));
                }
                let policy = match bridge.shell_debugging_restricted(*user) {
                    Ok(value) => value,
                    Err(error) => {
                        return Ok(Err(Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("shell user policy owner: {error:?}"),
                        )));
                    }
                };
                Some(policy)
            }
            _ => None,
        };
        let mut state = self.package_bootstrap.lock().unwrap();
        let Some(current) = state.current.as_mut().filter(|owner| {
            Arc::ptr_eq(&owner.bridge, &bridge)
                && owner
                    .persistence
                    .as_ref()
                    .is_some_and(|owner| Arc::ptr_eq(owner, &persistence))
        }) else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "mutation bootstrap owner changed",
            )));
        };
        let Some(capture) = current.queries.clone() else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "native mutation capture unavailable",
            )));
        };
        let resolution = match resolver.resolution(capture.state()) {
            Ok(resolution) => resolution,
            Err(error) => {
                let reply = error.reply().map_err(QueryError::Transport)?;
                return Ok(Reader::new(reply.data(), reply.objects())
                    .read_exception()
                    .map_err(QueryError::Transport)?);
            }
        };
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let plan = match request
            .decide_with_shell(&query, pid, restricted)
            .map_err(QueryError::NotModelled)?
        {
            Ok(plan) => plan,
            Err(exception) => return Ok(Err(exception)),
        };
        if !matches!(
            plan.change,
            Change::None
                | Change::SplashTheme(_)
                | Change::HarmfulWarning(_)
                | Change::CategoryHint(_)
                | Change::RelinquishUpdateOwner
                | Change::MinAspectRatio(_)
                | Change::UpdateAvailable(_)
        ) {
            return Err(QueryError::NotModelled(
                crate::package::apps_filter::NotModelled(
                    "mutation side-effect owners are unavailable",
                ),
            ));
        }
        let write_error = |error: WriteError| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
        };
        if let Err(error) = disk.validate_committed_scan(capture.scan().owner()) {
            return Ok(Err(write_error(error)));
        }
        if matches!(plan.change, Change::None) {
            return Ok(Ok(()));
        }
        let mut scan = capture.scan().owner().clone();
        if let Err(message) = plan.apply_scan(&mut scan) {
            return Ok(Err(write_error(WriteError {
                committed: false,
                message,
            })));
        }
        let update = match capture.prepare_package_update(scan) {
            Ok(update) => update,
            Err(message) => {
                return Ok(Err(write_error(WriteError {
                    committed: false,
                    message,
                })));
            }
        };
        let result = disk.commit_mutation(&plan);
        let committed = result.is_ok() || result.as_ref().is_err_and(|error| error.committed);
        if committed {
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page {
                page.publish(update.capture.scan().version());
            }
            state.version = update.capture.scan().version();
        }
        drop(state);
        drop(disk);
        if committed {
            if let Err(error) = self
                .check_package_bootstrap(&bridge)
                .map_err(|error| format!("{error:?}"))
                .and_then(|_| {
                    bridge
                        .invalidate_package_info_cache()
                        .map_err(|error| format!("{error:?}"))
                })
            {
                return Ok(Err(write_error(WriteError {
                    committed: true,
                    message: format!(
                        "mutation committed, cache invalidation failed: {error:?}; persistence: {result:?}"
                    ),
                })));
            }
        }
        Ok(result.map_err(write_error))
    }

    pub fn install_runtime_permission_metadata(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        metadata: Arc<Mutex<crate::package::owner::runtime_metadata::State>>,
    ) -> Result<()> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|owner| Arc::ptr_eq(&owner.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime metadata bootstrap owner changed",
                )
            })?;
        if current.runtime_metadata.is_some() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime metadata owner is already installed",
            ));
        }
        current.runtime_metadata = Some(metadata);
        Ok(())
    }

    fn runtime_permission_metadata_owner(
        &self,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
    ) -> Result<Arc<Mutex<crate::package::owner::runtime_metadata::State>>> {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_ref()
            .filter(|owner| {
                owner
                    .queries
                    .as_ref()
                    .is_some_and(|query| Arc::ptr_eq(query, capture))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime metadata query generation changed",
                )
            })?;
        current.runtime_metadata.clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime metadata owner is unavailable",
            )
        })
    }

    pub(crate) fn with_runtime_permission_metadata<T>(
        &self,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        action: impl FnOnce(&mut crate::package::owner::runtime_metadata::State) -> T,
    ) -> Result<T> {
        // Never wait for metadata while holding the bootstrap lock: flushes
        // hold metadata while checking the bootstrap around owner calls.
        let owner = self.runtime_permission_metadata_owner(capture)?;
        let mut metadata = owner.lock().unwrap();
        let state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_ref()
            .filter(|current| {
                current
                    .queries
                    .as_ref()
                    .is_some_and(|query| Arc::ptr_eq(query, capture))
                    && current
                        .runtime_metadata
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &owner))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime metadata generation changed while waiting",
                )
            })?;
        let _ = current;
        Ok(action(&mut metadata))
    }

    /// Persist the exact metadata owner mutated by Binder setters.
    pub fn flush_installed_runtime_permission_requests(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        inodes: &std::collections::BTreeMap<u32, aim_storage::guest_inode::GuestInode>,
    ) -> std::result::Result<Vec<u32>, crate::package::owner::runtime_metadata::FlushError> {
        let fail = |error: Exception| crate::package::owner::runtime_metadata::FlushError {
            user: -1,
            completed: Vec::new(),
            error: crate::package::owner::WriteError {
                committed: false,
                message: format!("installed runtime metadata: {error:?}"),
            },
        };
        self.check_package_bootstrap(bridge).map_err(fail)?;
        let owner = self
            .runtime_permission_metadata_owner(capture)
            .map_err(fail)?;
        let mut metadata = owner.lock().unwrap();
        metadata.bind_creation_metadata(inodes).map_err(|error| fail(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, error)))?;
        let current = self
            .runtime_permission_metadata_owner(capture)
            .map_err(fail)?;
        if !Arc::ptr_eq(&owner, &current) {
            return Err(fail(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime metadata owner changed while waiting",
            )));
        }
        let completed =
            self.flush_runtime_permission_requests(bridge, store, capture, &mut metadata, inodes)?;
        self.check_package_bootstrap(bridge).map_err(|error| {
            crate::package::owner::runtime_metadata::FlushError {
                user: -1,
                completed: completed.clone(),
                error: crate::package::owner::WriteError {
                    committed: !completed.is_empty(),
                    message: format!("runtime bootstrap after flush: {error:?}"),
                },
            }
        })?;
        Ok(completed)
    }

    /// Process elapsed async deadlines using the currently installed owner.
    pub fn process_due_runtime_permission_requests(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        capture: &Arc<crate::package::scan_snapshot::query_state::Capture>,
        inodes: &std::collections::BTreeMap<u32, aim_storage::guest_inode::GuestInode>,
        now: std::time::Instant,
    ) -> std::result::Result<Vec<u32>, crate::package::owner::runtime_metadata::FlushError> {
        let fail = |error: Exception| crate::package::owner::runtime_metadata::FlushError {
            user: -1,
            completed: Vec::new(),
            error: crate::package::owner::WriteError {
                committed: false,
                message: format!("runtime deadline owner: {error:?}"),
            },
        };
        self.check_package_bootstrap(bridge).map_err(fail)?;
        let owner = self
            .runtime_permission_metadata_owner(capture)
            .map_err(fail)?;
        let mut metadata = owner.lock().unwrap();
        let current = self
            .runtime_permission_metadata_owner(capture)
            .map_err(fail)?;
        if !Arc::ptr_eq(&owner, &current) {
            return Err(fail(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime deadline owner changed",
            )));
        }
        metadata.flush_due_with(now, |user, state| {
            let id = u32::try_from(user).map_err(|_| crate::package::owner::WriteError {
                committed: false,
                message: "negative runtime deadline user".into(),
            })?;
            let inode = state.creation_inode(id)
                    .ok_or_else(|| crate::package::owner::WriteError {
                        committed: false,
                        message: "missing runtime deadline creation owner".into(),
                    })?;
            self.commit_runtime_permissions_from_scan(bridge, store, capture, id, state, inode)?;
            Ok(id)
        })
    }

    /// Caller owns this worker guard and must drop it before boot teardown.
    pub fn start_runtime_permission_worker(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: Arc<Mutex<crate::package::owner::Store>>,
        inodes: std::collections::BTreeMap<u32, aim_storage::guest_inode::GuestInode>,
    ) -> Result<crate::package::owner::runtime_metadata::worker::Worker> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let metadata = self.runtime_permission_metadata_owner(&capture)?;
        metadata.lock().unwrap().bind_creation_metadata(&inodes).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let system = Arc::downgrade(self);
        let worker_bridge = bridge.clone();
        let bridge = bridge.clone();
        let worker =
            crate::package::owner::runtime_metadata::worker::Worker::start(&metadata, move |now| {
                let fail = |message: String| crate::package::owner::runtime_metadata::FlushError {
                    user: -1,
                    completed: Vec::new(),
                    error: crate::package::owner::WriteError {
                        committed: false,
                        message,
                    },
                };
                let system = system
                    .upgrade()
                    .ok_or_else(|| fail("runtime system owner stopped".into()))?;
                system
                    .check_package_bootstrap(&bridge)
                    .map_err(|error| fail(format!("runtime worker bootstrap: {error:?}")))?;
                let capture = system
                    .capture_package_queries()
                    .map_err(|error| fail(format!("runtime worker capture: {error:?}")))?;
                system.process_due_runtime_permission_requests(
                    &bridge,
                    &mut store.lock().unwrap(),
                    &capture,
                    &std::collections::BTreeMap::new(),
                    now,
                )
            })
            .map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
            })?;
        let registered = {
            let mut state = self.package_bootstrap.lock().unwrap();
            if let Some(current) = state
                .current
                .as_mut()
                .filter(|owner| Arc::ptr_eq(&owner.bridge, &worker_bridge))
            {
                current.runtime_worker = Some(worker.stop_handle());
                true
            } else {
                false
            }
        };
        if !registered {
            drop(worker);
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "runtime bootstrap changed while starting worker",
            ));
        }
        Ok(worker)
    }

    pub fn stop_runtime_permission_worker(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<()> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|owner| Arc::ptr_eq(&owner.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime worker bootstrap owner changed",
                )
            })?;
        if let Some(worker) = current.runtime_worker.take() {
            worker.stop();
        }
        Ok(())
    }

    /// Restore saved permission roles and metadata under one retained boot bridge.
    pub fn restore_package_runtime_permissions(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &crate::package::owner::Store,
        scan: &mut crate::package::scan::SigningScan,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<crate::package::owner::runtime_metadata::State> {
        self.check_package_bootstrap(bridge)?;
        let mut candidate = scan.clone();
        let metadata = store
            .restore_runtime_permission_owners(&mut candidate, config)
            .map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
            })?;
        self.check_package_bootstrap(bridge)?;
        *scan = candidate;
        Ok(metadata)
    }

    /// Use the retained original PackagePartitions build identity for controller metadata.
    pub fn set_runtime_permission_controller_version(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        metadata: &mut crate::package::owner::runtime_metadata::State,
        version: i64,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let build = bridge.current_package_version().map_err(|error| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("runtime build owner: {error:?}"),
            )
        })?;
        let fingerprint = build.fingerprint.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "missing runtime partition fingerprint",
            )
        })?;
        self.check_package_bootstrap(bridge)?;
        metadata.set_controller_version(&fingerprint, version);
        Ok(())
    }

    /// Persist live UID permission projections under the retained boot bridge.
    /// Metadata is supplied by the runtime persistence owner, not saved grants.
    pub fn commit_runtime_permissions_from_scan(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        user: u32,
        metadata: &crate::package::owner::runtime_metadata::State,
        inode: aim_storage::guest_inode::GuestInode,
    ) -> std::result::Result<(), crate::package::owner::WriteError> {
        use crate::package::owner::WriteError;
        let before = |message| WriteError {
            committed: false,
            message,
        };
        self.check_package_bootstrap(bridge)
            .map_err(|error| before(format!("runtime bootstrap owner: {error:?}")))?;
        store.validate_committed_scan(capture.scan().owner())?;
        let id = i32::try_from(user).map_err(|error| before(format!("runtime user: {error}")))?;
        let state = bridge
            .runtime_permissions(
                capture,
                id,
                metadata.version(id),
                metadata.fingerprint(id).map(str::to_owned),
            )
            .map_err(before)?;
        self.check_package_bootstrap(bridge)
            .map_err(|error| before(format!("runtime bootstrap owner before commit: {error:?}")))?;
        store.commit_runtime_permissions(user, &state, inode)?;
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: true,
                message: format!("runtime bootstrap owner after commit: {error:?}"),
            })
    }

    /// Drain requested runtime writes at a synchronous persistence boundary.
    /// Failed users remain queued, including a main commit with reserve failure.
    pub fn flush_runtime_permission_requests(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        metadata: &mut crate::package::owner::runtime_metadata::State,
        inodes: &std::collections::BTreeMap<u32, aim_storage::guest_inode::GuestInode>,
    ) -> std::result::Result<Vec<u32>, crate::package::owner::runtime_metadata::FlushError> {
        use crate::package::owner::WriteError;
        metadata.bind_creation_metadata(inodes).map_err(|message|
            crate::package::owner::runtime_metadata::FlushError {
                user: -1, completed: Vec::new(), error: WriteError { committed: false, message }
            })?;
        metadata.flush_with(|user, current| {
            let id = u32::try_from(user).map_err(|_| WriteError {
                committed: false,
                message: "negative runtime permission user".into(),
            })?;
            let inode = current.creation_inode(id).ok_or_else(|| WriteError {
                committed: false,
                message: "missing runtime permission creation owner".into(),
            })?;
            self.commit_runtime_permissions_from_scan(bridge, store, capture, id, current, inode)?;
            Ok(id)
        })
    }

    /// Initialize captured users under the retained boot owner and pinned policy.
    pub fn commit_initial_package_restrictions(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        store: &mut crate::package::owner::Store,
        scan: &crate::package::scan::SigningScan,
        user: u32,
        sections: aim_android_xml::Element,
    ) -> std::result::Result<(), crate::package::owner::WriteError> {
        use crate::package::owner::WriteError;
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: false,
                message: format!("initial user bootstrap owner: {error:?}"),
            })?;
        store.commit_initial_scan_restrictions(
            scan,
            user,
            crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS,
            sections,
        )?;
        self.check_package_bootstrap(bridge)
            .map_err(|error| WriteError {
                committed: true,
                message: format!("initial user bootstrap owner after commit: {error:?}"),
            })
    }

    /// Initialize constructor shared identities under the retained early bridge.
    /// Keep partial settings/UID/permission effects if a later owner fails.
    pub fn initialize_package_shared_users(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        config: &crate::package::system_config::SystemConfig,
        settings: &mut crate::package::settings::Settings,
        ids: &mut crate::package::owner::app_ids::AppIds,
        owners: &mut impl crate::package::settings::ReadOwners,
    ) -> Result<Vec<crate::package::owner::shared_users::Rejected>> {
        self.check_package_bootstrap(bridge)?;
        let boot = crate::package::owner::shared_users::Bootstrap::new(config);
        settings
            .initialize_shared_bootstrap(&boot, ids, owners)
            .map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string())
            })?;
        self.check_package_bootstrap(bridge)?;
        Ok(boot.rejected)
    }

    /// Recover through the native record dispatcher, sharing registration and
    /// per-attempt state across package, shared UID and keyset containers.
    pub fn recover_owned_package_settings(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        data: &std::path::Path,
        users: &[u32],
        settings: &mut crate::package::settings::Settings,
        ids: &mut crate::package::owner::app_ids::AppIds,
        attempt: &mut crate::package::settings::PackageReadAttempt,
        owners: &mut impl crate::package::settings::ReadOwners,
    ) -> std::result::Result<
        (
            crate::package::owner::Store,
            crate::package::owner::recovery::Report,
        ),
        crate::package::owner::recovery::Error,
    > {
        use crate::package::{
            owner::recovery::{Error, ReadError},
            settings::PackageReadAttempt,
        };
        self.check_package_bootstrap(bridge)
            .map_err(|error| Error {
                events: Vec::new(),
                message: format!("settings bootstrap owner: {error:?}"),
            })?;
        owners
            .start_attempt(settings, &attempt.pending)
            .map_err(|error| Error {
                events: Vec::new(),
                message: error.to_string(),
            })?;
        *attempt = PackageReadAttempt::default();
        let strict = bridge
            .domain_uuid_strict_validation()
            .map_err(|error| Error {
                events: Vec::new(),
                message: format!("settings domain UUID owner: {error:?}"),
            })?;
        let recovered =
            self.recover_package_settings(bridge, data, users, settings, |bytes, settings| {
                let read = settings.read_owned_document(bytes, ids, attempt, strict, owners);
                if matches!(read, Err(ReadError::File(_))) {
                    // failRead recursively starts a new attempt even if no file remains.
                    owners.start_attempt(settings, &attempt.pending)?;
                    *attempt = PackageReadAttempt::default();
                }
                read
            })?;
        Ok(recovered)
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
    include!("system_boot_configuration_tests.rs");

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

impl System {
    pub fn configure_native_package_image(&self, image: &std::path::Path, data: &std::path::Path,
        original_roots: &[std::path::PathBuf],
    ) -> Result<()> {
        let configuration = crate::package::boot_image::Configuration::pending(image, data, original_roots)
            .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        let mut current = self.package_boot_image_configuration.lock().unwrap();
        if let Some(previous) = current.as_ref() {
            if previous.image == configuration.image && previous.data == configuration.data && previous.original_roots == configuration.original_roots { return Ok(()); }
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native package image mapping already configured differently"));
        }
        *current = Some(configuration);
        Ok(())
    }
    fn boot_configuration_for(&self,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<Arc<BootConfiguration>> {
        let state=self.package_bootstrap.lock().unwrap();
        state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge))
            .map(|current|current.boot_configuration.clone()).ok_or_else(||Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,"native boot configuration epoch retired"))
    }
    pub fn configure_native_package_boot_policy(&self,policy:package_boot_inputs::Cli,
        resources:crate::package::parse::resources::Config)->Result<()> {
        let bridge=self.package_bootstrap()?;
        let state=self.package_bootstrap.lock().unwrap();
        let epoch=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&bridge)).ok_or_else(||Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"native boot policy epoch retired"))?;
        let mut values=epoch.boot_configuration.values.lock().unwrap();
        if values.policy.is_some(){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"native package boot policy already configured"));}
        values.policy=Some((policy,resources));Ok(())
    }
    pub fn configure_native_package_persistence(&self,inputs:crate::system_package_persistence_init::Inputs)->Result<()> {
        let bridge=self.package_bootstrap()?;self.check_boot_persistence_image(&inputs)?;
        let state=self.package_bootstrap.lock().unwrap();let epoch=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&bridge)).ok_or_else(||Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"native boot persistence epoch retired"))?;
        let mut values=epoch.boot_configuration.values.lock().unwrap();
        if values.persistence.is_some(){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"native package boot persistence already configured"));}
        values.persistence=Some(inputs);Ok(())
    }
    fn check_boot_persistence_image(&self,inputs:&crate::system_package_persistence_init::Inputs)->Result<()> {
        let image=self.native_package_image()?;
        if std::fs::canonicalize(&inputs.data).map_err(|error|Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,error.to_string()))?!=image.data {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"boot persistence uses another data owner"));
        }Ok(())
    }
    pub fn configure_native_package_runtime_facts(&self,facts:package_runtime::BootFacts)->Result<()> {
        if facts.density<=0{return Err(Exception::illegal_argument("native boot density must come from the actual image"));}
        let bridge=self.package_bootstrap()?;let state=self.package_bootstrap.lock().unwrap();
        let epoch=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&bridge)).ok_or_else(||Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"native boot facts epoch retired"))?;
        let mut values=epoch.boot_configuration.values.lock().unwrap();
        if values.facts.is_some(){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"native runtime facts already configured"));}
        values.facts=Some(facts);Ok(())
    }
    pub(crate) fn configure_native_package_early_for(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,
        early:Arc<crate::package::boot_configuration::Early>,policy:package_boot_inputs::Cli,
        resources:crate::package::parse::resources::Config,persistence:crate::system_package_persistence_init::Inputs)->Result<()> {
        self.check_boot_persistence_image(&persistence)?;
        let state=self.package_bootstrap.lock().unwrap();
        let epoch=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"early native boot configuration epoch retired"))?;
        let mut values=epoch.boot_configuration.values.lock().unwrap();
        if values.policy.is_some()||values.persistence.is_some()||values.early.is_some(){return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"early native package configuration already configured"));}
        values.policy=Some((policy,resources));values.persistence=Some(persistence);values.early=Some(early);Ok(())
    }
    pub fn configure_native_package_property_area(&self, path: &std::path::Path) -> Result<()> {
        let path = std::fs::canonicalize(path).map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
        if !path.is_dir() { return Err(Exception::illegal_argument("native property area is not a directory")); }
        let mut slot = self.package_property_area.lock().unwrap();
        if slot.as_ref().is_some_and(|previous| previous != &path) { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "native property area owner changed")); }
        *slot = Some(path); Ok(())
    }
    pub(crate) fn finish_native_package_runtime_configuration(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, effects: crate::package::boot_configuration::ScanEffects,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let early = self.boot_configuration_for(bridge)?.values.lock().unwrap().early.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual early native package configuration unavailable"))?;
        let capture = self.capture_package_queries()?;
        let late = early.after_scan(capture.state().clone(), effects).map_err(|message|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        self.complete_native_package_boot_facts(bridge, late)
    }
    pub(crate) fn complete_native_package_boot_facts(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>, late: crate::package::boot_configuration::Late,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let controller = capture.state().system.permission_controller_package.as_ref()
            .and_then(|name| name.as_ref()).and_then(|name| capture.state().packages.get(name))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "accepted permission controller unavailable"))?;
        if controller.version_code != late.persistence.controller_version { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "runtime controller version differs from accepted scan")); }
        let state=self.package_bootstrap.lock().unwrap();
        let epoch=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,"late boot configuration epoch retired"))?;
        let mut inputs=epoch.boot_configuration.values.lock().unwrap();
        let current = inputs.persistence.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
            "early persistence configuration unavailable"))?;
        if current.data != late.persistence.data || current.users != late.persistence.users
            || current.original_roots != late.persistence.original_roots {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "late boot persistence changes its original data owner"));
        }
        if current.controller_version != crate::package::boot_configuration::CONTROLLER_VERSION_DEFERRED {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "late boot facts already applied"));
        }
        if late.facts.density<=0{return Err(Exception::illegal_argument("native boot density must come from the actual image"));}
        if inputs.facts.is_some(){return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"late boot facts already applied"));}
        inputs.facts=Some(late.facts);
        inputs.persistence=Some(late.persistence);
        Ok(())
    }

    pub(crate) fn finish_native_boot_session_construction(self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>, scanned: package_boot_scan::Scanned,
        early: &Arc<crate::package::early_user_operations::Endpoint>, user_operations: Binder, factory_test: bool,
    ) -> Result<package_runtime::Runtime> {
        let image = self.native_package_image()?;
        let (policy, resources) = self.boot_configuration_for(bridge)?.values.lock().unwrap().policy.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "native boot scan policy unavailable"))?;
        if policy.factory_test != factory_test { return Err(Exception::illegal_argument("native boot factory-test epoch differs")); }
        let facts = self.boot_configuration_for(bridge)?.values.lock().unwrap().facts.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual native runtime facts unavailable"))?;
        let persistence_inputs = self.boot_configuration_for(bridge)?.values.lock().unwrap().persistence.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "native boot persistence inputs unavailable"))?;
        let properties = (facts.properties)()?;
        let system_config = crate::package::system_config::SystemConfig::read(&image.image, &|name| properties.get(name).cloned());
        let apks = image.apks(&system_config, self.package_signing_overrides()?, facts.density,
            &|name| Ok(properties.get(name).cloned())).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let kernel = Arc::new(crate::package::kernel_mappings::Owner::inspect(&image.host_path("/config/sdcardfs")
            .ok_or_else(|| Exception::illegal_argument("native kernel mapping path unavailable"))?)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?);
        let claims = image.data.join("system/native-package-moves");
        std::fs::create_dir_all(&claims).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))?;
        self.initialize_native_package_runtime(bridge, package_runtime::Inputs {
            image: image.image.clone(), data: image.data.clone(), apks, resource_config: resources,
            system_config, prepared: scanned.prepared, persistence_inputs, runtime_metadata: scanned.runtime_metadata,
            properties: facts.properties, dependency_installer_enabled: facts.dependency_installer_enabled,
            factory_test: policy.factory_test, parser_cache: policy.parser_cache,
            apex_results: scanned.apex_results, registrations: scanned.registrations,
            density: facts.density, storage_manager_package: facts.storage_manager_package,
            kernel, claims, fix_system_apps_first_install_time: facts.fix_system_apps_first_install_time,
            early_users: early.clone(), early_user_binder: user_operations,
            decompression: scanned.decompression, boot_apex_changed: facts.boot_apex_changed,
        })
    }

    pub(crate) fn begin_package_manager_boot(self: &Arc<Self>, factory_test: bool) -> Result<Binder> {
        if !self.package_requested.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "image does not request native PackageManager"));
        }
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        let image = self.native_package_image()?;
        let bridge = self.package_bootstrap()?;
        {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "boot session epoch changed"))?;
            if current.boot_session_starting || current.boot_session.as_ref().is_some_and(|session| session.upgrade().is_some()) {
                return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native package boot session already started"));
            }
            current.boot_session_starting = true;
        }
        struct BeginGuard<'a> { system: &'a System, bridge: Arc<crate::package::bootstrap::Bridge> }
        impl Drop for BeginGuard<'_> {
            fn drop(&mut self) {
                if let Some(current) = self.system.package_bootstrap.lock().unwrap().current.as_mut()
                    .filter(|current| Arc::ptr_eq(&current.bridge, &self.bridge)) { current.boot_session_starting = false; }
            }
        }
        let _begin = BeginGuard { system: self, bridge: bridge.clone() };
        let leaf = self.package_bootstrap_binder_leaf(&bridge, api::GET_PACKAGE_INITIAL_CONTEXT_LEAF)?;
        let initial = package_boot_scan::InitialOwner::new(leaf);
        let config = crate::package::system_config::SystemConfig::read(&image.image, &|name| initial.property_value(name));
        initial.check()?;
        // The original UM creates the first user roster. Settings recovery
        // therefore belongs to finishRawScan, after its constructor returns.
        let early = crate::package::early_user_operations::Endpoint::new(&config);
        let user_operations = self.process.add_service(early.clone());
        let internal = self.package_internal_host()?;
        let session = crate::package::boot_session::Session::new(self, bridge.clone(), early, user_operations, internal, factory_test);
        let mut state = self.package_bootstrap.lock().unwrap();
        let Some(current) = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge)) else {
            drop(state);
            let mut error = Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "boot session epoch retired during begin");
            if let Err(cleanup) = session.close() { error.message.push_str(&format!("; session cleanup: {}", cleanup.message)); }
            return Err(error);
        };
        current.boot_session = Some(Arc::downgrade(&session));
        drop(state);
        Ok(self.process.add_service(session))
    }
    pub(crate) fn prepare_native_boot_session_scan(self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>, factory_test: bool,
        early: &Arc<crate::package::early_user_operations::Endpoint>,
    ) -> Result<package_boot_scan::Begun> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        self.check_package_bootstrap(bridge)?;
        let image = self.native_package_image()?;
        let leaf = self.package_bootstrap_binder_leaf(bridge, api::GET_PACKAGE_INITIAL_CONTEXT_LEAF)?;
        let initial = package_boot_scan::InitialOwner::new(leaf);
        let area = self.package_property_area.lock().unwrap().clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual native property area unavailable"))?;
        let properties = initial.runtime_properties(area);
        let configuration_leaf = self.package_bootstrap_binder_leaf_with(bridge, api::GET_PACKAGE_BOOT_CONFIGURATION_LEAF, |parcel| parcel.write_bool(factory_test))?;
        let weak = Arc::downgrade(self);
        let epoch = bridge.clone();
        let source = crate::package::boot_configuration::Source::capture(Arc::new(configuration_leaf), &image.data,
            Arc::new(move || weak.upgrade().ok_or_else(|| "native package System stopped".to_owned())?
                .check_package_bootstrap(&epoch).map_err(|error| error.message)))
            .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        crate::package::boot_configuration::Early::from_native_entry(self, bridge, &source, properties)
            .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        let (policy, _) = self.boot_configuration_for(bridge)?.values.lock().unwrap().policy.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual native boot policy unavailable"))?;
        if policy.factory_test != factory_test { return Err(Exception::illegal_argument("factory-test mode differs from original invocation")); }
        let inputs = self.boot_configuration_for(bridge)?.values.lock().unwrap().persistence.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual native persistence inputs unavailable"))?;
        let config = crate::package::system_config::SystemConfig::read(&image.image, &|name| initial.property_value(name));
        initial.check()?;
        let recovered = crate::system_package_persistence_init::construct_package_persistence_owned(self, bridge, &inputs, &config)?;
        let ce = self.package_bootstrap_binder_leaf(bridge, api::GET_PACKAGE_LIFECYCLE_LEAF)?;
        let mut begun = self.begin_native_package_scan(bridge, recovered.prepared, &image.image, &image.data, initial, ce)?;
        begun.retained_readers = Some(recovered.readers);
        early.bind_lifecycle(begun.lifecycle.owner.clone())?;
        Ok(begun)
    }
    pub(crate) fn finish_native_boot_session_raw_scan(self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>, mut begun: package_boot_scan::Begun,
        _early: &Arc<crate::package::early_user_operations::Endpoint>,
    ) -> Result<package_boot_scan::RawScanned> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        let image = self.native_package_image()?;
        if begun.image != image.image || begun.data != image.data { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "initial scan uses another configured image owner")); }
        let context = self.package_bootstrap_binder_leaf(bridge, api::GET_PACKAGE_BOOT_CONTEXT_LEAF)?;
        begun.bind_original_context(context)?;
        let early_configuration = self.boot_configuration_for(bridge)?.values.lock().unwrap().early.clone()
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual early display configuration absent"))?;
        let apks = image.apks(&begun.system_config, self.package_signing_overrides()?, early_configuration.density_dpi(),
            &|name| begun.initial.property(name).map_err(|error| error.message)).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let (policy, resources) = self.boot_configuration_for(bridge)?.values.lock().unwrap().policy.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "actual native package boot policy unavailable"))?;
        self.finish_raw_from_image(bridge, begun, &apks, policy, resources)
    }
    pub(crate) fn finish_native_boot_session_initial_scan(self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>, raw: Arc<package_boot_scan::RawScanned>,
        _early: &Arc<crate::package::early_user_operations::Endpoint>,
    ) -> Result<package_boot_scan::Scanned> {
        let scanned = self.finish_native_package_capture(bridge, &raw)?;
        self.finish_native_package_runtime_configuration(bridge, crate::package::boot_configuration::ScanEffects {
            boot_apex_changed: scanned.boot_apex_changed,
        })?;
        Ok(scanned)
    }

    pub(crate) fn native_package_image(&self) -> Result<Arc<crate::package::boot_image::Owner>> {
        let mut current = self.package_boot_image.lock().unwrap();
        if let Some(owner) = current.as_ref() { return Ok(owner.clone()); }
        let configuration = self.package_boot_image_configuration.lock().unwrap();
        let configuration = configuration.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "native package image mapping is not configured"))?;
        let owner = configuration.open().map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        *current = Some(owner.clone());
        Ok(owner)
    }
    pub(crate) fn request_native_package_manager(&self) {
        self.package_requested.store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn package_make_uid_visible(&self, uid: i32,
        request: &crate::package::visibility_mutation::MakeUidVisible,
        resolver: &crate::package::resolve::Resolver)
        -> std::result::Result<std::result::Result<(), Exception>, crate::package::resolve::QueryError> {
        use crate::package::{apps_filter::NotModelled, resolve::QueryError, query::Query};
        let bridge = self.package_bootstrap().map_err(|_| QueryError::NotModelled(NotModelled("visibility bootstrap unavailable")))?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
            .ok_or(QueryError::NotModelled(NotModelled("visibility bootstrap changed")))?;
        let capture = current.queries.clone().ok_or(QueryError::NotModelled(NotModelled("visibility capture unavailable")))?;
        let resolution = match resolver.resolution(capture.state()) {
            Ok(value) => value,
            Err(error) => {
                let reply = error.reply().map_err(QueryError::Transport)?;
                return Ok(aim_binder_host::parcel::Reader::new(reply.data(), reply.objects())
                    .read_exception().map_err(QueryError::Transport)?);
            }
        };
        let query = Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid };
        let grants = match request.decide(&query).map_err(QueryError::NotModelled)? {
            Err(error) => return Ok(Err(error)),
            Ok(None) => return Ok(Ok(())),
            Ok(Some(value)) => value,
        };
        let update = match capture.prepare_visibility_update(grants) {
            Ok(value) => value,
            Err(error) => return Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))),
        };
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        bridge.invalidate_packages_for_uid_cache().map_err(|_error| QueryError::NotModelled(NotModelled("packages-for-UID cache invalidation failed")))?;
        Ok(Ok(()))
    }
}

impl System {
    /// Permission service callbacks invoke this after changing their actual
    /// state; a stale package generation cannot overwrite concurrent mutations.
    pub(crate) fn refresh_package_permission_queries(&self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        base: &Arc<crate::package::scan_snapshot::query_state::Capture>) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let update = base.prepare_permission_refresh(bridge)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"permission refresh bootstrap changed"))?;
        if !current.queries.as_ref().is_some_and(|current| Arc::ptr_eq(current, base)) {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"permission refresh generation changed"));
        }
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }
}

#[path="system_package_shell.rs"]
mod package_shell;
#[path="system_package_shell_read.rs"]
mod package_shell_read;

#[path="system_package_shell_install.rs"]
mod package_shell_install;
