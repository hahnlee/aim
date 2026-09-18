//! Owned verification handoff before an Android process's OS exit boundary.
//!
//! The observer owns no VM, Binder endpoint or shutdown obligation. Callers
//! supply value-only verification; fixture assertions remain in their client.

use crate::{HostError, HostOutcome};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

/// The main actor grants completion only after its last pump returned normally.
/// Dropping a request rejects it instead of accidentally authorizing success.
pub(crate) struct ActorCompletionRequest(SyncSender<()>);
pub(crate) struct ActorCompletionWait(Receiver<()>);

pub(crate) fn actor_completion_gate() -> (ActorCompletionRequest, ActorCompletionWait) {
    let (sender, receiver) = mpsc::sync_channel(1);
    (
        ActorCompletionRequest(sender),
        ActorCompletionWait(receiver),
    )
}

impl ActorCompletionRequest {
    pub(crate) fn authorize(self) {
        let _ = self.0.send(());
    }
}

impl ActorCompletionWait {
    pub(crate) fn wait(self) -> Result<(), HostError> {
        self.0
            .recv()
            .map_err(|_| HostError::HostService("AppKit actor rejected process completion".into()))
    }
}

type CompletionCallback = dyn Fn(&HostOutcome) -> Result<(), HostError> + Send + Sync;

/// A completion observer runs on the Android owner thread before `_exit`.
/// It must finish its result handoff synchronously and must not tear down ART,
/// release process providers or retain borrowed runtime state.
#[derive(Clone)]
pub struct ProcessCompletionObserver(Arc<CompletionCallback>);

impl ProcessCompletionObserver {
    pub fn new(
        observe: impl Fn(&HostOutcome) -> Result<(), HostError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(observe))
    }

    fn verify(&self, outcome: &HostOutcome) -> Result<(), HostError> {
        (self.0)(outcome)
    }
}

/// A process/cleanup failure never invokes the successful-completion observer.
/// Missing mandatory assertions are the client's error, not an exit-code proxy.
pub(crate) fn exit_status(
    outcome: Result<&HostOutcome, &HostError>,
    cleanup: Result<(), HostError>,
    observer: Option<&ProcessCompletionObserver>,
) -> i32 {
    let outcome = match (outcome, cleanup) {
        (Ok(outcome), Ok(())) => outcome,
        (Err(error), _) => {
            eprintln!("darwin-art-host: {error}");
            return 1;
        }
        (_, Err(error)) => {
            eprintln!("darwin-art-host: {error}");
            return 1;
        }
    };
    if let Some(observer) = observer
        && let Err(error) = observer.verify(outcome)
    {
        eprintln!("darwin-art-host: completion handoff failed: {error}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn outcome() -> HostOutcome {
        HostOutcome {
            process: crate::ProcessResult::default(),
            frames_presented: 7,
            last_frame: None,
        }
    }

    #[test]
    fn successful_handoff_runs_once_and_observes_the_actual_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let observer = ProcessCompletionObserver::new(move |outcome| {
            assert_eq!(outcome.frames_presented, 7);
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(exit_status(Ok(&outcome()), Ok(()), Some(&observer)), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(exit_status(Ok(&outcome()), Ok(()), None), 0);
    }

    #[test]
    fn failed_handoff_is_not_successful_process_completion() {
        let observer = ProcessCompletionObserver::new(|_| {
            Err(HostError::HostService("missing finish ACK".into()))
        });
        assert_eq!(exit_status(Ok(&outcome()), Ok(()), Some(&observer)), 1);
    }

    #[test]
    fn runtime_and_cleanup_failure_do_not_publish_success() {
        let observer = ProcessCompletionObserver::new(|_| panic!("must not verify failure"));
        let error = HostError::RuntimeFailed(27);
        assert_eq!(exit_status(Err(&error), Ok(()), Some(&observer)), 1);
        assert_eq!(exit_status(Ok(&outcome()), Err(error), Some(&observer)), 1);
    }

    #[test]
    fn main_actor_must_explicitly_authorize_completion() {
        let (request, wait) = actor_completion_gate();
        let actor = std::thread::spawn(move || request.authorize());
        assert!(wait.wait().is_ok());
        actor.join().unwrap();
        let (request, wait) = actor_completion_gate();
        drop(request);
        assert!(wait.wait().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn result_handoff_precedes_os_exit_without_runtime_destructors() {
        const CHILD: &str = "DARWIN_ART_COMPLETION_TEST_CHILD";
        if let Some(mode) = std::env::var_os(CHILD) {
            struct MustNotDrop;
            impl Drop for MustNotDrop {
                fn drop(&mut self) {
                    eprintln!("unexpected-runtime-drop");
                    std::process::abort();
                }
            }
            let _runtime = MustNotDrop;
            let fail = mode == "observer-failure";
            let observer = ProcessCompletionObserver::new(move |_| {
                eprintln!("completion-result-handoff");
                if fail {
                    Err(HostError::HostService("assertion failed".into()))
                } else {
                    Ok(())
                }
            });
            let cleanup = if mode == "cleanup-failure" {
                Err(HostError::HostService("cleanup failed".into()))
            } else {
                Ok(())
            };
            let status = exit_status(Ok(&outcome()), cleanup, Some(&observer));
            crate::process_exit::exit_android_process(status);
        }
        for (mode, expected_status, handoff) in [
            ("success", 0, true),
            ("observer-failure", 1, true),
            ("cleanup-failure", 1, false),
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process_completion::tests::result_handoff_precedes_os_exit_without_runtime_destructors",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(expected_status), "{mode}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr.contains("completion-result-handoff"),
                handoff,
                "{mode}"
            );
            assert!(!stderr.contains("unexpected-runtime-drop"), "{mode}");
        }
    }
}
