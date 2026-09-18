//! Strict CLI adapter for the profile-owned runtime service.
//!
//! The runtime key is intentionally caller-supplied. This adapter does not
//! derive or verify it; callers must obtain it from the trusted runtime-image
//! identity owner before invoking this function.

use crate::runtime_service_protocol::{
    ReadinessMask, RuntimeKey, StartRuntimeRequest, StartRuntimeResponse,
};
use crate::{runtime_service_client, ProfileError};
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::str::FromStr;

/// Parse and start PACKAGE KEY_HEX REQUIRED_MASK COMMAND [ARGS...].
///
/// The arguments slice excludes the CLI executable and subcommand itself. The
/// environment is supplied by the caller; this function never captures the
/// ambient process environment and never starts/ensures a daemon.
pub fn start_runtime_cli(
    socket: &Path,
    arguments: &[OsString],
    environment: Vec<(OsString, OsString)>,
) -> Result<StartRuntimeResponse, ProfileError> {
    let request = parse_request(arguments, environment)?;
    // Validate and bound the complete request before opening the socket. The
    // typed client encodes again as its transport boundary requires bytes.
    request.encode()?;
    runtime_service_client::start_runtime_service(socket, &request)
}

fn parse_request(
    arguments: &[OsString],
    environment: Vec<(OsString, OsString)>,
) -> Result<StartRuntimeRequest, ProfileError> {
    if arguments.len() < 4 {
        return Err(usage(
            "expected PACKAGE KEY_HEX REQUIRED_MASK COMMAND [ARGS...]",
        ));
    }
    let package = utf8_argument(&arguments[0], "package")?;
    let key_text = utf8_argument(&arguments[1], "runtime key")?;
    let key = RuntimeKey::from_str(key_text)?;
    let required_text = utf8_argument(&arguments[2], "readiness mask")?;
    let required = parse_mask(required_text)?;
    if arguments[3].as_os_str().as_bytes().is_empty() {
        return Err(usage("command must not be empty"));
    }
    let command = arguments[3..].to_vec();
    let request = StartRuntimeRequest {
        package: package.to_owned(),
        key,
        required,
        arguments: command,
        environment,
    };
    request.encode()?;
    Ok(request)
}

fn utf8_argument<'a>(argument: &'a OsString, name: &str) -> Result<&'a str, ProfileError> {
    argument
        .to_str()
        .ok_or_else(|| usage(&format!("{name} must be UTF-8")))
}

fn parse_mask(value: &str) -> Result<ReadinessMask, ProfileError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(usage("readiness mask must be an unsigned decimal integer"));
    }
    let bits = value
        .parse::<u32>()
        .map_err(|_| usage("readiness mask is out of range"))?;
    ReadinessMask::new(bits)
}

fn usage(message: &str) -> ProfileError {
    ProfileError::InvalidResponse(format!("runtime-service: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn key() -> OsString {
        OsString::from("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    }

    fn args() -> Vec<OsString> {
        vec![
            OsString::from("org.example.runtime"),
            key(),
            OsString::from("3"),
            OsString::from("/bin/runtime"),
            OsString::from("--guest"),
        ]
    }

    #[test]
    fn parses_strict_typed_request_without_connecting() {
        let request =
            parse_request(&args(), vec![(OsString::from("LANG"), OsString::from("C"))]).unwrap();
        assert_eq!(request.package, "org.example.runtime");
        assert_eq!(request.key.to_string(), key().to_str().unwrap());
        assert_eq!(request.required.bits(), 3);
        assert_eq!(request.arguments[0], OsString::from("/bin/runtime"));
        assert_eq!(request.environment.len(), 1);
        assert!(request.encode().is_ok());
    }

    #[test]
    fn rejects_missing_or_non_utf8_control_arguments() {
        assert!(parse_request(&args()[..3], Vec::new()).is_err());
        let mut invalid = args();
        invalid[0] = OsString::from_vec(vec![b'o', 0xff]);
        assert!(parse_request(&invalid, Vec::new()).is_err());
        let mut invalid = args();
        invalid[1] = OsString::from_vec(vec![b'g'; 64]);
        assert!(parse_request(&invalid, Vec::new()).is_err());
    }

    #[test]
    fn rejects_invalid_mask_package_and_empty_command() {
        for mask in ["", "0", "4", "3x", "+3", "0x3", "4294967296"] {
            let mut invalid = args();
            invalid[2] = OsString::from(mask);
            assert!(parse_request(&invalid, Vec::new()).is_err(), "mask={mask}");
        }
        for package in ["", ".bad", "bad.", "a..b", "a/b"] {
            let mut invalid = args();
            invalid[0] = OsString::from(package);
            assert!(
                parse_request(&invalid, Vec::new()).is_err(),
                "package={package}"
            );
        }
        let mut invalid = args();
        invalid[3] = OsString::new();
        assert!(parse_request(&invalid, Vec::new()).is_err());
    }

    #[test]
    fn preserves_non_utf8_command_arguments_for_wire_validation() {
        let mut arguments = args();
        arguments.push(OsString::from_vec(vec![b'a', 0x80, b'b']));
        let request = parse_request(&arguments, Vec::new()).unwrap();
        assert_eq!(
            request.arguments[2].as_os_str().as_bytes(),
            &[b'a', 0x80, b'b']
        );
        assert!(request.encode().is_ok());
    }

    #[test]
    fn ambient_environment_is_not_consulted() {
        let request = parse_request(&args(), Vec::new()).unwrap();
        assert!(request.environment.is_empty());
    }
}
