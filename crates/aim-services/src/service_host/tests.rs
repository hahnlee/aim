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
    legacy_reply: AtomicUsize,
    domain_reply: AtomicUsize,
    users_reply: AtomicUsize,
    apex_reply: AtomicUsize,
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
            bootstrap::IS_SHARED_UID_MIGRATION_BEST_EFFORT => reply.write_bool(false),
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
            bootstrap::GET_LEGACY_PERMISSION_STATE => {
                let app_id = call.data.read_i32()?;
                let users = aim_service_aidl::read_int_array(&mut call.data)?.unwrap();
                let mode = self.legacy_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_i32(app_id);
                payload.write_i32(users.len() as i32);
                for user in users {
                    payload.write_i32(user);
                    payload.write_bool(user == 10);
                    payload.write_i32(1);
                    payload.write_string16(None);
                    payload.write_bool(true);
                    payload.write_bool(false);
                    payload.write_i32(self.gid);
                }
                if mode == 2 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 1).then_some(payload.data()),
                );
                if mode == 3 {
                    reply.write_i32(99);
                }
            }
            bootstrap::NOTIFY_APEX_SCAN_RESULTS => {
                let bytes = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                assert_eq!(bytes, 0i32.to_le_bytes());
                assert_eq!(call.data.remaining(), 0);
                if self.apex_reply.load(Ordering::SeqCst) == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_APEX_BOOT_INVENTORY => {
                let mode = self.apex_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_i32(if mode == 1 { 0 } else { -1 });
                payload.write_i32(if mode == 1 { 0 } else { 1 });
                if mode != 1 {
                    payload.write_string16(Some("raw.module"));
                    payload.write_string16(Some("/apex/mounted"));
                    payload.write_string16(Some("/product/apex/factory.apex"));
                    payload.write_bool(false);
                    payload.write_string16(Some("/data/apex/active/updated.apex"));
                    payload.write_bool(true);
                }
                if mode == 2 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 3).then_some(payload.data()),
                );
                if mode == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_PACKAGE_SCAN_USERS => {
                let mode = self.users_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_bool(mode != 1);
                if mode != 1 {
                    payload.write_i32(if mode == 2 { 0 } else { 2 });
                    if mode != 2 {
                        for (id, pre_created, adb) in [(0, false, true), (10, true, false)] {
                            payload.write_i32(id);
                            payload.write_bool(pre_created);
                            payload.write_bool(adb);
                        }
                    }
                }
                if mode == 3 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 4).then_some(payload.data()),
                );
                if mode == 5 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GENERATE_NEW_DOMAIN_ID => {
                let mode = self.domain_reply.load(Ordering::SeqCst);
                let id = [self.gid as u8; 16];
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    match mode {
                        1 => None,
                        2 => Some(&[1; 15]),
                        3 => Some(&[1; 17]),
                        _ => Some(&id),
                    },
                );
                if mode == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_SE_INFO_TARGET_SDK_VERSION => {
                let cache = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                let parsed = crate::package::pkg::AndroidPackage::read_cache_entry(&cache)
                    .expect("original parsed package cache");
                reply.write_i32(parsed.target_sdk_version + 1);
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
    exercise_bootstrap(false);
}
#[test]
#[ignore = "requires pinned original image; run explicitly"]
fn native_boot_scan_uses_retained_original_bootstrap_owners() {
    exercise_bootstrap(true);
}
fn exercise_bootstrap(run_scan: bool) {
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
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
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
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
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
    let boot = old.resolve_boot(&config, &|_| None).unwrap();
    assert_eq!(
        boot.migration(),
        crate::package::scan::SharedUidMigration::NewInstallOnly
    );
    assert_eq!(boot.users().users.as_ref().unwrap().len(), 2);
    assert_eq!(boot.apex().active.len(), 1);
    owner.users_reply.store(1, Ordering::SeqCst);
    assert_eq!(
        old.resolve_boot(&config, &|_| None).unwrap().users().users,
        None
    );
    owner.users_reply.store(2, Ordering::SeqCst);
    assert_eq!(
        old.resolve_boot(&config, &|_| None).unwrap().users().users,
        Some(vec![])
    );
    owner.users_reply.store(3, Ordering::SeqCst);
    assert!(old.resolve_boot(&config, &|_| None).is_err());
    owner.users_reply.store(0, Ordering::SeqCst);
    owner.apex_reply.store(2, Ordering::SeqCst);
    assert!(old.resolve_boot(&config, &|_| None).is_err());
    owner.apex_reply.store(0, Ordering::SeqCst);
    if run_scan {
        verify_boot_scan(&system, &old, &owner, &config);
    }
    let mut parsed = crate::package::pkg::AndroidPackage {
        feature_flag_state: Some(Vec::new()),
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
    let captured_permissions = old.legacy_permissions(19001, &[10, 0]).unwrap();
    assert_eq!(captured_permissions.app_id(), 19001);
    assert!(captured_permissions.user(10).unwrap().missing);
    assert!(!captured_permissions.user(0).unwrap().missing);
    assert_eq!(captured_permissions.user(11), None);
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].name,
        None
    );
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].flags,
        3003
    );
    assert!(old.legacy_permissions(19001, &[0, 0]).is_err());
    for mode in 1..=3 {
        owner.legacy_reply.store(mode, Ordering::SeqCst);
        assert!(old.legacy_permissions(19001, &[10, 0]).is_err());
    }
    owner.legacy_reply.store(0, Ordering::SeqCst);
    assert_eq!(old.new_domain_id().unwrap(), [3003i32 as u8; 16]);
    for mode in 1..=4 {
        owner.domain_reply.store(mode, Ordering::SeqCst);
        assert!(matches!(
            old.new_domain_id(),
            Err(crate::package::bootstrap::OwnerError::Transport(
                aim_binder_host::parcel::BAD_VALUE
            ))
        ));
    }
    owner.domain_reply.store(0, Ordering::SeqCst);
    old.notify_apex_scan(&[]).unwrap();
    owner.apex_reply.store(4, Ordering::SeqCst);
    assert!(matches!(
        old.notify_apex_scan(&[]),
        Err(crate::package::bootstrap::OwnerError::Transport(
            aim_binder_host::parcel::BAD_VALUE
        ))
    ));
    owner.apex_reply.store(0, Ordering::SeqCst);
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.notify_apex_scan(&[]),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    owner.reject.store(false, Ordering::SeqCst);
    let apex = old.apex_inventory().unwrap();
    assert_eq!(apex.packages, None);
    let scan = apex.scan_apexes();
    assert_eq!(scan.len(), 1);
    assert_eq!(scan[0].module_name.as_deref(), Some("raw.module"));
    assert_eq!(scan[0].partition, crate::package::scan::Partition::Product);
    assert!(scan[0].active_changed && !scan[0].factory);
    owner.apex_reply.store(1, Ordering::SeqCst);
    assert_eq!(old.apex_inventory().unwrap().packages, Some(Vec::new()));
    for mode in 2..=4 {
        owner.apex_reply.store(mode, Ordering::SeqCst);
        assert!(matches!(
            old.apex_inventory(),
            Err(crate::package::bootstrap::OwnerError::Transport(
                aim_binder_host::parcel::BAD_VALUE
            ))
        ));
    }
    owner.apex_reply.store(0, Ordering::SeqCst);
    assert_eq!(apex.active.len(), 1);
    let users = old.scan_users().unwrap();
    assert_eq!(
        users
            .users
            .as_ref()
            .unwrap()
            .iter()
            .map(|u| (u.id, u.pre_created, u.adb_install_disallowed))
            .collect::<Vec<_>>(),
        [(0, false, true), (10, true, false)]
    );
    owner.users_reply.store(1, Ordering::SeqCst);
    assert_eq!(old.scan_users().unwrap().users, None);
    owner.users_reply.store(2, Ordering::SeqCst);
    assert_eq!(old.scan_users().unwrap().users, Some(Vec::new()));
    for mode in 3..=5 {
        owner.users_reply.store(mode, Ordering::SeqCst);
        assert!(old.scan_users().is_err());
    }
    owner.users_reply.store(0, Ordering::SeqCst);
    assert_eq!(users.users.as_ref().unwrap().len(), 2);
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
        old.apex_inventory(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.scan_users(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.new_domain_id(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.seinfo_target_sdk(&parsed),
        Err(crate::package::bootstrap::SeInfoError::Owner(_))
    ));
    assert!(matches!(
        old.legacy_permissions(19001, &[10, 0]),
        Err(crate::package::owner::legacy_permissions::Error::Owner(_))
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
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 3004,
    }));
    attach(&second, Some(replacement)).unwrap();
    let current = system.package_bootstrap().unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    assert_eq!(current.new_domain_id().unwrap(), [3004i32 as u8; 16]);
    driver.release(first.proc_handle());
    // Wait for the actual old endpoint death, then ensure it did not erase the new one.
    until(|| {
        matches!(
            old.permission_gids(19001, &[0]),
            Err(PermissionGidError::Transport(_))
        )
    });
    assert!(Arc::ptr_eq(&current, &system.package_bootstrap().unwrap()));
    assert!(matches!(
        old.legacy_permissions(19001, &[10, 0]),
        Err(crate::package::owner::legacy_permissions::Error::Transport(
            _
        ))
    ));
    assert_eq!(
        current
            .legacy_permissions(19001, &[10, 0])
            .unwrap()
            .user(10)
            .unwrap()
            .permissions[0]
            .flags,
        3004
    );
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].flags,
        3003
    );
    assert_eq!(current.permission_gids(19001, &[0]).unwrap(), [3004, 3004]);
    driver.release(second.proc_handle());
    until(|| system.package_bootstrap().is_err());
    assert!(!late.load(Ordering::SeqCst));
}

struct NonceOwner {
    file: File,
    reject: AtomicBool,
    trailing: AtomicBool,
}
impl Service for NonceOwner {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_ibridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::dev_aim_server_ibridge as bridge;
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(bridge::DESCRIPTOR)?;
        assert_eq!(call.code, bridge::GET_APPLICATION_SHARED_MEMORY);
        assert_eq!(call.data.remaining(), 0);
        let mut reply = Parcel::new();
        if self.reject.load(Ordering::SeqCst) {
            reply.write_exception(&Exception::security("shared memory denied"));
        } else {
            reply.write_no_exception();
            reply.write_i32(1); // nullable ParcelFileDescriptor presence
            reply.write_i32(0); // no comm channel
            reply.write_file(self.file.clone());
            if self.trailing.load(Ordering::SeqCst) {
                reply.write_i32(99);
            }
        }
        Ok(reply)
    }
}

#[test]
fn late_bridge_death_preserves_replacement_nonce_mapping() {
    use std::os::fd::AsFd;
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
    let manager = open(96001);
    let native = open(96002);
    let first = open(96003);
    let second = open(96004);
    let invalid = open(96005);
    let observer = open(96007);
    let _processes = Processes {
        driver: driver.clone(),
        processes: vec![
            manager.clone(),
            native.clone(),
            first.clone(),
            second.clone(),
            invalid.clone(),
            observer.clone(),
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
            96006,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    for process in &_processes.processes {
        process.start();
    }
    let system = System::new(native.clone(), &[]);
    let attaches = Arc::new(AtomicUsize::new(0));
    let observed = attaches.clone();
    system.add_bridge_listener(Box::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let owner = |nonce| {
        let fd = crate::nonces::tests::nonce_file(nonce);
        Arc::new(NonceOwner {
            file: aim_binder_host::server::file_from_fd(fd.as_fd()).unwrap(),
            reject: AtomicBool::new(false),
            trailing: AtomicBool::new(false),
        })
    };
    let old = owner(41);
    let new = owner(42);
    register(&first, "late-old", first.add_service(old));
    register(&second, "late-new", second.add_service(new.clone()));
    register(
        &invalid,
        "late-invalid",
        invalid.add_service(Arc::new(NonceOwner {
            file: Arc::new(()),
            reject: AtomicBool::new(false),
            trailing: AtomicBool::new(false),
        })),
    );
    let old = find(&native, "late-old");
    let new_node = find(&native, "late-new");
    let invalid_node = find(&native, "late-invalid");
    let handle = |node: &Strong| {
        let Binder::Handle(handle) = node.binder() else {
            unreachable!()
        };
        handle
    };
    system.attach_bridge(handle(&old)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(41));
    new.reject.store(true, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&new_node)).is_err());
    new.reject.store(false, Ordering::SeqCst);
    new.trailing.store(true, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&new_node)).is_err());
    new.trailing.store(false, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&invalid_node)).is_err());
    assert_eq!(system.package_info_nonce(), Some(41));
    assert_eq!(attaches.load(Ordering::SeqCst), 1);
    system.attach_bridge(handle(&new_node)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(42));
    assert_eq!(attaches.load(Ordering::SeqCst), 2);
    system.attach_bridge(handle(&new_node)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(42));
    assert_eq!(attaches.load(Ordering::SeqCst), 3);
    // The active source must keep its own Binder reference, independently of
    // this caller's temporary lookup handle and received parcel.
    drop(new_node);
    let old_died = Arc::new(AtomicBool::new(false));
    let observed = old_died.clone();
    // Binder permits one death registration per process/node reference. Observe
    // from another process, independently of the native owner's registration.
    let watched_old = find(&observer, "late-old");
    observer.link_to_death(
        &watched_old,
        Box::new(move || {
            observed.store(true, Ordering::SeqCst);
        }),
    );
    driver.release(first.proc_handle());
    until(|| old_died.load(Ordering::SeqCst));
    until(|| old.transact(0, &Parcel::new(), false).is_err());
    assert_eq!(system.package_info_nonce(), Some(42));
    // A failed replacement never registers a cleanup that owns the current map.
    driver.release(invalid.proc_handle());
    assert_eq!(system.package_info_nonce(), Some(42));
    driver.release(second.proc_handle());
    until(|| system.package_info_nonce().is_none());
}

fn verify_boot_scan(
    system: &System,
    bridge: &crate::package::bootstrap::Bridge,
    owner: &Owner,
    config: &SystemConfig,
) {
    use crate::package::{
        bootstrap::ScanPolicy,
        parse::Platform,
        scan::{AbiPolicy, NativeLibraryInstallPolicy, ScanClock},
        write::Apks,
    };
    let original = aim_paths::original_image();
    let root = std::env::temp_dir().join(format!("aim-bootstrap-scan-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    struct Data(std::path::PathBuf);
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let data = Data(root.clone());
    let framework = root.join("system/framework");
    std::fs::create_dir_all(&framework).unwrap();
    std::os::unix::fs::symlink(
        original.join("system/framework/framework-res.apk"),
        framework.join("framework-res.apk"),
    )
    .unwrap();
    let apks = Apks {
        files: Box::new(move |path| Some(root.join(path.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let seinfo = crate::package::owner::seinfo::Policy::load(&original).unwrap();
    let abi = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let policy = || ScanPolicy {
        seinfo: &seinfo,
        apex_parse_flags: crate::package::parse::PARSE_IS_SYSTEM_DIR,
        first_api_level: 36,
        vendor_sdk: 36,
        abi: &abi,
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        platform_runtime_64bit: true,
        install: NativeLibraryInstallPolicy {
            page_size: 4096,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        },
        clock: ScanClock {
            current_time: 0,
            user_id: 0,
            update_time: false,
        },
        factory_test: false,
        install_user: None,
        allow_install: true,
        instant_app: false,
        virtual_preload: false,
        stopped_system_app: false,
    };
    // Capture valid inventory before testing notification failure.
    owner.apex_reply.store(1, Ordering::SeqCst);
    let boot = bridge.resolve_boot(config, &|_| None).unwrap();
    owner.apex_reply.store(4, Ordering::SeqCst);
    assert!(
        matches!(boot.scan_first_boot(&apks, policy()), Err(crate::package::bootstrap::BootError::Scan(crate::package::scan::SigningError::Rejected(e))) if e.phase == "apex-notification")
    );
    owner.apex_reply.store(1, Ordering::SeqCst);
    let scan = system
        .scan_package_first_boot(&apks, config, &|_| None, policy())
        .unwrap();
    assert_eq!(scan.packages.len(), 1);
    assert_eq!(scan.packages[0].candidate.record.settings.name, "android");
    assert_eq!(
        scan.owner
            .scanned_user_states("android")
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [0]
    );
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        system.scan_package_first_boot(&apks, config, &|_| None, policy()),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    owner.reject.store(false, Ordering::SeqCst);
    owner.apex_reply.store(0, Ordering::SeqCst);
    drop(data);
}
