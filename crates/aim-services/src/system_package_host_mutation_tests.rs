//! App-host enabled handover through the persisted native session.
//! PMS setEnabledSettings / PackageMonitorCallbackHelper, android-16.0.0_r1.
use super::*;
use crate::app_host::{AppHost, PackageEffects, package_service::PackageService};
use crate::package::{
    mutation_dispatch::Dependencies,
    preferred::registry::{Identity, IdentityProvider, Registry},
    scan_snapshot::query_state::Capture,
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_os_iremotecallback as callback,
    dev_aim_server_ipackagemutationbridge as effects,
};
use std::sync::mpsc;
use std::time::Duration;
#[path = "system_package_shell_permission_tests.rs"]
mod shell_permission_tests;
#[path = "system_package_shell_read_tests.rs"]
mod shell_read_tests;

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
struct Bridge(
    Binder,
    Arc<std::sync::atomic::AtomicBool>,
    Binder,
    Binder,
    Binder,
    Binder,
    (
        Arc<LocalProcess>,
        Mutex<Option<aim_binder_host::local::Strong>>,
    ),
    std::sync::atomic::AtomicI32,
);
// Fixture-only export of a service in the original process to the native host.
const EXPORT_TEST_BINDER: u32 = 0xffff_fffe;
const SET_TEST_SHELL_READ: u32 = 0xffff_fffd;
const SET_TEST_DOMAIN_DELIVERY: u32 = 0xffff_fffc;
impl Service for Bridge {
    fn descriptor(&self) -> &str {
        bootstrap::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut reply = Parcel::new();
        match call.code {
            SET_TEST_DOMAIN_DELIVERY => {
                self.7.store(call.data.read_i32()?, Ordering::SeqCst);
                reply.write_no_exception();
            }
            bootstrap::SEND_DOMAIN_VERIFICATION_REQUEST => {
                assert_eq!(call.sender_euid, 1000);
                let a = bootstrap::SendDomainVerificationRequest::read(&mut call.data)?;
                assert_eq!(call.data.remaining(), 0);
                // A failing delivery owner lets callers observe the exact selected request.
                match self.7.load(Ordering::SeqCst) {
                    0 => reply.write_exception(&Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("fixture delivery: {:?}", a.package_names),
                    )),
                    1 => bootstrap::write_send_domain_verification_request_reply(&mut reply),
                    2 => {} // malformed reply
                    3 => {
                        reply.write_no_exception();
                        reply.write_i32(42);
                    }
                    4 => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
                    _ => panic!("unknown delivery fixture mode"),
                }
            }
            SET_TEST_SHELL_READ => {
                let Some(Binder::Handle(handle)) = call.data.read_binder()? else {
                    panic!("read leaf")
                };
                *self.6.1.lock().unwrap() = Some(self.6.0.strong(handle));
                reply.write_no_exception();
            }
            EXPORT_TEST_BINDER => {
                reply.write_binder(Some(Binder::Local(call.data.read_i64()? as u64)));
            }
            bootstrap::GET_PACKAGE_INTERNAL_EVENTS_BRIDGE => {
                bootstrap::GetPackageInternalEventsBridge::read(&mut call.data)?;
                bootstrap::write_get_package_internal_events_bridge_reply(&mut reply, Some(self.3));
            }
            bootstrap::GET_PACKAGE_SHELL_POLICY_BRIDGE => {
                bootstrap::GetPackageShellPolicyBridge::read(&mut call.data)?;
                bootstrap::write_get_package_shell_policy_bridge_reply(&mut reply, Some(self.4));
            }
            bootstrap::GET_PACKAGE_SHELL_READ_LEAF => {
                bootstrap::GetPackageShellReadLeaf::read(&mut call.data)?;
                let retained = self.6.1.lock().unwrap();
                let binder = retained
                    .as_ref()
                    .map_or(self.5, |owner| Binder::Handle(owner.handle));
                bootstrap::write_get_package_shell_read_leaf_reply(&mut reply, Some(binder));
            }
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
            bootstrap::IS_DOMAIN_VERIFICATION_SETTINGS_V2ENABLED => {
                let a = bootstrap::IsDomainVerificationSettingsV2Enabled::read(&mut call.data)?;
                assert!(matches!(a.package_name.as_deref(), Some("p" | "q")));
                assert!(a.target_sdk >= 0);
                bootstrap::write_is_domain_verification_settings_v2enabled_reply(&mut reply, true);
            }
            bootstrap::IS_APPLICATION_QUERY_FILTERING_ENABLED => {
                bootstrap::IsApplicationQueryFilteringEnabled::read(&mut call.data)?;
                bootstrap::write_is_application_query_filtering_enabled_reply(&mut reply, false);
            }
            bootstrap::GET_PACKAGE_GRANTED_PERMISSIONS => {
                let a = bootstrap::GetPackageGrantedPermissions::read(&mut call.data)?;
                assert!(matches!(
                    (a.package_name.as_deref(), a.app_id, a.user_id),
                    (Some("p"), 10100, 0)
                        | (Some("q"), 10101, 0)
                        | (
                            Some("com.google.android.bluetooth" | "com.android.bluetooth"),
                            1002,
                            0
                        )
                        | (Some("contacts"), 10100, 0)
                        | (Some("ungranted"), 10123, 0)
                        | (Some("android"), 1000, 0)
                ));
                let grants = if self.1.load(Ordering::SeqCst) {
                    vec![Some("android.permission.CAMERA".into())]
                } else {
                    vec![]
                };
                bootstrap::write_get_package_granted_permissions_reply(&mut reply, &Some(grants));
            }
            bootstrap::FORMAT_PACKAGE_TIMESTAMP => {
                let a = bootstrap::FormatPackageTimestamp::read(&mut call.data)?;
                bootstrap::write_format_package_timestamp_reply(
                    &mut reply,
                    &Some(format!("fixture-date-{}", a.millis)),
                );
            }
            bootstrap::GET_LEGACY_PERMISSION_STATE => {
                let a = bootstrap::GetLegacyPermissionState::read(&mut call.data)?;
                let mut p = Parcel::new();
                p.write_i32(a.app_id);
                let users = a.user_ids.unwrap();
                p.write_i32(users.len() as i32);
                for user in users {
                    p.write_i32(user);
                    p.write_bool(false);
                    p.write_i32(1);
                    p.write_string16(Some("fixture.permission"));
                    p.write_bool(true);
                    p.write_bool(true);
                    p.write_i32(1);
                }
                bootstrap::write_get_legacy_permission_state_reply(
                    &mut reply,
                    &Some(p.data().to_vec()),
                );
            }
            bootstrap::GET_PERMISSION_GIDS_FOR_UID => {
                let a = bootstrap::GetPermissionGidsForUid::read(&mut call.data)?;
                assert!(matches!(a.uid, 1000 | 1002 | 10100 | 10101 | 10123));
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
fn ok(reply: Parcel) {
    reply.reader().read_exception().unwrap().unwrap();
}
fn enabled_request(class: Option<&str>, new_state: i32, flags: i32) -> (u32, Parcel) {
    let mut request = Parcel::new();
    let code = if let Some(class) = class {
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
        pm::SET_COMPONENT_ENABLED_SETTING
    } else {
        pm::SetApplicationEnabledSetting {
            package_name: Some("p".into()),
            new_state,
            flags,
            user_id: 0,
            calling_package: None,
        }
        .write(&mut request);
        pm::SET_APPLICATION_ENABLED_SETTING
    };
    (code, request)
}
fn wait_events(system: &System, timeout: i64) {
    assert!(
        system
            .package_events()
            .unwrap()
            .wait(timeout, false)
            .unwrap()
    );
}
fn register_monitor(fixture: &Fixture, services: &[&dyn Service]) -> mpsc::Receiver<()> {
    let (send, receive) = mpsc::channel();
    let monitor = fixture.native.add_service(Arc::new(Monitor(send)));
    let mut request = Parcel::new();
    pm::RegisterPackageMonitorCallback {
        callback: Some(monitor),
        user_id: 0,
    }
    .write(&mut request);
    for service in services {
        ok(transact(
            *service,
            1000,
            pm::REGISTER_PACKAGE_MONITOR_CALLBACK,
            &request,
        ));
    }
    receive
}
pub(crate) fn complete_owner(owner: &mut crate::package::scan::SigningScan) {
    let names = ["p", "q"];
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
                    let labels = owner.seinfo_state(name).unwrap().unwrap();
                    (
                        (name.into(), false),
                        crate::package::scan::ReplicaRuntime {
                            usage: [0; 8],
                            seinfo: labels.base.clone(),
                            override_seinfo: labels.override_label.clone(),
                            library_files: vec![],
                            libraries: vec![],
                        },
                    )
                })
                .into(),
        )
        .unwrap();
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
    complete_owner(&mut owner);
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
    session_with_fixture(Fixture::new())
}
fn session_with_fixture(mut fixture: Fixture) -> Session {
    let host = AppHost::new(fixture.native.clone());
    fixture.host = Some(host.clone());
    let cached = PackageService::new(host.clone());
    host.set_system(fixture.system.clone());
    host.package_booted_for_test();
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
            original.add_service(Arc::new(Effects(PackageEffects::new(host_leaf.clone())))),
            bridge_grants,
            original.add_service(primitives),
            original.add_service(Arc::new(crate::app_host::InternalEvents::new(
                host_leaf.clone(),
            ))),
            original.add_service(Arc::new(
                crate::app_host::package_shell_policy::ShellPolicy(host_leaf.clone()),
            )),
            original.add_service(Arc::new(crate::app_host::package_shell_read::ShellRead(
                host_leaf,
            ))),
            (original.clone(), Mutex::new(None)),
            std::sync::atomic::AtomicI32::new(0),
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
fn cached_host_enabled_writes_preserve_caller_identity() {
    let Session {
        fixture: _fixture,
        cached,
        workers,
        ..
    } = session();
    let (_, request) = enabled_request(Some("p.Main"), 2, 0);
    let denied = transact(&cached, 10101, pm::SET_COMPONENT_ENABLED_SETTING, &request)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap_err();
    assert_eq!(denied.code, aim_binder_host::parcel::EX_SECURITY);
    drop(workers);
}

#[test]
fn cached_and_public_hosts_share_one_monitor_registry() {
    let Session {
        fixture,
        cached,
        old,
        workers,
        ..
    } = session();
    let (public, _) = crate::package::service::PackageQueries::from_system(&fixture.system);
    let receive = register_monitor(&fixture, &[&cached, public.as_ref()]);
    let path = fixture
        .root
        .join("data/system/users/0/package-restrictions.xml");
    let bytes = std::fs::read(&path).ok();
    for class in [Some("p.Main"), None] {
        let (code, request) = enabled_request(class, 0, 2);
        ok(transact(&cached, 1000, code, &request));
        assert!(Arc::ptr_eq(
            &old,
            &fixture.system.capture_package_queries().unwrap()
        ));
        assert_eq!(std::fs::read(&path).ok(), bytes);
    }
    wait_events(&fixture.system, 2000);
    assert!(receive.try_recv().is_err());
    let (code, request) = enabled_request(Some("p.Main"), 2, 0);
    ok(transact(&cached, 1000, code, &request));
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_events(&fixture.system, 2000);
    assert!(
        receive.try_recv().is_err(),
        "a callback registered through both handles fires once"
    );
    drop(workers);
}

#[test]
fn cached_host_enabled_writes_publish_and_persist_without_losing_runtime_state() {
    let Session {
        fixture,
        host,
        cached,
        old,
        workers,
        ..
    } = session();
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
    let (code, request) = enabled_request(Some("p.Main"), 2, 2);
    ok(transact(&cached, 1000, code, &request));
    let data = fixture.root.join("data");
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
    fixture
        .system
        .internal_remove_isolated_uid(99001, 1000, 0)
        .unwrap();
    drop(workers);
}

#[test]
fn concurrent_enabled_mutations_preserve_runtime_visibility_and_uid_publications() {
    let Session {
        fixture,
        host,
        cached,
        old,
        workers,
        ..
    } = session();
    let start = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            start.wait();
            for i in 0..16 {
                let (code, request) =
                    enabled_request(Some("p.Main"), if i % 2 == 0 { 1 } else { 2 }, 2);
                ok(transact(&cached, 1000, code, &request));
            }
        });
        threads.spawn(|| {
            start.wait();
            host.grant_implicit_access(10100, 10101).unwrap();
            for i in 0..16 {
                fixture
                    .system
                    .internal_add_isolated_uid(99000 + i, 10100, 1000, 0)
                    .unwrap();
            }
        });
    });
    let current = fixture.system.capture_package_queries().unwrap();
    assert_ne!(
        current.state().system.implicit_access,
        old.state().system.implicit_access
    );
    for i in 0..16 {
        assert!(
            current
                .state()
                .system
                .isolated_owners
                .contains(&(99000 + i, 10100))
        );
    }
    assert_eq!(
        current.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    assert!(
        old.state().packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    let persisted = crate::package::State::read(&fixture.root.join("data"), &[0])
        .unwrap()
        .unwrap();
    let package = persisted.users[0]
        .1
        .restrictions
        .packages
        .iter()
        .find(|(name, _)| name == "p")
        .unwrap();
    assert_eq!(
        package.1.disabled_components.as_deref(),
        Some(["p.Main".to_string()].as_slice())
    );
    drop(workers);
}

#[test]
fn permission_refresh_updates_launch_inputs_and_preserves_enabled_state() {
    let Session {
        fixture,
        host,
        cached,
        bridge,
        old,
        grants,
        workers,
        ..
    } = session();
    let old_launch = host.launch_inputs("p").unwrap();
    assert!(Arc::ptr_eq(&old_launch.capture, &old));
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
    let (code, request) = enabled_request(Some("p.Main"), 2, 0);
    ok(transact(&cached, 1000, code, &request));
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
    fixture
        .system
        .internal_remove_isolated_uid(99001, 1000, 0)
        .unwrap();
    drop(workers);
}

#[test]
fn delayed_enable_publishes_before_its_single_callback() {
    let Session {
        fixture,
        host,
        cached,
        workers,
        ..
    } = session();
    let receive = register_monitor(&fixture, &[&cached]);
    let (code, request) = enabled_request(Some("p.Main"), 2, 0);
    ok(transact(&cached, 1000, code, &request));
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_events(&fixture.system, 2000);
    let (_, request) = enabled_request(Some("p.Main"), 0, 1);
    ok(transact(
        &cached,
        1000,
        pm::SET_COMPONENT_ENABLED_SETTING,
        &request,
    ));
    assert!(
        host.packages().unwrap().0.packages["p"].users[&0]
            .disabled_components
            .is_empty()
    );
    assert!(receive.try_recv().is_err());
    receive.recv_timeout(Duration::from_secs(12)).unwrap();
    wait_events(&fixture.system, 2000);
    assert!(receive.try_recv().is_err());
    drop(workers);
}

fn retained_enabled_setter_notifies_once(application: bool) {
    let Session {
        fixture,
        cached,
        bridge,
        dependencies,
        workers,
        ..
    } = session();
    let receive = register_monitor(&fixture, &[&cached]);
    let base = fixture.system.capture_package_queries().unwrap();
    let resolution = base.resolution().unwrap();
    let query = crate::package::query::Query {
        state: base.state(),
        filter: &resolution.apps_filter,
        calling_uid: 1000,
    };
    let (code, request) = enabled_request(
        (!application).then_some("p.Main"),
        if application { 3 } else { 2 },
        0,
    );
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
    wait_events(&fixture.system, 2000);
    assert!(
        receive.try_recv().is_err(),
        "idempotent retained setter repeats no monitor effect"
    );
    drop(workers);
}

#[test]
fn retained_component_enabled_setter_notifies_once() {
    retained_enabled_setter_notifies_once(false);
}

#[test]
fn retained_application_enabled_setter_notifies_once() {
    retained_enabled_setter_notifies_once(true);
}

#[test]
fn component_reset_publishes_and_notifies_without_resetting_application() {
    let Session {
        fixture,
        cached,
        bridge,
        workers,
        ..
    } = session();
    let receive = register_monitor(&fixture, &[&cached]);
    for class in [Some("p.Main"), None] {
        let (code, request) = enabled_request(class, if class.is_some() { 2 } else { 3 }, 0);
        ok(transact(&cached, 1000, code, &request));
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_events(&fixture.system, 2000);
        assert!(receive.try_recv().is_err());
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
    wait_events(&fixture.system, 2000);
    assert!(receive.try_recv().is_err());
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
    ok(transact(
        &cached,
        1000,
        pm::REGISTER_PACKAGE_MONITOR_CALLBACK,
        &register,
    ));
    let denied = transact(&cached, 10101, pm::CLEAR_APPLICATION_USER_DATA, &clear("p"));
    assert_eq!(
        denied.reader().read_exception().unwrap().unwrap_err().code,
        aim_binder_host::parcel::EX_SECURITY
    );
    assert!(receive.try_recv().is_err());
    let (_, request) = enabled_request(Some("p.Main"), 2, 0);
    ok(transact(
        &cached,
        1000,
        pm::SET_COMPONENT_ENABLED_SETTING,
        &request,
    ));
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
    ok(transact(
        &cached,
        1000,
        pm::CLEAR_APPLICATION_USER_DATA,
        &clear("p"),
    ));
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
    wait_events(&fixture.system, 3000);
    assert!(event_receive.try_recv().is_err());
    fail_clear.store(true, Ordering::SeqCst);
    ok(transact(
        &cached,
        1000,
        pm::CLEAR_APPLICATION_USER_DATA,
        &clear("p"),
    ));
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
    ok(transact(
        &cached,
        1000,
        pm::CLEAR_APPLICATION_USER_DATA,
        &clear("missing"),
    ));
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        (Some("missing".into()), false)
    );
    assert!(receive.try_recv().is_err());
    wait_events(&fixture.system, 3000);
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
    let (_, request) = enabled_request(Some("p.Main"), 2, 0);
    ok(transact(
        &cached,
        1000,
        pm::SET_COMPONENT_ENABLED_SETTING,
        &request,
    ));
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
    wait_events(&fixture.system, 3000);
    drop(workers);
}

/// A native query publication withheld until the startup regression releases it.
pub(crate) fn with_late_package_boot(test: impl FnOnce(&Arc<AppHost>, &mut dyn FnMut())) {
    with_late_package_construction(|host, construct| {
        test(host, &mut || {
            construct();
            host.package_booted_for_test();
        });
    });
}

pub(crate) fn with_late_package_construction(test: impl FnOnce(&Arc<AppHost>, &mut dyn FnMut())) {
    let mut fixture = Fixture::new();
    let host = AppHost::new(fixture.native.clone());
    host.set_system(fixture.system.clone());
    let bridge = fixture.attach();
    let image = fixture.root.join("image");
    std::fs::create_dir_all(image.join("system/framework")).unwrap();
    std::fs::write(image.join("system/build.prop"), "ro.build.version.sdk=36\n").unwrap();
    std::fs::write(
        image.join("system/framework/framework-res.apk"),
        include_bytes!("../tests/fixtures/discovery/framework.apk"),
    )
    .unwrap();
    crate::app_host::configure_boot_test_host(
        &host,
        &image,
        &fixture.root.join("data"),
        &fixture.root.join("bpf"),
        Box::new(|name| match name {
            "ro.build.version.sdk" => Some("36".into()),
            "ro.build.version.codename" => Some("REL".into()),
            "persist.sys.locale" => Some("en-US".into()),
            "ro.sf.lcd_density" => Some("160".into()),
            _ => None,
        }),
    );
    assert!(fixture.system.capture_package_queries().is_err());
    test(&host, &mut || {
        publish_restart_capture(&fixture, &bridge);
    });
}

/// A real session and disposable Settings owner for service-discovery regressions.
pub(crate) fn with_discovery_session(
    test: impl FnOnce(
        &Arc<AppHost>,
        &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
        &dyn Fn(Option<&str>, i32),
    ),
) {
    with_discovery_binders(|host, publish, enabled, _| test(host, publish, enabled));
}

pub(crate) fn with_discovery_binders(
    test: impl FnOnce(
        &Arc<AppHost>,
        &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
        &dyn Fn(Option<&str>, i32),
        &dyn Fn(Arc<dyn Service>) -> aim_binder_host::local::Strong,
    ),
) {
    with_discovery_binders_flags(&[], true, test);
}

pub(crate) fn with_boot_discovery_binders(
    test: impl FnOnce(
        &Arc<AppHost>,
        &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
        &dyn Fn(Option<&str>, i32),
        &dyn Fn(Arc<dyn Service>) -> aim_binder_host::local::Strong,
    ),
) {
    with_discovery_binders_flags(&[], false, test);
}

pub(crate) fn with_discovery_session_flags(
    flags: &[(&str, bool)],
    test: impl FnOnce(&Arc<AppHost>),
) {
    with_discovery_binders_flags(flags, true, |host, _, _, export| {
        let activity = export(crate::app_host::test_activity_service(host));
        host.package_system()
            .unwrap()
            .services
            .lock()
            .unwrap()
            .insert("activity", Arc::new(activity));
        test(host);
    });
}

pub(crate) fn with_discovery_binders_flags(
    flags: &[(&str, bool)],
    ready: bool,
    test: impl FnOnce(
        &Arc<AppHost>,
        &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
        &dyn Fn(Option<&str>, i32),
        &dyn Fn(Arc<dyn Service>) -> aim_binder_host::local::Strong,
    ),
) {
    with_discovery_binders_lifecycle(flags, ready, |h, p, e, x, _| test(h, p, e, x));
}

pub(crate) fn with_discovery_binders_lifecycle(
    flags: &[(&str, bool)],
    ready: bool,
    test: impl FnOnce(
        &Arc<AppHost>,
        &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
        &dyn Fn(Option<&str>, i32),
        &dyn Fn(Arc<dyn Service>) -> aim_binder_host::local::Strong,
        &dyn Fn(),
    ),
) {
    let Session {
        fixture,
        host,
        bridge,
        workers,
        cached,
        ..
    } = session();
    let image = fixture.root.join("image");
    std::fs::create_dir_all(image.join("system/framework")).unwrap();
    std::fs::write(image.join("system/build.prop"), "ro.build.version.sdk=36\n").unwrap();
    std::fs::write(
        image.join("system/framework/framework-res.apk"),
        include_bytes!("../tests/fixtures/discovery/framework.apk"),
    )
    .unwrap();
    std::fs::create_dir_all(fixture.root.join("data/app/p")).unwrap();
    std::fs::write(
        fixture.root.join("data/app/p/base.apk"),
        include_bytes!("../tests/fixtures/discovery/providers.apk"),
    )
    .unwrap();
    let configure = if ready {
        crate::app_host::configure_test_host
    } else {
        crate::app_host::configure_boot_test_host
    };
    configure(
        &host,
        &fixture.root.join("image"),
        &fixture.root.join("data"),
        &fixture.root.join("bpf"),
        Box::new(|name| match name {
            "ro.build.version.sdk" => Some("36".into()),
            "ro.build.version.codename" => Some("REL".into()),
            "persist.sys.locale" => Some("en-US".into()),
            "ro.sf.lcd_density" => Some("160".into()),
            "ro.debuggable" => Some("1".into()),
            _ => None,
        }),
    );
    let mut publish = |edit: &mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)| {
        let base = fixture.system.capture_package_queries().unwrap();
        let mut owner = base.scan().owner().clone();
        for package in base
            .state()
            .packages
            .values()
            .filter_map(|ps| ps.pkg.as_deref())
        {
            let mut package = package.clone();
            edit(&mut package);
            if package.is(crate::package::pkg::booleans::SYSTEM) {
                owner
                    .settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == package.package_name)
                    .unwrap()
                    .flags |= crate::package::settings::FLAG_SYSTEM;
            }
            owner.load_test_code(package);
        }
        complete_owner(&mut owner);
        let mut context = (**base.context()).clone();
        context
            .system
            .flags
            .retain(|(name, _)| !flags.iter().any(|(flag, _)| name == flag));
        context
            .system
            .flags
            .extend(flags.iter().map(|(name, on)| ((*name).into(), *on)));
        context.scan_version = base.scan().version() + 1;
        fixture
            .system
            .package_persistence_owner(&base)
            .unwrap()
            .1
            .lock()
            .unwrap()
            .commit_scan_settings_owner(&owner)
            .unwrap();
        fixture
            .system
            .publish_package_scan_with_queries(
                &bridge,
                Some(base.scan()),
                owner,
                crate::package::owner::usage::Usage::new(["p", "q"]),
                context,
            )
            .unwrap();
    };
    if !flags.is_empty() {
        publish(&mut |_| {});
    }
    let enabled = |class: Option<&str>, new_state| {
        let before = fixture.system.capture_package_queries().unwrap();
        let (code, request) = enabled_request(class, new_state, 0);
        ok(transact(&cached, 1000, code, &request));
        assert!(!Arc::ptr_eq(
            &before,
            &fixture.system.capture_package_queries().unwrap()
        ));
        wait_events(&fixture.system, 3000);
    };
    let export = |service| {
        let Binder::Local(pointer) = fixture.original[0].add_service(service) else {
            unreachable!()
        };
        let mut request = Parcel::new();
        request.write_i64(pointer as i64);
        let reply = bridge
            .owner
            .transact(EXPORT_TEST_BINDER, &request, false)
            .unwrap();
        let binder = reply.reader().read_binder().unwrap().unwrap();
        reply.retain_remote_binder(binder).unwrap()
    };
    test(&host, &mut publish, &enabled, &export, &|| {
        fixture.original[0].shutdown().unwrap()
    });
    drop(workers);
}

pub(crate) fn install_publication(
    host: &Arc<AppHost>,
    context: crate::package::installer::permission_capture::ContextSource,
) -> crate::package::installer::environment::QueryPublication {
    let system = host.package_system().unwrap();
    let bridge = system.package_bootstrap().unwrap();
    let snapshots = system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .snapshots
        .clone()
        .unwrap();
    system
        .package_install_query_publication(&bridge, snapshots, context)
        .unwrap()
}

// Maintenance and PMInternal share this preparation/CAS loop. Pause after
// projection so each competing real owner must complete before publication.
#[test]
fn maintenance_usage_retries_concurrent_grant_canonical_usage_and_uid_context() {
    for change in 0..4 {
        let Session {
            fixture,
            host,
            bridge,
            old,
            workers,
            ..
        } = session();
        let system = fixture.system.clone();
        let retained = bridge.clone();
        let (ready, waiting) = mpsc::channel();
        let (release, proceed) = mpsc::channel();
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let publisher = std::thread::spawn(move || {
            system.publish_package_usage_with(Some((&retained, 1000)), Some("p"), 0, 111, || {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    ready.send(()).unwrap();
                    proceed.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            })
        });
        waiting.recv_timeout(Duration::from_secs(3)).unwrap();
        let system = fixture.system.clone();
        let (done, completed) = mpsc::channel();
        let concurrent = std::thread::spawn(move || {
            let result = match change {
                0 => host.grant_implicit_access(10100, 10101),
                1 | 3 => system
                    .publish_package_usage(Some("p"), 0, if change == 1 { 222 } else { 111 })
                    .and_then(|_| system.publish_package_usage(Some("p"), 2, 222)),
                _ => system.internal_add_isolated_uid(99001, 10100, 1000, 0),
            };
            let capture = system.capture_package_queries().unwrap();
            done.send((result, capture)).unwrap();
        });
        let advanced = completed.recv_timeout(Duration::from_secs(3));
        release.send(()).unwrap();
        publisher.join().unwrap().unwrap();
        concurrent.join().unwrap();
        let (result, intermediate) = advanced.expect("concurrent owner waited for preparation");
        result.unwrap();
        let after = fixture.system.capture_package_queries().unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(after.scan().usage().times("p").unwrap()[0], 111);
        assert_eq!(old.scan().usage().times("p").unwrap(), &[0; 8]);
        match change {
            0 => {
                assert_ne!(
                    intermediate.state().system.implicit_access,
                    old.state().system.implicit_access
                );
                assert_eq!(
                    after.state().system.implicit_access,
                    intermediate.state().system.implicit_access
                );
            }
            1 | 3 => {
                // Holding install admission advances only canonical usage; the
                // unchanged query pointer must not let the first candidate win.
                assert!(Arc::ptr_eq(&intermediate, &old));
                assert_eq!(after.scan().usage().times("p").unwrap()[2], 222);
            }
            _ => {
                assert_eq!(
                    intermediate.state().system.isolated_owners,
                    [(99001, 10100)]
                );
                assert_eq!(after.context().system.isolated_owners, [(99001, 10100)]);
                assert_eq!(after.state().system.isolated_owners, [(99001, 10100)]);
                assert_eq!(after.state().uid_owners, intermediate.state().uid_owners);
            }
        }
        let state = fixture.system.package_bootstrap.lock().unwrap();
        let current = state.current.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            &current.snapshots.as_ref().unwrap().capture(),
            after.scan()
        ));
        assert_eq!(state.version, after.scan().version());
        assert_eq!(after.context().scan_version, state.version);
        assert_eq!(after.state().generation, state.version);
        assert_eq!(
            after
                .scan()
                .owner()
                .replica_runtime("p", false)
                .unwrap()
                .unwrap()
                .usage,
            *after.scan().usage().times("p").unwrap()
        );
        let fresh = Capture::new(after.scan().clone(), (**after.context()).clone()).unwrap();
        assert_eq!(after.state(), fresh.state());
        drop(state);
        drop(workers);
    }
}

#[test]
fn maintenance_usage_rejects_bridge_retired_during_preparation() {
    let Session {
        mut fixture,
        bridge,
        workers,
        ..
    } = session();
    let system = fixture.system.clone();
    let retained = bridge.clone();
    let (ready, waiting) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let publisher = std::thread::spawn(move || {
        system.publish_package_usage_with(Some((&retained, 1000)), Some("p"), 0, 111, || {
            ready.send(()).unwrap();
            proceed.recv_timeout(Duration::from_secs(5)).unwrap();
        })
    });
    waiting.recv_timeout(Duration::from_secs(3)).unwrap();
    let replacement = fixture.attach();
    release.send(()).unwrap();
    assert!(
        publisher
            .join()
            .unwrap()
            .unwrap_err()
            .message
            .contains("retired")
    );
    assert!(Arc::ptr_eq(
        &fixture.system.package_bootstrap().unwrap(),
        &replacement
    ));
    drop(workers);
}

#[test]
fn enabled_preparation_retries_concurrent_visibility_publication() {
    let Session {
        fixture,
        host,
        bridge,
        dependencies,
        old,
        workers,
        ..
    } = session();
    let resolution = old.resolution().unwrap();
    let query = crate::package::query::Query {
        state: old.state(),
        filter: &resolution.apps_filter,
        calling_uid: 1000,
    };
    let (code, request) = enabled_request(Some("p.Main"), 2, 2);
    let prepared = crate::package::mutation_dispatch::prepare(
        code,
        &mut request.reader(),
        &query,
        77,
        &dependencies,
    )
    .unwrap()
    .unwrap();
    let mut attempts = 0;
    let mut intermediate = old.clone();
    let install = fixture.system.package_install_guard();
    fixture
        .system
        .publish_prepared_package_mutation_with(
            &bridge,
            &old,
            prepared,
            &dependencies,
            1000,
            install,
            true,
            || {
                attempts += 1;
                if attempts == 1 {
                    std::thread::scope(|threads| {
                        threads
                            .spawn(|| host.grant_implicit_access(10100, 10101))
                            .join()
                            .unwrap()
                            .unwrap();
                    });
                    intermediate = fixture.system.capture_package_queries().unwrap();
                } else if attempts == 2 {
                    fixture
                        .system
                        .publish_package_usage(Some("p"), 0, 111)
                        .unwrap();
                }
            },
        )
        .unwrap();
    assert_eq!(attempts, 3);
    let current = fixture.system.capture_package_queries().unwrap();
    assert_eq!(current.scan().usage().times("p").unwrap()[0], 111);
    assert_eq!(
        current.state().system.implicit_access,
        intermediate.state().system.implicit_access
    );
    assert_eq!(
        current.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    drop(workers);
}

#[test]
fn enabled_projection_matches_full_for_users_and_factory_aliases() {
    use crate::package::{
        scan::CapturedUsers,
        scan_snapshot::Store,
        write::{
            Enabled,
            mutation::{Change, Plan},
        },
    };
    let Session { old, workers, .. } = session();
    let mut owner = old.scan().owner().clone();
    let mut factory = owner.settings.packages[0].clone();
    factory.code_path = "/system/app/p".into();
    owner.settings.disabled_system_packages.push(factory);
    let mut context = (**old.context()).clone();
    let mut factory_extra = context.packages[&("p".into(), false)].clone();
    factory_extra.path = "/system/app/p".into();
    context.packages.insert(("p".into(), true), factory_extra);
    let mut users = BTreeMap::new();
    for user in [0, 10, 11] {
        users.insert(
            user,
            crate::package::restrictions::UserState {
                enabled: user % 3,
                enabled_components: Some(vec!["p.Other".into()]),
                ..Default::default()
            },
        );
        let mut record = context.users[&0].clone();
        record.id = user;
        context.users.insert(user, record);
        for inputs in context.packages.values_mut() {
            inputs.users.insert(user, inputs.users[&0].clone());
        }
    }
    let keys: Vec<_> = context.packages.keys().cloned().collect();
    owner
        .capture_user_states(
            keys.iter()
                .cloned()
                .map(|key| {
                    let active_aliases = if key.1 {
                        [0, 11].into()
                    } else {
                        Default::default()
                    };
                    (
                        key,
                        CapturedUsers {
                            states: users.clone(),
                            active_aliases,
                        },
                    )
                })
                .collect(),
        )
        .unwrap();
    let groups = owner
        .identities
        .shared_users
        .keys()
        .map(|n| (n.clone(), Default::default()))
        .collect();
    owner
        .capture_legacy_permissions(
            &[0, 10, 11],
            keys.iter()
                .cloned()
                .map(|k| (k, Default::default()))
                .collect(),
            groups,
        )
        .unwrap();
    owner
        .capture_install_permissions_fixed(keys.iter().cloned().map(|k| (k, false)).collect())
        .unwrap();
    owner
        .complete_library_dependencies(&|_, _| {
            Ok(crate::package::libraries::Policy {
                enforce_native_dependencies: false,
                sdk_library_independence: false,
            })
        })
        .unwrap();
    let original = old.scan().owner();
    let runtime = keys
        .iter()
        .map(|(name, factory)| {
            let value = original.replica_runtime(name, false).unwrap();
            ((name.clone(), *factory), value.unwrap().clone())
        })
        .collect();
    owner.capture_replica_runtime(runtime).unwrap();
    let store = Store::new_replica(owner, old.scan().usage().clone()).unwrap();
    context.scan_version = store.capture().version();
    let base = Capture::new(store.capture(), context).unwrap();
    let old_resolution = base.resolution().unwrap();
    for user in [0, 10, 11] {
        for application in [false, true] {
            for new in [1, 2, 0] {
                let mut enabled = Enabled::of(&base.state().packages["p"], user);
                if application {
                    enabled.enabled = new;
                } else {
                    enabled.enabled_components.retain(|c| c != "p.Main");
                    enabled.disabled_components.retain(|c| c != "p.Main");
                    if new == 1 {
                        enabled.enabled_components.insert("p.Main".into());
                    }
                    if new == 2 {
                        enabled.disabled_components.insert("p.Main".into());
                    }
                }
                let prepared = crate::package::mutation_dispatch::Prepared {
                    code: pm::SET_COMPONENT_ENABLED_SETTING,
                    plans: vec![Plan {
                        package: "p".into(),
                        user: Some(user),
                        change: Change::Enabled(enabled),
                    }],
                    global: None,
                    finish: crate::package::mutation_dispatch::Finish::Enabled(vec![]),
                    reply: Parcel::new(),
                    distraction_cleanup: vec![],
                };
                let mut full_owner = base.scan().owner().clone();
                prepared.apply_scan(&mut full_owner).unwrap();
                let full = base.prepare_package_update(full_owner).unwrap().capture;
                let delta = base
                    .prepare_enabled_update(base.scan().owner(), &prepared)
                    .unwrap()
                    .capture;
                assert_eq!(delta.scan().owner(), full.scan().owner());
                assert_eq!(delta.state(), full.state());
                assert_eq!(
                    delta.scan().metadata_revision(),
                    full.scan().metadata_revision()
                );
                let resolution = delta.resolution().unwrap();
                assert!(!Arc::ptr_eq(&old_resolution, &resolution));
                let full_resolution = full.resolution().unwrap();
                let component = crate::package::intent::ComponentName {
                    package: "p".into(),
                    class: "p.Main".into(),
                };
                for app_id in [1000, 10100, 10101] {
                    let caller = user * 100000 + app_id;
                    let query = |r: &crate::package::resolve::Resolution| {
                        crate::package::query::Query {
                            state: &r.state,
                            filter: &r.apps_filter,
                            calling_uid: caller,
                        }
                        .activity_info(&component, 0, caller, user)
                        .unwrap()
                        .unwrap()
                    };
                    assert_eq!(query(&resolution), query(&full_resolution));
                }
            }
        }
    }
    drop(workers);
}

#[test]
fn package_restrictions_timer_flush_shutdown_and_failure() {
    let session = session();
    let system = &session.fixture.system;
    let boot = &system.package_bootstrap;
    let tick = |now| system.write_due_package_restrictions(&session.bridge, Some(now));
    let path = session
        .fixture
        .root
        .join("data/system/users/0/package-restrictions.xml");
    let initial = std::fs::read(&path).ok();
    let deadline = || {
        let state = boot.lock().unwrap();
        let current = state.current.as_ref().unwrap();
        current.restriction_writes.as_ref().unwrap().deadline
    };
    let set = |state, flags| {
        let (code, request) = enabled_request(Some("p.Main"), state, flags);
        ok(transact(&session.cached, 1000, code, &request));
    };
    // No-op SYNCHRONOUS carries no timer and writes nothing.
    set(0, 2);
    assert!(deadline().is_none());
    set(1, 0);
    let at = deadline().unwrap();
    set(1, 2);
    assert_eq!(deadline(), Some(at));
    set(2, 0);
    assert_eq!(deadline(), Some(at));
    assert_eq!(std::fs::read(&path).ok(), initial);
    let current = system.capture_package_queries().unwrap();
    assert_eq!(
        current.state().packages["p"].users[&0].disabled_components,
        ["p.Main"]
    );
    tick(at - Duration::from_millis(1)).unwrap();
    assert_eq!(std::fs::read(&path).ok(), initial);
    tick(at).unwrap();
    assert!(deadline().is_none());
    let bytes = std::fs::read(&path).unwrap();
    let disk = crate::package::State::read(&session.fixture.root.join("data"), &[0])
        .unwrap()
        .unwrap();
    let user = &disk.users[0].1.restrictions;
    let package = user.packages.iter().find(|(name, _)| name == "p").unwrap();
    assert_eq!(
        package.1.disabled_components.as_deref(),
        Some(["p.Main".to_string()].as_slice())
    );
    std::fs::rename(&path, path.with_extension("saved")).unwrap();
    tick(at + Duration::from_secs(20)).unwrap();
    assert!(!path.exists());
    std::fs::rename(path.with_extension("saved"), &path).unwrap();
    set(1, 0);
    let stale = deadline().unwrap();
    set(0, 2);
    assert!(deadline().is_none());
    assert_ne!(std::fs::read(&path).unwrap(), bytes);
    tick(stale).unwrap();
    set(2, 0);
    let due = deadline().unwrap();
    let reserve = path.with_file_name("package-restrictions.xml.reservecopy");
    std::fs::remove_file(&reserve).unwrap();
    std::fs::create_dir(&reserve).unwrap();
    let published = system.capture_package_queries().unwrap();
    let error = tick(due).unwrap_err();
    assert!(error.message.contains("user 0:"), "{}", error.message);
    assert!(Arc::ptr_eq(
        &published,
        &system.capture_package_queries().unwrap()
    ));
    {
        let state = boot.lock().unwrap();
        assert!(
            state
                .current
                .as_ref()
                .unwrap()
                .dirty_restrictions
                .contains(&0)
        );
    }
    std::fs::remove_dir(&reserve).unwrap();
    // NativeServices::shutdown and app host stop both retire through this gate.
    system.detach_package_bootstrap(&session.bridge).unwrap();
    assert!(boot.lock().unwrap().current.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    drop(session.workers);
}
