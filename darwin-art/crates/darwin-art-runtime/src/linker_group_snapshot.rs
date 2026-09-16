//! Owned, ordered image-group snapshot for native load operations.
use super::*;
use std::ffi::CString;

pub struct LinkerImageGroup(Vec<(LinkerImageLease, CString)>);

/// # Safety
/// Live registry, writable output under a complete linker operation. Retains
/// only this namespace's ordered list, including local and inherited images.
/// Does not follow links, merge same-SONAME identities or grant visibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_image_snapshot(
    registry: *const LinkerRegistry,
    id: u64,
    output: *mut *mut LinkerImageGroup,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = std::ptr::null_mut();
    }
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    let images = {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        match state.ordered_images(NamespaceId::from_raw(id)) {
            Ok(images) => images,
            Err(_) => return -1,
        }
    };
    let entries = images
        .into_iter()
        .map(|image| {
            let name = CString::new(image.soname.as_bytes()).expect("validated SONAME");
            (LinkerImageLease(image, None), name)
        })
        .collect();
    unsafe {
        *output = Box::into_raw(Box::new(LinkerImageGroup(entries)));
    }
    0
}

/// # Safety
/// Live registry and writable output under the caller's linker operation.
/// Retains each exact resident once, including non-global images. This grants
/// no symbol visibility; callers must apply namespace access independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_registry_image_snapshot(
    registry: *const LinkerRegistry,
    output: *mut *mut LinkerImageGroup,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    let images = {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        let mut images = Vec::new();
        for image in state.resident_images() {
            if !images.iter().any(|previous| Arc::ptr_eq(previous, image)) {
                images.push(image.clone());
            }
        }
        images
    };
    let entries = images
        .into_iter()
        .map(|image| {
            let name = CString::new(image.soname.as_bytes()).expect("validated SONAME");
            (LinkerImageLease(image, None), name)
        })
        .collect();
    unsafe { *output = Box::into_raw(Box::new(LinkerImageGroup(entries))) };
    0
}

/// # Safety
/// Live snapshot and writable output. Returns an independent non-counting
/// image lease (0), end (1), or invalid input (-1); always clears output first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_image(
    group: *const LinkerImageGroup,
    index: usize,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(group) = (unsafe { group.as_ref() }) else {
        return -1;
    };
    let Some((lease, _)) = group.0.get(index) else {
        return 1;
    };
    unsafe { *output = image_kind::darwin_art_linker_image_clone(lease) };
    0
}

/// # Safety
/// Registry is live; output writable/non-aliasing. global_only=1 selects
/// DF_1_GLOBAL, 0 selects RTLD_GLOBAL. Caller serializes linker mutations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_snapshot(
    registry: *mut LinkerRegistry,
    id: u64,
    global_only: u8,
    output: *mut *mut LinkerImageGroup,
) -> i32 {
    if registry.is_null() || output.is_null() || global_only > 1 {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let images = {
        let Ok(state) = (unsafe { &*registry }).0.lock() else {
            return -2;
        };
        match state.shared_group(NamespaceId::from_raw(id), global_only != 0) {
            Ok(images) => images,
            Err(_) => return -1,
        }
    };
    // Strings and resources now belong to snapshot; no registry borrow remains.
    let entries = images
        .into_iter()
        .map(|image| {
            let name = CString::new(image.soname.as_bytes()).expect("validated SONAME");
            (LinkerImageLease(image, None), name)
        })
        .collect();
    unsafe { *output = Box::into_raw(Box::new(LinkerImageGroup(entries))) };
    0
}

/// # Safety
/// Group remains live while returned borrowed payload/name is used. Outputs
/// are writable and do not alias. Returns 1 at end, -1 invalid arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_item(
    group: *const LinkerImageGroup,
    index: usize,
    payload: *mut *mut c_void,
    soname: *mut *const c_char,
) -> i32 {
    if group.is_null() || payload.is_null() || soname.is_null() {
        return -1;
    }
    unsafe {
        *payload = std::ptr::null_mut();
        *soname = std::ptr::null();
    }
    let Some((lease, name)) = (unsafe { &*group }).0.get(index) else {
        return 1;
    };
    unsafe {
        *payload = lease.0.lease.value as *mut c_void;
        *soname = name.as_ptr();
    }
    0
}

/// # Safety
/// Group must stay live through use of the borrowed payload; output writable.
/// Returns 1 at end or -4 on representation mismatch, clearing output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_typed_item(
    group: *const LinkerImageGroup,
    index: usize,
    kind: u32,
    payload: *mut *mut c_void,
) -> i32 {
    if payload.is_null() {
        return -1;
    }
    unsafe { *payload = std::ptr::null_mut() };
    if group.is_null() || !matches!(kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8) {
        return -1;
    }
    let Some((lease, _)) = (unsafe { &*group }).0.get(index) else {
        return 1;
    };
    unsafe { image_kind::darwin_art_linker_image_typed_payload(lease, kind, payload) }
}

/// # Safety
/// Transfer group once, after its borrowed item users have drained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_group_destroy(group: *mut LinkerImageGroup) {
    if !group.is_null() {
        drop(unsafe { Box::from_raw(group) });
    }
}
