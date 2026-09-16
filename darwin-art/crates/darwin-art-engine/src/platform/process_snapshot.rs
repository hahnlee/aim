//! Safe engine-side owner for configured Android process-state inputs.
//!
//! This module owns only the temporary wire representation and the input
//! bytes. The native process-state owner copies every span synchronously;
//! `EngineSession` records ownership only after that copy reports success.

use super::process_credentials::ProcessCredentialsInputs;
use darwin_art_engine_sys::{
    PROCESS_SNAPSHOT_ABI_VERSION, ProcessSnapshotConfig, ProcessSnapshotEntry,
    ProcessSnapshotInstallConfiguredFn,
};
use darwin_art_engine_sys::{PROCESS_SNAPSHOT_CREDENTIALS_ABI_VERSION, ProcessSnapshotConfigV2};
use std::fmt;

#[derive(Clone, Debug)]
pub struct ProcessSnapshotInputs {
    pub environment: Vec<(Vec<u8>, Vec<u8>)>,
    pub properties: Vec<(Vec<u8>, Vec<u8>)>,
    pub page_size: u64,
    pub hwcap: u64,
    pub hwcap2: u64,
    pub secure: bool,
    pub random: [u8; 16],
}

impl ProcessSnapshotInputs {
    pub fn new(
        environment: Vec<(Vec<u8>, Vec<u8>)>,
        properties: Vec<(Vec<u8>, Vec<u8>)>,
        page_size: u64,
        hwcap: u64,
        hwcap2: u64,
        secure: bool,
        random: [u8; 16],
    ) -> Self {
        Self {
            environment,
            properties,
            page_size,
            hwcap,
            hwcap2,
            secure,
            random,
        }
    }

    fn prepare(&self) -> PreparedSnapshot<'_> {
        let environment = entries(&self.environment);
        let properties = entries(&self.properties);
        let config = ProcessSnapshotConfig {
            abi_version: PROCESS_SNAPSHOT_ABI_VERSION,
            struct_size: std::mem::size_of::<ProcessSnapshotConfig>() as u32,
            environment: environment.as_ptr(),
            environment_count: environment.len(),
            properties: properties.as_ptr(),
            property_count: properties.len(),
            page_size: self.page_size,
            hwcap: self.hwcap,
            hwcap2: self.hwcap2,
            secure: u8::from(self.secure),
            reserved: [0; 7],
            random: self.random,
        };
        PreparedSnapshot {
            _inputs: self,
            _environment: environment,
            _properties: properties,
            config,
        }
    }

    pub(crate) fn install_with(
        &self,
        install: ProcessSnapshotInstallConfiguredFn,
    ) -> Result<(), ProcessSnapshotError> {
        self.prepare().install(install)
    }

    pub(crate) fn install_with_credentials(
        &self,
        credentials: &ProcessCredentialsInputs,
        install: ProcessSnapshotInstallConfiguredFn,
    ) -> Result<(), ProcessSnapshotError> {
        let prepared = self.prepare();
        let mut config = ProcessSnapshotConfigV2 {
            base: prepared.config,
            credentials: credentials.wire(),
        };
        config.base.abi_version = PROCESS_SNAPSHOT_CREDENTIALS_ABI_VERSION;
        config.base.struct_size = std::mem::size_of::<ProcessSnapshotConfigV2>() as u32;
        // SAFETY: base is the initial member of the versioned allocation.
        // All input bytes, entry arrays and group storage remain live until
        // the native owner has synchronously validated and copied them.
        let status = unsafe { install(&config.base) };
        if status == 0 {
            Ok(())
        } else {
            Err(ProcessSnapshotError::NativeFailure(status))
        }
    }
}

fn entries(source: &[(Vec<u8>, Vec<u8>)]) -> Vec<ProcessSnapshotEntry> {
    source
        .iter()
        .map(|(name, value)| ProcessSnapshotEntry {
            name: name.as_ptr(),
            name_size: name.len(),
            value: value.as_ptr(),
            value_size: value.len(),
        })
        .collect()
}

struct PreparedSnapshot<'a> {
    // Keep both the caller's bytes and the entry arrays alive through the
    // synchronous native copy. Raw pointers in `config` are never retained.
    _inputs: &'a ProcessSnapshotInputs,
    _environment: Vec<ProcessSnapshotEntry>,
    _properties: Vec<ProcessSnapshotEntry>,
    config: ProcessSnapshotConfig,
}

impl PreparedSnapshot<'_> {
    fn install(
        &self,
        install: ProcessSnapshotInstallConfiguredFn,
    ) -> Result<(), ProcessSnapshotError> {
        // SAFETY: all pointers in `config` refer to `_inputs` and the entry
        // vectors owned by this value, which remain alive for this call.
        let status = unsafe { install(&self.config) };
        if status == 0 {
            Ok(())
        } else {
            Err(ProcessSnapshotError::NativeFailure(status))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessSnapshotError {
    EngineClosed,
    AlreadyInstalled,
    NativeFailure(i32),
}

impl fmt::Display for ProcessSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EngineClosed => formatter.write_str("process engine is closed"),
            Self::AlreadyInstalled => formatter.write_str("process snapshot already installed"),
            Self::NativeFailure(status) => {
                write!(
                    formatter,
                    "configured process snapshot install failed ({status})"
                )
            }
        }
    }
}

impl std::error::Error for ProcessSnapshotError {}

#[cfg(test)]
#[path = "process_credentials_tests.rs"]
mod credentials_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_inputs_materialize_stable_c_spans() {
        let inputs = ProcessSnapshotInputs::new(
            vec![(b"ENV_NAME".to_vec(), b"env-value".to_vec())],
            vec![(b"prop.name".to_vec(), b"prop-value".to_vec())],
            16_384,
            3,
            0,
            false,
            [0x27; 16],
        );
        let prepared = inputs.prepare();
        assert_eq!(prepared.config.abi_version, 1);
        assert_eq!(
            prepared.config.struct_size as usize,
            std::mem::size_of::<ProcessSnapshotConfig>()
        );
        assert_eq!(prepared.config.environment_count, 1);
        assert_eq!(prepared.config.property_count, 1);
        // SAFETY: pointers point into `inputs`, which is alive for this test.
        unsafe {
            let environment = &*prepared.config.environment;
            let properties = &*prepared.config.properties;
            assert_eq!(
                std::slice::from_raw_parts(environment.name, environment.name_size),
                b"ENV_NAME"
            );
            assert_eq!(
                std::slice::from_raw_parts(environment.value, environment.value_size),
                b"env-value"
            );
            assert_eq!(
                std::slice::from_raw_parts(properties.name, properties.name_size),
                b"prop.name"
            );
            assert_eq!(
                std::slice::from_raw_parts(properties.value, properties.value_size),
                b"prop-value"
            );
        }
    }
}
