//! android-16.0.0_r1 JobServiceContext and JobSchedulerService retry contracts.
use super::*;

fn attach(jobs: &Arc<JobScheduler>, state: &mut State, verb: Verb) -> Arc<JobCallback> {
    state.loaded = true;
    state.next_key += 1;
    state.next_callback += 1;
    let key = state.next_key;
    let callback = Arc::new(JobCallback {
        scheduler: Arc::downgrade(jobs),
        id: state.next_callback,
        stopped: Mutex::new(None),
    });
    state.jobs.insert(
        key,
        status(sync_job(), 1000, 10100, "p", Some("SyncManager")),
    );
    state.contexts.push(Context {
        key,
        callback: callback.clone(),
        callback_binder: jobs.host.process.add_service(callback.clone()),
        conn: Arc::new(Connection {
            scheduler: Arc::downgrade(jobs),
            callback: callback.id,
        }),
        verb,
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
        timeout_at: 9000,
        pending_stop: None,
        network: None,
    });
    callback
}
fn respond(callback: &JobCallback, code: u32, reschedule: bool) {
    let mut p = Parcel::new();
    match code {
        cb::ACKNOWLEDGE_START_MESSAGE => cb::AcknowledgeStartMessage {
            job_id: 42,
            ongoing: reschedule,
        }
        .write(&mut p),
        cb::ACKNOWLEDGE_STOP_MESSAGE => cb::AcknowledgeStopMessage {
            job_id: 42,
            reschedule,
        }
        .write(&mut p),
        cb::JOB_FINISHED => cb::JobFinished {
            job_id: 42,
            reschedule,
        }
        .write(&mut p),
        _ => unreachable!(),
    }
    invoke(callback, 1000, code, &p)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
}
fn retry(state: &State, failures: i32, system_stops: i32, earliest: Option<i64>) -> &JobStatus {
    assert!(state.contexts.is_empty());
    assert_eq!(state.jobs.len(), 1);
    let j = state.jobs.values().next().unwrap();
    assert!(j.tracked);
    assert_eq!(
        (j.num_failures, j.num_system_stops),
        (failures, system_stops)
    );
    if let Some(earliest) = earliest {
        assert_eq!(j.earliest, earliest);
    }
    assert_eq!(j.latest, NO_LATEST_RUNTIME);
    assert_eq!(
        (
            j.calling_uid,
            j.source_uid,
            j.namespace.as_deref(),
            j.source_package.as_str(),
            j.source_tag.as_deref()
        ),
        (1000, 10100, Some("SyncManager"), "p", Some("sync-tag"))
    );
    same_extras(&j.job.extras, &sync_job().extras);
    j
}
fn constraint_stop() -> StopReason {
    StopReason {
        reason: STOP_REASON_CONSTRAINT_CHARGING,
        internal: INTERNAL_STOP_REASON_CONSTRAINTS_NOT_SATISFIED,
        debug: Some("charging lost".into()),
    }
}

#[test]
fn cancel_and_timeout_send_distinct_internal_reasons() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(Probe(tx)));
        for cancel in [true, false] {
            let mut state = jobs.state.lock().unwrap();
            let callback = attach(&jobs, &mut state, Verb::Executing);
            state.contexts[0].service = Some(host.process.strong(remote.handle));
            if cancel {
                let key = state.contexts[0].key;
                jobs.cancel_impl(
                    &mut state,
                    key,
                    None,
                    STOP_REASON_CANCELLED_BY_APP,
                    INTERNAL_STOP_REASON_CANCELED,
                    "cancel",
                    2000,
                );
            } else {
                jobs.op_timeout(&mut state, 0, 11000);
            }
            assert_eq!(state.contexts[0].verb, Verb::Stopping);
            let timeout_at = state.contexts[0].timeout_at;
            jobs.cancel_context(&mut state, 0, constraint_stop());
            assert_eq!(state.contexts[0].timeout_at, timeout_at);
            drop(state);
            let (code, _, public, internal) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(code, job_service::STOP_JOB);
            assert_eq!((public, internal), if cancel { (1, 0) } else { (3, 3) });
            // The retained SyncJobService returns false: SyncManager owns its retry.
            respond(&callback, cb::ACKNOWLEDGE_STOP_MESSAGE, false);
            respond(&callback, cb::ACKNOWLEDGE_STOP_MESSAGE, true);
            assert!(jobs.state.lock().unwrap().jobs.is_empty());
            assert!(rx.try_recv().is_err());
        }
    });
}

#[test]
fn stops_during_binding_and_start_wait_for_the_original_boundary() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(Probe(tx)));
        for binding in [true, false] {
            for cancel in [true, false] {
                for ongoing in [true, false] {
                    let mut state = jobs.state.lock().unwrap();
                    let callback = attach(&jobs, &mut state, Verb::Binding);
                    if !binding {
                        jobs.connected(&mut state, callback.id, Some(remote.binder()), false, 1000);
                        assert_eq!(
                            rx.recv_timeout(Duration::from_secs(2)).unwrap().0,
                            job_service::START_JOB
                        );
                    }
                    let timeout_at = state.contexts[0].timeout_at;
                    if cancel {
                        let key = state.contexts[0].key;
                        jobs.cancel_impl(&mut state, key, None, 1, 0, "cancel", 2000);
                    } else {
                        jobs.cancel_context(&mut state, 0, constraint_stop());
                    }
                    assert!(state.contexts[0].cancelled);
                    assert_eq!(state.contexts[0].timeout_at, timeout_at);
                    if binding {
                        jobs.connected(&mut state, callback.id, Some(remote.binder()), false, 3000);
                    }
                    drop(state);
                    if !binding {
                        respond(&callback, cb::ACKNOWLEDGE_START_MESSAGE, ongoing);
                        if ongoing {
                            let (code, _, _, internal) =
                                rx.recv_timeout(Duration::from_secs(2)).unwrap();
                            assert_eq!(
                                (code, internal),
                                (job_service::STOP_JOB, if cancel { 0 } else { 1 })
                            );
                            respond(&callback, cb::ACKNOWLEDGE_STOP_MESSAGE, true);
                        }
                    }
                    let mut state = jobs.state.lock().unwrap();
                    assert!(state.contexts.is_empty());
                    if !cancel && (binding || ongoing) {
                        retry(&state, 0, 1, Some(0));
                    } else {
                        assert!(state.jobs.is_empty());
                    }
                    jobs.connected(&mut state, callback.id, Some(remote.binder()), false, 4000);
                    state.jobs.clear();
                    assert!(rx.try_recv().is_err());
                }
            }
        }
    });
}

#[test]
fn soft_failure_callback_orders_and_duplicates_do_not_repeat_a_retry() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for finish_first in [true, false] {
            let jobs = scheduler(host);
            let callback = attach(&jobs, &mut jobs.state.lock().unwrap(), Verb::Starting);
            // SyncJobService can call jobFinished(true) inside onStartJob when not ready.
            let codes = if finish_first {
                [cb::JOB_FINISHED, cb::ACKNOWLEDGE_START_MESSAGE]
            } else {
                [cb::ACKNOWLEDGE_START_MESSAGE, cb::JOB_FINISHED]
            };
            let before = elapsed_ms();
            respond(&callback, codes[0], true);
            assert_eq!(jobs.state.lock().unwrap().contexts[0].verb, Verb::Executing);
            respond(&callback, codes[1], true);
            let after = elapsed_ms();
            let mut state = jobs.state.lock().unwrap();
            let j = retry(&state, 1, 0, None);
            assert!((before + 30000..=after + 30000).contains(&j.earliest));
            let key = *state.jobs.keys().next().unwrap();
            let earliest = j.earliest;
            drop(state);
            for code in [
                cb::JOB_FINISHED,
                cb::ACKNOWLEDGE_START_MESSAGE,
                cb::ACKNOWLEDGE_STOP_MESSAGE,
            ] {
                respond(&callback, code, true);
            }
            state = jobs.state.lock().unwrap();
            assert_eq!(*state.jobs.keys().next().unwrap(), key);
            retry(&state, 1, 0, Some(earliest));
        }
        // Pinned doCallbackLocked routes even a duplicate current start ACK by verb.
        let jobs = scheduler(host);
        let old = attach(&jobs, &mut jobs.state.lock().unwrap(), Verb::Starting);
        respond(&old, cb::ACKNOWLEDGE_START_MESSAGE, true);
        respond(&old, cb::ACKNOWLEDGE_START_MESSAGE, false);
        assert!(jobs.state.lock().unwrap().jobs.is_empty());
        let current = attach(&jobs, &mut jobs.state.lock().unwrap(), Verb::Starting);
        for code in [
            cb::JOB_FINISHED,
            cb::ACKNOWLEDGE_START_MESSAGE,
            cb::ACKNOWLEDGE_STOP_MESSAGE,
        ] {
            respond(&old, code, true);
        }
        let mut state = jobs.state.lock().unwrap();
        jobs.connected(&mut state, old.id, None, true, 5000);
        assert_eq!(state.contexts[0].callback.id, current.id);
        assert_eq!(state.contexts[0].verb, Verb::Starting);
        drop(state);
        respond(&current, cb::ACKNOWLEDGE_START_MESSAGE, false);
        assert!(jobs.state.lock().unwrap().jobs.is_empty());
    });
}

#[test]
fn finish_during_stop_and_cancelled_replacement_keep_callback_identity() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let mut state = jobs.state.lock().unwrap();
        let old = attach(&jobs, &mut state, Verb::Stopping);
        state.contexts[0].stop = constraint_stop();
        drop(state);
        respond(&old, cb::JOB_FINISHED, true);
        respond(&old, cb::ACKNOWLEDGE_STOP_MESSAGE, true);
        let mut state = jobs.state.lock().unwrap();
        retry(&state, 1, 0, None); // jobFinished replaces the stop reason with success.
        state.jobs.clear();
        let old = attach(&jobs, &mut state, Verb::Executing);
        let (tx, rx) = mpsc::channel();
        let remote = export(Arc::new(Probe(tx)));
        state.contexts[0].service = Some(host.process.strong(remote.handle));
        let key = state.contexts[0].key;
        jobs.cancel_impl(&mut state, key, None, 1, 0, "replacement", 2000);
        let current = attach(&jobs, &mut state, Verb::Starting);
        drop(state);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().3, 0);
        respond(&old, cb::JOB_FINISHED, true);
        respond(&old, cb::ACKNOWLEDGE_STOP_MESSAGE, true);
        let state = jobs.state.lock().unwrap();
        assert_eq!(state.contexts.len(), 1);
        assert_eq!(state.contexts[0].callback.id, current.id);
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs.values().next().unwrap().num_failures, 0);
        drop(state);
        respond(&current, cb::ACKNOWLEDGE_START_MESSAGE, false);
        assert!(jobs.state.lock().unwrap().jobs.is_empty());
        assert!(rx.try_recv().is_err());
    });
}

#[test]
fn op_timeouts_and_disconnects_preserve_retry_policy_and_work() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for verb in [
            Verb::Binding,
            Verb::Starting,
            Verb::Executing,
            Verb::Stopping,
        ] {
            for disconnect in [true, false] {
                let jobs = scheduler(host);
                let mut state = jobs.state.lock().unwrap();
                let callback = attach(&jobs, &mut state, verb);
                if verb == Verb::Stopping {
                    state.contexts[0].stop = constraint_stop();
                }
                let key = state.contexts[0].key;
                let j = state.jobs.get_mut(&key).unwrap();
                j.enqueue_work(JobWorkItem::persisted(Default::default(), 0, -1, -1, -1));
                assert_eq!(j.dequeue_work().unwrap().delivery_count, 1);
                j.enqueue_work(JobWorkItem::persisted(Default::default(), 0, -1, -1, -1));
                if disconnect {
                    state.contexts[0].conn.connected(None, true);
                    let messages = std::mem::take(&mut *jobs.queue.lock().unwrap());
                    assert_eq!(messages.len(), 1);
                    for message in messages {
                        jobs.handle(&mut state, message, 4000);
                    }
                } else {
                    jobs.op_timeout(&mut state, 0, 11000);
                }
                if !disconnect && verb == Verb::Starting {
                    assert!(state.jobs.is_empty());
                } else {
                    // An executing timeout's stop send has no live endpoint here: close now.
                    let failure = !disconnect && verb != Verb::Stopping;
                    let j = retry(
                        &state,
                        i32::from(failure),
                        i32::from(!failure),
                        Some(if failure { 41000 } else { 0 }),
                    );
                    assert_eq!(
                        j.cumulative_execution,
                        if disconnect { 3000 } else { 10000 }
                    );
                    assert_eq!(j.work_count(), 2);
                    let key = *state.jobs.keys().next().unwrap();
                    let j = state.jobs.get_mut(&key).unwrap();
                    let first = j.dequeue_work().unwrap();
                    assert_eq!((first.work_id, first.delivery_count), (1, 2));
                    assert_eq!(j.dequeue_work().unwrap().work_id, 2);
                }
                let internal = callback
                    .stopped
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .successful;
                assert!(!internal);
                let count = state.jobs.len();
                jobs.connected(&mut state, callback.id, None, true, 12000);
                assert_eq!(state.jobs.len(), count);
            }
        }
        let jobs = scheduler(host);
        let mut state = jobs.state.lock().unwrap();
        let cb = attach(&jobs, &mut state, Verb::Binding);
        jobs.connected(&mut state, cb.id, None, false, 4000);
        assert!(state.jobs.is_empty()); // Explicit null binding is not a retry.
        let mut key = None;
        for stops in 1..=3 {
            let cb = attach(&jobs, &mut state, Verb::Executing);
            let temporary = state.contexts[0].key;
            if let Some(key) = key {
                state.jobs.remove(&temporary);
                state.contexts[0].key = key;
            }
            jobs.connected(&mut state, cb.id, None, true, 5000);
            retry(&state, 0, stops, Some(if stops < 3 { 0 } else { 35000 }));
            key = state.jobs.keys().next().copied();
        }
    });
}

#[test]
fn failed_stop_sends_do_not_invalidate_other_constraint_stops() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        let mut state = jobs.state.lock().unwrap();
        state.running = true;
        state.device = Some((
            Device {
                power_connected: false,
                charging: false,
                battery_not_low: true,
                storage_not_low: true,
                battery_saver: false,
                network: None,
            },
            1000,
        ));
        for id in 0..3 {
            attach(&jobs, &mut state, Verb::Executing);
            let c = state.contexts.last_mut().unwrap();
            c.service = Some(host.process.strong(u32::MAX));
            let key = c.key;
            let j = state.jobs.get_mut(&key).unwrap();
            j.job.id += id;
            j.required |= CONSTRAINT_CHARGING;
            j.job.constraint_flags |= CONSTRAINT_FLAG_CHARGING;
            j.process = Some("android".into());
            j.ready = true;
            j.satisfied = CONSTRAINT_CHARGING;
        }
        state.contexts.swap(0, 2); // Job order and context order need not agree.
        jobs.queue_ready(&mut state, 1000, false);
        assert!(state.contexts.is_empty());
        assert_eq!(state.jobs.len(), 3);
        for j in state.jobs.values() {
            assert_eq!((j.num_failures, j.num_system_stops), (0, 1));
        }
    });
}

#[test]
fn retry_boundaries_count_soft_system_stops_and_app_failures() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let jobs = scheduler(host);
        for policy in [BACKOFF_POLICY_LINEAR, BACKOFF_POLICY_EXPONENTIAL] {
            let mut j = status(sync_job(), 1000, 10100, "p", Some("SyncManager"));
            j.job.backoff_policy = policy;
            j.job.initial_backoff = 1;
            j.last_successful_run = 123;
            j.cumulative_execution = 456;
            for (stops, delay) in [
                (1, 0),
                (2, 0),
                (3, 10000),
                (4, 10000),
                (5, 10000),
                (6, 20000),
            ] {
                j = jobs.reschedule_for_failure(
                    &j,
                    STOP_REASON_CONSTRAINT_CHARGING,
                    INTERNAL_STOP_REASON_CONSTRAINTS_NOT_SATISFIED,
                    1000,
                );
                assert_eq!((j.num_failures, j.num_system_stops), (0, stops));
                assert_eq!(j.earliest, if delay == 0 { 0 } else { 1000 + delay });
            }
            j = jobs.reschedule_for_failure(
                &j,
                STOP_REASON_TIMEOUT,
                INTERNAL_STOP_REASON_TIMEOUT,
                2000,
            );
            assert_eq!((j.num_failures, j.num_system_stops), (1, 6));
            assert_eq!(
                j.earliest,
                2000 + if policy == BACKOFF_POLICY_LINEAR {
                    30000
                } else {
                    40000
                }
            );
            assert_eq!((j.last_successful_run, j.cumulative_execution), (123, 456));
            let user = jobs.reschedule_for_failure(
                &j,
                STOP_REASON_USER,
                INTERNAL_STOP_REASON_CANCELED,
                2000,
            );
            assert_eq!(user.num_failures, 2);
            assert_ne!(user.internal_flags & INTERNAL_FLAG_DEMOTED_BY_USER, 0);
        }
        for policy in [BACKOFF_POLICY_LINEAR, BACKOFF_POLICY_EXPONENTIAL, -1] {
            assert_eq!(backoff_delay(policy, 1, 0), None);
            assert_eq!(backoff_delay(policy, 1, 1), Some(10000));
            assert_eq!(
                backoff_delay(policy, i64::MAX, 2),
                Some(if policy == BACKOFF_POLICY_LINEAR {
                    -2
                } else {
                    MAX_BACKOFF_DELAY_MILLIS
                })
            );
            assert_eq!(
                backoff_delay(policy, 16777217, 1),
                Some(if policy == BACKOFF_POLICY_LINEAR {
                    16777217
                } else {
                    16777216
                })
            );
            assert_eq!(
                backoff_delay(policy, 1, i32::MAX),
                Some(MAX_BACKOFF_DELAY_MILLIS)
            );
        }
        assert_eq!(
            backoff_delay(BACKOFF_POLICY_LINEAR, 10000, 1799),
            Some(17990000)
        );
        assert_eq!(
            backoff_delay(BACKOFF_POLICY_LINEAR, 10000, 1800),
            Some(18000000)
        );
        assert_eq!(
            backoff_delay(BACKOFF_POLICY_LINEAR, 10000, 1801),
            Some(18000000)
        );
        assert_eq!(
            backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 10000, 11),
            Some(10240000)
        );
        assert_eq!(
            backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 10000, 12),
            Some(18000000)
        );
        let mut overflow = status(sync_job(), 1000, 10100, "p", Some("SyncManager"));
        overflow.job.set_backoff(i64::MAX, BACKOFF_POLICY_LINEAR);
        overflow.num_failures = 1;
        let next = jobs.reschedule_for_failure(
            &overflow,
            STOP_REASON_TIMEOUT,
            INTERNAL_STOP_REASON_TIMEOUT,
            1000,
        );
        assert_eq!(next.earliest, 998); // Java's long product wraps before the five-hour cap.
    });
}

#[test]
fn stale_work_grace_uses_finish_time_and_retains_the_first_stop_diagnostic() {
    let cb = JobCallback {
        scheduler: Weak::new(),
        id: 1,
        stopped: Mutex::new(None),
    };
    cb.stopped_because(Some("cancelled".into()), 1000);
    cb.stopped_because(Some("disconnected".into()), 2000);
    cb.finished(false, 3000);
    assert_eq!(cb.assert_current(None, 17999).unwrap(), None);
    let error = cb.assert_current(None, 18000).err().unwrap();
    assert!(error.message.contains("17000ms because: cancelled"));
    let stopped = cb.stopped.lock().unwrap();
    assert_eq!(stopped.as_ref().unwrap().at, 1000);
    drop(stopped);
    cb.finished(true, 19000);
    assert!(cb.assert_current(None, 19000).is_err());
}
