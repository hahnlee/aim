//! `scheduleDeferredPendingKillInstallObserver` and its no-kill variant: an
//! update's install result waits until its old processes are killed, or a
//! delay passes, so the caller never sees success while they still run.
//! AOSP android-16.0.0_r1 PackageManagerService, Apache License 2.0.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

/// `DEFERRED_PENDING_KILL_INSTALL_OBSERVER_DELAY_MS`.
pub const PENDING_KILL: Duration = Duration::from_millis(1000);
/// `DEFERRED_NO_KILL_INSTALL_OBSERVER_DELAY_MS`.
pub const NO_KILL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Deferred {
    pending: Mutex<BTreeMap<i32, BTreeSet<String>>>,
    changed: Condvar,
}
impl Deferred {
    /// Blocks until every package in `killed` had its processes killed, or
    /// `delay` passes; with nothing to kill it waits out `delay`.
    pub fn wait(&self, session: i32, killed: BTreeSet<String>, delay: Duration) {
        let deadline = Instant::now() + delay;
        let waits_for_kill = !killed.is_empty();
        let mut pending = self.pending.lock().unwrap();
        pending.insert(session, killed);
        loop {
            if waits_for_kill && pending[&session].is_empty() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            pending = self.changed.wait_timeout(pending, deadline - now).unwrap().0;
        }
        pending.remove(&session);
    }
    /// `onPackageProcessKilledForUninstall`.
    pub fn killed(&self, package: &str) {
        for packages in self.pending.lock().unwrap().values_mut() {
            packages.remove(package);
        }
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_kill_ends_the_wait_and_otherwise_the_delay_does() {
        let deferred = Arc::new(Deferred::default());
        let waiter = deferred.clone();
        let started = Instant::now();
        let thread = std::thread::spawn(move || waiter.wait(1, BTreeSet::from(["p".to_owned()]), Duration::from_secs(10)));
        while deferred.pending.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        deferred.killed("other");
        deferred.killed("p");
        thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));

        let started = Instant::now();
        deferred.wait(2, BTreeSet::from(["q".to_owned()]), Duration::from_millis(50));
        assert!(started.elapsed() >= Duration::from_millis(50));
        let started = Instant::now();
        deferred.wait(3, BTreeSet::new(), Duration::from_millis(50));
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(deferred.pending.lock().unwrap().is_empty());
    }
}
