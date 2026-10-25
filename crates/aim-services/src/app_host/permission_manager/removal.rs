//! Single-use removed-package scopes, PermissionService at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::*;
use aim_service_aidl::dev_aim_server_iinstallerpermissionremoval as removal;

struct Removal {
    owner: Arc<PermissionManager>,
    package: PackageState,
    user: i32,
    consumed: Mutex<bool>,
}

impl PermissionManager {
    pub(in crate::app_host) fn prepare_removal(
        self: &Arc<Self>,
        package: &str,
        app_id: i32,
        user: i32,
    ) -> Result<Binder, Exception> {
        let (state, _) = self.host.packages()?;
        let package = state.packages.get(package).ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "Native removed-package scope unavailable")
        })?;
        if package.app_id != app_id {
            return Err(Exception::illegal_argument(
                "removal permission app ID differs",
            ));
        }
        if !matches!(user, -1 | 0)
            || package.shared_user.is_some()
            || state
                .packages
                .values()
                .any(|ps| ps.app_id == app_id && ps.name != package.name)
        {
            return Err(self.host.unimplemented_named(
                "installer removal",
                "shared UID or non-user-0 permissions (#1996)",
            ));
        }
        self.retained_access()?;
        Ok(self.host.process.add_service(Arc::new(Removal {
            owner: self.clone(),
            package: package.clone(),
            user,
            consumed: Mutex::new(false),
        })))
    }

    fn remove_permissions(
        &self,
        package: &PackageState,
        user: i32,
        clear_grants: bool,
    ) -> Result<(), Exception> {
        // Modern PermissionService.onPackageRemoved(AndroidPackage) does
        // nothing. onPackageUninstalled decides full removal from live state.
        if !clear_grants {
            return Ok(());
        }
        let (state, _) = self.host.packages()?;
        let removed = !state.packages.contains_key(&package.name);
        let clear_user0 = user == 0
            || (user == -1 && super::super::user_service::user_ids(&self.host, true)?.contains(&0));
        self.retained_access()?;
        let mut held = self.owner.lock().unwrap();
        let before_system = held.system.clone().expect("loaded");
        let before_user = held.user0.clone().expect("loaded");
        let mut system = (*before_system).clone();
        let mut after = (*before_user).clone();
        let mut names = BTreeSet::new();
        if removed {
            // AppIdPermissionPolicy.trimPermissions and onAppIdRemoved.
            system.permissions.retain(|p| {
                if p.package == package.name {
                    names.insert(p.name.clone());
                    false
                } else {
                    true
                }
            });
            system
                .permission_trees
                .retain(|p| p.package != package.name);
            after
                .package_versions
                .retain(|(name, _)| *name != package.name);
        }
        let mut changes = Vec::new();
        for (app_id, flags) in &mut after.app_id_permissions {
            flags.retain(|(name, old)| {
                let remove = (clear_user0 && *app_id == package.app_id) || names.contains(name);
                if remove && *old != 0 {
                    changes.push(FlagsChanged {
                        app_id: *app_id,
                        permission: name.clone(),
                        old: *old,
                        new: 0,
                    });
                }
                !remove
            });
        }
        after
            .app_id_permissions
            .retain(|(_, flags)| !flags.is_empty());
        if system != *before_system {
            self.write_system(&system)?;
            held.system = Some(Arc::new(system));
        }
        if after != *before_user {
            self.write_user0(&after)?;
            held.user0 = Some(Arc::new(after));
        }
        drop(held);
        self.grants_changed()?;
        let effects = self.permission_effects(
            &before_system.permissions,
            &changes,
            Some(KILL_APP_REASON_PERMISSIONS_REVOKED),
        );
        let audio = self.host.refresh_audio_permissions();
        effects?;
        audio
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{BOOT_SYSTEM, write_access};
    use super::*;

    fn apply(
        owner: &PermissionManager,
        scope: Binder,
        code: bool,
        grants: bool,
    ) -> Result<(), Exception> {
        let Binder::Local(ptr) = scope else {
            panic!("local removal scope");
        };
        let mut p = Parcel::new();
        removal::Apply {
            code_removed: code,
            clear_grants: grants,
        }
        .write(&mut p);
        let response = owner
            .host
            .process
            .local_service(ptr)
            .unwrap()
            .transact(removal::APPLY, &p, false)
            .unwrap();
        removal::read_apply_reply(&mut response.reader()).unwrap()
    }

    #[test]
    fn removal_scopes_persist_grants_and_definitions_without_touching_other_access() {
        crate::system::with_discovery_session(|host, _, _| {
            write_access(host, SYSTEM_ACCESS, BOOT_SYSTEM);
            write_access(host, USER0_ACCESS, br#"<access><package-versions><package name="p" version="15"/><package name="q" version="15"/></package-versions>
                <app-id-permissions><app-id id="10100"><permission name="android.permission.INTERNET" flags="1"/><permission name="android.permission.CAMERA" flags="30"/></app-id>
                <app-id id="10101"><permission name="android.permission.INTERNET" flags="1"/><permission name="p.NORMAL" flags="1"/></app-id></app-id-permissions>
                <package-app-ops><package name="q"><app-op name="android:camera" mode="1"/></package></package-app-ops></access>"#);
            std::fs::write(host.data().unwrap().join("system/users/0.xml"),
                br#"<user id="0" serialNumber="0" flags="19" type="android.os.usertype.full.SYSTEM"/>"#).unwrap();
            let owner = Arc::new(PermissionManager::new(host.clone()));
            assert!(host.permission_manager.set(owner.clone()).is_ok());
            let system = host.package_system().unwrap();
            system.set_native_permissions(&owner);
            let mut definitions = (*owner.system().unwrap()).clone();
            for (name, tree) in [("p.NORMAL", false), ("p.tree", true)] {
                let p = Permission {
                    name: name.into(),
                    package: "p".into(),
                    protection_level: 0,
                    dynamic: None,
                };
                if tree {
                    definitions.permission_trees.push(p);
                } else {
                    definitions.permissions.push(p);
                }
            }
            owner.write_system(&definitions).unwrap();
            owner.owner.lock().unwrap().system = Some(Arc::new(definitions));
            let base = system.capture_package_queries().unwrap();
            let scan = base.scan().owner();
            let config = crate::package::system_config::SystemConfig::default();
            let mut domains = crate::package::domain_verification::owner::Owner::new(
                Default::default(),
                Default::default(),
            );
            let mut policies = BTreeMap::new();
            for setting in &scan.settings.packages {
                domains
                    .add(
                        crate::package::domain_verification::owner::Input {
                            id: setting.domain_set_id.as_deref().unwrap(),
                            name: &setting.name,
                            code: Some(scan.loaded_packages()[&setting.name].runtime_package()),
                            signatures: &[],
                            system: false,
                            restrict_domains: true,
                            pre_verified: None,
                        },
                        &config,
                    )
                    .unwrap();
                policies.insert(setting.name.clone(), true);
            }
            let mut context = (**base.context())
                .clone()
                .attach_boot_domains(
                    scan,
                    crate::package::domain_verification::owner::Boot {
                        owner: domains,
                        changes: vec![],
                    },
                    &config,
                    &policies,
                )
                .unwrap();
            context.scan_version += 1;
            context.access = Some(owner.retained_access().unwrap());
            let bridge = system.package_bootstrap().unwrap();
            system
                .publish_package_scan_with_queries(
                    &bridge,
                    Some(base.scan()),
                    scan.clone(),
                    base.scan().usage().clone(),
                    context,
                )
                .unwrap();
            let original = owner.retained_access().unwrap();
            let Binder::Local(ptr) = host.process.add_service(Arc::new(
                super::super::super::installer_removal::InstallerRemoval::new(host.clone()),
            )) else {
                unreachable!()
            };
            let external = crate::package::installer::removal::External::new(
                crate::system::ServiceOwner::Local(host.process.local_service(ptr).unwrap()),
                host.process.clone(),
            );
            let scope = external.prepare_permissions("p", 10100, 0).unwrap();
            external.apply_permissions(scope, false, false).unwrap();
            assert!(original.same_base(&owner.retained_access().unwrap()));
            assert!(owner.prepare_removal("p", 10101, 0).is_err());
            assert!(owner.prepare_removal("p", 10100, 10).is_err());
            let no_op = owner.prepare_removal("p", 10100, 0).unwrap();
            apply(&owner, no_op, true, false).unwrap();
            assert!(original.same_base(&owner.retained_access().unwrap()));
            assert!(
                apply(&owner, no_op, false, true)
                    .unwrap_err()
                    .message
                    .contains("consumed")
            );
            let failed = owner.prepare_removal("p", 10100, 0).unwrap();
            let blocked = host
                .data()
                .unwrap()
                .join(USER0_ACCESS)
                .with_extension("abx.new");
            std::fs::create_dir(&blocked).unwrap();
            assert!(apply(&owner, failed, false, true).is_err());
            assert!(original.same_base(&owner.retained_access().unwrap()));
            std::fs::remove_dir(blocked).unwrap();
            assert!(
                apply(&owner, failed, false, true)
                    .unwrap_err()
                    .message
                    .contains("consumed")
            );
            let per_user = owner.prepare_removal("p", 10100, 0).unwrap();
            apply(&owner, per_user, false, true).unwrap();
            let user = owner.retained_access().unwrap();
            assert_eq!(
                user.user0
                    .permission_flags(10100, "android.permission.INTERNET"),
                0
            );
            assert_eq!(
                user.user0.permission_flags(10101, "p.NORMAL"),
                INSTALL_GRANTED
            );
            assert!(user.system.permissions.iter().any(|p| p.name == "p.NORMAL"));
            let all = owner.prepare_removal("p", 10100, -1).unwrap();
            let capture = system.capture_package_queries().unwrap();
            let (bridge, disk) = system.package_persistence_owner(&capture).unwrap();
            let store = system.native_removal_store(&bridge, disk).unwrap();
            let mut uninstalled = store
                .snapshots
                .capture()
                .owner()
                .scanned_user_states("p")
                .unwrap()[&0]
                .clone();
            uninstalled.installed = false;
            store.user_state("p", 0, uninstalled).unwrap();
            store.remove_setting("p").unwrap();
            apply(&owner, all, true, true).unwrap();
            let after = owner.retained_access().unwrap();
            assert!(!after.system.permissions.iter().any(|p| p.package == "p"));
            assert!(
                !after
                    .system
                    .permission_trees
                    .iter()
                    .any(|p| p.package == "p")
            );
            assert_eq!(after.user0.permission_flags(10101, "p.NORMAL"), 0);
            assert_eq!(
                after
                    .user0
                    .permission_flags(10101, "android.permission.INTERNET"),
                INSTALL_GRANTED
            );
            assert!(!after.user0.package_versions.iter().any(|(p, _)| p == "p"));
            assert_eq!(after.user0.package_app_ops, original.user0.package_app_ops);
            let reload = PermissionManager::new(host.clone())
                .retained_access()
                .unwrap();
            assert_eq!(*reload.system, *after.system);
            assert_eq!(*reload.user0, *after.user0);
            let capture = system.capture_package_queries().unwrap();
            assert!(capture.context().access.as_ref().unwrap().same_base(&after));
            assert!(!capture.state().packages.contains_key("p"));
        });
    }
}

impl Service for Removal {
    fn descriptor(&self) -> &str {
        removal::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID as u32 {
            return reply(Err(Exception::security(
                "native installer bridge requires system UID",
            )));
        }
        if call.code != removal::APPLY {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        let a = removal::Apply::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        reply((|| {
            let mut consumed = self.consumed.lock().unwrap();
            if *consumed {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Removal permission scope already consumed",
                ));
            }
            *consumed = true;
            drop(consumed);
            // codeRemoved invokes the modern no-op onPackageRemoved above.
            self.owner
                .remove_permissions(&self.package, self.user, a.clear_grants)?;
            let mut p = Parcel::new();
            removal::write_apply_reply(&mut p);
            Ok(p)
        })())
    }
}
