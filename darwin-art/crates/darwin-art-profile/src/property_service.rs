//! init's property service for a profile (ADR 0009): the authority that
//! accepts or rejects `__system_property_set` requests. Requests arrive over
//! the profile protocol from the system server; the checks follow init's
//! property_service.cpp (name/value validation, write-once `ro.*`, no control
//! messages) with the system server as the only permitted writer, as the
//! platform policy grants few property writes to other domains. `persist.*`
//! values survive restarts, as init's persistent_properties does.
//!
//! Every accepted value is published for all processes, as init's shared
//! property area is: `dynamic_properties` holds the service's values and
//! `generation` is bumped after it, before the setter gets its reply. Each
//! process folds a newer generation into its own area before a read
//! (bionic-process-state-facade `property_publication`).

use crate::ProfileError;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn notify_post(name: *const std::ffi::c_char) -> u32;
}

/// bionic PROP_VALUE_MAX.
const VALUE_MAX: usize = 92;

/// system_properties.h PROP_ERROR_* results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetError {
    ReadOnlyProperty = 0x0b,
    InvalidName = 0x10,
    InvalidValue = 0x14,
    PermissionDenied = 0x18,
    HandleControlMessage = 0x20,
    SetFailed = 0x24,
}

pub(crate) struct PropertyService {
    persistent_path: PathBuf,
    publication: PathBuf,
    generation: u64,
    values: BTreeMap<String, String>,
}

/// init's IsLegalPropertyName.
fn legal_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-@:.".contains(&byte))
}

impl PropertyService {
    /// The service over `persistent_path`, with the persisted values loaded
    /// and published under `publication`.
    pub(crate) fn open(
        persistent_path: PathBuf,
        publication: PathBuf,
    ) -> Result<Self, ProfileError> {
        let mut values = BTreeMap::new();
        match fs::read_to_string(&persistent_path) {
            Ok(text) => {
                for line in text.lines().filter(|line| !line.is_empty()) {
                    let (name, value) = line.split_once('=').ok_or_else(|| {
                        ProfileError::Daemon("malformed persistent property".into())
                    })?;
                    values.insert(name.to_owned(), value.to_owned());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // Generations only grow, across daemon restarts too, so a process
        // never mistakes a new publication for one it already folded in.
        let generation = fs::read(publication.join("generation"))
            .ok()
            .and_then(|bytes| {
                bytes
                    .get(..8)
                    .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
            })
            .unwrap_or(0);
        let mut service = Self {
            persistent_path,
            publication,
            generation,
            values,
        };
        service.publish()?;
        Ok(service)
    }

    pub(crate) fn set(
        &mut self,
        system_caller: bool,
        name: &str,
        value: &str,
    ) -> Result<(), SetError> {
        if !legal_name(name) {
            return Err(SetError::InvalidName);
        }
        if name.starts_with("ctl.") {
            // No init services exist to control.
            return Err(SetError::HandleControlMessage);
        }
        if !system_caller {
            return Err(SetError::PermissionDenied);
        }
        let read_only = name.starts_with("ro.");
        if (!read_only && value.len() >= VALUE_MAX) || value.contains('\n') || value.contains('\0')
        {
            return Err(SetError::InvalidValue);
        }
        if read_only && self.values.contains_key(name) {
            return Err(SetError::ReadOnlyProperty);
        }
        let previous = self.values.insert(name.to_owned(), value.to_owned());
        if (name.starts_with("persist.") && self.persist().is_err()) || self.publish().is_err() {
            match previous {
                Some(previous) => self.values.insert(name.to_owned(), previous),
                None => self.values.remove(name),
            };
            return Err(SetError::SetFailed);
        }
        Ok(())
    }

    /// Rewrites `dynamic_properties`, then bumps `generation`: a process that
    /// sees the new generation reads the complete new values.
    fn publish(&mut self) -> std::io::Result<()> {
        fs::create_dir_all(&self.publication)?;
        let stage = self
            .publication
            .join(format!(".dynamic_properties.{}", std::process::id()));
        let mut output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&stage)?;
        for (name, value) in &self.values {
            writeln!(output, "{name}={value}")?;
        }
        output.sync_all()?;
        fs::rename(&stage, self.publication.join("dynamic_properties"))?;
        let next = self.generation + 1;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.publication.join("generation"))?
            .write_all_at(&next.to_le_bytes(), 0)?;
        self.generation = next;
        self.notify();
        Ok(())
    }

    /// Wakes processes blocked in `__system_property_wait`; the name matches
    /// bionic-process-state-facade `property_publication::notification_name`.
    fn notify(&self) {
        use std::os::unix::fs::MetadataExt;
        let Ok(metadata) = fs::metadata(&self.publication) else {
            return;
        };
        let name = format!(
            "dev.darwinart.properties.{}.{}",
            metadata.dev(),
            metadata.ino()
        );
        if let Ok(name) = std::ffi::CString::new(name) {
            // SAFETY: NUL-terminated name.
            unsafe { notify_post(name.as_ptr()) };
        }
    }

    fn persist(&self) -> std::io::Result<()> {
        let directory = self.persistent_path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(directory)?;
        let stage = directory.join(format!(".persistent_properties.{}", std::process::id()));
        let mut output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&stage)?;
        for (name, value) in self
            .values
            .iter()
            .filter(|(name, _)| name.starts_with("persist."))
        {
            writeln!(output, "{name}={value}")?;
        }
        output.sync_all()?;
        fs::rename(&stage, &self.persistent_path)?;
        fs::File::open(directory)?.sync_all()
    }
}

/// Request payload: name NUL value.
pub(crate) fn decode_request(payload: &[u8]) -> Result<(String, String), ProfileError> {
    let separator = payload
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| ProfileError::Daemon("malformed property request".into()))?;
    let text = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ProfileError::Daemon("property request is not UTF-8".into()))
    };
    Ok((
        text(&payload[..separator])?,
        text(&payload[separator + 1..])?,
    ))
}

pub(crate) fn encode_request(name: &str, value: &str) -> Vec<u8> {
    let mut payload = name.as_bytes().to_vec();
    payload.push(0);
    payload.extend_from_slice(value.as_bytes());
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(label: &str) -> (PathBuf, PropertyService) {
        let root =
            std::env::temp_dir().join(format!("darwin-properties-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("persistent_properties");
        (
            path.clone(),
            PropertyService::open(path, root.join("publication")).unwrap(),
        )
    }

    #[test]
    fn applies_init_policy() {
        let (_, mut properties) = service("policy");
        assert_eq!(
            properties.set(true, "sys.user.0.ce_available", "true"),
            Ok(())
        );
        assert_eq!(
            properties.set(false, "sys.user.0.ce_available", "true"),
            Err(SetError::PermissionDenied)
        );
        assert_eq!(
            properties.set(true, "ctl.start", "x"),
            Err(SetError::HandleControlMessage)
        );
        assert_eq!(
            properties.set(true, "bad..name", "x"),
            Err(SetError::InvalidName)
        );
        assert_eq!(
            properties.set(true, "sys.long", &"x".repeat(92)),
            Err(SetError::InvalidValue)
        );
        assert_eq!(properties.set(true, "ro.long", &"x".repeat(120)), Ok(()));
        assert_eq!(
            properties.set(true, "ro.long", "y"),
            Err(SetError::ReadOnlyProperty)
        );
    }

    #[test]
    fn persists_only_persist_properties() {
        let (path, mut properties) = service("persist");
        properties.set(true, "persist.sys.locale", "ko-KR").unwrap();
        properties.set(true, "sys.transient", "1").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "persist.sys.locale=ko-KR\n"
        );
        let reopened =
            PropertyService::open(path.clone(), path.parent().unwrap().join("publication"))
                .unwrap();
        assert_eq!(
            reopened
                .values
                .get("persist.sys.locale")
                .map(String::as_str),
            Some("ko-KR")
        );
        assert!(!reopened.values.contains_key("sys.transient"));
    }

    #[test]
    fn publishes_values_before_bumping_the_generation() {
        let (path, mut properties) = service("publish");
        let publication = path.parent().unwrap().join("publication");
        let generation = || {
            u64::from_le_bytes(
                fs::read(publication.join("generation")).unwrap()[..8]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(generation(), 1);
        properties
            .set(true, "persist.sys.timezone", "Asia/Seoul")
            .unwrap();
        properties.set(true, "sys.boot_completed", "1").unwrap();
        assert_eq!(generation(), 3);
        assert_eq!(
            fs::read_to_string(publication.join("dynamic_properties")).unwrap(),
            "persist.sys.timezone=Asia/Seoul\nsys.boot_completed=1\n"
        );
        // A rejected write publishes nothing.
        assert!(properties.set(false, "sys.x", "1").is_err());
        assert_eq!(generation(), 3);
        // Persisted values are published again, with a later generation,
        // when the service reopens.
        let reopened = PropertyService::open(path, publication.clone()).unwrap();
        assert_eq!(generation(), 4);
        assert_eq!(
            fs::read_to_string(publication.join("dynamic_properties")).unwrap(),
            "persist.sys.timezone=Asia/Seoul\n"
        );
        drop(reopened);
    }

    #[test]
    fn round_trips_requests() {
        assert_eq!(
            decode_request(&encode_request("a.b", "c=d")).unwrap(),
            ("a.b".to_owned(), "c=d".to_owned())
        );
        assert!(decode_request(b"no-separator").is_err());
    }
}
