//! Owned Darwin child supervision for the runtime service startup protocol.
//! Android service owners, not this worker, publish readiness.
use crate::process_incarnation::ProcessIncarnation;
use crate::runtime_service_endpoints::RuntimeEndpoints;
use crate::runtime_service_protocol::StartRuntimeRequest;
use crate::runtime_service_state::RuntimeInstance;
use crate::{PROFILE_SOCKET_ENV, ProfileError, process_command::prepare_command};
use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const INSTANCE_TOKEN_ENV: &str = "DARWIN_ART_RUNTIME_INSTANCE_TOKEN";
type ExitCallback = Box<dyn FnOnce() + Send>;

struct OwnedChild {
    child: Child,
    instance: Arc<RuntimeInstance>,
    on_exit: Option<ExitCallback>,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Covers timeout, spawn-worker failure and unwinding. Only this Child
        // handle is signaled, never a package/PID search or another instance.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            if let Err(error) = self.child.wait() {
                eprintln!("darwin-artd: runtime child reap failed: {error}");
                // Do not publish exit/allow replacement without proof of reap.
                let _ = self
                    .instance
                    .fail(format!("runtime child reap failed: {error}"));
                return;
            }
        }
        if let Some(on_exit) = self.on_exit.take() {
            on_exit();
        }
        let _ = self.instance.exited();
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
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = instance.fail(error.to_string());
            let _ = instance.exited();
            return Err(error.into());
        }
    };
    let mut owned = OwnedChild {
        child,
        instance: Arc::clone(&instance),
        on_exit: None,
    };
    let pid = owned.child.id();
    let registration = (|| {
        owned.on_exit = Some(register(pid)?);
        instance.attach(pid, ProcessIncarnation::read(pid)?)
    })();
    if let Err(error) = registration {
        let _ = instance.fail(error.to_string());
        return Err(error); // owned Drop kills/reaps before permitting retry.
    }
    if let Err(error) = gate.release() {
        let _ = instance.fail(error.to_string());
        return Err(error.into()); // OwnedChild reaps before reservation reuse.
    }
    std::thread::Builder::new()
        .name("runtime-service-child".into())
        .spawn(move || {
            let deadline = Instant::now() + timeout;
            loop {
                match owned.child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = owned
                            .instance
                            .fail(format!("runtime child exited: {status}"));
                        return;
                    }
                    Err(error) => {
                        let _ = owned
                            .instance
                            .fail(format!("runtime child wait failed: {error}"));
                        return;
                    }
                    Ok(None) => {}
                }
                let snapshot = match owned.instance.snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(_) => return,
                };
                if snapshot.is_failed || snapshot.is_exited {
                    return;
                }
                if !snapshot.is_ready && Instant::now() >= deadline {
                    let _ = owned.instance.fail("runtime service readiness timed out");
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })?;
    Ok(())
}

#[cfg(test)]
#[path = "runtime_service_launch_tests.rs"]
mod tests;
