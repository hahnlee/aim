//! The service's listeners on the owners of what its decisions read, as
//! the original's helpers register them when the system is ready: app
//! foreground state (`AppForegroundHelper`, a uid observer cut at
//! `IMPORTANCE_FOREGROUND_SERVICE`), runtime permissions
//! (`LocationPermissionsHelper`) and the coarse location app op
//! (`SystemAppOpsHelper`).

use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    android_app_iactivitymanager as am, android_app_iuidobserver as uid_observer,
    android_permission_ionpermissionschangelistener as permissions_listener,
    android_permission_ipermissionmanager as pm,
    com_android_internal_app_iappopscallback as ops_callback,
    com_android_internal_app_iappopsservice as appops,
};

use super::LocationManagerService;
use super::env::is_foreground;

/// `ActivityManager.UID_OBSERVER_PROCSTATE | UID_OBSERVER_GONE`, and the
/// process state `IMPORTANCE_FOREGROUND_SERVICE` maps to.
const UID_OBSERVER_PROCSTATE_AND_GONE: i32 = 1 | 2;
const PROCESS_STATE_FOREGROUND_SERVICE: i32 = 4;
/// `AppOpsManager.OP_COARSE_LOCATION`, `WATCH_FOREGROUND_CHANGES`.
const OP_COARSE_LOCATION: i32 = 0;
const WATCH_FOREGROUND_CHANGES: i32 = 1;

/// Registers the listeners; false if an owner could not be reached.
pub fn register(service: &Arc<LocationManagerService>) -> bool {
    let env = &service.env;
    let node = |kind| {
        env.process.add_service(Arc::new(Listener {
            kind,
            service: Arc::downgrade(service),
        }))
    };
    let observer = node(Kind::Uid);
    let permissions = node(Kind::Permissions);
    let ops = node(Kind::AppOp);
    let uid = am::RegisterUidObserver {
        observer: Some(observer),
        which: UID_OBSERVER_PROCSTATE_AND_GONE,
        cutpoint: PROCESS_STATE_FOREGROUND_SERVICE,
        calling_package: Some("android".into()),
    };
    let listener = pm::AddOnPermissionsChangeListener {
        listener: Some(permissions),
    };
    let watch = appops::StartWatchingModeWithFlags {
        op: OP_COARSE_LOCATION,
        package_name: None,
        flags: WATCH_FOREGROUND_CHANGES,
        callback: Some(ops),
    };
    let results = [
        env.system.call(
            "activity",
            am::REGISTER_UID_OBSERVER,
            |p| uid.write(p),
            am::read_register_uid_observer_reply,
        ),
        env.system.call(
            "permissionmgr",
            pm::ADD_ON_PERMISSIONS_CHANGE_LISTENER,
            |p| listener.write(p),
            pm::read_add_on_permissions_change_listener_reply,
        ),
        env.system.call(
            "appops",
            appops::START_WATCHING_MODE_WITH_FLAGS,
            |p| watch.write(p),
            appops::read_start_watching_mode_with_flags_reply,
        ),
    ];
    results.iter().all(|r| match r {
        Ok(()) => true,
        Err(e) => {
            eprintln!("location: cannot watch: {}", e.message);
            false
        }
    })
}

#[derive(Clone, Copy)]
enum Kind {
    Uid,
    Permissions,
    AppOp,
}

struct Listener {
    kind: Kind,
    service: Weak<LocationManagerService>,
}

impl Service for Listener {
    fn descriptor(&self) -> &str {
        match self.kind {
            Kind::Uid => uid_observer::DESCRIPTOR,
            Kind::Permissions => permissions_listener::DESCRIPTOR,
            Kind::AppOp => ops_callback::DESCRIPTOR,
        }
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(service) = self.service.upgrade() else {
            return Ok(Parcel::new());
        };
        let r = &mut call.data;
        match (self.kind, call.code) {
            (Kind::Uid, uid_observer::ON_UID_STATE_CHANGED) => {
                let a = uid_observer::OnUidStateChanged::read(r)?;
                service.on_foreground_changed(a.uid, is_foreground(a.proc_state));
            }
            (Kind::Uid, uid_observer::ON_UID_GONE) => {
                let a = uid_observer::OnUidGone::read(r)?;
                service.on_foreground_changed(a.uid, false);
            }
            (Kind::Uid, _) => {}
            (Kind::Permissions, permissions_listener::ON_PERMISSIONS_CHANGED) => {
                let a = permissions_listener::OnPermissionsChanged::read(r)?;
                service.on_permissions_changed(Some(a.uid), None);
            }
            (Kind::AppOp, ops_callback::OP_CHANGED) => {
                let a = ops_callback::OpChanged::read(r)?;
                service.on_permissions_changed(None, a.package_name);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(Parcel::new())
    }
}
