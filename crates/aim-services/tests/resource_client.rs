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
}
impl Service for Installer {
    fn descriptor(&self) -> &str {
        installd::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
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
    drop(resources);
    drop(fixture);
}
