//! Explicit trusted credentials attached to process snapshot ABI version 2.
use crate::ProcessSnapshotConfig;

#[repr(C)]
pub struct ProcessCredentialsConfig {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub groups: *const u32,
    pub group_count: usize,
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

#[repr(C)]
pub struct ProcessSnapshotConfigV2 {
    pub base: ProcessSnapshotConfig,
    pub credentials: ProcessCredentialsConfig,
}

pub const PROCESS_SNAPSHOT_CREDENTIALS_ABI_VERSION: u32 = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn credentials_and_extended_snapshot_layout() {
        assert_eq!(size_of::<ProcessCredentialsConfig>(), 64);
        assert_eq!(align_of::<ProcessCredentialsConfig>(), 8);
        assert_eq!(offset_of!(ProcessCredentialsConfig, uid), 0);
        assert_eq!(offset_of!(ProcessCredentialsConfig, sgid), 20);
        assert_eq!(offset_of!(ProcessCredentialsConfig, groups), 24);
        assert_eq!(offset_of!(ProcessCredentialsConfig, group_count), 32);
        assert_eq!(offset_of!(ProcessCredentialsConfig, permitted), 40);
        assert_eq!(offset_of!(ProcessCredentialsConfig, effective), 48);
        assert_eq!(offset_of!(ProcessCredentialsConfig, inheritable), 56);
        assert_eq!(size_of::<ProcessSnapshotConfigV2>(), 152);
        assert_eq!(offset_of!(ProcessSnapshotConfigV2, base), 0);
        assert_eq!(offset_of!(ProcessSnapshotConfigV2, credentials), 88);
    }
}
