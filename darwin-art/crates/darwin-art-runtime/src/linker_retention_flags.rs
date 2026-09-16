//! Effective load-flag transport. Caller owns Android linker operation ordering;
//! these flags alone do not authorize group removal or native finalization.
use super::*;
/// # Safety
/// Live image lease and distinct writable output bytes. Queries the flags of
/// the original local-group root, as used by Android soinfo_unload. This alone
/// does not authorize unload: linked state and reference counts remain separate.
/// Legacy publications return -4 because their original group is unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_group_retention_flags(
    lease: *const LinkerImageLease,
    global: *mut u8,
    nodelete: *mut u8,
) -> i32 {
    if global.is_null() || nodelete.is_null() || global == nodelete {
        return -1;
    }
    unsafe {
        *global = 0;
        *nodelete = 0;
    }
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    let Some(flags) = &lease.0.lease.group_root_flags else {
        return -4;
    };
    let (root_global, root_nodelete) = flags.snapshot();
    unsafe {
        *global = u8::from(root_global);
        *nodelete = u8::from(root_nodelete);
    }
    0
}
/// # Safety
/// Live image lease. Promotion is permanent for this original mapped image.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_promote_nodelete(
    lease: *const LinkerImageLease,
) -> i32 {
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    lease.0.visibility.promote_nodelete();
    0
}
/// # Safety
/// Live lease, writable distinct output bytes. Returns effective Android flags,
/// not Darwin dlopen constants and not an unload-eligibility decision.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_retention_flags(
    lease: *const LinkerImageLease,
    global: *mut u8,
    nodelete: *mut u8,
) -> i32 {
    if global.is_null() || nodelete.is_null() || global == nodelete {
        return -1;
    }
    unsafe {
        *global = 0;
        *nodelete = 0;
    }
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    let flags = lease.0.visibility.retention_flags();
    unsafe {
        *global = u8::from(flags.0);
        *nodelete = u8::from(flags.1);
    }
    0
}
