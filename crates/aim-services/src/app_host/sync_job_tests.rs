//! Android 16 SyncOperation.toJobInfoExtras and JobSchedulerService.LocalService.
#[path = "job_lifecycle_tests.rs"]
mod lifecycle;

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
            [1, 1, 1, 1, 1, 1]
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
struct Probe(mpsc::Sender<(u32, Binder, i32, i32)>);
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
        let reason = call.data.read_i32()?;
        let internal = call.data.read_i32()?;
        self.0
            .send((call.code, callback, reason, internal))
            .unwrap();
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
            let (code, callback_handle, reason, internal) =
                rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(reason, STOP_REASON_UNDEFINED);
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
                    rx.recv_timeout(Duration::from_secs(2)).unwrap().3,
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

// A disposable scan with the framework's manifest service and two adapter apps.
fn install_sync_manifest(host: &Arc<AppHost>) {
    use crate::package::{pkg, scan, settings};
    assert!(
        host.permission_manager
            .set(Arc::new(
                super::super::permission_manager::PermissionManager::new(host.clone())
            ))
            .is_ok()
    );
    let system = host.package_system().unwrap();
    let bridge = system.package_bootstrap().unwrap();
    let base = system.capture_package_queries().unwrap();
    let mut settings = base.scan().owner().settings.clone();
    let mut android = settings.packages[0].clone();
    android.name = "android".into();
    android.app_id = 1000;
    android.shared_user = true;
    android.shared_user_app_id = Some(1000);
    android.code_path = "/system/framework/framework-res.apk".into();
    android.flags |= settings::FLAG_SYSTEM;
    settings.packages.push(android);
    let mut owner = scan::SigningScan::new(&Default::default(), &settings, 36).unwrap();
    let names = ["p", "q", "android"];
    owner
        .capture_user_states(
            names
                .map(|name| {
                    (
                        (name.into(), false),
                        scan::CapturedUsers {
                            states: BTreeMap::from([(0, Default::default())]),
                            active_aliases: Default::default(),
                        },
                    )
                })
                .into(),
        )
        .unwrap();
    owner
        .capture_legacy_permissions(
            &[0],
            names
                .map(|n| ((n.into(), false), Default::default()))
                .into(),
            owner
                .identities
                .shared_users
                .keys()
                .map(|n| (n.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    owner
        .capture_install_permissions_fixed(names.map(|n| ((n.into(), false), false)).into())
        .unwrap();
    for package in base
        .state()
        .packages
        .values()
        .filter_map(|p| p.pkg.as_deref())
    {
        owner.load_test_code(package.clone());
    }
    let mut service = pkg::Service::default();
    service.main.component.name = sync_job().service.1;
    service.main.component.package_name = "android".into();
    service.main.process_name = Some("system".into());
    service.main.enabled = true;
    service.permission = Some("android.permission.BIND_JOB_SERVICE".into());
    owner.load_test_code(pkg::AndroidPackage {
        package_name: "android".into(),
        shared_user_id: Some("android.uid.system".into()),
        uid: 1000,
        path: Some("/system/framework/framework-res.apk".into()),
        booleans: pkg::booleans::SYSTEM | pkg::booleans::ENABLED,
        services: vec![service],
        feature_flag_state: Some(vec![]),
        ..Default::default()
    });
    owner
        .assign_seinfo_at_boot(
            &crate::package::owner::seinfo::Policy::parse(&[]).unwrap(),
            &mut |_: &pkg::AndroidPackage| Ok(36),
        )
        .unwrap();
    owner
        .complete_library_dependencies(&|_, _| {
            Ok(crate::package::libraries::Policy {
                enforce_native_dependencies: false,
                sdk_library_independence: false,
            })
        })
        .unwrap();
    owner
        .rebuild_shared_processes_from_native_members()
        .unwrap();
    owner
        .capture_replica_runtime(
            names
                .map(|name| {
                    let labels = owner.seinfo_state(name).unwrap().unwrap();
                    (
                        (name.into(), false),
                        scan::ReplicaRuntime {
                            usage: [0; 8],
                            seinfo: labels.base.clone(),
                            override_seinfo: labels.override_label.clone(),
                            library_files: vec![],
                            libraries: vec![],
                        },
                    )
                })
                .into(),
        )
        .unwrap();
    let mut context = (**base.context()).clone();
    let mut android = context.packages[&("p".into(), false)].clone();
    android.app_id = 1000;
    android.path = "/system/framework/framework-res.apk".into();
    context.packages.insert(("android".into(), false), android);
    context.scan_version = base.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &bridge,
            Some(base.scan()),
            owner,
            crate::package::owner::usage::Usage::new(names),
            context,
        )
        .unwrap();
}
fn schedule_sync(jobs: &JobScheduler, ns: Option<&str>, proxy: Option<(&str, i32, &str)>) {
    let mut p = Parcel::new();
    let code = if let Some((package, user, tag)) = proxy {
        js::ScheduleAsPackage {
            namespace: ns.map(Into::into),
            job: Some(sync_job()),
            package_name: Some(package.into()),
            user_id: user,
            tag: Some(tag.into()),
        }
        .write(&mut p);
        js::SCHEDULE_AS_PACKAGE
    } else {
        js::Schedule {
            namespace: ns.map(Into::into),
            job: Some(sync_job()),
        }
        .write(&mut p);
        js::SCHEDULE
    };
    let reply = invoke(jobs, 1000, code, &p);
    let mut r = reply.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), RESULT_SUCCESS);
}
fn cancel_sync(jobs: &JobScheduler, ns: Option<&str>) {
    let mut p = Parcel::new();
    js::Cancel {
        namespace: ns.map(Into::into),
        job_id: 42,
    }
    .write(&mut p);
    invoke(jobs, 1000, js::CANCEL, &p)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
}
fn assert_source(
    jobs: &JobScheduler,
    ns: Option<&str>,
    package: &str,
    uid: i32,
    tag: Option<&str>,
) {
    let mut state = jobs.state.lock().unwrap();
    jobs.ensure_loaded(&mut state).unwrap();
    let key = JobScheduler::find(&state, 1000, ns, 42).unwrap();
    let j = &state.jobs[&key];
    assert_eq!(
        (
            j.source_package.as_str(),
            j.source_uid,
            j.source_tag.as_deref()
        ),
        (package, uid, tag)
    );
    same_extras(&j.job.extras, &sync_job().extras);
}
// The retained SyncManager owns these two marker values. Reproduce its
// reverse-order reads/schedule/cancel sequence; do not add a second native owner.
fn migrate(jobs: &JobScheduler, markers: &mut (bool, bool)) {
    if markers.0 && markers.1 {
        return;
    }
    if !markers.0 {
        let pending = jobs.jobs_of(1000).unwrap();
        let mut complete = true;
        for (ns, job) in pending.into_iter().rev() {
            if ns.is_none()
                && job
                    .extras
                    .entries
                    .iter()
                    .any(|(k, v)| k.as_deref() == Some("SyncManagerJob") && *v == V::Bool(true))
            {
                schedule_sync(jobs, Some("SyncManager"), Some(("p", 0, "sync-tag")));
                cancel_sync(jobs, None);
                complete = false;
            }
        }
        markers.0 = complete;
    }
    let own = jobs.system_scheduled_own_jobs(Some("SyncManager")).unwrap();
    let mut complete = true;
    for _ in own.into_iter().rev() {
        schedule_sync(jobs, Some("SyncManager"), Some(("p", 0, "sync-tag")));
        complete = false;
    }
    markers.1 = complete;
}
#[test]
fn sync_namespace_migration_and_proxy_replacement_are_isolated_and_repeatable() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        schedule_sync(&jobs, None, None);
        schedule_sync(&jobs, Some("other"), None);
        schedule_sync(&jobs, Some("SyncManager"), None);
        // A default non-sync job must survive the migration too.
        let mut other = sync_job();
        other.id = 7;
        other.extras = Default::default();
        jobs.schedule(1000, None, Some(Ok(other)), None, None)
            .unwrap();
        let mut markers = (false, false);
        migrate(&jobs, &mut markers);
        assert_eq!(markers, (false, true));
        assert_eq!(jobs.jobs_of(1000).unwrap().len(), 3);
        assert_source(&jobs, Some("SyncManager"), "p", 10100, Some("sync-tag"));
        assert_source(&jobs, Some("other"), "android", 1000, None);
        migrate(&jobs, &mut markers);
        assert_eq!(markers, (true, true));
        let reopened = scheduler(host);
        assert_source(&reopened, Some("SyncManager"), "p", 10100, Some("sync-tag"));
        schedule_sync(&reopened, Some("SyncManager"), Some(("q", 0, "other-tag")));
        assert_source(
            &reopened,
            Some("SyncManager"),
            "q",
            10101,
            Some("other-tag"),
        );
        schedule_sync(&reopened, Some("SyncManager"), None);
        assert_source(&reopened, Some("SyncManager"), "android", 1000, None);
        let mut markers = (true, false);
        migrate(&reopened, &mut markers);
        assert_eq!(markers, (true, false));
        migrate(&reopened, &mut markers);
        assert_eq!(markers, (true, true));
        cancel_sync(&reopened, Some("SyncManager"));
        assert_eq!(reopened.jobs_of(1000).unwrap().len(), 2);
        assert_eq!(scheduler(host).jobs_of(1000).unwrap().len(), 2);
    });
}
#[test]
fn sync_migration_restarts_between_schedule_cancel_and_marker_writes() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for phase in 0..5 {
            let jobs = scheduler(host);
            schedule_sync(&jobs, None, None);
            jobs.jobs_of(1000).unwrap(); // Initial disk state.
            schedule_sync(&jobs, Some("SyncManager"), Some(("p", 0, "sync-tag")));
            if phase >= 1 {
                jobs.jobs_of(1000).unwrap();
            } // schedule durable
            cancel_sync(&jobs, None);
            if phase >= 2 {
                jobs.jobs_of(1000).unwrap();
            } // cancel durable
            let mut markers = (false, false);
            if phase >= 3 {
                migrate(&jobs, &mut markers);
            }
            if phase == 4 {
                assert_eq!(markers, (true, true));
            } else {
                markers = (false, false);
            } // status write not yet durable
            // No worker runs in this harness: dropping the owner loses pending writes.
            drop(jobs);
            let reopened = scheduler(host);
            migrate(&reopened, &mut markers);
            migrate(&reopened, &mut markers);
            assert_eq!(markers, (true, true));
            let reopened = scheduler(host);
            assert_source(&reopened, Some("SyncManager"), "p", 10100, Some("sync-tag"));
            assert_eq!(reopened.jobs_of(1000).unwrap().len(), 1);
            cancel_sync(&reopened, Some("SyncManager"));
            reopened.jobs_of(1000).unwrap();
        }
    });
}
#[test]
fn failed_job_commit_prevents_migration_confirmation_and_retries() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        schedule_sync(&jobs, Some("SyncManager"), None);
        jobs.jobs_of(1000).unwrap();
        schedule_sync(&jobs, Some("SyncManager"), Some(("p", 0, "sync-tag")));
        let blocker = host
            .inputs()
            .unwrap()
            .data
            .join("system/job/jobs_1000.xml.new");
        std::fs::create_dir(&blocker).unwrap();
        assert!(jobs.system_scheduled_own_jobs(Some("SyncManager")).is_err());
        assert!(jobs.jobs_of(1000).is_err());
        assert!(jobs.state.lock().unwrap().dirty.contains(&1000));
        std::fs::remove_dir(blocker).unwrap();
        assert!(
            jobs.system_scheduled_own_jobs(Some("SyncManager"))
                .unwrap()
                .is_empty()
        );
        assert_source(
            &scheduler(host),
            Some("SyncManager"),
            "p",
            10100,
            Some("sync-tag"),
        );
        cancel_sync(&jobs, Some("SyncManager"));
        let blocker = host
            .inputs()
            .unwrap()
            .data
            .join("system/job/jobs_1000.xml.new");
        std::fs::create_dir(&blocker).unwrap();
        assert!(jobs.jobs_of(1000).is_err());
        assert_source(
            &scheduler(host),
            Some("SyncManager"),
            "p",
            10100,
            Some("sync-tag"),
        );
        std::fs::remove_dir(blocker).unwrap();
        assert!(jobs.jobs_of(1000).unwrap().is_empty());
        assert!(scheduler(host).jobs_of(1000).unwrap().is_empty());
    });
}

#[test]
fn sync_attribution_repair_survives_restart_before_and_after_confirmation() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for phase in 0..3 {
            let jobs = scheduler(host);
            schedule_sync(&jobs, Some("SyncManager"), None);
            jobs.jobs_of(1000).unwrap();
            let mut markers = (true, false);
            migrate(&jobs, &mut markers);
            assert_eq!(markers, (true, false));
            if phase >= 1 {
                migrate(&jobs, &mut markers);
            }
            if phase < 2 {
                markers.1 = false;
            } // uncommitted status marker
            drop(jobs);
            let reopened = scheduler(host);
            migrate(&reopened, &mut markers);
            migrate(&reopened, &mut markers);
            assert_eq!(markers, (true, true));
            assert_source(
                &scheduler(host),
                Some("SyncManager"),
                "p",
                10100,
                // JobStore repairs an uncommitted old job's package from extras,
                // without inventing a tag; committed scheduleAsPackage keeps it.
                (phase != 0).then_some("sync-tag"),
            );
            cancel_sync(&reopened, Some("SyncManager"));
            reopened.jobs_of(1000).unwrap();
        }
    });
}
#[test]
fn proxy_attribution_uses_installed_source_and_target_user_quota() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        for (package, user) in [("missing", 0), ("p", 10), ("android", 10)] {
            schedule_sync(
                &jobs,
                Some("SyncManager"),
                Some((package, user, "discarded")),
            );
            assert_source(&jobs, Some("SyncManager"), "android", 1000, None);
        }
        assert!(
            jobs.state
                .lock()
                .unwrap()
                .schedule_calls
                .contains_key(&(10, "android".into()))
        );
        schedule_sync(&jobs, Some("SyncManager"), Some(("p", 0, "kept")));
        jobs.jobs_of(1000).unwrap();
        assert_source(
            &scheduler(host),
            Some("SyncManager"),
            "p",
            10100,
            Some("kept"),
        );
        // The proxy API still enforces the real Binder caller's privilege.
        let mut p = Parcel::new();
        js::ScheduleAsPackage {
            namespace: Some("SyncManager".into()),
            job: Some(sync_job()),
            package_name: Some("p".into()),
            user_id: 0,
            tag: None,
        }
        .write(&mut p);
        assert!(
            invoke(jobs.as_ref(), 10100, js::SCHEDULE_AS_PACKAGE, &p)
                .reader()
                .read_exception()
                .unwrap()
                .is_err()
        );
    });
}
