//! App-host enabled handover through the persisted native session.
//! PMS setEnabledSettings / PackageMonitorCallbackHelper, android-16.0.0_r1.
use super::*;
use crate::app_host::{AppHost, PackageEffects, package_service::PackageService};
use crate::package::{
    mutation_dispatch::Dependencies,
    preferred::registry::{Identity, IdentityProvider, Registry},
    scan_snapshot::query_state::Capture,
    write::mutation::{Change, Plan},
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_os_iremotecallback as callback,
    dev_aim_server_ipackagemutationbridge as effects,
};
use std::sync::mpsc;
use std::time::Duration;

struct NoIdentities;
impl IdentityProvider for NoIdentities {
    fn allocate(
        &self,
        _: crate::package::preferred::records::Record<'_>,
    ) -> std::result::Result<Identity, String> {
        Err("enabled mutation does not allocate a preference".into())
    }
}
struct Effects(PackageEffects);
impl Service for Effects {
    fn descriptor(&self) -> &str {
        effects::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == effects::IS_STATE_PROTECTED {
            effects::IsStateProtected::read(&mut call.data)?;
            let mut reply = Parcel::new();
            effects::write_is_state_protected_reply(&mut reply, false);
            Ok(reply)
        } else if call.code == effects::IS_DATA_PROTECTED {
            effects::IsDataProtected::read(&mut call.data)?;
            let mut reply = Parcel::new();
            effects::write_is_data_protected_reply(&mut reply, false);
            Ok(reply)
        } else {
            self.0.transact(call)
        }
    }
}
struct Bridge(Binder, Arc<std::sync::atomic::AtomicBool>, Binder);
impl Service for Bridge {
    fn descriptor(&self) -> &str {
        bootstrap::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut reply = Parcel::new();
        match call.code {
            bootstrap::GET_PACKAGE_MUTATION_BRIDGE => {
                bootstrap::GetPackageMutationBridge::read(&mut call.data)?;
                bootstrap::write_get_package_mutation_bridge_reply(&mut reply, Some(self.0));
            }
            bootstrap::GET_APPLICATION_DATA_BRIDGE => {
                bootstrap::GetApplicationDataBridge::read(&mut call.data)?;
                bootstrap::write_get_application_data_bridge_reply(&mut reply, Some(self.2));
            }
            bootstrap::PACKAGE_MONITOR_USER => {
                let a = bootstrap::PackageMonitorUser::read(&mut call.data)?;
                assert_eq!(a.calling_uid, 1000);
                bootstrap::write_package_monitor_user_reply(&mut reply, a.user_id);
            }
            bootstrap::INVALIDATE_PACKAGE_INFO_CACHE => {
                bootstrap::InvalidatePackageInfoCache::read(&mut call.data)?;
                bootstrap::write_invalidate_package_info_cache_reply(&mut reply);
            }
            bootstrap::INVALIDATE_PACKAGES_FOR_UID_CACHE => {
                bootstrap::InvalidatePackagesForUidCache::read(&mut call.data)?;
                bootstrap::write_invalidate_packages_for_uid_cache_reply(&mut reply);
            }
            bootstrap::GET_PACKAGE_INSTALLED_PERMISSIONS => {
                bootstrap::GetPackageInstalledPermissions::read(&mut call.data)?;
                bootstrap::write_get_package_installed_permissions_reply(&mut reply, &Some(vec![]));
            }
            bootstrap::IS_APPLICATION_QUERY_FILTERING_ENABLED => {
                bootstrap::IsApplicationQueryFilteringEnabled::read(&mut call.data)?;
                bootstrap::write_is_application_query_filtering_enabled_reply(&mut reply, false);
            }
            bootstrap::GET_PACKAGE_GRANTED_PERMISSIONS => {
                let a = bootstrap::GetPackageGrantedPermissions::read(&mut call.data)?;
                assert!(matches!(
                    (a.package_name.as_deref(), a.app_id, a.user_id),
                    (Some("p"), 10100, 0) | (Some("q"), 10101, 0)
                ));
                let grants = if self.1.load(Ordering::SeqCst) {
                    vec![Some("android.permission.CAMERA".into())]
                } else {
                    vec![]
                };
                bootstrap::write_get_package_granted_permissions_reply(&mut reply, &Some(grants));
            }
            bootstrap::GET_PERMISSION_GIDS_FOR_UID => {
                let a = bootstrap::GetPermissionGidsForUid::read(&mut call.data)?;
                assert!(matches!(a.uid, 10100 | 10101));
                let gids = if self.1.load(Ordering::SeqCst) {
                    vec![3003]
                } else {
                    vec![]
                };
                bootstrap::write_get_permission_gids_for_uid_reply(&mut reply, &Some(gids));
            }
            bootstrap::TRANSACT_BATCH => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
            _ => return OriginalBridge.transact(call),
        }
        Ok(reply)
    }
}
struct Monitor(mpsc::Sender<()>);
impl Service for Monitor {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, callback::SEND_RESULT);
        call.data.enforce_interface(callback::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 1);
        assert!(crate::bundle::read(&mut call.data)?.is_some());
        self.0.send(()).unwrap();
        Ok(Parcel::new())
    }
}
fn transact(service: &dyn Service, uid: u32, code: u32, data: &Parcel) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}
fn publish_code(
    fixture: &Fixture,
    bridge: &Arc<crate::package::bootstrap::Bridge>,
) -> Arc<Capture> {
    let base = publish(fixture, bridge);
    let mut owner = base.scan().owner().clone();
    // Two loaded packages let the real implicit-access owner create a grant.
    let mut other = owner.settings.packages[0].clone();
    other.name = "q".into();
    other.app_id = 10101;
    other.code_path = "/data/app/q".into();
    other.domain_set_id = Some("00000000-0000-0000-0000-000000000002".into());
    // Start a complete scan owner from both persisted identities.
    owner = crate::package::scan::SigningScan::new(
        &Default::default(),
        &crate::package::settings::Settings {
            packages: vec![owner.settings.packages[0].clone(), other],
            ..Default::default()
        },
        36,
    )
    .unwrap();
    let names = ["p", "q"];
    owner
        .capture_user_states(
            names
                .map(|name| {
                    (
                        (name.into(), false),
                        crate::package::scan::CapturedUsers {
                            states: BTreeMap::from([(
                                0,
                                crate::package::restrictions::UserState {
                                    ce_data_inode: 991,
                                    de_data_inode: 992,
                                    ..Default::default()
                                },
                            )]),
                            active_aliases: Default::default(),
                        },
                    )
                })
                .into(),
        )
        .unwrap();
    let groups = owner
        .identities
        .shared_users
        .keys()
        .map(|name| (name.clone(), Default::default()))
        .collect();
    owner
        .capture_legacy_permissions(
            &[0],
            names
                .map(|name| ((name.into(), false), Default::default()))
                .into(),
            groups,
        )
        .unwrap();
    owner
        .capture_install_permissions_fixed(names.map(|name| ((name.into(), false), false)).into())
        .unwrap();
    owner
        .rebuild_shared_processes_from_native_members()
        .unwrap();
    let mut context = (**base.context()).clone();
    context.scan_version = base.scan().version() + 1;
    context.system.uninstall_blocks = Some(Arc::new(Default::default()));
    let mut q = context.packages[&("p".into(), false)].clone();
    q.app_id = 10101;
    q.path = "/data/app/q".into();
    context.packages.insert(("q".into(), false), q);
    for (name, uid) in [("p", 10100), ("q", 10101)] {
        let mut activity = crate::package::pkg::Activity::default();
        activity.main.component.name = format!("{name}.Main");
        activity.main.component.package_name = name.into();
        activity.max_aspect_ratio = Some(0.0);
        activity.min_aspect_ratio = Some(0.0);
        activity.main.enabled = true;
        activity.main.exported = true;
        owner.load_test_code(crate::package::pkg::AndroidPackage {
            package_name: name.into(),
            path: Some(format!("/data/app/{name}")),
            uid,
            booleans: crate::package::pkg::booleans::ENABLED
                | crate::package::pkg::booleans::HAS_CODE
                | crate::package::pkg::booleans::RESET_ENABLED_SETTINGS_ON_APP_DATA_CLEARED,
            feature_flag_state: Some(vec![]),
            activities: vec![activity],
            ..Default::default()
        });
    }
    owner
        .assign_seinfo_at_boot(
            &crate::package::owner::seinfo::Policy::parse(&[]).unwrap(),
            &mut |_: &crate::package::pkg::AndroidPackage| Ok(36),
        )
        .unwrap();
    owner
        .complete_library_dependencies(&|_, _| {
            Ok(crate::package::libraries::Policy {
                enforce_native_dependencies: false,
                sdk_library_independence: false,
            })
        })
        .unwrap();
    owner
        .rebuild_shared_processes_from_native_members()
        .unwrap();
    owner
        .capture_replica_runtime(
            names
                .map(|name| {
                    (
                        (name.into(), false),
                        crate::package::scan::ReplicaRuntime {
                            usage: [0; 8],
                            seinfo: owner.seinfo(name).unwrap().map(str::to_owned),
                            override_seinfo: None,
                            library_files: vec![],
                            libraries: vec![],
                        },
                    )
                })
                .into(),
        )
        .unwrap();
    fixture
        .system
        .publish_package_scan_with_queries(
            bridge,
            Some(base.scan()),
            owner,
            crate::package::owner::usage::Usage::new(names),
            context,
        )
        .unwrap()
}
struct Session {
    fixture: Fixture,
    host: Arc<AppHost>,
    cached: PackageService,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    old: Arc<Capture>,
    workers: Option<crate::package::events::Workers>,
    dependencies: Arc<Dependencies>,
    grants: Arc<std::sync::atomic::AtomicBool>,
    fail_clear: Arc<std::sync::atomic::AtomicBool>,
}
fn session() -> Session {
    let mut fixture = Fixture::new();
    let host = AppHost::new(fixture.native.clone());
    let cached = PackageService::new(host.clone());
    host.set_system(fixture.system.clone());
    let host_leaf = host.clone();
    let grants = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bridge_grants = grants.clone();
    let fail_clear = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let primitives = Arc::new(DataPrimitives {
        system: Arc::downgrade(&fixture.system),
        grants: grants.clone(),
        fail: fail_clear.clone(),
    });
    let bridge = fixture.attach_service(move |original| {
        Arc::new(Bridge(
            original.add_service(Arc::new(Effects(PackageEffects::new(host_leaf)))),
            bridge_grants,
            original.add_service(primitives),
        ))
    });
    let old = publish_code(&fixture, &bridge);
    let workers = fixture.system.configure_package_events(&bridge).unwrap();
    let data = fixture.root.join("data");
    std::fs::create_dir_all(data.join("system/users/0")).unwrap();
    let mut disk = crate::package::owner::Store::create(&data, &[0]).unwrap();
    disk.commit_scan_settings(old.scan()).unwrap();
    fixture
        .system
        .install_package_persistence(&bridge, &old, Arc::new(Mutex::new(disk)))
        .unwrap();
    let effects = fixture.system.package_effects_owner(&bridge).unwrap();
    let changes = fixture
        .system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .changes
        .clone();
    let preferred = Arc::new(Registry::new(Arc::new(NoIdentities)));
    fixture
        .system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .preferred_registry = Some(preferred.clone());
    let dependencies = Arc::new(Dependencies {
        effects,
        changes,
        preferred,
        shell_restricted: Box::new(|_| Ok(false)),
        enable_compressed: Box::new(|_, _| {
            Err(Exception::illegal_argument("no compressed package"))
        }),
        commit_preferred: Box::new(|_| Err(Exception::illegal_argument("no preferred mutation"))),
        system_install_state: Box::new(|_, _, _| {
            Err(Exception::illegal_argument("no system install"))
        }),
        quarantine_enabled: false,
    });
    fixture
        .system
        .install_package_mutations(&bridge, dependencies.clone())
        .unwrap();
    Session {
        fixture,
        host,
        cached,
        bridge,
        old,
        workers,
        dependencies,
        grants,
        fail_clear,
    }
}
#[test]
fn cached_host_enabled_writes_publish_and_notify_one_registry() {
    let Session {
        fixture,
        host,
        cached,
        bridge,
        old,
        workers,
        dependencies,
        grants,
        ..
    } = session();
    let old_launch = host.launch_inputs("p").unwrap();
    assert!(Arc::ptr_eq(&old_launch.capture, &old));
    let data = fixture.root.join("data");
    let (send, receive) = mpsc::channel();
    let monitor = fixture.native.add_service(Arc::new(Monitor(send)));
    let (public, _) = crate::package::service::PackageQueries::from_system(&fixture.system);
    let mut request = Parcel::new();
    pm::RegisterPackageMonitorCallback {
        callback: Some(monitor),
        user_id: 0,
    }
    .write(&mut request);
    for service in [&cached as &dyn Service, public.as_ref()] {
        transact(
            service,
            1000,
            pm::REGISTER_PACKAGE_MONITOR_CALLBACK,
            &request,
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    }
    let setting = |class: &str, new_state, flags| {
        let mut request = Parcel::new();
        pm::SetComponentEnabledSetting {
            component_name: Some(crate::package::intent::ComponentName {
                package: "p".into(),
                class: class.into(),
            }),
            new_state,
            flags,
            user_id: 0,
            calling_package: None,
        }
        .write(&mut request);
        request
    };
    // Binder identity is preserved for the cached front's denied caller.
    let request = setting("p.Main", 2, 0);
    let denied = transact(&cached, 10101, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap_err();
    assert_eq!(denied.code, aim_binder_host::parcel::EX_SECURITY);
    host.grant_implicit_access(10100, 10101).unwrap();
    fixture
        .system
        .internal_add_isolated_uid(99001, 10100, 1000, 0)
        .unwrap();
    let runtime = fixture.system.capture_package_queries().unwrap();
    assert_ne!(
        runtime.state().system.implicit_access,
        old.state().system.implicit_access
    );
    transact(&cached, 1000, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    let events = fixture.system.package_events().unwrap();
    assert!(events.wait(2000, false).unwrap());
    assert!(
        receive.try_recv().is_err(),
        "a callback registered through both handles fires once"
    );
    let current = fixture.system.capture_package_queries().unwrap();
    let (state, resolution) = host.packages().unwrap();
    assert!(Arc::ptr_eq(&state, current.state()));
    let query = crate::package::query::Query {
        state: &state,
        filter: &resolution.apps_filter,
        calling_uid: 1000,
    };
    let component = crate::package::intent::ComponentName {
        package: "p".into(),
        class: "p.Main".into(),
    };
    assert!(
        query
            .activity_info(&component, 0, 1000, 0)
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(
        old.state().packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    assert_eq!(
        current.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    assert_eq!(current.state().system.isolated_owners, [(99001, 10100)]);
    assert_eq!(
        current.state().system.implicit_access,
        runtime.state().system.implicit_access
    );
    // The reset path is also a session publication; its pending effect is real.
    // Exercise it through the native bridge's host Writes owner below.
    let persisted = crate::package::State::read(&data, &[0]).unwrap().unwrap();
    assert_eq!(
        persisted.users[0]
            .1
            .restrictions
            .packages
            .iter()
            .find(|(name, _)| name == "p")
            .unwrap()
            .1
            .disabled_components
            .as_deref(),
        Some(["p.Main".to_string()].as_slice())
    );
    grants.store(true, Ordering::SeqCst);
    fixture
        .system
        .refresh_package_permission_queries(&bridge, &old)
        .unwrap();
    let granted = fixture.system.capture_package_queries().unwrap();
    assert_eq!(
        granted.state().packages["p"].users[&0].granted_permissions,
        ["android.permission.CAMERA"]
    );
    assert_eq!(granted.state().packages["p"].users[&0].gids, [3003]);
    let granted_launch = host.launch_inputs("p").unwrap();
    assert!(Arc::ptr_eq(&granted_launch.capture, &granted));
    assert_eq!(granted_launch.permission_gids, [3003]);
    assert_eq!(granted_launch.application.uid, old_launch.application.uid);
    assert_eq!(
        granted_launch.application.se_info,
        old_launch.application.se_info
    );
    assert!(old_launch.permission_gids.is_empty());
    assert_eq!(
        granted.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    assert_eq!(granted.state().system.isolated_owners, [(99001, 10100)]);
    assert!(
        old.state().packages["p"].users[&0]
            .granted_permissions
            .is_empty()
    );
    grants.store(false, Ordering::SeqCst);
    fixture
        .system
        .refresh_package_permission_queries(&bridge, &old)
        .unwrap();
    let revoked = fixture.system.capture_package_queries().unwrap();
    assert!(
        revoked.state().packages["p"].users[&0]
            .granted_permissions
            .is_empty()
    );
    assert!(revoked.state().packages["p"].users[&0].gids.is_empty());
    assert_eq!(granted.state().packages["p"].users[&0].gids, [3003]);
    let revoked_launch = host.launch_inputs("p").unwrap();
    assert!(Arc::ptr_eq(&revoked_launch.capture, &revoked));
    assert!(revoked_launch.permission_gids.is_empty());
    assert_eq!(granted_launch.permission_gids, [3003]);
    // A delayed enable is visible before its callback, then delivers once.
    let request = setting("p.Main", 0, 1);
    transact(&cached, 1000, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    assert!(
        host.packages().unwrap().0.packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    assert!(receive.try_recv().is_err());
    receive.recv_timeout(Duration::from_secs(12)).unwrap();
    assert!(events.wait(2000, false).unwrap());
    assert!(receive.try_recv().is_err());
    // Retained same-value single requests must not repeat effects after rebasing.
    for application in [false, true] {
        let base = fixture.system.capture_package_queries().unwrap();
        let resolution = base.resolution().unwrap();
        let query = crate::package::query::Query {
            state: base.state(),
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let (code, request) = if application {
            let mut data = Parcel::new();
            pm::SetApplicationEnabledSetting {
                package_name: Some("p".into()),
                new_state: 3,
                flags: 0,
                user_id: 0,
                calling_package: None,
            }
            .write(&mut data);
            (pm::SET_APPLICATION_ENABLED_SETTING, data)
        } else {
            (pm::SET_COMPONENT_ENABLED_SETTING, setting("p.Main", 2, 0))
        };
        let first = crate::package::mutation_dispatch::prepare(
            code,
            &mut request.reader(),
            &query,
            77,
            &dependencies,
        )
        .unwrap()
        .unwrap();
        let second = crate::package::mutation_dispatch::prepare(
            code,
            &mut request.reader(),
            &query,
            77,
            &dependencies,
        )
        .unwrap()
        .unwrap();
        for prepared in [first, second] {
            fixture
                .system
                .publish_prepared_package_mutation(&bridge, &base, prepared, &dependencies, 1000)
                .unwrap();
        }
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(events.wait(2000, false).unwrap());
        assert!(
            receive.try_recv().is_err(),
            "idempotent retained setter repeats no monitor effect"
        );
    }
    let before_reset = fixture.system.capture_package_queries().unwrap();
    let plans = crate::package::application_data::reset_components(before_reset.state(), "p", 0);
    fixture
        .system
        .publish_package_plans(&bridge, &before_reset, plans, 1000)
        .unwrap();
    let reset = fixture.system.capture_package_queries().unwrap();
    assert!(
        reset.state().packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    assert_eq!(reset.state().packages["p"].users[&0].enabled, 3);
    assert_eq!(
        before_reset.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(events.wait(2000, false).unwrap());
    assert!(receive.try_recv().is_err());
    fixture
        .system
        .internal_remove_isolated_uid(99001, 1000, 0)
        .unwrap();
    drop(workers);
}

struct DataPrimitives {
    system: std::sync::Weak<System>,
    grants: Arc<std::sync::atomic::AtomicBool>,
    fail: Arc<std::sync::atomic::AtomicBool>,
}
impl Service for DataPrimitives {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_iapplicationdatabridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::dev_aim_server_iapplicationdatabridge as api;
        assert_eq!(call.sender_euid, 1000);
        let system = self.system.upgrade().unwrap();
        let mut reply = Parcel::new();
        match call.code {
            api::ENFORCE_PERMISSION => {
                let a = api::EnforcePermission::read(&mut call.data)?;
                assert_eq!(
                    a.permission.as_deref(),
                    Some("android.permission.CLEAR_APP_USER_DATA")
                );
                if a.uid != 1000 {
                    reply.write_exception(&Exception::security(
                        "clear-data fixture permission denied",
                    ));
                    return Ok(reply);
                }
                api::write_enforce_permission_reply(&mut reply);
            }
            api::KILL_AND_WAIT => {
                let a = api::KillAndWait::read(&mut call.data)?;
                assert_eq!((a.package_name.as_deref(), a.app_id), (Some("p"), 10100));
                assert_eq!(
                    system
                        .capture_package_queries()
                        .unwrap()
                        .frozen_packages()
                        .unwrap()
                        .get("p"),
                    Some(&1)
                );
                api::write_kill_and_wait_reply(&mut reply);
            }
            api::CLEAR_DATA => {
                let a = api::ClearData::read(&mut call.data)?;
                assert_eq!((a.package_name.as_deref(), a.user), (Some("p"), 0));
                if self.fail.load(Ordering::SeqCst) {
                    reply.write_exception(&Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "injected installd failure",
                    ));
                    return Ok(reply);
                }
                let retained = system.capture_package_queries().unwrap();
                assert_eq!(retained.state().packages["p"].users[&0].ce_data_inode, 991);
                assert_eq!(retained.state().packages["p"].users[&0].de_data_inode, 992);
                self.grants.store(false, Ordering::SeqCst);
                let bridge = system.package_bootstrap().unwrap();
                let capture = system.capture_package_queries().unwrap();
                system
                    .refresh_package_permission_queries(&bridge, &capture)
                    .unwrap();
                assert_eq!(
                    system
                        .capture_package_queries()
                        .unwrap()
                        .frozen_packages()
                        .unwrap()
                        .get("p"),
                    Some(&1)
                );
                api::write_clear_data_reply(&mut reply, true);
            }
            api::CHECK_MEMORY => {
                api::CheckMemory::read(&mut call.data)?;
                let capture = system.capture_package_queries().unwrap();
                assert!(capture.frozen_packages().unwrap().is_empty());
                assert!(
                    capture.state().packages["p"].users[&0]
                        .disabled_components
                        .is_empty()
                );
                api::write_check_memory_reply(&mut reply);
            }
            _ => panic!("unexpected application-data primitive {}", call.code),
        }
        Ok(reply)
    }
}
struct DataObserver(mpsc::Sender<(Option<String>, bool)>);
impl Service for DataObserver {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_content_pm_ipackagedataobserver::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_content_pm_ipackagedataobserver as api;
        assert_eq!(call.code, api::ON_REMOVE_COMPLETED);
        let a = api::OnRemoveCompleted::read(&mut call.data)?;
        self.0.send((a.package_name, a.succeeded)).unwrap();
        Ok(Parcel::new())
    }
}
#[test]
fn cached_host_data_clear_completes_after_session_publication_and_reports_failure() {
    let Session {
        fixture,
        cached,
        bridge,
        dependencies,
        grants,
        fail_clear,
        workers,
        ..
    } = session();
    let base = fixture.system.capture_package_queries().unwrap();
    let lifecycle = crate::package::lifecycle::Owner::from_settings(
        &crate::package::settings::Settings {
            versions: vec![crate::package::settings::Version {
                fingerprint: Some("clear-fixture".into()),
                sdk_version: 36,
                ..Default::default()
            }],
            ..Default::default()
        },
        true,
        "clear-fixture",
        Box::new(|_| None),
        Box::new(|_| Ok(true)),
    )
    .unwrap();
    let (instant, instant_worker) =
        crate::package::instant::Owner::open(&fixture.root.join("data"), base.state(), 16384, 320)
            .unwrap();
    let mut context = (**base.context()).clone();
    context.scan_version += 1;
    context.system.lifecycle = Some(lifecycle);
    context.system.instant_registry = Some(instant);
    fixture
        .system
        .publish_package_scan_with_queries(
            &bridge,
            Some(base.scan()),
            base.scan().owner().clone(),
            base.scan().usage().clone(),
            context,
        )
        .unwrap();
    fixture
        .system
        .install_package_freezer_publication(&bridge)
        .unwrap();
    let system = Arc::downgrade(&fixture.system);
    let attached = bridge.clone();
    let commit = Arc::new(move |base, plans, uid| {
        system
            .upgrade()
            .unwrap()
            .publish_package_plans(&attached, &base, plans, uid)
    });
    let data_worker = fixture
        .system
        .install_package_application_data(&bridge, commit)
        .unwrap();
    let (send, receive) = mpsc::channel();
    let observer = fixture.native.add_service(Arc::new(DataObserver(send)));
    let clear = |name: &str| {
        let mut data = Parcel::new();
        pm::ClearApplicationUserData {
            package_name: Some(name.into()),
            observer: Some(observer),
            user_id: 0,
        }
        .write(&mut data);
        data
    };
    let (event_send, event_receive) = mpsc::channel();
    let monitor = fixture.native.add_service(Arc::new(Monitor(event_send)));
    let mut register = Parcel::new();
    pm::RegisterPackageMonitorCallback {
        callback: Some(monitor),
        user_id: 0,
    }
    .write(&mut register);
    transact(
        &cached,
        1000,
        pm::REGISTER_PACKAGE_MONITOR_CALLBACK,
        &register,
    )
    .reader()
    .read_exception()
    .unwrap()
    .unwrap();
    let denied = transact(&cached, 10101, pm::CLEAR_APPLICATION_USER_DATA, &clear("p"));
    assert_eq!(
        denied.reader().read_exception().unwrap().unwrap_err().code,
        aim_binder_host::parcel::EX_SECURITY
    );
    assert!(receive.try_recv().is_err());
    let mut request = Parcel::new();
    pm::SetComponentEnabledSetting {
        component_name: Some(crate::package::intent::ComponentName {
            package: "p".into(),
            class: "p.Main".into(),
        }),
        new_state: 2,
        flags: 0,
        user_id: 0,
        calling_package: None,
    }
    .write(&mut request);
    transact(&cached, 1000, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    let base = fixture.system.capture_package_queries().unwrap();
    event_receive.recv_timeout(Duration::from_secs(2)).unwrap();
    grants.store(true, Ordering::SeqCst);
    fixture
        .system
        .refresh_package_permission_queries(&bridge, &base)
        .unwrap();
    let before = fixture.system.capture_package_queries().unwrap();
    assert_eq!(
        before.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    transact(&cached, 1000, pm::CLEAR_APPLICATION_USER_DATA, &clear("p"))
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        (Some("p".into()), true)
    );
    let after = fixture.system.capture_package_queries().unwrap();
    assert!(
        after.state().packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    assert!(
        after.state().packages["p"].users[&0]
            .granted_permissions
            .is_empty()
    );
    assert_eq!(
        before.state().packages["p"].users[&0].granted_permissions,
        ["android.permission.CAMERA"]
    );
    let persisted = crate::package::State::read(&fixture.root.join("data"), &[0])
        .unwrap()
        .unwrap();
    let restriction = &persisted.users[0]
        .1
        .restrictions
        .packages
        .iter()
        .find(|(name, _)| name == "p")
        .unwrap()
        .1;
    assert!(
        restriction
            .disabled_components
            .as_ref()
            .is_none_or(Vec::is_empty)
    );
    assert_eq!(
        (restriction.ce_data_inode, restriction.de_data_inode),
        (991, 992)
    );
    event_receive.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        fixture
            .system
            .package_events()
            .unwrap()
            .wait(3000, false)
            .unwrap()
    );
    assert!(event_receive.try_recv().is_err());
    fail_clear.store(true, Ordering::SeqCst);
    transact(&cached, 1000, pm::CLEAR_APPLICATION_USER_DATA, &clear("p"))
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        (Some("p".into()), false)
    );
    assert!(
        fixture
            .system
            .capture_package_queries()
            .unwrap()
            .frozen_packages()
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .system
            .package_application_data()
            .unwrap()
            .errors()
            .iter()
            .any(|error| error.contains("injected installd failure"))
    );
    transact(
        &cached,
        1000,
        pm::CLEAR_APPLICATION_USER_DATA,
        &clear("missing"),
    )
    .reader()
    .read_exception()
    .unwrap()
    .unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        (Some("missing".into()), false)
    );
    assert!(receive.try_recv().is_err());
    assert!(
        fixture
            .system
            .package_events()
            .unwrap()
            .wait(3000, false)
            .unwrap()
    );
    drop(data_worker);
    drop(instant_worker);
    drop(dependencies);
    drop(workers);
}

#[test]
fn host_user_unlocking_publishes_and_survives_enabled_and_permission_updates() {
    let Session {
        fixture,
        host,
        cached,
        bridge,
        grants,
        workers,
        ..
    } = session();
    let initial = fixture.system.capture_package_queries().unwrap();
    let mut context = (**initial.context()).clone();
    context.scan_version += 1;
    context.users.get_mut(&0).unwrap().unlocking_or_unlocked = false;
    let locked = fixture
        .system
        .publish_package_scan_with_queries(
            &bridge,
            Some(initial.scan()),
            initial.scan().owner().clone(),
            initial.scan().usage().clone(),
            context,
        )
        .unwrap();
    assert!(!locked.state().users[&0].unlocking_or_unlocked);
    assert!(!host.user_unlocking_or_unlocked(0).unwrap());
    assert!(host.user_unlocking(99).is_err());
    assert!(!host.user_unlocking_or_unlocked(99).unwrap());
    host.user_unlocking(0).unwrap();
    let unlocked = fixture.system.capture_package_queries().unwrap();
    assert!(unlocked.state().users[&0].unlocking_or_unlocked);
    assert!(unlocked.context().users[&0].unlocking_or_unlocked);
    assert!(host.user_unlocking_or_unlocked(0).unwrap());
    assert!(!locked.state().users[&0].unlocking_or_unlocked);
    assert_eq!(
        unlocked.scan().metadata_revision(),
        locked.scan().metadata_revision()
    );
    host.user_unlocking(0).unwrap();
    assert!(Arc::ptr_eq(
        &unlocked,
        &fixture.system.capture_package_queries().unwrap()
    ));
    let mut request = Parcel::new();
    pm::SetComponentEnabledSetting {
        component_name: Some(crate::package::intent::ComponentName {
            package: "p".into(),
            class: "p.Main".into(),
        }),
        new_state: 2,
        flags: 0,
        user_id: 0,
        calling_package: None,
    }
    .write(&mut request);
    transact(&cached, 1000, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    grants.store(true, Ordering::SeqCst);
    fixture
        .system
        .refresh_package_permission_queries(&bridge, &locked)
        .unwrap();
    let after = fixture.system.capture_package_queries().unwrap();
    assert!(after.state().users[&0].unlocking_or_unlocked);
    assert_eq!(
        after.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    assert_eq!(
        after.state().packages["p"].users[&0].granted_permissions,
        ["android.permission.CAMERA"]
    );
    assert_eq!(after.state().packages["p"].users[&0].ce_data_inode, 991);
    assert!(Arc::ptr_eq(&host.packages().unwrap().0, after.state()));
    assert!(
        fixture
            .system
            .package_events()
            .unwrap()
            .wait(3000, false)
            .unwrap()
    );
    drop(workers);
}
