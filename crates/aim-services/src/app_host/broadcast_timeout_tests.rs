//! BroadcastQueueImpl's delivery timeout at android-16.0.0_r1: a delivery
//! waited for, of a manifest receiver or of a broadcast ordered, with a
//! result or prioritized, ends after BROADCAST_FG_TIMEOUT or
//! BROADCAST_BG_TIMEOUT unless its receiver finished first, and its
//! broadcast moves on; a silent ANR kills its process first.
use super::priority_tranche_tests::{
    ACTION, ManifestThread, Scheduled, delivered, finish, manifest, nothing, process,
    second_receiver, unordered,
};
use super::*;
use aim_binder_host::local::{Reply, Service};
use std::sync::mpsc;

/// The queue's timer, held by the test: the timeouts started.
fn timer(host: &AppHost) -> mpsc::Receiver<Started> {
    let (send, started) = mpsc::channel();
    *host.broadcasts.timer.lock().unwrap() = Some(send);
    started
}

/// The next timeout started, for the process of `seq`, of a foreground
/// broadcast or not, since `before`: its generation.
fn started(timer: &mpsc::Receiver<Started>, before: Instant, seq: i64, foreground: bool) -> u64 {
    let s = timer.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!((s.seq, s.foreground), (seq, foreground));
    assert!(s.at >= before && s.at <= Instant::now());
    s.generation
}

/// The app host agent, taking the pids it is to kill.
struct Agent(mpsc::Sender<i32>);
impl Service for Agent {
    fn descriptor(&self) -> &str {
        super::super::AGENT_DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(self.descriptor())?;
        assert_eq!(call.code, super::super::KILL_PROCESS);
        self.0.send(call.data.read_i32()?).unwrap();
        let mut reply = Parcel::new();
        reply.write_no_exception();
        Ok(reply)
    }
}

fn agent(host: &AppHost, export: &dyn Fn(Arc<dyn Service>) -> Strong) -> mpsc::Receiver<i32> {
    let (send, kills) = mpsc::channel();
    *host.agent.lock().unwrap() = Some(export(Arc::new(Agent(send))));
    kills
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
fn an_ordered_receiver_not_finishing_times_out_and_the_next_follows() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let kills = agent(host, export);
        let p = process(host, export, "p", 1000, 0, 0);
        let q = process(host, export, "q", 1001, 1, 0);
        let before = Instant::now();
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        let generation = started(&timeouts, before, 1000, true);
        nothing(&q.scheduled);
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&q.scheduled);
        nothing(&result);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
        // A top process's ANR is not silent: it is kept.
        nothing(&kills);
    });
}

#[test]
fn a_background_tranche_times_out() {
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
        let generation = started(&timeouts, before, 1000, false);
        nothing(&q.scheduled);
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&q.scheduled);
        finish(host, q.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn a_manifest_receiver_not_finishing_times_out() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        process(host, export, "android", 1000, 0, 0);
        let (send, scheduled) = mpsc::channel();
        let thread = export(Arc::new(ManifestThread(send)));
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&1000).unwrap();
            app.process = "system".into();
            app.config = Some(Default::default());
            app.thread = Some(thread);
        }
        let q = process(host, export, "q", 1001, 1, 0);
        let record = unordered(host);
        let before = Instant::now();
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
        let generation = started(&timeouts, before, 1000, false);
        nothing(&q.scheduled);
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&q.scheduled);
        let st = host.broadcasts.state.lock().unwrap();
        assert!(st.tranches.is_empty() && st.timeouts.is_empty() && !st.busy.contains(&1000));
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
        let first = started(&timeouts, before, 1000, true);
        finish(host, p.thread);
        delivered(&q.scheduled);
        let second = started(&timeouts, before, 1001, true);
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
fn a_silent_anr_kills_its_process_before_it_is_sent_more() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let kills = agent(host, export);
        let p = process(host, export, "p", 1000, 0, 0);
        second_receiver(host, export, 0, 0);
        // A cached process: its ANR is silent.
        host.apps.lock().unwrap().get_mut(&1000).unwrap().proc_state = 16;
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        let generation = started(
            &timeouts,
            Instant::now() - Duration::from_secs(2),
            1000,
            true,
        );
        host.broadcasts.timed_out(host, 1000, generation);
        assert_eq!(kills.recv_timeout(Duration::from_secs(2)).unwrap(), 1000);
        // Its next receiver waits for its death, which retires it.
        nothing(&p.scheduled);
        // The killed receiver's late finish completes nothing.
        finish(host, p.thread);
        nothing(&p.scheduled);
        nothing(&result);
        host.broadcasts.process_died(host, 1000);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn the_system_process_is_not_killed() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let kills = agent(host, export);
        let p = process(host, export, "android", 1000, 0, 0);
        second_receiver(host, export, 0, 0);
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&1000).unwrap();
            app.process = "system".into();
            app.proc_state = 16;
        }
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        let generation = started(
            &timeouts,
            Instant::now() - Duration::from_secs(2),
            1000,
            true,
        );
        host.broadcasts.timed_out(host, 1000, generation);
        delivered(&p.scheduled);
        nothing(&kills);
        finish(host, p.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn which_anrs_are_silent() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        process(host, export, "p", 1000, 0, 0);
        let mut apps = host.apps.lock().unwrap();
        let app = apps.get_mut(&1000).unwrap();
        let interesting = super::super::services::interesting_for_background_traces;
        assert!(interesting(app));
        app.proc_state = 16;
        assert!(!interesting(app));
        app.persistent = true;
        assert!(interesting(app));
        app.persistent = false;
        app.package = "com.android.systemui".into();
        assert!(interesting(app));
        app.package = "p".into();
        app.process = "system".into();
        assert!(interesting(app));
    });
}

#[test]
fn no_timeout_before_processes_are_ready() {
    crate::system::with_boot_discovery_binders(|host, _, _, export| {
        let timeouts = timer(host);
        let p = process(host, export, "p", 1000, 0, 0);
        let result = send_ordered(host, ACTION, FLAG_RECEIVER_FOREGROUND);
        delivered(&p.scheduled);
        nothing(&timeouts);
        finish(host, p.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
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
        let a: Scheduled = p.scheduled.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            a.intent.unwrap().intent.action.as_deref(),
            Some(ACTION_PRE_BOOT_COMPLETED)
        );
        nothing(&timeouts);
        finish(host, p.thread);
        result.recv_timeout(Duration::from_secs(2)).unwrap();
    });
}

#[test]
fn the_timeouts_follow_their_settings_and_the_multiplier() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        assert_eq!(Broadcasts::timeout(host, true), BROADCAST_FG_TIMEOUT);
        assert_eq!(Broadcasts::timeout(host, false), BROADCAST_BG_TIMEOUT);
        host.set_property("ro.hw_timeout_multiplier", "3");
        assert_eq!(Broadcasts::timeout(host, true), Duration::from_secs(30));
        let set = |name: &str, value: &str| {
            let change = super::super::settings::Change {
                value: Some(value),
                tag: None,
                make_default: false,
                package: Some("android"),
                caller_system: true,
                overrideable_by_restore: false,
            };
            let settings = &host.platform().unwrap().settings;
            settings
                .insert(super::super::settings::Table::Global, name, &change, false)
                .unwrap();
        };
        set("bcast_bg_constants", "foo=1, timeout = 1234");
        assert_eq!(
            Broadcasts::timeout(host, false),
            Duration::from_millis(1234)
        );
        assert_eq!(Broadcasts::timeout(host, true), Duration::from_secs(30));
    });
}
