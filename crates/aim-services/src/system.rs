//! The original system services a native one consults, as their client:
//! found through servicemanager, called with the pinned AIDL's codes.
//!
//! These are the binder equivalents of what the original service asks
//! other services in its process; where the original uses a
//! system_server-internal API that has none, the method says what it
//! stands in for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{
    ReadParcelable, Returned, android_app_iactivitymanager as am,
    android_app_iactivitytaskmanager as atm, android_app_trust_itrustmanager as trust,
    android_os_iservicemanager as sm, android_os_iusermanager as um,
    android_permission_ipermissionmanager as pm, com_android_internal_app_iappopsservice as appops,
    com_android_internal_view_iinputmethodmanager as imm,
};

/// `PackageManager.PERMISSION_GRANTED`.
const PERMISSION_GRANTED: i32 = 0;
/// `VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT`.
const PERSISTENT_DEVICE_ID_DEFAULT: &str = "default:0";
/// `AppOpsManager.MODE_*`.
pub const MODE_ALLOWED: i32 = 0;
pub const MODE_ERRORED: i32 = 2;

/// A failure to reach a service, as the exception a Java caller would
/// see once system_server rethrew it.
fn unreachable_service(what: &str, status: i32) -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        format!("{what}: binder status {status}"),
    )
}

pub struct System {
    process: Arc<LocalProcess>,
    services: Mutex<HashMap<&'static str, Arc<Strong>>>,
}

type Result<T> = std::result::Result<T, Exception>;

impl System {
    pub fn new(process: Arc<LocalProcess>) -> Arc<Self> {
        Arc::new(Self {
            process,
            services: Mutex::new(HashMap::new()),
        })
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

    /// `PackageManager.checkPermission(permission, package)` in
    /// system_server's context (user 0, the default device).
    pub fn package_has_permission(
        self: &Arc<Self>,
        permission: &str,
        package: &str,
    ) -> Result<bool> {
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
    }

    /// `AppOpsManager.checkPackage`: throws unless `package` is `uid`'s.
    pub fn check_package(self: &Arc<Self>, uid: i32, package: &str) -> Result<()> {
        let args = appops::CheckPackage {
            uid,
            package_name: Some(package.into()),
        };
        let mode = self.call(
            "appops",
            appops::CHECK_PACKAGE,
            |p| args.write(p),
            appops::read_check_package_reply,
        )?;
        if mode != MODE_ALLOWED {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(())
    }

    /// `AppOpsManager.noteOp` (`note`) or `checkOp`, on the default device;
    /// both throw on `MODE_ERRORED`.
    pub fn app_op(
        self: &Arc<Self>,
        note: bool,
        op: i32,
        uid: i32,
        package: &str,
        attribution_tag: Option<&str>,
    ) -> Result<i32> {
        let mode = if note {
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
            )?
            .map_or(MODE_ERRORED, |op| op.mode)
        } else {
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
            )?
        };
        if mode == MODE_ERRORED {
            return Err(Exception::security(format!(
                "uid {uid} does not have app op {op} for {package}"
            )));
        }
        Ok(mode)
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
    pub fn device_locked(self: &Arc<Self>, user_id: i32, device_id: i32) -> Result<bool> {
        let args = trust::IsDeviceLocked { user_id, device_id };
        self.call(
            "trust",
            trust::IS_DEVICE_LOCKED,
            |p| args.write(p),
            trust::read_is_device_locked_reply,
        )
    }

    /// Stands in for `WindowManagerInternal.isUidFocused`, which has no
    /// binder form: whether `uid` owns the focused root task (#430).
    pub fn uid_focused(self: &Arc<Self>, uid: i32) -> Result<bool> {
        let focused = self.call(
            "activity_task",
            atm::GET_FOCUSED_ROOT_TASK_INFO,
            |p| atm::GetFocusedRootTaskInfo {}.write(p),
            atm::read_get_focused_root_task_info_reply::<RootTaskInfo>,
        )?;
        Ok(focused.is_some_and(|task| task.effective_uid == uid))
    }

    /// The package of `user`'s current input method, which stands in for
    /// `Settings.Secure.DEFAULT_INPUT_METHOD` (the input method service
    /// keeps the setting and its current method the same).
    pub fn default_ime_package(self: &Arc<Self>, user_id: i32) -> Result<Option<String>> {
        let args = imm::GetCurrentInputMethodInfoAsUser { user_id };
        let info = self.call(
            "input_method",
            imm::GET_CURRENT_INPUT_METHOD_INFO_AS_USER,
            |p| args.write(p),
            imm::read_get_current_input_method_info_as_user_reply::<InputMethodInfo>,
        )?;
        Ok(info.and_then(|i| i.id?.split('/').next().map(str::to_string)))
    }
}

/// `SyncNotedAppOp`: its mode.
struct SyncNotedAppOp {
    mode: i32,
}

impl ReadParcelable for SyncNotedAppOp {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let flags = r.read_i32()?;
        let mode = r.read_i32()?;
        r.read_i32()?; // op
        if flags & 0x4 != 0 {
            r.read_string16()?;
        }
        if flags & 0x8 != 0 {
            r.read_string16()?;
        }
        Ok(Self { mode })
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

/// `InputMethodInfo`: its id (`package/class`).
struct InputMethodInfo {
    id: Option<String>,
}

impl ReadParcelable for InputMethodInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            id: r.read_string16()?,
        })
    }
}
