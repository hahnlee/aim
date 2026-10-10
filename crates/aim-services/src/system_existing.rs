//! Native existing-code admission and exclusive user-state publication.
use super::System;
use crate::package::{
    apps_filter,
    installer::{
        existing::{Owner, Request},
        policy,
    },
    resolve::Resolver,
};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use std::sync::Arc;

fn error(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
impl System {
    pub fn new_existing_package_owner(
        self: &Arc<Self>,
        bridge: Arc<crate::package::bootstrap::Bridge>,
        effects: Arc<crate::package::effects::Owner>,
        changes: Arc<crate::package::changes::Owner>,
        restores: Arc<crate::package::installer::existing::Restores>,
    ) -> Result<Arc<Owner>, Exception> {
        self.check_package_bootstrap(&bridge)?;
        let owner = Owner::new(
            Arc::downgrade(self),
            bridge.clone(),
            effects,
            changes,
            restores,
        );
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
            .ok_or_else(|| error("existing installer bootstrap changed before registration"))?;
        if current.existing_installer.is_some() {
            return Err(error("existing installer already registered"));
        }
        current.existing_installer = Some(owner.clone());
        Ok(owner)
    }
    pub(crate) fn existing_package_owner(&self) -> Result<Arc<Owner>, Exception> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.existing_installer.clone())
            .ok_or_else(|| error("native existing installer unavailable"))
    }
    pub(crate) fn finish_existing_package_install(
        &self,
        uid: u32,
        token: i32,
        did_launch: bool,
    ) -> Result<bool, Exception> {
        if uid != 0 && uid != 1000 {
            return Err(Exception::security(
                "Only the system is allowed to finish installs",
            ));
        }
        let owner = self.existing_package_owner()?;
        self.check_package_bootstrap(&owner.bridge)?;
        owner.restores.finish(token, did_launch)
    }
    pub(crate) fn install_existing_package(
        &self,
        owner: &Owner,
        uid: i32,
        pid: i32,
        request: Request,
    ) -> Result<i32, Exception> {
        self.check_package_bootstrap(&owner.bridge)?;
        let before = self.capture_package_queries()?;
        let resolver = Resolver::default();
        let resolution = resolver
            .resolution(before.state())
            .map_err(|e| error(format!("existing install resolution: {e:?}")))?;
        let query = crate::package::query::Query {
            state: before.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        if !policy::permission(&query, "android.permission.INSTALL_PACKAGES")?
            && !policy::permission(&query, "android.permission.INSTALL_EXISTING_PACKAGES")?
        {
            return Err(Exception::security(format!(
                "Neither user {uid} nor current process has android.permission.INSTALL_PACKAGES."
            )));
        }
        query
            .internal_enforce_cross_user(
                uid,
                request.user,
                true,
                false,
                &format!("installExistingPackage for user {}", request.user),
            )
            .map_err(|e| error(e.0))??;
        let (_, user_policy) = owner
            .bridge
            .existing_install_user_policy(request.user)
            .map_err(|e| error(format!("existing install user policy: {e:?}")))?;
        if uid == 2000 && user_policy.disallow_debugging_features {
            return Err(Exception::security(
                "Shell does not have permission to access user",
            ));
        }
        if user_policy.disallow_install_apps {
            return owner.complete_immediate(request, -111);
        }
        let protected = owner
            .effects
            .state_protected_nullable(request.package.as_deref(), request.user)?;
        let admin = owner
            .effects
            .device_admin_nullable(request.package.as_deref(), request.user)?;
        let Some(name) = request.package.as_deref() else {
            return owner.complete_immediate(request, -3);
        };
        let name = name.to_owned();
        let (capture, persistence) = {
            let bootstrap = self.package_bootstrap.lock().unwrap();
            let current = bootstrap
                .current
                .as_ref()
                .filter(|c| Arc::ptr_eq(&c.bridge, &owner.bridge))
                .ok_or_else(|| error("existing install bootstrap replaced"))?;
            (
                current
                    .queries
                    .clone()
                    .ok_or_else(|| error("existing install capture unavailable"))?,
                current
                    .persistence
                    .clone()
                    .ok_or_else(|| error("existing install disk owner unavailable"))?,
            )
        };
        let mut disk = persistence.lock().unwrap();
        let mut bootstrap = self.package_bootstrap.lock().unwrap();
        let current = bootstrap
            .current
            .as_mut()
            .filter(|c| {
                Arc::ptr_eq(&c.bridge, &owner.bridge)
                    && c.queries.as_ref().is_some_and(|c| Arc::ptr_eq(c, &capture))
            })
            .ok_or_else(|| error("existing install generation changed"))?;
        disk.validate_committed_scan(capture.scan().owner())
            .map_err(|e| error(e.to_string()))?;
        let current_resolution=resolver.resolution(capture.state()).map_err(|e|error(format!("existing install locked resolution: {e:?}")))?;
        let Some(package) = capture
            .state()
            .packages
            .get(&name)
            .filter(|p| p.pkg.is_some())
        else {
            drop(bootstrap); drop(disk); return owner.complete_immediate(request, -3);
        };
        let instant = request.flags & 0x800 != 0;
        let full = request.flags & 0x4000 != 0;
        if instant && (package.is.system || package.is.updated_system_app || admin || protected) {
            drop(bootstrap); drop(disk); return owner.complete_immediate(request, -3);
        }
        // The original clears Binder identity before canViewInstantApps;
        // its Context permission checks see the system, while same-app checks
        // keep the explicitly captured caller UID.
        let privileged=crate::package::query::Query{state:capture.state(),filter:&current_resolution.apps_filter,calling_uid:1000};
        let can_view=if uid<10000 || policy::permission(&privileged,"android.permission.ACCESS_INSTANT_APPS")? {true}
            else if policy::permission(&privileged,"android.permission.VIEW_INSTANT_APPS")? {
                let home=crate::package::query::preferred::default_home_for_instant(&privileged,apps_filter::user_id(uid)).map_err(|e|error(e.0))?;
                let home_matches=apps_filter::is_caller_same_app(capture.state(),home.as_ref().map(|home|home.package.as_str()),uid).map_err(|e|error(e.0))?;
                if home_matches {true} else {
                    let prediction=capture.state().system.roles.as_ref().ok_or_else(||error("existing install prediction owner absent"))?
                        .package(crate::package::roles::Role::AppPrediction,&privileged)?;
                    apps_filter::is_caller_same_app(capture.state(),prediction.as_deref(),uid).map_err(|e|error(e.0))?
                }
            } else {false};
        if !can_view && capture.state().users.keys().all(|user|package.users.get(user).is_some_and(|state|state.instant_app)) {
            drop(bootstrap); drop(disk); return owner.complete_immediate(request,-3);
        }
        let mut scan = capture.scan().owner().clone();
        let original_user_state = scan
            .scanned_user_states(&name)
            .ok_or_else(|| error("existing install sparse user owner absent"))?
            .get(&request.user)
            .cloned()
            .unwrap_or_default();
        let mut user_state = original_user_state.clone();
        let newly_installed = !user_state.installed;
        let changed = newly_installed || full && user_state.instant_app;
        if newly_installed {
            user_state.installed = true;
            user_state.hidden = false;
            user_state.install_reason = request.reason;
            user_state.uninstall_reason = 0;
            user_state.archive_state = None;
            user_state.first_install_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| error(e.to_string()))?
                .as_millis()
                .try_into()
                .map_err(|_| error("existing install timestamp overflow"))?;
        }
        if instant && !user_state.instant_app {
            user_state.instant_app = true;
        } else if full && user_state.instant_app {
            user_state.instant_app = false;
        }
        if !changed && user_state == original_user_state {
            drop(bootstrap);
            drop(disk);
            return owner.complete_immediate(request, 1);
        }
        scan.set_user_state(&name, request.user, user_state)
            .map_err(error)?;
        if changed {
            let keep_owner = apps_filter::is_caller_same_app(
                before.state(),
                package.install_source.update_owner.as_deref(),
                uid,
            )
            .map_err(|e| error(e.0))?
                || package.is.system && user_policy.organization_managed;
            if !keep_owner {
                scan.settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == name)
                    .ok_or_else(|| error("existing install setting absent"))?
                    .install_source
                    .update_owner = None;
            }
        }
        let update = capture
            .prepare_package_update(scan.clone())
            .map_err(error)?;
        let write = if changed {
            disk.commit_scan_settings(update.capture.scan()).and_then(|_| {
                disk.commit_live_install_restrictions(
                    &scan,
                    request.user as u32,
                    &[name.clone()].into(),
                    true,
                )
            })
        } else {
            Ok(())
        };
        let committed = write.is_ok() || write.as_ref().is_err_and(|e| e.committed);
        if committed {
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page {
                page.publish(update.capture.scan().version());
            }
            bootstrap.version = update.capture.scan().version();
        }
        drop(bootstrap);
        drop(disk);
        write.map_err(|e| error(format!("existing install persistence: {e}")))?;
        owner
            .bridge
            .invalidate_package_info_cache()
            .map_err(|e| error(format!("existing install committed; cache owner: {e:?}")))?;
        if !changed {
            return owner.complete_immediate(request, 1);
        }
        let installed = self.capture_package_queries()?;
        let mut record = Parcel::new();
        record.write_i32(1);
        record.write_i64(installed.scan().version() as i64);
        let setting =
            crate::package::scan_snapshot::setting_record::captured(installed.scan(), &name, false)
                .map_err(error)?
                .ok_or_else(|| error("existing install setting transport absent"))?;
        aim_service_aidl::write_byte_array(&mut record, Some(&setting));
        let code = crate::package::scan_snapshot::endpoint::PackageCode::captured(
            installed.scan(),
            &name,
            false,
        )
        .map_err(error)?
        .ok_or_else(|| error("existing install code transport absent"))?;
        let mut bytes = Parcel::new();
        code.write_to(&mut bytes);
        aim_service_aidl::write_byte_array(&mut record, Some(bytes.data()));
        let user_ids =
            crate::package::scan_snapshot::user_record::ids(installed.scan(), &name, false)
                .map_err(error)?
                .ok_or_else(|| error("existing install user IDs absent"))?;
        record.write_i32(user_ids.len() as i32);
        for user in user_ids {
            let state = crate::package::scan_snapshot::user_record::captured(
                installed.scan(),
                &name,
                false,
                user,
            )
            .map_err(error)?
            .ok_or_else(|| error("existing install user transport absent"))?;
            aim_service_aidl::write_byte_array(&mut record, Some(&state));
        }
        let seinfo = installed
            .scan()
            .owner()
            .seinfo(&name)
            .map_err(error)?
            .ok_or_else(|| error("existing install seinfo absent"))?;
        record.write_string16(Some(seinfo));
        record.write_bool(newly_installed && original_user_state.archive_state.is_some());
        self.check_package_bootstrap(&owner.bridge)?;
        let input=crate::package::installer::existing::Record::new(record.data().to_vec());
        let binder=self.process.add_service(input.clone());
        let preparation=owner.bridge.existing_package_installed(binder,request.user,request.flags);
        input.revoke();
        let inodes=preparation.map_err(|e|error(format!("existing install committed; preparation owner: {e:?}")))?;
        self.commit_existing_data_inodes(&owner.bridge, &name, request.user, inodes)?;
        let resolution = resolver
            .resolution(installed.state())
            .map_err(|e| error(format!("existing install publication resolution: {e:?}")))?;
        let query = crate::package::query::Query {
            state: installed.state(),
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let prediction = installed
            .state()
            .system
            .roles
            .as_ref()
            .ok_or_else(|| error("existing install role owner absent"))?
            .package(crate::package::roles::Role::AppPrediction, &query)?;
        owner
            .effects
            .package_added(&name, request.user, false, 0, prediction.as_deref())?;
        owner.changes.update(&name, &[request.user]);
        owner.start_restore(request, &name)?;
        let _ = pid;
        Ok(1)
    }
    pub(crate) fn retain_existing_status_receiver(
        &self,
        receiver: Option<&crate::package::installer::existing::IntentSender>,
    ) -> Result<Option<Arc<aim_binder_host::local::Strong>>, Exception> {
        Ok(match receiver.and_then(|receiver| receiver.0) {
            Some(aim_binder_host::parcel::Binder::Handle(handle)) => {
                Some(Arc::new(self.process.strong(handle)))
            }
            _ => None,
        })
    }
    pub(crate) fn restore_existing_install_preferences(
        &self,
        name: &str,
        user: i32,
    ) -> Result<(), Exception> {
        let capture = self.capture_package_queries()?;
        let preferred = capture
            .state()
            .system
            .preferred_owner
            .clone()
            .ok_or_else(|| error("existing install preferred owner absent"))?;
        let persistence = {
            let bootstrap = self.package_bootstrap.lock().unwrap();
            bootstrap
                .current
                .as_ref()
                .and_then(|current| current.persistence.clone())
                .ok_or_else(|| error("existing install preference persistence absent"))?
        };
        let pending = persistence
            .lock()
            .unwrap()
            .pending_default_browser(user as u32)
            .map_err(error)?;
        if pending.as_deref() == Some(name) {
            preferred
                .actions
                .restore_browser(user, name, true)
                .map_err(|e| error(format!("existing install browser restore: {e:?}")))?;
            persistence
                .lock()
                .unwrap()
                .take_matching_pending_browser(user as u32, name)
                .map_err(error)?;
        }
        preferred
            .actions
            .reconcile_home(user, 1000)
            .map_err(|e| error(format!("existing install home restore: {e:?}")))?;
        Ok(())
    }
    fn commit_existing_data_inodes(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        name: &str,
        user: i32,
        inodes: [i64; 3],
    ) -> Result<(), Exception> {
        let (capture, persistence) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .filter(|c| Arc::ptr_eq(&c.bridge, bridge))
                .ok_or_else(|| error("existing inode bootstrap replaced"))?;
            (
                current
                    .queries
                    .clone()
                    .ok_or_else(|| error("existing inode capture absent"))?,
                current
                    .persistence
                    .clone()
                    .ok_or_else(|| error("existing inode disk absent"))?,
            )
        };
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|c| {
                Arc::ptr_eq(&c.bridge, bridge)
                    && c.queries.as_ref().is_some_and(|c| Arc::ptr_eq(c, &capture))
            })
            .ok_or_else(|| error("existing inode generation changed"))?;
        let mut scan = capture.scan().owner().clone();
        let mut user_state = scan
            .scanned_user_states(name)
            .and_then(|users| users.get(&user))
            .cloned()
            .ok_or_else(|| error("existing inode sparse user absent"))?;
        if inodes[0] & 2 != 0 && inodes[1] != -1 {
            user_state.ce_data_inode = inodes[1];
        }
        if inodes[0] & 1 != 0 && inodes[2] != -1 {
            user_state.de_data_inode = inodes[2];
        }
        scan.set_user_state(name, user, user_state).map_err(error)?;
        let update = capture
            .prepare_package_update(scan.clone())
            .map_err(error)?;
        let write = disk.commit_live_install_restrictions(
            &scan,
            user as u32,
            &[name.to_owned()].into(),
            true,
        );
        if write.is_ok() || write.as_ref().is_err_and(|e| e.committed) {
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page {
                page.publish(update.capture.scan().version());
            }
            state.version = update.capture.scan().version();
        }
        write.map_err(|e| {
            error(format!(
                "existing install committed; inode persistence: {e}"
            ))
        })
    }
}
