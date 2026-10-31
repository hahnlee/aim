//! ActiveServices / ProcessList at android-16.0.0_r1: pending binds, async
//! start failure, attach ordering, and processStartTimedOutLocked teardown.
use super::*;
use crate::package::info::{ApplicationInfo, ComponentInfo, PackageItemInfo};

struct Connection(mpsc::Sender<(Option<Binder>, bool)>);
impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, dead: bool) {
        self.0.send((service, dead)).unwrap();
    }
}
fn info(name: &str, isolated: bool) -> ServiceInfo {
    ServiceInfo {
        info: ComponentInfo {
            item: PackageItemInfo {
                name: Some(name.into()),
                package_name: Some("p".into()),
                ..Default::default()
            },
            application_info: Arc::new(ApplicationInfo {
                uid: 10123,
                ..Default::default()
            }),
            process_name: Some("p".into()),
            split_name: None,
            attribution_tags: None,
            description_res: 0,
            enabled: true,
            exported: true,
            direct_boot_aware: true,
        },
        permission: None,
        flags: if isolated { FLAG_ISOLATED_PROCESS } else { 0 },
        foreground_service_type: 0,
    }
}
fn bind(
    host: &AppHost,
    name: &str,
    instance: Option<String>,
    conn: Arc<dyn HostConnection>,
    isolated: bool,
) {
    host.bind_found(
        RawIntent::of(None, 0, &[]).with_component("p", name),
        info(name, isolated),
        instance,
        Conn::Host(conn),
        None,
        None,
        BIND_AUTO_CREATE,
        0,
    )
    .unwrap();
}
fn pending(
    host: &AppHost,
    name: &str,
    isolated: bool,
) -> (
    Binder,
    Arc<dyn HostConnection>,
    mpsc::Receiver<(Option<Binder>, bool)>,
) {
    let (tx, rx) = mpsc::channel();
    let conn: Arc<dyn HostConnection> = Arc::new(Connection(tx));
    bind(host, name, None, conn.clone(), isolated);
    let mut state = host.services.state.lock().unwrap();
    let token = state.records.last().unwrap().token;
    state.pending_services.push(token);
    (token, conn, rx)
}

#[test]
fn paused_start_accepts_unrelated_binds_and_joins_one_isolated_instance() {
    let host = super::super::tests::host_with_start(7, Arc::new(|| {}));
    let (token, conn, rx) = pending(&host, "A", true);
    host.processes_ready.set(Ok(())).unwrap();
    let (entered, wait) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    host.queue_service_start(token, move |_| {
        entered.send(()).unwrap();
        resume.recv_timeout(Duration::from_secs(5)).unwrap();
        Err(Exception::new(EX_ILLEGAL_STATE, "failed fork"))
    });
    wait.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done, result) = mpsc::channel();
    let other = host.clone();
    let binding = std::thread::spawn(move || {
        bind(&other, "A", None, conn.clone(), true);
        bind(&other, "B", Some("one".into()), conn.clone(), true);
        bind(&other, "B", Some("two".into()), conn, true);
        done.send(()).unwrap();
    });
    let accepted = result.recv_timeout(Duration::from_secs(2));
    if accepted.is_ok() {
        let state = host.services.state.lock().unwrap();
        assert_eq!(
            (
                state.records.len(),
                state.pending_services.len(),
                state.connections.len()
            ),
            (3, 3, 4)
        );
    }
    // Always release the worker, including on a failed concurrency assertion.
    release.send(()).unwrap();
    accepted.unwrap();
    binding.join().unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        (None, true)
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        (None, true)
    );
    // Drain the serial start queue before shutting its Binder process down.
    let sender = host
        .services
        .process_starts
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let (done, wait) = mpsc::channel();
    sender
        .send(Box::new(move |_| done.send(()).unwrap()))
        .unwrap();
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(host.services.state.lock().unwrap().records.is_empty());
    host.process.shutdown().unwrap();
}

#[test]
fn death_before_association_or_attach_drops_pending_without_resurrection() {
    for associate in [false, true] {
        let host = super::super::tests::host_with_start(7, Arc::new(|| {}));
        let (token, _, rx) = pending(&host, "A", true);
        if associate {
            host.service_process_started(token, Ok(7));
        }
        host.apps.lock().unwrap().remove(&7);
        host.services_process_died(7, true);
        host.service_process_started(token, Ok(7));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (None, true)
        );
        let state = host.services.state.lock().unwrap();
        assert!(state.records.is_empty() && state.pending_services.is_empty());
        drop(state);
        host.process.shutdown().unwrap();
    }
}

struct NoMemory;
impl aim_binder_driver::GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
        panic!("no user memory")
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
        panic!("no user memory")
    }
    fn get_file(&mut self, _: u32) -> Result<aim_binder_driver::File, i32> {
        panic!("no files")
    }
    fn install_file(&mut self, _: aim_binder_driver::File) -> Result<u32, i32> {
        panic!("no files")
    }
    fn close_fd(&mut self, _: u32) {
        panic!("no files")
    }
}
struct Thread(mpsc::Sender<u32>);
impl aim_binder_host::local::Service for Thread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(
        &self,
        call: &mut aim_binder_host::local::Call<'_>,
    ) -> aim_binder_host::local::Reply {
        self.0.send(call.code).unwrap();
        Ok(Parcel::new())
    }
}
#[test]
fn attach_preserves_pending_order_and_unbound_services_create_then_stop() {
    use aim_binder_driver::{Credentials, Device, Driver, uapi::*};
    use aim_binder_host::local::LocalProcess;
    for early_attach in [false, true] {
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 10123,
                    security_context: None,
                },
            )
        };
        let remote = open(97102);
        let host = AppHost::new(open(97101));
        let template = super::super::tests::host_with_start(7, Arc::new(|| {}));
        let mut app = template.apps.lock().unwrap().remove(&7).unwrap();
        template.process.shutdown().unwrap();
        app.process = "p".into();
        app.uid = 10123;
        app.config = Some(super::super::config::Configuration::from_proto(&[]).unwrap());
        host.apps.lock().unwrap().insert(7, app);
        let path = std::path::Path::new("target/service-start-test-unused");
        super::super::configure_boot_test_host(&host, path, path, path, Box::new(|_| None));
        assert!(
            host.platform
                .set(Arc::new(super::super::Platform {
                    data: path.into(),
                    compat: Default::default(),
                    settings: Default::default(),
                    locales: Mutex::new(super::super::persistent_configuration::Locales::new(
                        "en-US".into()
                    )),
                    serial: String::new(),
                    device_density: 320,
                    shared_memory: Default::default(),
                }))
                .is_ok()
        );
        let (calls, received) = mpsc::channel();
        let Binder::Local(ptr) = remote.add_service(Arc::new(Thread(calls))) else {
            panic!()
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
                remote.proc_handle(),
                97103,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        remote.start();
        // Existing record order differs from pending bring-up order.
        let (first, _, connected) = pending(&host, "A", false);
        let (second, gone, gone_rx) = pending(&host, "B", false);
        host.services.state.lock().unwrap().records.reverse();
        host.unbind_service_from_host(&gone);
        assert_eq!(
            host.services.state.lock().unwrap().pending_services,
            [first, second]
        );
        if !early_attach {
            host.service_process_started(first, Ok(7));
        }
        host.services_attached(7);
        assert_eq!(
            host.services.state.lock().unwrap().pending_services,
            [first, second]
        );
        host.apps.lock().unwrap().get_mut(&7).unwrap().thread = Some(host.process.strong(0));
        host.services_attached(7);
        host.service_process_started(first, Ok(7)); // attach before result association is harmless
        let expected = [
            thread::SCHEDULE_CREATE_SERVICE,
            thread::SCHEDULE_BIND_SERVICE,
            thread::SCHEDULE_CREATE_SERVICE,
            thread::SCHEDULE_STOP_SERVICE,
        ];
        for code in expected {
            assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), code);
        }
        assert!(gone_rx.try_recv().is_err());
        assert!(host.services.state.lock().unwrap().record(second).is_none());
        // Both onServiceConnected and onNullBinding reach the publishing and later connections.
        let intent = || RawIntent::of(None, 0, &[]).with_component("p", "A");
        let binder = (!early_attach).then_some(Binder::Handle(0));
        host.publish_service(Some(first), Some(intent()), binder);
        assert_eq!(
            connected.recv_timeout(Duration::from_secs(2)).unwrap(),
            (binder, false)
        );
        let (late, late_rx) = mpsc::channel();
        bind(&host, "A", None, Arc::new(Connection(late)), false);
        assert_eq!(
            late_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (binder, false)
        );
        assert!(connected.try_recv().is_err());
        assert!(received.try_recv().is_err());
        host.process.shutdown().unwrap();
        remote.shutdown().unwrap();
    }
}
