//! Binder transport contract; retained Java policy is covered by netstats_owner.
use super::*;
use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, FAILED_TRANSACTION};
use aim_service_aidl::android_os_iservicemanager as sm;
use std::collections::HashMap;
use std::sync::Weak;

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
