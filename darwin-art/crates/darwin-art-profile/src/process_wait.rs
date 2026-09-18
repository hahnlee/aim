//! Incarnation-scoped ownership of a Darwin child wait status.
//!
//! `std::process::Child` caches a status observed by its `try_wait`/`wait`
//! methods.  That cache is unsafe for a process which may be inspected by a
//! debugger (a traced stop is not process termination), or whose wait status
//! may be consumed by another waiter.  Once a child enters this owner, all
//! observations use raw `waitpid` and a PID birth identity probe.

use crate::ProfileError;
use crate::process_incarnation::{ProcessIncarnation, ProcessObservation};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

type ExitCallback = Box<dyn FnOnce() + Send + 'static>;

/// A status which does not claim termination unless the original process was
/// actually reaped, or its original incarnation has disappeared.
#[derive(Debug)]
pub(crate) enum PollOutcome {
    Pending,
    OwnershipUnavailableLive,
    TerminalAwaitingReap,
    Reaped(ExitStatus),
    GoneWithoutStatus,
    Unknown(WaitError),
}

#[derive(Debug)]
pub(crate) enum WaitError {
    Wait(io::Error),
    Probe(ProfileError),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wait(error) => write!(formatter, "waitpid: {error}"),
            Self::Probe(error) => write!(formatter, "process identity probe: {error}"),
        }
    }
}

impl std::error::Error for WaitError {}

struct Shared {
    pid: u32,
    incarnation: ProcessIncarnation,
    callback: Mutex<Option<ExitCallback>>,
    terminal: AtomicBool,
    terminal_outcome: Mutex<Option<TerminalOutcome>>,
    #[cfg(test)]
    observed_stop: AtomicBool,
}

#[derive(Clone, Debug)]
enum TerminalOutcome {
    Reaped(ExitStatus),
    GoneWithoutStatus,
}

/// The sole host owner of a child PID's wait status.
pub(crate) struct ProcessWaitOwner {
    shared: Arc<Shared>,
}

impl ProcessWaitOwner {
    /// Transfer a spawned child into the raw wait owner.  Dropping `Child`
    /// here is intentional: no caller may subsequently use its cached wait
    /// state.  Dropping a `Child` does not wait or signal on Darwin.
    pub(crate) fn from_child(
        child: Child,
        incarnation: ProcessIncarnation,
        callback: ExitCallback,
    ) -> Self {
        let pid = child.id();
        drop(child);
        Self::from_pid(pid, incarnation, callback)
    }

    pub(crate) fn from_pid(
        pid: u32,
        incarnation: ProcessIncarnation,
        callback: ExitCallback,
    ) -> Self {
        assert!(pid != 0, "process wait owner requires a nonzero PID");
        Self {
            shared: Arc::new(Shared {
                pid,
                incarnation,
                callback: Mutex::new(Some(callback)),
                terminal: AtomicBool::new(false),
                terminal_outcome: Mutex::new(None),
                #[cfg(test)]
                observed_stop: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.shared.pid
    }

    pub(crate) fn poll(&mut self) -> PollOutcome {
        let outcome = poll_shared(&self.shared);
        if matches!(
            outcome,
            PollOutcome::Reaped(_) | PollOutcome::GoneWithoutStatus
        ) {
            settle_callback(&self.shared);
        }
        outcome
    }

    /// Attach caller-owned retirement before transferring this sole waiter.
    /// A worker-creation failure must not drop a bound-service reservation's
    /// completion while the process-lease completion survives in quarantine.
    pub(crate) fn add_completion(&mut self, completion: ExitCallback) {
        let mut callback = self
            .shared
            .callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shared.terminal.load(Ordering::Acquire) {
            drop(callback);
            completion();
            return;
        }
        let previous = callback.take();
        *callback = Some(Box::new(move || {
            if let Some(previous) = previous {
                previous();
            }
            completion();
        }));
    }

    /// Send SIGKILL only after an exact live-incarnation probe.  The probe and
    /// signal are not atomic. Birth, liveness and parent ownership are read
    /// from one kernel snapshot; a debugger-owned process is never admitted.
    pub(crate) fn kill_if_exact_live(&self) -> Result<bool, WaitError> {
        if self.shared.terminal.load(Ordering::Acquire) {
            return Ok(false);
        }
        let details =
            ProcessIncarnation::observe_with_parent(self.shared.pid).map_err(WaitError::Probe)?;
        match details.observation {
            ProcessObservation::Live(actual)
                if actual == self.shared.incarnation
                    && details.parent_pid == std::process::id() =>
            {
                let result = unsafe { libc::kill(self.shared.pid as i32, libc::SIGKILL) };
                if result == 0 {
                    Ok(true)
                } else {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::ESRCH) {
                        Ok(false)
                    } else {
                        Err(WaitError::Wait(error))
                    }
                }
            }
            ProcessObservation::Live(_)
            | ProcessObservation::Zombie(_)
            | ProcessObservation::Absent => Ok(false),
        }
    }

    fn spawn_quarantine(&self) {
        if self.shared.terminal.load(Ordering::Acquire) {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let synchronous = Arc::clone(&shared);
        let result = thread::Builder::new()
            .name("darwin-art-process-quarantine".into())
            .spawn(move || quarantine(shared));
        if let Err(error) = result {
            // Keep the callback/identity owner synchronously when the worker
            // cannot be created; publishing a lease release is never safe.
            eprintln!("darwin-artd: process wait quarantine unavailable: {error}");
            quarantine(synchronous);
        }
    }
}

impl Drop for ProcessWaitOwner {
    fn drop(&mut self) {
        self.spawn_quarantine();
    }
}

fn poll_shared(shared: &Shared) -> PollOutcome {
    if shared.terminal.load(Ordering::Acquire) {
        return shared
            .terminal_outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .map(|outcome| match outcome {
                TerminalOutcome::Reaped(status) => PollOutcome::Reaped(status),
                TerminalOutcome::GoneWithoutStatus => PollOutcome::GoneWithoutStatus,
            })
            .unwrap_or(PollOutcome::Unknown(WaitError::Wait(io::Error::new(
                io::ErrorKind::Other,
                "terminal process wait owner lost its cached outcome",
            ))));
    }
    // Check birth and current parent together on EVERY attempt. An ECHILD
    // observation is neither death nor permanent loss: ptrace can return the
    // exact original child to us, including an unreaped zombie. Numeric
    // probe/wait is not atomic against an uncontrolled external reaper.
    match ProcessIncarnation::observe_with_parent(shared.pid) {
        Ok(details) => match details.observation {
            ProcessObservation::Live(actual) if actual == shared.incarnation => {
                if details.parent_pid != std::process::id() {
                    return PollOutcome::OwnershipUnavailableLive;
                }
            }
            ProcessObservation::Zombie(actual) if actual == shared.incarnation => {
                if details.parent_pid != std::process::id() {
                    return PollOutcome::TerminalAwaitingReap;
                }
            }
            ProcessObservation::Absent
            | ProcessObservation::Live(_)
            | ProcessObservation::Zombie(_) => {
                cache_terminal(shared, TerminalOutcome::GoneWithoutStatus);
                settle_callback(shared);
                return PollOutcome::GoneWithoutStatus;
            }
        },
        Err(error) => return PollOutcome::Unknown(WaitError::Probe(error)),
    }

    let mut status = 0_i32;
    let result = loop {
        let result = unsafe {
            libc::waitpid(
                shared.pid as libc::pid_t,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED,
            )
        };
        if result < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break result;
    };

    if result == shared.pid as libc::pid_t {
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            let status = ExitStatus::from_raw(status);
            cache_terminal(shared, TerminalOutcome::Reaped(status.clone()));
            return PollOutcome::Reaped(status);
        }
        // WIFSTOPPED/WIFCONTINUED are observations, not terminal status.
        #[cfg(test)]
        if libc::WIFSTOPPED(status) {
            shared.observed_stop.store(true, Ordering::Release);
        }
        return PollOutcome::Pending;
    }
    if result == 0 {
        return PollOutcome::Pending;
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::ECHILD) {
        return PollOutcome::Unknown(WaitError::Wait(error));
    }

    match ProcessIncarnation::observe_with_parent(shared.pid) {
        Ok(details) if matches!(details.observation, ProcessObservation::Absent) => {
            cache_terminal(shared, TerminalOutcome::GoneWithoutStatus);
            PollOutcome::GoneWithoutStatus
        }
        Ok(details) if matches!(details.observation, ProcessObservation::Live(actual) if actual == shared.incarnation) => {
            PollOutcome::OwnershipUnavailableLive
        }
        Ok(details) if matches!(details.observation, ProcessObservation::Zombie(actual) if actual == shared.incarnation) => {
            PollOutcome::TerminalAwaitingReap
        }
        Ok(_) => {
            cache_terminal(shared, TerminalOutcome::GoneWithoutStatus);
            PollOutcome::GoneWithoutStatus
        }
        Err(error) => PollOutcome::Unknown(WaitError::Probe(error)),
    }
}

fn cache_terminal(shared: &Shared, outcome: TerminalOutcome) {
    let mut cached = shared
        .terminal_outcome
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cached.is_none() {
        *cached = Some(outcome);
        shared.terminal.store(true, Ordering::Release);
    }
}

fn settle_callback(shared: &Shared) {
    let callback = shared
        .callback
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(callback) = callback {
        callback();
    }
}

fn quarantine(shared: Arc<Shared>) {
    loop {
        if shared.terminal.load(Ordering::Acquire) {
            return;
        }
        match poll_shared(&shared) {
            PollOutcome::Reaped(_) | PollOutcome::GoneWithoutStatus => {
                settle_callback(&shared);
                return;
            }
            PollOutcome::Pending
            | PollOutcome::OwnershipUnavailableLive
            | PollOutcome::TerminalAwaitingReap
            | PollOutcome::Unknown(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_incarnation::ProcessIncarnation;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn child_owner(args: &[&str], callback_count: &Arc<AtomicUsize>) -> ProcessWaitOwner {
        let child = Command::new(args[0]).args(&args[1..]).spawn().unwrap();
        let incarnation = ProcessIncarnation::read(child.id()).unwrap();
        let callback_count = Arc::clone(callback_count);
        ProcessWaitOwner::from_child(
            child,
            incarnation,
            Box::new(move || {
                callback_count.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    fn wait_terminal(owner: &mut ProcessWaitOwner) -> PollOutcome {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let outcome = owner.poll();
            if matches!(
                outcome,
                PollOutcome::Reaped(_) | PollOutcome::GoneWithoutStatus
            ) {
                return outcome;
            }
            assert!(
                Instant::now() < deadline,
                "owner did not settle: {outcome:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn actual_short_lived_child_is_reaped_and_callback_is_once() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let mut owner = child_owner(&["/bin/sh", "-c", "exit 7"], &callbacks);
        let outcome = wait_terminal(&mut owner);
        match outcome {
            PollOutcome::Reaped(status) => assert_eq!(status.code(), Some(7)),
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert!(matches!(owner.poll(), PollOutcome::Reaped(_)));
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancellation_signal_is_incarnation_scoped() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let mut owner = child_owner(&["/bin/sleep", "60"], &callbacks);
        assert!(owner.kill_if_exact_live().unwrap());
        assert!(matches!(wait_terminal(&mut owner), PollOutcome::Reaped(_)));
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mismatched_birth_is_gone_without_consuming_numeric_pid_status() {
        let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        drop(child);
        let callbacks = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&callbacks);
        let mut owner = ProcessWaitOwner::from_pid(
            pid,
            ProcessIncarnation::fixture(u64::MAX),
            Box::new(move || {
                callback_count.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert!(matches!(owner.poll(), PollOutcome::GoneWithoutStatus));
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, 0) },
            pid as i32
        );
    }

    #[test]
    fn stop_status_is_pending_not_reaped() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let mut owner = child_owner(&["/bin/sleep", "60"], &callbacks);
        assert_eq!(unsafe { libc::kill(owner.pid() as i32, libc::SIGSTOP) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(matches!(owner.poll(), PollOutcome::Pending));
            if owner.shared.observed_stop.load(Ordering::Acquire) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert_eq!(unsafe { libc::kill(owner.pid() as i32, libc::SIGCONT) }, 0);
        owner.kill_if_exact_live().unwrap();
        assert!(matches!(wait_terminal(&mut owner), PollOutcome::Reaped(_)));
    }

    #[test]
    fn original_zombie_owned_by_this_parent_is_actually_reaped() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let mut owner = child_owner(&["/bin/sleep", "60"], &callbacks);
        assert!(owner.kill_if_exact_live().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let details = ProcessIncarnation::observe_with_parent(owner.pid()).unwrap();
            if matches!(details.observation, ProcessObservation::Zombie(_)) {
                assert_eq!(details.parent_pid, std::process::id());
                break;
            }
            assert!(Instant::now() < deadline, "child did not become a zombie");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(matches!(owner.poll(), PollOutcome::Reaped(_)));
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert!(matches!(
            ProcessIncarnation::observe(owner.pid()).unwrap(),
            ProcessObservation::Absent
        ));
    }

    #[test]
    fn additional_completion_survives_drop_and_runs_once() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let mut owner = child_owner(&["/bin/sleep", "60"], &callbacks);
        let appended = Arc::clone(&callbacks);
        owner.add_completion(Box::new(move || {
            assert_eq!(appended.fetch_add(1, Ordering::SeqCst), 1);
        }));
        assert!(owner.kill_if_exact_live().unwrap());
        drop(owner);
        let deadline = Instant::now() + Duration::from_secs(5);
        while callbacks.load(Ordering::SeqCst) != 2 {
            assert!(Instant::now() < deadline, "appended completion was lost");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
