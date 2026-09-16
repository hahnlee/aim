//! Native image representation is independent of Android namespace policy.
//! Legacy opaque payloads cannot be consumed as typed native images.
use super::*;

/// # Safety
/// Live leases and writable output. Compare exact registry image ownership,
/// not payload addresses, path equality or SONAME strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_same(
    left: *const LinkerImageLease,
    right: *const LinkerImageLease,
    output: *mut i32,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let (Some(left), Some(right)) = (unsafe { left.as_ref() }, unsafe { right.as_ref() }) else {
        return -1;
    };
    unsafe { *output = i32::from(Arc::ptr_eq(&left.0, &right.0)) };
    0
}

/// # Safety
/// Live lease and writable output. Identity refers to the publishing registry;
/// it grants no visibility and must not be used against another/new registry.
/// Shared-child visibility does not change an image's primary namespace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_primary_namespace(
    lease: *const LinkerImageLease,
    output: *mut u64,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    unsafe { *output = lease.0.lease.primary_namespace };
    0
}

/// # Safety
/// Live registry, lease and output. Resolve the original publishing namespace
/// only if this exact image is still registered here. Caller serializes the
/// containing linker operation so the returned identity remains valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_registered_primary_namespace(
    registry: *const LinkerRegistry,
    lease: *const LinkerImageLease,
    output: *mut u64,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let (Some(registry), Some(lease)) = (unsafe { registry.as_ref() }, unsafe { lease.as_ref() })
    else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    if !state
        .resident_images()
        .any(|image| Arc::ptr_eq(image, &lease.0))
    {
        return -1;
    }
    unsafe { *output = lease.0.lease.primary_namespace };
    0
}

/// # Safety
/// Live registry, lease and writable output. Tests exact primary/secondary
/// membership, not namespace links or matching SONAME. A foreign/unpublished
/// identity or unknown namespace fails with a cleared output. The caller holds
/// the encompassing linker operation across membership and symbol lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_namespace_member(
    registry: *const LinkerRegistry,
    namespace: u64,
    lease: *const LinkerImageLease,
    output: *mut u8,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let (Some(registry), Some(lease)) = (unsafe { registry.as_ref() }, unsafe { lease.as_ref() })
    else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    if !state
        .resident_images()
        .any(|image| Arc::ptr_eq(image, &lease.0))
    {
        return -1;
    }
    match state.contains_image(NamespaceId::from_raw(namespace), &lease.0) {
        Ok(member) => {
            unsafe { *output = u8::from(member) };
            0
        }
        Err(_) => -1,
    }
}

/// # Safety
/// Input is a live borrowed lease. Result owns a new reference and must be
/// released once. No provider callback runs and no namespace visibility changes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_clone(
    lease: *const LinkerImageLease,
) -> *mut LinkerImageLease {
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(LinkerImageLease(
        Arc::clone(&lease.0),
        lease.1.clone(),
    )))
}

/// # Safety
/// Same publication contract as publish_flags. Retain must return a live
/// DarwinArtElfSelectedImage for kind=1, or a dlopen handle for kind=2, and
/// kind=3 carries the private Bionic provider image owner, kind=4 an ANGLE
/// dispatch image owner, kind=5 a NativeWindow facade owner, kind=6 the
/// process-owned ld-android linker ABI marker, and kind=7 a Vulkan dispatch
/// image owner, kind=8 the original graphics NDK dispatch marker
/// (none of these are dyld handles).
/// Release must consume
/// that representation. These are trusted host inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_image(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    path: *const c_char,
    source: *mut c_void,
    retain: Option<RetainImage>,
    release: Option<ReleaseImage>,
    flags_1: u64,
    global: u8,
    kind: u32,
) -> i32 {
    if !matches!(kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8) {
        return -1;
    }
    unsafe {
        publish_image(
            registry, id, soname, path, source, retain, release, flags_1, global, kind,
        )
    }
}

/// # Safety
/// Live lease, writable output; payload remains borrowed from lease.
/// Returns -4 for a different/legacy representation, without exposing payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_typed_payload(
    lease: *const LinkerImageLease,
    kind: u32,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    if lease.is_null() || !matches!(kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8) {
        return -1;
    }
    let owner = &unsafe { &*lease }.0.lease;
    if owner.kind != kind {
        return -4;
    }
    unsafe { *output = owner.value as *mut c_void };
    0
}
