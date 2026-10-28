//! ActivityManagerService.setSystemProcess / ActivityThread.handleReceiver,
//! android-16.0.0_r1: manifest deliveries and finishReceiver share the
//! attached system ApplicationThread, regardless of the declaring package.
use super::*;
use crate::package::{info, model, pkg};
use aim_binder_host::local::{Reply, Service};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct SystemThread(mpsc::Sender<RawIntent>);
impl Service for SystemThread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_RECEIVER);
        call.data.enforce_interface(thread::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 1);
        let intent = RawIntent::read_from(&mut call.data)?;
        assert_eq!(call.data.read_i32()?, 1); // ActivityInfo
        let component = intent.intent.component.as_ref().unwrap();
        assert_eq!(
            call.data.read_string8()?.as_deref(),
            Some(component.class.as_str())
        );
        assert_eq!(
            call.data.read_string8()?.as_deref(),
            Some(component.package.as_str())
        );
        self.0.send(intent).unwrap();
        Ok(Parcel::new())
    }
}

fn receiver(package: &str, class: &str, uid: u32) -> Arc<Manifest> {
    let pkg = pkg::AndroidPackage {
        package_name: package.into(),
        uid: uid as i32,
        target_sdk_version: 36,
        process_name: Some("system".into()),
        ..Default::default()
    };
    let state = model::PackageUserState {
        installed: true,
        ..Default::default()
    };
    let activity = pkg::Activity {
        main: pkg::MainComponent {
            component: pkg::Component {
                name: class.into(),
                package_name: package.into(),
                ..Default::default()
            },
            process_name: Some("system".into()),
            exported: true,
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let info = info::generate_activity_info(
        &info::Target {
            sys: &model::System::default(),
            pkg: &pkg,
            ps: &model::PackageState::default(),
            state: &state,
            user: 0,
        },
        &activity,
        0,
        None,
    )
    .unwrap();
    assert_eq!(info.info.application_info.uid, uid as i32);
    Arc::new(Manifest {
        info,
        process: "system".into(),
        uid,
        priority: 0,
    })
}

#[test]
fn system_manifest_receivers_are_delivered_and_finished_in_the_attached_process() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (send, received) = mpsc::channel();
        let thread = export(Arc::new(SystemThread(send)));
        let who = thread.binder();
        let seq = 1000;
        host.apps.lock().unwrap().insert(
            seq,
            super::super::App {
                package: "android".into(),
                packages: vec!["android".into()],
                process: "system".into(),
                uid: 1000,
                package_uid: 1000,
                isolated: false,
                entry_point: None,
                crash_handler: None,
                application_info: None,
                pid: 42,
                started: Instant::now(),
                start_elapsed: 0,
                start_uptime: 0,
                thread: Some(thread),
                activities: vec![],
                config: Some(Default::default()),
                providers: vec![],
                removed: false,
                first_frame: None,
                last_activity: 0,
                persistent: true,
                package_deps: vec![],
                last_mem_info: None,
                proc_state: PROCESS_STATE_TOP,
                oom_adj: 0,
            },
        );
        let record = Arc::new(Record {
            intent: RawIntent::new(
                "android.intent.action.BOOT_COMPLETED",
                FLAG_RECEIVER_INCLUDE_BACKGROUND,
                &[],
            ),
            binders: Arc::new(vec![]),
            sender: Sender::system(&[]),
            ordered: false,
            initial_sticky: false,
            user: 0,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
            id_for_response_event: 0,
            temp_allowlist: None,
            urgent: false,
        });
        let targets = [
            receiver("android", "com.android.server.BootReceiver", 1000),
            receiver(
                "android",
                "com.android.server.updates.CertPinInstallReceiver",
                1000,
            ),
            receiver(
                "another.system.package",
                "another.system.package.Receiver",
                1000,
            ),
        ];
        // The process name alone never gives a different uid access to it.
        assert!(matches!(
            host.broadcasts.manifest_queue(
                host,
                &receiver("ordinary.package", "ordinary.package.Receiver", 10123)
            ),
            QueueId::Cold(_, 10123)
        ));
        {
            let mut state = host.broadcasts.state.lock().unwrap();
            host.broadcasts.enqueue(
                host,
                &mut state,
                &record,
                targets.iter().cloned().map(Target::Manifest).collect(),
            );
            assert!(state.busy.contains(&seq));
            assert!(state.cold.is_empty());
        }
        for target in &targets {
            let delivered = received.recv_timeout(Duration::from_secs(5)).unwrap();
            let component = delivered.intent.component.unwrap();
            assert_eq!(component.package, manifest_package(target));
            assert_eq!(
                Some(component.class.as_str()),
                target.info.info.item.name.as_deref()
            );
            assert!(
                received.try_recv().is_err(),
                "the next receiver must wait for finishReceiver"
            );
            host.broadcasts
                .finish(
                    host,
                    am::FinishReceiver {
                        who: Some(who),
                        result_code: 0,
                        result_data: None,
                        map: None,
                        abort_broadcast: false,
                        flags: 0,
                    },
                )
                .unwrap();
        }
        let state = host.broadcasts.state.lock().unwrap();
        assert!(!state.busy.contains(&seq));
        assert!(state.queues.get(&seq).is_none_or(VecDeque::is_empty));
        assert!(state.cold.is_empty());
        assert_eq!(host.apps.lock().unwrap().len(), 1);
    });
}

#[test]
fn system_manifest_receivers_keep_export_checks() {
    let host = AppHost::new(crate::test_process(1000));
    let mut target = receiver("android", "com.android.server.Receiver", 1000);
    Arc::get_mut(&mut target).unwrap().info.info.exported = false;
    let mut sender = Sender::system(&[]);
    sender.uid = 10123;
    for preflight in [true, false] {
        let denial = host
            .broadcasts
            .skip_manifest(&host, &sender, &target.info, &Intent::default(), preflight)
            .unwrap()
            .unwrap();
        assert!(denial.contains("not exported from uid 1000"));
    }
}
