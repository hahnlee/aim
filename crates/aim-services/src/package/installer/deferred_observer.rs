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

/// Each package's latest kill, ordered, so a kill reported before its
/// install starts waiting still ends that wait.
#[derive(Default)]
pub struct Deferred {
    kills: Mutex<Kills>,
    changed: Condvar,
}
#[derive(Default)]
struct Kills {
    sequence: u64,
    latest: BTreeMap<String, u64>,
}
impl Deferred {
    /// The point after which a kill counts for an install starting now.
    pub fn mark(&self) -> u64 {
        self.kills.lock().unwrap().sequence
    }
    /// Blocks until every package in `killed` had its processes killed after
    /// `since`, or `delay` passes; with nothing to kill it waits out `delay`.
    pub fn wait(&self, since: u64, killed: &BTreeSet<String>, delay: Duration) {
        let deadline = Instant::now() + delay;
        let mut kills = self.kills.lock().unwrap();
        loop {
            if !killed.is_empty()
                && killed
                    .iter()
                    .all(|name| kills.latest.get(name).is_some_and(|at| *at > since))
            {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            kills = self.changed.wait_timeout(kills, deadline - now).unwrap().0;
        }
    }
    /// `onPackageProcessKilledForUninstall`.
    pub fn killed(&self, package: &str) {
        let mut kills = self.kills.lock().unwrap();
        kills.sequence += 1;
        let sequence = kills.sequence;
        kills.latest.insert(package.to_owned(), sequence);
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
        let since = deferred.mark();
        let waiter = deferred.clone();
        let started = Instant::now();
        let thread = std::thread::spawn(move || {
            waiter.wait(
                since,
                &BTreeSet::from(["p".to_owned()]),
                Duration::from_secs(10),
            )
        });
        deferred.killed("other");
        deferred.killed("p");
        thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));

        // A kill reported before the wait begins still ends it.
        let since = deferred.mark();
        deferred.killed("q");
        let started = Instant::now();
        deferred.wait(
            since,
            &BTreeSet::from(["q".to_owned()]),
            Duration::from_secs(10),
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        // An earlier install's kill does not.
        let since = deferred.mark();
        let started = Instant::now();
        deferred.wait(
            since,
            &BTreeSet::from(["q".to_owned()]),
            Duration::from_millis(50),
        );
        assert!(started.elapsed() >= Duration::from_millis(50));
        let started = Instant::now();
        deferred.wait(since, &BTreeSet::new(), Duration::from_millis(50));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }
}
