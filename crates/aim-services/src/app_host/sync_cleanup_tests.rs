//! Pinned retained SyncManager bodies drive native owners over a synchronous test pipe.
use super::*;
use aim_service_aidl::{
    android_os_iservicemanager as sm, com_android_internal_app_ibatterystats as bs,
};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[path = "sync_acceptance_tests.rs"]
mod acceptance;

// Bootstrap calls relay parcels through Binder so every process gets its own handles.
// The private registration codes are fixture protocols, never production routes.
struct Registry {
    process: std::sync::Weak<aim_binder_host::local::LocalProcess>,
    fallback: Mutex<Option<Strong>>,
    services: Mutex<std::collections::HashMap<String, Strong>>,
}
impl Service for Registry {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == 0x00ff_ff00 {
            let Some(Binder::Handle(h)) = call.data.read_binder()? else {
                panic!("remote host expected")
            };
            *self.fallback.lock().unwrap() = Some(self.process.upgrade().unwrap().strong(h));
            return Ok(Parcel::new());
        }
        if call.code == 0x00ff_ff01 {
            let name = call.data.read_string16()?.unwrap();
            let Some(Binder::Handle(h)) = call.data.read_binder()? else {
                panic!("remote service expected")
            };
            self.services
                .lock()
                .unwrap()
                .insert(name, self.process.upgrade().unwrap().strong(h));
            return Ok(Parcel::new());
        }
        let at = call.data.position();
        let manager = call.data.enforce_interface(sm::DESCRIPTOR).is_ok();
        call.data.set_position(at);
        if !manager {
            call.data.skip(call.data.remaining())?;
            let (bytes, objects) = call.data.since(at);
            let mut request = Parcel::new();
            request.write_raw(bytes, &objects);
            return self
                .fallback
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .transact(call.code, &request, call.is_oneway())
                .map(|r| r.into_parcel());
        }
        assert_eq!(call.code, sm::CHECK_SERVICE);
        let a = sm::CheckService::read(&mut call.data)?;
        let mut p = Parcel::new();
        sm::write_check_service_reply(
            &mut p,
            self.services
                .lock()
                .unwrap()
                .get(a.name.as_deref().unwrap())
                .map(Strong::binder),
        );
        Ok(p)
    }
}
struct Suspend {
    process: std::sync::Weak<aim_binder_host::local::LocalProcess>,
    releases: Arc<Mutex<Vec<usize>>>,
}
struct Released(Arc<Mutex<Vec<usize>>>, usize);
impl Service for Released {
    fn descriptor(&self) -> &str {
        WAKE_LOCK_DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(WAKE_LOCK_DESCRIPTOR)?;
        assert_eq!(call.code, RELEASE);
        let mut counts = self.0.lock().unwrap();
        counts[self.1] += 1;
        assert_eq!(counts[self.1], 1, "duplicate blocker release");
        Ok(Parcel::new())
    }
}
impl Service for Suspend {
    fn descriptor(&self) -> &str {
        SYSTEM_SUSPEND_DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(SYSTEM_SUSPEND_DESCRIPTOR)?;
        assert_eq!(call.code, ACQUIRE_WAKE_LOCK);
        assert_eq!(call.data.read_i32()?, WAKE_LOCK_PARTIAL);
        assert_eq!(
            call.data.read_string16()?.as_deref(),
            Some("PowerManagerService.WakeLocks")
        );
        let index = self.releases.lock().unwrap().len();
        self.releases.lock().unwrap().push(0);
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_binder(Some(
            self.process
                .upgrade()
                .unwrap()
                .add_service(Arc::new(Released(self.releases.clone(), index))),
        ));
        Ok(p)
    }
}
// Separate driver processes own the probe and tokens; neither dies with package bootstrap.
struct Peer(Arc<aim_binder_host::local::LocalProcess>);
impl Peer {
    fn new(driver: &Arc<aim_binder_driver::Driver>, pid: i32) -> Self {
        let p = aim_binder_host::local::LocalProcess::open(
            driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid,
                euid: 1000,
                security_context: None,
            },
        );
        p.start();
        Self(p)
    }
    fn register(&self, name: &str, service: Arc<dyn Service>) {
        let mut request = Parcel::new();
        request.write_string16(Some(name));
        request.write_binder(Some(self.0.add_service(service)));
        self.0
            .strong(0)
            .transact(0x00ff_ff01, &request, false)
            .unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.0.shutdown().unwrap();
    }
}
struct Connection(std::sync::mpsc::Sender<bool>);
impl super::super::services::HostConnection for Connection {
    fn connected(&self, _: Option<Binder>, dead: bool) {
        self.0.send(dead).unwrap();
    }
}
struct Source(i32);
impl WriteParcelable for Source {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1);
        aim_service_aidl::write_int_array(p, Some(&[self.0]));
        p.write_i32(-1);
        p.write_i32(-1);
    }
}
fn invoke(service: &dyn Service, code: u32, data: &Parcel) {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: 1000,
            data: data.reader(),
        })
        .unwrap()
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
}
fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let a = source.find(start).unwrap();
    &source[a..a + source[a..].find(end).unwrap()]
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

#[test]
#[ignore = "requires pinned AOSP source and JDK; execute explicitly without a guest"]
fn retained_sync_cleanup_reaches_native_owners() {
    run(false);
}

#[test]
#[ignore = "requires pinned AOSP source, image and JDK; execute explicitly without a guest"]
fn retained_sync_cross_owner_acceptance() {
    run(true);
}

fn run(acceptance: bool) {
    use sha2::{Digest, Sha256};
    let source = fs::read_to_string(
        aim_paths::aosp()
            .join("frameworks/base/services/core/java/com/android/server/content/SyncManager.java"),
    )
    .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(source.as_bytes())),
        "ac8deedeeca97dadea2d9da8b7e2956305fd63e44d9fdb8d80222ddf76f1ed51"
    );
    // Apache-2.0, AOSP android-16.0.0_r1. Bodies remain unchanged; only Android boundaries double.
    let mut java = include_str!("sync_cleanup_fixture.java").to_owned();
    java = java.replace("@ACCEPTANCE@", include_str!("sync_acceptance_fixture.java"));
    for (key, start, end) in [
        (
            "@SCHEDULE@",
            "    private void scheduleSyncOperationH(SyncOperation syncOperation) {",
            "    /**\n     * Remove scheduled sync operations.",
        ),
        (
            "@EXEMPTED_START@",
            "            if (op.isAppStandbyExempted()) {\n                final UsageStatsManagerInternal usmi",
            "            // Connect to the sync adapter.",
        ),
        (
            "@RECONNECT@",
            "    private BroadcastReceiver mConnectivityIntentReceiver =",
            "    private String getJobStats()",
        ),
        (
            "@ACTIVE@",
            "    class ActiveSyncContext extends",
            "    protected void dump(",
        ),
        (
            "@HANDLE@",
            "        public void handleMessage(Message msg)",
            "        private void handleSyncMessage(",
        ),
        (
            "@LOCK@",
            "        private PowerManager.WakeLock getSyncWakeLock(",
            "        /**",
        ),
        (
            "@BOUND@",
            "        private void runBoundToAdapterH(",
            "        /**",
        ),
        (
            "@FINISH@",
            "        private void runSyncFinishedOrCanceledH(",
            "        private void closeActiveSyncContext(",
        ),
        (
            "@CLOSE@",
            "        private void closeActiveSyncContext(",
            "        /**",
        ),
        (
            "@DISCONNECT@",
            "                    case SyncHandler.MESSAGE_SERVICE_DISCONNECTED: {",
            "                    case SyncHandler.MESSAGE_MONITOR_SYNC:",
        ),
        (
            "@STILL@",
            "    private boolean isSyncStillActiveH(",
            "    /**",
        ),
    ] {
        java = java.replace(key, section(&source, start, end));
    }
    let work = aim_paths::scratch_dir("sync-cleanup-java");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("Cleanup.java"), java).unwrap();
    let bin =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    assert!(
        Command::new(bin.join("javac"))
            .arg("-d")
            .arg(&work)
            .arg(work.join("Cleanup.java"))
            .status()
            .unwrap()
            .success()
    );
    let peers = Mutex::new(None);
    let flags = [("android.permission.flags.appop_mode_caching_enabled", true)];
    crate::system::with_discovery_context(
        if acceptance { &flags } else { &[] },
        true,
        true,
        |driver, p, _| {
            *peers.lock().unwrap() = Some((Peer::new(driver, 97120), Peer::new(driver, 97121)));
            Arc::new(Registry {
                process: Arc::downgrade(p),
                fallback: Mutex::new(None),
                services: Mutex::new(Default::default()),
            })
        },
        |host, publish, _, export, _| {
            if !acceptance {
                let permissions = super::super::permission_manager::PermissionManager::with_system(
                    host.clone(),
                    Default::default(),
                );
                assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            }
            publish(&mut |p| {
                p.services.push(crate::package::pkg::Service {
                    main: crate::package::pkg::MainComponent {
                        component: crate::package::pkg::Component {
                            name: format!("{}.Sync", p.package_name),
                            package_name: p.package_name.clone(),
                            ..Default::default()
                        },
                        process_name: Some(p.package_name.clone()),
                        enabled: true,
                        exported: true,
                        direct_boot_aware: true,
                        ..Default::default()
                    },
                    foreground_service_type: 0,
                    permission: None,
                })
            });
            if acceptance {
                super::super::job_scheduler::sync_job_tests::install_sync_manifest(host);
            }
            let power = Arc::new(PowerManager::new(host.clone()));
            assert!(host.power_service.set(power.clone()).is_ok());
            power.state.lock().unwrap().asleep = true; // No display blocker or assertion for these partial locks.
            let releases = Arc::new(Mutex::new(Vec::new()));
            let (probe, owner) = peers.lock().unwrap().take().unwrap();
            probe.register(
                SYSTEM_SUSPEND,
                Arc::new(Suspend {
                    process: Arc::downgrade(&probe.0),
                    releases: releases.clone(),
                }),
            );
            let tokens: Vec<_> = (0..3)
                .map(|i| {
                    let name = format!("test.token.{i}");
                    owner.register(&name, power.clone());
                    super::super::display::check_service(&host.process, &name).unwrap()
                })
                .collect();
            let acceptance = acceptance.then(|| acceptance::Owners::new(host, export));
            let (tx, rx) = std::sync::mpsc::channel();
            let mut connections = std::collections::HashMap::new();
            let mut child = Child(
                Command::new(bin.join("java"))
                    .arg("-cp")
                    .arg(&work)
                    .arg("Cleanup")
                    .args(acceptance.as_ref().map(|_| "acceptance"))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let mut input = child.0.stdin.take().unwrap();
            for line in BufReader::new(child.0.stdout.take().unwrap()).lines() {
                let line = line.unwrap();
                let fields: Vec<_> = line.split_whitespace().collect();
                let number = |i: usize| fields[i].parse::<i32>().unwrap();
                let mut p = Parcel::new();
                let mut answer = 1;
                let was_held = !power.state.lock().unwrap().locks.is_empty();
                let previous = releases.lock().unwrap().len();
                match fields[0] {
                    "acquire" => {
                        pm::AcquireWakeLock {
                            lock: Some(tokens[number(1) as usize].binder()),
                            flags: PARTIAL_WAKE_LOCK,
                            tag: Some(fields[2].into()),
                            package_name: Some("android".into()),
                            ws: (number(3) >= 0).then(|| Source(number(3))),
                            history_tag: None,
                            display_id: INVALID_DISPLAY,
                            callback: None,
                        }
                        .write(&mut p);
                        invoke(power.as_ref(), pm::ACQUIRE_WAKE_LOCK, &p);
                    }
                    "release" => {
                        pm::ReleaseWakeLock {
                            lock: Some(tokens[number(1) as usize].binder()),
                            flags: 0,
                        }
                        .write(&mut p);
                        invoke(power.as_ref(), pm::RELEASE_WAKE_LOCK, &p);
                    }
                    "source" => {
                        pm::UpdateWakeLockWorkSource {
                            lock: Some(tokens[number(1) as usize].binder()),
                            ws: (number(2) >= 0).then(|| Source(number(2))),
                            history_tag: None,
                        }
                        .write(&mut p);
                        invoke(power.as_ref(), pm::UPDATE_WAKE_LOCK_WORK_SOURCE, &p);
                    }
                    "start" | "finish" => {
                        let start = fields[0] == "start";
                        if start {
                            bs::NoteSyncStart {
                                name: Some(fields[1].into()),
                                uid: number(2),
                            }
                            .write(&mut p);
                        } else {
                            bs::NoteSyncFinish {
                                name: Some(fields[1].into()),
                                uid: number(2),
                            }
                            .write(&mut p);
                        }
                        invoke(
                            power.battery().as_ref(),
                            if start {
                                bs::NOTE_SYNC_START
                            } else {
                                bs::NOTE_SYNC_FINISH
                            },
                            &p,
                        );
                        power.battery().assert_sync_nesting_for_test(
                            number(2),
                            fields[1],
                            i32::from(start),
                        );
                    }
                    "bind" => {
                        let conn: Arc<dyn super::super::services::HostConnection> =
                            Arc::new(Connection(tx.clone()));
                        answer = host
                            .bind_service_from_host(
                                None,
                                "p",
                                if number(2) == 0 {
                                    "p.Missing"
                                } else {
                                    "p.Sync"
                                },
                                0,
                                0,
                                0,
                                conn.clone(),
                            )
                            .unwrap() as i32;
                        if answer != 0 {
                            connections.insert(number(1), conn);
                        }
                    }
                    "unbind" => {
                        host.unbind_service_from_host(&connections.remove(&number(1)).unwrap())
                    }
                    "bindings" => host.assert_sync_service_bindings_for_test(number(1) as usize),
                    "process-death" => {
                        host.sync_service_death_for_test();
                        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
                    }
                    "check" => {
                        let state = power.state.lock().unwrap();
                        assert_eq!(state.locks.len(), number(1) as usize, "{line}");
                        assert!(state.screen_assertion.name().is_none());
                        drop(state);
                        assert_eq!(
                            power.battery().wake_nesting(number(2), fields[3]),
                            number(4),
                            "{line}"
                        );
                    }
                    "idle" => {
                        assert!(power.state.lock().unwrap().locks.is_empty());
                        assert!(power.state.lock().unwrap().wake_locks.is_none());
                        host.assert_sync_service_bindings_for_test(0);
                        assert!(connections.is_empty());
                        power.battery().assert_sync_cleanup_for_test();
                    }
                    "owner-death" => {
                        owner.0.shutdown().unwrap();
                        let end = Instant::now() + Duration::from_secs(2);
                        while !power.state.lock().unwrap().locks.is_empty() {
                            assert!(Instant::now() < end);
                            std::thread::yield_now();
                        }
                    }
                    _ => answer = acceptance.as_ref().unwrap().request(&fields),
                }
                // One compatibility blocker per nonempty interval, including loop-only work.
                // Source changes and overlapping adapters must retain it, with no extra release.
                let state = power.state.lock().unwrap();
                let held = !state.locks.is_empty();
                assert_eq!(state.wake_locks.is_some(), held, "{line}");
                assert!(state.display.is_none() && state.screen_assertion.name().is_none());
                drop(state);
                let expected = previous + usize::from(!was_held && held);
                let end = Instant::now() + Duration::from_secs(2);
                loop {
                    let counts = releases.lock().unwrap();
                    assert_eq!(counts.len(), expected, "{line}");
                    assert!(counts.iter().all(|n| *n <= 1), "duplicate release: {line}");
                    if counts
                        .iter()
                        .enumerate()
                        .all(|(i, n)| *n == usize::from(!held || i + 1 != expected))
                    {
                        break;
                    }
                    assert!(Instant::now() < end, "blocker lifetime: {line}: {counts:?}");
                    drop(counts);
                    std::thread::yield_now();
                }
                writeln!(input, "{answer}").unwrap();
                input.flush().unwrap();
            }
            assert!(child.0.wait().unwrap().success());
            let counts = releases.lock().unwrap();
            assert!(!counts.is_empty() && counts.iter().all(|n| *n == 1));
            eprintln!(
                "retained cleanup: {} blockers acquired, each released exactly once",
                counts.len()
            );
        },
    );
    fs::remove_dir_all(work).unwrap();
}
