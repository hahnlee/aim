//! android-16.0.0_r1 Time/Battery/Storage/ConnectivityController and SyncManager.
use super::lifecycle::{attach, respond};
use super::*;

fn submit(jobs: &JobScheduler, job: JobInfo) -> i32 {
    let mut p = Parcel::new();
    js::ScheduleAsPackage {
        namespace: Some("SyncManager".into()),
        job: Some(job),
        package_name: Some("p".into()),
        user_id: 0,
        tag: Some("sync-tag".into()),
    }
    .write(&mut p);
    let response = invoke(jobs, 1000, js::SCHEDULE_AS_PACKAGE, &p);
    let mut r = response.reader();
    r.read_exception().unwrap().unwrap();
    r.read_i32().unwrap()
}

#[test]
fn periodic_flex_boundaries_and_missed_windows() {
    let h = 60 * MINUTE;
    let l = 10 * h;
    // Explicit expected windows from getRescheduleJobForPeriodic, including
    // both sides of its strict buffer and inclusive custom-flex thresholds.
    for (flex, now, earliest, latest) in [
        (h, l - h / 6 - 1, l, l + h),
        (h, l - h / 6, l, l + h),
        (h, l - h / 6 + 1, l + 1, l + h),
        (h, l, l + h / 6, l + h),
        (h, l + 1, l, l + h),
        (h, l + h, l + h, l + 2 * h),
        (h, l + 5 * h / 2, l + 2 * h, l + 3 * h),
        (15 * MINUTE, l + 2550000 - 1, l + 45 * MINUTE, l + h),
        (15 * MINUTE, l + 2550000, l + h + 45 * MINUTE, l + 2 * h),
        (15 * MINUTE, l + h, l + h + 45 * MINUTE, l + 2 * h),
    ] {
        assert_eq!(periodic_window(h, flex, l, now), (earliest, latest));
    }
    for invalid in [-1, NO_LATEST_RUNTIME] {
        assert_eq!(periodic_window(h, h, invalid, l), (l + h / 6, l + h));
    }
    assert_eq!(
        periodic_window(1, 1, l, l),
        (
            l + MIN_PERIOD_MILLIS - MIN_FLEX_MILLIS,
            l + MIN_PERIOD_MILLIS
        )
    );
    assert_eq!(
        periodic_window(i64::MAX, i64::MAX, l, l),
        (l + PERIODIC_JOB_WINDOW_BUFFER, l + MAX_ALLOWED_PERIOD_MS)
    );
}

#[test]
fn periodic_schedule_retry_and_completion_keep_the_original_window_and_extras() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let h = 60 * MINUTE;
        for flex in [h, 15 * MINUTE] {
            let mut job = sync_job();
            job.set_periodic(h, flex);
            job.constraint_flags = CONSTRAINT_STORAGE_NOT_LOW;
            for (k, v) in &mut job.extras.entries {
                match k.as_deref() {
                    Some("isPeriodic") => *v = V::Bool(true),
                    Some("periodMillis") => *v = V::Long(h),
                    Some("flexMillis") => *v = V::Long(flex),
                    _ => {}
                }
            }
            assert_eq!(submit(&jobs, job.clone()), RESULT_SUCCESS);
            let mut state = jobs.state.lock().unwrap();
            let key = JobScheduler::find(&state, 1000, Some("SyncManager"), 42).unwrap();
            let device = jobs.read_device();
            let j = state.jobs.get_mut(&key).unwrap();
            let latest = j.enqueue_time + h;
            assert_eq!(
                (j.earliest, j.latest, j.original_latest),
                (latest - flex, latest, latest)
            );
            assert!(!j.evaluate(latest - flex - 1, &device, false));
            assert!(j.evaluate(latest - flex, &device, false));
            assert!(j.evaluate(latest, &device, false));
            assert!(j.evaluate(latest + 3 * h, &device, false));
            assert!(!j.evaluate(
                latest + 3 * h,
                &Device {
                    storage_not_low: false,
                    ..device.clone()
                },
                false
            ));
            let mut failed = jobs.reschedule_for_failure(j, 3, 3, latest + 2 * h);
            assert_eq!(failed.original_latest, latest);
            assert_eq!(failed.latest, NO_LATEST_RUNTIME);
            assert_eq!(failed.num_failures, 1);
            failed.cumulative_execution = 123;
            failed.last_failed_run = 456;
            failed.tracked = true;
            state.jobs.insert(key, failed);
            // Successful completion after several missed windows must not
            // anchor the next window to the failure's backoff deadline.
            let now = latest + 5 * h / 2;
            jobs.completed(
                &mut state,
                key,
                0,
                INTERNAL_STOP_REASON_SUCCESSFUL_FINISH,
                false,
                now,
            );
            let next = state.jobs.values().find(|j| j.tracked).unwrap();
            assert_eq!(
                (next.earliest, next.latest),
                periodic_window(h, flex, latest, now)
            );
            assert_eq!(next.original_latest, next.latest);
            assert_eq!(
                (
                    next.num_failures,
                    next.num_system_stops,
                    next.cumulative_execution
                ),
                (0, 0, 0)
            );
            assert_eq!(next.last_failed_run, 456);
            assert!(next.last_successful_run > 0);
            same_extras(&next.job.extras, &job.extras);
            assert_eq!(
                (
                    next.source_uid,
                    next.source_package.as_str(),
                    next.source_tag.as_deref()
                ),
                (10100, "p", Some("sync-tag"))
            );
        }
    });
}

fn storage_threshold(host: &AppHost, low: bool) {
    use super::super::super::settings::{Change, Table};
    let settings = &host.inputs().unwrap().settings;
    // Configure the real disposable data-volume owner's threshold, without
    // filling a host disk or overriding the scheduler's constraint bits.
    for (name, value) in [
        (
            "sys_storage_threshold_percentage",
            if low { "100" } else { "0" },
        ),
        ("sys_storage_threshold_max_bytes", "9223372036854775807"),
    ] {
        settings
            .insert(
                Table::Global,
                name,
                &Change {
                    value: Some(value),
                    tag: None,
                    make_default: false,
                    package: Some("android"),
                    caller_system: true,
                    overrideable_by_restore: false,
                },
                false,
            )
            .unwrap();
    }
}

fn uplink() -> aim_uplink::Uplink {
    aim_uplink::Uplink {
        index: 4,
        addr: [192, 168, 1, 2].into(),
        prefix: 24,
        gateway: [192, 168, 1, 1].into(),
        mtu: 1500,
    }
}

#[test]
fn native_owner_constraint_loss_stops_and_restoration_readies_sync_jobs() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        install_sync_manifest(host);
        super::super::super::sync_owner::tests::install(
            host,
            export(super::super::super::test_activity_service(host)),
        );
        let owner = export(super::super::super::test_activity_service(host));
        host.attach_sync_owner(42, 100, Some(owner.binder()))
            .unwrap();
        host.set_user_unlocked_for_package_test(0, true);
        host.complete_sync_owner_replay(42, 100).unwrap();
        let battery = Arc::new(super::super::super::battery_service::BatteryService::new(
            host.clone(),
        ));
        assert!(host.battery_service.set(battery.clone()).is_ok());
        assert!(
            host.battery_stats
                .set(Arc::new(
                    super::super::super::battery_stats::BatteryStats::for_jobs_test(host)
                ))
                .is_ok()
        );
        let connectivity = Arc::new(
            super::super::super::connectivity_service::ConnectivityService::for_test(
                host.clone(),
                None,
            ),
        );
        assert!(host.connectivity_service.set(connectivity.clone()).is_ok());
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(Probe(tx)));
        for (constraint, reason, pending) in [
            (
                CONSTRAINT_CHARGING,
                6,
                PENDING_JOB_REASON_CONSTRAINT_CHARGING,
            ),
            (
                CONSTRAINT_STORAGE_NOT_LOW,
                9,
                PENDING_JOB_REASON_CONSTRAINT_STORAGE_NOT_LOW,
            ),
            (
                CONSTRAINT_CONNECTIVITY,
                7,
                PENDING_JOB_REASON_CONSTRAINT_CONNECTIVITY,
            ),
        ] {
            let jobs = scheduler(host);
            battery.report_job_power(true, 2, 100, 1);
            storage_threshold(host, false);
            connectivity.follow_network(Some((uplink(), vec![])));
            let now = elapsed_ms();
            let mut state = jobs.state.lock().unwrap();
            let callback = attach(&jobs, &mut state, Verb::Executing);
            state.running = true;
            let key = state.contexts[0].key;
            state.contexts[0].timeout_at = now + 10 * DEVICE_READ_INTERVAL;
            state.contexts[0].service = Some(host.process.strong(remote.handle));
            let j = state.jobs.get_mut(&key).unwrap();
            j.required = constraint;
            if constraint == CONSTRAINT_CONNECTIVITY {
                j.job.network = Some(job_info::restored_request(&[12, 11], &[], &[]));
            } else {
                j.job.constraint_flags = constraint;
            }
            j.process = jobs.process_of(j);
            let initial = jobs.device(&mut state, now);
            assert!(
                state
                    .jobs
                    .get_mut(&key)
                    .unwrap()
                    .evaluate(now, &initial, false)
            );
            match constraint {
                CONSTRAINT_CHARGING => battery.report_job_power(true, 4, 80, 1),
                CONSTRAINT_STORAGE_NOT_LOW => storage_threshold(host, true),
                _ => connectivity.follow_network(None),
            }
            jobs.tick(&mut state, now + DEVICE_READ_INTERVAL);
            assert_eq!(state.contexts[0].verb, Verb::Stopping);
            assert_eq!(
                (
                    state.contexts[0].stop.reason,
                    state.contexts[0].stop.internal
                ),
                (reason, 1)
            );
            assert_eq!(state.jobs[&key].pending_reasons(false), [pending]);
            state.running = false;
            drop(state);
            let (code, _, public, internal) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!((code, public, internal), (job_service::STOP_JOB, reason, 1));
            respond(&callback, cb::ACKNOWLEDGE_STOP_MESSAGE, true);
            let mut state = jobs.state.lock().unwrap();
            let key = *state.jobs.keys().next().unwrap();
            state.running = true;
            assert!(!jobs.ready_to_execute(&mut state, key, now + 2 * DEVICE_READ_INTERVAL));
            battery.report_job_power(true, 2, 100, 1);
            storage_threshold(host, false);
            connectivity.follow_network(Some((uplink(), vec![])));
            assert!(jobs.ready_to_execute(&mut state, key, now + 3 * DEVICE_READ_INTERVAL));
            same_extras(&state.jobs[&key].job.extras, &sync_job().extras);
        }
    });
}

#[test]
fn native_charging_state_distinguishes_power_and_adaptive_policy() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let battery = Arc::new(super::super::super::battery_service::BatteryService::new(
            host.clone(),
        ));
        assert!(host.battery_service.set(battery.clone()).is_ok());
        assert!(
            host.battery_stats
                .set(Arc::new(
                    super::super::super::battery_stats::BatteryStats::for_jobs_test(host)
                ))
                .is_ok()
        );
        let jobs = scheduler(host);
        let mut job = sync_job();
        job.constraint_flags = CONSTRAINT_CHARGING;
        let mut j = status(job, 1000, 10100, "p", Some("SyncManager"));
        j.process = Some("android".into());
        battery.report_job_power(true, 2, 80, 1);
        assert!(j.evaluate(0, &jobs.read_device(), false));
        for (powered, status, level, policy, ready) in [
            (true, 2, 100, 1, true),
            (true, 4, 80, 1, false),
            (true, 5, 100, 1, true),
            (false, 3, 80, 1, false),
            (true, 4, 69, 2, false),
            (true, 4, 70, 2, true),
            (true, 4, 70, 3, true),
            (true, 4, 70, 4, true),
            (true, 4, 80, i32::MIN, false),
        ] {
            battery.report_job_power(false, 3, level, 1);
            battery.report_job_power(powered, status, level, policy);
            // BatteryStats processes the plug transition before the next
            // stable plugged reading applies its charging threshold.
            battery.report_job_power(powered, status, level, policy);
            let device = jobs.read_device();
            assert_eq!(j.evaluate(0, &device, false), ready, "{device:?}");
            assert_eq!(j.evaluate(0, &device, true), powered);
        }
    });
}

struct ExpeditedProbe {
    extras: Bundle,
    started: mpsc::Sender<Binder>,
}
impl Service for ExpeditedProbe {
    fn descriptor(&self) -> &str {
        job_service::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(
            (call.sender_euid, call.code),
            (1000, job_service::START_JOB)
        );
        call.data.enforce_interface(job_service::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 1);
        assert_eq!(call.data.read_i32()?, 42);
        assert_eq!(call.data.read_string16()?.as_deref(), Some("SyncManager"));
        same_extras(&Bundle::read_from(&mut call.data)?, &self.extras);
        assert_eq!(call.data.read_i32()?, 0); // transient extras
        assert_eq!(call.data.read_i32()?, 0); // clip data
        let callback = call.data.read_binder()?.unwrap();
        assert_eq!(call.data.read_i32()?, 0); // override deadline
        assert!(call.data.read_bool()?); // expedited JobParameters
        assert!(!call.data.read_bool()?); // user initiated
        self.started.send(callback).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn expedited_sync_is_admitted_and_runs_with_preserved_extras() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let connectivity = Arc::new(
            super::super::super::connectivity_service::ConnectivityService::for_test(
                host.clone(),
                None,
            ),
        );
        assert!(host.connectivity_service.set(connectivity.clone()).is_ok());
        connectivity.follow_network(Some((uplink(), vec![])));
        storage_threshold(host, false);
        let mut expedited = sync_job();
        expedited.flags |= job_info::FLAG_EXPEDITED;
        expedited.priority = PRIORITY_MAX;
        expedited.constraint_flags = CONSTRAINT_STORAGE_NOT_LOW;
        expedited.network = Some(job_info::restored_request(&[12], &[], &[]));
        for (k, v) in &mut expedited.extras.entries {
            match (k.as_deref(), v) {
                (Some("ejDowngradedToRegular"), v) => *v = V::Bool(false),
                (Some("syncExtras"), V::Bundle(b)) => b
                    .entries
                    .push((Some("schedule_as_expedited_job".into()), V::Bool(true))),
                _ => {}
            }
        }
        assert_eq!(submit(&jobs, expedited.clone()), RESULT_SUCCESS);
        let pending = jobs.own_pending_job("SyncManager", 42).unwrap().unwrap();
        assert!(pending.is_expedited());
        same_extras(&pending.extras, &expedited.extras);
        let reopened = scheduler(host);
        let pending = reopened
            .own_pending_job("SyncManager", 42)
            .unwrap()
            .unwrap();
        assert!(pending.is_expedited());
        same_extras(&pending.extras, &expedited.extras);
        let mut state = reopened.state.lock().unwrap();
        state.running = true;
        let key = *state.jobs.keys().next().unwrap();
        assert!(!reopened.ready_to_execute(&mut state, key, elapsed_ms()));
        assert!(!reopened.execute(&mut state, key, elapsed_ms()));
        assert!(state.contexts.is_empty());
        assert_eq!(state.next_callback, 0); // No bind/callback allocated before completion.
        super::super::super::sync_owner::tests::install(
            host,
            export(super::super::super::test_activity_service(host)),
        );
        let owner = export(super::super::super::test_activity_service(host));
        host.attach_sync_owner(42, 100, Some(owner.binder()))
            .unwrap();
        host.set_user_unlocked_for_package_test(0, true);
        assert!(!reopened.ready_to_execute(&mut state, key, elapsed_ms()));
        host.complete_sync_owner_replay(42, 100).unwrap();
        assert!(reopened.ready_to_execute(&mut state, key, elapsed_ms()));
        host.sync_owner_died(100);
        assert!(!reopened.ready_to_execute(&mut state, key, elapsed_ms()));
        assert!(!reopened.execute(&mut state, key, elapsed_ms()));
        state.running = false;
        let j = state.jobs.remove(&key).unwrap();
        assert!(j.expedited());
        assert_eq!(
            (
                j.source_uid,
                j.source_package.as_str(),
                j.source_tag.as_deref()
            ),
            (10100, "p", Some("sync-tag"))
        );
        // As in the lifecycle fixture, supply the binding context; the
        // production connection/start/acknowledgement/completion path runs.
        let callback = attach(&reopened, &mut state, Verb::Binding);
        let key = state.contexts[0].key;
        state.contexts[0].expedited = j.expedited();
        state.jobs.insert(key, j);
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(ExpeditedProbe {
            extras: expedited.extras.clone(),
            started: tx,
        }));
        reopened.connected(
            &mut state,
            callback.id,
            Some(remote.binder()),
            false,
            elapsed_ms(),
        );
        assert_eq!(state.contexts[0].verb, Verb::Starting);
        drop(state);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Binder::Handle(_)
        ));
        respond(&callback, cb::ACKNOWLEDGE_START_MESSAGE, true);
        assert_eq!(
            reopened.state.lock().unwrap().contexts[0].verb,
            Verb::Executing
        );
        respond(&callback, cb::JOB_FINISHED, false);
        let state = reopened.state.lock().unwrap();
        assert!(state.contexts.is_empty());
        assert!(state.jobs.is_empty());
    });
}
