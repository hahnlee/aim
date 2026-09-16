//! Client for the system-authority-only bound-service launch operation.

use crate::bound_service_process::{BoundServiceProcessRequest, BoundServiceProcessResponse};
use crate::{ProfileError, protocol};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub fn start_bound_service_process(
    socket: &Path,
    request: &BoundServiceProcessRequest,
) -> Result<BoundServiceProcessResponse, ProfileError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let payload = request.encode()?;
    protocol::write_request(&mut stream, protocol::OP_START_BOUND_SERVICE, &payload)?;
    let response = protocol::expect_ok(&mut stream, protocol::OP_START_BOUND_SERVICE)?;
    BoundServiceProcessResponse::decode(&response)
}

/// Release the daemon's startup gate after system_server has committed its
/// Java-side process reservation. The handle is one-shot and incarnation-safe.
pub fn activate_bound_service_process(
    socket: &Path,
    handle: BoundServiceProcessResponse,
) -> Result<(), ProfileError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let payload = handle.encode()?;
    protocol::write_request(&mut stream, protocol::OP_ACTIVATE_BOUND_SERVICE, &payload)?;
    let response = protocol::expect_ok(&mut stream, protocol::OP_ACTIVATE_BOUND_SERVICE)?;
    if response.is_empty() {
        Ok(())
    } else {
        Err(ProfileError::InvalidResponse(
            "unexpected bound-service activation payload".into(),
        ))
    }
}
