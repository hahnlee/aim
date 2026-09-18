//! Typed profile transport for runtime-instance startup and readiness.
use crate::runtime_service_protocol::{
    LossRequest, ReadyRequest, StartRuntimeRequest, StartRuntimeResponse,
};
use crate::unix_connect::connect_with_timeout;
use crate::{ProfileError, protocol};
use std::path::Path;
use std::time::Duration;

pub fn start_runtime_service(
    socket: &Path,
    request: &StartRuntimeRequest,
) -> Result<StartRuntimeResponse, ProfileError> {
    let response = exchange(
        socket,
        protocol::OP_START_RUNTIME,
        &request.encode()?,
        Duration::from_secs(40),
    )?;
    StartRuntimeResponse::decode(&response)
}

/// Must be called by the actual child after its service owners are ready.
/// No claimed PID/key is sent: the daemon authenticates the socket's peer.
pub fn publish_runtime_ready(socket: &Path, request: ReadyRequest) -> Result<(), ProfileError> {
    let response = exchange(
        socket,
        protocol::OP_RUNTIME_READY,
        &request.encode()?,
        Duration::from_secs(5),
    )?;
    if !response.is_empty() {
        return Err(ProfileError::InvalidResponse(
            "unexpected readiness response payload".into(),
        ));
    }
    Ok(())
}

/// Must be called by an actual child when an advertised service transport is
/// lost while the process remains alive.  The daemon authenticates the
/// socket peer and the opaque token; no PID or runtime key is caller-supplied.
pub fn publish_runtime_lost(socket: &Path, request: LossRequest) -> Result<(), ProfileError> {
    let response = exchange(
        socket,
        protocol::OP_RUNTIME_LOST,
        &request.encode()?,
        Duration::from_secs(5),
    )?;
    if !response.is_empty() {
        return Err(ProfileError::InvalidResponse(
            "unexpected readiness-loss response payload".into(),
        ));
    }
    Ok(())
}

fn exchange(
    socket: &Path,
    operation: u16,
    payload: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, ProfileError> {
    let mut stream = connect_with_timeout(socket, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    protocol::write_request(&mut stream, operation, payload)?;
    protocol::expect_ok(&mut stream, operation).map_err(Into::into)
}
