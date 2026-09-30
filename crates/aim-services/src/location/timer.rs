//! The location service's own threads: an executor for what the original
//! posts to `FgThread` (work that must not run under the service's
//! lock), and alarms (`AlarmHelper.setDelayedAlarm`) on the guest's
//! elapsed-realtime clock.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

type Task = Box<dyn FnOnce() + Send>;

/// Runs tasks in order on one thread.
#[derive(Clone)]
pub struct Executor(Sender<Task>);

impl Executor {
    pub fn new(name: &str) -> Executor {
        let (tx, rx) = mpsc::channel::<Task>();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for task in rx {
                    task();
                }
            })
            .expect("spawn the executor");
        Executor(tx)
    }

    pub fn post(&self, task: impl FnOnce() + Send + 'static) {
        let _ = self.0.send(Box::new(task));
    }
}

#[derive(Default)]
struct Pending {
    /// (due, id) → what to run.
    due: BTreeMap<(i64, u64), Task>,
    next_id: u64,
}

/// Alarms on the elapsed-realtime clock; each runs once on the alarm
/// thread unless cancelled first.
#[derive(Clone)]
pub struct Alarms(Arc<(Mutex<Pending>, Condvar)>);

impl Alarms {
    pub fn new(now_ms: fn() -> i64) -> Alarms {
        let alarms = Alarms(Arc::new((Mutex::new(Pending::default()), Condvar::new())));
        let shared = alarms.0.clone();
        std::thread::Builder::new()
            .name("location-alarms".into())
            .spawn(move || {
                let (lock, cond) = &*shared;
                let mut pending = lock.lock().unwrap();
                loop {
                    let now = now_ms();
                    let next = pending.due.keys().next().copied();
                    match next {
                        Some(key) if key.0 <= now => {
                            let task = pending.due.remove(&key).unwrap();
                            drop(pending);
                            task();
                            pending = lock.lock().unwrap();
                        }
                        Some((due, _)) => {
                            let wait = Duration::from_millis((due - now).max(1) as u64);
                            pending = cond.wait_timeout(pending, wait).unwrap().0;
                        }
                        None => pending = cond.wait(pending).unwrap(),
                    }
                }
            })
            .expect("spawn the alarm thread");
        alarms
    }

    /// Runs `task` at `due` (elapsed realtime, ms); the id cancels it.
    pub fn set(&self, due: i64, task: impl FnOnce() + Send + 'static) -> u64 {
        let (lock, cond) = &*self.0;
        let mut pending = lock.lock().unwrap();
        pending.next_id += 1;
        let id = pending.next_id;
        pending.due.insert((due, id), Box::new(task));
        cond.notify_one();
        id
    }

    pub fn cancel(&self, id: u64) {
        let (lock, _) = &*self.0;
        lock.lock().unwrap().due.retain(|&(_, i), _| i != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn now() -> i64 {
        aim_hostcall::clock::boottime_ns() / 1_000_000
    }

    #[test]
    fn alarms_run_once_in_order_unless_cancelled() {
        let alarms = Alarms::new(now);
        let (tx, rx) = mpsc::channel();
        let t = now();
        let tx2 = tx.clone();
        alarms.set(t + 30, move || tx2.send(2).unwrap());
        alarms.set(t + 10, move || tx.send(1).unwrap());
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        let id = alarms.set(t + 20, move || {
            r.fetch_add(1, Ordering::Relaxed);
        });
        alarms.cancel(id);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        assert!(now() >= t + 30);
        assert_eq!(ran.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn executor_runs_in_order() {
        let e = Executor::new("test-executor");
        let (tx, rx) = mpsc::channel();
        for i in 0..5 {
            let tx = tx.clone();
            e.post(move || tx.send(i).unwrap());
        }
        let got: Vec<i32> = (0..5).map(|_| rx.recv().unwrap()).collect();
        assert_eq!(got, [0, 1, 2, 3, 4]);
    }
}
