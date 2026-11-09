//! BroadcastQueueImpl's delivery timeout at android-16.0.0_r1: a delivery
//! waited for, of a broadcast ordered, with a result or prioritized, ends
//! after BROADCAST_FG_TIMEOUT or BROADCAST_BG_TIMEOUT unless its receiver
//! finished first, and its broadcast moves on.
use super::priority_tranche_tests::{ACTION, delivered, finish, nothing, process};
use super::*;
use std::sync::mpsc;

/// The queue's timer, held by the test: the timeouts it is given.
fn timer(host: &AppHost) -> mpsc::Receiver<(Instant, i64, u64)> {
    let (send, timeouts) = mpsc::channel();
    *host.broadcasts.timer.lock().unwrap() = Some(send);
    timeouts
}

/// The next timeout, for the process of `seq`, due `after` from when it was
/// set, between `before` and now: its generation.
fn timeout(
    timeouts: &mpsc::Receiver<(Instant, i64, u64)>,
    before: Instant,
    seq: i64,
    after: Duration,
) -> u64 {
    let (at, of, generation) = timeouts.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(of, seq);
    assert!(at >= before + after && at <= Instant::now() + after);
    generation
}

/// Sends `action` ordered with `flags` to the registered receivers, with a
/// result: its arrival.
fn send_ordered(host: &AppHost, action: &str, flags: i32) -> mpsc::Receiver<()> {
    let (done, result) = mpsc::channel();
    host.broadcasts.send_from_system_with(
        host,
        RawIntent::new(action, FLAG_RECEIVER_REGISTERED_ONLY | flags, &[]),
        USER_ALL,
        &[],
        None,
        Some(Box::new(move || done.send(()).unwrap())),
    );
    result
}

#[test]
fn an_ordered_receiver_not_finishing_times_out_at_the_foreground_timeout() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let p = process(host, export, "p", 1000, 0, 0);
        let q = process(host, export, "q", 1001, 1, 0);
        let before = Instant::now();
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        let generation = timeout(&timeouts, before, 1000, BROADCAST_FG_TIMEOUT);
        nothing(&q.scheduled);
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&q.scheduled);
        nothing(&result);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn a_background_tranche_times_out_at_the_background_timeout() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let p = process(host, export, "p", 1000, 0, 10);
        let q = process(host, export, "q", 1001, 1, 0);
        let (done, result) = mpsc::channel();
        let before = Instant::now();
        host.broadcasts.send_from_system_with_callback(
            host,
            RawIntent::new(ACTION, FLAG_RECEIVER_REGISTERED_ONLY, &[]),
            USER_ALL,
            Box::new(move || done.send(()).unwrap()),
        );
        delivered(&p.scheduled);
        let generation = timeout(&timeouts, before, 1000, BROADCAST_BG_TIMEOUT);
        nothing(&q.scheduled);
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&q.scheduled);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn a_receiver_finishing_before_its_deadline_is_not_timed_out() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let p = process(host, export, "p", 1000, 0, 0);
        let q = process(host, export, "q", 1001, 1, 0);
        let before = Instant::now();
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        let first = timeout(&timeouts, before, 1000, BROADCAST_FG_TIMEOUT);
        finish(host, p.thread);
        delivered(&q.scheduled);
        let second = timeout(&timeouts, before, 1001, BROADCAST_FG_TIMEOUT);
        // The first deadline passes after its receiver finished.
        host.broadcasts.timed_out(host, 1000, first);
        assert_eq!(
            host.broadcasts.state.lock().unwrap().timeouts.get(&1001),
            Some(&second)
        );
        nothing(&result);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(host.broadcasts.state.lock().unwrap().timeouts.is_empty());
    });
}

#[test]
fn pre_boot_completed_is_exempt() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let p = process(host, export, "p", 1000, 0, 0);
        host.broadcasts.state.lock().unwrap().receivers[0].filters[0]
            .filter
            .add_action(ACTION_PRE_BOOT_COMPLETED);
        let result = send_ordered(host, ACTION_PRE_BOOT_COMPLETED, 0);
        delivered_action(&p.scheduled, ACTION_PRE_BOOT_COMPLETED);
        nothing(&timeouts);
        finish(host, p.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

fn delivered_action(
    scheduled: &mpsc::Receiver<super::priority_tranche_tests::Scheduled>,
    action: &str,
) {
    let a = scheduled.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(a.intent.unwrap().intent.action.as_deref(), Some(action));
}
