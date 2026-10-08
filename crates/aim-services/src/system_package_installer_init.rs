//! Concrete policy producers for the single recovered native installer owner.
//! Original policy flags and DPM/UM/permission leaves retain their actual owners.
use crate::{system::System, package::{bootstrap::Bridge, installer::{self,
    native::{NativeOwners, PolicySource}, policy::DevicePolicy}, write::Apks}};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_iinstallerpolicybridge as api;
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

pub type Properties = Arc<dyn Fn() -> Result<BTreeMap<String, String>, Exception> + Send + Sync>;
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }

struct DeviceOwner {
    system: std::sync::Weak<System>,
    bridge: Arc<Bridge>,
    leaf: Strong,
}
impl DeviceOwner {
    fn source(self: &Arc<Self>) -> PolicySource {
        let owner = self.clone();
        Arc::new(move |uid, requested_user| owner.capture(uid, requested_user))
    }
    fn capture(&self, uid: u32, requested_user: i32) -> Result<DevicePolicy, Exception> {
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
        if uid != 1000 && system.check_permission("android.permission.USE_SYSTEM_DATA_LOADERS", -1, uid_i32)? {
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
        Ok(DevicePolicy { debuggable: flags & 1 != 0, apex_supported: flags & 2 != 0,
            rollback_lifetime: flags & 4 != 0, users, adopted_shell_uids, verifier_uid })
    }
}

pub fn policy_source(system: &Arc<System>, bridge: &Arc<Bridge>, leaf: Strong)
    -> Result<PolicySource, Exception> {
    system.check_package_bootstrap(bridge)?;
    Ok(Arc::new(DeviceOwner { system: Arc::downgrade(system), bridge: bridge.clone(), leaf }).source())
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
