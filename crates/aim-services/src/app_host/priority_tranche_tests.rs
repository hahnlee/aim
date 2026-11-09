//! BroadcastRecord.calculateBlockedUntilBeyondCount and
//! BroadcastQueueModernImpl at android-16.0.0_r1: a broadcast not ordered
//! whose receivers differ in priority delivers its lower tranche once its
//! higher one finished, across processes unless LIMIT_PRIORITY_SCOPE
//! limits priority to each process.
use super::*;
use aim_binder_host::local::{Reply, Service};
use std::sync::mpsc;
use std::time::Duration;

pub(super) const ACTION: &str = "fixture.PRIORITIZED";

pub(super) type Scheduled = thread::ScheduleRegisteredReceiver<RawIntent, RawBundle>;

struct AppThread(mpsc::Sender<Scheduled>);
impl Service for AppThread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_REGISTERED_RECEIVER);
        self.0.send(Scheduled::read(&mut call.data)?).unwrap();
        Ok(Parcel::new())
    }
}

pub(super) struct Process {
    pub(super) scheduled: mpsc::Receiver<Scheduled>,
    pub(super) thread: Binder,
}

/// A process of `uid` with a receiver of ACTION at `priority`, the `at`th
/// registered.
pub(super) fn process(
    host: &Arc<AppHost>,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
    package: &str,
    uid: i32,
    at: usize,
    priority: i32,
) -> Process {
    super::internal_package_tests::register_filter(host, export, package, uid, false);
    let mut filter = IntentFilter::default();
    filter.add_action(ACTION);
    filter.priority = priority;
    host.broadcasts.state.lock().unwrap().receivers[at].filters[0].filter = filter;
    let (send, scheduled) = mpsc::channel();
    let thread = export(Arc::new(AppThread(send)));
    let who = thread.binder();
    host.apps
        .lock()
        .unwrap()
        .get_mut(&i64::from(uid))
        .unwrap()
        .thread = Some(thread);
    Process {
        scheduled,
        thread: who,
    }
}

/// A second receiver of the `at`th registered's process, at `priority`.
fn second_receiver(
    host: &AppHost,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
    at: usize,
    priority: i32,
) {
    let (send, _) = mpsc::channel();
    let receiver = export(Arc::new(AppThread(send)));
    let mut st = host.broadcasts.state.lock().unwrap();
    let first = &st.receivers[at];
    let f = &first.filters[0];
    let mut filter = f.filter.clone();
    filter.priority = priority;
    let second = Receiver {
        receiver,
        start_seq: first.start_seq,
        pid: first.pid,
        uid: first.uid,
        user: first.user,
        filters: vec![Filter {
            filter,
            package: f.package.clone(),
            feature_id: None,
            receiver_id: None,
            permission: None,
            instant_app: false,
            visible_to_instant_apps: false,
            exported: true,
        }],
    };
    st.receivers.push(second);
}

/// Sends ACTION to the registered receivers with a result: its result's
/// arrival.
fn send(host: &AppHost) -> mpsc::Receiver<()> {
    let (done, result) = mpsc::channel();
    host.broadcasts.send_from_system_with_callback(
        host,
        RawIntent::new(ACTION, FLAG_RECEIVER_REGISTERED_ONLY, &[]),
        USER_ALL,
        Box::new(move || done.send(()).unwrap()),
    );
    result
}

/// The receiver of the next delivery.
pub(super) fn delivered(scheduled: &mpsc::Receiver<Scheduled>) -> Option<Binder> {
    let a = scheduled.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(a.intent.unwrap().intent.action.as_deref(), Some(ACTION));
    a.receiver
}

pub(super) fn nothing<T>(r: &mpsc::Receiver<T>) {
    assert!(r.recv_timeout(Duration::from_millis(150)).is_err());
}

pub(super) fn finish(host: &AppHost, who: Binder) {
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

#[test]
fn a_lower_tranche_waits_for_the_higher_in_another_process() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let p = process(host, export, "p", 1000, 0, 10);
        let q = process(host, export, "q", 1001, 1, 0);
        let result = send(host);
        delivered(&p.scheduled);
        nothing(&q.scheduled);
        finish(host, p.thread);
        delivered(&q.scheduled);
        nothing(&result);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn limited_priority_scope_delivers_to_other_processes_at_once() {
    let flags = [(LIMIT_PRIORITY_SCOPE_FLAG, true)];
    crate::system::with_discovery_binders_flags(&flags, true, |host, _, _, export| {
        let p = process(host, export, "p", 1000, 0, 10);
        let q = process(host, export, "q", 1001, 1, 0);
        let result = send(host);
        delivered(&p.scheduled);
        delivered(&q.scheduled);
        finish(host, q.thread);
        nothing(&result);
        finish(host, p.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn limited_priority_scope_keeps_the_order_within_a_process() {
    let flags = [(LIMIT_PRIORITY_SCOPE_FLAG, true)];
    crate::system::with_discovery_binders_flags(&flags, true, |host, _, _, export| {
        let p = process(host, export, "p", 1000, 0, 0);
        second_receiver(host, export, 0, 10);
        // Each in turn, the higher first: swapping their priorities swaps
        // their order.
        let mut firsts = Vec::new();
        for priorities in [[0, 10], [10, 0]] {
            for (r, priority) in host
                .broadcasts
                .state
                .lock()
                .unwrap()
                .receivers
                .iter_mut()
                .zip(priorities)
            {
                r.filters[0].filter.priority = priority;
            }
            let result = send(host);
            let first = delivered(&p.scheduled);
            nothing(&p.scheduled);
            finish(host, p.thread);
            assert_ne!(delivered(&p.scheduled), first);
            nothing(&result);
            finish(host, p.thread);
            result.recv_timeout(Duration::from_secs(2)).unwrap();
            firsts.push(first);
        }
        assert_ne!(firsts[0], firsts[1]);
    });
}

/// A plain broadcast's queue blocked behind a higher tranche runs once
/// that tranche is terminal.
#[test]
fn a_blocked_queue_becomes_runnable_past_its_tranche() {
    let record = Arc::new(Record {
        enqueue_id: 4,
        intent: RawIntent::new(ACTION, 0, &[]),
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
    let to = |start_seq, priority| Registered {
        handle: 1,
        filter: 0,
        priority,
        start_seq,
    };
    let mut st = State::default();
    let blocked_until = blocked_until_beyond_count(&[10, 10, 0], false, &[]);
    st.tranches.insert(
        4,
        Tranches {
            terminal: vec![false; 3],
            blocked_until,
            beyond: 0,
        },
    );
    for (seq, at, priority) in [(1, 0, 10), (2, 1, 10), (3, 2, 0)] {
        st.queues.insert(
            seq,
            VecDeque::from([Delivery::Once(record.clone(), to(seq, priority), at)]),
        );
        Broadcasts::make_runnable(&mut st, QueueId::Warm(seq));
    }
    assert!(st.runnable == [QueueId::Warm(1), QueueId::Warm(2)]);
    st.runnable.clear();
    st.queues.remove(&2);
    Broadcasts::terminal(&mut st, 4, 1);
    assert!(st.runnable.is_empty());
    st.queues.remove(&1);
    Broadcasts::terminal(&mut st, 4, 0);
    assert!(st.runnable == [QueueId::Warm(3)]);
    st.queues.remove(&3);
    Broadcasts::terminal(&mut st, 4, 2);
    assert!(st.tranches.is_empty());
}

#[test]
fn tranches_without_limited_scope_follow_priority() {
    assert_eq!(
        blocked_until_beyond_count(&[10, 10, 0, 0, -5], false, &[]),
        [0, 0, 2, 2, 4]
    );
    assert_eq!(blocked_until_beyond_count(&[3, 3, 3], false, &[]), [-1; 3]);
}

#[test]
fn limited_scope_blocks_only_around_receivers_with_the_change_disabled() {
    let all = |priorities: &[i32], enabled: bool| {
        blocked_until_beyond_count(priorities, true, &vec![enabled; priorities.len()])
    };
    assert_eq!(all(&[10, 0, -5], true), [-1; 3]);
    assert_eq!(all(&[10, 10, 0, 0], false), [0, 0, 2, 2]);
    assert_eq!(all(&[5, 5], false), [-1; 2]);
    // An enabled receiver waits for a disabled one of higher priority, and
    // a disabled one for the whole higher tranche.
    assert_eq!(
        blocked_until_beyond_count(&[10, 0], true, &[false, true]),
        [0, 1]
    );
    assert_eq!(
        blocked_until_beyond_count(&[10, 0], true, &[true, false]),
        [0, 1]
    );
    assert_eq!(
        blocked_until_beyond_count(&[10, 10, 0, -5], true, &[false, true, true, true]),
        [0, 0, 1, 1]
    );
}

/// A process dying with the higher tranche's delivery finishes it: the
/// lower tranche goes on.
#[test]
fn a_dead_higher_process_unblocks_the_lower_tranche() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let p = process(host, export, "p", 1000, 0, 10);
        let q = process(host, export, "q", 1001, 1, 0);
        let result = send(host);
        delivered(&p.scheduled);
        nothing(&q.scheduled);
        host.broadcasts.process_died(host, 1000);
        delivered(&q.scheduled);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

fn unordered(host: &AppHost) -> Arc<Record> {
    Arc::new(Record {
        enqueue_id: host.broadcasts.state.lock().unwrap().enqueue_id(),
        intent: RawIntent::new(ACTION, FLAG_RECEIVER_INCLUDE_BACKGROUND, &[]),
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
    })
}

/// A manifest receiver at `priority` of the process `system` of `uid`.
fn manifest(class: &str, uid: u32, priority: i32) -> Arc<Manifest> {
    let mut m = super::system_receiver_tests::receiver("android", class, uid);
    Arc::get_mut(&mut m).unwrap().priority = priority;
    m
}

/// A process not running is not started for a blocked receiver: its
/// queue waits until the tranche before it is terminal.
#[test]
fn a_cold_start_waits_while_its_receiver_is_blocked() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let q = process(host, export, "q", 1001, 0, 10);
        let record = unordered(host);
        let mut st = host.broadcasts.state.lock().unwrap();
        let first = Registered {
            handle: st.receivers[0].receiver.handle,
            filter: 0,
            priority: 10,
            start_seq: 1001,
        };
        // Nothing runs as `system` of this uid.
        let cold = manifest("android.Cold", 10123, 0);
        let targets = vec![Target::Registered(first), Target::Manifest(cold.clone())];
        // The registered receiver is assumed delivered once sent: hold its
        // process busy so that it is not yet.
        st.busy.insert(1001);
        host.broadcasts
            .prioritize(host, &mut st, &record, &targets)
            .unwrap();
        host.broadcasts.enqueue(host, &mut st, &record, targets);
        let queue = QueueId::Cold("system".into(), 10123);
        assert!(!st.runnable.contains(&queue));
        host.broadcasts.start_cold(host, &mut st, "system", 10123);
        assert_eq!(st.cold.len(), 1);
        assert!(st.cold_start.is_none());
        st.busy.remove(&1001);
        host.broadcasts.send_next(host, &mut st, 1001);
        assert!(st.runnable.contains(&queue));
        drop(st);
        delivered(&q.scheduled);
    });
}

/// A plain broadcast: its registered receiver of the lower tranche is sent
/// it once the manifest receiver of the higher one, in another process,
/// finished.
#[test]
fn a_plain_broadcast_waits_for_its_higher_manifest_receiver() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        process(host, export, "android", 1000, 0, 0);
        let (send, scheduled) = mpsc::channel();
        let thread = export(Arc::new(ManifestThread(send)));
        let who = thread.binder();
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&1000).unwrap();
            app.process = "system".into();
            app.config = Some(Default::default());
            app.thread = Some(thread);
        }
        let q = process(host, export, "q", 1001, 1, 0);
        let record = unordered(host);
        let mut st = host.broadcasts.state.lock().unwrap();
        let lower = Registered {
            handle: st.receivers[1].receiver.handle,
            filter: 0,
            priority: 0,
            start_seq: 1001,
        };
        let targets = vec![
            Target::Manifest(manifest("android.Higher", 1000, 10)),
            Target::Registered(lower),
        ];
        host.broadcasts
            .prioritize(host, &mut st, &record, &targets)
            .unwrap();
        host.broadcasts.enqueue(host, &mut st, &record, targets);
        drop(st);
        scheduled.recv_timeout(Duration::from_secs(2)).unwrap();
        nothing(&q.scheduled);
        finish(host, who);
        delivered(&q.scheduled);
        assert!(host.broadcasts.state.lock().unwrap().tranches.is_empty());
    });
}

/// A thread that takes a manifest receiver's `scheduleReceiver`.
struct ManifestThread(mpsc::Sender<()>);
impl Service for ManifestThread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_RECEIVER);
        self.0.send(()).unwrap();
        Ok(Parcel::new())
    }
}
