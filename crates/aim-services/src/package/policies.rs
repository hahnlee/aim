//! IPM non-PMS policy routing at android-16.0.0_r1 (AOSP, Apache-2.0).
//! Settings/UserManager/DPM/external-source objects live in the typed original leaf.
use super::{apps_filter::app_id, info::user_state, query::Query};
use aim_binder_host::{
    local::{Call, Reply, Strong},
    parcel::{
        BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
        Result as ParcelResult,
    },
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackagepolicybridge as policy,
};

pub struct Owner {
    bridge: Strong,
}
impl Owner {
    pub fn new(bridge: Strong) -> Self {
        Self { bridge }
    }
    /// Retain the concrete policy Binder returned by the current original bootstrap.
    pub fn capture(bootstrap_owner: &Strong) -> Result<Self, Exception> {
        let mut p = Parcel::new();
        bootstrap::GetPackagePolicyBridge {}.write(&mut p);
        let reply = bootstrap_owner
            .transact(bootstrap::GET_PACKAGE_POLICY_BRIDGE, &p, false)
            .map_err(|error| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("Policy attach transport: {error}"),
                )
            })?;
        let mut r = reply.reader();
        r.read_exception().map_err(|error| {
            Exception::new(EX_ILLEGAL_STATE, format!("Policy attach parcel: {error}"))
        })??;
        let binder = r
            .read_binder()
            .map_err(|error| {
                Exception::new(EX_ILLEGAL_STATE, format!("Policy attach Binder: {error}"))
            })?
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Package policy owner unavailable"))?;
        if r.remaining() != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Trailing policy attach reply",
            ));
        }
        let bridge = reply.retain_remote_binder(binder).map_err(|error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Policy capability retention: {error}"),
            )
        })?;
        Ok(Self::new(bridge))
    }
    fn invoke<T>(
        &self,
        code: u32,
        data: &Parcel,
        decode: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Result<T, Exception>>,
    ) -> Result<T, Exception> {
        let reply = self.bridge.transact(code, data, false).map_err(|error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Package policy transport: {error}"),
            )
        })?;
        let mut reader = reply.reader();
        let result = decode(&mut reader).map_err(|error| {
            Exception::new(EX_ILLEGAL_STATE, format!("Package policy parcel: {error}"))
        })??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Trailing package policy reply",
            ));
        }
        Ok(result)
    }
    fn report_admin_permission_denied(&self) -> Result<(), Exception> {
        let mut p = Parcel::new();
        policy::ReportAdminPermissionDenied {}.write(&mut p);
        self.invoke(
            policy::REPORT_ADMIN_PERMISSION_DENIED,
            &p,
            policy::read_report_admin_permission_denied_reply,
        )
    }
    pub fn is_storage_low(&self) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        policy::IsStorageLow {}.write(&mut p);
        self.invoke(
            policy::IS_STORAGE_LOW,
            &p,
            policy::read_is_storage_low_reply,
        )
    }
    pub fn get_install_location(&self) -> Result<i32, Exception> {
        let mut p = Parcel::new();
        policy::GetInstallLocation {}.write(&mut p);
        self.invoke(
            policy::GET_INSTALL_LOCATION,
            &p,
            policy::read_get_install_location_reply,
        )
    }
    pub fn set_install_location(
        &self,
        pid: i32,
        uid: i32,
        location: i32,
    ) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        policy::SetInstallLocation {
            caller_pid: pid,
            caller_uid: uid,
            location,
        }
        .write(&mut p);
        self.invoke(
            policy::SET_INSTALL_LOCATION,
            &p,
            policy::read_set_install_location_reply,
        )
    }
    fn install_disabled(
        &self,
        name: Option<String>,
        uid: i32,
        user: i32,
    ) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        policy::IsInstallDisabled {
            package_name: name,
            package_uid: uid,
            user_id: user,
        }
        .write(&mut p);
        self.invoke(
            policy::IS_INSTALL_DISABLED,
            &p,
            policy::read_is_install_disabled_reply,
        )
    }
    pub fn protected(
        &self,
        name: Option<String>,
        user: i32,
        data: bool,
    ) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        if data {
            policy::QueryPackageDataProtected {
                package_name: name,
                user_id: user,
            }
            .write(&mut p);
            self.invoke(
                policy::QUERY_PACKAGE_DATA_PROTECTED,
                &p,
                policy::read_query_package_data_protected_reply,
            )
        } else {
            policy::QueryPackageStateProtected {
                package_name: name,
                user_id: user,
            }
            .write(&mut p);
            self.invoke(
                policy::QUERY_PACKAGE_STATE_PROTECTED,
                &p,
                policy::read_query_package_state_protected_reply,
            )
        }
    }
    fn shell_restricted(&self, user: i32) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        policy::IsShellDebuggingRestricted { user_id: user }.write(&mut p);
        self.invoke(
            policy::IS_SHELL_DEBUGGING_RESTRICTED,
            &p,
            policy::read_is_shell_debugging_restricted_reply,
        )
    }
    pub fn device_admin(
        &self,
        name: Option<String>,
        package_exists: bool,
    ) -> Result<bool, Exception> {
        let mut p = Parcel::new();
        policy::IsPackageDeviceAdmin {
            package_name: name,
            package_exists,
        }
        .write(&mut p);
        self.invoke(
            policy::IS_PACKAGE_DEVICE_ADMIN,
            &p,
            policy::read_is_package_device_admin_reply,
        )
    }
    fn request_installs(
        &self,
        q: &Query<'_>,
        name: Option<String>,
        user: i32,
    ) -> Result<bool, Exception> {
        let package = name.as_deref().unwrap_or("");
        let uid = q
            .package_uid_internal(package, 0, user, q.calling_uid)
            .map_err(unmodelled)?;
        if q.calling_uid != uid && !matches!(q.calling_uid, 0 | 1000) {
            return Err(Exception::security(format!(
                "Caller uid {} does not own package {}",
                q.calling_uid,
                name.as_deref().unwrap_or("null")
            )));
        }
        let Some(state) = q.state.packages.get(package) else {
            return Ok(false);
        };
        if user_state(state, user).instant_app {
            return Ok(false);
        }
        let Some(pkg) = state.pkg.as_deref() else {
            return Ok(false);
        };
        if pkg.target_sdk_version < 26 {
            return Ok(false);
        }
        if !pkg
            .requested_permissions
            .iter()
            .any(|p| p == "android.permission.REQUEST_INSTALL_PACKAGES")
        {
            return Err(Exception::security(
                "Need to declare android.permission.REQUEST_INSTALL_PACKAGES to call this api",
            ));
        }
        self.install_disabled(name, uid, user)
            .map(|disabled| !disabled)
    }
    fn protected_for_caller(
        &self,
        q: &Query<'_>,
        name: Option<String>,
        user: i32,
    ) -> Result<bool, Exception> {
        if user < 0 {
            return Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            )));
        }
        if q.calling_uid == 2000 && self.shell_restricted(user)? {
            return Err(Exception::security(format!(
                "Shell does not have permission to access user {user}"
            )));
        }
        q.internal_enforce_cross_user(q.calling_uid, user, false, false, "isPackageStateProtected")
            .map_err(unmodelled)??;
        if !matches!(app_id(q.calling_uid), 0 | 1000)
            && !q
                .uid_has_permission(q.calling_uid, "android.permission.MANAGE_DEVICE_ADMINS")
                .map_err(unmodelled)?
        {
            return Err(Exception::security(
                "Caller must have the android.permission.MANAGE_DEVICE_ADMINS permission.",
            ));
        }
        self.protected(name, user, false)
    }
    fn admin_for_caller(&self, q: &Query<'_>, name: Option<String>) -> Result<bool, Exception> {
        if !q
            .uid_has_permission(q.calling_uid, "android.permission.MANAGE_USERS")
            .map_err(unmodelled)?
        {
            self.report_admin_permission_denied()?;
            return Err(Exception::security(
                "android.permission.MANAGE_USERS permission is required to call this API",
            ));
        }
        if q.internal_instant_package_name(q.calling_uid)
            .map_err(unmodelled)??
            .is_some()
            && !q
                .internal_caller_same_app(name.as_deref(), q.calling_uid, false)
                .map_err(unmodelled)??
        {
            return Ok(false);
        }
        let exists = name
            .as_deref()
            .is_some_and(|name| q.state.packages.contains_key(name));
        self.device_admin(name, exists)
    }
    /// Route before model-only fallback. Decode the complete original payload before any change.
    pub fn answer(&self, q: &Query<'_>, call: &mut Call<'_>) -> Option<Reply> {
        let result = (|| -> ParcelResult<Parcel> {
            let mut p = Parcel::new();
            match call.code {
                pm::IS_STORAGE_LOW => {
                    pm::IsStorageLow::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.is_storage_low() {
                        Ok(value) => pm::write_is_storage_low_reply(&mut p, value),
                        Err(error) => p.write_exception(&error),
                    }
                }

                pm::GET_INSTALL_LOCATION => {
                    pm::GetInstallLocation::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.get_install_location() {
                        Ok(value) => pm::write_get_install_location_reply(&mut p, value),
                        Err(error) => p.write_exception(&error),
                    }
                }
                pm::SET_INSTALL_LOCATION => {
                    let a = pm::SetInstallLocation::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.set_install_location(call.sender_pid, call.sender_euid as i32, a.loc)
                    {
                        Ok(value) => pm::write_set_install_location_reply(&mut p, value),
                        Err(error) => p.write_exception(&error),
                    }
                }
                pm::CAN_REQUEST_PACKAGE_INSTALLS => {
                    let a = pm::CanRequestPackageInstalls::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.request_installs(q, a.package_name, a.user_id) {
                        Ok(value) => pm::write_can_request_package_installs_reply(&mut p, value),
                        Err(error) => p.write_exception(&error),
                    }
                }
                pm::IS_PACKAGE_STATE_PROTECTED => {
                    let a = pm::IsPackageStateProtected::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.protected_for_caller(q, a.package_name, a.user_id) {
                        Ok(value) => pm::write_is_package_state_protected_reply(&mut p, value),
                        Err(error) => p.write_exception(&error),
                    }
                }
                pm::IS_PACKAGE_DEVICE_ADMIN_ON_ANY_USER => {
                    let a = pm::IsPackageDeviceAdminOnAnyUser::read(&mut call.data)?;
                    complete(&call.data)?;
                    match self.admin_for_caller(q, a.package_name) {
                        Ok(value) => {
                            pm::write_is_package_device_admin_on_any_user_reply(&mut p, value)
                        }
                        Err(error) => p.write_exception(&error),
                    }
                }
                _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
            }
            Ok(p)
        })();
        if matches!(
            call.code,
            pm::IS_STORAGE_LOW
                | pm::GET_INSTALL_LOCATION
                | pm::SET_INSTALL_LOCATION
                | pm::CAN_REQUEST_PACKAGE_INSTALLS
                | pm::IS_PACKAGE_STATE_PROTECTED
                | pm::IS_PACKAGE_DEVICE_ADMIN_ON_ANY_USER
        ) {
            Some(result)
        } else {
            None
        }
    }
}
fn unmodelled(error: super::apps_filter::NotModelled) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, error.0)
}
fn complete(reader: &Reader<'_>) -> ParcelResult<()> {
    if reader.remaining() == 0 {
        Ok(())
    } else {
        Err(BAD_VALUE)
    }
}
