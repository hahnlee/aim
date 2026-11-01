//! Cross-owner native side of the pinned handler fixture; no guest or host network changes.
use super::*;
use crate::app_host::{
    connectivity_service::{ConnectivityService, broadcast_tests as network},
    job_info,
    job_scheduler::{JobScheduler, sync_job_tests as jobs},
    kept::RawIntent,
    system_process::SystemProcess,
};
use aim_service_aidl::{android_app_job_ijobscheduler as js, dev_aim_server_isystemprocess as sp};
use std::sync::mpsc;

pub(super) struct Owners {
    host: Arc<AppHost>,
    jobs: Arc<JobScheduler>,
    connectivity: Arc<ConnectivityService>,
    events: mpsc::Receiver<(RawIntent, bool)>,
    _image: aim_storage::system::ImageLease,
}
impl Owners {
    pub(super) fn new(host: &Arc<AppHost>, export: &dyn Fn(Arc<dyn Service>) -> Strong) -> Self {
        let image = aim_storage::system::ImageLease::read(&aim_paths::system_image()).unwrap();
        assert!(
            host.framework
                .set(
                    crate::app_host::resources::Framework::load(
                        &aim_paths::original_image(),
                        160,
                        "en-US",
                    )
                    .unwrap()
                )
                .is_ok()
        );
        assert!(host.app_idle_enabled().unwrap());
        host.inputs()
            .unwrap()
            .settings
            .set_config(
                "device_idle/",
                &mut [(
                    "device_idle/max_temp_app_allowlist_duration_ms".into(),
                    Some("100000".into()),
                )],
                Some("android"),
                true,
            )
            .unwrap();
        let _ = crate::app_host::broadcasts::internal_package_tests::register_filter(
            host, export, "p", 1000, false,
        );
        let (tx, events) = mpsc::channel();
        let receiver = export(Arc::new(network::Receiver(tx)));
        let binder = receiver.binder();
        host.apps.lock().unwrap().get_mut(&1000).unwrap().thread = Some(receiver);
        let connectivity = Arc::new(ConnectivityService::for_test(host.clone(), None));
        assert!(host.connectivity_service.set(connectivity.clone()).is_ok());
        assert!(network::register(host, binder, true).is_none());
        connectivity.system_ready();
        let now = crate::app_host::clock_ms(libc::CLOCK_MONOTONIC_RAW);
        let mut history = crate::app_host::battery_history::tests::history(
            &host.data().unwrap().join("acceptance-history"),
            128 * 1024,
            now,
        );
        history.set_initial_battery_state(false, 3, 50, 0);
        history.start_recording(now, true);
        host.power_service
            .get()
            .unwrap()
            .battery()
            .set_history_for_test(history);
        Self {
            host: host.clone(),
            jobs: jobs::scheduler(host),
            connectivity,
            events,
            _image: image,
        }
    }
    pub(super) fn request(&self, fields: &[&str]) -> i32 {
        let number = |i: usize| fields[i].parse::<i32>().unwrap();
        let mut p = Parcel::new();
        let system = SystemProcess::new(self.host.clone());
        match fields[0] {
            "bucket" => {
                sp::GetSyncAdapterStandbyBucket {
                    package_name: Some("p".into()),
                    user_id: 0,
                }
                .write(&mut p);
                let r = dispatch(&system, 1000, sp::GET_SYNC_ADAPTER_STANDBY_BUCKET, &p);
                return sp::read_get_sync_adapter_standby_bucket_reply(&mut r.reader())
                    .unwrap()
                    .unwrap();
            }
            "scheduled" => {
                sp::ReportSyncScheduled {
                    package_name: Some("p".into()),
                    user_id: 0,
                    exempted: number(1) != 0,
                }
                .write(&mut p);
                invoke(&system, sp::REPORT_SYNC_SCHEDULED, &p);
            }
            "exempted-start" => {
                sp::ReportExemptedSyncStart {
                    package_name: Some("p".into()),
                    user_id: 0,
                }
                .write(&mut p);
                invoke(&system, sp::REPORT_EXEMPTED_SYNC_START, &p);
            }
            "allowlist" => {
                sp::AddSyncTempAllowlist {
                    calling_uid: 1000,
                    package_name: Some("p".into()),
                    duration_ms: 30000,
                    r#type: number(1),
                    user_id: 0,
                    sync: false,
                    reason_code: 306,
                    reason: Some("sync by top app".into()),
                }
                .write(&mut p);
                let r = dispatch(&system, 1000, sp::ADD_SYNC_TEMP_ALLOWLIST, &p);
                if number(1) == 2 {
                    let e = r.reader().read_exception().unwrap().unwrap_err();
                    assert_eq!(e.code, EX_UNSUPPORTED_OPERATION);
                    assert!(e.message.contains("#2016"));
                    return -1;
                }
                r.reader().read_exception().unwrap().unwrap();
            }
            "membership" => {
                use aim_service_aidl::android_os_ideviceidlecontroller as di;
                di::GetAppIdTempWhitelist {}.write(&mut p);
                let r = dispatch(
                    self.host.device_idle().as_ref(),
                    1000,
                    di::GET_APP_ID_TEMP_WHITELIST,
                    &p,
                );
                assert!(
                    di::read_get_app_id_temp_whitelist_reply(&mut r.reader())
                        .unwrap()
                        .unwrap()
                        .unwrap()
                        .contains(&10100)
                );
                assert_eq!(self.host.fgs_temp_reason_for_test(10100), None);
            }
            "job" => {
                let mut job = jobs::sync_job();
                job.id = number(1);
                job.min_latency = fields[2].parse().unwrap();
                job.has_early_constraint = job.min_latency > 0;
                job.flags = number(3)
                    | if number(4) != 0 {
                        job_info::FLAG_EXPEDITED
                    } else {
                        0
                    };
                job.priority = if number(4) != 0 {
                    job_info::PRIORITY_MAX
                } else {
                    job_info::PRIORITY_DEFAULT
                };
                job.constraint_flags = job_info::CONSTRAINT_FLAG_STORAGE_NOT_LOW
                    | if number(5) != 0 {
                        job_info::CONSTRAINT_FLAG_CHARGING
                    } else {
                        0
                    };
                job.network = Some(job_info::restored_request(
                    if number(6) == 2 { &[12, 11] } else { &[12] },
                    &[],
                    &[],
                ));
                let period = fields[7].parse().unwrap();
                if period > 0 {
                    job.set_periodic(period, fields[8].parse().unwrap());
                }
                use crate::package::restrictions::persistable::Value as V;
                for (key, value) in &mut job.extras.entries {
                    *value = match key.as_deref().unwrap() {
                        "jobId" => V::Int(job.id),
                        "syncExemptionFlag" => V::Int(number(9)),
                        "expectedRuntime" => V::Long(fields[10].parse().unwrap()),
                        "ejDowngradedToRegular" => V::Bool(number(11) != 0),
                        "isPeriodic" => V::Bool(job.periodic),
                        "periodMillis" => V::Long(job.interval),
                        "flexMillis" => V::Long(job.flex),
                        _ => value.clone(),
                    };
                }
                js::ScheduleAsPackage {
                    namespace: Some("SyncManager".into()),
                    job: Some(job.clone()),
                    package_name: Some("p".into()),
                    user_id: 0,
                    tag: Some(fields[12].into()),
                }
                .write(&mut p);
                let r = dispatch(self.jobs.as_ref(), 1000, js::SCHEDULE_AS_PACKAGE, &p);
                assert_eq!(
                    js::read_schedule_as_package_reply(&mut r.reader())
                        .unwrap()
                        .unwrap(),
                    1
                );
                let stored = self
                    .jobs
                    .own_pending_job("SyncManager", job.id)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    (
                        stored.min_latency,
                        stored.flags,
                        stored.constraint_flags,
                        stored.periodic
                    ),
                    (
                        job.min_latency,
                        job.flags,
                        job.constraint_flags,
                        job.periodic
                    )
                );
                job.network.as_mut().unwrap().caps.uids = Some(vec![(10100, 10100)]);
                assert_eq!(stored.network, job.network);
                jobs::same_extras(&stored.extras, &job.extras);
                let reopened = jobs::scheduler(&self.host)
                    .own_pending_job("SyncManager", job.id)
                    .unwrap()
                    .unwrap();
                jobs::same_extras(&reopened.extras, &job.extras);
            }
            "pending" => {
                return i32::from(
                    self.jobs
                        .own_pending_job("SyncManager", number(1))
                        .unwrap()
                        .is_some(),
                );
            }
            "cancel-job" => {
                js::Cancel {
                    namespace: Some("SyncManager".into()),
                    job_id: number(1),
                }
                .write(&mut p);
                invoke(self.jobs.as_ref(), js::CANCEL, &p);
                assert!(
                    self.jobs
                        .own_pending_job("SyncManager", number(1))
                        .unwrap()
                        .is_none()
                );
            }
            "network-read" => return i32::from(network::active(&self.connectivity)),
            "network" => {
                let was = self.connectivity.default_network().map(|n| n.0);
                self.connectivity.follow_network(if number(1) == 0 {
                    None
                } else {
                    network::lease(number(1) as u8)
                });
                let now = self.connectivity.default_network().map(|n| n.0);
                let count = if was == now {
                    0
                } else if was.is_some() && now.is_some() {
                    2
                } else {
                    1
                };
                for i in 0..count {
                    network::receive(
                        &self.events,
                        now.is_some() && (count != 2 || i == 1),
                        count == 2 && i == 0,
                        false,
                    );
                }
                assert!(self.events.try_recv().is_err());
                return count;
            }
            "unsupported" => {
                // Unsupported calls preserve the type-1 exclusion from FGS rights.
                assert_eq!(self.host.fgs_temp_reason_for_test(10100), None);
                sp::ReportSyncScheduled {
                    package_name: Some("p".into()),
                    user_id: 0,
                    exempted: true,
                }
                .write(&mut p);
                assert_eq!(
                    dispatch(&system, 10100, sp::REPORT_SYNC_SCHEDULED, &p)
                        .reader()
                        .read_exception()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    aim_binder_host::parcel::EX_SECURITY
                );
                // An unavailable native owner is an error, never a successful promotion.
                let absent = SystemProcess::new(AppHost::new(crate::test_process(1000)));
                assert!(
                    dispatch(&absent, 1000, sp::REPORT_SYNC_SCHEDULED, &p)
                        .reader()
                        .read_exception()
                        .unwrap()
                        .is_err()
                );
            }
            "history" => {
                let buffer = self
                    .host
                    .power_service
                    .get()
                    .unwrap()
                    .battery()
                    .history_buffer();
                let events = crate::app_host::battery_history::tests::events(&buffer);
                assert_eq!(
                    events
                        .iter()
                        .filter(|(code, _, _)| *code & 0x3fff == 17)
                        .cloned()
                        .collect::<Vec<_>>(),
                    [(0x8011, "sync by top app".into(), 10100)]
                );
                let sync: Vec<_> = events
                    .into_iter()
                    .filter(|(code, _, _)| code & 0x3fff == 4)
                    .collect();
                for (name, uid, count) in [("a", 10100, 9), ("b", 10101, 1)] {
                    for code in [0x8004, 0x4004] {
                        assert_eq!(
                            sync.iter()
                                .filter(|e| **e == (code, name.into(), uid))
                                .count(),
                            count
                        );
                    }
                }
                assert_eq!(sync.len(), 20);
            }
            "persist" => {
                self.host.flush_standby().unwrap();
                let xml = fs::read_to_string(
                    self.host
                        .data()
                        .unwrap()
                        .join("system/users/0/app_idle_stats.xml"),
                )
                .unwrap();
                assert!(xml.contains("name=\"p\"") && xml.contains("<expiryTimes>"));
                assert!(
                    self.jobs
                        .system_scheduled_own_jobs(Some("SyncManager"))
                        .unwrap()
                        .is_empty()
                );
            }
            _ => panic!("unknown acceptance request: {fields:?}"),
        }
        1
    }
}
fn dispatch(owner: &dyn Service, uid: u32, code: u32, data: &Parcel) -> Parcel {
    owner
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}
