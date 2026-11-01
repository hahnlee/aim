//! Android 16 SyncOperation.toJobInfoExtras and JobSchedulerService.LocalService.
use super::*;
use crate::package::restrictions::persistable::{Bundle, Double, Value as V};
use aim_service_aidl::{ReadParcelable, WriteParcelable, dev_aim_server_isystemprocess as sp};
use std::sync::mpsc;

fn same_extras(a: &Bundle, b: &Bundle) {
    fn sorted(mut b: Bundle) -> Bundle {
        for (_, value) in &mut b.entries {
            if let V::Bundle(child) = value {
                *child = sorted(child.clone());
            }
        }
        b.entries.sort_by(|a, b| a.0.cmp(&b.0));
        b
    }
    assert_eq!(sorted(a.clone()), sorted(b.clone()));
}
fn scheduler(host: &Arc<AppHost>) -> Arc<JobScheduler> {
    Arc::new_cyclic(|me| JobScheduler {
        me: me.clone(),
        host: host.clone(),
        state: Mutex::new(State::default()),
        queue: Mutex::new(Vec::new()),
        wake: Condvar::new(),
        limit: 1,
    })
}
fn sync_job() -> JobInfo {
    let mut job = JobInfo::persisted_builder(
        42,
        "android".into(),
        "com.android.server.content.SyncJobService".into(),
    );
    let fields = [
        ("SyncManagerJob", V::Bool(true)),
        ("provider", V::String("authority".into())),
        ("accountName", V::String("name".into())),
        ("accountType", V::String("type".into())),
        ("userId", V::Int(0)),
        ("owningUid", V::Int(10100)),
        ("owningPackage", V::String("p".into())),
        ("reason", V::Int(-4)),
        ("source", V::Int(3)),
        ("allowParallelSyncs", V::Bool(true)),
        ("jobId", V::Int(42)),
        ("isPeriodic", V::Bool(false)),
        ("sourcePeriodicId", V::Int(-1)),
        ("periodMillis", V::Long(0)),
        ("flexMillis", V::Long(0)),
        ("expectedRuntime", V::Long(1234567890123)),
        ("retries", V::Int(2)),
        ("syncExemptionFlag", V::Int(1)),
        ("ejDowngradedToRegular", V::Bool(true)),
        (
            "syncExtras",
            V::Bundle(Bundle {
                entries: vec![
                    (Some("upload".into()), V::Bool(true)),
                    (Some("long".into()), V::Long(i64::MAX)),
                    (Some("int".into()), V::Int(i32::MIN)),
                    (Some("double".into()), V::Double(Double::new(-0.0))),
                    (Some("float".into()), V::Double(Double::new(0.1_f32 as f64))),
                    (Some("text".into()), V::String("한글<&".into())),
                    (Some("null".into()), V::Null),
                    (Some("ACCOUNT:extra".into()), V::String("collision".into())),
                ],
            }),
        ),
        (
            "ACCOUNT:extra",
            V::Bundle(Bundle {
                entries: vec![
                    (Some("accountName".into()), V::String("other".into())),
                    (Some("accountType".into()), V::String("other-type".into())),
                ],
            }),
        ),
    ];
    job.extras.entries = fields
        .into_iter()
        .map(|(k, v)| (Some(k.into()), v))
        .collect();
    job
}
fn status(job: JobInfo, uid: i32, source: i32, package: &str, ns: Option<&str>) -> JobStatus {
    let mut j = JobStatus::new(
        job,
        uid,
        Source {
            uid: source,
            user: 0,
            package: package.into(),
            tag: Some("sync-tag".into()),
        },
        ns.map(Into::into),
        NO_EARLIEST_RUNTIME,
        NO_LATEST_RUNTIME,
        Arc::new(Vec::new()),
    );
    j.tracked = true;
    j
}
fn invoke(owner: &impl Service, uid: u32, code: u32, args: &Parcel) -> Parcel {
    owner
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 1,
            sender_euid: uid,
            data: args.reader(),
        })
        .unwrap()
}
#[test]
fn sync_extras_survive_job_parcel_atomic_store_and_reopen() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let job = sync_job();
        let mut p = Parcel::new();
        job.write_to(&mut p);
        let decoded = JobInfo::read(&mut p.reader()).unwrap().ok().unwrap();
        same_extras(&decoded.extras, &job.extras);
        let stored = status(decoded, 1000, 10100, "p", Some("SyncManager")).stored();
        let dir = host.inputs().unwrap().data.join("sync-job-roundtrip");
        job_store::write(&dir, 1000, &[&stored], 1000, 2000).unwrap();
        let (back, legacy) = job_store::read(&dir, 3000, 4000);
        assert!(!legacy);
        assert_eq!(back.len(), 1);
        let b = &back[0];
        same_extras(&b.job.extras, &job.extras);
        assert_eq!(
            (
                b.uid,
                b.source_user,
                b.namespace.as_deref(),
                b.source_package.as_deref(),
                b.source_tag.as_deref()
            ),
            (1000, 0, Some("SyncManager"), Some("p"), Some("sync-tag"))
        );
        assert!(b.job.persisted && !b.job.periodic);
    });
}
#[test]
fn local_jobs_select_source_and_namespace_and_expose_real_load_counts() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let jobs = scheduler(host);
        let stored = status(sync_job(), 1000, 10100, "p", Some("SyncManager")).stored();
        job_store::write(
            &host.inputs().unwrap().data.join("system/job"),
            1000,
            &[&stored],
            elapsed_ms(),
            wall_ms(),
        )
        .unwrap();
        assert_eq!(jobs.persist_stats().unwrap(), [1, 1, 1, -1, -1, -1]);
        assert!(
            jobs.system_scheduled_own_jobs(Some("SyncManager"))
                .unwrap()
                .is_empty()
        );
        let mut state = jobs.state.lock().unwrap();
        for (key, uid, source, package, ns) in [
            (10, 1000, 1000, "android", Some("SyncManager")),
            (11, 1000, 1000, "android", None),
            (12, 1000, 10100, "p", Some("SyncManager")),
            (13, 1000, 1000, "q", Some("SyncManager")),
            (14, 10100, 1000, "android", Some("SyncManager")),
        ] {
            let mut j = sync_job();
            j.id = key as i32;
            state.jobs.insert(key, status(j, uid, source, package, ns));
        }
        drop(state);
        assert!(host.job_scheduler.set(jobs.clone()).is_ok());
        let owner = super::super::system_process::SystemProcess::new(host.clone());
        let mut p = Parcel::new();
        sp::GetSystemScheduledOwnJobs {
            namespace: Some("SyncManager".into()),
        }
        .write(&mut p);
        // Decode the typed JobInfo list without a test-only adapter.
        let result = invoke(&owner, 1000, sp::GET_SYSTEM_SCHEDULED_OWN_JOBS, &p);
        let mut r = result.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(JobInfo::read(&mut r).unwrap().ok().unwrap().id, 10);
        assert_eq!(jobs.system_scheduled_own_jobs(None).unwrap()[0].id, 11);
        assert!(
            invoke(&owner, 10100, sp::GET_SYSTEM_SCHEDULED_OWN_JOBS, &p)
                .reader()
                .read_exception()
                .unwrap()
                .is_err()
        );
        let mut p = Parcel::new();
        sp::GetJobPersistStats {}.write(&mut p);
        assert_eq!(
            sp::read_get_job_persist_stats_reply(
                &mut invoke(&owner, 1000, sp::GET_JOB_PERSIST_STATS, &p).reader()
            )
            .unwrap()
            .unwrap()
            .unwrap(),
            [1, 1, 1, -1, -1, -1]
        );
        assert!(
            sp::read_get_job_persist_stats_reply(
                &mut invoke(&owner, 2000, sp::GET_JOB_PERSIST_STATS, &p).reader()
            )
            .unwrap()
            .is_err()
        );
        {
            let mut state = jobs.state.lock().unwrap();
            state.dirty.extend([1000, 10100]);
            state.write_at = Some(elapsed_ms());
        }
        let weak = Arc::downgrade(&jobs);
        std::thread::spawn(move || run(weak));
        jobs.post(Message::Persist);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while jobs.persist_stats().unwrap()[3..] != [6, 5, 5] {
            assert!(
                std::time::Instant::now() < deadline,
                "save counts did not publish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(persisted_counts([&stored].into_iter()), [1, 1, 1]);
    });
}
struct Probe(mpsc::Sender<(u32, Binder, i32)>);
impl Service for Probe {
    fn descriptor(&self) -> &str {
        job_service::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(job_service::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 1);
        assert_eq!(call.data.read_i32()?, 42);
        assert_eq!(call.data.read_string16()?.as_deref(), Some("SyncManager"));
        same_extras(&Bundle::read_from(&mut call.data)?, &sync_job().extras);
        assert_eq!(call.data.read_i32()?, 0);
        assert_eq!(call.data.read_i32()?, 0);
        let callback = call.data.read_binder()?.unwrap();
        for _ in 0..3 {
            call.data.read_i32()?;
        }
        assert_eq!(call.data.read_i32()?, -1);
        assert_eq!(call.data.read_i32()?, -1);
        assert_eq!(call.data.read_i32()?, 0);
        call.data.read_i32()?;
        let internal = call.data.read_i32()?;
        self.0.send((call.code, callback, internal)).unwrap();
        Ok(Parcel::new())
    }
}
#[test]
fn sync_job_start_finish_stop_preserve_system_binder_identity() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let jobs = scheduler(host);
        jobs.state.lock().unwrap().loaded = true;
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(Probe(tx)));
        for finish in [true, false] {
            let id = if finish { 1 } else { 2 };
            let callback = Arc::new(JobCallback {
                scheduler: Arc::downgrade(&jobs),
                id,
                stopped: Mutex::new(None),
            });
            let binder = host.process.add_service(callback.clone());
            let mut state = jobs.state.lock().unwrap();
            state.jobs.insert(
                id,
                status(sync_job(), 1000, 10100, "p", Some("SyncManager")),
            );
            state.contexts.push(Context {
                key: id,
                callback: callback.clone(),
                callback_binder: binder,
                conn: Arc::new(Connection {
                    scheduler: Arc::downgrade(&jobs),
                    callback: id,
                }),
                verb: Verb::Binding,
                service: None,
                cancelled: false,
                stop: StopReason {
                    reason: STOP_REASON_UNDEFINED,
                    internal: INTERNAL_STOP_REASON_UNKNOWN,
                    debug: None,
                },
                override_deadline_expired: false,
                expedited: false,
                execution_start: 1000,
                last_stamp: 1000,
                min_guarantee: 1000,
                max_execution: 10000,
                timeout_at: 2000,
                pending_stop: None,
                network: None,
            });
            jobs.connected(&mut state, id, Some(remote.binder()), false, 1000);
            drop(state);
            let (code, callback_handle, internal) =
                rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!((code, internal), (job_service::START_JOB, -1));
            assert!(matches!(callback_handle, Binder::Handle(_)));
            let mut p = Parcel::new();
            cb::AcknowledgeStartMessage {
                job_id: 42,
                ongoing: true,
            }
            .write(&mut p);
            invoke(callback.as_ref(), 1000, cb::ACKNOWLEDGE_START_MESSAGE, &p);
            if finish {
                let mut p = Parcel::new();
                cb::JobFinished {
                    job_id: 42,
                    reschedule: false,
                }
                .write(&mut p);
                invoke(callback.as_ref(), 1000, cb::JOB_FINISHED, &p);
            } else {
                jobs.cancel_impl(
                    &mut jobs.state.lock().unwrap(),
                    id,
                    None,
                    STOP_REASON_CANCELLED_BY_APP,
                    INTERNAL_STOP_REASON_CANCELED,
                    "cancel",
                    1100,
                );
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(2)).unwrap().2,
                    INTERNAL_STOP_REASON_CANCELED
                );
                let mut p = Parcel::new();
                cb::AcknowledgeStopMessage {
                    job_id: 42,
                    reschedule: false,
                }
                .write(&mut p);
                invoke(callback.as_ref(), 1000, cb::ACKNOWLEDGE_STOP_MESSAGE, &p);
            }
            assert!(jobs.state.lock().unwrap().contexts.is_empty());
            assert!(jobs.state.lock().unwrap().jobs.is_empty());
        }
    });
}
