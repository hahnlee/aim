//! Profile-owned runtime service lifecycle state.
//!
//! This module owns reservation, identity and readiness state only.  It does
//! not spawn or reap processes, mutate profiles, or implement Android
//! lifecycle policy; the caller owns those operations.

use crate::ProfileError;
use crate::process_incarnation::ProcessIncarnation;
use crate::runtime_service_protocol::{
    InstanceToken, LossRequest, ReadinessMask, ReadyRequest, StartRuntimeRequest,
    StartRuntimeResponse,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const MAX_FAILURE_MESSAGE_BYTES: usize = 4096;
const READY_ATTACH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeStatus {
    Starting,
    Ready { readiness: ReadinessMask },
    Failed { message: String },
    Exited,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeInstanceData {
    status: RuntimeStatus,
    exit_failure: Option<String>,
    pid: Option<u32>,
    incarnation: Option<ProcessIncarnation>,
}

/// A deliberately small snapshot suitable for a child watchdog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeSnapshot {
    pub(crate) is_ready: bool,
    pub(crate) is_failed: bool,
    pub(crate) is_exited: bool,
    pub(crate) pid: Option<u32>,
}

/// One daemon-owned reservation and its attached child identity.
pub(crate) struct RuntimeInstance {
    key: InstanceKey,
    required: ReadinessMask,
    token: InstanceToken,
    launch_request: StartRuntimeRequest,
    data: Mutex<RuntimeInstanceData>,
    changed: Condvar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InstanceKey {
    runtime: crate::runtime_service_protocol::RuntimeKey,
    required: ReadinessMask,
}

impl RuntimeInstance {
    fn new(request: &StartRuntimeRequest) -> Result<Self, ProfileError> {
        Ok(Self {
            key: InstanceKey {
                runtime: request.key,
                required: request.required,
            },
            required: request.required,
            token: generate_token()?,
            launch_request: request.clone(),
            data: Mutex::new(RuntimeInstanceData {
                status: RuntimeStatus::Starting,
                exit_failure: None,
                pid: None,
                incarnation: None,
            }),
            changed: Condvar::new(),
        })
    }

    pub(crate) fn token(&self) -> InstanceToken {
        self.token
    }

    fn launch_request(&self) -> Result<StartRuntimeRequest, ProfileError> {
        let data = self.lock_data()?;
        if matches!(
            data.status,
            RuntimeStatus::Failed { .. } | RuntimeStatus::Exited
        ) {
            return Err(invalid("runtime launch template is no longer live"));
        }
        Ok(self.launch_request.clone())
    }

    pub(crate) fn attach(
        &self,
        pid: u32,
        incarnation: ProcessIncarnation,
    ) -> Result<(), ProfileError> {
        validate_pid(pid)?;
        let mut data = self.lock_data()?;
        match data.status {
            RuntimeStatus::Starting => {
                match (data.pid, data.incarnation) {
                    (Some(existing_pid), Some(existing_incarnation))
                        if existing_pid == pid && existing_incarnation == incarnation =>
                    {
                        return Ok(());
                    }
                    (Some(_), Some(_)) => {
                        return Err(invalid("runtime instance is attached to another process"));
                    }
                    _ => {}
                }
                data.pid = Some(pid);
                data.incarnation = Some(incarnation);
                self.changed.notify_all();
                Ok(())
            }
            RuntimeStatus::Ready { .. } => {
                if data.pid == Some(pid) && data.incarnation == Some(incarnation) {
                    Ok(())
                } else {
                    Err(invalid("cannot replace attached process identity"))
                }
            }
            RuntimeStatus::Failed { .. } => Err(invalid("runtime instance has failed")),
            RuntimeStatus::Exited => Err(invalid("runtime instance has exited")),
        }
    }

    pub(crate) fn publish_ready(
        &self,
        peer_pid: u32,
        peer_incarnation: ProcessIncarnation,
        request: ReadyRequest,
    ) -> Result<(), ProfileError> {
        let mut data = self.authenticate_peer(peer_pid, peer_incarnation, request.token)?;
        match &mut data.status {
            RuntimeStatus::Starting => {
                data.status = RuntimeStatus::Ready {
                    readiness: request.readiness,
                };
            }
            RuntimeStatus::Ready { readiness } => {
                *readiness = readiness.union(request.readiness);
            }
            RuntimeStatus::Failed { .. } => {
                return Err(invalid("runtime instance has failed"));
            }
            RuntimeStatus::Exited => {
                return Err(invalid("runtime instance has exited"));
            }
        }
        self.changed.notify_all();
        Ok(())
    }

    fn authenticate_peer(
        &self,
        peer_pid: u32,
        peer_incarnation: ProcessIncarnation,
        token: InstanceToken,
    ) -> Result<MutexGuard<'_, RuntimeInstanceData>, ProfileError> {
        validate_pid(peer_pid)?;
        if token != self.token {
            return Err(invalid("runtime readiness token mismatch"));
        }
        let mut data = self.lock_data()?;
        let deadline = Instant::now() + READY_ATTACH_TIMEOUT;
        while data.pid.is_none() || data.incarnation.is_none() {
            if matches!(
                data.status,
                RuntimeStatus::Failed { .. } | RuntimeStatus::Exited
            ) {
                return Err(invalid("runtime instance is no longer starting"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(invalid("runtime readiness arrived before process attach"));
            }
            let (next, result) = self
                .changed
                .wait_timeout(data, remaining)
                .map_err(|_| invalid("runtime state lock poisoned"))?;
            data = next;
            if result.timed_out() && (data.pid.is_none() || data.incarnation.is_none()) {
                return Err(invalid("runtime readiness arrived before process attach"));
            }
        }
        if data.pid != Some(peer_pid) || data.incarnation != Some(peer_incarnation) {
            return Err(invalid("runtime readiness process identity mismatch"));
        }
        Ok(data)
    }

    pub(crate) fn fail(&self, message: impl Into<String>) -> Result<(), ProfileError> {
        let message = message.into();
        if message.is_empty() || message.len() > MAX_FAILURE_MESSAGE_BYTES {
            return Err(invalid("runtime failure message is empty or too large"));
        }
        let mut data = self.lock_data()?;
        match data.status {
            RuntimeStatus::Starting | RuntimeStatus::Ready { .. } => {
                data.status = RuntimeStatus::Failed { message };
                self.changed.notify_all();
                Ok(())
            }
            RuntimeStatus::Failed { .. } => Ok(()),
            RuntimeStatus::Exited => Err(invalid("runtime instance has exited")),
        }
    }

    /// Record loss of a child-owned transport capability.  Authentication is
    /// deliberately the same as startup readiness: token plus the daemon's
    /// peer PID/incarnation, never caller-supplied identity fields.
    pub(crate) fn publish_runtime_lost(
        &self,
        peer_pid: u32,
        peer_incarnation: ProcessIncarnation,
        request: LossRequest,
    ) -> Result<(), ProfileError> {
        let mut data = self.authenticate_peer(peer_pid, peer_incarnation, request.token)?;
        match data.status {
            RuntimeStatus::Starting | RuntimeStatus::Ready { .. } => {
                if self.required.bits() & request.lost.bits() != 0 {
                    data.status = RuntimeStatus::Failed {
                        message: format!(
                            "runtime required readiness lost (mask=0x{:x})",
                            request.lost.bits()
                        ),
                    };
                    self.changed.notify_all();
                }
                Ok(())
            }
            RuntimeStatus::Failed { .. } => Ok(()),
            RuntimeStatus::Exited => Err(invalid("runtime instance has exited")),
        }
    }

    pub(crate) fn exited(&self) -> Result<(), ProfileError> {
        let mut data = self.lock_data()?;
        if !matches!(data.status, RuntimeStatus::Exited) {
            // Reaping permits replacement, but must not erase the reason a
            // startup waiter failed before it can reacquire this lock.
            data.exit_failure = match &data.status {
                RuntimeStatus::Failed { message } => Some(message.clone()),
                _ => None,
            };
            data.status = RuntimeStatus::Exited;
            self.changed.notify_all();
        }
        Ok(())
    }

    pub(crate) fn wait_ready(
        &self,
        timeout: Duration,
    ) -> Result<StartRuntimeResponse, ProfileError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(60 * 60 * 24 * 365 * 100));
        let mut data = self.lock_data()?;
        loop {
            match &data.status {
                RuntimeStatus::Ready { readiness } if readiness.contains(self.required) => {
                    let pid = data
                        .pid
                        .ok_or_else(|| invalid("ready runtime has no attached PID"))?;
                    let incarnation = data
                        .incarnation
                        .ok_or_else(|| invalid("ready runtime has no process identity"))?;
                    let current = ProcessIncarnation::read(pid)?;
                    if current != incarnation {
                        return Err(invalid("ready runtime process identity changed"));
                    }
                    return Ok(StartRuntimeResponse {
                        pid,
                        token: self.token,
                    });
                }
                RuntimeStatus::Failed { message } => {
                    return Err(ProfileError::Daemon(message.clone()));
                }
                RuntimeStatus::Exited => {
                    return Err(match &data.exit_failure {
                        Some(message) => ProfileError::Daemon(message.clone()),
                        None => invalid("runtime instance exited"),
                    });
                }
                RuntimeStatus::Starting | RuntimeStatus::Ready { .. } => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(invalid("runtime readiness timed out"));
            }
            let (next, result) = self
                .changed
                .wait_timeout(data, remaining)
                .map_err(|_| invalid("runtime state lock poisoned"))?;
            data = next;
            if result.timed_out() {
                return Err(invalid("runtime readiness timed out"));
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Result<RuntimeSnapshot, ProfileError> {
        let data = self.lock_data()?;
        let is_ready = matches!(&data.status, RuntimeStatus::Ready { readiness } if readiness.contains(self.required));
        Ok(RuntimeSnapshot {
            is_ready,
            is_failed: matches!(data.status, RuntimeStatus::Failed { .. }),
            is_exited: matches!(data.status, RuntimeStatus::Exited),
            pid: data.pid,
        })
    }

    pub(crate) fn wait_exited(&self, timeout: Duration) -> Result<(), ProfileError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
        let mut data = self.lock_data()?;
        while !matches!(data.status, RuntimeStatus::Exited) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(invalid("runtime generation replacement timed out"));
            }
            let (next, result) = self
                .changed
                .wait_timeout(data, remaining)
                .map_err(|_| invalid("runtime state lock poisoned"))?;
            data = next;
            if result.timed_out() && !matches!(data.status, RuntimeStatus::Exited) {
                return Err(invalid("runtime generation replacement timed out"));
            }
        }
        Ok(())
    }

    fn lock_data(&self) -> Result<MutexGuard<'_, RuntimeInstanceData>, ProfileError> {
        self.data
            .lock()
            .map_err(|_| invalid("runtime state lock poisoned"))
    }
}

/// The singleton reservation owner, keyed by Android package name.
#[derive(Default)]
pub(crate) struct RuntimeServiceState {
    instances: Mutex<BTreeMap<String, Arc<RuntimeInstance>>>,
    changed: Condvar,
}

impl RuntimeServiceState {
    pub(crate) fn conflicting_instance(
        &self,
        request: &StartRuntimeRequest,
    ) -> Result<Option<Arc<RuntimeInstance>>, ProfileError> {
        request.encode()?;
        let instances = self
            .instances
            .lock()
            .map_err(|_| invalid("runtime reservation lock poisoned"))?;
        let Some(existing) = instances.get(&request.package) else {
            return Ok(None);
        };
        let snapshot = existing.snapshot()?;
        if snapshot.is_exited
            || (existing.key.runtime == request.key && existing.key.required == request.required)
        {
            return Ok(None);
        }
        Ok(Some(Arc::clone(existing)))
    }

    pub(crate) fn reserve(
        &self,
        request: &StartRuntimeRequest,
    ) -> Result<(Arc<RuntimeInstance>, bool), ProfileError> {
        // This performs all protocol-level validation, including package,
        // argument/environment and total wire-size bounds.
        request.encode()?;
        let mut instances = self
            .instances
            .lock()
            .map_err(|_| invalid("runtime reservation lock poisoned"))?;
        if let Some(existing) = instances.get(&request.package).cloned() {
            let snapshot = existing.snapshot()?;
            if snapshot.is_exited {
                instances.remove(&request.package);
            } else if existing.key.runtime != request.key
                || existing.key.required != request.required
            {
                return Err(invalid(
                    "package already has a runtime with a different key or readiness mask",
                ));
            } else if snapshot.is_failed {
                return Err(invalid(
                    "failed runtime must be reaped and marked exited before reuse",
                ));
            } else {
                return Ok((existing, false));
            }
        }
        let instance = Arc::new(RuntimeInstance::new(request)?);
        instances.insert(request.package.clone(), instance.clone());
        self.changed.notify_all();
        Ok((instance, true))
    }

    pub(crate) fn publish_ready(
        &self,
        peer_pid: u32,
        peer_incarnation: ProcessIncarnation,
        request: ReadyRequest,
    ) -> Result<(), ProfileError> {
        let instances = self
            .instances
            .lock()
            .map_err(|_| invalid("runtime reservation lock poisoned"))?;
        let instance = instances
            .values()
            .find(|instance| instance.token() == request.token)
            .cloned()
            .ok_or_else(|| invalid("unknown runtime readiness token"))?;
        drop(instances);
        instance.publish_ready(peer_pid, peer_incarnation, request)
    }

    pub(crate) fn publish_runtime_lost(
        &self,
        peer_pid: u32,
        peer_incarnation: ProcessIncarnation,
        request: LossRequest,
    ) -> Result<(), ProfileError> {
        let instances = self
            .instances
            .lock()
            .map_err(|_| invalid("runtime reservation lock poisoned"))?;
        let instance = instances
            .values()
            .find(|instance| instance.token() == request.token)
            .cloned()
            .ok_or_else(|| invalid("unknown runtime readiness-loss token"))?;
        drop(instances);
        instance.publish_runtime_lost(peer_pid, peer_incarnation, request)
    }

    pub(crate) fn contains_package(&self, package: &str) -> bool {
        let Ok(instances) = self.instances.lock() else {
            // A poisoned gate must fail closed: permitting a second launch
            // could violate package singleton ownership.
            return true;
        };
        match instances.get(package) {
            None => false,
            Some(instance) => instance
                .snapshot()
                .map(|snapshot| !snapshot.is_exited)
                .unwrap_or(true),
        }
    }

    /// Return the daemon-owned command template for the live system process.
    ///
    /// This is deliberately not a general package lookup. Bound application
    /// processes must be derived from the single authenticated system runtime
    /// configuration instead of accepting an executable or environment from
    /// Java framework policy.
    pub(crate) fn system_launch_template(&self) -> Result<StartRuntimeRequest, ProfileError> {
        let instances = self
            .instances
            .lock()
            .map_err(|_| invalid("runtime reservation lock poisoned"))?;
        let instance = instances
            .get("android.system")
            .cloned()
            .ok_or_else(|| invalid("system runtime launch template is unavailable"))?;
        drop(instances);
        instance.launch_request()
    }
}

fn generate_token() -> Result<InstanceToken, ProfileError> {
    let mut bytes = [0_u8; 16];
    let mut random = File::open("/dev/urandom").map_err(ProfileError::Io)?;
    random.read_exact(&mut bytes).map_err(ProfileError::Io)?;
    Ok(InstanceToken(bytes))
}

fn validate_pid(pid: u32) -> Result<(), ProfileError> {
    if pid == 0 || pid > i32::MAX as u32 {
        Err(invalid("runtime process PID must be in 1..=i32::MAX"))
    } else {
        Ok(())
    }
}

fn invalid(detail: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("invalid runtime service state: {detail}"))
}

#[cfg(test)]
#[path = "runtime_service_state_tests.rs"]
mod tests;
