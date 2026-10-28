//! Binder transport contract; retained Java policy is covered by netstats_owner.
use super::*;
use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, FAILED_TRANSACTION};
use aim_service_aidl::android_os_iservicemanager as sm;
use std::collections::HashMap;
use std::sync::Weak;

fn poll_permissions(host: &Arc<AppHost>, granted: &str) {
    use crate::package::permissions::{AccessSystem, Permission};
    let path = host
        .data()
        .unwrap()
        .join("misc_de/0/apexdata/com.android.permission/access.abx");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let xml = format!(
        "<access><app-id-permissions>
        <app-id id='10100'><permission name='{granted}' flags='4'/></app-id>
        <app-id id='10101'><permission name='android.permission.NETWORK_STATS_PROVIDER' flags='4'/></app-id>
        </app-id-permissions></access>"
    );
    let xml = aim_android_xml::read(xml.as_bytes()).unwrap();
    std::fs::write(path, aim_android_xml::abx::write(&xml).unwrap()).unwrap();
    let manager = super::super::permission_manager::PermissionManager::with_system(
        host.clone(),
        AccessSystem {
            permissions: [
                NETWORK_STACK,
                MAINLINE_NETWORK_STACK,
                "android.permission.NETWORK_STATS_PROVIDER",
            ]
            .into_iter()
            .map(|name| Permission {
                name: name.into(),
                package: "android".into(),
                protection_level: 2,
                dynamic: None,
            })
            .collect(),
            ..Default::default()
        },
    );
    assert!(host.permission_manager.set(Arc::new(manager)).is_ok());
}

// Exercise bpf(BPF_MAP_CREATE/UPDATE/DELETE), exactly as NetBpfLoad and
// netd do, rather than constructing the shared file's private layout.
fn syscall(nr: u64, args: [u64; 6]) -> i64 {
    // SAFETY: zero is a valid register context; dispatch only uses its
    // registers for these bpf/close calls, with live host argument buffers.
    let mut ctx = unsafe { std::mem::zeroed::<aim_linux_abi::context::GuestContext>() };
    ctx.x[..6].copy_from_slice(&args);
    ctx.x[8] = nr;
    aim_linux_abi::sys::dispatch(&mut ctx);
    ctx.x[0] as i64
}

fn bpf(cmd: u64, attr: &[u64]) -> i64 {
    syscall(
        280,
        [
            cmd,
            attr.as_ptr() as u64,
            std::mem::size_of_val(attr) as u64,
            0,
            0,
            0,
        ],
    )
}

struct UidMap(i32);

impl Drop for UidMap {
    fn drop(&mut self) {
        assert_eq!(syscall(57, [self.0 as u64, 0, 0, 0, 0, 0]), 0);
    }
}

fn create_uid_map(host: &AppHost, key_size: u32, value_size: u32) -> UidMap {
    let path = host.bpf().unwrap().join(APP_UID_STATS_MAP);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut attr = [0u32; 24];
    attr[..5].copy_from_slice(&[HASH, key_size, value_size, 4, 0]);
    let fd = syscall(
        280,
        [
            0,
            attr.as_ptr() as u64,
            std::mem::size_of_val(&attr) as u64,
            0,
            0,
            0,
        ],
    );
    assert!(fd >= 0, "BPF_MAP_CREATE: {fd}");
    let mut name = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes for this live map fd.
    assert_eq!(
        unsafe { libc::fcntl(fd as i32, libc::F_GETPATH, name.as_mut_ptr()) },
        0
    );
    let end = name.iter().position(|&b| b == 0).unwrap();
    use std::os::unix::ffi::OsStrExt;
    std::fs::hard_link(std::ffi::OsStr::from_bytes(&name[..end]), &path).unwrap();
    UidMap(fd as i32)
}

fn uid_map(host: &AppHost) -> UidMap {
    create_uid_map(
        host,
        std::mem::size_of::<u32>() as u32,
        std::mem::size_of::<[u64; 4]>() as u32,
    )
}

fn record(map: &UidMap, uid: i32, values: [u64; 4]) {
    assert_eq!(
        bpf(
            2,
            &[
                map.0 as u64,
                (&uid as *const i32) as u64,
                values.as_ptr() as u64,
                0
            ]
        ),
        0
    );
}

fn delete(map: &UidMap, uid: i32) {
    assert_eq!(
        bpf(3, &[map.0 as u64, (&uid as *const i32) as u64, 0, 0]),
        0
    );
}

fn force_update(service: &NetworkStats, caller: u32) -> Result<(), Exception> {
    let mut p = Parcel::new();
    p.write_interface_token(ns::DESCRIPTOR);
    let result = service
        .transact(&mut Call {
            code: ns::FORCE_UPDATE,
            flags: 0,
            sender_pid: 71,
            sender_euid: caller,
            data: p.reader(),
        })
        .unwrap();
    result.reader().read_exception().unwrap()
}

#[test]
fn force_update_checks_both_permissions_before_polling() {
    use aim_binder_host::parcel::EX_SECURITY;
    for permission in [NETWORK_STACK, MAINLINE_NETWORK_STACK] {
        crate::system::with_discovery_binders(|host, _, _, _| {
            poll_permissions(host, permission);
            let service = NetworkStats::new(host.clone());
            // NETWORK_STATS_PROVIDER alone does not pass the network-stack check.
            let e = force_update(&service, 10101).unwrap_err();
            assert_eq!(e.code, EX_SECURITY, "{e:?}");
            assert!(e.message.contains(NETWORK_STACK));
            assert!(e.message.contains(MAINLINE_NETWORK_STACK));
            // Authorized callers reach the real poll even when its map is missing.
            for uid in [10100, 1000] {
                force_update(&service, uid).unwrap();
                assert_eq!(service.poll_uid_stats().unwrap_err().code, EX_ILLEGAL_STATE);
            }
            let _map = uid_map(host);
            for uid in [10100, 1000] {
                force_update(&service, uid).unwrap();
            }
        });
    }
}

#[test]
fn force_update_refreshes_cached_counters_and_keeps_uid_query_access() {
    use aim_binder_host::parcel::EX_SECURITY;
    crate::system::with_discovery_binders(|host, _, _, _| {
        poll_permissions(host, NETWORK_STACK);
        let map = uid_map(host);
        record(&map, 10123, [1, 100, 2, 200]);
        let service = NetworkStats::new(host.clone());
        assert!(
            service
                .uid_cache
                .set(Some(Mutex::new(RateLimitCache {
                    expiry_ms: 60_000,
                    max: 400,
                    entries: VecDeque::new(),
                })))
                .is_ok()
        );
        let old = service.uid_stats(10123, 10123).unwrap().unwrap();
        assert_eq!((old.rx_bytes, old.tx_bytes), (100, 200));
        record(&map, 10123, [3, 300, 4, 400]);
        assert_eq!(service.uid_stats(10123, 10123).unwrap(), Some(old));
        assert_eq!(force_update(&service, 10101).unwrap_err().code, EX_SECURITY);
        assert_eq!(service.uid_stats(10123, 10123).unwrap(), Some(old));
        force_update(&service, 10100).unwrap();
        let current = service.uid_stats(10123, 10123).unwrap().unwrap();
        assert_eq!(
            (
                current.rx_packets,
                current.rx_bytes,
                current.tx_packets,
                current.tx_bytes
            ),
            (3, 300, 4, 400)
        );
        assert_eq!(service.uid_stats(10124, 10123).unwrap(), None);
        record(&map, 10123, [0; 4]);
        force_update(&service, 10100).unwrap();
        assert_eq!(
            service.uid_stats(1000, 10123).unwrap(),
            Some(Entry::default())
        );
        assert!(
            service
                .uid_cache()
                .unwrap()
                .unwrap()
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
    });
}

#[test]
fn force_update_polls_entries_deleted_entries_and_an_empty_hash_without_a_cache() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        poll_permissions(host, NETWORK_STACK);
        let map = uid_map(host);
        let service = NetworkStats::new(host.clone());
        assert!(service.uid_cache.set(None).is_ok());
        assert!(service.poll_uid_stats().unwrap().is_empty());
        force_update(&service, 10100).unwrap();
        record(&map, 10123, [1, 100, 2, 200]);
        record(&map, 10124, [3, 300, 4, 400]);
        record(&map, 10125, [5, 500, 6, 600]);
        let stats = service.poll_uid_stats().unwrap();
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[&10123].rx_bytes, 100);
        assert_eq!(stats[&10124].tx_packets, 4);
        assert_eq!(stats[&10125].tx_bytes, 600);
        delete(&map, 10124);
        let stats = service.poll_uid_stats().unwrap();
        assert_eq!(stats.len(), 2);
        assert!(!stats.contains_key(&10124));
        force_update(&service, 10100).unwrap();
        assert_eq!(
            service.uid_stats(10123, 10123).unwrap().unwrap().rx_bytes,
            100
        );
        delete(&map, 10123);
        delete(&map, 10125);
        assert!(service.poll_uid_stats().unwrap().is_empty());
        force_update(&service, 10100).unwrap();
    });
}

#[test]
fn force_update_logs_invalid_map_sizes_without_publishing_a_cache_update() {
    for (key_size, value_size) in [(4, 24), (4, 40), (8, 32)] {
        crate::system::with_discovery_binders(|host, _, _, _| {
            poll_permissions(host, NETWORK_STACK);
            let _map = create_uid_map(host, key_size, value_size);
            let service = NetworkStats::new(host.clone());
            let cached = Entry {
                rx_packets: 1,
                rx_bytes: 100,
                tx_packets: 2,
                tx_bytes: 200,
            };
            assert!(
                service
                    .uid_cache
                    .set(Some(Mutex::new(RateLimitCache {
                        expiry_ms: 60_000,
                        max: 400,
                        entries: VecDeque::from([(10123, 123, cached)]),
                    })))
                    .is_ok()
            );
            let e = service.poll_uid_stats().unwrap_err();
            assert_eq!(e.code, EX_ILLEGAL_STATE);
            assert!(e.message.contains("invalid UID stats type or sizes"));
            // The pinned performPollLocked logs IllegalStateException and returns.
            force_update(&service, 10100).unwrap();
            let cache = service.uid_cache().unwrap().unwrap().lock().unwrap();
            assert_eq!(cache.entries, VecDeque::from([(10123, 123, cached)]));
        });
    }
}

struct Registry {
    process: Weak<LocalProcess>,
    names: Mutex<HashMap<String, Strong>>,
}
impl Service for Registry {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let process = self.process.upgrade().unwrap();
        let mut p = Parcel::new();
        match call.code {
            sm::ADD_SERVICE => {
                let a = sm::AddService::read(&mut call.data)?;
                let Some(Binder::Handle(h)) = a.service else {
                    return Err(BAD_VALUE);
                };
                self.names
                    .lock()
                    .unwrap()
                    .insert(a.name.unwrap(), process.strong(h));
                sm::write_add_service_reply(&mut p);
            }
            sm::CHECK_SERVICE => {
                let a = sm::CheckService::read(&mut call.data)?;
                let names = self.names.lock().unwrap();
                sm::write_check_service_reply(
                    &mut p,
                    names.get(a.name.as_deref().unwrap()).map(Strong::binder),
                );
            }
            _ => return Err(BAD_VALUE),
        }
        Ok(p)
    }
}
struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<File, i32> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> Result<u32, i32> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("no files")
    }
}
struct Processes(Vec<Arc<LocalProcess>>);
impl Drop for Processes {
    fn drop(&mut self) {
        for p in self.0.iter().rev() {
            p.shutdown().unwrap();
        }
    }
}
fn publish(process: &LocalProcess, name: &str, binder: Binder) {
    let mut p = Parcel::new();
    sm::AddService {
        name: Some(name.into()),
        service: Some(binder),
        allow_isolated: false,
        dump_priority: 0,
    }
    .write(&mut p);
    sm::read_add_service_reply(
        &mut process
            .transact(0, sm::ADD_SERVICE, &p, false)
            .unwrap()
            .reader(),
    )
    .unwrap()
    .unwrap();
}

struct Session(Mutex<bool>);
impl Service for Session {
    fn descriptor(&self) -> &str {
        "test.netstats.SessionTransport"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!((call.sender_pid, call.sender_euid), (97164, 10123));
        *self.0.lock().unwrap() = true;
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }
}
struct JavaOwner {
    session: Binder,
    calls: Mutex<Vec<u32>>,
}
impl Service for JavaOwner {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!((call.sender_pid, call.sender_euid), (97164, 10123));
        self.calls.lock().unwrap().push(call.code);
        let mut p = Parcel::new();
        match call.code {
            bridge::OPEN_SESSION_FOR_USAGE_STATS => {
                let a = bridge::OpenSessionForUsageStats::read(&mut call.data)?;
                assert_eq!(
                    (a.flags, a.calling_package.as_deref()),
                    (5, Some("app.pkg"))
                );
                bridge::write_open_session_for_usage_stats_reply(&mut p, Some(self.session));
            }
            bridge::INCREMENT_OPERATION_COUNT => {
                let a = bridge::IncrementOperationCount::read(&mut call.data)?;
                assert_eq!((a.uid, a.tag, a.operation_count), (10124, -7, 2));
                return reply(Err(Exception::security(
                    "original UPDATE_DEVICE_STATS denial",
                )));
            }
            _ => return Err(BAD_VALUE),
        }
        Ok(p)
    }
}

#[test]
fn real_app_caller_and_returned_session_survive_the_java_round_trip() {
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
    let manager = open(97161, 1000);
    let java = open(97162, 1000);
    let native = open(97163, 1000);
    let app = open(97164, 10123);
    let _processes = Processes(vec![
        manager.clone(),
        java.clone(),
        native.clone(),
        app.clone(),
    ]);
    let Binder::Local(ptr) = manager.add_service(Arc::new(Registry {
        process: Arc::downgrade(&manager),
        names: Mutex::new(HashMap::new()),
    })) else {
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
            97161,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    for process in [manager.clone(), java.clone(), native.clone(), app.clone()] {
        process.start();
    }
    let session = Arc::new(Session(Mutex::new(false)));
    let java_owner = Arc::new(JavaOwner {
        session: java.add_service(session.clone()),
        calls: Mutex::new(Vec::new()),
    });
    publish(&java, "aim.netstats", java.add_service(java_owner.clone()));
    let service = Arc::new(NetworkStats::new(AppHost::new(native.clone())));
    publish(&native, "netstats", native.add_service(service.clone()));
    let remote = super::super::display::check_service(&app, "netstats").unwrap();
    let mut p = Parcel::new();
    ns::OpenSessionForUsageStats {
        flags: 5,
        calling_package: Some("app.pkg".into()),
    }
    .write(&mut p);
    // A synthetic Call cannot forward the asserted app identity.
    assert_eq!(
        service
            .transact(&mut Call {
                code: ns::OPEN_SESSION_FOR_USAGE_STATS,
                flags: 0,
                sender_pid: 97164,
                sender_euid: 10123,
                data: p.reader()
            })
            .err(),
        Some(FAILED_TRANSACTION)
    );
    let received = remote
        .transact(ns::OPEN_SESSION_FOR_USAGE_STATS, &p, false)
        .unwrap();
    let Some(Binder::Handle(handle)) =
        ns::read_open_session_for_usage_stats_reply(&mut received.reader())
            .unwrap()
            .unwrap()
    else {
        panic!("no session")
    };
    let session_remote = app.strong(handle);
    // Transport probe on the returned binder; actual INetworkStatsSession.close
    // semantics are exercised by the production Java fixture.
    let closed = session_remote.transact(1, &Parcel::new(), false).unwrap();
    closed.reader().read_exception().unwrap().unwrap();
    assert!(*session.0.lock().unwrap());
    let mut p = Parcel::new();
    ns::IncrementOperationCount {
        uid: 10124,
        tag: -7,
        operation_count: 2,
    }
    .write(&mut p);
    let denied = remote
        .transact(ns::INCREMENT_OPERATION_COUNT, &p, false)
        .unwrap();
    let e = ns::read_increment_operation_count_reply(&mut denied.reader())
        .unwrap()
        .unwrap_err();
    assert_eq!(
        e,
        Exception::security("original UPDATE_DEVICE_STATS denial")
    );
    assert_eq!(
        *java_owner.calls.lock().unwrap(),
        [
            bridge::OPEN_SESSION_FOR_USAGE_STATS,
            bridge::INCREMENT_OPERATION_COUNT
        ]
    );
}
