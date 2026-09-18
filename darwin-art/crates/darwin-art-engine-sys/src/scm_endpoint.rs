//! Raw private SCM endpoint installation boundary. No guest policy or ownership.
use core::ffi::c_void;

pub const SCM_ENDPOINT_ABI_VERSION: u32 = 1;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pair_installation_layout_is_native_arm64_contract() {
        assert_eq!(core::mem::size_of::<ScmPairOfferV1>(), 56);
        assert_eq!(core::mem::size_of::<ScmPairInstallerV1>(), 32);
        assert_eq!(core::mem::size_of::<ScmEndpointProviderV1>(), 48);
        assert_eq!(core::mem::offset_of!(ScmPairOfferV1, holder_a), 24);
    }
}
