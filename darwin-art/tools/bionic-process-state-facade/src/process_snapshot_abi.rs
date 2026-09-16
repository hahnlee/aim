//! Bounded copying boundary for a host/service-supplied process snapshot.
//! This is not a guest property setter or permission bypass. The caller owns
//! configuration authority; lifetime installation is owned by process_owner.
use crate::credentials::{CapabilitySets, CredentialIds, Credentials};
use crate::{AuxSnapshot, Snapshot, install_process_snapshot};
use std::sync::Arc;

#[repr(C)]
pub struct Entry {
    pub name: *const u8,
    pub name_size: usize,
    pub value: *const u8,
    pub value_size: usize,
}

#[repr(C)]
pub struct ConfigV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub environment: *const Entry,
    pub environment_count: usize,
    pub properties: *const Entry,
    pub property_count: usize,
    pub page_size: u64,
    pub hwcap: u64,
    pub hwcap2: u64,
    pub secure: u8,
    pub reserved: [u8; 7],
    pub random: [u8; 16],
}

pub type Config = ConfigV1;

#[repr(C)]
pub struct CredentialsConfig {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub supplementary_groups: *const u32,
    pub supplementary_group_count: usize,
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

#[repr(C)]
pub struct ConfigV2 {
    pub base: ConfigV1,
    pub credentials: CredentialsConfig,
}

const MAX_ENTRIES: usize = 16384;
const MAX_BYTES: usize = 16 * 1024 * 1024;

unsafe fn copy_bytes(pointer: *const u8, len: usize, remaining: &mut usize) -> Result<Vec<u8>, ()> {
    *remaining = remaining.checked_sub(len).ok_or(())?;
    if len == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err(());
    }
    // SAFETY: caller's ABI contract guarantees valid input spans. Length is bounded above.
    Ok(unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec())
}

type OwnedEntries = Vec<(Vec<u8>, Vec<u8>)>;

unsafe fn copy_entries(
    pointer: *const Entry,
    count: usize,
    remaining: &mut usize,
) -> Result<OwnedEntries, ()> {
    if count > MAX_ENTRIES {
        return Err(());
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() || !(pointer as usize).is_multiple_of(std::mem::align_of::<Entry>()) {
        return Err(());
    }
    let entries = unsafe { std::slice::from_raw_parts(pointer, count) };
    entries
        .iter()
        .map(|entry| unsafe {
            Ok((
                copy_bytes(entry.name, entry.name_size, remaining)?,
                copy_bytes(entry.value, entry.value_size, remaining)?,
            ))
        })
        .collect()
}

unsafe fn copy_credentials(config: &CredentialsConfig) -> Result<Credentials, ()> {
    if config.supplementary_group_count > crate::credentials::MAX_SUPPLEMENTARY_GROUPS {
        return Err(());
    }
    if config.supplementary_group_count != 0
        && (config.supplementary_groups.is_null()
            || !(config.supplementary_groups as usize).is_multiple_of(std::mem::align_of::<u32>()))
    {
        return Err(());
    }
    let groups = if config.supplementary_group_count == 0 {
        Vec::new()
    } else {
        // SAFETY: the caller's ABI contract guarantees a readable group span;
        // the count is bounded above before this slice is formed.
        unsafe {
            std::slice::from_raw_parts(
                config.supplementary_groups,
                config.supplementary_group_count,
            )
            .to_vec()
        }
    };
    Credentials::new(
        CredentialIds {
            uid: config.uid,
            euid: config.euid,
            suid: config.suid,
            gid: config.gid,
            egid: config.egid,
            sgid: config.sgid,
        },
        groups,
        CapabilitySets {
            permitted: config.permitted,
            effective: config.effective,
            inheritable: config.inheritable,
        },
    )
    .map_err(|_| ())
}

/// # Safety
/// A nonnull config must point to a readable, aligned Config. Each nonempty
/// referenced array/span must be readable for its declared length during this
/// call. Installation must precede process execution; teardown must quiesce it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_bionic_process_state_install_configured(
    config: *const Config,
) -> i32 {
    if config.is_null() || !(config as usize).is_multiple_of(std::mem::align_of::<Config>()) {
        return -1;
    }
    let version = unsafe { (*config).abi_version };
    let (config, credentials) = match version {
        1 => {
            let config = unsafe { &*config };
            if config.struct_size as usize != std::mem::size_of::<ConfigV1>() {
                return -1;
            }
            (config, None)
        }
        2 => {
            // Validate the V1 prefix before forming a reference to the larger
            // V2 object: callers may supply a V1 allocation with a bad version.
            if unsafe { (*config).struct_size } as usize != std::mem::size_of::<ConfigV2>() {
                return -1;
            }
            let config_v2 = unsafe { &*(config.cast::<ConfigV2>()) };
            let Ok(credentials) = (unsafe { copy_credentials(&config_v2.credentials) }) else {
                return -1;
            };
            (&config_v2.base, Some(credentials))
        }
        _ => return -1,
    };
    if config.secure > 1 || config.reserved != [0; 7] {
        return -1;
    }
    let mut remaining = MAX_BYTES;
    let Ok(environment) =
        (unsafe { copy_entries(config.environment, config.environment_count, &mut remaining) })
    else {
        return -1;
    };
    let Ok(properties) =
        (unsafe { copy_entries(config.properties, config.property_count, &mut remaining) })
    else {
        return -1;
    };
    let aux = AuxSnapshot {
        page_size: config.page_size,
        hwcap: config.hwcap,
        hwcap2: config.hwcap2,
        secure: config.secure != 0,
        random: config.random,
    };
    let snapshot = match credentials {
        Some(credentials) => {
            Snapshot::new_with_credentials(environment, properties, aux, credentials)
        }
        None => Snapshot::new(environment, properties, aux),
    };
    let Ok(snapshot) = snapshot else {
        return -1;
    };
    match install_process_snapshot(Arc::new(snapshot)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CredentialsOutput;

    fn config() -> ConfigV1 {
        Config {
            abi_version: 1,
            struct_size: std::mem::size_of::<Config>() as u32,
            environment: std::ptr::null(),
            environment_count: 0,
            properties: std::ptr::null(),
            property_count: 0,
            page_size: 16384,
            hwcap: 3,
            hwcap2: 0,
            secure: 0,
            reserved: [0; 7],
            random: [0x43; 16],
        }
    }

    fn credentials_config(groups: &[u32]) -> CredentialsConfig {
        CredentialsConfig {
            uid: 1000,
            euid: 1001,
            suid: 1002,
            gid: 1000,
            egid: 1001,
            sgid: 1002,
            supplementary_groups: groups.as_ptr(),
            supplementary_group_count: groups.len(),
            permitted: 0b111,
            effective: 0b011,
            inheritable: 0b100,
        }
    }

    fn config_v2(groups: &[u32]) -> ConfigV2 {
        ConfigV2 {
            base: Config {
                abi_version: 2,
                struct_size: std::mem::size_of::<ConfigV2>() as u32,
                ..config()
            },
            credentials: credentials_config(groups),
        }
    }
    #[test]
    fn configured_abi_preserves_caller_values_and_copies_storage() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let mut name = b"caller.property".to_vec();
        let mut value = b"configured".to_vec();
        let entry = Entry {
            name: name.as_ptr(),
            name_size: name.len(),
            value: value.as_ptr(),
            value_size: value.len(),
        };
        let mut c = config();
        c.properties = &entry;
        c.property_count = 1;
        assert_eq!(
            unsafe { darwin_art_bionic_process_state_install_configured(&c) },
            0
        );
        name.fill(b'x');
        value.fill(b'x');
        let snapshot = crate::active_snapshot().unwrap();
        assert_eq!(
            &*snapshot
                .properties
                .get(b"caller.property")
                .unwrap()
                .unwrap()
                .value,
            b"configured\0"
        );
        assert!(
            snapshot
                .properties
                .get(b"device.cpu.count")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            unsafe { darwin_art_bionic_process_state_install_configured(&config()) },
            -1
        );
        assert_eq!(
            crate::darwin_art_bionic_process_state_process_uninstall(),
            0
        );
        assert!(crate::active_snapshot().is_none());
    }
    #[test]
    fn configured_abi_rejects_invalid_inputs_without_activation() {
        let _test_owner_lock = crate::test_process_owner_guard();
        // Only a V1 allocation exists; reject the size before borrowing V2.
        let invalid_version = ConfigV1 {
            abi_version: 2,
            ..config()
        };
        assert_eq!(
            unsafe { darwin_art_bionic_process_state_install_configured(&invalid_version) },
            -1
        );
        for c in [
            Config {
                struct_size: 1,
                ..config()
            },
            Config {
                secure: 2,
                ..config()
            },
            Config {
                property_count: 1,
                ..config()
            },
            Config {
                property_count: MAX_ENTRIES + 1,
                ..config()
            },
        ] {
            assert_eq!(
                unsafe { darwin_art_bionic_process_state_install_configured(&c) },
                -1
            );
            assert!(crate::active_snapshot().is_none());
        }
    }

    #[test]
    fn configured_v2_copies_credentials_and_failed_install_keeps_current() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let mut groups = vec![2000, 2001, 2002];
        let mut configured = config_v2(&groups);
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_state_install_configured(
                    (&mut configured as *mut ConfigV2).cast::<Config>(),
                )
            },
            0
        );
        groups.fill(0);
        let mut output = CredentialsOutput::default();
        let mut copied = [0_u32; 3];
        assert_eq!(
            unsafe {
                crate::darwin_art_bionic_process_state_read_credentials_core(
                    &mut output,
                    copied.as_mut_ptr(),
                    copied.len(),
                )
            },
            0
        );
        assert_eq!(copied, [2000, 2001, 2002]);
        assert_eq!(output.uid, 1000);
        assert_eq!(output.supplementary_group_count, 3);

        configured.credentials.effective = 1_u64 << 40;
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_state_install_configured(
                    (&configured as *const ConfigV2).cast::<Config>(),
                )
            },
            -1
        );
        let mut current = CredentialsOutput::default();
        let mut current_groups = [0_u32; 3];
        assert_eq!(
            unsafe {
                crate::darwin_art_bionic_process_state_read_credentials_core(
                    &mut current,
                    current_groups.as_mut_ptr(),
                    current_groups.len(),
                )
            },
            0
        );
        assert_eq!(current, output);
        assert_eq!(current_groups, [2000, 2001, 2002]);
        assert_eq!(
            crate::darwin_art_bionic_process_state_process_uninstall(),
            0
        );
        assert_eq!(crate::darwin_art_bionic_process_state_is_installed(), 0);
    }

    #[test]
    fn configured_v2_rejects_invalid_ids_and_capabilities() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let groups = [3000_u32];
        let mut invalid_id = config_v2(&groups);
        invalid_id.credentials.uid = u32::MAX;
        let mut invalid_caps = config_v2(&groups);
        invalid_caps.credentials.permitted = 1_u64 << 41;
        let mut invalid_effective = config_v2(&groups);
        invalid_effective.credentials.permitted = 1;
        invalid_effective.credentials.effective = 2;
        for config in [&invalid_id, &invalid_caps, &invalid_effective] {
            assert_eq!(
                unsafe {
                    darwin_art_bionic_process_state_install_configured(
                        (config as *const ConfigV2).cast::<Config>(),
                    )
                },
                -1
            );
            assert_eq!(crate::darwin_art_bionic_process_state_is_installed(), 0);
        }
    }

    #[test]
    fn configured_v1_is_uncredentialed_and_teardown_is_clean() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let config = config();
        assert_eq!(
            unsafe { darwin_art_bionic_process_state_install_configured(&config) },
            0
        );
        let mut output = CredentialsOutput::default();
        assert_eq!(
            unsafe {
                crate::darwin_art_bionic_process_state_read_credentials_core(
                    &mut output,
                    std::ptr::null_mut(),
                    0,
                )
            },
            -1
        );
        assert_eq!(
            crate::darwin_art_bionic_process_state_process_uninstall(),
            0
        );
        assert_eq!(crate::darwin_art_bionic_process_state_is_installed(), 0);
    }
}
