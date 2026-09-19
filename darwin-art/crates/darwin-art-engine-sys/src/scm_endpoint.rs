//! Raw private SCM endpoint installation boundary. No guest policy or ownership.
use core::ffi::c_void;

pub const SCM_ENDPOINT_ABI_VERSION: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmPairOfferV1 {
    pub authority: [u8; 16],
    pub carrier: u64,
    pub holder_a: [u8; 16],
    pub holder_b: [u8; 16],
}

pub type ScmPairInstallFn = unsafe extern "C" fn(*mut c_void, *const ScmPairOfferV1) -> i32;
pub type ScmPairClearFn = unsafe extern "C" fn(*mut c_void);

/// Borrowed synchronous target for TWO unpublished native endpoint objects.
/// Install must be allocation/RPC-free and no-unwind. Clear rolls back both
/// objects WITHOUT releasing daemon grants (the Rust operation owns those
/// until confirmed success). A caller-owned receipt retains rollback through
/// subsequent guest publication; a boolean-success fixture is not this owner.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmPairInstallerV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub target: *mut c_void,
    pub install: Option<ScmPairInstallFn>,
    pub clear: Option<ScmPairClearFn>,
}

pub const SCM_MAX_PAYLOADS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct ScmGrantV2 {
    pub authority: [u8; 16],
    pub carrier: u64,
    pub holder: [u8; 16],
    pub side: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmManagedPayloadV2 {
    pub ordinal: u64,
    pub holder: [u8; 16],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmPrepareRequestV2 {
    pub carrier_holder: [u8; 16],
    pub payload_fds: *const i32,
    pub payload_count: u32,
    pub managed_count: u32,
    pub managed: *const ScmManagedPayloadV2,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmPreparedV2 {
    pub authority: [u8; 16],
    pub ticket: u64,
    pub metadata_fd: i32,
    pub guardian_fd: i32,
    pub payload_count: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmAdmitRequestV2 {
    pub carrier_holder: [u8; 16],
    pub metadata_fd: i32,
    pub payload_count: u32,
    pub publish_ordinals: *const u64,
    pub publish_count: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScmClaimV2 {
    pub ordinal: u64,
    pub grant: ScmGrantV2,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScmCredentialsV2 {
    pub process_id: i32,
    pub user_id: u32,
    pub group_id: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ScmAdmissionV2 {
    pub authority: [u8; 16],
    pub ticket: u64,
    pub claim_count: u32,
    pub reserved: u32,
    pub credentials: ScmCredentialsV2,
    pub claims: [ScmClaimV2; SCM_MAX_PAYLOADS],
}

pub type ScmPrepareFn =
    unsafe extern "C" fn(*mut c_void, *const ScmPrepareRequestV2, *mut ScmPreparedV2) -> i32;
pub type ScmAdmitFn =
    unsafe extern "C" fn(*mut c_void, *const ScmAdmitRequestV2, *mut ScmAdmissionV2) -> i32;
pub type ScmSettleFn = unsafe extern "C" fn(*mut c_void, *const u8, u64, u32) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmBinderBindingV2 {
    pub source_connection: u64,
    pub transfer: u64,
    pub ordinal: u64,
    pub object_offset: u64,
}
pub type ScmBinderBindFn =
    unsafe extern "C" fn(*mut c_void, *const u8, *const ScmBinderBindingV2, *mut u8) -> i32;
pub type ScmBinderCancelFn = unsafe extern "C" fn(*mut c_void, *const ScmBinderBindingV2) -> i32;
pub type ScmBinderClaimFn = unsafe extern "C" fn(
    *mut c_void,
    *const ScmBinderBindingV2,
    *const u8,
    u32,
    *mut ScmGrantV2,
) -> i32;

pub type ScmContextRetainFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
pub type ScmContextReleaseFn = unsafe extern "C" fn(*mut c_void);
pub type ScmRegisterPairFn = unsafe extern "C" fn(*mut c_void, *const ScmPairInstallerV1) -> i32;
pub type ScmReleaseHolderFn = unsafe extern "C" fn(*mut c_void, *const u8) -> i32;
pub type ScmEndpointProviderInstallFn = unsafe extern "C" fn(*const ScmEndpointProviderV1) -> i32;
pub type ScmEndpointProviderUninstallFn = unsafe extern "C" fn() -> i32;
pub type SocketBrokerIsActiveFn = unsafe extern "C" fn() -> i32;

/// Versioned inert provider contract. Native installation copies this table
/// and retains context; operations/final endpoint cleanup retain their own
/// references. Stop acquisition/drain BEFORE context or runtime image teardown.
/// Operation status is 0 or a positive native errno, never a fabricated ACK.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScmEndpointProviderV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub context: *mut c_void,
    pub retain: Option<ScmContextRetainFn>,
    pub release: Option<ScmContextReleaseFn>,
    pub register_pair: Option<ScmRegisterPairFn>,
    pub release_holder: Option<ScmReleaseHolderFn>,
    pub prepare: Option<ScmPrepareFn>,
    pub admit: Option<ScmAdmitFn>,
    pub settle: Option<ScmSettleFn>,
    pub bind_binder: Option<ScmBinderBindFn>,
    pub cancel_binder: Option<ScmBinderCancelFn>,
    pub claim_binder: Option<ScmBinderClaimFn>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pair_installation_layout_is_native_arm64_contract() {
        assert_eq!(core::mem::size_of::<ScmGrantV2>(), 48);
        assert_eq!(core::mem::size_of::<ScmPrepareRequestV2>(), 40);
        assert_eq!(core::mem::size_of::<ScmPreparedV2>(), 40);
        assert_eq!(core::mem::size_of::<ScmAdmitRequestV2>(), 40);
        assert_eq!(core::mem::size_of::<ScmCredentialsV2>(), 12);
        assert_eq!(core::mem::size_of::<ScmAdmissionV2>(), 944);
        assert_eq!(core::mem::size_of::<ScmPairOfferV1>(), 56);
        assert_eq!(core::mem::size_of::<ScmPairInstallerV1>(), 32);
        assert_eq!(core::mem::size_of::<ScmEndpointProviderV1>(), 96);
        assert_eq!(core::mem::offset_of!(ScmPairOfferV1, holder_a), 24);
    }
}
