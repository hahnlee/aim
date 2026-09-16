//! Resident lookup uses the referring image's namespace, never a global list.
use super::*;
use std::os::unix::ffi::OsStrExt;

/// # Safety
/// Live non-reentrant discovery and writable output. Returns an owned lease
/// for the exact admitted ID, 1 for a file entry, negative for an invalid ID.
/// Does not search the registry again or expand namespace visibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_resident_image(
    context: *const LinkerDiscovery,
    image: u64,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(state) = (unsafe { context.as_ref() }) else {
        return -1;
    };
    let Some(origin) = image
        .checked_sub(1)
        .and_then(|id| usize::try_from(id).ok())
        .and_then(|index| state.images.get(index))
    else {
        return -1;
    };
    let Some(resident) = &origin.resident else {
        return 1;
    };
    unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(Arc::clone(resident), None))) };
    0
}

/// # Safety
/// Same lookup contract as find_resident; context is exclusively borrowed.
/// Returned ID belongs to this context and retains the actual resident image.
/// Its children use its primary namespace, not the namespace observing it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_admit_resident(
    context: *mut LinkerDiscovery,
    parent: u64,
    name: *const c_char,
    output: *mut *mut LinkerImageLease,
    image_id: *mut u64,
) -> i32 {
    if image_id.is_null() || output.is_null() {
        return -1;
    }
    unsafe {
        *image_id = 0;
        *output = std::ptr::null_mut();
    }
    let status =
        unsafe { darwin_art_linker_discovery_find_resident(context, parent, name, output) };
    if status != 0 {
        return status;
    }
    let lease = unsafe { Box::from_raw(*output) };
    unsafe {
        *output = std::ptr::null_mut();
    }
    let state = unsafe { &mut *context };
    let id = if let Some(index) = state.images.iter().position(|origin| {
        origin
            .resident
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &lease.0))
    }) {
        index + 1
    } else {
        let Ok(path) = CString::new(lease.0.path.as_os_str().as_bytes()) else {
            return -1;
        };
        state.images.push(ImageContext {
            namespace: lease.0.lease.primary_namespace,
            path,
            resident: Some(Arc::clone(&lease.0)),
        });
        state.images.len()
    };
    unsafe {
        *image_id = id as u64;
        *output = Box::into_raw(lease);
    }
    if !state.edges.contains(&(parent, id as u64)) {
        state.edges.push((parent, id as u64));
    }
    0
}

/// # Safety
/// Context is live and non-reentrant. Name is readable/NUL terminated; output
/// writable. On success the caller owns an image lease independently of this
/// discovery context and must retain it through use of any resolved address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_find_resident(
    context: *mut LinkerDiscovery,
    parent: u64,
    name: *const c_char,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(state) = (unsafe { context.as_ref() }) else {
        return -1;
    };
    let Some(index) = parent
        .checked_sub(1)
        .and_then(|id| usize::try_from(id).ok())
    else {
        return -1;
    };
    let Some(origin) = state.images.get(index) else {
        return -1;
    };
    unsafe { darwin_art_linker_namespace_find(state.registry, origin.namespace, name, output) }
}
