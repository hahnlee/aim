//! ShutdownThread.run at android-16.0.0_r1: ACTION_SHUTDOWN to the
//! registered receivers, its result waited for until the deadline.
use super::*;
use aim_binder_host::local::{Reply, Service};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SHUTDOWN: &str = "android.intent.action.ACTION_SHUTDOWN";

type Scheduled = thread::ScheduleRegisteredReceiver<RawIntent, RawBundle>;

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

/// A system process's receiver of ACTION_SHUTDOWN, the `at`th registered,
/// in the process of start `uid`: its deliveries and its thread.
fn shutdown_receiver(
    host: &Arc<AppHost>,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
    package: &str,
    uid: i32,
    at: usize,
) -> (mpsc::Receiver<Scheduled>, Binder) {
    super::internal_package_tests::register_filter(host, export, package, uid, false);
    let mut filter = IntentFilter::default();
    filter.add_action(SHUTDOWN);
    host.broadcasts.state.lock().unwrap().receivers[at].filters[0].filter = filter;
    let (send, received) = mpsc::channel();
    let thread = export(Arc::new(AppThread(send)));
    let who = thread.binder();
    host.apps
        .lock()
        .unwrap()
        .get_mut(&i64::from(uid))
        .unwrap()
        .thread = Some(thread);
    (received, who)
}

/// `finishReceiver` of the receiver in the process of `who`.
fn finish(host: &AppHost, who: Binder) {
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

fn delivered(received: &mpsc::Receiver<Scheduled>) {
    let a = received.recv_timeout(Duration::from_secs(2)).unwrap();
    let intent = a.intent.unwrap().intent;
    assert_eq!(intent.action.as_deref(), Some(SHUTDOWN));
    // FOREGROUND | REGISTERED_ONLY, and AM's EXCLUDE_STOPPED_PACKAGES.
    assert_eq!(intent.flags, 0x5000_0010);
    // broadcastIntentWithCallback: a result, not serialized.
    assert!(!a.ordered && !a.assume_delivered && !a.sticky);
    assert_eq!(a.sending_user, USER_ALL);
}

#[test]
fn shutdown_waits_for_its_receivers_to_finish() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (received, who) = shutdown_receiver(host, export, "p", 1000, 0);
        let finishing = host.clone();
        let receiver = std::thread::spawn(move || {
            delivered(&received);
            std::thread::sleep(Duration::from_millis(100));
            finish(&finishing, who);
        });
        let start = Instant::now();
        host.shutdown_thread(start + Duration::from_secs(5))
            .unwrap();
        let elapsed = start.elapsed();
        receiver.join().unwrap();
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_secs(5),
            "{elapsed:?}"
        );
    });
}

#[test]
fn shutdown_goes_on_at_its_deadline() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (received, _) = shutdown_receiver(host, export, "p", 1000, 0);
        let start = Instant::now();
        host.shutdown_thread(start + Duration::from_millis(200))
            .unwrap();
        let elapsed = start.elapsed();
        delivered(&received);
        assert!(
            elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );
    });
}

/// Not ordered, ACTION_SHUTDOWN goes to every process at once: a hung
/// receiver keeps none of the others from it, and the result waits for it
/// until the deadline.
#[test]
fn a_hung_receiver_holds_back_no_other_process() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (hung, _) = shutdown_receiver(host, export, "p", 1000, 0);
        let (received, who) = shutdown_receiver(host, export, "q", 1001, 1);
        let finishing = host.clone();
        let receiver = std::thread::spawn(move || {
            delivered(&received);
            finish(&finishing, who);
        });
        let start = Instant::now();
        host.shutdown_thread(start + Duration::from_millis(300))
            .unwrap();
        let elapsed = start.elapsed();
        receiver.join().unwrap();
        delivered(&hung);
        assert!(
            elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );
    });
}

/// A receiver's process dying while it has the broadcast counts as its
/// finish.
#[test]
fn a_receiver_dying_mid_delivery_finishes_it() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (dying, _) = shutdown_receiver(host, export, "p", 1000, 0);
        let (received, who) = shutdown_receiver(host, export, "q", 1001, 1);
        let finishing = host.clone();
        let receivers = std::thread::spawn(move || {
            delivered(&dying);
            finishing.broadcasts.process_died(&finishing, 1000);
            delivered(&received);
            finish(&finishing, who);
        });
        let start = Instant::now();
        host.shutdown_thread(start + Duration::from_secs(5))
            .unwrap();
        let elapsed = start.elapsed();
        receivers.join().unwrap();
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    });
}
