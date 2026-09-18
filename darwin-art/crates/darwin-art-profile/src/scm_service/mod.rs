//! Private Unix descriptor-transfer ownership, not Binder or Android policy.
mod client_transport;
mod native_endpoint;
pub use native_endpoint::NativeScmEndpointProvider;
mod metadata;
mod owner;
mod participants;
mod transport;
mod wire;

use crate::{
    ProfileError,
    process_incarnation::{ProcessIncarnation, ProcessObservation},
    process_registry::ProcessRegistry,
};
use darwin_art_scm_transfer::{AuthorityEpoch, ProcessEpoch};
use std::{os::unix::net::UnixStream, sync::Mutex};

pub(crate) struct ScmService {
    owner: Mutex<owner::Owner>,
    participants: Mutex<participants::Participants>,
}

impl ScmService {
    pub(crate) fn new() -> Result<Self, ProfileError> {
        let mut bytes = [0u8; 16];
        if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            participants: Default::default(),
            owner: Mutex::new(owner::Owner::new(AuthorityEpoch {
                instance: u128::from_ne_bytes(bytes),
            })?),
        })
    }

    pub(crate) fn serve(
        &self,
        processes: &Mutex<ProcessRegistry>,
        stream: &mut UnixStream,
        request: &[u8],
    ) -> Result<(), ProfileError> {
        // Audit-token/version verification binds this actual connection to the
        // live kernel birth, then registry membership binds it to our runtime.
        // Neither PID nor ProcessEpoch is accepted from the private wire.
        let (pid, birth) = crate::peer_process::identity(stream)?;
        if processes
            .lock()
            .map_err(|_| failed("process registry poisoned"))?
            .package(pid, birth)
            .is_none()
        {
            return Err(failed("peer is not a registered runtime process"));
        }
        let epoch = process_epoch(pid, birth);
        self.participants
            .lock()
            .map_err(|_| failed("participants poisoned"))?
            .track(epoch, birth)?;
        transport::serve(&self.owner, epoch, stream, request)
    }

    pub(crate) fn has_pending(&self) -> Result<bool, ProfileError> {
        let snapshot = self
            .participants
            .lock()
            .map_err(|_| failed("participants poisoned"))?
            .snapshot();
        // Observe outside both mutexes. Only kernel absence, zombie/reuse with
        // this exact cached birth proves death. Socket EOF never does.
        for participant in snapshot.into_iter().flatten() {
            let dead = match ProcessIncarnation::observe(participant.epoch.pid)? {
                ProcessObservation::Absent => true,
                ProcessObservation::Zombie(_) => true,
                ProcessObservation::Live(current) => current != participant.birth,
            };
            if dead {
                self.process_died(participant.epoch.pid, participant.birth)?;
            }
        }
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        // Open Description grants still require this authority even with no
        // queued transfer. A reconnectable live holder is not an idle daemon.
        Ok(owner.has_pending()? || owner.has_authority_refs())
    }

    pub(crate) fn process_died(
        &self,
        pid: u32,
        birth: ProcessIncarnation,
    ) -> Result<(), ProfileError> {
        let result = self
            .owner
            .lock()
            .map_err(|_| failed("owner poisoned"))?
            .process_died(process_epoch(pid, birth));
        self.participants
            .lock()
            .map_err(|_| failed("participants poisoned"))?
            .remove(process_epoch(pid, birth));
        result
    }
}

fn process_epoch(pid: u32, birth: ProcessIncarnation) -> ProcessEpoch {
    let [seconds, microseconds] = birth.parts();
    ProcessEpoch {
        pid,
        instance: ((seconds as u128) << 64) | microseconds as u128,
    }
}

fn failed(reason: impl std::fmt::Display) -> ProfileError {
    ProfileError::Daemon(format!("SCM service: {reason}"))
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
