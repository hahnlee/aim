//! Host wait ownership for a daemonized application, independent of IPC policy.

use crate::ProfileError;
use crate::process_wait::{PollOutcome, ProcessWaitOwner};
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn supervise(waiter: ProcessWaitOwner) -> Result<(), ProfileError> {
    supervise_with_spawn(waiter, |job| {
        thread::Builder::new()
            .name("darwin-art-application-wait".into())
            .spawn(job)
            .map(|_| ())
    })
}

fn supervise_with_spawn(
    waiter: ProcessWaitOwner,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
) -> Result<(), ProfileError> {
    // The caller keeps a transfer slot until worker creation succeeds. A
    // failed spawn drops the closure, not the only completion owner.
    let transfer = Arc::new(Mutex::new(Some(waiter)));
    let worker_transfer = Arc::clone(&transfer);
    let result = spawn(Box::new(move || {
        let mut waiter = worker_transfer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .expect("application wait owner is transferred once");
        let mut last_diagnostic = None;
        loop {
            match waiter.poll() {
                PollOutcome::Reaped(_) | PollOutcome::GoneWithoutStatus => return,
                PollOutcome::Unknown(error) => {
                    let now = Instant::now();
                    if last_diagnostic
                        .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
                    {
                        eprintln!("darwin-artd: application {} wait: {error}", waiter.pid());
                        last_diagnostic = Some(now);
                    }
                }
                PollOutcome::Pending
                | PollOutcome::OwnershipUnavailableLive
                | PollOutcome::TerminalAwaitingReap => {}
            }
            thread::sleep(Duration::from_millis(10));
        }
    }));
    if let Err(error) = result {
        if let Some(waiter) = transfer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            // Failure to start supervision is launch cancellation policy.
            // Drop retains reap/callback responsibility even if signaling is
            // unavailable; it never publishes an unproven child exit.
            let _ = waiter.kill_if_exact_live();
            drop(waiter);
        }
        return Err(ProfileError::Daemon(format!(
            "application supervisor: {error}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_incarnation::ProcessIncarnation;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn owner(script: &str, completed: &Arc<AtomicUsize>) -> ProcessWaitOwner {
        let child = Command::new("/bin/sh")
            .args(["-c", script])
            .spawn()
            .unwrap();
        let identity = ProcessIncarnation::read(child.id()).unwrap();
        let completed = Arc::clone(completed);
        ProcessWaitOwner::from_child(
            child,
            identity,
            Box::new(move || {
                completed.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    fn await_completion(completed: &AtomicUsize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while completed.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "completion owner was lost");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn worker_reaps_application_before_completion() {
        let completed = Arc::new(AtomicUsize::new(0));
        supervise(owner("exit 7", &completed)).unwrap();
        await_completion(&completed);
    }

    #[test]
    fn failed_worker_spawn_retains_completion_owner() {
        let completed = Arc::new(AtomicUsize::new(0));
        let result = supervise_with_spawn(owner("sleep 1", &completed), |job| {
            drop(job);
            Err(io::Error::other("injected worker creation failure"))
        });
        assert!(result.is_err());
        await_completion(&completed);
    }
}
