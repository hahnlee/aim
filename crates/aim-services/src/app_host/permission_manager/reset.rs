//! PermissionService.resetRuntimePermissionsForUser, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

impl PermissionManager {
    pub(in crate::app_host) fn reset_runtime_permissions_for_user(
        &self,
        user: i32,
    ) -> Result<(), Exception> {
        self.mutate(Some(KILL_APP_REASON_PERMISSIONS_REVOKED), |access, new| {
            access.reset_runtime_permissions_for_user(new, user)
        })
    }
}

impl Access<'_> {
    fn reset_runtime_permissions_for_user(
        &self,
        new: &mut AccessUser,
        user: i32,
    ) -> Result<Vec<FlagsChanged>, Exception> {
        // The original batches every non-APEX package in one access mutation.
        let mut changes = Vec::new();
        for ps in self.state.packages.values().filter(|ps| !ps.is.apex) {
            changes.extend(self.reset_runtime_permissions(new, &ps.name, user)?);
        }
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::AndroidPackage;
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    use std::sync::mpsc;
    const CAMERA: &str = "android.permission.CAMERA";

    #[test]
    fn per_user_batch_skips_apex_removed_code_and_fixed_grants() {
        let mut state = State::default();
        let mut user0 = AccessUser::default();
        for (name, id) in [
            ("live", 10001),
            ("apex", 10002),
            ("removed", 10003),
            ("fixed", 10004),
        ] {
            state.packages.insert(
                name.into(),
                PackageState {
                    name: name.into(),
                    app_id: id,
                    is: crate::package::model::StateFlags {
                        apex: name == "apex",
                        ..Default::default()
                    },
                    pkg: (name != "removed").then(|| {
                        Arc::new(AndroidPackage {
                            package_name: name.into(),
                            requested_permissions: vec![CAMERA.into()],
                            ..Default::default()
                        })
                    }),
                    ..Default::default()
                },
            );
            user0.app_id_permissions.push((
                id,
                vec![(
                    CAMERA.into(),
                    RUNTIME_GRANTED | USER_SET | if name == "fixed" { POLICY_FIXED } else { 0 },
                )],
            ));
        }
        let definitions = [Permission {
            name: CAMERA.into(),
            package: "android".into(),
            protection_level: 1,
            dynamic: None,
        }];
        let access = Access {
            state: &state,
            filter: &AppsFilter::default(),
            defined: &definitions,
            groups: &[],
            user0: &user0,
            compat: &CompatConfig::default(),
        };
        let mut new = user0.clone();
        let changes = access
            .reset_runtime_permissions_for_user(&mut new, 0)
            .unwrap();
        assert_eq!(
            changes.iter().map(|c| c.app_id).collect::<Vec<_>>(),
            [10001]
        );
        assert_eq!(new.permission_flags(10001, CAMERA), 0);
        for id in [10002, 10003, 10004] {
            assert_eq!(
                new.permission_flags(id, CAMERA),
                user0.permission_flags(id, CAMERA)
            );
        }
    }

    #[test]
    fn per_user_reset_requires_a_permission_owner() {
        let process = crate::test_process(1000);
        let host = AppHost::new(process);
        let system = crate::app_host::system_process::SystemProcess::new(host.clone());
        let mut request = Parcel::new();
        sp::ResetPackageRuntimePermissionsForUser { user_id: 0 }.write(&mut request);
        let out = system
            .transact(&mut Call {
                code: sp::RESET_PACKAGE_RUNTIME_PERMISSIONS_FOR_USER,
                flags: 0,
                sender_pid: 77,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        assert_eq!(
            sp::read_reset_package_runtime_permissions_for_user_reply(&mut out.reader())
                .unwrap()
                .unwrap_err(),
            Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable")
        );
        host.process.shutdown().unwrap();
    }

    struct Listener(Arc<AppHost>, mpsc::Sender<i32>);
    impl Service for Listener {
        fn descriptor(&self) -> &str {
            changed_listener::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, changed_listener::ON_PERMISSIONS_CHANGED);
            let a = changed_listener::OnPermissionsChanged::read(&mut call.data)?;
            let capture = self.0.package_capture().unwrap();
            let access = capture.context().access.as_ref().unwrap();
            assert_eq!(access.user0.permission_flags(a.uid, CAMERA), 0);
            let disk = std::fs::read(self.0.data().unwrap().join(USER0_ACCESS)).unwrap();
            let saved = AccessUser::parse(&aim_android_xml::read(&disk).unwrap()).unwrap();
            assert_eq!(saved.permission_flags(a.uid, CAMERA), 0);
            for ps in capture.state().packages.values().filter(|ps| !ps.is.apex) {
                assert_eq!(access.user0.permission_flags(ps.app_id, CAMERA), 0);
                assert_eq!(saved.permission_flags(ps.app_id, CAMERA), 0);
            }
            self.1.send(a.uid).unwrap();
            Ok(Parcel::new())
        }
    }

    #[test]
    fn per_user_reset_admits_system_and_publishes_before_revocation_effects() {
        crate::system::with_discovery_session(|host, publish, _| {
            publish(&mut |pkg| {
                pkg.requested_permissions = vec![CAMERA.into()];
            });
            let mut app_ids: Vec<_> = host
                .packages()
                .unwrap()
                .0
                .packages
                .values()
                .map(|p| p.app_id)
                .collect();
            app_ids.sort();
            let owner = Arc::new(PermissionManager::with_system(
                host.clone(),
                AccessSystem {
                    permissions: vec![Permission {
                        name: CAMERA.into(),
                        package: "android".into(),
                        protection_level: 1,
                        dynamic: None,
                    }],
                    ..Default::default()
                },
            ));
            owner.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser {
                app_id_permissions: app_ids
                    .iter()
                    .map(|id| (*id, vec![(CAMERA.into(), RUNTIME_GRANTED | USER_SET)]))
                    .collect(),
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
            let (tx, rx) = mpsc::channel();
            owner.listeners.lock().unwrap().push(
                host.process
                    .add_service(Arc::new(Listener(host.clone(), tx))),
            );
            let system = crate::app_host::system_process::SystemProcess::new(host.clone());
            let call = |uid, user| {
                let mut p = Parcel::new();
                sp::ResetPackageRuntimePermissionsForUser { user_id: user }.write(&mut p);
                let out = system
                    .transact(&mut Call {
                        code: sp::RESET_PACKAGE_RUNTIME_PERMISSIONS_FOR_USER,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap();
                sp::read_reset_package_runtime_permissions_for_user_reply(&mut out.reader())
                    .unwrap()
            };
            for uid in [0, 2000, 10100] {
                assert_eq!(
                    call(uid, 0).unwrap_err().code,
                    aim_binder_host::parcel::EX_SECURITY
                );
            }
            // Absent users have no access state, as in the original policy.
            for user in [-1, 10] {
                call(1000, user).unwrap();
            }
            assert!(owner.access_writes.lock().unwrap().is_empty());
            let before = host.package_capture().unwrap();
            // A persistence failure must leave all grants and effects untouched.
            let file = host.data().unwrap().join(USER0_ACCESS);
            std::fs::create_dir_all(&file).unwrap();
            assert_eq!(call(1000, 0).unwrap_err().code, EX_ILLEGAL_STATE);
            std::fs::remove_dir(&file).unwrap();
            assert!(Arc::ptr_eq(&before, &host.package_capture().unwrap()));
            assert!(owner.access_writes.lock().unwrap().is_empty());
            assert!(owner.permission_kills.lock().unwrap().is_empty());
            assert!(rx.try_recv().is_err());
            call(1000, 0).unwrap();
            // PermissionPolicy's normal app-op synchronization can write
            // this same file after the batched grant mutation.
            let writes = owner.access_writes.lock().unwrap().clone();
            assert!(!writes.is_empty());
            assert!(writes.iter().all(|path| path == USER0_ACCESS));
            let mut seen = Vec::new();
            for _ in &app_ids {
                seen.push(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
            }
            seen.sort();
            assert_eq!(seen, app_ids);
            let mut kills = owner.permission_kills.lock().unwrap().clone();
            kills.sort();
            assert_eq!(
                kills,
                app_ids
                    .iter()
                    .map(|id| (*id, KILL_APP_REASON_PERMISSIONS_REVOKED.into()))
                    .collect::<Vec<_>>()
            );
            for id in &app_ids {
                assert_eq!(
                    before
                        .context()
                        .access
                        .as_ref()
                        .unwrap()
                        .user0
                        .permission_flags(*id, CAMERA),
                    RUNTIME_GRANTED | USER_SET
                );
                assert_eq!(
                    owner
                        .retained_access()
                        .unwrap()
                        .user0
                        .permission_flags(*id, CAMERA),
                    0
                );
            }
            call(1000, 0).unwrap();
            assert_eq!(owner.access_writes.lock().unwrap().len(), writes.len());
            assert!(rx.try_recv().is_err());
            assert_eq!(owner.permission_kills.lock().unwrap().len(), app_ids.len());
        });
    }
}
