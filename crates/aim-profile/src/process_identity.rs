use crate::{ProfileError, registry::validate_package};

/// Profile-daemon process registration plus the package-manager UID (user 0).
/// This is not a substitute for verifying a transport's actual peer PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub uid: u32,
    pub package: String,
}

impl ProcessIdentity {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = self.pid.to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.uid.to_le_bytes());
        bytes.extend_from_slice(self.package.as_bytes());
        bytes
    }

    pub(crate) fn decode(bytes: &[u8], expected_pid: u32) -> Result<Self, ProfileError> {
        let invalid = || ProfileError::Daemon("invalid process identity response".into());
        if bytes.len() < 9 || bytes.len() > 263 {
            return Err(invalid());
        }
        let pid = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let uid = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let package = std::str::from_utf8(&bytes[8..]).map_err(|_| invalid())?;
        validate_package(package)?;
        let valid_uid = if package == "android.system" {
            uid == 1000
        } else {
            (10000..=19999).contains(&uid) || (99000..=99999).contains(&uid)
        };
        if pid == 0 || pid != expected_pid || !valid_uid {
            return Err(invalid());
        }
        Ok(Self {
            pid,
            uid,
            package: package.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_query_uses_typed_protocol_and_rejects_privileged_reply() {
        use std::os::unix::net::UnixListener;
        for uid in [10042, 1000] {
            let path = std::env::temp_dir().join(format!(
                "darwin-uid-query-{}-{uid}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let listener = UnixListener::bind(&path).unwrap();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let message = crate::protocol::read_message(&mut stream).unwrap();
                assert_eq!(message.operation, crate::protocol::OP_PROCESS_IDENTITY);
                assert_eq!(message.payload, 123u32.to_le_bytes());
                let identity = ProcessIdentity {
                    pid: 123,
                    uid,
                    package: "org.example.app".into(),
                };
                crate::protocol::write_response(
                    &mut stream,
                    message.operation,
                    0,
                    &identity.encode(),
                )
                .unwrap();
            });
            let result = crate::resolve_process_identity_at(&path, 123);
            worker.join().unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(result.is_ok(), uid == 10042);
        }
    }
    #[test]
    fn identity_roundtrip_and_rejects_mismatched_pid_or_uid() {
        let identity = ProcessIdentity {
            pid: 123,
            uid: 10042,
            package: "org.example.app".into(),
        };
        assert_eq!(
            ProcessIdentity::decode(&identity.encode(), 123).unwrap(),
            identity
        );
        assert!(ProcessIdentity::decode(&identity.encode(), 124).is_err());
        let privileged = ProcessIdentity {
            uid: 1000,
            ..identity.clone()
        };
        assert!(ProcessIdentity::decode(&privileged.encode(), 123).is_err());
        assert!(ProcessIdentity::decode(&[0; 8], 123).is_err());
        let isolated = ProcessIdentity {
            uid: 99042,
            ..identity
        };
        assert_eq!(
            ProcessIdentity::decode(&isolated.encode(), 123).unwrap(),
            isolated
        );
    }
}
