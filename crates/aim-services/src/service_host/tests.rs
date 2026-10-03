//! Synchronous native package bootstrap, independent of late-service listeners.
use crate::{
    package::{owner::permission_gids::PermissionGidError, system_config::SystemConfig},
    service_host::ServiceHost,
    system::System,
};
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service, Strong},
    parcel::{Binder, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{
    dev_aim_server_ipackagebootstrapbridge as bootstrap, dev_aim_server_iservicehost as host,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
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
        panic!("unexpected fd")
    }
}
struct Registry {
    process: Weak<LocalProcess>,
    nodes: Mutex<BTreeMap<String, Strong>>,
}
impl Service for Registry {
    fn descriptor(&self) -> &str {
        "fixture.Registry"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let name = call.data.read_string16()?.unwrap();
        let mut reply = Parcel::new();
        match call.code {
            1 => {
                let Some(Binder::Handle(handle)) = call.data.read_binder()? else {
                    panic!("remote node expected")
                };
                self.nodes
                    .lock()
                    .unwrap()
                    .insert(name, self.process.upgrade().unwrap().strong(handle));
            }
            2 => reply.write_binder(Some(self.nodes.lock().unwrap()[&name].binder())),
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}
fn register(process: &Arc<LocalProcess>, name: &str, node: Binder) {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    p.write_binder(Some(node));
    process.strong(0).transact(1, &p, false).unwrap();
}
fn find(process: &Arc<LocalProcess>, name: &str) -> Strong {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    let reply = process.strong(0).transact(2, &p, false).unwrap();
    let Some(Binder::Handle(handle)) = reply.reader().read_binder().unwrap() else {
        panic!("remote node expected")
    };
    process.strong(handle)
}
struct Owner {
    calls: Mutex<Vec<i32>>,
    reject: AtomicBool,
    bcp_reads: AtomicUsize,
    malformed_bcp: AtomicBool,
    malformed_seinfo: AtomicBool,
    gid: i32,
}
impl Service for Owner {
    fn descriptor(&self) -> &str {
        bootstrap::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(bootstrap::DESCRIPTOR)?;
        if call.code == bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH {
            self.bcp_reads.fetch_add(1, Ordering::SeqCst);
        }
        let mut reply = Parcel::new();
        if self.reject.load(Ordering::SeqCst) {
            reply.write_exception(&Exception::security("original owner denied"));
            return Ok(reply);
        }
        reply.write_no_exception();
        match call.code {
            bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                reply.write_i32(1);
                if self.malformed_bcp.load(Ordering::SeqCst) {
                    reply.write_i32(99);
                }
            }
            bootstrap::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED => {
                assert_eq!(
                    call.data.read_string16()?.as_deref(),
                    Some("fixture.package")
                );
                let sdk = call.data.read_i32()?;
                reply.write_i32(i32::from(sdk >= 31));
            }
            bootstrap::GET_PERMISSION_GIDS_FOR_UID => {
                self.calls.lock().unwrap().push(call.data.read_i32()?);
                reply.write_i32(2);
                reply.write_i32(self.gid);
                reply.write_i32(self.gid);
            }
            bootstrap::GET_SE_INFO_TARGET_SDK_VERSION => {
                let cache = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                let parsed = crate::package::pkg::AndroidPackage::read_cache_entry(&cache)
                    .expect("original parsed package cache");
                assert_eq!(parsed.package_name, "fixture.package");
                assert_eq!(parsed.target_sdk_version, 29);
                reply.write_i32(30);
                if self.malformed_seinfo.load(Ordering::SeqCst) {
                    reply.write_i32(99);
                }
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        assert_eq!(call.data.remaining(), 0);
        Ok(reply)
    }
}
fn attach(process: &Arc<LocalProcess>, bridge: Option<Binder>) -> Result<(), Exception> {
    let service = find(process, "host");
    // Independent Java Parcel primitive/interface-object encoding.
    let mut p = Parcel::new();
    p.write_interface_token(host::DESCRIPTOR);
    p.write_binder(bridge);
    let reply = service
        .transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE, &p, false)
        .unwrap();
    host::read_attach_package_bootstrap_bridge_reply(&mut reply.reader()).unwrap()
}
struct Processes {
    driver: Arc<Driver>,
    processes: Vec<Arc<LocalProcess>>,
}
impl Drop for Processes {
    fn drop(&mut self) {
        for p in &self.processes {
            self.driver.release(p.proc_handle());
        }
    }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Binder death notification did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn synchronous_package_bootstrap_preserves_replacement_and_propagates_owner_failures() {
    let driver = Driver::new();
    let open = |pid, euid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid,
                security_context: None,
            },
        )
    };
    let manager = open(94001, 1000);
    let native = open(94002, 1000);
    let first = open(94003, 1000);
    let second = open(94004, 1000);
    let foreign = open(94005, 19001);
    let _processes = Processes {
        driver: driver.clone(),
        processes: vec![
            manager.clone(),
            native.clone(),
            first.clone(),
            second.clone(),
            foreign.clone(),
        ],
    };
    let registry = Arc::new(Registry {
        process: Arc::downgrade(&manager),
        nodes: Mutex::new(BTreeMap::new()),
    });
    let Binder::Local(ptr) = manager.add_service(registry) else {
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
            94006,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    for p in &_processes.processes {
        p.start();
    }
    let system = System::new(native.clone(), &[]);
    let late = Arc::new(AtomicBool::new(false));
    let told = late.clone();
    system.add_bridge_listener(Box::new(move |_| {
        told.store(true, Ordering::SeqCst);
    }));
    let node = native.add_service(Arc::new(ServiceHost::new(native.clone(), &system)));
    register(&native, "host", node);
    assert!(system.package_bootstrap().is_err());
    assert!(attach(&first, None).is_err());
    let owner = Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 3003,
    });
    let node = first.add_service(owner.clone());
    assert!(attach(&foreign, None).is_err_and(|e| e.code == -1));
    let foreign_node = foreign.add_service(Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 999,
    }));
    assert!(attach(&foreign, Some(foreign_node)).is_err_and(|e| e.code == -1));
    assert!(system.package_bootstrap().is_err());
    attach(&first, Some(node)).unwrap();
    let mut wrong_token = Parcel::new();
    wrong_token.write_interface_token("wrong.interface");
    wrong_token.write_binder(Some(node));
    assert!(
        find(&first, "host")
            .transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE, &wrong_token, false)
            .is_err()
    );
    let old = system.package_bootstrap().unwrap();
    assert_eq!(owner.bcp_reads.load(Ordering::SeqCst), 1);
    assert!(!late.load(Ordering::SeqCst));
    let config = SystemConfig::default();
    let mut parsed = crate::package::pkg::AndroidPackage {
        package_name: "fixture.package".into(),
        target_sdk_version: 36,
        uses_libraries: vec!["android.test.base".into()],
        ..Default::default()
    };
    old.library_compatibility(&config, &|_| None)
        .unwrap()
        .apply(&mut parsed, false, false, None)
        .unwrap();
    assert!(
        !parsed
            .uses_libraries
            .iter()
            .any(|name| name == "android.test.base")
    );
    assert!(
        !old.library_policy("fixture.package", 30)
            .unwrap()
            .enforce_native_dependencies
    );
    assert!(
        old.library_policy("fixture.package", 31)
            .unwrap()
            .enforce_native_dependencies
    );
    assert_eq!(
        old.permission_gids(19001, &[10, 0]).unwrap(),
        [3003, 3003, 3003, 3003]
    );
    assert_eq!(*owner.calls.lock().unwrap(), [1019001, 19001]);
    parsed.target_sdk_version = 29;
    assert_eq!(old.seinfo_target_sdk(&parsed).unwrap(), 30);
    assert_eq!(
        crate::package::scan::SeInfoCompatibility::target_sdk(old.as_ref(), &parsed).unwrap(),
        30
    );
    owner.malformed_seinfo.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.seinfo_target_sdk(&parsed),
        Err(crate::package::bootstrap::SeInfoError::Transport(
            aim_binder_host::parcel::BAD_VALUE
        ))
    ));
    assert!(crate::package::scan::SeInfoCompatibility::target_sdk(old.as_ref(), &parsed).is_err());
    owner.malformed_seinfo.store(false, Ordering::SeqCst);
    assert!(old.permission_gids(19001, &[0, 0]).is_err());
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.seinfo_target_sdk(&parsed),
        Err(crate::package::bootstrap::SeInfoError::Owner(_))
    ));
    assert!(old.library_compatibility(&config, &|_| None).is_ok());
    assert_eq!(owner.bcp_reads.load(Ordering::SeqCst), 1);
    assert!(attach(&first, Some(node)).is_err_and(|error| error.code == -1));
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    owner.reject.store(false, Ordering::SeqCst);
    owner.malformed_bcp.store(true, Ordering::SeqCst);
    assert!(attach(&first, Some(node)).is_err_and(|error| error.code == -5));
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    owner.malformed_bcp.store(false, Ordering::SeqCst);
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.library_policy("fixture.package", 31),
        Err(crate::package::libraries::NativePolicyError::Owner(_))
    ));
    assert!(matches!(
        old.permission_gids(19001, &[0]),
        Err(PermissionGidError::Owner(_))
    ));
    let wrong = first.add_service(Arc::new(Registry {
        process: Arc::downgrade(&first),
        nodes: Mutex::new(BTreeMap::new()),
    }));
    assert!(attach(&first, Some(wrong)).is_err());
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    let replacement = second.add_service(Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 3004,
    }));
    attach(&second, Some(replacement)).unwrap();
    let current = system.package_bootstrap().unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    driver.release(first.proc_handle());
    // Wait for the actual old endpoint death, then ensure it did not erase the new one.
    until(|| {
        matches!(
            old.permission_gids(19001, &[0]),
            Err(PermissionGidError::Transport(_))
        )
    });
    assert!(Arc::ptr_eq(&current, &system.package_bootstrap().unwrap()));
    assert_eq!(current.permission_gids(19001, &[0]).unwrap(), [3004, 3004]);
    driver.release(second.proc_handle());
    until(|| system.package_bootstrap().is_err());
    assert!(!late.load(Ordering::SeqCst));
}
