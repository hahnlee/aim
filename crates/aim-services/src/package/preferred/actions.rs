//! Concrete live preferred policy/effect owners. Every external operation uses
//! the retained original bridge with current-bootstrap guards on both sides.
use super::{
    Mutation, PreferredActivity, Selection,
    registry::{ActionError, Actions},
};
use crate::package::{
    apps_filter::NotModelled,
    bootstrap::{Bridge, OwnerError},
    query::preferred::Owner,
};
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};

pub struct SystemActions {
    system: Weak<crate::system::System>,
    bridge: Arc<Bridge>,
    image: PathBuf,
}
impl SystemActions {
    pub fn new(
        system: &Arc<crate::system::System>,
        bridge: Arc<Bridge>,
        image: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            system: Arc::downgrade(system),
            bridge,
            image,
        })
    }
    fn system(&self) -> Result<Arc<crate::system::System>, ActionError> {
        let system = self
            .system
            .upgrade()
            .ok_or(ActionError::Unavailable(NotModelled(
                "preferred system owner stopped",
            )))?;
        system
            .check_package_bootstrap(&self.bridge)
            .map_err(ActionError::Exception)?;
        Ok(system)
    }
    fn leaf<T>(
        &self,
        call: impl FnOnce(&Bridge) -> Result<T, OwnerError>,
    ) -> Result<T, ActionError> {
        let system = self.system()?;
        let result = call(&self.bridge).map_err(|error| match error {
            OwnerError::Owner(exception) => ActionError::Exception(exception),
            error => ActionError::Exception(aim_binder_host::parcel::Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("preferred independent owner: {error:?}"),
            )),
        })?;
        system
            .check_package_bootstrap(&self.bridge)
            .map_err(ActionError::Exception)?;
        Ok(result)
    }
    fn changed(&self, user: i32) -> Result<(), ActionError> {
        self.leaf(|bridge| bridge.preferred_changed(user))
    }
    fn update_home(&self, user: i32, uid: i32) -> Result<bool, ActionError> {
        let system = self.system()?;
        let capture = system
            .capture_package_queries()
            .map_err(ActionError::Exception)?;
        let lifecycle =
            capture
                .state()
                .system
                .lifecycle
                .as_ref()
                .ok_or(ActionError::Unavailable(NotModelled(
                    "preferred systemReady owner unavailable",
                )))?;
        if !lifecycle.is_system_ready() {
            return Ok(false);
        }
        let handle =
            capture
                .state()
                .system
                .preferred_owner
                .as_ref()
                .ok_or(ActionError::Unavailable(NotModelled(
                    "preferred native registry unavailable",
                )))?;
        let Some(snapshot) = handle.snapshot(user)? else {
            return Ok(false);
        };
        let resolution = crate::package::resolve::Resolver::default()
            .resolution(capture.state())
            .map_err(|_| {
                ActionError::Unavailable(NotModelled(
                    "preferred native component resolver unavailable",
                ))
            })?;
        let intent = crate::package::intent::Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec![
                "android.intent.category.HOME".into(),
                "android.intent.category.DEFAULT".into(),
            ]),
            ..Default::default()
        };
        let flags = crate::package::info::flags::MATCH_DIRECT_BOOT_AWARE
            | crate::package::info::flags::MATCH_DIRECT_BOOT_UNAWARE;
        let candidates = resolution
            .query_intent_activities(&intent, None, flags, user, uid)
            .map_err(|_| {
                ActionError::Unavailable(NotModelled("preferred home candidate owner unavailable"))
            })?;
        let plan = resolution
            .plan_preferred_from_candidates(
                &intent,
                None,
                0,
                user,
                uid,
                snapshot.state.as_ref(),
                false,
                true,
                false,
                candidates,
            )
            .map_err(|_| {
                ActionError::Unavailable(NotModelled("preferred home selection owner unavailable"))
            })?;
        if !plan.selection.edits.is_empty() {
            system
                .commit_package_preferred_selection(
                    &self.bridge,
                    &system
                        .package_preferred_registry(&self.bridge)
                        .map_err(ActionError::Exception)?,
                    user,
                    snapshot.generation,
                    &plan.selection,
                )
                .map_err(ActionError::Exception)?;
        }
        let package = plan
            .chosen
            .as_ref()
            .map(|chosen| chosen.component().0.to_owned());
        let current = self.default_home(user)?;
        if current == package {
            return Ok(false);
        }
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let permission_controller = capture
            .state()
            .system
            .permission_controller_package
            .as_ref()
            .ok_or(ActionError::Unavailable(NotModelled(
                "preferred permission-controller owner unavailable",
            )))?;
        if let Some(controller) = permission_controller {
            if query
                .packages_for_uid(uid)?
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name.as_ref() == Some(controller)))
            {
                return Ok(false);
            }
        }
        let Some(package) = package else {
            return Ok(false);
        };
        self.leaf(|bridge| {
            bridge.preferred_set_role("android.app.role.HOME", &package, user, true)
        })?;
        Ok(true)
    }
    fn effects(
        &self,
        user: i32,
        effects: &super::MutationEffects,
        calling_uid: i32,
    ) -> Result<(), ActionError> {
        // Reset follows native preferred defaults, domain, permission, home,
        // then network owners, matching PreferredActivityHelper's source order.
        if effects.broadcast_before_reset && effects.preferred_changed_broadcast {
            self.changed(user)?;
        }
        if effects.reset_domain_user {
            self.system()?
                .reset_package_preferred_domains(&self.bridge, user)
                .map_err(ActionError::Exception)?;
        }
        if effects.reset_runtime_permissions {
            self.leaf(|bridge| bridge.preferred_reset_permissions(user))?;
        }
        let home_sent = if effects.update_home {
            self.update_home(user, calling_uid)?
        } else {
            false
        };
        if effects.preferred_changed_broadcast
            && !effects.broadcast_before_reset
            && (!effects.broadcast_if_home_unsent || !home_sent)
        {
            self.changed(user)?;
        }
        if effects.reset_network_policies {
            self.leaf(|bridge| bridge.preferred_reset_network(user))?;
        }
        Ok(())
    }
}
impl Actions for SystemActions {
    fn context_permission(&self, calling_uid: i32, name: &str) -> Result<bool, ActionError> {
        use aim_service_aidl::android_app_iactivitymanager as activity;
        let system = self.system()?;
        let request = context_permission_request(calling_uid, name,
            system.process().authenticated_inbound_identity())?;
        let granted = system.call("activity", activity::CHECK_PERMISSION,
            |parcel| request.write(parcel), activity::read_check_permission_reply)
            .map_err(ActionError::Exception)? == 0;
        system.check_package_bootstrap(&self.bridge).map_err(ActionError::Exception)?;
        Ok(granted)
    }

    fn commit_after_selection(
        &self,
        user: i32,
        mutation: &Mutation,
        calling_uid: i32,
    ) -> Result<bool, ActionError> {
        let system = self.system()?;
        let registry = system
            .package_preferred_registry(&self.bridge)
            .map_err(ActionError::Exception)?;
        let generation = registry
            .snapshot(user)?
            .ok_or(ActionError::Unavailable(NotModelled(
                "preferred user registry unavailable",
            )))?
            .generation;
        self.commit_mutation(user, generation, mutation, calling_uid)
    }
    fn commit_mutation(
        &self,
        user: i32,
        generation: u64,
        mutation: &Mutation,
        calling_uid: i32,
    ) -> Result<bool, ActionError> {
        let system = self.system()?;
        let registry = system
            .package_preferred_registry(&self.bridge)
            .map_err(ActionError::Exception)?;
        let prepared = registry
            .prepare_mutation(user, generation, mutation)
            .map_err(|error| {
                ActionError::Exception(aim_binder_host::parcel::Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    error,
                ))
            })?;
        if let Some(stage) = prepared.stage {
            system
                .commit_package_preferred_stage(
                    &self.bridge,
                    &registry,
                    stage,
                    matches!(mutation, Mutation::CrossRemove { .. }) && !prepared.changed,
                )
                .map_err(ActionError::Exception)?;
        }
        self.effects(
            user,
            &prepared.effects,
            if matches!(mutation, Mutation::Reset { .. }) {
                1000
            } else {
                calling_uid
            },
        )?;
        Ok(prepared.changed)
    }
    fn cross_access(
        &self,
        calling_uid: i32,
        source: i32,
        target: i32,
        adding: bool,
    ) -> Result<i32, ActionError> {
        let access = self.leaf(|bridge| bridge.preferred_cross_access_control(source, target))?;
        let privileged = matches!(calling_uid, 0 | 1000);
        if access == 10 && !privileged || access == 20 && (!adding || !privileged) {
            return Err(ActionError::Exception(
                aim_binder_host::parcel::Exception::security(format!(
                    "CrossProfileIntentFilter cannot be accessed by user {calling_uid}"
                )),
            ));
        }
        Ok(access)
    }
    fn cross_accessible(
        &self,
        calling_uid: i32,
        source: i32,
        target: i32,
    ) -> Result<bool, ActionError> {
        let access = self.leaf(|bridge| bridge.preferred_cross_access_control(source, target))?;
        Ok(access != 20 && (access != 10 || matches!(calling_uid, 0 | 1000)))
    }
    fn enforce_shell_restriction(&self, calling_uid: i32, user: i32) -> Result<(), ActionError> {
        if calling_uid == 2000 && self.leaf(|bridge| bridge.shell_debugging_restricted(user))? {
            return Err(ActionError::Exception(
                aim_binder_host::parcel::Exception::security(format!(
                    "Shell does not have permission to access user {user}"
                )),
            ));
        }
        Ok(())
    }
    fn reconcile_home(&self, user: i32, calling_uid: i32) -> Result<bool, ActionError> {
        self.update_home(user, calling_uid)
    }
    fn commit_home_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &Selection,
    ) -> Result<(), ActionError> {
        let system = self.system()?;
        let registry = system
            .package_preferred_registry(&self.bridge)
            .map_err(ActionError::Exception)?;
        if let Some(stage) = registry
            .prepare_selection(user, generation, selection)
            .map_err(|error| {
                ActionError::Exception(aim_binder_host::parcel::Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    error,
                ))
            })?
        {
            system
                .commit_package_preferred_stage(&self.bridge, &registry, stage, true)
                .map_err(ActionError::Exception)?;
        }
        Ok(())
    }
    fn default_home(&self, user: i32) -> Result<Option<String>, ActionError> {
        self.leaf(|bridge| bridge.preferred_role_holder("android.app.role.HOME", user))
    }
    fn default_browser(&self, user: i32) -> Result<Option<String>, ActionError> {
        self.leaf(|bridge| bridge.preferred_role_holder("android.app.role.BROWSER", user))
    }
    fn restore_browser(
        &self,
        user: i32,
        package: &str,
        installed: bool,
    ) -> Result<(), ActionError> {
        if installed {
            self.leaf(|bridge| {
                bridge.preferred_set_role("android.app.role.BROWSER", package, user, false)
            })
        } else {
            self.system()?
                .set_package_preferred_pending_browser(&self.bridge, user, Some(package))
                .map_err(ActionError::Exception)
        }
    }
    fn default_preferences(&self, user: i32) -> Result<Vec<PreferredActivity>, ActionError> {
        let system = self.system()?;
        let capture = system
            .capture_package_queries()
            .map_err(ActionError::Exception)?;
        let resolution = crate::package::resolve::Resolver::default()
            .resolution(capture.state())
            .map_err(|_| {
                ActionError::Unavailable(NotModelled(
                    "default preferred component resolver unavailable",
                ))
            })?;
        let order = capture
            .scan()
            .owner()
            .settings
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect::<Vec<_>>();
        super::defaults::collect(capture.state(), &resolution, &self.image, user, &order).map_err(
            |error| {
                ActionError::Exception(aim_binder_host::parcel::Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    error,
                ))
            },
        )
    }
}

/// Carry the actual inbound caller to the original independent permission
/// owner. Rechecking each call observes adoption and drop without grant caches.
fn context_permission_request(
    uid: i32, name: &str, identity: Option<(i32, u32)>,
) -> Result<aim_service_aidl::android_app_iactivitymanager::CheckPermission, ActionError> {
    let (pid, actual_uid) = identity.ok_or_else(|| ActionError::Exception(
        aim_binder_host::parcel::Exception::security("Preferred permission check outside authenticated inbound call")))?;
    if i32::try_from(actual_uid).ok() != Some(uid) {
        return Err(ActionError::Exception(aim_binder_host::parcel::Exception::security(
            "Preferred permission caller differs from authenticated inbound identity")));
    }
    Ok(aim_service_aidl::android_app_iactivitymanager::CheckPermission {
        permission: Some(name.into()), pid, uid,
    })
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    #[test]
    fn actual_binder_permission_receiver_observes_adoption_and_drop_per_call() {
        use aim_binder_host::{local::{Call, LocalProcess, Reply, Service}, parcel::{Parcel, Binder, UNKNOWN_TRANSACTION}};
        use aim_binder_driver::{Credentials, Device, Driver};
        use aim_service_aidl::android_app_iactivitymanager as activity;
        use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
        struct PermissionReceiver { adopted: AtomicBool, callers: Mutex<Vec<(i32,i32)>> }
        impl Service for PermissionReceiver {
            fn descriptor(&self) -> &str { activity::DESCRIPTOR }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                if call.code != activity::CHECK_PERMISSION { return Err(UNKNOWN_TRANSACTION); }
                let request = activity::CheckPermission::read(&mut call.data)?;
                assert_eq!(call.data.remaining(), 0);
                assert_eq!(request.permission.as_deref(), Some("android.permission.SET_PREFERRED_APPLICATIONS"));
                assert_eq!((request.pid, request.uid), (717, 10226));
                self.callers.lock().unwrap().push((request.pid,request.uid));
                let mut reply = Parcel::new();
                activity::write_check_permission_reply(&mut reply,
                    if self.adopted.load(Ordering::SeqCst) { 0 } else { -1 });
                Ok(reply)
            }
        }
        let driver = Driver::new();
        let process = LocalProcess::open(&driver, Device::Binder, Credentials {
            pid: 500, euid: 1000, security_context: None,
        });
        let receiver = Arc::new(PermissionReceiver { adopted: AtomicBool::new(false), callers: Mutex::new(Vec::new()) });
        let Binder::Local(ptr) = process.add_service(receiver.clone()) else { unreachable!() };
        let service = process.local_service(ptr).unwrap();
        for adopted in [false, true, false] {
            receiver.adopted.store(adopted,Ordering::SeqCst);
            let request = context_permission_request(10226, "android.permission.SET_PREFERRED_APPLICATIONS", Some((717,10226))).unwrap();
            let mut parcel = Parcel::new(); request.write(&mut parcel);
            let response = service.transact(activity::CHECK_PERMISSION,&parcel,false).unwrap();
            assert_eq!(activity::read_check_permission_reply(&mut response.reader()).unwrap().unwrap(), if adopted {0}else{-1});
        }
        assert_eq!(receiver.callers.lock().unwrap().as_slice(), &[(717,10226);3]);
    }

    #[test]
    fn preferred_context_permission_uses_authenticated_pid_uid_not_package_grants() {
        use aim_service_aidl::android_app_iactivitymanager as activity;
        use aim_binder_host::parcel::Parcel;
        let request = context_permission_request(10226, "android.permission.SET_PREFERRED_APPLICATIONS", Some((717, 10226))).unwrap();
        let mut parcel = Parcel::new(); request.write(&mut parcel);
        let mut reader = parcel.reader();
        let decoded = activity::CheckPermission::read(&mut reader).unwrap();
        assert_eq!(decoded.pid, 717); assert_eq!(decoded.uid, 10226);
        assert_eq!(decoded.permission.as_deref(), Some("android.permission.SET_PREFERRED_APPLICATIONS"));
        assert_eq!(reader.remaining(), 0);
        assert!(context_permission_request(2000, "android.permission.SET_PREFERRED_APPLICATIONS", Some((717, 10226))).is_err());
        assert!(context_permission_request(10226, "android.permission.SET_PREFERRED_APPLICATIONS", None).is_err());
    }
}
