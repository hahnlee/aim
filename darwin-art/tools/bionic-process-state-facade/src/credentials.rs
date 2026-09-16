//! Immutable Android credentials owned by a configured process snapshot.
//!
//! This is a transport boundary only. It deliberately does not alter the
//! existing Darwin-backed getuid/setuid routes; a producer must first publish
//! this state through the configured snapshot ABI.

use crate::{
    ANDROID_E2BIG, ANDROID_EFAULT, ANDROID_EIO, CAPABILITY_FAILURE, active_snapshot, set_errno,
};
use std::ffi::c_int;
use std::ptr;

pub(crate) const MAX_SUPPLEMENTARY_GROUPS: usize = 65_536;
const INVALID_ID: u32 = u32::MAX;
const CAPABILITY_MASK: u64 = (1_u64 << 41) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CredentialIds {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CapabilitySets {
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

/// Trusted, immutable credentials copied from a host/service activation
/// request. The fields stay private so callers cannot mutate an installed
/// snapshot behind its owner/lifetime boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Credentials {
    uid: u32,
    euid: u32,
    suid: u32,
    gid: u32,
    egid: u32,
    sgid: u32,
    supplementary: Vec<u32>,
    permitted: u64,
    effective: u64,
    inheritable: u64,
}

impl Credentials {
    pub(crate) fn new(
        ids: CredentialIds,
        supplementary: Vec<u32>,
        capabilities: CapabilitySets,
    ) -> Result<Self, &'static str> {
        if [ids.uid, ids.euid, ids.suid, ids.gid, ids.egid, ids.sgid].contains(&INVALID_ID) {
            return Err("invalid Android credential ID");
        }
        if supplementary.len() > MAX_SUPPLEMENTARY_GROUPS || supplementary.contains(&INVALID_ID) {
            return Err("invalid supplementary group list");
        }
        if (capabilities.permitted | capabilities.effective | capabilities.inheritable)
            & !CAPABILITY_MASK
            != 0
        {
            return Err("unsupported Android capability bit");
        }
        if capabilities.effective & !capabilities.permitted != 0 {
            return Err("effective capabilities exceed permitted capabilities");
        }
        Ok(Self {
            uid: ids.uid,
            euid: ids.euid,
            suid: ids.suid,
            gid: ids.gid,
            egid: ids.egid,
            sgid: ids.sgid,
            supplementary,
            permitted: capabilities.permitted,
            effective: capabilities.effective,
            inheritable: capabilities.inheritable,
        })
    }

    pub(crate) fn supplementary_len(&self) -> usize {
        self.supplementary.len()
    }
}

/// Native readback shape. The supplementary groups themselves are returned in
/// the caller-provided buffer passed to
/// [`darwin_art_bionic_process_state_read_credentials_core`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CredentialsOutput {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub supplementary_group_count: usize,
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

impl Credentials {
    fn output(&self) -> CredentialsOutput {
        CredentialsOutput {
            uid: self.uid,
            euid: self.euid,
            suid: self.suid,
            gid: self.gid,
            egid: self.egid,
            sgid: self.sgid,
            supplementary_group_count: self.supplementary.len(),
            permitted: self.permitted,
            effective: self.effective,
            inheritable: self.inheritable,
        }
    }
}

fn invalid_output_pointer(output: *mut CredentialsOutput) -> bool {
    output.is_null() || !(output as usize).is_multiple_of(std::mem::align_of::<CredentialsOutput>())
}

fn invalid_group_pointer(groups: *mut u32, capacity: usize) -> bool {
    capacity != 0
        && (groups.is_null() || !(groups as usize).is_multiple_of(std::mem::align_of::<u32>()))
}

/// Read the active snapshot's credentials while retaining its `Arc` for the
/// entire copy. No output is written unless every pointer and capacity check
/// succeeds; in particular, a too-small group buffer leaves all outputs
/// untouched and returns `-1` with Android `E2BIG`.
///
/// # Safety
/// `output` must be writable for one [`CredentialsOutput`]. When
/// `group_capacity` is nonzero, `groups` must be writable for that many
/// `u32`s. The pointers must remain valid for this synchronous call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_bionic_process_state_read_credentials_core(
    output: *mut CredentialsOutput,
    groups: *mut u32,
    group_capacity: usize,
) -> c_int {
    if invalid_output_pointer(output) {
        set_errno(ANDROID_EFAULT);
        return -1;
    }
    if invalid_group_pointer(groups, group_capacity) {
        set_errno(ANDROID_EFAULT);
        return -1;
    }
    let Some(snapshot) = active_snapshot() else {
        crate::missing_snapshot();
        return -1;
    };
    let Some(credentials) = snapshot.credentials.as_ref() else {
        CAPABILITY_FAILURE.store(true, std::sync::atomic::Ordering::Release);
        set_errno(ANDROID_EIO);
        return -1;
    };
    if group_capacity < credentials.supplementary_len() {
        set_errno(ANDROID_E2BIG);
        return -1;
    }
    if credentials.supplementary_len() != 0 {
        // `group_capacity >= len` above and the pointer check establish the
        // exact writable span required for this copy.
        unsafe {
            ptr::copy_nonoverlapping(
                credentials.supplementary.as_ptr(),
                groups,
                credentials.supplementary_len(),
            );
        }
    }
    // SAFETY: the output pointer and full group span were checked above; all
    // source data remains owned by the active snapshot Arc held in `snapshot`.
    unsafe { output.write(credentials.output()) };
    0
}

/// Read only the scalar Android credential IDs from the active process
/// snapshot.  Identity queries such as getuid(2) must not depend on the size
/// of the supplementary-group list, so this is deliberately separate from
/// the bounded full-credential copy above.
///
/// # Safety
/// `output` must be writable for one [`CredentialsOutput`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_bionic_process_state_read_credential_ids_core(
    output: *mut CredentialsOutput,
) -> c_int {
    if invalid_output_pointer(output) {
        set_errno(ANDROID_EFAULT);
        return -1;
    }
    let Some(snapshot) = active_snapshot() else {
        crate::missing_snapshot();
        return -1;
    };
    let Some(credentials) = snapshot.credentials.as_ref() else {
        CAPABILITY_FAILURE.store(true, std::sync::atomic::Ordering::Release);
        set_errno(ANDROID_EIO);
        return -1;
    };
    // SAFETY: the output pointer was checked above and the copied value owns
    // no references into the snapshot.
    unsafe { output.write(credentials.output()) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuxSnapshot, SAFE_HWCAP, Snapshot, install_process_snapshot};
    use std::sync::Arc;

    fn credentials(groups: Vec<u32>) -> Credentials {
        Credentials::new(
            CredentialIds {
                uid: 10,
                euid: 11,
                suid: 12,
                gid: 20,
                egid: 21,
                sgid: 22,
            },
            groups,
            CapabilitySets {
                permitted: 0b111,
                effective: 0b011,
                inheritable: 0b100,
            },
        )
        .unwrap()
    }

    fn snapshot(credentials: Credentials) -> Arc<Snapshot> {
        Arc::new(
            Snapshot::new_with_credentials(
                Vec::new(),
                Vec::new(),
                AuxSnapshot {
                    page_size: 16_384,
                    hwcap: SAFE_HWCAP,
                    hwcap2: 0,
                    secure: false,
                    random: [0x51; 16],
                },
                credentials,
            )
            .unwrap(),
        )
    }

    struct OwnerGuard;

    impl Drop for OwnerGuard {
        fn drop(&mut self) {
            let _ = crate::uninstall_process_snapshot();
        }
    }

    #[test]
    fn credentials_validate_ids_groups_and_capability_relationships() {
        let ids = CredentialIds {
            uid: 1,
            euid: 1,
            suid: 1,
            gid: 1,
            egid: 1,
            sgid: 1,
        };
        assert!(
            Credentials::new(
                CredentialIds {
                    uid: INVALID_ID,
                    ..ids
                },
                Vec::new(),
                CapabilitySets {
                    permitted: 0,
                    effective: 0,
                    inheritable: 0
                }
            )
            .is_err()
        );
        assert!(
            Credentials::new(
                ids,
                vec![INVALID_ID],
                CapabilitySets {
                    permitted: 0,
                    effective: 0,
                    inheritable: 0
                }
            )
            .is_err()
        );
        assert!(
            Credentials::new(
                ids,
                Vec::new(),
                CapabilitySets {
                    permitted: 1_u64 << 41,
                    effective: 0,
                    inheritable: 0
                }
            )
            .is_err()
        );
        assert!(
            Credentials::new(
                ids,
                Vec::new(),
                CapabilitySets {
                    permitted: 1,
                    effective: 2,
                    inheritable: 0
                }
            )
            .is_err()
        );
        assert!(
            Credentials::new(
                ids,
                vec![0; MAX_SUPPLEMENTARY_GROUPS + 1],
                CapabilitySets {
                    permitted: 0,
                    effective: 0,
                    inheritable: 0,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn read_copies_groups_and_scalar_credentials() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let groups = vec![30, 31, 32];
        install_process_snapshot(snapshot(credentials(groups.clone()))).unwrap();
        let _owner = OwnerGuard;
        let mut output = CredentialsOutput::default();
        let mut copied = [0_u32; 3];
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_state_read_credentials_core(
                    &mut output,
                    copied.as_mut_ptr(),
                    copied.len(),
                )
            },
            0
        );
        assert_eq!(copied.as_slice(), groups.as_slice());
        assert_eq!(output.uid, 10);
        assert_eq!(output.euid, 11);
        assert_eq!(output.suid, 12);
        assert_eq!(output.gid, 20);
        assert_eq!(output.egid, 21);
        assert_eq!(output.sgid, 22);
        assert_eq!(output.supplementary_group_count, 3);
        assert_eq!(output.permitted, 0b111);
        assert_eq!(output.effective, 0b011);
        assert_eq!(output.inheritable, 0b100);
    }

    #[test]
    fn too_small_group_buffer_has_no_partial_writes() {
        let _test_owner_lock = crate::test_process_owner_guard();
        install_process_snapshot(snapshot(credentials(vec![30, 31]))).unwrap();
        let _owner = OwnerGuard;
        let sentinel = CredentialsOutput {
            uid: 0xa1,
            euid: 0xa2,
            suid: 0xa3,
            gid: 0xb1,
            egid: 0xb2,
            sgid: 0xb3,
            supplementary_group_count: 0xc1,
            permitted: 0xd1,
            effective: 0xd2,
            inheritable: 0xd3,
        };
        let mut output = sentinel;
        let mut copied = [0xfeed_beef_u32; 1];
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_state_read_credentials_core(
                    &mut output,
                    copied.as_mut_ptr(),
                    copied.len(),
                )
            },
            -1
        );
        assert_eq!(output, sentinel);
        assert_eq!(copied, [0xfeed_beef]);
    }

    #[test]
    fn scalar_identity_read_does_not_require_a_group_buffer() {
        let _test_owner_lock = crate::test_process_owner_guard();
        install_process_snapshot(snapshot(credentials(vec![30, 31]))).unwrap();
        let _owner = OwnerGuard;
        let mut output = CredentialsOutput::default();
        assert_eq!(
            unsafe { darwin_art_bionic_process_state_read_credential_ids_core(&mut output) },
            0
        );
        assert_eq!(output.uid, 10);
        assert_eq!(output.euid, 11);
        assert_eq!(output.gid, 20);
        assert_eq!(output.supplementary_group_count, 2);
    }

    #[test]
    fn v1_snapshot_has_no_credentials_and_teardown_clears_owner() {
        let _test_owner_lock = crate::test_process_owner_guard();
        install_process_snapshot(Arc::new(
            Snapshot::new(
                Vec::new(),
                Vec::new(),
                AuxSnapshot {
                    page_size: 16_384,
                    hwcap: SAFE_HWCAP,
                    hwcap2: 0,
                    secure: false,
                    random: [0x52; 16],
                },
            )
            .unwrap(),
        ))
        .unwrap();
        let _owner = OwnerGuard;
        let mut output = CredentialsOutput::default();
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_state_read_credentials_core(
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
