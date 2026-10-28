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

fn uid_map(host: &AppHost) -> Mapped {
    use aim_storage::bpf_object::{HEADER, KIND_MAP, MAGIC};
    let path = host.bpf().unwrap().join(APP_UID_STATS_MAP);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut data = vec![0u8; 4 * 48 + HEADER];
    let h = &mut data[4 * 48..];
    h[..8].copy_from_slice(&MAGIC.to_ne_bytes());
    for (offset, value) in [(8, KIND_MAP), (12, HASH), (16, 4), (20, 32), (24, 4)] {
        h[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    h[64..72].copy_from_slice(&192u64.to_ne_bytes());
    std::fs::write(&path, data).unwrap();
    Mapped::open(&path).unwrap()
}

fn record(map: &Mapped, uid: i32, values: [u64; 4]) {
    let map = map.map();
    let _guard = map.lock();
    let slot = map.claim(&uid.to_ne_bytes(), 0).unwrap();
    let value: Vec<u8> = values.into_iter().flat_map(u64::to_ne_bytes).collect();
    // SAFETY: this fixture's locked map has 32-byte values.
    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), map.value_of(slot), 32);
    }
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
                let e = force_update(&service, uid).unwrap_err();
                assert_eq!(e.code, EX_ILLEGAL_STATE, "{e:?}");
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
fn force_update_polls_without_a_cache_and_rejects_a_bad_map_layout() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        poll_permissions(host, NETWORK_STACK);
        let service = NetworkStats::new(host.clone());
        assert!(service.uid_cache.set(None).is_ok());
        assert_eq!(
            force_update(&service, 10100).unwrap_err().code,
            EX_ILLEGAL_STATE
        );
        let map = uid_map(host);
        record(&map, 10123, [1, 100, 2, 200]);
        force_update(&service, 10100).unwrap();
        assert_eq!(
            service.uid_stats(10123, 10123).unwrap().unwrap().rx_bytes,
            100
        );
        // SAFETY: exclusively mutate this disposable fixture's header, while
        // no other thread is using it; polling must validate before dereferencing.
        unsafe {
            let h = map.map().h() as *const aim_storage::bpf_object::Header
                as *mut aim_storage::bpf_object::Header;
            std::ptr::addr_of_mut!((*h).value_size).write(24);
        }
        let e = force_update(&service, 10100).unwrap_err();
        assert_eq!(e.code, EX_ILLEGAL_STATE);
        assert!(e.message.contains("invalid UID stats layout"));
    });
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
