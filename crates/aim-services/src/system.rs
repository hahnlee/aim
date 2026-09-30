//! The original system services a native one consults, as their client:
//! found through servicemanager, called with the pinned AIDL's codes.
//!
//! These are the binder equivalents of what the original service asks
//! other services in its process; where the original uses a
//! system_server-internal API that has none, the method says what it
//! stands in for. What a decision reads on every call (package ownership,
//! app-op modes, focus, the default input method, whether the device is
//! locked) is mirrored ([`crate::mirror`]), each fed by its owner's
//! listener; noting an app op, which only records the access, is sent in
//! the background. Permissions are asked each time: shell permission
//! delegation (`adoptShellPermissionIdentity`) changes them without a
//! notification.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{
    ReadParcelable, Returned, WriteParcelable, android_app_iactivitymanager as am,
    android_app_iactivitytaskmanager as atm, android_app_itaskstacklistener as task_listener,
    android_app_trust_itrustmanager as trust, android_content_icontentservice as content,
    android_content_pm_ipackagemanager as package, android_database_icontentobserver as observer,
    android_os_iremotecallback as remote_callback, android_os_iservicemanager as sm,
    android_os_iusermanager as um, android_permission_ipermissionmanager as pm,
    com_android_internal_app_iappopscallback as ops_callback,
    com_android_internal_app_iappopsservice as appops,
    com_android_internal_policy_idevicelockedstatelistener as lock_listener,
    com_android_internal_view_iinputmethodmanager as imm,
};

use crate::mirror::Mirror;

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
/// `Build.VERSION.SDK_INT` of the pinned image, as system_server registers.
const SDK_INT: i32 = 36;
/// `Settings.Secure.getUriFor(DEFAULT_INPUT_METHOD)`.
const DEFAULT_INPUT_METHOD_URI: &str = "content://settings/secure/default_input_method";

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
    /// The focused root task's effective uid.
    focus: Mirror<(), Option<i32>>,
    /// The package of each user's input method.
    ime: Mirror<i32, Option<String>>,
    /// Whether the system user's device is locked.
    locked: Mirror<(), bool>,
    notes: Sender<Note>,
}

/// The listener nodes the mirrors are fed by.
struct Listeners {
    task_stack: Binder,
    /// One per watched op: a callback is registered for one op.
    ops: Vec<(i32, Binder)>,
    packages: Binder,
    ime: Binder,
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
                // A package added, removed or changed: its uid and modes
                // with it.
                packages: node(remote_callback::DESCRIPTOR, false, |s| {
                    s.packages.invalidate();
                    s.modes.invalidate();
                }),
                ime: node(observer::DESCRIPTOR, false, |s| s.ime.invalidate()),
                locked: node(lock_listener::DESCRIPTOR, false, |s| s.locked.invalidate()),
            };
            Self {
                process: process.clone(),
                services: Mutex::new(HashMap::new()),
                listeners,
                packages: Mirror::new(),
                modes: Mirror::new(),
                focus: Mirror::new(),
                ime: Mirror::new(),
                locked: Mirror::new(),
                notes,
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

    /// Tells the package and mode mirrors of every package change
    /// (`PackageMonitor`'s callback).
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
        let mode = self.modes.get(
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
            || {
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
            },
        )?;
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

    /// The package of `user`'s current input method, which stands in for
    /// `Settings.Secure.DEFAULT_INPUT_METHOD` (the input method service
    /// sets the setting when it switches, and follows it when another
    /// writes it); the setting's observer tells of a change.
    pub fn default_ime_package(self: &Arc<Self>, user_id: i32) -> Result<Option<String>> {
        self.ime.get(
            user_id,
            || {
                let observer = self.listeners.ime;
                self.watch(
                    "content",
                    content::REGISTER_CONTENT_OBSERVER,
                    |p| {
                        content::RegisterContentObserver {
                            uri: Some(Uri(DEFAULT_INPUT_METHOD_URI)),
                            notify_for_descendants: false,
                            observer: Some(observer),
                            user_handle: USER_ALL,
                            target_sdk_version: SDK_INT,
                        }
                        .write(p)
                    },
                    content::read_register_content_observer_reply,
                    |s| s.ime.unwatch(),
                )
            },
            || {
                let args = imm::GetCurrentInputMethodInfoAsUser { user_id };
                let info = self.call(
                    "input_method",
                    imm::GET_CURRENT_INPUT_METHOD_INFO_AS_USER,
                    |p| args.write(p),
                    imm::read_get_current_input_method_info_as_user_reply::<InputMethodInfo>,
                )?;
                Ok(info.and_then(|i| i.id?.split('/').next().map(str::to_string)))
            },
        )
    }
}

/// `SyncNotedAppOp`, not read: the note's mode is the one already decided.
struct SyncNotedAppOp;

impl ReadParcelable for SyncNotedAppOp {
    fn read_from(_: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self)
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

/// `Uri`, as a `StringUri` writes itself.
struct Uri(&'static str);

impl WriteParcelable for Uri {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1); // StringUri.TYPE_ID
        p.write_string8(Some(self.0));
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
