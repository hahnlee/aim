//! Per-call assertions from Android 16 TimeZoneDetectorService / StrategyImpl,
//! TimeZoneCapabilities.tryApplyConfigChanges (android-16.0.0_r1; source-derived).
use super::super::*;
use super::*;
use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno};
use aim_binder_host::local::LocalProcess;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use aim_service_aidl::android_app_timezonedetector_itimezonedetectorservice as tz;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        panic!("no fixture files")
    }
}
fn open(driver: &Arc<Driver>, pid: i32) -> Arc<LocalProcess> {
    let process = LocalProcess::open(
        driver,
        Device::Binder,
        Credentials {
            pid,
            euid: 1000,
            security_context: None,
        },
    );
    process.set_max_threads(0);
    process.start();
    process
}
fn state(s: &dyn Service, zone: &str, confirm: i32) {
    let p = call(s, tz::GET_TIME_ZONE_STATE, 1000, |_| {});
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    assert_eq!(r.read_string8().unwrap().as_deref(), Some(zone));
    assert_eq!(r.read_i32().unwrap(), confirm);
    assert_eq!(r.remaining(), 0);
}

fn call(s: &dyn Service, code: u32, uid: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(s.descriptor());
    args(&mut p);
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}
fn exception(p: &Parcel) -> i32 {
    p.reader().read_exception().unwrap().unwrap_err().code
}
fn int(p: &Parcel) -> i32 {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    let value = r.read_i32().unwrap();
    assert_eq!(r.remaining(), 0);
    value
}

fn update(s: &dyn Service, changes: &[(&str, Value)]) -> i32 {
    int(&call(s, tz::UPDATE_CONFIGURATION, 1000, |p| {
        p.write_i32(1);
        bundle::write(p, changes);
    }))
}
fn manual(s: &dyn Service, code: u32) -> i32 {
    int(&call(s, code, 1000, |p| {
        p.write_i32(1);
        p.write_string16(Some("Europe/London"));
        p.write_i32(-1);
    }))
}
fn confirm(s: &dyn Service, zone: &str) -> i32 {
    int(&call(s, tz::CONFIRM_TIME_ZONE, 1000, |p| {
        p.write_string16(Some(zone))
    }))
}
fn ok(p: &Parcel) {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.remaining(), 0);
}

fn class(r: &mut Reader<'_>, name: &str) {
    assert_eq!(r.read_string16().unwrap().as_deref(), Some(name));
}

fn capabilities(p: &Parcel, location: i32) {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    class(&mut r, "android.app.time.TimeZoneDetectorStatus");
    assert_eq!(r.read_i32().unwrap(), 3);
    class(&mut r, "android.app.time.TelephonyTimeZoneAlgorithmStatus");
    assert_eq!(r.read_i32().unwrap(), 3); // passive host origin
    class(&mut r, "android.app.time.LocationTimeZoneAlgorithmStatus");
    for expected in [1, 1] {
        assert_eq!(r.read_i32().unwrap(), expected);
    }
    assert!(r.read_string16().unwrap().is_none());
    assert_eq!(r.read_i32().unwrap(), 1);
    assert!(r.read_string16().unwrap().is_none());
    class(&mut r, "android.app.time.TimeZoneCapabilities");
    for expected in [0, 20, location, 10, 20, 10] {
        assert_eq!(r.read_i32().unwrap(), expected);
    }
    class(&mut r, "android.app.time.TimeZoneConfiguration");
    let config = bundle::read(&mut r).unwrap().unwrap();
    assert_eq!(config["autoDetectionEnabled"], Value::Bool(true));
    for key in ["geoDetectionEnabled", "notificationsEnabled"] {
        assert_eq!(config[key], Value::Bool(false));
    }
    assert_eq!(config.len(), 3);
    assert_eq!(r.remaining(), 0);
}

#[test]
fn detector_capabilities_and_rejected_mutations() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let pm =
            permission_manager::PermissionManager::with_system(host.clone(), Default::default());
        assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
        let s = time_zone_detector::TimeZoneDetector::new(host.clone());
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let mut context = (**base.context()).clone();
        context.system.system_permissions = Some(
            [
                (2901, [MANAGE.into()].into()),
                (2902, [SUGGEST.into()].into()),
            ]
            .into(),
        );
        context.scan_version = base.scan().version() + 1;
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                base.scan().owner().clone(),
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        capabilities(&call(&s, tz::GET_CAPABILITIES_AND_CONFIG, 1000, |_| {}), 0);
        for code in [tz::ADD_LISTENER, tz::REMOVE_LISTENER, tz::CONFIRM_TIME_ZONE] {
            assert_eq!(exception(&call(&s, code, 1000, |_| {})), EX_BAD_PARCELABLE);
        }
        for key in [AUTO, GEO, NOTIFICATIONS] {
            for value in [false, true] {
                assert_eq!(update(&s, &[(key, Value::Bool(value))]), 0);
            }
        }
        assert_eq!(update(&s, &[]), 1);
        for code in [tz::SET_MANUAL_TIME_ZONE, tz::SUGGEST_MANUAL_TIME_ZONE] {
            assert_eq!(manual(&s, code), 0);
        }
        for code in [
            tz::UPDATE_CONFIGURATION,
            tz::ADD_LISTENER,
            tz::REMOVE_LISTENER,
            tz::CONFIRM_TIME_ZONE,
            tz::SET_MANUAL_TIME_ZONE,
            tz::SUGGEST_MANUAL_TIME_ZONE,
        ] {
            let (manage, suggest) = if code == tz::SUGGEST_MANUAL_TIME_ZONE {
                (EX_SECURITY, EX_NULL_POINTER)
            } else {
                (EX_NULL_POINTER, EX_SECURITY)
            };
            for (uid, expected) in [
                (1000, EX_NULL_POINTER),
                (99000, EX_SECURITY),
                (2901, manage),
                (2902, suggest),
            ] {
                assert_eq!(
                    exception(&call(&s, code, uid, |p| match code {
                        tz::ADD_LISTENER | tz::REMOVE_LISTENER => p.write_binder(None),
                        tz::CONFIRM_TIME_ZONE => p.write_string16(None),
                        _ => p.write_i32(0),
                    })),
                    expected
                );
            }
        }
        for code in [tz::GET_CAPABILITIES_AND_CONFIG, tz::GET_TIME_ZONE_STATE] {
            assert_eq!(exception(&call(&s, code, 99000, |_| {})), EX_SECURITY);
        }
        assert_eq!(
            exception(&call(&s, tz::SUGGEST_TELEPHONY_TIME_ZONE, 1000, |_| {})),
            EX_UNSUPPORTED_OPERATION
        );
    });
}

#[test]
fn detector_reads_alarm_properties_and_confirms_only_the_current_zone() {
    crate::system::with_discovery_binders(|template, _, _, _| {
        std::fs::copy(
            aim_paths::original_image().join("system/framework/framework.jar"),
            template
                .root()
                .unwrap()
                .join("system/framework/framework.jar"),
        )
        .unwrap();
        let driver = Driver::new();
        let process = open(&driver, 98290);
        let host = AppHost::new(process.clone());
        assert!(
            host.system
                .set(template.package_system().unwrap().clone())
                .is_ok()
        );
        host.package_booted_for_test();
        let properties = Arc::new(Mutex::new(HashMap::from([
            ("ro.build.version.sdk".into(), "36".into()),
            ("ro.build.version.codename".into(), "REL".into()),
            ("persist.sys.locale".into(), "en-US".into()),
            ("ro.sf.lcd_density".into(), "160".into()),
            ("fw.max_users".into(), "1".into()),
            (ZONE.into(), "Asia/Seoul".into()),
            (CONFIDENCE.into(), "0".into()),
        ])));
        let read = properties.clone();
        let write = properties.clone();
        let writes = Arc::new(AtomicUsize::new(0));
        let write_count = writes.clone();
        let dir = std::env::temp_dir().join(format!(
            "aim-2490-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        host.configure(
            template.root().unwrap(),
            &dir,
            &dir,
            &dir,
            Box::new(move |n| read.lock().unwrap().get(n).cloned()),
            Box::new(|_, _, _| Ok(false)),
            Box::new(move |n, v| {
                write_count.fetch_add(1, Ordering::SeqCst);
                write.lock().unwrap().insert(n.into(), v.into());
            }),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        let pm =
            permission_manager::PermissionManager::with_system(host.clone(), Default::default());
        assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
        let services = host.services();
        let published = services
            .iter()
            .find(|(name, _)| *name == "time_zone_detector")
            .unwrap();
        assert_eq!(published.1.descriptor(), tz::DESCRIPTOR);
        let s = host.time_zone_detector.get().unwrap();
        for zone in ["Asia/Seoul", "America/Los_Angeles"] {
            properties.lock().unwrap().insert(ZONE.into(), zone.into());
            state(s.as_ref(), zone, 1);
            let before = properties.lock().unwrap().clone();
            assert_eq!(update(s.as_ref(), &[(AUTO, Value::Bool(false))]), 0);
            assert_eq!(*properties.lock().unwrap(), before);
            let count = writes.load(Ordering::SeqCst);
            for (suggested, expected) in [("Europe/London", 0), (zone, 1)] {
                assert_eq!(confirm(s.as_ref(), suggested), expected);
            }
            assert_eq!(writes.load(Ordering::SeqCst), count + 1);
            assert_eq!(properties.lock().unwrap()[CONFIDENCE], "100");
            state(s.as_ref(), zone, 0);
            assert_eq!(confirm(s.as_ref(), zone), 1);
            assert_eq!(writes.load(Ordering::SeqCst), count + 1);
            properties
                .lock()
                .unwrap()
                .insert(CONFIDENCE.into(), "0".into());
        }
        listener_lifecycle(&host, &driver, s, &properties);
        host.finish_jobs_for_test();
        process.shutdown().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn configuration_follows_support_policy_and_mode_precedence() {
    for (passive, geo, auto, allowed, location, capabilities, status) in [
        (
            true,
            false,
            true,
            false,
            false,
            [20, 0, 10, 20, 10],
            [3, 3, 1],
        ),
        (
            false,
            false,
            true,
            false,
            true,
            [10, 1, 10, 20, 10],
            [1, 1, 1],
        ),
        (
            false,
            false,
            true,
            true,
            false,
            [10, 0, 10, 40, 10],
            [1, 1, 1],
        ),
        (
            true,
            false,
            false,
            true,
            false,
            [40, 0, 10, 40, 10],
            [2, 3, 1],
        ),
        (
            true,
            false,
            true,
            true,
            false,
            [40, 0, 10, 30, 10],
            [3, 3, 1],
        ),
        (
            false,
            true,
            true,
            true,
            true,
            [40, 1, 10, 30, 10],
            [3, 1, 2],
        ),
        (
            true,
            true,
            false,
            true,
            true,
            [40, 1, 30, 40, 10],
            [2, 3, 2],
        ),
        (
            true,
            true,
            true,
            true,
            false,
            [40, 0, 30, 30, 10],
            [3, 3, 2],
        ),
        (true, true, true, true, true, [40, 1, 40, 30, 10], [3, 3, 2]),
    ] {
        assert_eq!(
            Inputs {
                passive,
                geo,
                auto,
                allowed,
                location
            }
            .derive(),
            (capabilities, status)
        );
    }
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "Binder cleanup timed out"
        );
        std::thread::yield_now();
    }
}

fn listener_lifecycle(
    host: &Arc<AppHost>,
    driver: &Arc<aim_binder_driver::Driver>,
    s: &Arc<TimeZoneDetector>,
    properties: &Arc<Mutex<HashMap<String, String>>>,
) {
    use aim_binder_driver::uapi::*;
    use std::sync::mpsc;
    use std::time::Duration;
    struct Listener(std::sync::Weak<TimeZoneDetector>, mpsc::Sender<i32>);
    impl Service for Listener {
        fn descriptor(&self) -> &str {
            listener::DESCRIPTOR
        }
        fn transact(&self, c: &mut Call<'_>) -> Reply {
            if c.code == listener::ON_CHANGE {
                assert!(c.is_oneway());
                listener::OnChange::read(&mut c.data).unwrap();
                assert_eq!(c.data.remaining(), 0);
                let s = self.0.upgrade().unwrap();
                let location = TimeZoneDetector::location(&s.host).unwrap() as i32;
                capabilities(
                    &call(s.as_ref(), tz::GET_CAPABILITIES_AND_CONFIG, 1000, |_| {}),
                    location,
                );
                self.1.send(location).unwrap();
            } else {
                assert_eq!(c.code, u32::MAX); // FIFO barrier on the same oneway node
                self.1.send(-1).unwrap();
            }
            Ok(Parcel::new())
        }
    }
    let Binder::Local(ptr) = host.process.add_service(s.clone()) else {
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
            host.process.proc_handle(),
            98290,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    let remote = open(driver, 98291);
    let dying = open(driver, 98292);
    let (send, events) = mpsc::channel();
    let first = remote.add_service(Arc::new(Listener(Arc::downgrade(s), send.clone())));
    let second = remote.add_service(Arc::new(Listener(Arc::downgrade(s), send.clone())));
    let dead = dying.add_service(Arc::new(Listener(Arc::downgrade(s), send)));
    let register = |process: &LocalProcess, binder, code| {
        let mut p = Parcel::new();
        p.write_interface_token(tz::DESCRIPTOR);
        p.write_binder(Some(binder));
        ok(&process.transact(0, code, &p, false).unwrap().into_parcel());
    };
    register(&remote, first, tz::ADD_LISTENER);
    let first_handle = *s.listeners.lock().unwrap().keys().next().unwrap();
    for binder in [first, second] {
        register(&remote, binder, tz::ADD_LISTENER);
    }
    assert_eq!(s.listeners.lock().unwrap().len(), 2);
    let barrier = || {
        let (sent, done) = mpsc::channel();
        let s = s.clone();
        host.post_job(move |host| {
            let handles: Vec<_> = s.listeners.lock().unwrap().keys().copied().collect();
            for handle in &handles {
                host.process
                    .strong(*handle)
                    .transact(u32::MAX, &Parcel::new(), true)
                    .unwrap();
            }
            sent.send(handles.len()).unwrap();
        });
        let mut pending = done.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut changes = Vec::new();
        while pending > 0 {
            match events.recv_timeout(Duration::from_secs(5)).unwrap() {
                -1 => pending -= 1,
                location => changes.push(location),
            }
        }
        changes
    };
    let settings = settings_provider::SettingsProvider::new(host.clone());
    let location = |value: &str| {
        settings
            .call(
                &settings_provider::Caller {
                    uid: 0,
                    package: Some("root".into()),
                },
                "PUT_secure",
                Some("location_mode"),
                &HashMap::from([("value".into(), Value::String(Some(value.into())))]),
                None,
            )
            .unwrap();
    };
    location("3");
    assert_eq!(barrier(), [1, 1]);
    location("1"); // same effective boolean
    let before = properties.lock().unwrap().clone();
    let inputs = host.inputs().unwrap();
    let setting = inputs.settings.get(Table::Secure, "location_mode");
    assert_eq!(update(s.as_ref(), &[]), 1);
    assert_eq!(update(s.as_ref(), &[(AUTO, Value::Bool(false))]), 0);
    for code in [tz::SET_MANUAL_TIME_ZONE, tz::SUGGEST_MANUAL_TIME_ZONE] {
        assert_eq!(manual(s.as_ref(), code), 0);
    }
    assert_eq!(*properties.lock().unwrap(), before);
    assert_eq!(inputs.settings.get(Table::Secure, "location_mode"), setting);
    properties
        .lock()
        .unwrap()
        .insert(ZONE.into(), "Europe/London".into());
    assert_eq!(confirm(s.as_ref(), "Europe/London"), 1);
    assert!(barrier().is_empty());
    for _ in 0..2 {
        register(&remote, first, tz::REMOVE_LISTENER);
    }
    assert_eq!(s.listeners.lock().unwrap().len(), 1);
    location("0");
    assert_eq!(barrier(), [0]);
    let mut info = [0; NODE_INFO_FOR_REF_SIZE];
    info[..4].copy_from_slice(&first_handle.to_le_bytes());
    assert_eq!(
        driver.ioctl(
            host.process.proc_handle(),
            98290,
            BINDER_GET_NODE_INFO_FOR_REF,
            &mut info,
            &mut NoMemory
        ),
        Err(errno::EINVAL), // no retained strong reference for the removed listener
    );
    let survivor = *s.listeners.lock().unwrap().keys().next().unwrap();
    register(&dying, dead, tz::ADD_LISTENER);
    let dead_handle = *s
        .listeners
        .lock()
        .unwrap()
        .keys()
        .find(|&&h| h != survivor)
        .unwrap();

    dying.shutdown().unwrap();
    wait_until(|| !s.listeners.lock().unwrap().contains_key(&dead_handle));
    assert_eq!(s.listeners.lock().unwrap().len(), 1);
    location("3");
    assert_eq!(barrier(), [1]);
    let late = host.process.strong(survivor);
    register(&remote, second, tz::REMOVE_LISTENER);
    assert!(s.listeners.lock().unwrap().is_empty());
    remote.shutdown().unwrap();
    ok(&call(s.as_ref(), tz::ADD_LISTENER, 1000, |p| {
        p.write_binder(Some(late.binder()))
    }));
    wait_until(|| s.listeners.lock().unwrap().is_empty());
}
