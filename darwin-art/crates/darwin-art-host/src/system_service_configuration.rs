//! Canonical configuration for the profile-owned system-service runtime.
//!
//! The launcher and the typed profile request must use the same filtered
//! environment.  Per-instance transport credentials are supplied by the
//! profile daemon after reservation and therefore never participate in the
//! stable system-service key.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt;

const KEY_DOMAIN: &[u8] = b"darwin-art-system-service-key-v1";
const INSTANCE_ONLY: &[&[u8]] = &[
    b"DARWIN_ART_RUNTIME_INSTANCE_TOKEN",
    b"DARWIN_ART_PROFILE_LEASE_FD",
    b"DARWIN_ART_DESKTOP_PRESENTATION",
];

/// Capture only the environment that the profile CLI is permitted to forward
/// to a new system-service process.  Raw bytes are preserved and keys are
/// returned in deterministic byte order.
pub(crate) fn capture(
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<Vec<(OsString, OsString)>> {
    let mut selected = BTreeMap::<Vec<u8>, (OsString, OsString)>::new();
    for (name, value) in environment {
        let name_bytes = name.as_os_str().as_bytes();
        if !is_forwarded_name(name_bytes) {
            continue;
        }
        if selected
            .insert(name_bytes.to_vec(), (name, value))
            .is_some()
        {
            return Err(invalid("duplicate forwarded environment key"));
        }
    }
    Ok(selected.into_values().collect())
}

/// Hash the prepared system-service identity and its stable launch
/// configuration.  Every component is length-delimited to prevent boundary
/// ambiguities; environment entries are canonically ordered by raw key/value
/// bytes, while argv order remains significant.
pub(crate) fn key(
    identity: [u8; 32],
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    field(&mut digest, KEY_DOMAIN);
    field(&mut digest, &identity);
    field(&mut digest, &(arguments.len() as u64).to_le_bytes());
    for argument in arguments {
        field(&mut digest, argument.as_os_str().as_bytes());
    }

    let mut entries = environment.iter().collect::<Vec<_>>();
    entries.sort_by(|(left_name, left_value), (right_name, right_value)| {
        left_name
            .as_os_str()
            .as_bytes()
            .cmp(right_name.as_os_str().as_bytes())
            .then_with(|| {
                left_value
                    .as_os_str()
                    .as_bytes()
                    .cmp(right_value.as_os_str().as_bytes())
            })
    });
    field(&mut digest, &(entries.len() as u64).to_le_bytes());
    for (name, value) in entries {
        field(&mut digest, name.as_os_str().as_bytes());
        field(&mut digest, value.as_os_str().as_bytes());
    }
    digest.finalize().into()
}

fn is_forwarded_name(name: &[u8]) -> bool {
    if INSTANCE_ONLY.iter().any(|excluded| *excluded == name) {
        return false;
    }
    name.starts_with(b"DARWIN_ART_")
        || name.starts_with(b"ANDROID_")
        || matches!(
            name,
            b"PATH" | b"HOME" | b"TMPDIR" | b"LANG" | b"LC_ALL" | b"SHELL"
        )
}

fn field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    fn env(name: &str, value: &str) -> (OsString, OsString) {
        (os(name), os(value))
    }

    #[test]
    fn capture_whitelists_sorts_and_excludes_instance_fields() {
        let captured = capture([
            env("ANDROID_Z", "z"),
            env("PATH", "/bin"),
            env("DARWIN_ART_PROFILE_LEASE_FD", "17"),
            env("DARWIN_ART_RUNTIME_INSTANCE_TOKEN", "token"),
            env("DARWIN_ART_DESKTOP_PRESENTATION", "1"),
            env("DARWIN_ART_ALPHA", "a"),
            env("UNRELATED", "no"),
            env("HOME", "/tmp/home"),
        ])
        .unwrap();
        let names = captured
            .iter()
            .map(|(name, _)| name.as_os_str().as_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                b"ANDROID_Z".as_slice(),
                b"DARWIN_ART_ALPHA",
                b"HOME",
                b"PATH"
            ]
        );
    }

    #[test]
    fn capture_rejects_duplicate_forwarded_names() {
        assert!(capture([env("PATH", "/one"), env("PATH", "/two")]).is_err());
        assert!(capture([env("UNRELATED", "one"), env("UNRELATED", "two")]).is_ok());
    }

    #[test]
    fn capture_preserves_non_utf8_values_and_supported_names() {
        let captured = capture([(
            OsString::from_vec(b"ANDROID_BYTES".to_vec()),
            OsString::from_vec(vec![b'a', 0xff, b'b']),
        )])
        .unwrap();
        assert_eq!(captured[0].0.as_os_str().as_bytes(), b"ANDROID_BYTES");
        assert_eq!(captured[0].1.as_os_str().as_bytes(), &[b'a', 0xff, b'b']);
    }

    #[test]
    fn key_is_environment_order_independent_but_argv_order_sensitive() {
        let first = vec![env("B", "2"), env("A", "1")];
        let second = vec![env("A", "1"), env("B", "2")];
        let arguments = vec![os("runtime"), os("--system")];
        assert_eq!(
            key([7; 32], &arguments, &first),
            key([7; 32], &arguments, &second)
        );

        let reordered = vec![os("--system"), os("runtime")];
        assert_ne!(
            key([7; 32], &arguments, &first),
            key([7; 32], &reordered, &first)
        );
    }

    #[test]
    fn key_is_length_delimited_and_identity_bound() {
        let joined = vec![os("ab"), os("c")];
        let split = vec![os("a"), os("bc")];
        assert_ne!(
            key([7; 32], &joined, &[]),
            key([7; 32], &split, &[]),
            "argv boundaries must affect the key"
        );
        assert_ne!(key([7; 32], &joined, &[]), key([8; 32], &joined, &[]));
        assert_ne!(
            key([7; 32], &joined, &[env("LANG", "C")]),
            key([7; 32], &joined, &[env("LANG", "en_US")])
        );
        assert_ne!(
            key([7; 32], &[], &[env("A", "BC")]),
            key([7; 32], &[], &[env("AB", "C")])
        );
    }
}
