//! Native cleanup over generated Binder calls; the endpoint owns disposable files.
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service},
    parcel::{Binder, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{android_os_iinstalld as installd, android_os_iservicemanager as sm};
use aim_services::package::owner::resources::CodeResources;
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
struct Registry(Binder);
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
            (name.as_deref() == Some("installd")).then_some(self.0),
        );
        Ok(reply)
    }
}
struct Installer {
    data: PathBuf,
    calls: Mutex<Vec<(String, String)>>,
    reject_parent: Mutex<bool>,
    data_calls: Mutex<Vec<(String, i32, i32, i64)>>,
    reject_user: Mutex<Option<i32>>,
}
impl Service for Installer {
    fn descriptor(&self) -> &str {
        installd::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
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
    });
    let endpoint = manager.add_service(installer.clone());
    let Binder::Local(ptr) = manager.add_service(Arc::new(Registry(endpoint))) else {
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
