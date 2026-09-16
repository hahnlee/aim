//! Per-instance Binder and SurfaceFlinger endpoint names.
//!
//! Startup requests carry stable legacy base paths for compatibility. The
//! profile owner derives namespaced paths from the daemon-issued instance
//! token and overrides those two environment values only on the child being
//! spawned. Existing legacy socket files are never removed here.

use crate::runtime_service_protocol::InstanceToken;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub const SYSTEM_SERVER_SOCKET_ENV: &str = "DARWIN_ART_SYSTEM_SERVER_SOCKET";
pub const SURFACEFLINGER_SOCKET_ENV: &str = "DARWIN_ART_SURFACEFLINGER_SOCKET";

const UNIX_SOCKET_PATH_MAX: usize = 104;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEndpoints {
    pub binder: PathBuf,
    pub compositor: PathBuf,
}

impl RuntimeEndpoints {
    /// Resolve stable legacy base paths into namespaced per-instance sockets.
    /// Generic runtime tests may omit both endpoint keys and receive `None`.
    pub fn resolve(
        environment: &[(OsString, OsString)],
        token: InstanceToken,
    ) -> io::Result<Option<Self>> {
        let mut system_server = None;
        let mut surfaceflinger = None;
        for (name, value) in environment {
            let key = name.as_os_str().as_bytes();
            if key == SYSTEM_SERVER_SOCKET_ENV.as_bytes() {
                if system_server.replace(value.clone()).is_some() {
                    return Err(invalid("duplicate system-server socket key"));
                }
            } else if key == SURFACEFLINGER_SOCKET_ENV.as_bytes() {
                if surfaceflinger.replace(value.clone()).is_some() {
                    return Err(invalid("duplicate SurfaceFlinger socket key"));
                }
            }
        }
        let (system_server, surfaceflinger) = match (system_server, surfaceflinger) {
            (None, None) => return Ok(None),
            (Some(system_server), Some(surfaceflinger)) => (system_server, surfaceflinger),
            _ => return Err(invalid("endpoint keys must be supplied as a pair")),
        };
        let system_parent = validated_parent(&system_server)?;
        let surface_parent = validated_parent(&surfaceflinger)?;
        if system_parent != surface_parent {
            return Err(invalid("endpoint base paths must share a parent"));
        }
        let token = token_hex(token);
        let binder = system_parent.join(format!("{token}.system.sock"));
        let compositor = system_parent.join(format!("{token}.sf.sock"));
        validate_socket_path(&binder)?;
        validate_socket_path(&compositor)?;
        Ok(Some(Self { binder, compositor }))
    }

    /// Return the two environment overrides for `Command::envs` or equivalent.
    pub fn environment(&self) -> [(OsString, OsString); 2] {
        [
            (
                OsString::from(SYSTEM_SERVER_SOCKET_ENV),
                self.binder.clone().into_os_string(),
            ),
            (
                OsString::from(SURFACEFLINGER_SOCKET_ENV),
                self.compositor.clone().into_os_string(),
            ),
        ]
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn validated_parent(base: &OsString) -> io::Result<PathBuf> {
    if base.to_str().is_none() {
        return Err(invalid("endpoint path must be UTF-8"));
    }
    let path = Path::new(base);
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.contains(&b'\n') || bytes.contains(&b'\r') {
        return Err(invalid("endpoint path contains an unsafe byte"));
    }
    if !path.is_absolute() {
        return Err(invalid("endpoint path must be absolute"));
    }
    if path.file_name().is_none() {
        return Err(invalid("endpoint path must name a socket"));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| invalid("endpoint path has no parent"))?;
    if parent
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid("endpoint parent may not traverse upward"));
    }
    validate_socket_path(parent)?;
    Ok(parent.to_path_buf())
}

fn validate_socket_path(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= UNIX_SOCKET_PATH_MAX {
        return Err(invalid(
            "Unix socket path is empty, contains NUL, or is too long",
        ));
    }
    Ok(())
}

fn token_hex(token: InstanceToken) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(32);
    for byte in token.0 {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn environment(parent: &str) -> Vec<(OsString, OsString)> {
        vec![
            (
                SYSTEM_SERVER_SOCKET_ENV.into(),
                format!("{parent}/legacy-system.sock").into(),
            ),
            (
                SURFACEFLINGER_SOCKET_ENV.into(),
                format!("{parent}/legacy-sf.sock").into(),
            ),
        ]
    }

    #[test]
    fn neither_endpoint_key_is_generic_runtime_none() {
        assert_eq!(
            RuntimeEndpoints::resolve(&[], InstanceToken([1; 16])).unwrap(),
            None
        );
    }

    #[test]
    fn same_token_is_stable_and_different_tokens_are_distinct() {
        let env = environment("/tmp/darwin-art-endpoints");
        let first = RuntimeEndpoints::resolve(&env, InstanceToken([0xab; 16]))
            .unwrap()
            .unwrap();
        let same = RuntimeEndpoints::resolve(&env, InstanceToken([0xab; 16]))
            .unwrap()
            .unwrap();
        let other = RuntimeEndpoints::resolve(&env, InstanceToken([0xcd; 16]))
            .unwrap()
            .unwrap();
        assert_eq!(first, same);
        assert_ne!(first, other);
        assert_eq!(
            first.binder,
            Path::new("/tmp/darwin-art-endpoints/abababababababababababababababab.system.sock")
        );
        assert_eq!(
            first.compositor,
            Path::new("/tmp/darwin-art-endpoints/abababababababababababababababab.sf.sock")
        );
    }

    #[test]
    fn endpoint_environment_overrides_retain_stable_keys() {
        let endpoints =
            RuntimeEndpoints::resolve(&environment("/tmp/endpoints"), InstanceToken([2; 16]))
                .unwrap()
                .unwrap();
        assert_eq!(
            endpoints.environment()[0].0,
            OsString::from(SYSTEM_SERVER_SOCKET_ENV)
        );
        assert_eq!(
            endpoints.environment()[1].0,
            OsString::from(SURFACEFLINGER_SOCKET_ENV)
        );
    }

    #[test]
    fn invalid_paired_endpoint_inputs_fail_closed() {
        let cases = [
            vec![(SYSTEM_SERVER_SOCKET_ENV.into(), "/tmp/system.sock".into())],
            vec![
                (SYSTEM_SERVER_SOCKET_ENV.into(), "/tmp/system.sock".into()),
                (SYSTEM_SERVER_SOCKET_ENV.into(), "/tmp/other.sock".into()),
                (SURFACEFLINGER_SOCKET_ENV.into(), "/tmp/sf.sock".into()),
            ],
            vec![
                (
                    SYSTEM_SERVER_SOCKET_ENV.into(),
                    "relative/system.sock".into(),
                ),
                (SURFACEFLINGER_SOCKET_ENV.into(), "/tmp/sf.sock".into()),
            ],
            vec![
                (
                    SYSTEM_SERVER_SOCKET_ENV.into(),
                    OsString::from_vec(b"/tmp/\xff/system.sock".to_vec()),
                ),
                (SURFACEFLINGER_SOCKET_ENV.into(), "/tmp/sf.sock".into()),
            ],
            vec![
                (
                    SYSTEM_SERVER_SOCKET_ENV.into(),
                    "/tmp/bad\n/system.sock".into(),
                ),
                (
                    SURFACEFLINGER_SOCKET_ENV.into(),
                    "/tmp/bad\n/sf.sock".into(),
                ),
            ],
            vec![
                (
                    SYSTEM_SERVER_SOCKET_ENV.into(),
                    "/tmp/a/../system.sock".into(),
                ),
                (SURFACEFLINGER_SOCKET_ENV.into(), "/tmp/a/../sf.sock".into()),
            ],
            vec![
                (
                    SYSTEM_SERVER_SOCKET_ENV.into(),
                    format!("/{}/system.sock", "a".repeat(100)).into(),
                ),
                (
                    SURFACEFLINGER_SOCKET_ENV.into(),
                    format!("/{}/sf.sock", "a".repeat(100)).into(),
                ),
            ],
        ];
        for environment in cases {
            assert!(RuntimeEndpoints::resolve(&environment, InstanceToken([3; 16])).is_err());
        }
    }

    #[test]
    fn mismatched_parents_fail_closed() {
        let environment = vec![
            (SYSTEM_SERVER_SOCKET_ENV.into(), "/tmp/system.sock".into()),
            (
                SURFACEFLINGER_SOCKET_ENV.into(),
                "/private/tmp/sf.sock".into(),
            ),
        ];
        assert!(RuntimeEndpoints::resolve(&environment, InstanceToken([4; 16])).is_err());
    }
}
