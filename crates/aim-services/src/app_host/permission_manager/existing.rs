//! PermissionService.onPackageInstalled and restricted exemption updates,
//! android-16.0.0_r1 (Apache-2.0), through the existing access owner.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

impl PermissionManager {
    pub(in crate::app_host) fn existing_package_installed(
        &self,
        name: &str,
        allowlisted: Option<&[Option<String>]>,
        user: i32,
    ) -> Result<(), Exception> {
        let allowlisted =
            allowlisted.ok_or_else(|| Exception::illegal_argument("missing allowlist"))?;
        if allowlisted.iter().any(Option::is_none) {
            return Err(Exception::illegal_argument("null allowlist permission"));
        }
        // PermissionService makes two mutations: clear implicit upgrade exemptions,
        // then union the installer's allowlist. Each publishes its own effects.
        for installing in [false, true] {
            self.mutate(Some(KILL_APP_REASON_PERMISSIONS_REVOKED), |access, new| {
                let ps = access.state.packages.get(name).ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "existing permission package absent")
                })?;
                if !access.user_exists(user) || user != 0 {
                    return Err(Exception::illegal_argument(
                        "existing permission user absent",
                    ));
                }
                if ps.is.apex {
                    return Ok(Vec::new());
                }
                let pkg = ps.pkg.as_deref().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "existing permission code absent")
                })?;
                let app_id = ps.shared_user_app_id.unwrap_or(ps.app_id);
                let packages = access.packages_of(app_id);
                let mut changes = Vec::new();
                for name in &pkg.requested_permissions {
                    let Some(permission) = access.definition(name) else {
                        continue;
                    };
                    let flags = access
                        .state
                        .packages
                        .get(&permission.package)
                        .and_then(|p| p.pkg.as_deref())
                        .and_then(|p| p.permissions.iter().find(|p| p.component.name == *name))
                        .map_or(0, |p| p.component.flags);
                    if flags & (FLAG_HARD_RESTRICTED | FLAG_SOFT_RESTRICTED) == 0 {
                        continue;
                    }
                    let old = new.permission_flags(app_id, name);
                    let exempt = if installing {
                        if !allowlisted.iter().any(|p| p.as_deref() == Some(name)) {
                            continue;
                        }
                        old | INSTALLER_EXEMPT
                    } else {
                        if ps.is.system
                            || packages.iter().any(|p| {
                                p.is.system
                                    && p.pkg
                                        .as_ref()
                                        .is_some_and(|p| p.requested_permissions.contains(name))
                            })
                        {
                            continue;
                        }
                        old & !UPGRADE_EXEMPT
                    };
                    let target = packages
                        .iter()
                        .filter_map(|p| p.pkg.as_deref())
                        .filter(|p| p.requested_permissions.contains(name))
                        .map(|p| p.target_sdk_version)
                        .min()
                        .unwrap_or(10000);
                    changes.extend(set_flags(
                        new,
                        app_id,
                        name,
                        exemption_flags(name, flags, target, old, exempt),
                    ));
                }
                Ok(changes)
            })?;
        }
        Ok(())
    }
}

fn exemption_flags(name: &str, info: i32, target: i32, old: i32, mut flags: i32) -> i32 {
    if old == flags {
        return flags;
    }
    let exempt = flags & (INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT) != 0;
    if info & FLAG_HARD_RESTRICTED != 0 && !exempt {
        flags |= RESTRICTION_REVOKED;
        if permission_granted(old) {
            flags &= !POLICY_FIXED;
        }
    } else {
        flags &= !RESTRICTION_REVOKED;
    }
    let storage_exempt = target >= 29
        && matches!(
            name,
            "android.permission.READ_EXTERNAL_STORAGE"
                | "android.permission.WRITE_EXTERNAL_STORAGE"
        );
    if info & FLAG_SOFT_RESTRICTED != 0 && !exempt && !storage_exempt {
        flags |= SOFT_RESTRICTED;
    } else {
        flags &= !SOFT_RESTRICTED;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Service};
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;

    #[test]
    fn install_existing_permissions_publish_exemptions_and_admit_only_system() {
        crate::system::with_discovery_session(|host, publish, _| {
            const NAME: &str = "p.RESTRICTED";
            publish(&mut |pkg| {
                pkg.requested_permissions = vec![NAME.into()];
                if pkg.package_name == "p" {
                    pkg.permissions = vec![crate::package::pkg::Permission {
                        component: crate::package::pkg::Component {
                            name: NAME.into(),
                            flags: FLAG_HARD_RESTRICTED,
                            ..Default::default()
                        },
                        protection_level: 1,
                        ..Default::default()
                    }];
                }
            });
            let app_id = host.packages().unwrap().0.packages["p"].app_id;
            let owner = Arc::new(PermissionManager::with_system(
                host.clone(),
                AccessSystem {
                    permissions: vec![Permission {
                        name: NAME.into(),
                        package: "p".into(),
                        protection_level: 1,
                        dynamic: None,
                    }],
                    ..Default::default()
                },
            ));
            owner.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser {
                app_id_permissions: vec![(
                    app_id,
                    vec![(NAME.into(), UPGRADE_EXEMPT | RUNTIME_GRANTED | POLICY_FIXED)],
                )],
                ..Default::default()
            }));
            assert!(host.permission_manager.set(owner.clone()).is_ok());
            let package_system = host.package_system().unwrap();
            package_system.set_native_permissions(&owner);
            // The discovery fixture normally uses Java permission projections.
            // Native publication retains its domain owner's SystemConfig too.
            let base = host.package_capture().unwrap();
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
            package_system
                .publish_package_scan_with_queries(
                    &package_system.package_bootstrap().unwrap(),
                    Some(base.scan()),
                    scan.clone(),
                    base.scan().usage().clone(),
                    context,
                )
                .unwrap();
            let system = crate::app_host::system_process::SystemProcess::new(host.clone());
            let request = |allowlisted| {
                let mut p = Parcel::new();
                sp::ExistingPackagePermissionsInstalled {
                    package_name: Some("p".into()),
                    allowlisted,
                    user_id: 0,
                }
                .write(&mut p);
                p
            };
            let call = |uid, code, p: &Parcel| {
                system
                    .transact(&mut Call {
                        code,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap()
            };
            let p = request(Some(vec![]));
            for uid in [0, 2000, 10100] {
                let out = call(uid, sp::EXISTING_PACKAGE_PERMISSIONS_INSTALLED, &p);
                assert_eq!(
                    out.reader().read_exception().unwrap().unwrap_err().code,
                    aim_binder_host::parcel::EX_SECURITY
                );
            }
            assert!(owner.access_writes.lock().unwrap().is_empty());
            let out = call(1000, sp::EXISTING_PACKAGE_PERMISSIONS_INSTALLED, &p);
            sp::read_existing_package_permissions_installed_reply(&mut out.reader())
                .unwrap()
                .unwrap();
            assert_eq!(
                owner
                    .retained_access()
                    .unwrap()
                    .user0
                    .permission_flags(app_id, NAME),
                RUNTIME_GRANTED | RESTRICTION_REVOKED
            );
            assert_eq!(
                host.package_capture()
                    .unwrap()
                    .context()
                    .access
                    .as_ref()
                    .unwrap()
                    .user0
                    .permission_flags(app_id, NAME),
                RUNTIME_GRANTED | RESTRICTION_REVOKED
            );
            let out = call(
                1000,
                sp::EXISTING_PACKAGE_PERMISSIONS_INSTALLED,
                &request(Some(vec![Some(NAME.into())])),
            );
            sp::read_existing_package_permissions_installed_reply(&mut out.reader())
                .unwrap()
                .unwrap();
            assert_eq!(
                owner
                    .retained_access()
                    .unwrap()
                    .user0
                    .permission_flags(app_id, NAME),
                RUNTIME_GRANTED | INSTALLER_EXEMPT
            );
            let mut p = Parcel::new();
            sp::IsPackageCeStoragePrepared { user_id: 10 }.write(&mut p);
            assert_eq!(
                call(2000, sp::IS_PACKAGE_CE_STORAGE_PREPARED, &p)
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                aim_binder_host::parcel::EX_SECURITY
            );
            let out = call(1000, sp::IS_PACKAGE_CE_STORAGE_PREPARED, &p);
            assert!(
                !sp::read_is_package_ce_storage_prepared_reply(&mut out.reader())
                    .unwrap()
                    .unwrap()
            );
        });
    }

    #[test]
    fn restriction_updates_follow_exemption_and_shared_target_rules() {
        let granted = RUNTIME_GRANTED | POLICY_FIXED | UPGRADE_EXEMPT;
        assert_eq!(
            exemption_flags(
                "sms",
                FLAG_HARD_RESTRICTED,
                36,
                granted,
                granted & !UPGRADE_EXEMPT
            ),
            RUNTIME_GRANTED | RESTRICTION_REVOKED
        );
        assert_eq!(
            exemption_flags(
                "sms",
                FLAG_HARD_RESTRICTED,
                36,
                RUNTIME_GRANTED | RESTRICTION_REVOKED,
                RUNTIME_GRANTED | RESTRICTION_REVOKED | INSTALLER_EXEMPT
            ),
            RUNTIME_GRANTED | INSTALLER_EXEMPT
        );
        for (target, expected) in [(28, SOFT_RESTRICTED), (29, 0)] {
            assert_eq!(
                exemption_flags(
                    "android.permission.READ_EXTERNAL_STORAGE",
                    FLAG_SOFT_RESTRICTED,
                    target,
                    UPGRADE_EXEMPT,
                    0
                ),
                expected
            );
        }
    }
}
