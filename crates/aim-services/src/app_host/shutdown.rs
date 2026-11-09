//! The app host's part of `ShutdownThread.run` (`android-16.0.0_r1`) at a
//! guest stop: the shutdown broadcast, waited for until its deadline, then
//! `ActivityManagerService.shutdown`'s usage stats shutdown. The activities
//! are not put to sleep first (`shuttingDown`), and app ops, battery stats
//! and process stats write nothing at shutdown (#1872).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::AppHost;
use super::kept::RawIntent;

/// `ShutdownThread.MAX_BROADCAST_TIME`.
pub const MAX_BROADCAST_TIME: Duration = Duration::from_secs(10);
const ACTION_SHUTDOWN: &str = "android.intent.action.ACTION_SHUTDOWN";
/// `Intent.FLAG_RECEIVER_FOREGROUND | FLAG_RECEIVER_REGISTERED_ONLY`.
const SHUTDOWN_FLAGS: i32 = 0x1000_0000 | 0x4000_0000;
const USER_ALL: i32 = -1;

impl AppHost {
    /// ACTION_SHUTDOWN to the registered receivers of every user, its
    /// result waited for until `deadline`, then the activity manager shut
    /// down. The native jobs are committed first: SyncManager's receiver
    /// writes the sync state that refers to them.
    pub(crate) fn shutdown_thread(&self, deadline: Instant) -> Result<(), String> {
        let jobs = self.job_scheduler.get().map_or(Ok(()), |jobs| jobs.flush());
        eprintln!("app host: Sending shutdown broadcast...");
        let (sent, done) = mpsc::sync_channel(1);
        // Its options' DEFERRAL_POLICY_UNTIL_ACTIVE is not modelled: the
        // broadcast queue defers no delivery to a cached process.
        self.broadcasts.send_from_system_with_callback(
            self,
            RawIntent::new(ACTION_SHUTDOWN, SHUTDOWN_FLAGS, &[]),
            USER_ALL,
            Box::new(move || {
                // After the deadline nobody waits for the result.
                let _ = sent.try_send(());
            }),
        );
        if done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            eprintln!("app host: Shutdown broadcast timed out");
        }
        eprintln!("app host: Shutting down activity manager...");
        let usage = self.prepare_usage_shutdown();
        jobs.map_err(|e| format!("job store shutdown: {}", e.message))?;
        usage.map_err(|e| format!("usagestats shutdown: {}", e.message))
    }
}
