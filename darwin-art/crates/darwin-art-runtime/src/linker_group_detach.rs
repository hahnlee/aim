//! Eligible namespace detachment, not a public dlclose implementation.
//! Returned resources stay alive outside the registry lock. The Android owner
//! must still order finalization, dependency release and cascading group closes.
use super::*;
use std::sync::atomic::Ordering;

pub struct DetachedGroup {
    _images: Vec<Arc<NamespaceImage<ImageOwner>>>,
}

/// # Safety
/// Live registry/lease/output. Caller serializes the whole linker operation,
/// including flag/reference changes and later finalization. Returns 0 detached,
/// 1 retained by refs/flags, -4 unknown group contract, -1 invalid/retired.
/// This does not consume an open reference or run foreign code under the lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_detach(
    registry: *mut LinkerRegistry,
    lease: *const LinkerImageLease,
    output: *mut *mut DetachedGroup,
) -> i32 {
    unsafe { retire_group(registry, lease, output, None) }
}

pub type FinalizeGroup = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;

/// # Safety
/// Detachment ordering/quiescence contract applies. Callback receives original
/// root payload plus context outside registry locks, while membership remains.
/// Failure must mean no destructor side effects; it restores open admission.
/// Success commits detachment. Caller owns later physical release/cascade.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_finalize_and_detach(
    registry: *mut LinkerRegistry,
    lease: *const LinkerImageLease,
    finalize: Option<FinalizeGroup>,
    context: *mut c_void,
    output: *mut *mut DetachedGroup,
) -> i32 {
    if !output.is_null() {
        unsafe {
            *output = std::ptr::null_mut();
        }
    }
    let Some(finalize) = finalize else {
        return -1;
    };
    unsafe { retire_group(registry, lease, output, Some((finalize, context))) }
}

unsafe fn retire_group(
    registry: *mut LinkerRegistry,
    lease: *const LinkerImageLease,
    output: *mut *mut DetachedGroup,
    finalize: Option<(FinalizeGroup, *mut c_void)>,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = std::ptr::null_mut();
    }
    let (Some(registry), Some(lease)) = (unsafe { registry.as_ref() }, unsafe { lease.as_ref() })
    else {
        return -1;
    };
    let owner = &lease.0.lease;
    if !owner.group_dependencies_complete {
        return -4;
    }
    let (Some(group), Some(flags), Some(opens), Some(incoming), Some(retired)) = (
        owner.group,
        &owner.group_root_flags,
        &owner.group_opens,
        &owner.group_incoming,
        &owner.group_retired,
    ) else {
        return -4;
    };
    let Ok(mut state) = registry.0.lock() else {
        return -2;
    };
    let Ok(mut retired_ids) = registry.2.lock() else {
        return -2;
    };
    if retired.load(Ordering::Acquire) || retired_ids.contains(&group.id) {
        return -1;
    }
    let mut images = Vec::new();
    let mut contains_input = false;
    for image in state.resident_images() {
        if image
            .lease
            .group
            .is_some_and(|candidate| candidate.id == group.id)
        {
            contains_input |= Arc::ptr_eq(image, &lease.0);
            if !images.iter().any(|previous| Arc::ptr_eq(previous, image)) {
                images.push(image.clone());
            }
        }
    }
    if !contains_input {
        return -1;
    } // No cross-registry ID authorization.
    let Some(root) = images.iter().find(|image| {
        image
            .lease
            .group
            .is_some_and(|metadata| metadata.is_root != 0)
    }) else {
        return -4;
    };
    if flags.snapshot() != (false, false)
        || opens.load(Ordering::Acquire) != 0
        || incoming.load(Ordering::Acquire) != 0
    {
        return 1;
    }
    retired.store(true, Ordering::Release);
    // Reject an acquisition that raced its first retired check; never detach
    // an open that already completed before retirement became visible.
    if opens.load(Ordering::Acquire) != 0 || incoming.load(Ordering::Acquire) != 0 {
        retired.store(false, Ordering::Release);
        return 1;
    }
    if let Some((finalize, context)) = finalize {
        let root_payload = root.lease.value as *mut c_void;
        drop(retired_ids);
        drop(state);
        if unsafe { finalize(root_payload, context) } != 0 {
            retired.store(false, Ordering::Release);
            return -6;
        }
        state = match registry.0.lock() {
            Ok(state) => state,
            Err(_) => return -2,
        };
        retired_ids = match registry.2.lock() {
            Ok(ids) => ids,
            Err(_) => return -2,
        };
    }
    retired_ids.insert(group.id);
    state.unpublish_images(&images);
    drop(retired_ids);
    drop(state);
    unsafe {
        *output = Box::into_raw(Box::new(DetachedGroup { _images: images }));
    }
    0
}

/// # Safety
/// Destroy exactly once, outside registry locks, with native execution quiescent.
/// Other physical leases may defer finalizers; this is not forced unmapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_detached_group_destroy(group: *mut DetachedGroup) {
    if !group.is_null() {
        drop(unsafe { Box::from_raw(group) });
    }
}
