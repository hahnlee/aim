//! AppOpsService.setMode/setUidMode and AppOpService at android-16.0.0_r1.
//! Host Binder calls for switch modes, permission checks and paired callbacks.
use super::*;
use crate::package::permissions::AccessUser;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY};

const APP: i32 = 10100;
const USER_ACCESS: &str = "misc_de/0/apexdata/com.android.permission/access.abx";

fn session(test: impl FnOnce(&Arc<AppHost>, &Arc<PermissionManager>, &AppOps)) {
    session_flags(&[(APPOP_MODE_CACHING, true)], test);
}

fn session_flags(
    flags: &[(&str, bool)],
    test: impl FnOnce(&Arc<AppHost>, &Arc<PermissionManager>, &AppOps),
) {
    session_setup(flags, |_| {}, test);
}

fn session_setup(
    flags: &[(&str, bool)],
    setup: impl FnOnce(&Arc<AppHost>),
    test: impl FnOnce(&Arc<AppHost>, &Arc<PermissionManager>, &AppOps),
) {
    crate::system::with_discovery_session_flags(flags, |host| {
        setup(host);
        write(
            host,
            "misc/apexdata/com.android.permission/access.abx",
            b"<access><permissions/></access>",
        );
        write(
            host,
            USER_ACCESS,
            b"<access><app-id-permissions/><extension value='retained'/></access>",
        );
        let owner = Arc::new(PermissionManager::new(host.clone()));
        host.permission_manager.set(owner.clone()).ok().unwrap();
        host.package_system()
            .unwrap()
            .set_native_permissions(&owner);
        super::super::user_service::ensure_system_user(host).unwrap();
        let service = AppOps::new(host.clone(), owner.clone());
        test(host, &owner, &service);
    });
}

fn write(host: &AppHost, name: &str, xml: &[u8]) {
    let path = host.data().unwrap().join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        aim_android_xml::abx::write(&aim_android_xml::read(xml).unwrap()).unwrap(),
    )
    .unwrap();
}

fn disk(host: &AppHost) -> AccessUser {
    let xml =
        aim_android_xml::read(&std::fs::read(host.data().unwrap().join(USER_ACCESS)).unwrap())
            .unwrap();
    assert!(xml.children().any(|e| e.name == "extension"));
    AccessUser::parse(&xml).unwrap()
}

fn set(
    service: &AppOps,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<Option<&str>>,
    mode: i32,
) -> Result<(), Exception> {
    let mut data = Parcel::new();
    let transaction = match package {
        Some(package) => {
            appops::SetMode {
                code,
                uid,
                package_name: package.map(str::to_owned),
                mode,
            }
            .write(&mut data);
            appops::SET_MODE
        }
        None => {
            appops::SetUidMode { code, uid, mode }.write(&mut data);
            appops::SET_UID_MODE
        }
    };
    service
        .transact(&mut Call {
            code: transaction,
            flags: 0,
            sender_pid: 77,
            sender_euid: caller as u32,
            data: data.reader(),
        })
        .unwrap()
        .reader()
        .read_exception()
        .unwrap()
}

#[test]
fn setters_authorize_validate_and_persist_switch_modes() {
    session(|host, owner, service| {
        for package in [None, Some(Some("p"))] {
            assert_eq!(
                set(service, APP, -1, APP, package, MODE_IGNORED)
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            assert_eq!(
                set(service, SYSTEM_UID, -1, APP, package, MODE_IGNORED)
                    .unwrap_err()
                    .code,
                EX_ILLEGAL_ARGUMENT
            );
            let error = set(
                service,
                SYSTEM_UID,
                OP_CAMERA,
                110100,
                package,
                MODE_IGNORED,
            )
            .unwrap_err();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(error.message.contains("#1594"));
        }
        for package in [None, Some(Some("p"))] {
            let error = set(service, SYSTEM_UID, 96, APP, package, MODE_ALLOWED).unwrap_err();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(error.message.contains("#1594"));
        }
        assert!(disk(host).app_id_app_ops.is_empty());
        // GPS switches to COARSE_LOCATION; package and UID writes use that switch.
        set(service, SYSTEM_UID, 2, APP, Some(Some("p")), MODE_IGNORED).unwrap();
        assert_eq!(
            disk(host).package_app_ops,
            [(
                "p".into(),
                vec![("android:coarse_location".into(), MODE_IGNORED)]
            )]
        );
        assert_eq!(owner.package_mode("p", "android:gps").unwrap(), None);
        assert_eq!(mode(host, 2, APP, Some("p")).unwrap(), MODE_IGNORED);
        set(service, SYSTEM_UID, 2, APP, None, MODE_ERRORED).unwrap();
        assert_eq!(mode(host, 2, APP, Some("p")).unwrap(), MODE_ERRORED);
        let reloaded = PermissionManager::new(host.clone());
        assert_eq!(
            reloaded.uid_mode(APP, "android:coarse_location").unwrap(),
            Some(MODE_ERRORED)
        );
        assert_eq!(
            reloaded
                .package_mode("p", "android:coarse_location")
                .unwrap(),
            Some(MODE_IGNORED)
        );
        // The switch's default removes the UID override, exposing the package.
        set(service, SYSTEM_UID, 2, APP, None, MODE_ALLOWED).unwrap();
        assert!(disk(host).app_id_app_ops.is_empty());
        assert_eq!(mode(host, 2, APP, Some("p")).unwrap(), MODE_IGNORED);
        set(service, SYSTEM_UID, 2, APP, Some(Some("p")), MODE_ALLOWED).unwrap();
        assert!(disk(host).package_app_ops.is_empty());
        for package in [None, Some("absent"), Some("q")] {
            set(
                service,
                SYSTEM_UID,
                OP_CAMERA,
                APP,
                Some(package),
                MODE_IGNORED,
            )
            .unwrap();
        }
        assert!(disk(host).package_app_ops.is_empty());
        for package in [None, Some(Some("p"))] {
            assert!(
                set(service, SYSTEM_UID, 87, APP, package, MODE_ALLOWED)
                    .unwrap_err()
                    .message
                    .contains("#1455")
            );
        }
    });
}

#[test]
fn package_auto_revoke_queries_follow_live_appops_modes() {
    use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
    session_setup(
        &[(APPOP_MODE_CACHING, true)],
        |host| {
            // Shell writes consult the original protected provisioning package.
            std::fs::copy(
                aim_paths::original_image().join("system/framework/framework-res.apk"),
                host.root()
                    .unwrap()
                    .join("system/framework/framework-res.apk"),
            )
            .unwrap();
        },
        |host, _, service| {
            let system = host.package_system().unwrap();
            let base = host.package_capture().unwrap();
            let mut context = (**base.context()).clone();
            context.system.system_permissions =
                Some([(2000, [MANAGE_APP_OPS_MODES.into()].into())].into());
            context.scan_version = base.scan().version() + 1;
            system
                .publish_package_scan_with_queries(
                    &system.package_bootstrap().unwrap(),
                    Some(base.scan()),
                    base.scan().owner().clone(),
                    base.scan().usage().clone(),
                    context,
                )
                .unwrap();
            let leaf = super::super::package_bootstrap::PackageBootstrap::new(host.clone());
            let op = super::super::package_initial_context::OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED;
            for value in [MODE_ALLOWED, MODE_IGNORED, MODE_ERRORED, MODE_ALLOWED] {
                set(service, 2000, op, APP, Some(Some("p")), value).unwrap();
                for name in [Some("p"), None, Some("unknown")] {
                    let mut p = Parcel::new();
                    bridge::IsAutoRevokeWhitelisted {
                        calling_uid: APP,
                        package_name: name.map(str::to_owned),
                    }
                    .write(&mut p);
                    let response = leaf
                        .transact(&mut Call {
                            code: bridge::IS_AUTO_REVOKE_WHITELISTED,
                            flags: 0,
                            sender_pid: 77,
                            sender_euid: 1000,
                            data: p.reader(),
                        })
                        .unwrap();
                    let actual =
                        bridge::read_is_auto_revoke_whitelisted_reply(&mut response.reader())
                            .unwrap();
                    assert_eq!(actual, mode(host, op, APP, name).map(|m| m == MODE_IGNORED));
                    if name == Some("p") {
                        assert_eq!(actual.unwrap(), value == MODE_IGNORED);
                    }
                }
            }
        },
    );
}

#[test]
fn setters_preserve_original_started_op_mode_rules_and_finish_pair() {
    session(|host, _, service| {
        let client = Binder::Local(42);
        let op = Unchecked {
            code: OP_CAMERA,
            uid: APP,
            package: Some("p"),
            tag: None,
            proxy: None,
            proxy_tag: None,
            flags: OP_FLAG_SELF,
            collect: None,
        };
        let attribution = Attribution {
            start_if_mode_default: false,
            flags: OP_FLAG_SELF,
            chain_id: ATTRIBUTION_CHAIN_ID_NONE,
        };
        let key = (APP, "p".to_owned(), OP_CAMERA);
        with_ops(host, APP, |ops| ops.start(Some(client), &op, attribution)).unwrap();
        with_ops(host, APP, |ops| ops.start(Some(client), &op, attribution)).unwrap();
        let running = || host.app_ops.started.lock().unwrap().running(&key);
        assert!(running());
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_ERRORED).unwrap();
        assert!(running()); // Original deliberately leaves MODE_ERRORED alone.
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_IGNORED).unwrap();
        assert!(!running());
        assert_eq!(
            host.app_ops.started.lock().unwrap().ops[&key][&None].paused[&client].starts,
            2
        );
        // A restriction left in place prevents resume even after the UID allows it.
        host.app_ops
            .restrictions
            .lock()
            .unwrap()
            .0
            .add_client(1, None);
        host.app_ops
            .restrictions
            .lock()
            .unwrap()
            .0
            .set(1, &[0], OP_CAMERA, true, None);
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_ALLOWED).unwrap();
        assert!(!running());
        host.app_ops
            .restrictions
            .lock()
            .unwrap()
            .0
            .set(1, &[0], OP_CAMERA, false, None);
        // An unchanged write does not re-evaluate starts.
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_ALLOWED).unwrap();
        assert!(!running());
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_FOREGROUND).unwrap();
        assert!(running());
        // A package mode that no longer permits the op is skipped by the
        // original updateStartedOpModeForUid, including on its own callback.
        set(
            service,
            SYSTEM_UID,
            OP_CAMERA,
            APP,
            Some(Some("p")),
            MODE_IGNORED,
        )
        .unwrap();
        assert!(running());
        set(service, SYSTEM_UID, OP_CAMERA, APP, None, MODE_IGNORED).unwrap();
        assert!(running());
        set(
            service,
            SYSTEM_UID,
            OP_CAMERA,
            APP,
            Some(Some("p")),
            MODE_ALLOWED,
        )
        .unwrap();
        assert!(running());
        with_ops(host, APP, |ops| {
            ops.finish(Some(client), OP_CAMERA, APP, Some("p"), None)
        })
        .unwrap();
        assert!(running());
        with_ops(host, APP, |ops| {
            ops.finish(Some(client), OP_CAMERA, APP, Some("p"), None)
        })
        .unwrap();
        assert!(!running());
        assert!(
            host.app_ops.started.lock().unwrap().ops[&key][&None]
                .paused
                .is_empty()
        );
    });
}

#[test]
fn runtime_mapping_rejects_direct_mode_writes_as_the_original_checking_service_does() {
    session_flags(
        &[
            (APPOP_MODE_CACHING, true),
            (RUNTIME_PERMISSION_APPOPS_MAPPING, true),
        ],
        |host, owner, service| {
            write(host, "misc/apexdata/com.android.permission/access.abx",
            b"<access><permissions><permission name='android.permission.CAMERA' packageName='android' protectionLevel='1' type='0'/></permissions></access>");
            for package in [None, Some(Some("p"))] {
                set(service, SYSTEM_UID, OP_CAMERA, APP, package, MODE_IGNORED).unwrap();
            }
            assert_eq!(owner.uid_mode(APP, "android:camera").unwrap(), None);
            assert_eq!(owner.package_mode("p", "android:camera").unwrap(), None);
            assert!(disk(host).app_id_app_ops.is_empty());
            assert!(disk(host).package_app_ops.is_empty());
        },
    );
}

struct ModeListener(Arc<Mutex<Vec<(i32, i32, Option<String>)>>>);
impl Service for ModeListener {
    fn descriptor(&self) -> &str {
        mode_callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, mode_callback::OP_CHANGED);
        let a = mode_callback::OpChanged::read(&mut call.data).unwrap();
        assert_eq!(
            a.persistent_device_id.as_deref(),
            Some(PERSISTENT_DEVICE_ID_DEFAULT)
        );
        self.0.lock().unwrap().push((a.op, a.uid, a.package_name));
        Ok(Parcel::new())
    }
}

#[test]
fn setters_notify_mode_watchers_once_per_change_until_unregistered() {
    session(|host, _, service| {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let callback = host
            .process
            .add_service(Arc::new(ModeListener(heard.clone())));
        service
            .start_watching_mode(SYSTEM_UID, 77, 2, Some("p"), 0, Some(callback))
            .unwrap();
        set(service, SYSTEM_UID, 2, APP, Some(Some("p")), MODE_IGNORED).unwrap();
        set(service, SYSTEM_UID, 2, APP, Some(Some("p")), MODE_IGNORED).unwrap();
        set(service, SYSTEM_UID, 2, APP, None, MODE_ERRORED).unwrap();
        assert_eq!(
            *heard.lock().unwrap(),
            [(2, APP, Some("p".into())), (2, APP, Some("p".into()))]
        );
        let mut data = Parcel::new();
        appops::StopWatchingMode {
            callback: Some(callback),
        }
        .write(&mut data);
        service
            .transact(&mut Call {
                code: appops::STOP_WATCHING_MODE,
                flags: 0,
                sender_pid: 77,
                sender_euid: SYSTEM_UID as u32,
                data: data.reader(),
            })
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        set(service, SYSTEM_UID, 2, APP, None, MODE_ALLOWED).unwrap();
        assert_eq!(heard.lock().unwrap().len(), 2);
    });
}
