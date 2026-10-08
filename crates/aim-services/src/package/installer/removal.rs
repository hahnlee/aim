//! Native uninstall admission and durable state transitions. Ported from
//! android-16.0.0_r1 PackageInstallerService/DeletePackageHelper (AOSP, Apache-2.0).
#[path = "removal_production.rs"]
mod production;
use super::preapproval::IntentSender;
use super::{codec::Object, policy};
use crate::package::{
    apps_filter,
    model::State,
    owner,
    query::Query,
    restrictions::UserState,
    scan_snapshot::{self, Snapshot},
};
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, Exception};
use aim_binder_host::{
    local::{LocalProcess, Strong},
    parcel::{BAD_VALUE, Binder, Parcel, Reader},
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, dev_aim_server_iinstallerremovalbridge as external_api,
};
pub use production::{
    Factory, Inputs as ProductionInputs, Roles as ProductionRoles, build as build_production,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct VersionedPackage {
    pub name: String,
    pub version: i64,
}
impl ReadParcelable for VersionedPackage {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self, i32> {
        Ok(Self {
            name: reader.read_string16()?.ok_or(BAD_VALUE)?,
            version: reader.read_i64()?,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct UserHandle(pub i32);
impl ReadParcelable for UserHandle {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self, i32> {
        Ok(Self(reader.read_i32()?))
    }
}
impl WriteParcelable for UserHandle {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_i32(self.0);
    }
}

pub struct External {
    owner: Strong,
    process: Arc<LocalProcess>,
    revoked: std::sync::atomic::AtomicBool,
}
impl External {
    pub fn delete_user_action(
        &self,
        package: &str,
        flags: i32,
        observer: Binder,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_DELETE_USER_ACTION,
            |p| {
                p.write_string16(Some(package));
                p.write_i32(flags);
                p.write_binder(Some(observer));
            },
            |_| Ok(()),
        )
    }
    pub fn retain_sender(&self, sender: IntentSender) -> Result<Arc<Strong>, Exception> {
        match sender.target {
            Binder::Handle(handle) => Ok(Arc::new(self.process.strong(handle))),
            _ => Err(illegal(
                "IntentSender requires a received remote capability",
            )),
        }
    }
    pub fn new(owner: Strong, process: Arc<LocalProcess>) -> Arc<Self> {
        Arc::new(Self {
            owner,
            process,
            revoked: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn revoke(&self) {
        self.revoked
            .store(true, std::sync::atomic::Ordering::Release);
    }
    fn method_name(code: u32) -> &'static str {
        match code {
            external_api::PREPARE_PERMISSIONS => "preparePermissions",
            external_api::DESTROY_APP_DATA => "destroyAppData",
            external_api::DESTROY_PROFILES => "destroyProfiles",
            external_api::REMOVE_CODE => "removeCode",
            external_api::CLEAR_ARCHIVE_CACHES => "clearArchiveCaches",
            external_api::SEND_REMOVED_BROADCAST => "sendRemovedBroadcast",
            external_api::SEND_UNINSTALL_STATUS => "sendUninstallStatus",
            external_api::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED => "isSdkLibraryIndependenceEnabled",
            _ => "policy/archive/status",
        }
    }
    fn call<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>,
    ) -> Result<T, Exception> {
        self.call_with_reply(code, write, |reader, _| read(reader))
    }
    fn call_with_reply<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>, &aim_binder_host::local::Received) -> Result<T, i32>,
    ) -> Result<T, Exception> {
        let stage = format!("{} transaction {code}", Self::method_name(code));
        if self.revoked.load(std::sync::atomic::Ordering::Acquire) {
            return Err(illegal(format!("Installer removal {stage} owner revoked")));
        }
        let mut request = Parcel::new();
        request.write_interface_token(external_api::DESCRIPTOR);
        write(&mut request);
        let reply = self
            .owner
            .transact(code, &request, false)
            .map_err(|code| illegal(format!("Removal {stage} owner transport: {code}")))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|code| illegal(format!("Removal {stage} exception transport: {code}")))?
            .map_err(|mut error| {
                error.message = format!("Removal {stage} original exception {}: {}", error.code, error.message);
                error
            })?;
        let value =
            read(&mut reader, &reply).map_err(|code| illegal(format!("Removal {stage} reply decode: {code}")))?;
        if reader.remaining() != 0 || self.revoked.load(std::sync::atomic::Ordering::Acquire) {
            return Err(illegal(format!("Removal {stage} owner changed or sent {} trailing reply bytes", reader.remaining())));
        }
        Ok(value)
    }
    pub fn permission(&self, name: &str, pid: i32, uid: u32) -> Result<bool, Exception> {
        self.call(
            external_api::CHECK_PERMISSION,
            |p| {
                p.write_string16(Some(name));
                p.write_i32(pid);
                p.write_i32(uid as i32);
            },
            |r| Ok(r.read_i32()? == 0),
        )
    }
    pub fn check_package(&self, uid: u32, name: Option<&str>) -> Result<(), Exception> {
        self.call(
            external_api::CHECK_PACKAGE,
            |p| {
                p.write_i32(uid as i32);
                p.write_string16(name);
            },
            |_| Ok(()),
        )
    }
    pub fn silent(&self, uid: u32, name: Option<&str>) -> Result<bool, Exception> {
        self.call(
            external_api::CAN_SILENTLY_INSTALL,
            |p| {
                p.write_string16(name);
                p.write_i32(uid as i32);
            },
            |r| r.read_bool(),
        )
    }
    pub fn active_admin(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(
            external_api::HAS_ACTIVE_ADMIN,
            |parcel| {
                parcel.write_string16(Some(name));
                parcel.write_i32(user);
            },
            |reader| reader.read_bool(),
        )
    }
    pub fn pinned(&self, name: &str) -> Result<bool, Exception> {
        self.call(
            external_api::IS_PINNED,
            |p| p.write_string16(Some(name)),
            |r| r.read_bool(),
        )
    }
    pub fn sdk_library_independence(&self) -> Result<bool, Exception> {
        self.call(external_api::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED, |_| {}, |reader| reader.read_bool())
    }
    pub fn users(&self) -> Result<Vec<i32>, Exception> {
        self.call(
            external_api::GET_USERS,
            |_| {},
            |r| aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE),
        )
    }
    pub fn children(&self, user: i32) -> Result<Vec<i32>, Exception> {
        self.call(
            external_api::GET_CHILDREN_DELETED_WITH_PARENT,
            |p| p.write_i32(user),
            |r| aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE),
        )
    }
    pub fn restricted(&self, user: i32) -> Result<bool, Exception> {
        self.call(
            external_api::IS_UNINSTALL_RESTRICTED,
            |p| p.write_i32(user),
            |r| r.read_bool(),
        )
    }
    pub fn opted_out(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        self.call(
            external_api::IS_ARCHIVE_OPTED_OUT,
            |p| {
                p.write_string16(Some(package));
                p.write_i32(uid);
            },
            |r| r.read_bool(),
        )
    }
    pub fn collect_archive(
        &self,
        name: &str,
        installer: &str,
        user: i32,
    ) -> Result<crate::package::restrictions::ArchiveState, Exception> {
        self.call(
            external_api::COLLECT_ARCHIVE,
            |p| {
                p.write_string16(Some(name));
                p.write_string16(Some(installer));
                p.write_i32(user);
            },
            |r| {
                if r.read_i32()? == 0 {
                    return Err(BAD_VALUE);
                }
                super::archiver::read_metadata(r)
            },
        )
    }
    pub fn collect_archived(
        &self,
        archived: &super::archiver::ArchivedPackage,
        installer: &str,
        user: i32,
    ) -> Result<crate::package::restrictions::ArchiveState, Exception> {
        self.call(
            external_api::COLLECT_ARCHIVED,
            |p| {
                p.write_i32(1);
                archived.write_to(p);
                p.write_string16(Some(installer));
                p.write_i32(user);
            },
            |r| {
                if r.read_i32()? == 0 {
                    return Err(BAD_VALUE);
                }
                super::archiver::read_metadata(r)
            },
        )
    }
    pub fn destroy_data(
        &self,
        volume: Option<&str>,
        name: &str,
        user: i32,
        ce: i64,
    ) -> Result<(), Exception> {
        self.call(
            external_api::DESTROY_APP_DATA,
            |p| {
                p.write_string16(volume);
                p.write_string16(Some(name));
                p.write_i32(user);
                p.write_i64(ce);
            },
            |_| Ok(()),
        )
    }
    pub fn destroy_profiles(&self, name: &str) -> Result<(), Exception> {
        self.call(
            external_api::DESTROY_PROFILES,
            |p| p.write_string16(Some(name)),
            |_| Ok(()),
        )
    }
    pub fn remove_code(&self, name: &str, path: &str) -> Result<(), Exception> {
        self.call(
            external_api::REMOVE_CODE,
            |p| {
                p.write_string16(Some(name));
                p.write_string16(Some(path));
            },
            |_| Ok(()),
        )
    }
    pub fn prepare_permissions(
        &self,
        name: &str,
        app: i32,
        user: i32,
    ) -> Result<Strong, Exception> {
        self.call_with_reply(
            external_api::PREPARE_PERMISSIONS,
            |p| {
                p.write_string16(Some(name));
                p.write_i32(app);
                p.write_i32(user);
            },
            |r, reply| {
                // Acquire in the actual receiving process before freeing the
                // reply buffer's original reference.
                reply.retain_remote_binder(r.read_binder()?.ok_or(BAD_VALUE)?)
            },
        )
    }
    pub fn apply_permissions(
        &self,
        scope: Strong,
        code_removed: bool,
        clear_grants: bool,
    ) -> Result<(), Exception> {
        use aim_service_aidl::dev_aim_server_iinstallerpermissionremoval as permission_api;
        let mut data = Parcel::new();
        data.write_interface_token(permission_api::DESCRIPTOR);
        data.write_bool(code_removed);
        data.write_bool(clear_grants);
        let reply = scope
            .transact(permission_api::APPLY, &data, false)
            .map_err(|code| illegal(format!("Permission removal transport: {code}")))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|code| illegal(format!("Permission removal reply: {code}")))??;
        if reader.remaining() != 0 {
            return Err(illegal("Permission removal reply has trailing data"));
        }
        Ok(())
    }
    pub fn status(
        &self,
        receiver: Option<IntentSender>,
        package: &str,
        status: i32,
        message: Option<&str>,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_UNINSTALL_STATUS,
            |p| {
                aim_service_aidl::write_typed(p, receiver.as_ref());
                p.write_string16(Some(package));
                p.write_i32(status);
                p.write_string16(message);
            },
            |_| Ok(()),
        )
    }
    pub fn install_status(
        &self,
        receiver: IntentSender,
        id: i32,
        package: &str,
        status: i32,
        message: Option<&str>,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_ARCHIVED_INSTALL_STATUS,
            |p| {
                aim_service_aidl::write_typed(p, Some(&receiver));
                p.write_i32(id);
                p.write_string16(Some(package));
                p.write_i32(status);
                p.write_string16(message);
            },
            |_| Ok(()),
        )
    }
    pub fn clear_archive_caches(
        &self,
        volume: Option<&str>,
        name: &str,
        user: i32,
        ce: i64,
    ) -> Result<(), Exception> {
        self.call(
            external_api::CLEAR_ARCHIVE_CACHES,
            |p| {
                p.write_string16(volume);
                p.write_string16(Some(name));
                p.write_i32(user);
                p.write_i64(ce);
            },
            |_| Ok(()),
        )
    }
    pub fn user_action(
        &self,
        receiver: Option<IntentSender>,
        package: &str,
        flags: i32,
        observer: Binder,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_UNINSTALL_USER_ACTION,
            |p| {
                aim_service_aidl::write_typed(p, receiver.as_ref());
                p.write_string16(Some(package));
                p.write_i32(flags);
                p.write_binder(Some(observer));
            },
            |_| Ok(()),
        )
    }
    fn forward_action(
        &self,
        receiver: Option<IntentSender>,
        package: &str,
        action: &super::codec::Object,
    ) -> Result<(), Exception> {
        self.call(
            external_api::FORWARD_UNINSTALL_USER_ACTION,
            |p| {
                aim_service_aidl::write_typed(p, receiver.as_ref());
                p.write_string16(Some(package));
                p.write_raw(&action.bytes, &action.objects);
            },
            |_| Ok(()),
        )
    }
    pub fn confirmation(
        &self,
        receiver: IntentSender,
        name: &str,
        user: i32,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_UNARCHIVE_CONFIRMATION,
            |p| {
                aim_service_aidl::write_typed(p, Some(&receiver));
                p.write_string16(Some(name));
                p.write_i32(user);
            },
            |_| Ok(()),
        )
    }
    pub fn unarchive_status(
        &self,
        receiver: IntentSender,
        name: &str,
        installer: &str,
        title: &str,
        user: i32,
        status: i32,
        bytes: i64,
        action: Option<IntentSender>,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_UNARCHIVE_STATUS,
            |p| {
                aim_service_aidl::write_typed(p, Some(&receiver));
                p.write_string16(Some(name));
                p.write_string16(Some(installer));
                p.write_string16(Some(title));
                p.write_i32(user);
                p.write_i32(status);
                p.write_i64(bytes);
                aim_service_aidl::write_typed(p, action.as_ref());
            },
            |_| Ok(()),
        )
    }
    pub fn broadcast_unarchive(
        &self,
        name: &str,
        installer: &str,
        user: i32,
        id: i32,
    ) -> Result<(), Exception> {
        self.call(
            external_api::BROADCAST_UNARCHIVE,
            |p| {
                p.write_string16(Some(name));
                p.write_string16(Some(installer));
                p.write_i32(user);
                p.write_i32(id);
                p.write_bool(false);
            },
            |_| Ok(()),
        )
    }
    pub fn removed(
        &self,
        name: &str,
        app: i32,
        user: i32,
        data: bool,
        full: bool,
        uid: bool,
        version: i64,
    ) -> Result<(), Exception> {
        self.call(
            external_api::SEND_REMOVED_BROADCAST,
            |p| {
                p.write_string16(Some(name));
                p.write_i32(app);
                p.write_i32(user);
                p.write_bool(data);
                p.write_bool(full);
                p.write_bool(uid);
                p.write_i64(version);
            },
            |_| Ok(()),
        )
    }
}

pub const KEEP_DATA: i32 = 1;
pub const ALL_USERS: i32 = 2;
pub const SYSTEM_APP: i32 = 4;
pub const ARCHIVE: i32 = 16;
pub const SUCCEEDED: i32 = 1;
pub const INTERNAL_ERROR: i32 = -1;
pub const DEVICE_POLICY: i32 = -2;
pub const USER_RESTRICTED: i32 = -3;
pub const OWNER_BLOCKED: i32 = -4;
pub const USED_LIBRARY: i32 = -6;
pub const APP_PINNED: i32 = -7;

#[derive(Clone, Debug)]
pub struct Request {
    pub package: String,
    pub version: i64,
    pub caller_package: Option<String>,
    pub uid: u32,
    pub pid: i32,
    pub user: i32,
    pub flags: i32,
    pub existing_only: bool,
}
/// Captured from the original UM/DPM/ATM owners and the native protection owner.
/// Empty sets mean the owners explicitly answered no, not unavailable owners.
pub struct Policy {
    pub device: super::policy::DevicePolicy,
    pub can_silently_install: bool,
    pub emergency_installer: bool,
    pub system_protection_role: bool,
    pub pinned: bool,
    pub admins: BTreeSet<i32>,
    pub protected: BTreeSet<i32>,
    pub uninstall_restricted: BTreeSet<i32>,
    pub users: Vec<i32>,
    pub child_users: BTreeMap<i32, Vec<i32>>,
    pub verifier_packages: Vec<String>,
    pub uninstaller_package: Option<String>,
    pub storage_manager_package: Option<String>,
    pub keep_uninstalled: bool,
    pub sdk_library_independence: bool,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub request: Request,
    pub internal_package: String,
    pub users: Vec<i32>,
    pub blocked_users: Vec<i32>,
    pub keep_uninstalled: bool,
}
pub enum Admission {
    UserAction,
    Rejected(i32),
    Accepted(Plan),
}

fn installer_matches(q: &Query<'_>, name: Option<&str>) -> Result<bool, Exception> {
    let Some(name) = name else { return Ok(false) };
    Ok(
        q.package_uid_internal(name, 0, apps_filter::user_id(q.calling_uid), 1000)
            .map_err(policy::unknown)?
            == q.calling_uid,
    )
}
pub fn admit(
    q: &Query<'_>,
    request: Request,
    policy_state: &Policy,
) -> Result<Admission, Exception> {
    policy::cross_user(q, &policy_state.device, request.user, true, "uninstall")?;
    if !matches!(request.uid, 0 | 2000) {
        policy::check_package(q, request.caller_package.as_deref())?;
    }
    let has_delete = policy::permission(q, "android.permission.DELETE_PACKAGES")?;
    if request.existing_only && !has_delete {
        return Err(Exception::security(
            "uninstallExistingPackage requires DELETE_PACKAGES",
        ));
    }
    let privileged = has_delete
        || policy_state.can_silently_install
        || policy_state.emergency_installer
        || policy_state.system_protection_role;
    if !privileged {
        let caller = request
            .caller_package
            .as_deref()
            .and_then(|name| q.state.packages.get(name));
        if caller.is_none_or(|package| package.target_sdk_version >= 28)
            && !policy::permission(q, "android.permission.REQUEST_DELETE_PACKAGES")?
        {
            return Err(Exception::security(
                "uninstall requires REQUEST_DELETE_PACKAGES",
            ));
        }
        return Ok(Admission::UserAction);
    }
    if request.version < -1 {
        return Err(Exception::new(
            EX_ILLEGAL_ARGUMENT,
            "versionCode must be >= -1",
        ));
    }
    if policy_state.pinned {
        return Ok(Admission::Rejected(APP_PINNED));
    }
    let internal = q.resolve_internal_package_name(&request.package, request.version);
    let Some(package) = q.state.packages.get(&internal) else {
        return Ok(Admission::Rejected(INTERNAL_ERROR));
    };
    let mut users = if request.flags & ALL_USERS != 0 {
        policy_state.users.clone()
    } else {
        vec![request.user]
    };
    if request.existing_only {
        let count = policy_state
            .users
            .iter()
            .filter(|user| package.users.get(user).is_none_or(|state| state.installed))
            .count();
        if count <= 1 {
            return Ok(Admission::Rejected(INTERNAL_ERROR));
        }
    }
    let silently_allowed = matches!(request.uid, 0 | 2000)
        || apps_filter::app_id(request.uid as i32) == 1000
        || package.install_source.is_orphaned
        || policy_state.can_silently_install
        || policy_state.emergency_installer
        || policy_state.system_protection_role
        || request.existing_only
        || installer_matches(q, package.install_source.installer.as_deref())?
        || policy_state
            .verifier_packages
            .iter()
            .map(|name| installer_matches(q, Some(name)))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|matches| matches)
        || installer_matches(q, policy_state.uninstaller_package.as_deref())?
        || installer_matches(q, policy_state.storage_manager_package.as_deref())?
        || policy::permission(q, "android.permission.MANAGE_PROFILE_AND_DEVICE_OWNERS")?;
    if !silently_allowed {
        return Ok(Admission::UserAction);
    }
    for &user in &users {
        policy::cross_user(q, &policy_state.device, user, true, "deletePackage")?;
        if policy_state.admins.contains(&user) {
            return Ok(Admission::Rejected(DEVICE_POLICY));
        }
        if policy_state.protected.contains(&user) {
            return Ok(Admission::Rejected(INTERNAL_ERROR));
        }
    }
    if policy_state.uninstall_restricted.contains(&request.user) {
        return Ok(Admission::Rejected(USER_RESTRICTED));
    }
    let blocks = q
        .state
        .system
        .uninstall_blocks
        .as_ref()
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Uninstall block owner unavailable"))?;
    let blocked: Vec<_> = users
        .iter()
        .copied()
        .filter(|user| blocks.get(*user, Some(&internal)))
        .collect();
    if request.flags & ALL_USERS == 0 && !blocked.is_empty() {
        return Ok(Admission::Rejected(OWNER_BLOCKED));
    }
    users.retain(|user| !blocked.contains(user));
    if request.flags & ALL_USERS == 0 {
        for &child in policy_state
            .child_users
            .get(&request.user)
            .into_iter()
            .flatten()
        {
            if package
                .users
                .get(&child)
                .is_some_and(|state| state.installed)
                && !users.contains(&child)
            {
                users.push(child);
            }
        }
    }
    if request.version != -1 && request.version != package.version_code {
        return Ok(Admission::Rejected(INTERNAL_ERROR));
    }
    Ok(Admission::Accepted(Plan {
        request,
        internal_package: internal,
        users,
        blocked_users: blocked,
        keep_uninstalled: policy_state.keep_uninstalled,
    }))
}

/// PackageSetting.setUserState preserves the inode/runtime owner while resetting
/// exactly the fields DeletePackageHelper supplies. KEEP_DATA retains components,
/// first-install time and archive metadata; it does not retain enabled state.
pub fn uninstalled(old: &UserState, flags: i32) -> UserState {
    let mut state = old.clone();
    state.installed = false;
    state.enabled = 0;
    state.stopped = true;
    state.not_launched = true;
    state.hidden = false;
    state.distraction_flags = 0;
    state.suspensions = None;
    state.instant_app = false;
    state.virtual_preload = false;
    state.last_disable_app_caller = None;
    state.install_reason = 0;
    state.uninstall_reason = 0;
    state.harmful_app_warning = None;
    state.splash_screen_theme = None;
    state.min_aspect_ratio = 0;
    if flags & KEEP_DATA == 0 {
        state.enabled_components = None;
        state.disabled_components = None;
        state.first_install_time = 0;
        state.archive_state = None;
    } else {
        state.enabled_components = Some(old.enabled_components.clone().unwrap_or_default());
        state.disabled_components = Some(old.disabled_components.clone().unwrap_or_default());
    }
    state
}

pub type Publication =
    Arc<dyn Fn(&Arc<Snapshot>, &Arc<Snapshot>) -> Result<(), Exception> + Send + Sync>;
pub type CacheInvalidation = Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>;
pub struct NativeStore {
    pub snapshots: Arc<scan_snapshot::Store>,
    pub disk: Arc<Mutex<owner::Store>>,
    pub publish: Publication,
    pub invalidate: CacheInvalidation,
}
pub type PolicySource =
    Arc<dyn Fn(&Query<'_>, &Request) -> Result<Policy, Exception> + Send + Sync>;
pub type PreferredRemoval = Arc<dyn Fn(&str, i32, bool) -> Result<PostCommitEffect, Exception> + Send + Sync>;
pub enum PostCommitEffect {
    Deferred(Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>),
    Added {
        owner: Arc<super::super::effects::Owner>,
        package: String,
        user: i32,
    },
    Removed {
        owner: Arc<External>,
        package: String,
        app_id: i32,
        user: i32,
        data_removed: bool,
        package_removed: bool,
        uid_removed: bool,
        version: i64,
    },
}
impl PostCommitEffect {
    fn finish(self) -> Result<(), Exception> {
        match self {
            Self::Deferred(finish) => finish(),
            Self::Added {
                owner,
                package,
                user,
            } => owner.package_added(&package, user, false, 0, None),
            Self::Removed {
                owner,
                package,
                app_id,
                user,
                data_removed,
                package_removed,
                uid_removed,
                version,
            } => owner.removed(
                &package,
                app_id,
                user,
                data_removed,
                package_removed,
                uid_removed,
                version,
            ),
        }
    }
}
pub struct FactoryEffects {
    pub effects: Vec<PostCommitEffect>,
}
pub type RestoreFactory = Arc<dyn Fn(&Plan) -> Result<FactoryEffects, Exception> + Send + Sync>;
pub struct Controller {
    pub store: Arc<NativeStore>,
    pub source: super::native::QuerySource,
    pub resolver: crate::package::resolve::Resolver,
    pub effects: Arc<crate::package::effects::Owner>,
    pub external: Arc<External>,
    pub policy: PolicySource,
    pub publisher: super::native::Publisher,
    pub factory_restore: RestoreFactory,
    pub preferred_removal: PreferredRemoval,
    pub gate: Arc<Mutex<()>>,
    pub keystore: Arc<owner::keystore::KeystoreCleanup>,
}
impl Controller {
    fn with_query<T>(
        &self,
        uid: u32,
        action: impl FnOnce(&Query<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let state = (self.source)()?;
        let resolved = self
            .resolver
            .resolution(&state)
            .map_err(|error| illegal(format!("Removal visibility: {error:?}")))?;
        action(&Query {
            state: &state,
            filter: &resolved.apps_filter,
            calling_uid: uid as i32,
        })
    }
    pub fn cross_user(
        &self,
        query: &Query<'_>,
        uid: u32,
        user: i32,
        operation: &str,
    ) -> Result<(), Exception> {
        let request = Request {
            package: String::new(),
            version: -1,
            caller_package: None,
            uid,
            pid: 0,
            user,
            flags: 0,
            existing_only: false,
        };
        let capture = (self.policy)(query, &request)?;
        policy::cross_user(query, &capture.device, user, true, operation)
    }
    pub fn verify_unarchive_receiver(
        &self,
        state: &State,
        installer: &str,
        user: i32,
        shell: bool,
    ) -> Result<(), Exception> {
        let info = state
            .packages
            .get(installer)
            .filter(|package| package.users.get(&user).is_none_or(|user| user.installed))
            .ok_or_else(|| super::archiver::name_error("Failed to obtain Installer info"))?;
        if shell {
            return Ok(());
        }
        let resolution = self
            .resolver
            .resolution(&Arc::new(state.clone()))
            .map_err(|error| illegal(format!("Unarchive receiver: {error:?}")))?;
        let intent = crate::package::intent::Intent {
            action: Some("android.intent.action.UNARCHIVE_PACKAGE".into()),
            package: Some(info.name.clone()),
            ..Default::default()
        };
        let targets = resolution
            .query_intent_receivers(&intent, None, 0, user, 1000)
            .map_err(|error| illegal(format!("Unarchive receiver lookup: {error:?}")))?;
        if targets.is_empty() {
            return Err(super::archiver::name_error(
                "Installer does not support unarchival",
            ));
        }
        Ok(())
    }
    pub fn uninstall(
        self: &Arc<Self>,
        request: Request,
        receiver: Option<IntentSender>,
    ) -> Result<(), Exception> {
        let admission = self.with_query(request.uid, |query| {
            let captured = (self.policy)(query, &request)?;
            policy::cross_user(query, &captured.device, request.user, true, "uninstall")?;
            if !matches!(request.uid, 0 | 2000) {
                self.external
                    .check_package(request.uid, request.caller_package.as_deref())?;
            }
            admit(query, request.clone(), &captured)
        })?;
        match admission {
            Admission::Rejected(status) => self.complete(&request, receiver, status, None),
            Admission::UserAction => {
                let reference = receiver
                    .map(|receiver| self.external.retain_sender(receiver))
                    .transpose()?;
                let observer = (self.publisher)(Arc::new(DeleteObserver {
                    owner: self.clone(),
                    request: request.clone(),
                    receiver,
                    _receiver_ref: reference,
                }))?;
                self.external
                    .user_action(receiver, &request.package, request.flags, observer)
            }
            Admission::Accepted(plan) => {
                let status = self.execute(&plan)?;
                self.complete(&request, receiver, status, None)
            }
        }
    }
    fn complete(
        &self,
        request: &Request,
        receiver: Option<IntentSender>,
        status: i32,
        message: Option<&str>,
    ) -> Result<(), Exception> {
        if status != SUCCEEDED && request.flags & ARCHIVE != 0 {
            let guard = self.gate.lock().unwrap();
            let capture = self.store.snapshots.capture();
            let before = capture.version();
            let result = (|| -> Result<(), Exception> {
                if let Some(mut state) = capture
                    .owner()
                    .scanned_user_states(&request.package)
                    .and_then(|users| users.get(&request.user))
                    .cloned()
                {
                    state.archive_state = None;
                    self.store
                        .user_state(&request.package, request.user, state)?;
                }
                Ok(())
            })();
            drop(guard);
            self.store.finish_after_unlock(before, result)?;
        }
        self.external
            .status(receiver, &request.package, status, message)
    }
    /// Source DeletePackageX: internal pruning bypasses public caller admission.
    /// It still owns version/library/system gates, storage and durable publication.
    pub fn delete_x(
        &self,
        package: &str,
        version: i64,
        user: i32,
        flags: i32,
        removed_by_system: bool,
    ) -> Result<i32, Exception> {
        let state = (self.source)()?;
        let request = Request {
            package: package.into(),
            version,
            caller_package: None,
            uid: 1000,
            pid: 0,
            user,
            flags,
            existing_only: false,
        };
        let resolved = self
            .resolver
            .resolution(&state)
            .map_err(|error| illegal(format!("Internal removal resolution: {error:?}")))?;
        let query = Query {
            state: &state,
            filter: &resolved.apps_filter,
            calling_uid: 1000,
        };
        let policy = (self.policy)(&query, &request)?;
        let users = if flags & ALL_USERS != 0 {
            policy.users
        } else {
            vec![user]
        };
        let plan = Plan {
            internal_package: query.resolve_internal_package_name(package, version),
            request,
            users,
            blocked_users: vec![],
            keep_uninstalled: policy.keep_uninstalled,
        };
        let _ = removed_by_system;
        self.execute(&plan)
    }
    pub(crate) fn execute(&self, plan: &Plan) -> Result<i32, Exception> {
        let guard = self.gate.lock().unwrap();
        let before = self.store.snapshots.capture().version();
        let mut effects = Vec::new();
        let result = self.execute_locked(plan, &mut effects);
        drop(guard);
        let mut result = self.store.finish_after_unlock(before, result);
        for effect in effects {
            if let Err(error) = effect.finish() {
                match &mut result {
                    Ok(_) => result = Err(error),
                    Err(cause) => cause
                        .message
                        .push_str(&format!("; post-commit effect: {}", error.message)),
                }
            }
        }
        result
    }
    fn execute_locked(
        &self,
        plan: &Plan,
        effects: &mut Vec<PostCommitEffect>,
    ) -> Result<i32, Exception> {
        if plan.users.is_empty() {
            return Ok(if plan.blocked_users.is_empty() {
                INTERNAL_ERROR
            } else {
                OWNER_BLOCKED
            });
        }
        let sdk_independence = self.with_query(plan.request.uid, |query| {
            Ok((self.policy)(query, &plan.request)?.sdk_library_independence)
        })?;
        let current = (self.source)()?;
        let Some(package) = current.packages.get(&plan.internal_package).cloned() else {
            return Ok(INTERNAL_ERROR);
        };
        if plan.request.version != -1 && plan.request.version != package.version_code {
            return Ok(INTERNAL_ERROR);
        }
        if let Some(code) = &package.pkg {
            for other in current
                .packages
                .values()
                .filter(|other| other.name != package.name)
            {
                if !plan
                    .users
                    .iter()
                    .any(|user| other.users.get(user).is_none_or(|state| state.installed))
                {
                    continue;
                }
                if code
                    .static_shared_library_name
                    .as_ref()
                    .is_some_and(|name| {
                        other.uses_static_libraries.iter().any(|(used, version)| {
                            used == name && *version == code.static_shared_lib_version
                        })
                    })
                    || code.sdk_library_name.as_ref().is_some_and(|name| {
                        other.uses_sdk_libraries.iter().any(|used| {
                            used.name == *name
                                && used.version_major == code.sdk_lib_version_major as i64
                                && (!sdk_independence || !used.optional)
                        })
                    })
                {
                    return Ok(USED_LIBRARY);
                }
            }
        }
        if package.is.system && plan.request.flags & SYSTEM_APP == 0 {
            if !current.disabled_system_packages.contains_key(&package.name) {
                return Ok(INTERNAL_ERROR);
            }
            effects.extend((self.factory_restore)(plan)?.effects);
            return Ok(SUCCEEDED);
        }
        if package.is.system && plan.request.flags & ALL_USERS != 0 {
            return Ok(INTERNAL_ERROR);
        }
        let data_removed = plan.request.flags & KEEP_DATA == 0;
        let remove_setting = !package.is.system && !plan.keep_uninstalled && data_removed
            && !package.users.iter().any(|(user, state)| state.installed && !plan.users.contains(user));
        self.external.destroy_profiles(&package.name)?;
        for &user in &plan.users {
            self.effects
                .kill(&package.name, package.app_id, user, "uninstall pkg", 10)?;
            let captured = self.store.snapshots.capture();
            let old = captured
                .owner()
                .scanned_user_states(&package.name)
                .and_then(|users| users.get(&user))
                .cloned()
                .unwrap_or_default();
            if data_removed {
                self.external.destroy_data(
                    package.volume_uuid.as_deref(),
                    &package.name,
                    user,
                    old.ce_data_inode,
                )?;
            } else if plan.request.flags & ARCHIVE != 0 {
                self.external.clear_archive_caches(
                    package.volume_uuid.as_deref(),
                    &package.name,
                    user,
                    old.ce_data_inode,
                )?;
            }
            let mut next = uninstalled(&old, plan.request.flags);
            if data_removed {
                next.ce_data_inode = -1;
                next.de_data_inode = -1;
            }
            self.store.user_state(&package.name, user, next)?;
            if data_removed {
                self.store.clear_domain_user(&package.name, user)?;
                if !remove_setting || plan.users.first() == Some(&user) {
                    let preferred_user = if remove_setting { -1 } else { user };
                    effects.push((self.preferred_removal)(&package.name, preferred_user, false)?);
                }
                let scope =
                    self.external
                        .prepare_permissions(&package.name, package.app_id, user)?;
                self.external.apply_permissions(scope, false, true)?;
                self.keystore
                    .post(package.app_id, &[user])
                    .map_err(illegal)?;
            }
        }
        let capture = self.store.snapshots.capture();
        let installed = capture
            .owner()
            .scanned_user_states(&package.name)
            .is_some_and(|users| users.values().any(|state| state.installed));
        let remove_code = !package.is.system && !installed && !plan.keep_uninstalled;
        if remove_code {
            let permissions =
                self.external
                    .prepare_permissions(&package.name, package.app_id, -1)?;
            if data_removed {
                self.store.remove_setting(&package.name)?;
            } else {
                self.store.retire_code(&package.name)?;
            }
            self.external
                .apply_permissions(permissions, true, data_removed)?;
            self.external.remove_code(&package.name, &package.path)?;
            if let Some(real) = &package.real_name {
                if let Some(real) = real {
                    self.store
                        .disk
                        .lock()
                        .unwrap()
                        .commit_removed_renamed_package(real)
                        .map_err(|error| {
                            illegal(format!("Removal renamed cleanup: {}", error.message))
                        })?;
                }
            }
        }
        let uid_removed = remove_code
            && data_removed
            && self
                .store
                .snapshots
                .capture()
                .owner()
                .identities
                .ids
                .get(package.app_id)
                .is_none();
        for &user in &plan.users {
            effects.push(PostCommitEffect::Removed {
                owner: self.external.clone(),
                package: package.name.clone(),
                app_id: package.app_id,
                user,
                data_removed,
                package_removed: remove_code && data_removed,
                uid_removed,
                version: package.version_code,
            });
        }
        Ok(if plan.blocked_users.is_empty() {
            SUCCEEDED
        } else {
            OWNER_BLOCKED
        })
    }
}
struct DeleteObserver {
    owner: Arc<Controller>,
    request: Request,
    receiver: Option<IntentSender>,
    _receiver_ref: Option<Arc<Strong>>,
}
impl aim_binder_host::local::Service for DeleteObserver {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_content_pm_ipackagedeleteobserver2::DESCRIPTOR
    }
    fn transact(
        &self,
        call: &mut aim_binder_host::local::Call<'_>,
    ) -> aim_binder_host::local::Reply {
        use aim_service_aidl::android_content_pm_ipackagedeleteobserver2 as api;
        let result = match call.code {
            api::ON_PACKAGE_DELETED => {
                let args = api::OnPackageDeleted::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(BAD_VALUE);
                }
                self.owner.complete(
                    &self.request,
                    self.receiver,
                    args.return_code,
                    args.msg.as_deref(),
                )
            }
            api::ON_USER_ACTION_REQUIRED => {
                call.data.enforce_interface(api::DESCRIPTOR)?;
                let start = call.data.position();
                call.data.skip(call.data.remaining())?;
                let (bytes, objects) = call.data.since(start);
                self.owner.external.forward_action(
                    self.receiver,
                    &self.request.package,
                    &Object {
                        bytes: bytes.to_vec(),
                        objects,
                    },
                )
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        let mut reply = Parcel::new();
        match result {
            Ok(()) => reply.write_no_exception(),
            Err(error) => reply.write_exception(&error),
        };
        Ok(reply)
    }
}

/// Shared endpoint integration for the six installer removal/archive methods.
/// The caller supplies its exact native query snapshot; all input bytes are
/// consumed before calling any policy, persistence or callback owner.
pub fn dispatch(
    controller: &Arc<Controller>,
    archiver: &Arc<super::archiver::Owner>,
    query: &Query<'_>,
    call: &mut aim_binder_host::local::Call<'_>,
) -> Option<aim_binder_host::local::Reply> {
    use aim_service_aidl::android_content_pm_ipackageinstaller as api;
    if !matches!(
        call.code,
        api::UNINSTALL
            | api::UNINSTALL_EXISTING_PACKAGE
            | api::REQUEST_ARCHIVE
            | api::REQUEST_UNARCHIVE
            | api::INSTALL_PACKAGE_ARCHIVED
            | api::REPORT_UNARCHIVAL_STATUS
    ) {
        return None;
    }
    Some((|| {
        enum Action {
            Delete(api::Uninstall<VersionedPackage, IntentSender>),
            DeleteExisting(api::UninstallExistingPackage<VersionedPackage, IntentSender>),
            Archive(api::RequestArchive<IntentSender, UserHandle>),
            Unarchive(api::RequestUnarchive<IntentSender, UserHandle>),
            Install(
                api::InstallPackageArchived<
                    super::archiver::ArchivedPackage,
                    super::codec::SessionParams,
                    IntentSender,
                    UserHandle,
                >,
            ),
            Report(api::ReportUnarchivalStatus<IntentSender, UserHandle>),
        }
        let action = match call.code {
            api::UNINSTALL => Action::Delete(api::Uninstall::read(&mut call.data)?),
            api::UNINSTALL_EXISTING_PACKAGE => {
                Action::DeleteExisting(api::UninstallExistingPackage::read(&mut call.data)?)
            }
            api::REQUEST_ARCHIVE => Action::Archive(api::RequestArchive::read(&mut call.data)?),
            api::REQUEST_UNARCHIVE => {
                Action::Unarchive(api::RequestUnarchive::read(&mut call.data)?)
            }
            api::INSTALL_PACKAGE_ARCHIVED => {
                Action::Install(api::InstallPackageArchived::read(&mut call.data)?)
            }
            api::REPORT_UNARCHIVAL_STATUS => {
                Action::Report(api::ReportUnarchivalStatus::read(&mut call.data)?)
            }
            _ => unreachable!(),
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let uid = call.sender_euid;
        let pid = call.sender_pid;
        let result = (|| -> Result<(), Exception> {
            let null = || {
                Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "null installer argument",
                )
            };
            match action {
                Action::Delete(args) => {
                    let versioned = args.versioned_package.ok_or_else(null)?;
                    controller.uninstall(
                        Request {
                            package: versioned.name,
                            version: versioned.version,
                            caller_package: args.caller_package_name,
                            uid,
                            pid,
                            user: args.user_id,
                            flags: args.flags,
                            existing_only: false,
                        },
                        args.status_receiver,
                    )
                }
                Action::DeleteExisting(args) => {
                    let versioned = args.versioned_package.ok_or_else(null)?;
                    controller.uninstall(
                        Request {
                            package: versioned.name,
                            version: versioned.version,
                            caller_package: args.caller_package_name,
                            uid,
                            pid,
                            user: args.user_id,
                            flags: 0,
                            existing_only: true,
                        },
                        args.status_receiver,
                    )
                }
                Action::Archive(args) => archiver.request_archive(
                    query,
                    &args.package_name.ok_or_else(null)?,
                    &args.caller_package_name.ok_or_else(null)?,
                    uid,
                    pid,
                    args.user_handle.ok_or_else(null)?.0,
                    args.flags,
                    args.status_receiver.ok_or_else(null)?,
                ),
                Action::Unarchive(args) => archiver.request_unarchive(
                    query,
                    &args.package_name.ok_or_else(null)?,
                    &args.caller_package_name.ok_or_else(null)?,
                    uid,
                    pid,
                    args.user_handle.ok_or_else(null)?.0,
                    args.status_receiver.ok_or_else(null)?,
                    false,
                ),
                Action::Install(args) => archiver.install_archived(
                    query,
                    uid,
                    pid,
                    args.archived_package_parcel.ok_or_else(null)?,
                    args.params.ok_or_else(null)?,
                    &args.installer_package_name.ok_or_else(null)?,
                    args.user_handle.ok_or_else(null)?.0,
                    args.status_receiver.ok_or_else(null)?,
                ),
                Action::Report(args) => archiver.report(
                    uid,
                    args.unarchive_id,
                    args.user_handle.ok_or_else(null)?.0,
                    args.status,
                    args.required_storage_bytes,
                    args.user_action_intent,
                ),
            }
        })();
        let mut reply = Parcel::new();
        match result {
            Ok(()) => reply.write_no_exception(),
            Err(error) => reply.write_exception(&error),
        };
        Ok(reply)
    })())
}
impl NativeStore {
    /// Every publication is purely native. The owning operation releases its
    /// installation gate before calling this synchronous cache owner boundary.
    pub fn finish_after_unlock<T>(
        &self,
        before: u64,
        result: Result<T, Exception>,
    ) -> Result<T, Exception> {
        if self.snapshots.capture().version() == before {
            return result;
        }
        match ((self.invalidate)(), result) {
            (Ok(()), result) => result,
            (Err(cache), Ok(_)) => Err(cache),
            (Err(cache), Err(mut cause)) => {
                cause.message.push_str(&format!(
                    "; committed cache invalidation: {}",
                    cache.message
                ));
                Err(cause)
            }
        }
    }

    pub fn clear_domain_user(&self, name: &str, user: i32) -> Result<Arc<Snapshot>, Exception> {
        let base = self.snapshots.capture();
        let mut next = base.owner().clone();
        for package in next
            .settings
            .domain_verification
            .active
            .iter_mut()
            .chain(&mut next.settings.domain_verification.restored)
            .filter(|package| package.name == name)
        {
            package.users.retain(|state| state.id != user);
        }
        let mut disk = self.disk.lock().unwrap();
        let result = self
            .snapshots
            .publish_removal_after(&base, &BTreeSet::from([name.to_owned()]), next, |snapshot| {
                disk.commit_domains(&snapshot.owner().settings.domain_verification)
            });
        drop(disk);
        self.finish_publication(base, result)
    }
    pub fn retire_code(&self, name: &str) -> Result<Arc<Snapshot>, Exception> {
        let base = self.snapshots.capture();
        let next = base
            .owner()
            .prepare_live_code_retirement(name)
            .map_err(illegal)?;
        let result = self.snapshots.publish_removal_after(&base,&BTreeSet::from([name.to_owned()]),next, |_| Ok(()));
        self.finish_publication(base,result)
    }
    pub fn user_state(
        &self,
        name: &str,
        user: i32,
        state: UserState,
    ) -> Result<Arc<Snapshot>, Exception> {
        self.user_states(user,vec![(name.to_owned(),state)])
    }
    /// One reconciliation volume publishes its actual installd inode receipts
    /// together. Physical creation has already happened, even if another member
    /// subsequently failed; the caller still returns those operation failures.
    pub fn user_states(&self,user:i32,states:Vec<(String,UserState)>) -> Result<Arc<Snapshot>,Exception> {
        if user<0{return Err(illegal("Negative app-data receipt user"));}
        let base = self.snapshots.capture();
        let mut next = base.owner().clone();
        let mut names=BTreeSet::new();
        for (name,state) in states {
            if !names.insert(name.clone()){return Err(illegal("Duplicate app-data receipt package"));}
            next.set_user_state(&name,user,state).map_err(illegal)?;
        }
        if names.is_empty(){return Ok(base);}
        next.complete_retained_library_dependencies().map_err(illegal)?;
        if !next.loaded_packages().is_empty() {
            super::permission_prepare::complete_candidate_runtime(&mut next, &base, base.usage()).map_err(illegal)?;
        }
        let mut disk = self.disk.lock().unwrap();
        let result = self
            .snapshots
            .publish_removal_after(&base, &names, next, |snapshot| {
                disk.commit_live_install_restrictions(
                    snapshot.owner(),
                    user as u32,
                    &names,
                    crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS,
                )
            });
        drop(disk);
        self.finish_publication(base, result)
    }
    fn finish_publication(
        &self,
        base: Arc<Snapshot>,
        result: Result<Arc<Snapshot>, scan_snapshot::CommitError>,
    ) -> Result<Arc<Snapshot>, Exception> {
        match result {
            Ok(next) => {
                (self.publish)(&base, &next)?;
                Ok(next)
            }
            Err(scan_snapshot::CommitError::Disk {
                snapshot: Some(next),
                error,
            }) => {
                (self.publish)(&base, &next)?;
                Err(illegal(format!(
                    "Native removal committed: {}",
                    error.message
                )))
            }
            Err(error) => Err(illegal(format!("Native removal publication: {error:?}"))),
        }
    }
    pub fn remove_setting(&self, name: &str) -> Result<Arc<Snapshot>, Exception> {
        let base = self.snapshots.capture();
        let mut metadata = base.owner().clone();
        metadata.settings.domain_verification.clear_package(name);
        owner::key_sets::clear_package(&mut metadata.settings, name).map_err(illegal)?;
        let next = metadata.prepare_live_package_removal(name).map_err(illegal)?;
        let mut disk = self.disk.lock().unwrap();
        let result = self.snapshots.publish_removal_after(&base, &BTreeSet::from([name.to_owned()]), next, |snapshot| {
            disk.commit_removed_package_all(snapshot.owner(), name)
        });
        drop(disk);
        let next = self.finish_publication(base, result)?;
        let users: Vec<_> = self
            .disk
            .lock()
            .unwrap()
            .state()
            .users
            .iter()
            .map(|(id, _)| *id)
            .collect();
        for user in users {
            self.disk
                .lock()
                .unwrap()
                .commit_removed_package_restrictions(name, user)
                .map_err(|error| {
                    illegal(format!(
                        "Removed setting committed; restriction cleanup: {}",
                        error.message
                    ))
                })?;
        }
        Ok(next)
    }
}

#[cfg(test)]
mod batch_user_state_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize,Ordering};

    struct Directory(std::path::PathBuf);
    impl Drop for Directory {fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}

    #[test]
    fn batch_user_states_commits_two_inode_receipts_once_and_rejects_invalid_second() {
        let directory=Directory(std::env::temp_dir().join(format!("aim-batch-app-data-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(directory.0.join("system/users/0")).unwrap();
        let settings=crate::package::settings::Settings {packages:[("core.one",10100),("core.two",10101)].into_iter()
            .map(|(name,app_id)|crate::package::settings::Package {name:name.into(),app_id,code_path:format!("/system/app/{name}"),
                domain_set_id:Some(format!("00000000-0000-0000-0000-{app_id:012x}")),..Default::default()}).collect(),..Default::default()};
        let mut scan=crate::package::scan::SigningScan::new(&Default::default(),&settings,36).unwrap();
        let original=UserState {installed:true,stopped:true,..Default::default()};
        scan.capture_user_states(settings.packages.iter().map(|setting|((setting.name.clone(),false),crate::package::scan::CapturedUsers {
            states:BTreeMap::from([(0,original.clone())]),active_aliases:Default::default(),
        })).collect()).unwrap();
        let snapshots=Arc::new(scan_snapshot::Store::new(scan,crate::package::owner::usage::Usage::new(["core.one","core.two"])).unwrap());
        let mut disk=owner::Store::create(&directory.0,&[0]).unwrap();
        disk.commit_scan_settings(&snapshots.capture()).unwrap();
        let publications=Arc::new(AtomicUsize::new(0));let publish_count=publications.clone();
        let invalidations=Arc::new(AtomicUsize::new(0));let invalidation_count=invalidations.clone();
        let disk=Arc::new(Mutex::new(disk));
        let published_disk=disk.clone();
        let store=NativeStore {snapshots:snapshots.clone(),disk,
            publish:Arc::new(move |base,next|{assert!(published_disk.try_lock().is_ok(),"query publisher must run after disk guard is released");assert_eq!(next.version(),base.version()+1);publish_count.fetch_add(1,Ordering::SeqCst);Ok(())}),
            invalidate:Arc::new(move ||{invalidation_count.fetch_add(1,Ordering::SeqCst);Ok(())})};
        let before=snapshots.capture();
        let first=UserState {ce_data_inode:111,de_data_inode:112,..original.clone()};
        let second=UserState {ce_data_inode:221,de_data_inode:222,..original};
        let committed=store.user_states(0,vec![("core.one".into(),first.clone()),("core.two".into(),second.clone())]).unwrap();
        assert_eq!(committed.version(),before.version()+1);
        assert_eq!(publications.load(Ordering::SeqCst),1);
        assert_eq!(committed.owner().scanned_user_states("core.one").unwrap()[&0],first);
        assert_eq!(committed.owner().scanned_user_states("core.two").unwrap()[&0],second);
        let path=directory.0.join("system/users/0/package-restrictions.xml");
        let bytes=std::fs::read(&path).unwrap();
        let root=aim_android_xml::read(&bytes).unwrap();
        let restrictions=crate::package::restrictions::Restrictions::parse(&root).unwrap();
        let written=restrictions.packages.into_iter().collect::<BTreeMap<_,_>>();
        assert_eq!(written["core.one"].ce_data_inode,111);
        assert_eq!(written["core.one"].de_data_inode,112);
        assert_eq!(written["core.two"].ce_data_inode,221);
        assert_eq!(written["core.two"].de_data_inode,222);
        let changed=UserState {de_data_inode:999,..first};
        assert!(store.user_states(0,vec![("core.one".into(),changed),("unaccepted.package".into(),second)]).is_err());
        assert!(Arc::ptr_eq(&committed,&snapshots.capture()));
        assert_eq!(publications.load(Ordering::SeqCst),1);
        assert_eq!(std::fs::read(&path).unwrap(),bytes);
        store.finish_after_unlock(before.version(),Ok(())).unwrap();
        assert_eq!(invalidations.load(Ordering::SeqCst),1);
        store.clear_domain_user("core.one",0).unwrap();
        let uninstalled=UserState {installed:false,..snapshots.capture().owner().scanned_user_states("core.two").unwrap()[&0].clone()};
        store.user_state("core.two",0,uninstalled).unwrap();
        store.remove_setting("core.two").unwrap();
        assert_eq!(publications.load(Ordering::SeqCst),4);
    }
}
fn illegal(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

pub fn archive_installer(state: &State, name: &str) -> Option<String> {
    let source = &state.packages.get(name)?.install_source;
    source
        .update_owner
        .as_ref()
        .filter(|name| !name.is_empty())
        .or(source.installer.as_ref())
        .cloned()
}
