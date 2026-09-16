//! Android write-command inspection and complete sender-memory capture for a
//! typed authority target. Unix transport and request waiting do not belong
//! here.

use super::*;

pub struct RemoteCommand {
    target: crate::authority_protocol::NodeToken,
    snapshot: crate::transaction_snapshot::TransactionSnapshot,
    extra: Vec<u8>,
    objects: Vec<crate::remote_objects::OutboundObject>,
    code: u32,
    flags: u32,
    bytes: usize,
}

pub struct RemoteReplyCommand {
    call: crate::authority_protocol::CallToken,
    submission: crate::thread::RemoteReplySubmission,
    snapshot: crate::transaction_snapshot::TransactionSnapshot,
    extra: Vec<u8>,
    objects: Vec<crate::remote_objects::OutboundObject>,
    code: u32,
    flags: u32,
    bytes: usize,
}

#[derive(Debug)]
pub enum RemoteCommandError<E> {
    Closed,
    Framing(crate::transaction_request::Error),
    Device(Error<std::convert::Infallible>),
    Capture(crate::ioctl::CaptureError<E>),
    ScatterGather(crate::scatter_gather::Error<E>),
    OutOfMemory,
    ReplyTargetRequired,
}

impl RemoteCommand {
    pub fn target(&self) -> crate::authority_protocol::NodeToken {
        self.target
    }

    pub fn snapshot(&self) -> &crate::transaction_snapshot::TransactionSnapshot {
        &self.snapshot
    }

    pub fn extra(&self) -> &[u8] {
        &self.extra
    }

    pub fn objects(&self) -> &[crate::remote_objects::OutboundObject] {
        &self.objects
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl RemoteReplyCommand {
    pub fn call(&self) -> crate::authority_protocol::CallToken {
        self.call
    }

    pub fn submission(&self) -> crate::thread::RemoteReplySubmission {
        self.submission
    }

    pub fn snapshot(&self) -> &crate::transaction_snapshot::TransactionSnapshot {
        &self.snapshot
    }

    pub fn extra(&self) -> &[u8] {
        &self.extra
    }

    pub fn objects(&self) -> &[crate::remote_objects::OutboundObject] {
        &self.objects
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl OpenConnection {
    /// Returns `None` only for a valid transaction whose target belongs to the
    /// local device. A remote result owns every sender-memory byte needed after
    /// this call; no binder_proc/session lock survives into transport.
    pub fn capture_remote_command<M: ClientMemory>(
        &self,
        input: &[u8],
        memory: &mut M,
    ) -> Result<Option<RemoteCommand>, RemoteCommandError<M::Error>> {
        self.key.as_ref().ok_or(RemoteCommandError::Closed)?;
        let (request, bytes) =
            crate::transaction_request::decode(input).map_err(RemoteCommandError::Framing)?;
        let Some(target) = self
            .resolve_remote_transaction_target(&request)
            .map_err(RemoteCommandError::Device)?
        else {
            return Ok(None);
        };
        let (snapshot, extra) = capture_payload(&request, memory)?;
        let objects = self
            .resolve_remote_objects(&snapshot)
            .map_err(RemoteCommandError::Device)?;
        Ok(Some(RemoteCommand {
            target,
            snapshot,
            extra,
            objects,
            code: request.code(),
            flags: request.flags(),
            bytes,
        }))
    }

    pub fn capture_remote_reply_command<M: ClientMemory>(
        &self,
        thread_id: u64,
        input: &[u8],
        memory: &mut M,
    ) -> Result<Option<RemoteReplyCommand>, RemoteCommandError<M::Error>> {
        let (request, bytes) =
            crate::transaction_request::decode(input).map_err(RemoteCommandError::Framing)?;
        if request.target() != crate::transaction_request::Target::Reply {
            return Err(RemoteCommandError::ReplyTargetRequired);
        }
        let Some((submission, call)) = self
            .begin_remote_reply(thread_id)
            .map_err(RemoteCommandError::Device)?
        else {
            return Ok(None);
        };
        let captured = capture_payload(&request, memory);
        let (snapshot, extra) = match captured {
            Ok(captured) => captured,
            Err(error) => {
                let _ = self.abort_remote_reply(thread_id, submission, call);
                return Err(error);
            }
        };
        let objects = match self.resolve_remote_objects(&snapshot) {
            Ok(objects) => objects,
            Err(error) => {
                let _ = self.abort_remote_reply(thread_id, submission, call);
                return Err(RemoteCommandError::Device(error));
            }
        };
        Ok(Some(RemoteReplyCommand {
            call,
            submission,
            snapshot,
            extra,
            objects,
            code: request.code(),
            flags: request.flags(),
            bytes,
        }))
    }
}

fn capture_payload<M: ClientMemory>(
    request: &crate::transaction_request::Request,
    memory: &mut M,
) -> Result<(crate::transaction_snapshot::TransactionSnapshot, Vec<u8>), RemoteCommandError<M::Error>>
{
    let snapshot =
        crate::ioctl::capture_transaction(request, memory).map_err(RemoteCommandError::Capture)?;
    let capture =
        crate::scatter_gather::ScatterGather::capture_with(&snapshot, |address, destination| {
            memory.copy_from(address, destination)
        })
        .map_err(RemoteCommandError::ScatterGather)?;
    let mut extra = Vec::new();
    extra
        .try_reserve_exact(snapshot.layout().extra().len())
        .map_err(|_| RemoteCommandError::OutOfMemory)?;
    extra.resize(snapshot.layout().extra().len(), 0);
    let base = snapshot.layout().extra().start;
    for buffer in capture.buffers() {
        let start = buffer.destination().start - base;
        extra[start..start + buffer.bytes().len()].copy_from_slice(buffer.bytes());
    }
    drop(capture);
    Ok((snapshot, extra))
}
