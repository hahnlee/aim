//! Bounded daemon requests (installd, property service) for callers that
//! already hold a socket.
//!
//! This module deliberately does not select a profile, consult environment
//! state, or start a daemon.  The socket path is an explicit capability owned
//! by the caller.

use crate::{ProfileError, protocol};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub use crate::installd::Request as InstalldRequest;

const INSTALLD_TIMEOUT: Duration = Duration::from_secs(30);

/// Run one installd request through the profile daemon. `Ok` carries the
/// operation's reply; `Err((errno, message))` is installd's Android errno.
pub fn installd_at(
    socket: &Path,
    request: &InstalldRequest,
) -> Result<Result<Vec<u8>, (u32, String)>, ProfileError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(INSTALLD_TIMEOUT))?;
    stream.set_write_timeout(Some(INSTALLD_TIMEOUT))?;
    protocol::write_request(&mut stream, protocol::OP_INSTALLD, &request.encode())?;
    let message = protocol::read_message(&mut stream)?;
    if message.operation != protocol::OP_INSTALLD | protocol::RESPONSE_BIT
        || message.payload.len() < 4
    {
        return Err(ProfileError::InvalidResponse(
            "bad installd response envelope".into(),
        ));
    }
    let status = u32::from_le_bytes(message.payload[..4].try_into().unwrap());
    let payload = message.payload[4..].to_vec();
    Ok(if status == 0 {
        Ok(payload)
    } else {
        Err((status, String::from_utf8_lossy(&payload).into_owned()))
    })
}

/// Asks the profile's property service to set `name`; returns its
/// system_properties.h result (0 on success, PROP_ERROR_* otherwise).
pub fn property_set_at(socket: &Path, name: &str, value: &str) -> Result<u32, ProfileError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(INSTALLD_TIMEOUT))?;
    stream.set_write_timeout(Some(INSTALLD_TIMEOUT))?;
    protocol::write_request(
        &mut stream,
        protocol::OP_PROPERTY_SET,
        &crate::property_service::encode_request(name, value),
    )?;
    let message = protocol::read_message(&mut stream)?;
    if message.operation != protocol::OP_PROPERTY_SET | protocol::RESPONSE_BIT
        || message.payload.len() < 4
    {
        return Err(ProfileError::InvalidResponse(
            "bad property response envelope".into(),
        ));
    }
    Ok(u32::from_le_bytes(message.payload[..4].try_into().unwrap()))
}
