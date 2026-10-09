//! Concrete policy producers for the single recovered native installer owner.
//! Original policy flags and DPM/UM/permission leaves retain their actual owners.
use crate::{system::System, package::{bootstrap::Bridge, installer::{self,
    native::{NativeOwners, PolicySource}, policy::{CallingPermissions, DevicePolicy}}, write::Apks}};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, EX_ILLEGAL_STATE}};
use aim_service_aidl::{dev_aim_server_iinstallerpolicybridge as api,
    dev_aim_server_ipackagemaintenancebridge as maintenance};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

pub type Properties = Arc<dyn Fn() -> Result<BTreeMap<String, String>, Exception> + Send + Sync>;
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }

struct DeviceOwner {
    system: std::sync::Weak<System>,
    bridge: Arc<Bridge>,
    leaf: Strong,
    permissions: CallingPermissions,
}
impl DeviceOwner {
    fn source(self: &Arc<Self>) -> PolicySource {
        let owner = self.clone();
        Arc::new(move |uid, pid, requested_user| owner.capture(uid, pid, requested_user))
    }
    fn capture(&self, uid: u32, pid: i32, requested_user: i32) -> Result<DevicePolicy, Exception> {
        let system = self.system.upgrade().ok_or_else(|| illegal("installer policy system stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;
        let uid_i32 = i32::try_from(uid).map_err(|_| Exception::illegal_argument("installer UID exceeds Android range"))?;
        let capture = system.capture_package_queries()?;
        let mut request = Parcel::new(); request.write_interface_token(api::DESCRIPTOR);
        let reply = self.leaf.transact(api::GET_POLICY_FLAGS, &request, false)
            .map_err(|status| illegal(format!("installer flag owner transport: {status}")))?;
        let mut reader = reply.reader();
        reader.read_exception().map_err(|status| illegal(format!("installer flag owner reply: {status}")))??;
        let flags = reader.read_i32().map_err(|status| illegal(format!("installer flag owner payload: {status}")))?;
        if flags & !7 != 0 || reader.remaining() != 0 { return Err(illegal("installer flag owner malformed payload")); }
        let mut ids = capture.state().users.keys().copied().collect::<BTreeSet<_>>();
        ids.insert(requested_user);
        let mut users = BTreeMap::new();
        for user in ids {
            if user < 0 { return Err(Exception::illegal_argument("negative installer user ID")); }
            if let Some(policy) = self.bridge.installer_user_policy(user)
                .map_err(|error| illegal(format!("installer user policy owner: {error:?}")))? {
                users.insert(user, policy);
            }
        }
        // PackageManagerServiceUtils.isAdoptedShell checks this permission for
        // the original calling identity, excluding SYSTEM_UID. System asks its
        // actual original permission owner for precisely that caller's UID.
        let mut adopted_shell_uids = BTreeSet::new();
        if uid != 1000 && self.permissions.check("android.permission.USE_SYSTEM_DATA_LOADERS", pid, uid_i32)? {
            adopted_shell_uids.insert(uid);
        }
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state())
            .map_err(|error| illegal(format!("installer verifier resolution: {error:?}")))?;
        let query = crate::package::query::Query {
            state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid_i32,
        };
        let roles = capture.state().system.roles.as_ref()
            .ok_or_else(|| illegal("installer verifier KnownPackages unavailable"))?;
        let verifiers = roles.known_packages(&query, 4, uid_i32 / 100_000)
            .map_err(installer::policy::unknown)?;
        let mut verifier_uid = None;
        for name in verifiers.into_iter().flatten() {
            if query.package_uid(&name, 0, uid_i32 / 100_000)
                .map_err(installer::policy::unknown)?? == uid_i32 { verifier_uid = Some(uid); break; }
        }
        system.check_package_bootstrap(&self.bridge)?;
        // Original createSessionInternal retains its Computer snapshot while
        // querying live UM/DPM/permission owners. This is a read, not a package
        // publication: another writer may publish without invalidating it.
        let permissions = self.permissions.clone();
        Ok(DevicePolicy { permissions, debuggable: flags & 1 != 0, apex_supported: flags & 2 != 0,
            rollback_lifetime: flags & 4 != 0, users, adopted_shell_uids, verifier_uid })
    }
}

pub fn policy_source(system: &Arc<System>, bridge: &Arc<Bridge>, leaf: Strong)
    -> Result<PolicySource, Exception> {
    system.check_package_bootstrap(bridge)?;
    let permissions = permission_source(system, bridge)?;
    Ok(Arc::new(DeviceOwner { system: Arc::downgrade(system), bridge: bridge.clone(), leaf, permissions }).source())
}

/// Retains the original Context authority installed before finishConstruction.
/// Its own framework permission semantics also apply before AMS publication.
pub fn permission_source(system: &Arc<System>, bridge: &Arc<Bridge>) -> Result<CallingPermissions, Exception> {
    system.check_package_bootstrap(bridge)?;
    let leaf = bridge.package_maintenance().map_err(|error| match error {
        crate::package::bootstrap::OwnerError::Owner(error) => error,
        error => illegal(format!("installer permission bridge owner: {error:?}")),
    })?;
    let reply = leaf.transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
        .map_err(|status| illegal(format!("installer permission descriptor transport: {status}")))?;
    let mut reader = reply.reader();
    if reader.read_string16().map_err(|status| illegal(format!("installer permission descriptor reply: {status}")))?.as_deref()
        != Some(maintenance::DESCRIPTOR) || reader.remaining() != 0 {
        return Err(illegal("installer permission bridge descriptor differs"));
    }
    system.check_package_bootstrap(bridge)?;
    let weak = Arc::downgrade(system);
    let retained = bridge.clone();
    Ok(CallingPermissions::new(Arc::new(move |name, pid, uid| {
        let system = weak.upgrade().ok_or_else(|| illegal("installer permission owner stopped"))?;
        system.check_package_bootstrap(&retained)?;
        let mut request = Parcel::new();
        maintenance::CheckPermission { permission: Some(name.into()), calling_uid: uid, calling_pid: pid }.write(&mut request);
        let reply = leaf.transact(maintenance::CHECK_PERMISSION, &request, false)
            .map_err(|status| illegal(format!("installer permission owner transport: {status}")))?;
        let mut reader = reply.reader();
        let granted = maintenance::read_check_permission_reply(&mut reader)
            .map_err(|status| illegal(format!("installer permission owner reply: {status}")))??;
        if reader.remaining() != 0 { return Err(illegal("installer permission owner reply tail")); }
        system.check_package_bootstrap(&retained)?;
        Ok(granted)
    })))
}

/// Persistence.prepare/restore/install supplies the sole recovered NativeOwners.
/// The root installs these policies once before publishing its installer facade.
pub fn configure_existing(system: &Arc<System>, bridge: &Arc<Bridge>,
    owner: &Arc<NativeOwners>, apks: Arc<Apks>, properties: Properties,
    dependency_installer_enabled: bool) -> Result<(), Exception> {
    system.check_package_bootstrap(bridge)?;
    let sdk = apks.platform.sdk;
    let codenames = apks.platform.codenames.clone();
    let weak = Arc::downgrade(system); let retained = bridge.clone();
    owner.configure_lite_policy(Arc::new(move || {
        let system = weak.upgrade().ok_or_else(|| illegal("installer lite system stopped"))?;
        system.check_package_bootstrap(&retained)?;
        let properties = properties()?;
        let art_v3 = retained.installer_art_service_v3_enabled()
            .map_err(|error| illegal(format!("installer ART filter owner: {error:?}")))?;
        system.check_package_bootstrap(&retained)?;
        Ok(installer::native::LitePolicy {
            environment: crate::package::parse::lite::Environment { sdk, codenames: codenames.clone(), properties },
            art_managed_extensions: if art_v3 { vec![".dm".into(), ".prof".into(), ".sdm".into()] }
                else { vec![".dm".into()] },
        })
    }))?;
    owner.configure_commit(system.package_installer_commit_policy(bridge)?)?;
    system.configure_native_installer_confirmation(bridge, owner, apks,
        dependency_installer_enabled)?;
    system.check_package_bootstrap(bridge)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
    use aim_binder_host::{local::{Call, LocalProcess, Reply, Service}, parcel::Binder};
    use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
    use std::sync::{Mutex, atomic::{AtomicBool, AtomicU8, Ordering}};

    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> { Err(errno::EFAULT) }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> { Err(errno::EFAULT) }
        fn get_file(&mut self, _: u32) -> Result<File, Errno> { Err(errno::EBADF) }
        fn install_file(&mut self, _: File) -> Result<u32, Errno> { Err(errno::EBADF) }
        fn close_fd(&mut self, _: u32) { panic!("permission fixture exchanges no files") }
    }
    // Test external authority; production invokes unchanged original Context.
    struct Authority {
        granted: AtomicBool,
        mode: AtomicU8,
        foreign: AtomicBool,
        calls: Mutex<Vec<(String, i32, i32)>>,
        retire: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }
    impl Service for Authority {
        fn descriptor(&self) -> &str {
            if self.foreign.load(Ordering::SeqCst) { "fixture.foreign.permission" } else { maintenance::DESCRIPTOR }
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            assert_eq!(call.code, maintenance::CHECK_PERMISSION);
            let args = maintenance::CheckPermission::read(&mut call.data)?;
            assert_eq!(call.data.remaining(), 0);
            self.calls.lock().unwrap().push((args.permission.unwrap(), args.calling_pid, args.calling_uid));
            if let Some(retire) = self.retire.lock().unwrap().take() { retire(); }
            let mut reply = Parcel::new();
            if self.mode.load(Ordering::SeqCst) == 1 {
                reply.write_exception(&Exception::security("original context denied"));
            } else {
                maintenance::write_check_permission_reply(&mut reply, self.granted.load(Ordering::SeqCst));
                if self.mode.load(Ordering::SeqCst) == 2 { reply.write_i32(9); }
            }
            Ok(reply)
        }
    }
    struct OriginalBridge { maintenance: Binder }
    impl Service for OriginalBridge {
        fn descriptor(&self) -> &str { bootstrap::DESCRIPTOR }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            call.data.enforce_interface(bootstrap::DESCRIPTOR)?;
            assert_eq!(call.data.remaining(), 0);
            let mut reply = Parcel::new();
            match call.code {
                bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH => bootstrap::write_is_test_base_on_bootclasspath_reply(&mut reply, true),
                bootstrap::IS_SIGNING_DEBUGGABLE => bootstrap::write_is_signing_debuggable_reply(&mut reply, false),
                bootstrap::GET_PACKAGE_MAINTENANCE_BRIDGE => bootstrap::write_get_package_maintenance_bridge_reply(&mut reply, Some(self.maintenance)),
                _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
            }
            Ok(reply)
        }
    }
    struct Fixture {
        driver: Arc<Driver>, native: Arc<LocalProcess>, original: Arc<LocalProcess>,
        system: Arc<System>, bridge: Arc<Bridge>, authority: Arc<Authority>,
    }
    impl Fixture {
        fn new() -> Self {
            let driver = Driver::new();
            let native = LocalProcess::open(&driver, Device::Binder, Credentials { pid: 99601, euid: 1000, security_context: None });
            let original = LocalProcess::open(&driver, Device::Binder, Credentials { pid: 99602, euid: 1000, security_context: None });
            let authority = Arc::new(Authority { granted: AtomicBool::new(true), mode: AtomicU8::new(0), foreign: AtomicBool::new(false), calls: Mutex::new(Vec::new()), retire: Mutex::new(None) });
            let node = original.add_service(authority.clone());
            let Binder::Local(pointer) = original.add_service(Arc::new(OriginalBridge { maintenance: node })) else { panic!("local owner required") };
            let mut object = FlatBinderObject { kind: BINDER_TYPE_BINDER, flags: 0, binder: pointer, cookie: pointer }.encode();
            driver.ioctl(original.proc_handle(), 99603, BINDER_SET_CONTEXT_MGR_EXT, &mut object, &mut NoMemory).unwrap();
            original.start(); native.start();
            let system = System::new(native.clone(), &[]);
            system.attach_package_bootstrap(0).unwrap();
            let bridge = system.package_bootstrap().unwrap();
            Self { driver, native, original, system, bridge, authority }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.driver.release(self.native.proc_handle());
            self.driver.release(self.original.proc_handle());
        }
    }
    #[test]
    fn maintenance_permission_transport_works_without_activity_and_reads_current_owner() {
        let fixture = Fixture::new();
        let reader = permission_source(&fixture.system, &fixture.bridge).unwrap();
        // No ActivityManager or service-manager lookup exists in this fixture.
        for (pid, uid) in [(77, 0), (78, 1000), (79, 10100)] {
            assert!(reader.check("android.permission.INSTALL_PACKAGES", pid, uid).unwrap());
        }
        fixture.authority.granted.store(false, Ordering::SeqCst);
        assert!(!reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).unwrap());
        fixture.authority.granted.store(true, Ordering::SeqCst);
        assert!(reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).unwrap());
        assert_eq!(*fixture.authority.calls.lock().unwrap(), [("android.permission.INSTALL_PACKAGES".into(), 77, 0),
            ("android.permission.INSTALL_PACKAGES".into(), 78, 1000), ("android.permission.INSTALL_PACKAGES".into(), 79, 10100),
            ("android.permission.INSTALL_PACKAGES".into(), 79, 10100), ("android.permission.INSTALL_PACKAGES".into(), 79, 10100)]);
    }
    #[test]
    fn maintenance_permission_transport_rejects_foreign_descriptor_errors_and_tail() {
        let fixture = Fixture::new();
        fixture.authority.foreign.store(true, Ordering::SeqCst);
        assert!(permission_source(&fixture.system, &fixture.bridge).is_err());
        assert!(fixture.authority.calls.lock().unwrap().is_empty());
        fixture.authority.foreign.store(false, Ordering::SeqCst);
        let reader = permission_source(&fixture.system, &fixture.bridge).unwrap();
        fixture.authority.mode.store(1, Ordering::SeqCst);
        let error = reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).unwrap_err();
        assert_eq!((error.code, error.message.as_str()), (aim_binder_host::parcel::EX_SECURITY, "original context denied"));
        fixture.authority.mode.store(2, Ordering::SeqCst);
        assert_eq!(reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).unwrap_err().message, "installer permission owner reply tail");
    }
    #[test]
    fn maintenance_permission_transport_rejects_retired_epoch_before_and_after_rpc() {
        let fixture = Fixture::new();
        let reader = permission_source(&fixture.system, &fixture.bridge).unwrap();
        let weak = Arc::downgrade(&fixture.system);
        let bridge = fixture.bridge.clone();
        *fixture.authority.retire.lock().unwrap() = Some(Box::new(move || {
            weak.upgrade().unwrap().detach_package_bootstrap(&bridge).unwrap();
        }));
        assert!(reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).is_err());
        assert_eq!(fixture.authority.calls.lock().unwrap().len(), 1);
        assert!(reader.check("android.permission.INSTALL_PACKAGES", 79, 10100).is_err());
        assert_eq!(fixture.authority.calls.lock().unwrap().len(), 1);
    }
}
