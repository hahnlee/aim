//! Darwin process startup handshake, independent of Android service policy.
//! The profile owner issues the token; the actual service owner publishes only
//! after its transport and service state are prepared. Socket existence is not
//! a readiness signal. The profile authenticates the publishing process itself.
use darwin_art_profile::runtime_service_protocol::{
    InstanceToken, LossRequest, ReadinessMask, ReadyRequest,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

struct Publisher {
    socket: PathBuf,
    token: InstanceToken,
    pid: u32,
}

impl Publisher {
    fn from_launch(
        token: Option<OsString>,
        socket: Option<OsString>,
    ) -> Result<Option<Self>, String> {
        let Some(token) = token else {
            return Ok(None);
        };
        let token = token
            .to_str()
            .ok_or("runtime instance token is not UTF-8")?;
        if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid runtime instance token".into());
        }
        let mut bytes = [0; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&token[index * 2..index * 2 + 2], 16)
                .map_err(|_| "invalid runtime instance token")?;
        }
        let socket = PathBuf::from(socket.ok_or("runtime instance has no profile socket")?);
        if !socket.is_absolute()
            || socket.file_name().is_none()
            || socket
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err("runtime instance requires an absolute profile socket".into());
        }
        Ok(Some(Self {
            socket,
            token: InstanceToken(bytes),
            pid: std::process::id(),
        }))
    }

    fn publish(&self, readiness: ReadinessMask) -> Result<(), String> {
        if self.pid != std::process::id() {
            return Err("forked process cannot publish its parent's readiness".into());
        }
        darwin_art_profile::publish_runtime_ready(
            &self.socket,
            ReadyRequest {
                token: self.token,
                readiness,
            },
        )
        .map_err(|error| error.to_string())
    }

    fn report_lost(&self, lost: ReadinessMask) -> Result<(), String> {
        if self.pid != std::process::id() {
            return Err("forked process cannot revoke its parent's readiness".into());
        }
        darwin_art_profile::publish_runtime_lost(
            &self.socket,
            LossRequest {
                token: self.token,
                lost,
            },
        )
        .map_err(|error| error.to_string())
    }
}

/// `false` means an unmanaged standalone process: no publication took place.
/// Malformed managed launch configuration or rejected publication is an error.
pub(crate) fn publish(bits: u32) -> Result<bool, String> {
    let readiness = ReadinessMask::new(bits).map_err(|error| error.to_string())?;
    with_publisher(|publisher| publisher.publish(readiness))
}

/// Actual service-owner failure, never inferred from a pathname or a request
/// error. Uses the same captured launch identity as initial publication.
pub(crate) fn report_lost(bits: u32) -> Result<bool, String> {
    let lost = ReadinessMask::new(bits).map_err(|error| error.to_string())?;
    with_publisher(|publisher| publisher.report_lost(lost))
}

fn with_publisher(action: impl FnOnce(&Publisher) -> Result<(), String>) -> Result<bool, String> {
    static LAUNCH: OnceLock<Result<Option<Publisher>, String>> = OnceLock::new();
    let launch = LAUNCH.get_or_init(|| {
        Publisher::from_launch(
            std::env::var_os("DARWIN_ART_RUNTIME_INSTANCE_TOKEN"),
            std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV),
        )
    });
    match launch {
        Ok(Some(publisher)) => {
            action(publisher)?;
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "executed by publisher wire test in isolated child processes"]
    fn native_ready_child_fixture() {
        use crate::service_endpoint_ffi::*;
        use std::os::unix::ffi::OsStrExt;
        let path = std::env::var_os("DARWIN_ART_TEST_READY_ENDPOINT").unwrap();
        let expected: i32 = std::env::var("DARWIN_ART_TEST_READY_RESULT")
            .unwrap()
            .parse()
            .unwrap();
        let (mut handle, mut descriptor) = (0, -1);
        let publish: fn(u64, u32) -> i32 =
            if std::env::var_os("DARWIN_ART_TEST_READY_LOSS").is_some() {
                |handle, bits| darwin_art_service_endpoint_lost(handle, bits)
            } else {
                |handle, bits| darwin_art_service_endpoint_ready(handle, bits)
            };
        assert_eq!(publish(0, 2), -1);
        let bytes = path.as_os_str().as_bytes();
        assert_eq!(
            unsafe {
                darwin_art_service_endpoint_open(
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut handle,
                    &mut descriptor,
                )
            },
            0
        );
        assert_eq!(publish(handle, 0), -1);
        assert_eq!(publish(handle, 4), -1);
        assert_eq!(publish(handle, 2), expected);
        assert_eq!(darwin_art_service_endpoint_close(handle), 0);
        assert_eq!(publish(handle, 2), -1);
    }

    #[test]
    fn publisher_sends_exact_token_and_capability_and_requires_acknowledgment() {
        exercise_publication(13);
    }

    #[test]
    fn owner_loss_uses_separate_operation_and_requires_acknowledgment() {
        exercise_publication(14);
    }

    fn exercise_publication(operation: u16) {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        let root = std::env::temp_dir().join(format!(
            "dar-ready-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let socket = root.join("profile.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let publisher =
            Publisher::from_launch(Some("12".repeat(16).into()), Some(socket.into_os_string()))
                .unwrap()
                .unwrap();
        let expected = if operation == 13 {
            ReadyRequest {
                token: publisher.token,
                readiness: ReadinessMask::COMPOSITOR,
            }
            .encode()
        } else {
            LossRequest {
                token: publisher.token,
                lost: ReadinessMask::COMPOSITOR,
            }
            .encode()
        }
        .unwrap();
        let worker = std::thread::spawn(move || {
            // This is a wire fixture, not the profile daemon's authentication
            // test. Real peer/token/instance checks live in darwin-art-profile.
            for status in [0_u32, 1, 0, 1] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut header = [0; 16];
                stream.read_exact(&mut header).unwrap();
                assert_eq!(&header[..8], b"DARTD001");
                assert_eq!(u16::from_le_bytes(header[8..10].try_into().unwrap()), 1);
                assert_eq!(
                    u16::from_le_bytes(header[10..12].try_into().unwrap()),
                    operation
                );
                assert_eq!(
                    u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize,
                    expected.len()
                );
                let mut payload = vec![0; expected.len()];
                stream.read_exact(&mut payload).unwrap();
                assert_eq!(payload, expected);
                header[10..12].copy_from_slice(&(operation | 0x8000).to_le_bytes());
                header[12..16].copy_from_slice(&4_u32.to_le_bytes());
                stream.write_all(&header).unwrap();
                stream.write_all(&status.to_le_bytes()).unwrap();
            }
        });
        let send = || {
            if operation == 13 {
                publisher.publish(ReadinessMask::COMPOSITOR)
            } else {
                publisher.report_lost(ReadinessMask::COMPOSITOR)
            }
        };
        assert!(send().is_ok());
        assert!(send().is_err());
        for expected in [0, -1, 1] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--ignored",
                    "--exact",
                    "service_readiness::tests::native_ready_child_fixture",
                ])
                .env("DARWIN_ART_TEST_READY_ENDPOINT", root.join("native.sock"))
                .env("DARWIN_ART_TEST_READY_RESULT", expected.to_string())
                .env(darwin_art_profile::PROFILE_SOCKET_ENV, &publisher.socket);
            if operation == 14 {
                child.env("DARWIN_ART_TEST_READY_LOSS", "1");
            } else {
                child.env_remove("DARWIN_ART_TEST_READY_LOSS");
            }
            if expected == 1 {
                child.env_remove("DARWIN_ART_RUNTIME_INSTANCE_TOKEN");
            } else {
                child.env("DARWIN_ART_RUNTIME_INSTANCE_TOKEN", "12".repeat(16));
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unmanaged_process_is_not_a_ready_publication() {
        assert!(
            Publisher::from_launch(None, Some("/tmp/profile.sock".into()))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn managed_configuration_is_strict_and_preserves_token() {
        let token = "0123456789abcdef0123456789abcdef";
        let publisher =
            Publisher::from_launch(Some(token.into()), Some("/tmp/profile.sock".into()))
                .unwrap()
                .unwrap();
        assert_eq!(
            publisher.token.0,
            [
                1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239
            ]
        );
        assert!(Publisher::from_launch(Some(token.into()), None).is_err());
        assert!(Publisher::from_launch(Some(token.into()), Some("relative".into())).is_err());
        assert!(Publisher::from_launch(Some("bad".into()), Some("/tmp/p".into())).is_err());
        assert!(Publisher::from_launch(Some(token.into()), Some("/tmp/../p".into())).is_err());
    }
    #[test]
    fn unavailable_profile_and_inherited_process_identity_are_errors() {
        let mut publisher = Publisher::from_launch(
            Some("01".repeat(16).into()),
            Some("/dev/null/not-a-socket".into()),
        )
        .unwrap()
        .unwrap();
        assert!(publisher.publish(ReadinessMask::BINDER).is_err());
        assert!(publisher.report_lost(ReadinessMask::BINDER).is_err());
        publisher.pid = 0;
        assert!(
            publisher
                .publish(ReadinessMask::BINDER)
                .unwrap_err()
                .contains("forked")
        );
        assert!(
            publisher
                .report_lost(ReadinessMask::BINDER)
                .unwrap_err()
                .contains("forked")
        );
    }
}
