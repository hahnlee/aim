//! PMS.systemReady's default grants before AMS opens process startup, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use aim_binder_host::local::{Call, Reply, Service};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
use aim_service_aidl::dev_aim_server_ipackagedefaultgrantqueries as queries;

impl PermissionManager {
    pub(in crate::app_host) fn system_ready(&self) -> Result<(), Exception> {
        let fingerprint =
            super::super::package_bootstrap::build_fingerprint(|name| self.host.property(name));
        if self
            .user0()?
            .default_permission_grant_fingerprint
            .as_deref()
            == Some(&fingerprint)
        {
            return Ok(());
        }
        // DefaultPermissionGrantPolicy.apply returns after its synchronous Binder
        // mutations. Hold no access lock across Java's calls back into this owner.
        let owner = self
            .host
            .package_bootstrap_java
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "default permission grant owner unavailable",
                )
            })?;
        let queries = self
            .host
            .process
            .add_service(Arc::new(DefaultGrantQueries(self.host.clone())));
        let mut request = Parcel::new();
        bridge::GrantDefaultPermissions {
            queries: Some(queries),
            user_id: 0,
        }
        .write(&mut request);
        let response = owner
            .transact(bridge::GRANT_DEFAULT_PERMISSIONS, &request, false)
            .map_err(|s| {
                Exception::new(EX_ILLEGAL_STATE, format!("default permission grants: {s}"))
            })?;
        bridge::read_grant_default_permissions_reply(&mut response.reader()).map_err(|s| {
            Exception::new(EX_ILLEGAL_STATE, format!("default permission grants: {s}"))
        })??;
        // Retain the state after the callbacks, never the pre-grant snapshot.
        let mut held = self.owner.lock().unwrap();
        let mut after = (**held.user0.as_ref().expect("loaded")).clone();
        after.default_permission_grant_fingerprint = Some(fingerprint);
        self.write_user0(&after)?;
        held.user0 = Some(Arc::new(after));
        drop(held);
        self.grants_changed()
    }
}

struct DefaultGrantQueries(Arc<AppHost>);
impl Service for DefaultGrantQueries {
    fn descriptor(&self) -> &str {
        queries::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID as u32 {
            return super::super::reply(Err(Exception::security(
                "default grant queries require system uid",
            )));
        }
        let result = (|| {
            let capture = self.0.package_capture()?;
            let state = capture.state();
            let mut reply = Parcel::new();
            match call.code {
                queries::GET_KNOWN_PACKAGE_NAMES => {
                    let a = queries::GetKnownPackageNames::read(&mut call.data)
                        .map_err(|_| Exception::illegal_argument("known packages"))?;
                    let resolution = super::super::package_resolution(&capture, "default grants")?;
                    let query = crate::package::query::Query {
                        state,
                        filter: &resolution.apps_filter,
                        calling_uid: SYSTEM_UID,
                    };
                    let roles = state.system.roles.as_ref().ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "default grant known packages unavailable")
                    })?;
                    let names = roles
                        .known_packages(&query, a.kind, a.user_id)
                        .map_err(not_modelled)?;
                    queries::write_get_known_package_names_reply(&mut reply, &Some(names));
                }
                queries::GET_DISABLED_SYSTEM_PACKAGE_NAME => {
                    let a = queries::GetDisabledSystemPackageName::read(&mut call.data)
                        .map_err(|_| Exception::illegal_argument("disabled package"))?;
                    let name = a
                        .package_name
                        .as_deref()
                        .and_then(|n| state.disabled_system_packages.get(n))
                        .and_then(|p| p.pkg.as_deref())
                        .map(|p| p.package_name.clone());
                    queries::write_get_disabled_system_package_name_reply(&mut reply, &name);
                }
                queries::IS_PLATFORM_SIGNED => {
                    let a = queries::IsPlatformSigned::read(&mut call.data)
                        .map_err(|_| Exception::illegal_argument("platform signing"))?;
                    let signed = if let Some(ps) = a
                        .package_name
                        .as_deref()
                        .and_then(|n| state.packages.get(n))
                    {
                        let signing = ps.signatures.as_ref().ok_or_else(|| {
                            Exception::new(
                                EX_ILLEGAL_STATE,
                                "default grant signing owner unavailable",
                            )
                        })?;
                        let platform = state
                            .packages
                            .get("android")
                            .and_then(|p| p.signatures.as_ref())
                            .ok_or_else(|| {
                                Exception::new(
                                    EX_ILLEGAL_STATE,
                                    "platform signing owner unavailable",
                                )
                            })?;
                        let signing = crate::package::sign::History::saved(signing);
                        let platform = crate::package::sign::History::saved(platform);
                        signing.has_ancestor_or_self(&platform)
                            || platform.check_capability(&signing, 4)
                    } else {
                        false
                    };
                    queries::write_is_platform_signed_reply(&mut reply, signed);
                }
                queries::IS_RESOLVE_ACTIVITY_COMPONENT => {
                    let a = queries::IsResolveActivityComponent::read(&mut call.data)
                        .map_err(|_| Exception::illegal_argument("resolver component"))?;
                    let resolver = state.system.resolver_owner.as_ref().ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "resolver owner unavailable")
                    })?;
                    let item = &resolver.activity.info.item;
                    queries::write_is_resolve_activity_component_reply(
                        &mut reply,
                        item.package_name == a.package_name && item.name == a.class_name,
                    );
                }
                _ => return Err(Exception::illegal_argument("unknown default grant query")),
            }
            Ok(reply)
        })();
        super::super::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{Binder, EX_SECURITY};
    use aim_service_aidl::android_content_icontentservice as cs;
    use std::sync::mpsc;
    const READ: &str = "android.permission.READ_CONTACTS";
    const WRITE: &str = "android.permission.WRITE_CONTACTS";

    // ContentService's observer permission check, with the boot interleaving
    // from #2361. PMS 4280-4317 and AMS 9102-9109 at android-16.0.0_r1:
    // default grants complete before any persistent app can run its observer check.
    fn packages(host: &Arc<AppHost>) {
        use crate::package::{pkg, restrictions, scan, settings};
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let names = [
            "com.google.android.bluetooth",
            "com.android.bluetooth",
            "contacts",
            "ungranted",
            "android",
        ];
        let mut saved = base.scan().owner().settings.clone();
        let template = saved.packages[0].clone();
        saved.packages.clear();
        for (index, name) in names.iter().enumerate() {
            let mut ps = template.clone();
            ps.name = (*name).into();
            ps.app_id = if index < 2 {
                1002
            } else if index == 2 {
                10100
            } else if index == 3 {
                10123
            } else {
                1000
            };
            ps.shared_user = index < 2 || index == 4;
            ps.shared_user_app_id = ps.shared_user.then_some(ps.app_id);
            ps.flags = if index != 3 { settings::FLAG_SYSTEM } else { 0 };
            ps.code_path = format!("/system/app/{name}");
            ps.domain_set_id = Some(format!("00000000-0000-0000-0000-{:012}", index + 1));
            saved.packages.push(ps);
        }
        saved.shared_users = vec![settings::SharedUser {
            name: "android.uid.bluetooth".into(),
            app_id: 1002,
            flags: settings::FLAG_SYSTEM,
            signatures: None,
        }];
        let mut owner = scan::SigningScan::new(&Default::default(), &saved, 36).unwrap();
        owner
            .capture_user_states(
                names
                    .map(|name| {
                        (
                            (name.into(), false),
                            scan::CapturedUsers {
                                states: BTreeMap::from([(
                                    0,
                                    restrictions::UserState {
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
        owner
            .capture_legacy_permissions(
                &[0],
                names
                    .map(|n| ((n.into(), false), Default::default()))
                    .into(),
                owner
                    .identities
                    .shared_users
                    .keys()
                    .map(|name| (name.clone(), Default::default()))
                    .collect(),
            )
            .unwrap();
        owner
            .capture_install_permissions_fixed(names.map(|n| ((n.into(), false), false)).into())
            .unwrap();
        let mut context = (**base.context()).clone();
        let template = context.packages[&("p".into(), false)].clone();
        context.packages.clear();
        for (index, name) in names.iter().enumerate() {
            let mut ps = template.clone();
            ps.app_id = if index < 2 {
                1002
            } else if index == 2 {
                10100
            } else if index == 3 {
                10123
            } else {
                1000
            };
            ps.path = format!("/system/app/{name}");
            context.packages.insert(((*name).into(), false), ps);
            let mut code = pkg::AndroidPackage {
                package_name: (*name).into(),
                path: Some(format!("/system/app/{name}")),
                uid: if index < 2 {
                    1002
                } else if index == 2 {
                    10100
                } else if index == 3 {
                    10123
                } else {
                    1000
                },
                target_sdk_version: 36,
                booleans: pkg::booleans::ENABLED
                    | pkg::booleans::HAS_CODE
                    | if index != 3 { pkg::booleans::SYSTEM } else { 0 }
                    | if index < 2 {
                        pkg::booleans::PERSISTENT
                    } else {
                        0
                    },
                feature_flag_state: Some(vec![]),
                requested_permissions: vec![READ.into(), WRITE.into()],
                ..Default::default()
            };
            if index < 2 {
                code.shared_user_id = Some("android.uid.bluetooth".into());
            } else if index == 2 {
                let mut provider = pkg::Provider::default();
                provider.main.component.name =
                    "com.android.providers.contacts.ContactsProvider2".into();
                provider.main.component.package_name = (*name).into();
                provider.main.enabled = true;
                provider.main.exported = true;
                provider.authority = Some("com.android.contacts".into());
                provider.read_permission = Some(READ.into());
                provider.write_permission = Some(WRITE.into());
                code.providers.push(provider);
            }
            if index == 4 {
                code.shared_user_id = Some("android.uid.system".into());
                code.requested_permissions.clear();
                code.permissions = [READ, WRITE]
                    .map(|name| pkg::Permission {
                        component: pkg::Component {
                            name: name.into(),
                            package_name: "android".into(),
                            ..Default::default()
                        },
                        protection_level: 1,
                        ..Default::default()
                    })
                    .to_vec();
            }
            owner.load_test_code(code);
        }
        owner
            .assign_seinfo_at_boot(
                &crate::package::owner::seinfo::Policy::parse(&[]).unwrap(),
                &mut |_| Ok(36),
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
                            scan::ReplicaRuntime {
                                usage: [0; 8],
                                seinfo: owner.seinfo_state(name).unwrap().unwrap().base.clone(),
                                override_seinfo: owner
                                    .seinfo_state(name)
                                    .unwrap()
                                    .unwrap()
                                    .override_label
                                    .clone(),
                                library_files: vec![],
                                libraries: vec![],
                            },
                        )
                    })
                    .into(),
            )
            .unwrap();
        context.scan_version += 1;
        context.system.permission_controller_package = Some(None);
        let config = crate::package::system_config::SystemConfig::default();
        let mut domains = crate::package::domain_verification::owner::Owner::new(
            Default::default(),
            Default::default(),
        );
        let mut policies = BTreeMap::new();
        for ps in &owner.settings.packages {
            domains
                .add(
                    crate::package::domain_verification::owner::Input {
                        id: ps.domain_set_id.as_deref().unwrap(),
                        name: &ps.name,
                        code: Some(owner.loaded_packages()[&ps.name].runtime_package()),
                        signatures: &[],
                        system: true,
                        restrict_domains: true,
                        pre_verified: None,
                    },
                    &config,
                )
                .unwrap();
            policies.insert(ps.name.clone(), true);
        }
        context = context
            .attach_boot_domains(
                &owner,
                crate::package::domain_verification::owner::Boot {
                    owner: domains,
                    changes: vec![],
                },
                &config,
                &policies,
            )
            .unwrap();
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                owner,
                crate::package::owner::usage::Usage::new(names),
                context,
            )
            .unwrap();
    }

    #[test]
    fn default_grant_queries_require_the_system_uid() {
        let host = AppHost::new(crate::test_process(1000));
        let queries = DefaultGrantQueries(host.clone());
        for uid in [0, 1002, 2000, 10123] {
            let request = Parcel::new();
            let response = queries
                .transact(&mut Call {
                    code: queries::GET_KNOWN_PACKAGE_NAMES,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: uid,
                    data: request.reader(),
                })
                .unwrap();
            assert_eq!(
                response
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
        }
        host.process.shutdown().unwrap();
    }

    struct Policy {
        owner: Arc<PermissionManager>,
        entered: mpsc::Sender<()>,
        commit: Mutex<mpsc::Receiver<()>>,
        fail: bool,
    }
    impl Service for Policy {
        fn descriptor(&self) -> &str {
            bridge::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, bridge::GRANT_DEFAULT_PERMISSIONS);
            assert_eq!(call.sender_euid, 1000);
            let args = bridge::GrantDefaultPermissions::read(&mut call.data)?;
            assert_eq!(args.user_id, 0);
            assert!(args.queries.is_some());
            self.entered.send(()).unwrap();
            self.commit.lock().unwrap().recv().unwrap();
            if self.fail {
                return super::super::super::reply(Err(Exception::security("policy failed")));
            }
            // The original policy uses these same synchronous native Binder mutations.
            for permission in [READ, WRITE] {
                let mut request = Parcel::new();
                pm::GrantRuntimePermission {
                    package_name: Some("com.google.android.bluetooth".into()),
                    permission_name: Some(permission.into()),
                    persistent_device_id: Some("default:0".into()),
                    user_id: 0,
                }
                .write(&mut request);
                let response = self.owner.transact(&mut Call {
                    code: pm::GRANT_RUNTIME_PERMISSION,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: 1000,
                    data: request.reader(),
                })?;
                pm::read_grant_runtime_permission_reply(&mut response.reader())
                    .unwrap()
                    .unwrap();
                let mut request = Parcel::new();
                pm::UpdatePermissionFlags {
                    package_name: Some("com.google.android.bluetooth".into()),
                    permission_name: Some(permission.into()),
                    flag_mask: (1 << 4) | (1 << 5),
                    flag_values: (1 << 4) | (1 << 5),
                    check_adjust_policy_flag_permission: false,
                    persistent_device_id: Some("default:0".into()),
                    user_id: 0,
                }
                .write(&mut request);
                let response = self.owner.transact(&mut Call {
                    code: pm::UPDATE_PERMISSION_FLAGS,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: 1000,
                    data: request.reader(),
                })?;
                pm::read_update_permission_flags_reply(&mut response.reader())
                    .unwrap()
                    .unwrap();
            }
            let mut response = Parcel::new();
            bridge::write_grant_default_permissions_reply(&mut response);
            Ok(response)
        }
    }
    struct Observer;
    impl Service for Observer {
        fn descriptor(&self) -> &str {
            "android.database.IContentObserver"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            panic!("no content change expected")
        }
    }

    fn register(host: &Arc<AppHost>, uid: u32, observer: u32) -> Result<(), Exception> {
        let service = super::super::super::content_service::ContentService::new(host.clone());
        let mut request = Parcel::new();
        request.write_interface_token(cs::DESCRIPTOR);
        request.write_i32(1);
        crate::package::uri::Uri::parse("content://com.android.contacts/contacts")
            .write(&mut request);
        request.write_bool(true);
        request.write_binder(Some(Binder::Handle(observer)));
        request.write_i32(0);
        request.write_i32(36);
        let response = service
            .transact(&mut Call {
                code: cs::REGISTER_CONTENT_OBSERVER,
                flags: 0,
                sender_pid: 77,
                sender_euid: uid,
                data: request.reader(),
            })
            .unwrap();
        cs::read_register_content_observer_reply(&mut response.reader()).unwrap()
    }

    fn run(fail: bool) {
        crate::system::with_boot_discovery_binders(|host, _, _, export| {
            packages(host);
            let observer = export(Arc::new(Observer));
            let observer = observer.handle;
            let owner = Arc::new(PermissionManager::with_system(
                host.clone(),
                AccessSystem {
                    permissions: [READ, WRITE]
                        .map(|name| Permission {
                            name: name.into(),
                            package: "android".into(),
                            protection_level: 1,
                            dynamic: None,
                        })
                        .to_vec(),
                    ..Default::default()
                },
            ));
            owner.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser::default()));
            assert!(host.permission_manager.set(owner.clone()).is_ok());
            host.package_system()
                .unwrap()
                .set_native_permissions(&owner);
            assert_eq!(
                host.package_capture().unwrap().state().shared_users["android.uid.bluetooth"]
                    .packages
                    .len(),
                2
            );
            let before = owner.retained_access().unwrap();
            let denied = register(host, 1002, observer).unwrap_err();
            assert_eq!(denied.code, EX_SECURITY);
            assert!(denied.message.contains("ContactsProvider2 (uid=1002) requires android.permission.READ_CONTACTS or android.permission.WRITE_CONTACTS"), "{denied:?}");
            let (entered, waiting) = mpsc::channel();
            let (commit, continue_policy) = mpsc::channel();
            let policy = export(Arc::new(Policy {
                owner: owner.clone(),
                entered,
                commit: Mutex::new(continue_policy),
                fail,
            }));
            *host.package_bootstrap_java.lock().unwrap() = Some(Arc::new(policy));
            let boot = host.clone();
            let worker = std::thread::spawn(move || boot.system_ready());
            waiting.recv().unwrap();
            assert!(host.processes_ready.get().is_none());
            assert!(register(host, 1002, observer).is_err());
            // A start queued before policy.apply cannot run its application check.
            let app = host.clone();
            let (started, start_queued) = mpsc::channel();
            let early = std::thread::spawn(move || {
                started.send(()).unwrap();
                let start = app.start_package_process(
                    "com.google.android.bluetooth",
                    "com.google.android.bluetooth",
                    None,
                    false,
                    false,
                );
                app.processes_ready
                    .get()
                    .expect("startup bypassed readiness")
                    .clone()?;
                // This host fixture has no zygote; the pending start has reached
                // launch input/relay validation, after permission readiness.
                assert!(start.is_err());
                register(&app, 1002, observer)
            });
            start_queued.recv().unwrap();
            commit.send(()).unwrap();
            let ready = worker.join().unwrap();
            let observed = early.join().unwrap();
            if fail {
                assert_eq!(ready.unwrap_err().message, "policy failed");
                assert_eq!(observed.unwrap_err().message, "policy failed");
                assert!(owner.retained_access().unwrap().same_base(&before));
                assert!(owner.access_writes.lock().unwrap().is_empty());
            } else {
                ready.unwrap();
                observed.unwrap();
                assert_eq!(
                    register(host, 10123, observer).unwrap_err().code,
                    EX_SECURITY
                );
                for name in ["com.google.android.bluetooth", "com.android.bluetooth"] {
                    assert!(owner.package_holds(name, READ, 0).unwrap());
                }
                assert_eq!(before.user0.permission_flags(1002, READ), 0);
                let disk = std::fs::read(host.data().unwrap().join(USER0_ACCESS)).unwrap();
                let saved = AccessUser::parse(&aim_android_xml::read(&disk).unwrap()).unwrap();
                assert_eq!(
                    saved.permission_flags(1002, READ),
                    RUNTIME_GRANTED | SYSTEM_FIXED | PREGRANT
                );
                assert_eq!(
                    saved.default_permission_grant_fingerprint,
                    owner.user0().unwrap().default_permission_grant_fingerprint
                );
                assert_eq!(
                    saved.default_permission_grant_fingerprint.as_deref(),
                    Some("unknown/unknown/unknown:unknown/unknown/unknown:unknown/unknown")
                );
                let reload = PermissionManager::new(host.clone()).user0().unwrap();
                assert_eq!(*reload, *owner.user0().unwrap());
                let writes = owner.access_writes.lock().unwrap().len();
                host.system_ready().unwrap();
                assert_eq!(owner.access_writes.lock().unwrap().len(), writes);
            }
        });
    }
    #[test]
    fn contacts_observer_waits_for_committed_shared_uid_default_grants() {
        run(false);
    }
    #[test]
    fn failed_default_grants_do_not_open_process_startup() {
        run(true);
    }
}
