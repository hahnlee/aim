use super::*;
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_iservicehost as host,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> std::result::Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> std::result::Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> std::result::Result<File, Errno> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> std::result::Result<u32, Errno> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("fixture does not exchange files")
    }
}
struct Registry {
    names: Mutex<BTreeMap<String, Arc<Strong>>>,
    process: std::sync::Weak<LocalProcess>,
}
impl Service for Registry {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let at = call.data.position();
        let manager = call.data.enforce_interface(sm::DESCRIPTOR).is_ok();
        call.data.set_position(at);
        if !manager {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        let mut reply = Parcel::new();
        match call.code {
            sm::ADD_SERVICE => {
                let args = sm::AddService::read(&mut call.data)?;
                let Binder::Handle(handle) = args.service.unwrap() else {
                    panic!("registry owns remote nodes")
                };
                self.names.lock().unwrap().insert(
                    args.name.unwrap(),
                    Arc::new(self.process.upgrade().unwrap().strong(handle)),
                );
                reply.write_no_exception();
            }
            sm::CHECK_SERVICE => {
                let args = sm::CheckService::read(&mut call.data)?;
                sm::write_check_service_reply(
                    &mut reply,
                    self.names
                        .lock()
                        .unwrap()
                        .get(&args.name.unwrap())
                        .map(|owner| owner.binder()),
                );
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}
struct PermissionReads(std::sync::atomic::AtomicI32);
impl Service for PermissionReads {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_permission_ipermissionmanager::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_permission_ipermissionmanager as permission;
        assert_eq!((call.sender_pid, call.sender_euid), (98240, 2000));
        match call.code {
            permission::CHECK_PERMISSION => {
                let args = permission::CheckPermission::read(&mut call.data)?;
                assert_eq!(
                    (
                        args.package_name.as_deref(),
                        args.permission_name.as_deref(),
                        args.persistent_device_id.as_deref(),
                        args.user_id
                    ),
                    (Some("a.b"), Some("p.camera"), Some("default:0"), 0)
                );
            }
            permission::CHECK_UID_PERMISSION => {
                let args = permission::CheckUidPermission::read(&mut call.data)?;
                assert_eq!(
                    (args.uid, args.permission_name.as_deref(), args.device_id),
                    (99001, Some("p.camera"), 0)
                );
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        assert_eq!(call.data.remaining(), 0);
        let mut reply = Parcel::new();
        let value = self.0.load(std::sync::atomic::Ordering::Relaxed);
        if value == -2 {
            reply.write_exception(&Exception::security("owner denied"));
        } else {
            reply.write_no_exception();
            reply.write_i32(value);
        }
        Ok(reply)
    }
}

struct Original;
impl Service for Original {
    fn descriptor(&self) -> &str {
        bootstrap::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(bootstrap::DESCRIPTOR)?;
        let mut reply = Parcel::new();
        match call.code {
            bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                bootstrap::write_is_test_base_on_bootclasspath_reply(&mut reply, true)
            }
            bootstrap::IS_SIGNING_DEBUGGABLE => {
                bootstrap::write_is_signing_debuggable_reply(&mut reply, false)
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}
fn feature(process: &Arc<LocalProcess>, binder: Binder, name: &str) -> Result<bool> {
    feature_on(process, binder, name, false)
}
fn feature_on(
    process: &Arc<LocalProcess>,
    binder: Binder,
    name: &str,
    native: bool,
) -> Result<bool> {
    use aim_service_aidl::android_content_pm_ipackagemanagernative as api;
    let mut request = Parcel::new();
    let code = if native {
        api::HasSystemFeature {
            feature_name: Some(name.into()),
            version: 1,
        }
        .write(&mut request);
        api::HAS_SYSTEM_FEATURE
    } else {
        pm::HasSystemFeature {
            name: Some(name.into()),
            version: 1,
        }
        .write(&mut request);
        pm::HAS_SYSTEM_FEATURE
    };
    let reply = match binder {
        Binder::Local(pointer) => process
            .local_service(pointer)
            .unwrap()
            .transact(code, &request, false),
        Binder::Handle(handle) => process.strong(handle).transact(code, &request, false),
    }
    .unwrap_or_else(|status| panic!("feature {name} native={native} binder={binder:?}: {status}"));
    pm::read_has_system_feature_reply(&mut reply.reader()).unwrap()
}
fn publish(system: &Arc<System>, bridge: &Arc<crate::package::bootstrap::Bridge>, name: &str) {
    use crate::package::{
        bootstrap::{ApexInventory, ScanUsers},
        model,
        scan::{SigningScan, User},
        scan_snapshot::query_state::Context,
    };
    let mut owner = SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
    let shared = owner
        .identities
        .shared_users
        .keys()
        .map(|name| (name.clone(), Default::default()))
        .collect();
    owner
        .capture_legacy_permissions(&[0], Default::default(), shared)
        .unwrap();
    owner
        .complete_shared_processes(
            owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Vec::new()))
                .collect(),
        )
        .unwrap();
    let context = Context {
        scan_version: 1,
        native_domains: None,
        boot_classes: None,
        nonce: Some(17),
        system: model::System {
            sdk_sandbox_package: Some(None),
            roles: Some(crate::package::roles::Owner::priority_fixture(None)),
            features: vec![(name.into(), 1)],
            ..Default::default()
        },
        platform: Default::default(),
        users: [(
            0,
            model::User {
                id: 0,
                profile_group_id: 0,
                unlocking_or_unlocked: true,
                ..Default::default()
            },
        )]
        .into(),
        apex_inventory: ApexInventory {
            packages: Some(vec![]),
            active: vec![],
        },
        scan_users: ScanUsers {
            users: Some(vec![User {
                id: 0,
                pre_created: false,
                adb_install_disallowed: false,
            }]),
        },
        cross_user_suspensions: false,
        packages: Default::default(),
        retained_packages: Default::default(),
    };
    system
        .publish_package_scan_with_queries(
            bridge,
            None,
            owner,
            crate::package::owner::usage::Usage::new(std::iter::empty::<&str>()),
            context,
        )
        .unwrap();
}
// A recovered native installer, with an actual sessions file. Policy is not
// invoked by these retained-handle reads; fail if dispatch accidentally creates
// another installer or attempts to normalize a session.
struct InstallerFixture {
    data: std::path::PathBuf,
    id: i32,
}
impl InstallerFixture {
    fn new() -> Self {
        use crate::package::installer::{Record, Sessions, codec::SessionParams};
        let data = std::env::temp_dir().join(format!(
            "aim-public-installer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(data.join("system")).unwrap();
        std::fs::create_dir(data.join("app")).unwrap();
        let sessions = Sessions::default();
        let id = sessions
            .create_record(
                Record {
                    params: SessionParams {
                        mode: 1,
                        app_package_name: Some("a.b".into()),
                        ..Default::default()
                    },
                    installer_uid: 1000,
                    user: 0,
                    installer_package: None,
                    installer_attribution_tag: None,
                    created_millis: 1,
                    initiating_package: None,
                    originating_package: None,
                    installer_package_uid: -1,
                },
                true,
            )
            .unwrap();
        let fixture = Self { data, id };
        fixture.disk().write(&sessions.records()).unwrap();
        fixture
    }
    fn disk(&self) -> crate::package::installer::storage::Store {
        use crate::package::installer::storage;
        let inode = aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        };
        storage::Store::open(
            self.data.clone(),
            inode,
            aim_storage::guest_inode::GuestInode {
                mode: Some(0o775),
                ..inode
            },
            Arc::new(|path, guest| {
                use std::os::unix::ffi::OsStrExt;
                assert_eq!(guest, "/data/system/install_sessions.xml.new");
                let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                let label = b"u:object_r:system_data_file:s0\0";
                if unsafe {
                    libc::setxattr(
                        path.as_ptr(),
                        c"dev.aim.xattr.security.selinux".as_ptr(),
                        label.as_ptr().cast(),
                        label.len(),
                        0,
                        libc::XATTR_NOFOLLOW,
                    )
                } != 0
                {
                    return Err(storage::Error {
                        legacy_status: -110,
                        committed: false,
                        message: std::io::Error::last_os_error().to_string(),
                    });
                }
                Ok(())
            }),
        )
        .unwrap()
    }
    fn install(
        &self,
        system: &Arc<System>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Binder {
        use crate::package::installer::{Sessions, endpoint::Endpoint, native::NativeOwners};
        let weak = Arc::downgrade(system);
        let retained = bridge.clone();
        let sessions = Arc::new(Sessions::default());
        let owner = NativeOwners::open(
            sessions.clone(),
            Arc::new(move || {
                Ok(weak
                    .upgrade()
                    .unwrap()
                    .capture_package_queries_for(&retained)?
                    .state()
                    .clone())
            }),
            Arc::new(|_, _, _| panic!("retained-handle read cannot normalize")),
            Default::default(),
            self.disk(),
            {
                let weak = Arc::downgrade(&system.process);
                Arc::new(move |node| Ok(weak.upgrade().unwrap().add_service(node)))
            },
            Arc::new(|_| panic!("retained-handle read cannot mutate sessions")),
            system.process.clone(),
        )
        .unwrap();
        assert_eq!(owner.sessions.records()[0].0.id, self.id);
        let binder = system.process.add_service(Arc::new(Endpoint {
            sessions,
            owners: owner.clone(),
        }));
        system
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_mut()
            .unwrap()
            .installer = Some((owner, binder));
        binder
    }
    fn check(&self, process: &Arc<LocalProcess>, public: Binder, expected: Binder) {
        use aim_service_aidl::android_content_pm_ipackageinstaller as installer;
        let Binder::Local(pointer) = public else {
            panic!("local front")
        };
        let mut request = Parcel::new();
        pm::GetPackageInstaller {}.write(&mut request);
        let reply = process
            .local_service(pointer)
            .unwrap()
            .transact(pm::GET_PACKAGE_INSTALLER, &request, false)
            .unwrap();
        let binder = pm::read_get_package_installer_reply(&mut reply.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(binder, expected);
        let Binder::Local(pointer) = binder else {
            panic!("local installer")
        };
        let mut request = Parcel::new();
        installer::GetSessionInfo {
            session_id: self.id,
        }
        .write(&mut request);
        let node = process.local_service(pointer).unwrap();
        let reply = node
            .transact(installer::GET_SESSION_INFO, &request, false)
            .unwrap();
        let info = installer::read_get_session_info_reply::<
            crate::package::installer::codec::SessionInfo,
        >(&mut reply.reader())
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(info.session_id, self.id);
        assert_eq!(info.app_package_name.as_deref(), Some("a.b"));
    }
}
impl Drop for InstallerFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.data).unwrap();
    }
}
#[test]
fn original_names_stay_same_binder_across_two_epochs_private_reference_retires() {
    let driver = Driver::new();
    let native = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 98201,
            euid: 1000,
            security_context: None,
        },
    );
    let system = System::new(native.clone(), &[]);
    let manager = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 98200,
            euid: 1000,
            security_context: None,
        },
    );
    let registry = Arc::new(Registry {
        names: Mutex::new(BTreeMap::new()),
        process: Arc::downgrade(&manager),
    });
    let Binder::Local(pointer) = manager.add_service(registry.clone()) else {
        panic!("local registry expected")
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: pointer,
        cookie: pointer,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            98202,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    manager.start();
    native.start();
    // The exact app-host nodes issued before construction become the native
    // process-lifetime pair; construction must not replace either descriptor.
    let app_host = crate::app_host::AppHost::new(native.clone());
    app_host.set_system(system.clone());
    let early_public = native.add_service(Arc::new(
        crate::app_host::package_service::PackageService::new(app_host.clone()),
    ));
    let early_native = native.add_service(Arc::new(
        crate::app_host::package_service::PackageService::native(app_host.clone()),
    ));
    system
        .retain_public_package_fronts(early_public, early_native)
        .unwrap();
    assert!(
        system
            .retain_public_package_fronts(early_public, early_native)
            .is_err()
    );
    for (name, binder, is_native) in [
        ("package", early_public, false),
        ("package_native", early_native, true),
    ] {
        system.add_service(name, binder).unwrap();
        assert_eq!(
            feature_on(&native, binder, "epoch.first", is_native)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_ILLEGAL_STATE
        );
    }
    let client = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 98242,
            euid: 2000,
            security_context: None,
        },
    );
    client.start();
    let lookup = |name: &str| {
        let mut request = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut request);
        let reply = client
            .transact(0, sm::CHECK_SERVICE, &request, false)
            .unwrap();
        let Binder::Handle(handle) = sm::read_check_service_reply(&mut reply.reader())
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("remote package front")
        };
        client.strong(handle)
    };
    let before_owners = [lookup("package"), lookup("package_native")];
    let before = [before_owners[0].binder(), before_owners[1].binder()];
    for (binder, is_native) in before.into_iter().zip([false, true]) {
        assert_eq!(
            feature_on(&client, binder, "epoch.first", is_native)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_ILLEGAL_STATE
        );
    }
    let host_node = native.add_service(Arc::new(crate::service_host::ServiceHost::new(
        native.clone(),
        &system,
    )));
    system.add_service("fixture.host", host_node).unwrap();
    let mut originals = Vec::new();
    let mut attach = || {
        let original = LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid: 98210 + originals.len() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        original.start();
        let node = original.add_service(Arc::new(Original));
        let mut request = Parcel::new();
        host::AttachPackageBootstrapBridge { bridge: Some(node) }.write(&mut request);
        let mut lookup = Parcel::new();
        sm::CheckService {
            name: Some("fixture.host".into()),
        }
        .write(&mut lookup);
        let looked = original
            .strong(0)
            .transact(sm::CHECK_SERVICE, &lookup, false)
            .unwrap();
        let Some(Binder::Handle(handle)) = sm::read_check_service_reply(&mut looked.reader())
            .unwrap()
            .unwrap()
        else {
            panic!("host capability expected")
        };
        let reply = original
            .strong(handle)
            .transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE, &request, false)
            .unwrap();
        host::read_attach_package_bootstrap_bridge_reply(&mut reply.reader())
            .unwrap()
            .unwrap();
        originals.push(original);
        system.package_bootstrap().unwrap()
    };
    let first = attach();
    publish(&system, &first, "epoch.first");
    let installer = InstallerFixture::new();
    let first_installer = installer.install(&system, &first);
    // A retained app-host adapter reads constructor role selections from
    // this session without loading its old joined package model.
    let app_host = crate::app_host::AppHost::new(native.clone());
    app_host.set_system(system.clone());
    let adapter = crate::app_host::package_service::PackageService::new(app_host);
    let mut request = Parcel::new();
    pm::GetSetupWizardPackageName {}.write(&mut request);
    let mut call = Call {
        code: pm::GET_SETUP_WIZARD_PACKAGE_NAME,
        flags: 0,
        sender_pid: 98221,
        sender_euid: 1000,
        data: request.reader(),
    };
    let reply = adapter.transact(&mut call).unwrap();
    assert_eq!(
        pm::read_get_setup_wizard_package_name_reply(&mut reply.reader()).unwrap(),
        Ok(None)
    );
    call.sender_euid = 2000;
    call.data = request.reader();
    let reply = adapter.transact(&mut call).unwrap();
    assert_eq!(
        reply.reader().read_exception().unwrap().unwrap_err().code,
        aim_binder_host::parcel::EX_SECURITY
    );

    let one = system.register_public_package_fronts(&first).unwrap();
    assert_eq!((one.public, one.native), (early_public, early_native));
    installer.check(&native, early_public, first_installer);
    installer.check(&native, one.public, first_installer);
    assert!(feature(&native, one.public, "epoch.first").unwrap());
    for (name, binder, is_native) in [
        ("package", before[0], false),
        ("package_native", before[1], true),
    ] {
        assert_eq!(lookup(name).binder(), binder);
        assert!(feature_on(&client, binder, "epoch.first", is_native).unwrap());
    }
    let shell = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 98240,
            euid: 2000,
            security_context: None,
        },
    );
    shell.start();
    let permissions = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 98241,
            euid: 1000,
            security_context: None,
        },
    );
    permissions.start();
    for process in [&native, &permissions] {
        let access = Arc::new(PermissionReads(std::sync::atomic::AtomicI32::new(-1)));
        let mut registration = Parcel::new();
        sm::AddService {
            name: Some("permissionmgr".into()),
            service: Some(process.add_service(access.clone())),
            allow_isolated: false,
            dump_priority: 8,
        }
        .write(&mut registration);
        let registered = process
            .transact(0, sm::ADD_SERVICE, &registration, false)
            .unwrap();
        registered.reader().read_exception().unwrap().unwrap();
        let mut lookup = Parcel::new();
        sm::CheckService {
            name: Some("package".into()),
        }
        .write(&mut lookup);
        let found = shell
            .transact(0, sm::CHECK_SERVICE, &lookup, false)
            .unwrap();
        let Some(Binder::Handle(handle)) = sm::read_check_service_reply(&mut found.reader())
            .unwrap()
            .unwrap()
        else {
            panic!("remote package front")
        };
        let front = shell.strong(handle);
        let capture = system.capture_package_queries().unwrap();
        for value in [-1, 0, -2] {
            access.0.store(value, std::sync::atomic::Ordering::Relaxed);
            let mut request = Parcel::new();
            pm::CheckPermission {
                perm_name: Some("p.camera".into()),
                pkg_name: Some("a.b".into()),
                user_id: 0,
            }
            .write(&mut request);
            let reply = front
                .transact(pm::CHECK_PERMISSION, &request, false)
                .unwrap();
            let read = pm::read_check_permission_reply(&mut reply.reader()).unwrap();
            if value == -2 {
                assert_eq!(read.unwrap_err().message, "owner denied");
            } else {
                assert_eq!(read.unwrap(), value);
            }
            let mut request = Parcel::new();
            pm::CheckUidPermission {
                perm_name: Some("p.camera".into()),
                uid: 99001,
            }
            .write(&mut request);
            let reply = front
                .transact(pm::CHECK_UID_PERMISSION, &request, false)
                .unwrap();
            let read = pm::read_check_uid_permission_reply(&mut reply.reader()).unwrap();
            if value == -2 {
                assert_eq!(read.unwrap_err().message, "owner denied");
            } else {
                assert_eq!(read.unwrap(), value);
            }
            assert!(Arc::ptr_eq(
                &capture,
                &system.capture_package_queries().unwrap()
            ));
        }
    }
    driver.release(permissions.proc_handle());
    driver.release(shell.proc_handle());

    let (private, _) = crate::package::service::PackageQueries::from_bootstrap(&system, &first);
    let private = native.add_service(private);
    let second = attach();
    publish(&system, &second, "epoch.second");
    let next_installer = installer.install(&system, &second);
    assert_ne!(first_installer, next_installer);
    let two = system.register_public_package_fronts(&second).unwrap();
    installer.check(&native, early_public, next_installer);
    installer.check(&native, one.public, next_installer);
    installer.check(&native, two.public, next_installer);
    assert_eq!(one.public, two.public);
    assert_eq!(one.native, two.native);
    for (name, binder) in [("package", two.public), ("package_native", two.native)] {
        let mut lookup = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut lookup);
        let reply = native
            .transact(0, sm::CHECK_SERVICE, &lookup, false)
            .unwrap();
        assert_eq!(
            sm::read_check_service_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            Some(binder)
        );
    }
    for (name, binder, is_native) in [
        ("package", before[0], false),
        ("package_native", before[1], true),
    ] {
        assert_eq!(lookup(name).binder(), binder);
        assert!(feature_on(&client, binder, "epoch.second", is_native).unwrap());
        assert!(!feature_on(&client, binder, "epoch.first", is_native).unwrap());
        // Each descriptor rejects the other one's token, even through the
        // compatibility adapter; UNKNOWN_TRANSACTION is never retried.
        let mut wrong = Parcel::new();
        wrong.write_interface_token(if is_native {
            pm::DESCRIPTOR
        } else {
            aim_service_aidl::android_content_pm_ipackagemanagernative::DESCRIPTOR
        });
        let Binder::Handle(handle) = binder else {
            panic!("remote front")
        };
        assert!(client.transact(handle, if is_native {
            aim_service_aidl::android_content_pm_ipackagemanagernative::HAS_SYSTEM_FEATURE
        } else { pm::HAS_SYSTEM_FEATURE }, &wrong, false).is_err());
        assert_eq!(
            client
                .transact(handle, u32::MAX, &Parcel::new(), false)
                .err()
                .unwrap(),
            aim_binder_host::parcel::UNKNOWN_TRANSACTION
        );
    }
    assert!(feature(&native, one.public, "epoch.second").unwrap());
    assert!(!feature(&native, one.public, "epoch.first").unwrap());
    assert!(feature(&native, private, "epoch.second").is_err());
    system.detach_package_bootstrap(&second).unwrap();
    for original in originals {
        driver.release(original.proc_handle());
    }
    driver.release(client.proc_handle());
    driver.release(native.proc_handle());
    driver.release(manager.proc_handle());
}
