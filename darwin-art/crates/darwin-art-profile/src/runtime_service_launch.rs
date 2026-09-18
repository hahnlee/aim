//! Owned Darwin child supervision for the runtime service startup protocol.
//! Android service owners, not this worker, publish readiness.
use crate::process_incarnation::ProcessIncarnation;
use crate::process_wait::{PollOutcome, ProcessWaitOwner};
use crate::runtime_service_endpoints::RuntimeEndpoints;
use crate::runtime_service_protocol::StartRuntimeRequest;
use crate::runtime_service_state::RuntimeInstance;
use crate::{PROFILE_SOCKET_ENV, ProfileError, process_command::prepare_command};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const INSTANCE_TOKEN_ENV: &str = "DARWIN_ART_RUNTIME_INSTANCE_TOKEN";
type ExitCallback = Box<dyn FnOnce() + Send>;

struct OwnedChild {
    waiter: ProcessWaitOwner,
    instance: Arc<RuntimeInstance>,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // If this owner is dropped before a terminal outcome, its Drop keeps
        // the exact callback/identity in a quarantine watcher. No lease is
        // released here; ProcessWaitOwner settles only Reaped/Gone.
    }
}

impl OwnedChild {
    fn cancel_and_retain_wait(&self) {
        // The launch policy requests cancellation; the host owner decides
        // whether the exact captured incarnation may be signalled.  If wait
        // ownership was lost or probing is unknown, quarantine keeps the
        // callback and reservation instead of guessing terminal state.
        let _ = self.waiter.kill_if_exact_live();
    }
}

pub(crate) fn launch(
    instance: Arc<RuntimeInstance>,
    request: &StartRuntimeRequest,
    working_directory: &Path,
    profile_socket: &Path,
    register: impl FnOnce(u32) -> Result<ExitCallback, ProfileError>,
) -> Result<(), ProfileError> {
    launch_with_timeout(
        instance,
        request,
        working_directory,
        profile_socket,
        register,
        START_TIMEOUT,
    )
}

fn launch_with_timeout(
    instance: Arc<RuntimeInstance>,
    request: &StartRuntimeRequest,
    working_directory: &Path,
    profile_socket: &Path,
    register: impl FnOnce(u32) -> Result<ExitCallback, ProfileError>,
    timeout: Duration,
) -> Result<(), ProfileError> {
    let mut command =
        match prepare_command(&request.arguments, &request.environment, working_directory) {
            Ok(command) => command,
            Err(error) => {
                let _ = instance.fail(error.to_string());
                let _ = instance.exited();
                return Err(error);
            }
        };
    let endpoints = match RuntimeEndpoints::resolve(&request.environment, instance.token()) {
        Ok(endpoints) => endpoints,
        Err(error) => {
            let _ = instance.fail(error.to_string());
            let _ = instance.exited();
            return Err(error.into());
        }
    };
    if let Some(endpoints) = endpoints {
        for (name, value) in endpoints.environment() {
            command.env(name, value);
        }
    }
    let token = instance
        .token()
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    command
        .env(INSTANCE_TOKEN_ENV, token)
        .env(PROFILE_SOCKET_ENV, profile_socket);
    let gate = match crate::process_start_gate::StartGate::prepare(&mut command) {
        Ok(gate) => gate,
        Err(error) => {
            let _ = instance.fail(error.to_string());
            let _ = instance.exited();
            return Err(error.into());
        }
    };
    let child = match crate::spawn_owned(&mut command) {
        Ok(child) => child,
        Err(error) => {
            let _ = instance.fail(error.to_string());
            let _ = instance.exited();
            return Err(error.into());
        }
    };
    let pid = child.id();
    let incarnation = match ProcessIncarnation::read(pid) {
        Ok(incarnation) => incarnation,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            let _ = instance.fail(error.to_string());
            return Err(error);
        }
    };
    let callback = match register(pid) {
        Ok(callback) => callback,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            let _ = instance.fail(error.to_string());
            return Err(error);
        }
    };
    let callback_instance = Arc::clone(&instance);
    let waiter = ProcessWaitOwner::from_child(
        child,
        incarnation,
        Box::new(move || {
            callback();
            let _ = callback_instance.exited();
        }),
    );
    let mut owned = OwnedChild {
        waiter,
        instance: Arc::clone(&instance),
    };
    let registration = instance.attach(pid, incarnation);
    if let Err(error) = registration {
        let _ = instance.fail(error.to_string());
        owned.cancel_and_retain_wait();
        return Err(error); // the owner quarantines before permitting callback reuse.
    }
    if let Err(error) = gate.release() {
        let _ = instance.fail(error.to_string());
        owned.cancel_and_retain_wait();
        return Err(error.into()); // OwnedChild reaps before reservation reuse.
    }
    let transfer = Arc::new(std::sync::Mutex::new(Some(owned)));
    let worker_transfer = Arc::clone(&transfer);
    let result = std::thread::Builder::new()
        .name("runtime-service-child".into())
        .spawn(move || {
            let mut owned = worker_transfer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .expect("runtime-service wait owner transfers once");
            let deadline = Instant::now() + timeout;
            loop {
                match owned.waiter.poll() {
                    PollOutcome::Reaped(status) => {
                        let _ = owned
                            .instance
                            .fail(format!("runtime child exited: {status}"));
                        return;
                    }
                    PollOutcome::GoneWithoutStatus => return,
                    PollOutcome::Pending
                    | PollOutcome::OwnershipUnavailableLive
                    | PollOutcome::TerminalAwaitingReap
                    | PollOutcome::Unknown(_) => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                let snapshot = match owned.instance.snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(_) => {
                        owned.cancel_and_retain_wait();
                        return;
                    }
                };
                if snapshot.is_failed || snapshot.is_exited {
                    owned.cancel_and_retain_wait();
                    return;
                }
                if !snapshot.is_ready && Instant::now() >= deadline {
                    let _ = owned.instance.fail("runtime service readiness timed out");
                    owned.cancel_and_retain_wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    if let Err(error) = result {
        let _ = instance.fail(error.to_string());
        if let Some(owned) = transfer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            owned.cancel_and_retain_wait();
            drop(owned);
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "runtime_service_launch_tests.rs"]
mod tests;
