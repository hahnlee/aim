//! PackageManagerShellCommand grant/revoke/reset through real Binder calls,
//! android-16.0.0_r1; CtsPermissionTestCases grant/revoke permission contracts.
use super::*;
use crate::app_host::{package_shell_policy::ShellPolicy, permission_manager::PermissionManager};
use aim_binder_host::local::LocalProcess;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY};
use aim_service_aidl::{
    android_os_iservicemanager as sm, android_permission_ionpermissionschangelistener as changed,
    android_permission_ipermissionmanager as permission,
    com_android_internal_os_iresultreceiver as receiver,
    dev_aim_server_ipackageshellpolicybridge as policy,
};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;

const CAMERA: &str = "android.permission.CAMERA";
const NORMAL: &str = "fixture.NORMAL";
const SIGNATURE: &str = "fixture.SIGNATURE";
const SYSTEM_ACCESS: &str = "misc/apexdata/com.android.permission/access.abx";
const USER_ACCESS: &str = "misc_de/0/apexdata/com.android.permission/access.abx";

// The existing session's context manager still handles its bootstrap protocol;
// shell clients can also discover the actual native services in this fixture.
struct ContextManager {
    host: crate::service_host::ServiceHost,
    services: Arc<Mutex<BTreeMap<String, Binder>>>,
}
impl Service for ContextManager {
    fn descriptor(&self) -> &str {
        self.host.descriptor()
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let at = call.data.position();
        let lookup = call.data.enforce_interface(sm::DESCRIPTOR).is_ok();
        call.data.set_position(at);
        if lookup {
            assert_eq!(call.code, sm::CHECK_SERVICE);
            let a = sm::CheckService::read(&mut call.data)?;
            let mut p = Parcel::new();
            sm::write_check_service_reply(
                &mut p,
                self.services
                    .lock()
                    .unwrap()
                    .get(a.name.as_deref().unwrap())
                    .copied(),
            );
            Ok(p)
        } else {
            self.host.transact(call)
        }
    }
}
struct ResultReceiver(mpsc::Sender<i32>);
impl Service for ResultReceiver {
    fn descriptor(&self) -> &str {
        receiver::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, receiver::SEND);
        call.data.enforce_interface(receiver::DESCRIPTOR)?;
        self.0.send(call.data.read_i32()?).unwrap();
        assert_eq!(call.data.read_i32()?, 0);
        Ok(Parcel::new())
    }
}
struct PermissionListener(mpsc::Sender<i32>);
impl Service for PermissionListener {
    fn descriptor(&self) -> &str {
        changed::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, changed::ON_PERMISSIONS_CHANGED);
        call.data.enforce_interface(changed::DESCRIPTOR)?;
        let uid = call.data.read_i32()?;
        assert_eq!(call.data.read_string16()?.as_deref(), Some("default:0"));
        self.0.send(uid).unwrap();
        Ok(Parcel::new())
    }
}
fn write_access(data: &std::path::Path, path: &str, xml: &str) {
    let path = data.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let root = aim_android_xml::read(xml.as_bytes()).unwrap();
    std::fs::write(path, aim_android_xml::abx::write(&root).unwrap()).unwrap();
}
fn leaf(leaf: &dyn Service, uid: u32, code: u32, data: &Parcel) -> Parcel {
    transact(leaf, uid, code, data)
}
fn classify(leaf: &dyn Service, requested: Option<Vec<Option<String>>>) -> Parcel {
    let mut p = Parcel::new();
    policy::RuntimePermissions { requested }.write(&mut p);
    leaf.transact(&mut Call {
        code: policy::RUNTIME_PERMISSIONS,
        flags: 0,
        sender_pid: 77,
        sender_euid: 1000,
        data: p.reader(),
    })
    .unwrap()
}

#[test]
fn runtime_classification_uses_live_definitions_order_duplicates_and_java_null_checks() {
    let data = crate::app_host::test_temp_dir("shell-classification");
    let host = AppHost::new(crate::test_process(1000));
    crate::app_host::configure_test_host(&host, &data, &data, &data, Box::new(|_| None));
    write_access(&data, SYSTEM_ACCESS, &format!("<access><permissions>
        <permission name='{CAMERA}' packageName='p' protectionLevel='1001' type='0'/>
        <permission name='{NORMAL}' packageName='p' protectionLevel='0' type='0'/>
        <permission name='{SIGNATURE}' packageName='p' protectionLevel='22' type='0'/>
        </permissions><permission-trees><permission name='tree' packageName='p' protectionLevel='1' type='0'/></permission-trees></access>"));
    // Classification needs definitions, not a package capture or user state.
    host.services();
    let policy = ShellPolicy(host.clone());
    let names = |names: &[&str]| Some(names.iter().map(|name| Some((*name).into())).collect());
    let p = classify(
        &policy,
        names(&[NORMAL, CAMERA, "unknown", SIGNATURE, CAMERA, "", "tree"]),
    );
    assert_eq!(
        policy::read_runtime_permissions_reply(&mut p.reader())
            .unwrap()
            .unwrap(),
        names(&[CAMERA, CAMERA])
    );
    let p = classify(&policy, Some(vec![]));
    assert_eq!(
        policy::read_runtime_permissions_reply(&mut p.reader())
            .unwrap()
            .unwrap(),
        Some(vec![])
    );
    for requested in [
        None,
        Some(vec![None]),
        Some(vec![Some(CAMERA.into()), None]),
    ] {
        let p = classify(&policy, requested);
        assert_eq!(
            p.reader().read_exception().unwrap().unwrap_err().code,
            EX_NULL_POINTER
        );
    }
    let mut p = Parcel::new();
    policy::RuntimePermissions {
        requested: names(&[CAMERA]),
    }
    .write(&mut p);
    for uid in [0, 2000, 10100] {
        assert_eq!(
            leaf(&policy, uid, policy::RUNTIME_PERMISSIONS, &p)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
    }
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn shell_grant_revoke_reset_persist_publish_and_deny_unprivileged_callers() {
    let services = Arc::new(Mutex::new(BTreeMap::new()));
    let registry = services.clone();
    let fixture = Fixture::with_context(|process, system| {
        Arc::new(ContextManager {
            host: crate::service_host::ServiceHost::new(process.clone(), system),
            services: registry,
        })
    });
    let Session {
        fixture,
        host,
        bridge,
        workers,
        ..
    } = session_with_fixture(fixture);
    let image = fixture.root.join("image");
    std::fs::create_dir_all(image.join("system/framework")).unwrap();
    std::fs::write(image.join("system/build.prop"), "ro.build.version.sdk=36\n").unwrap();
    std::fs::write(
        image.join("system/framework/framework-res.apk"),
        include_bytes!("../tests/fixtures/discovery/framework.apk"),
    )
    .unwrap();
    crate::app_host::configure_test_host(
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
    write_access(
        &fixture.root.join("data"),
        SYSTEM_ACCESS,
        &format!(
            "<access><permissions><permission name='{CAMERA}' packageName='p' protectionLevel='1' type='0'/></permissions></access>"
        ),
    );
    write_access(
        &fixture.root.join("data"),
        USER_ACCESS,
        "<access><app-id-permissions/></access>",
    );
    // Install the packages' requests and shell's real SystemConfig assignments.
    let before = fixture.system.capture_package_queries().unwrap();
    let mut owner = before.scan().owner().clone();
    for ps in before.state().packages.values() {
        let mut pkg = (**ps.pkg.as_ref().unwrap()).clone();
        pkg.target_sdk_version = 36;
        pkg.requested_permissions = vec![CAMERA.into()];
        pkg.uses_permissions = vec![crate::package::pkg::UsesPermission {
            name: Some(CAMERA.into()),
            flags: 0,
        }];
        if pkg.package_name == "p" {
            pkg.permissions = vec![crate::package::pkg::Permission {
                component: crate::package::pkg::Component {
                    name: CAMERA.into(),
                    package_name: "p".into(),
                    ..Default::default()
                },
                protection_level: 1,
                ..Default::default()
            }];
        }
        owner.load_test_code(pkg);
    }
    complete_owner(&mut owner);
    let mut context = (**before.context()).clone();
    context.scan_version = before.scan().version() + 1;
    context.system.permission_controller_package = Some(None);
    context.system.system_permissions = Some(
        [(
            2000,
            [
                "android.permission.GRANT_RUNTIME_PERMISSIONS".into(),
                "android.permission.REVOKE_RUNTIME_PERMISSIONS".into(),
                "android.permission.INTERACT_ACROSS_USERS_FULL".into(),
            ]
            .into(),
        )]
        .into(),
    );
    context.system.system_permissions.as_mut().unwrap().insert(
        1041,
        ["android.permission.REVOKE_RUNTIME_PERMISSIONS".into()].into(),
    );
    let mut config = crate::package::system_config::SystemConfig::default();
    config.system_permissions = context.system.system_permissions.clone().unwrap();
    let policies = [("p".into(), true), ("q".into(), true)].into();
    let mut domain_owner = crate::package::domain_verification::owner::Owner::new(
        Default::default(),
        Default::default(),
    );
    for setting in &owner.settings.packages {
        domain_owner
            .add(
                crate::package::domain_verification::owner::Input {
                    id: setting.domain_set_id.as_deref().unwrap(),
                    name: &setting.name,
                    code: Some(&owner.domain_code_owner(setting).unwrap().unwrap().package),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &config,
            )
            .unwrap();
    }
    let domains = crate::package::domain_verification::owner::Boot {
        owner: domain_owner,
        changes: vec![],
    };
    let context = context
        .attach_boot_domains(&owner, domains, &config, &policies)
        .unwrap();
    fixture
        .system
        .package_persistence_owner(&before)
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
            Some(before.scan()),
            owner,
            crate::package::owner::usage::Usage::new(["p", "q"]),
            context,
        )
        .unwrap();
    for (name, service) in host.services() {
        // The fixture's context manager shares the native process; its system
        // lookups use exported owners so Binder never transacts to itself.
        if matches!(name, "permissionmgr" | "activity") {
            let Binder::Local(pointer) = fixture.original[0].add_service(service.clone()) else {
                unreachable!()
            };
            let mut request = Parcel::new();
            request.write_i64(pointer as i64);
            let reply = bridge
                .owner
                .transact(EXPORT_TEST_BINDER, &request, false)
                .unwrap();
            let binder = reply.reader().read_binder().unwrap().unwrap();
            fixture
                .system
                .services
                .lock()
                .unwrap()
                .insert(name, Arc::new(reply.retain_remote_binder(binder).unwrap()));
        }
        services
            .lock()
            .unwrap()
            .insert(name.into(), fixture.native.add_service(service));
    }
    let manager = fixture.system.native_permissions().unwrap();
    let (changed_tx, changed_rx) = mpsc::channel();
    let listener = fixture
        .native
        .add_service(Arc::new(PermissionListener(changed_tx)));
    let mut p = Parcel::new();
    permission::AddOnPermissionsChangeListener {
        listener: Some(listener),
    }
    .write(&mut p);
    leaf(
        manager.as_ref(),
        1000,
        permission::ADD_ON_PERMISSIONS_CHANGE_LISTENER,
        &p,
    )
    .reader()
    .read_exception()
    .unwrap()
    .unwrap();
    let current = fixture.system.capture_package_queries().unwrap();
    fixture
        .system
        .refresh_package_permission_queries(&bridge, &current)
        .unwrap();
    let shell = LocalProcess::open(
        &fixture.driver,
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: 97300,
            euid: 2000,
            security_context: None,
        },
    );
    shell.start();
    let lookup = |process: &Arc<LocalProcess>, name: &str| {
        let mut p = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut p);
        let r = process.transact(0, sm::CHECK_SERVICE, &p, false).unwrap();
        let Binder::Handle(handle) = sm::read_check_service_reply(&mut r.reader())
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("remote service")
        };
        process.strong(handle)
    };
    let package = lookup(&shell, "package");
    let permissions = lookup(&shell, "permissionmgr");
    let (tx, rx) = mpsc::channel();
    let result = shell.add_service(Arc::new(ResultReceiver(tx)));
    let path = fixture.root.join("shell-output");
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut command = |args: &[&str]| {
        output.set_len(0).unwrap();
        output.seek(SeekFrom::Start(0)).unwrap();
        let mut p = Parcel::new();
        for _ in 0..3 {
            p.write_file(aim_binder_host::server::file_from_fd(output.as_fd()).unwrap());
        }
        p.write_i32(args.len() as i32);
        for arg in args {
            p.write_string16(Some(arg));
        }
        p.write_binder(None);
        p.write_binder(Some(result));
        let r = package
            .transact(crate::shell::SHELL_COMMAND_TRANSACTION, &p, false)
            .unwrap();
        r.reader().read_exception().unwrap().unwrap();
        let status = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut text = String::new();
        output.seek(SeekFrom::Start(0)).unwrap();
        output.read_to_string(&mut text).unwrap();
        assert_eq!(status, 0, "{args:?}: {text}");
    };
    let check = |expected| {
        let access = manager.retained_access().unwrap();
        let state = fixture.system.capture_package_queries().unwrap();
        assert_eq!(
            access.user0.permission_flags(10100, CAMERA) & (1 << 4) != 0,
            expected == 0,
            "live flags {:?}, p={:?}",
            access.user0.app_id_permissions,
            state.state().packages["p"]
                .pkg
                .as_ref()
                .map(|p| (p.uid, p.target_sdk_version))
        );
        let mut p = Parcel::new();
        permission::CheckPermission {
            package_name: Some("p".into()),
            permission_name: Some(CAMERA.into()),
            persistent_device_id: Some("default:0".into()),
            user_id: 0,
        }
        .write(&mut p);
        let r = permissions
            .transact(permission::CHECK_PERMISSION, &p, false)
            .unwrap();
        assert_eq!(
            permission::read_check_permission_reply(&mut r.reader())
                .unwrap()
                .unwrap(),
            expected,
            "live flags {:?}, user {:?}",
            access.user0.app_id_permissions,
            state.state().packages["p"].users[&0]
        );
        let mut p = Parcel::new();
        pm::CheckPermission {
            perm_name: Some(CAMERA.into()),
            pkg_name: Some("p".into()),
            user_id: 0,
        }
        .write(&mut p);
        let r = package.transact(pm::CHECK_PERMISSION, &p, false).unwrap();
        assert_eq!(
            pm::read_check_permission_reply(&mut r.reader())
                .unwrap()
                .unwrap(),
            expected
        );
        let capture = fixture.system.capture_package_queries().unwrap();
        assert_eq!(
            capture.state().packages["p"].users[&0]
                .granted_permissions
                .iter()
                .any(|p| p == CAMERA),
            expected == 0
        );
        assert_eq!(
            PermissionManager::new(host.clone())
                .retained_access()
                .unwrap()
                .user0
                .permission_flags(10100, CAMERA)
                & (1 << 4)
                != 0,
            expected == 0
        );
    };
    check(-1);
    command(&["grant", "p", CAMERA]);
    check(0);
    assert_eq!(
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        10100
    );
    command(&["revoke", "p", CAMERA]);
    check(-1);
    assert_eq!(
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        10100
    );
    command(&["grant", "--all-permissions", "p"]);
    check(0);
    assert_eq!(
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        10100
    );
    command(&["grant", "--all-permissions"]);
    assert_eq!(
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        10101
    );
    command(&["reset-permissions"]);
    check(-1);
    let mut reset_uids = [
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        changed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
    ];
    reset_uids.sort();
    assert_eq!(reset_uids, [10100, 10101]);
    assert_eq!(
        manager
            .retained_access()
            .unwrap()
            .user0
            .permission_flags(10101, CAMERA),
        0
    );
    // The actual mutation leaf refuses both operations from an app's Binder UID.
    let app = LocalProcess::open(
        &fixture.driver,
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: 97301,
            euid: 10100,
            security_context: None,
        },
    );
    app.start();
    let denied = lookup(&app, "permissionmgr");
    for granted in [true, false] {
        let mut p = Parcel::new();
        permission::GrantRuntimePermission {
            package_name: Some("p".into()),
            permission_name: Some(CAMERA.into()),
            persistent_device_id: Some("default:0".into()),
            user_id: 0,
        }
        .write(&mut p);
        if !granted {
            p.write_string16(None);
        }
        let r = denied
            .transact(
                if granted {
                    permission::GRANT_RUNTIME_PERMISSION
                } else {
                    permission::REVOKE_RUNTIME_PERMISSION
                },
                &p,
                false,
            )
            .unwrap();
        let error = r.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(error.code, EX_SECURITY);
        assert!(error.message.contains(if granted {
            "GRANT_RUNTIME_PERMISSIONS"
        } else {
            "REVOKE_RUNTIME_PERMISSIONS"
        }));
    }
    let policy = ShellPolicy(host.clone());
    for (pid, uid, exception) in [
        (0, 2000, aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT),
        (77, -1, aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT),
        (77, 10100, EX_SECURITY),
        (77, 1041, EX_SECURITY),
    ] {
        let mut p = Parcel::new();
        policy::ResetRuntimePermissions {
            caller_pid: pid,
            caller_uid: uid,
        }
        .write(&mut p);
        assert_eq!(
            leaf(&policy, 1000, policy::RESET_RUNTIME_PERMISSIONS, &p)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            exception
        );
    }
    let mut p = Parcel::new();
    policy::ResetRuntimePermissions {
        caller_pid: 77,
        caller_uid: 0,
    }
    .write(&mut p);
    for uid in [0, 2000, 10100] {
        assert_eq!(
            leaf(&policy, uid, policy::RESET_RUNTIME_PERMISSIONS, &p)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
    }
    check(-1);
    assert!(changed_rx.try_recv().is_err());
    fixture.driver.release(app.proc_handle());
    fixture.driver.release(shell.proc_handle());
    drop(workers);
}
