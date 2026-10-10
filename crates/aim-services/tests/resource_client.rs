//! Native cleanup over generated Binder calls; the endpoint owns disposable files.
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service},
    parcel::{Binder, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{android_os_iinstalld as installd, android_os_iservicemanager as sm};
use aim_services::package::owner::{resources::CodeResources, sdk_data::SdkData};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<File, Errno> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> Result<u32, Errno> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("unexpected file descriptor");
    }
}
struct Registry(Binder, &'static str);
impl Service for Registry {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != sm::CHECK_SERVICE {
            return Err(UNKNOWN_TRANSACTION);
        }
        let name = sm::CheckService::read(&mut call.data)?.name;
        let mut reply = Parcel::new();
        sm::write_check_service_reply(
            &mut reply,
            (name.as_deref() == Some(self.1)).then_some(self.0),
        );
        Ok(reply)
    }
}
struct SdkRecord(SdkData);
impl aim_service_aidl::ReadParcelable for SdkRecord {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        let args = SdkData {
            uuid: r.read_string16()?,
            package_name: r.read_string16()?,
            sub_dir_names: aim_service_aidl::read_string_list(r)?,
            user_id: r.read_i32()?,
            app_id: r.read_i32()?,
            previous_app_id: r.read_i32()?,
            se_info: r.read_string16()?,
            flags: r.read_i32()?,
        };
        if size < 4 || r.position() - start != size as usize {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(Self(args))
    }
}
struct Installer {
    data: PathBuf,
    calls: Mutex<Vec<(String, String)>>,
    reject_parent: Mutex<bool>,
    data_calls: Mutex<Vec<(String, i32, i32, i64)>>,
    reject_user: Mutex<Option<i32>>,
    sdk_calls: Mutex<Vec<SdkData>>,
    reject_sdk: Mutex<bool>,
}
impl Service for Installer {
    fn descriptor(&self) -> &str {
        installd::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == installd::RECONCILE_SDK_DATA {
            assert_eq!(call.sender_euid, 1000);
            let args = installd::ReconcileSdkData::<SdkRecord>::read(&mut call.data)?
                .args
                .unwrap()
                .0;
            assert_eq!(call.data.remaining(), 0);
            self.sdk_calls.lock().unwrap().push(args);
            let mut reply = Parcel::new();
            if *self.reject_sdk.lock().unwrap() {
                let mut error = Exception::new(-8, "SDK filesystem failure");
                error.service_specific = 73;
                reply.write_exception(&error);
            } else {
                installd::write_reconcile_sdk_data_reply(&mut reply);
            }
            return Ok(reply);
        }
        if call.code == installd::DESTROY_APP_DATA {
            assert_eq!(call.sender_euid, 1000);
            let request = installd::DestroyAppData::read(&mut call.data)?;
            assert!(request.uuid.is_none());
            let name = request.package_name.unwrap();
            self.data_calls.lock().unwrap().push((
                name.clone(),
                request.user_id,
                request.flags,
                request.ce_data_inode,
            ));
            let mut reply = Parcel::new();
            if *self.reject_user.lock().unwrap() == Some(request.user_id) {
                reply.write_exception(&Exception::new(-8, "installer data failure"));
            } else {
                for (directory, flag) in [("user", 2), ("user_de", 1)] {
                    if request.flags & flag != 0 {
                        let path = self
                            .data
                            .join(directory)
                            .join(request.user_id.to_string())
                            .join(&name);
                        match fs::remove_dir_all(path) {
                            Ok(()) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => panic!("{e}"),
                        }
                    }
                }
                installd::write_destroy_app_data_reply(&mut reply);
            }
            return Ok(reply);
        }
        if call.code != installd::RM_PACKAGE_DIR {
            return Err(UNKNOWN_TRANSACTION);
        }
        assert_eq!(call.sender_euid, 1000);
        let request = installd::RmPackageDir::read(&mut call.data)?;
        let name = request.package_name.unwrap();
        let path = request.package_dir.unwrap();
        self.calls.lock().unwrap().push((name, path.clone()));
        let mut reply = Parcel::new();
        if path.ends_with("/~~native-proof") && *self.reject_parent.lock().unwrap() {
            reply.write_exception(&Exception::new(-8, "installer parent failure"));
        } else {
            fs::remove_dir_all(self.data.join(path.strip_prefix("/data/").unwrap())).unwrap();
            installd::write_rm_package_dir_reply(&mut reply);
        }
        Ok(reply)
    }
}
struct Fixture {
    driver: Arc<Driver>,
    manager: Arc<LocalProcess>,
    client: Arc<LocalProcess>,
    data: PathBuf,
}

struct PermissionGids {
    calls: Mutex<Vec<i32>>,
    reject: Mutex<Option<i32>>,
    result: Mutex<Option<Vec<i32>>>,
}
impl Service for PermissionGids {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_ibridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::dev_aim_server_ibridge as bridge;
        if call.code != bridge::GET_PERMISSION_GIDS_FOR_UID {
            return Err(UNKNOWN_TRANSACTION);
        }
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(bridge::DESCRIPTOR)?;
        let uid = call.data.read_i32()?;
        self.calls.lock().unwrap().push(uid);
        let mut reply = Parcel::new();
        if *self.reject.lock().unwrap() == Some(uid) {
            reply.write_exception(&Exception::security("permission owner denied"));
        } else {
            reply.write_no_exception();
            // Independent primitive int-array encoding, including null.
            match self.result.lock().unwrap().as_ref() {
                None => reply.write_i32(-1),
                Some(gids) => {
                    reply.write_i32(gids.len() as i32);
                    for gid in gids {
                        reply.write_i32(*gid);
                    }
                }
            }
        }
        Ok(reply)
    }
}

#[test]
fn permission_owner_gids_preserve_active_user_order_and_duplicates_without_partial_success() {
    use aim_services::package::owner::permission_gids::{PermissionGidError, query};
    let driver = Driver::new();
    let open = |pid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid: 1000,
                security_context: None,
            },
        )
    };
    let manager = open(93001);
    let client = open(93002);
    let data =
        std::env::temp_dir().join(format!("aim-native-permission-gids-{}", std::process::id()));
    fs::create_dir(&data).unwrap();
    let _fixture = Fixture {
        driver: driver.clone(),
        manager: manager.clone(),
        client: client.clone(),
        data,
    };
    let owner = Arc::new(PermissionGids {
        calls: Mutex::new(Vec::new()),
        reject: Mutex::new(None),
        result: Mutex::new(Some(vec![3003, 3003])),
    });
    let Binder::Local(ptr) = manager.add_service(owner.clone()) else {
        unreachable!()
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            93003,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    manager.start();
    let strong = client.strong(0);
    assert_eq!(
        query(&strong, 19001, &[10, 0]).unwrap(),
        [3003, 3003, 3003, 3003]
    );
    assert_eq!(*owner.calls.lock().unwrap(), [1019001, 19001]);
    owner.calls.lock().unwrap().clear();
    for (app, users) in [
        (-1, vec![0]),
        (19001, vec![]),
        (19001, vec![0, 0]),
        (19001, vec![-1]),
        (19001, vec![i32::MAX]),
    ] {
        assert!(matches!(
            query(&strong, app, &users),
            Err(PermissionGidError::Input(_))
        ));
        assert!(owner.calls.lock().unwrap().is_empty());
    }
    *owner.reject.lock().unwrap() = Some(1019001);
    assert_eq!(
        query(&strong, 19001, &[0, 10, 11]).unwrap_err(),
        PermissionGidError::Owner(Exception::security("permission owner denied"))
    );
    assert_eq!(*owner.calls.lock().unwrap(), [19001, 1019001]);
    *owner.reject.lock().unwrap() = None;
    *owner.result.lock().unwrap() = None;
    assert!(matches!(
        query(&strong, 19001, &[0]),
        Err(PermissionGidError::Input(_))
    ));
    *owner.result.lock().unwrap() = Some(vec![-1]);
    assert!(matches!(
        query(&strong, 19001, &[0]),
        Err(PermissionGidError::Input(_))
    ));
    *owner.result.lock().unwrap() = Some(Vec::new());
    assert!(query(&strong, 19001, &[0]).unwrap().is_empty());
    fs::create_dir_all(_fixture.data.join("system")).unwrap();
    fs::write(_fixture.data.join("system/packages.xml"), b"<packages><package name='app' codePath='/data/app/app' userId='19001' version='7'/></packages>").unwrap();
    let mut store = aim_services::package::owner::Store::open(&_fixture.data, &[0])
        .unwrap()
        .unwrap();
    let rows =
        aim_services::package::list::parse("app 19001 0 /data/user/0/app default 42 0 7 0 @null\n")
            .unwrap();
    *owner.result.lock().unwrap() = Some(vec![3003, 3003]);
    *owner.reject.lock().unwrap() = Some(1019001);
    assert!(
        !store
            .commit_package_list_from_permissions(&rows, &[0, 10], &strong)
            .unwrap_err()
            .committed
    );
    assert!(!_fixture.data.join("system/packages.list").exists());
    assert!(store.state().list.is_empty());
    *owner.reject.lock().unwrap() = None;
    store
        .commit_package_list_from_permissions(&rows, &[10, 0], &strong)
        .unwrap();
    assert_eq!(store.state().list[0].gids, [3003, 3003, 3003, 3003]);
    assert_eq!(rows[0].gids, [42]);
    assert_eq!(
        fs::read_to_string(_fixture.data.join("system/packages.list")).unwrap(),
        "app 19001 0 /data/user/0/app default 3003,3003,3003,3003 0 7 0 @null\n"
    );
    drop(strong);
}

struct Maintenance {
    calls: Mutex<Vec<i64>>,
    keys: Mutex<std::collections::BTreeSet<i64>>,
    reject: Mutex<Option<i64>>,
}
impl Service for Maintenance {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_security_maintenance_ikeystoremaintenance::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_security_maintenance_ikeystoremaintenance as maintenance;
        assert_eq!(call.sender_euid, 1000);
        if call.code != maintenance::CLEAR_NAMESPACE {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Decode the documented primitive wire format independently of the
        // generated reader: an enum is not a nullable typed Parcelable.
        call.data.enforce_interface(maintenance::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 0); // Domain.APP
        let nspace = call.data.read_i64()?;
        self.calls.lock().unwrap().push(nspace);
        let mut reply = Parcel::new();
        if *self.reject.lock().unwrap() == Some(nspace) {
            reply.write_exception(&Exception::new(-8, "keystore namespace failure"));
        } else {
            self.keys.lock().unwrap().remove(&nspace);
            reply.write_no_exception();
        }
        Ok(reply)
    }
}

#[test]
fn generated_keystore_calls_capture_uid_scope_and_preserve_fifo_failure_retry() {
    use aim_services::package::owner::keystore::KeystoreCleanup;
    let driver = Driver::new();
    let open = |pid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid: 1000,
                security_context: None,
            },
        )
    };
    let manager = open(92001);
    let client = open(92002);
    let data = std::env::temp_dir().join(format!("aim-native-keystore-{}", std::process::id()));
    fs::create_dir(&data).unwrap();
    let _fixture = Fixture {
        driver: driver.clone(),
        manager: manager.clone(),
        client: client.clone(),
        data,
    };
    let service = Arc::new(Maintenance {
        calls: Mutex::new(Vec::new()),
        keys: Mutex::new([19001, 1019001, 19002, 19003].into()),
        reject: Mutex::new(Some(1019001)),
    });
    let endpoint = manager.add_service(service.clone());
    let Binder::Local(ptr) =
        manager.add_service(Arc::new(Registry(endpoint, "android.security.maintenance")))
    else {
        unreachable!()
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            92003,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    manager.start();
    let cleanup = KeystoreCleanup::new(client);
    let mut users = vec![0, 10];
    cleanup.post(19001, &users).unwrap();
    users[1] = 11;
    cleanup.post(19002, &[0]).unwrap();
    cleanup.post(19001, &[0]).unwrap();
    assert_eq!(cleanup.pending(), 4);
    for (app_id, users) in [
        (-1, vec![0]),
        (19001, vec![]),
        (19001, vec![0, 0]),
        (19001, vec![-1]),
        (19001, vec![i32::MAX]),
    ] {
        assert!(cleanup.post(app_id, &users).is_err());
        assert_eq!(cleanup.pending(), 4);
    }
    assert!(cleanup.complete_next().unwrap());
    assert!(!service.keys.lock().unwrap().contains(&19001));
    assert!(
        cleanup
            .complete_next()
            .unwrap_err()
            .contains("keystore namespace failure")
    );
    assert_eq!(cleanup.pending(), 3);
    assert!(service.keys.lock().unwrap().contains(&1019001));
    *service.reject.lock().unwrap() = None;
    while cleanup.complete_next().unwrap() {}
    assert_eq!(
        *service.calls.lock().unwrap(),
        [19001, 1019001, 1019001, 19002, 19001]
    );
    assert_eq!(*service.keys.lock().unwrap(), [19003].into());
    assert_eq!(cleanup.pending(), 0);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.driver.release(self.client.proc_handle());
        self.driver.release(self.manager.proc_handle());
        fs::remove_dir_all(&self.data).unwrap();
    }
}
#[test]
fn generated_installer_calls_preserve_failure_and_retry_parent_cleanup() {
    let driver = Driver::new();
    let open = |pid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid: 1000,
                security_context: None,
            },
        )
    };
    let manager = open(91001);
    let client = open(91002);
    let data = std::env::temp_dir().join(format!("aim-native-installer-{}", std::process::id()));
    fs::create_dir(&data).unwrap();
    let fixture = Fixture {
        driver: driver.clone(),
        manager: manager.clone(),
        client: client.clone(),
        data: data.clone(),
    };
    fs::create_dir_all(data.join("app/~~native-proof/package/lib")).unwrap();
    fs::write(
        data.join("app/~~native-proof/package/base.apk"),
        b"disposable code",
    )
    .unwrap();
    let installer = Arc::new(Installer {
        data: data.clone(),
        calls: Mutex::new(Vec::new()),
        reject_parent: Mutex::new(true),
        data_calls: Mutex::new(Vec::new()),
        reject_user: Mutex::new(Some(10)),
        sdk_calls: Mutex::new(Vec::new()),
        reject_sdk: Mutex::new(false),
    });
    let endpoint = manager.add_service(installer.clone());
    let Binder::Local(ptr) = manager.add_service(Arc::new(Registry(endpoint, "installd"))) else {
        unreachable!()
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            91003,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    manager.start();
    let resources = CodeResources::new(client, data.clone(), None);
    use aim_services::package::scan::{
        DataImage, Kind, Location, Partition, Rejected, SigningError, SigningScan,
    };
    let image = DataImage {
        packages: Vec::new(),
        rejected: vec![Rejected {
            location: Location {
                path: "/data/app/~~native-proof/package".into(),
                partition: Partition::Data,
                kind: Kind::App,
                apex: None,
            },
            reason: "invalid disposable package".into(),
        }],
    };
    let incremental = Default::default();
    let error =
        SigningScan::clean_invalid_data_inputs(&image, &resources, &incremental).unwrap_err();
    assert!(
        matches!(error, SigningError::Fatal(ref e) if e.phase == "data-cleanup" && e.message.contains("installer parent failure")),
        "{error:?}"
    );
    assert!(!data.join("app/~~native-proof/package").exists());
    assert!(data.join("app/~~native-proof").exists());
    *installer.reject_parent.lock().unwrap() = false;
    SigningScan::clean_invalid_data_inputs(&image, &resources, &incremental).unwrap();
    assert!(!data.join("app/~~native-proof").exists());
    assert_eq!(
        *installer.calls.lock().unwrap(),
        [
            ("package".into(), "/data/app/~~native-proof/package".into()),
            ("package".into(), "/data/app/~~native-proof".into()),
            ("package".into(), "/data/app/~~native-proof".into()),
        ]
    );
    let sdk = SdkData {
        uuid: None,
        package_name: Some("fixture.sdk.client".into()),
        sub_dir_names: Some(vec![Some("sdk-α".into()), Some("sdk-b".into())]),
        user_id: 10,
        app_id: 19001,
        previous_app_id: 19000,
        se_info: Some("default:targetSdkVersion=36".into()),
        flags: 3,
    };
    resources.reconcile_sdk_data(sdk.clone()).unwrap();
    assert_eq!(*installer.sdk_calls.lock().unwrap(), [sdk.clone()]);
    *installer.reject_sdk.lock().unwrap() = true;
    let error = resources.reconcile_sdk_data(sdk.clone()).unwrap_err();
    assert_eq!(
        (error.code, error.service_specific, error.message.as_str()),
        (-8, 73, "SDK filesystem failure")
    );
    *installer.reject_sdk.lock().unwrap() = false;
    resources.reconcile_sdk_data(sdk.clone()).unwrap();
    assert_eq!(
        *installer.sdk_calls.lock().unwrap(),
        [sdk.clone(), sdk.clone(), sdk]
    );
    use aim_services::package::{restrictions::UserState, scan::User, settings::Package};
    let package = Package {
        name: "org.example.removed".into(),
        ..Default::default()
    };
    let users = [0, 10].map(|id| User {
        id,
        pre_created: false,
        adb_install_disallowed: false,
    });
    let states = std::collections::BTreeMap::from([(
        0,
        UserState {
            ce_data_inode: i64::MAX,
            ..Default::default()
        },
    )]);
    for user in [0, 10] {
        for directory in ["user", "user_de"] {
            for name in [package.name.as_str(), "org.example.keep"] {
                let path = data.join(directory).join(user.to_string()).join(name);
                fs::create_dir_all(&path).unwrap();
                fs::write(path.join("data"), b"disposable app data").unwrap();
            }
        }
    }
    let error = resources
        .destroy_boot_app_storage(&package, &users, &states)
        .unwrap_err();
    assert!(error.contains("installer data failure") && error.contains("user 10"));
    for directory in ["user", "user_de"] {
        assert!(!data.join(directory).join("0").join(&package.name).exists());
        assert!(data.join(directory).join("10").join(&package.name).exists());
        for user in ["0", "10"] {
            assert!(
                data.join(directory)
                    .join(user)
                    .join("org.example.keep/data")
                    .exists()
            );
        }
    }
    *installer.reject_user.lock().unwrap() = None;
    resources
        .destroy_boot_app_storage(&package, &users, &states)
        .unwrap();
    assert_eq!(
        *installer.data_calls.lock().unwrap(),
        [
            (package.name.clone(), 0, 7, i64::MAX),
            (package.name.clone(), 10, 7, 0),
            (package.name.clone(), 0, 7, i64::MAX),
            (package.name.clone(), 10, 7, 0),
        ]
    );
    for directory in ["user", "user_de"] {
        assert!(!data.join(directory).join("10").join(&package.name).exists());
    }
    let count = installer.data_calls.lock().unwrap().len();
    for invalid in [
        Package {
            name: "../escape".into(),
            ..package.clone()
        },
        Package {
            volume_uuid: Some("private-volume".into()),
            ..package.clone()
        },
    ] {
        assert!(
            resources
                .destroy_boot_app_storage(&invalid, &users, &states)
                .is_err()
        );
    }
    for invalid_users in [
        &[][..],
        &[users[0], users[0]][..],
        &[User { id: -1, ..users[0] }][..],
    ] {
        assert!(
            resources
                .destroy_boot_app_storage(&package, invalid_users, &states)
                .is_err()
        );
    }
    assert_eq!(installer.data_calls.lock().unwrap().len(), count);
    drop(resources);
    drop(fixture);
}
