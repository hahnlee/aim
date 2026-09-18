//! Bounded package-registry transport for callers that already hold a socket.
//!
//! This module deliberately does not select a profile, consult environment
//! state, or start a daemon.  The socket path is an explicit capability owned
//! by the caller.

use crate::registry::validate_package;
use crate::{protocol, ProfileError};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(1);
const RECORD_VERSION: &[u8] = b"darwin-art-launch-v1\n";
const MAX_RECORD: usize = 60 * 1024;

/// Resolve one installed package through an already-running profile daemon.
///
/// `Ok(None)` means the daemon authoritatively reports that the package is not
/// installed.  Transport, framing, and malformed-record failures remain
/// errors; this function never fabricates a package record.
pub fn resolve_package_at(socket: &Path, package: &str) -> Result<Option<Vec<u8>>, ProfileError> {
    validate_package(package)?;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(RESOLVE_TIMEOUT))?;
    stream.set_write_timeout(Some(RESOLVE_TIMEOUT))?;
    protocol::write_request(&mut stream, protocol::OP_RESOLVE, package.as_bytes())?;

    let message = protocol::read_message(&mut stream)?;
    if message.operation != protocol::OP_RESOLVE | protocol::RESPONSE_BIT
        || message.payload.len() < 4
    {
        return Err(ProfileError::InvalidResponse(
            "bad package resolve response envelope".into(),
        ));
    }
    let status = u32::from_le_bytes(message.payload[..4].try_into().unwrap());
    let payload = &message.payload[4..];
    match status {
        0 => decode_record(payload).map(Some),
        protocol::STATUS_NOT_FOUND => Ok(None),
        status => {
            let detail = String::from_utf8_lossy(payload);
            Err(ProfileError::Daemon(format!(
                "package resolve failed: status={status} {detail}"
            )))
        }
    }
}

fn decode_record(record: &[u8]) -> Result<Vec<u8>, ProfileError> {
    if !record.starts_with(RECORD_VERSION)
        || record.len() > MAX_RECORD
        || record.contains(&0)
        || !record.ends_with(b"\n")
    {
        return Err(ProfileError::InvalidResponse(
            "invalid launch record framing".into(),
        ));
    }
    std::str::from_utf8(record)
        .map_err(|_| ProfileError::InvalidResponse("launch record is not UTF-8".into()))?;
    Ok(record.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    fn socket_path(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        // macOS limits a sockaddr_un path to 103 bytes. Keep the endpoint
        // under /tmp rather than extending the platform's often-long temp
        // directory prefix.
        std::path::PathBuf::from(format!(
            "/tmp/dart-pc-{label}-{}-{id}.sock",
            std::process::id()
        ))
    }

    fn accept_request(listener: &UnixListener) -> io::Result<std::os::unix::net::UnixStream> {
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                    let request = protocol::read_message(&mut stream)?;
                    if request.operation != protocol::OP_RESOLVE
                        || request.payload != b"com.example.app"
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "mock server received an unexpected resolve request",
                        ));
                    }
                    return Ok(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "mock server accept timed out",
                        ));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn serve_response(
        operation: u16,
        status: u32,
        payload: Vec<u8>,
    ) -> (std::path::PathBuf, thread::JoinHandle<()>) {
        let path = socket_path("response");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || {
            let mut stream = accept_request(&listener).unwrap();
            protocol::write_response(&mut stream, operation, status, &payload).unwrap();
        });
        (path, worker)
    }

    fn query(
        operation: u16,
        status: u32,
        payload: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, ProfileError> {
        let (path, worker) = serve_response(operation, status, payload);
        let result = resolve_package_at(&path, "com.example.app");
        worker.join().unwrap();
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn success_returns_authoritative_record() {
        let record = b"darwin-art-launch-v1\napk=/host/base.apk\n".to_vec();
        assert_eq!(
            query(protocol::OP_RESOLVE, 0, record.clone()).unwrap(),
            Some(record)
        );
    }

    #[test]
    fn missing_package_is_distinct_from_transport_error() {
        assert_eq!(
            query(
                protocol::OP_RESOLVE,
                protocol::STATUS_NOT_FOUND,
                b"not installed".to_vec()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn daemon_error_is_not_treated_as_missing() {
        assert!(query(protocol::OP_RESOLVE, 1, b"registry unavailable".to_vec()).is_err());
    }

    #[test]
    fn legacy_expect_ok_still_rejects_missing_status() {
        let mut response = Vec::new();
        protocol::write_response(
            &mut response,
            protocol::OP_RESOLVE,
            protocol::STATUS_NOT_FOUND,
            b"package is not installed",
        )
        .unwrap();
        assert!(protocol::expect_ok(&mut response.as_slice(), protocol::OP_RESOLVE).is_err());
    }

    #[test]
    fn wrong_operation_is_rejected() {
        assert!(query(protocol::OP_STATUS, 0, b"darwin-art-launch-v1\n".to_vec()).is_err());
    }

    #[test]
    fn malformed_and_truncated_records_are_rejected() {
        for record in [
            b"truncated".to_vec(),
            b"darwin-art-launch-v1\napk=/host/base.apk\0\n".to_vec(),
            vec![b'd'; MAX_RECORD + 1],
            {
                let mut invalid_utf8 = b"darwin-art-launch-v1\napk=".to_vec();
                invalid_utf8.extend_from_slice(&[0xff, b'\n']);
                invalid_utf8
            },
        ] {
            assert!(query(protocol::OP_RESOLVE, 0, record).is_err());
        }
    }

    #[test]
    fn truncated_response_is_rejected() {
        let path = socket_path("truncated");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || {
            let mut stream = accept_request(&listener).unwrap();
            stream.write_all(b"DARTD001").unwrap();
        });
        assert!(resolve_package_at(&path, "com.example.app").is_err());
        worker.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_package_is_rejected_before_connecting() {
        assert!(resolve_package_at(Path::new("/does/not/exist"), "../escape").is_err());
    }
}
