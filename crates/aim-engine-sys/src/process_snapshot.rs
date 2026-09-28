//! Raw ABI for the configured process-state snapshot owner.
//!
//! The pointed-to bytes are borrowed only for the duration of the
//! synchronous install call. The native owner copies them before returning;
//! no Rust allocation or slice crosses this boundary.

#[repr(C)]
pub struct ProcessSnapshotEntry {
    pub name: *const u8,
    pub name_size: usize,
    pub value: *const u8,
    pub value_size: usize,
}

#[repr(C)]
pub struct ProcessSnapshotConfig {
    pub abi_version: u32,
    pub struct_size: u32,
    pub environment: *const ProcessSnapshotEntry,
    pub environment_count: usize,
    pub properties: *const ProcessSnapshotEntry,
    pub property_count: usize,
    pub page_size: u64,
    pub hwcap: u64,
    pub hwcap2: u64,
    pub secure: u8,
    pub reserved: [u8; 7],
    pub random: [u8; 16],
}

pub type ProcessSnapshotInstallConfiguredFn =
    unsafe extern "C" fn(*const ProcessSnapshotConfig) -> i32;
pub type ProcessSnapshotUninstallFn = unsafe extern "C" fn() -> i32;

pub const PROCESS_SNAPSHOT_ABI_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn configured_snapshot_abi_matches_c_header() {
        assert_eq!(size_of::<ProcessSnapshotEntry>(), 32);
        assert_eq!(align_of::<ProcessSnapshotEntry>(), 8);
        assert_eq!(offset_of!(ProcessSnapshotEntry, name), 0);
        assert_eq!(offset_of!(ProcessSnapshotEntry, name_size), 8);
        assert_eq!(offset_of!(ProcessSnapshotEntry, value), 16);
        assert_eq!(offset_of!(ProcessSnapshotEntry, value_size), 24);

        assert_eq!(size_of::<ProcessSnapshotConfig>(), 88);
        assert_eq!(align_of::<ProcessSnapshotConfig>(), 8);
        assert_eq!(offset_of!(ProcessSnapshotConfig, abi_version), 0);
        assert_eq!(offset_of!(ProcessSnapshotConfig, struct_size), 4);
        assert_eq!(offset_of!(ProcessSnapshotConfig, environment), 8);
        assert_eq!(offset_of!(ProcessSnapshotConfig, environment_count), 16);
        assert_eq!(offset_of!(ProcessSnapshotConfig, properties), 24);
        assert_eq!(offset_of!(ProcessSnapshotConfig, property_count), 32);
        assert_eq!(offset_of!(ProcessSnapshotConfig, page_size), 40);
        assert_eq!(offset_of!(ProcessSnapshotConfig, hwcap), 48);
        assert_eq!(offset_of!(ProcessSnapshotConfig, hwcap2), 56);
        assert_eq!(offset_of!(ProcessSnapshotConfig, secure), 64);
        assert_eq!(offset_of!(ProcessSnapshotConfig, reserved), 65);
        assert_eq!(offset_of!(ProcessSnapshotConfig, random), 72);
    }

    #[test]
    fn configured_snapshot_function_pointers_are_data_sized() {
        assert_eq!(
            size_of::<ProcessSnapshotInstallConfiguredFn>(),
            size_of::<*mut c_void>()
        );
        assert_eq!(
            size_of::<ProcessSnapshotUninstallFn>(),
            size_of::<*mut c_void>()
        );
    }
}
