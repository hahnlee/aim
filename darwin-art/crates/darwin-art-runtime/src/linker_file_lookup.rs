//! AOSP file identity lookup: same inode/device/offset and direct namespace
//! visibility. Zero identity never aliases an unknown or native-only image.
use super::*;
/// # Safety
/// Registry live, output writable; returned lease independently owned. Identity
/// comes from the admitted descriptor, not a pathname stat/reopen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_find_file(
    registry: *const LinkerRegistry,
    id: u64,
    device: u64,
    inode: u64,
    offset: u64,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    unsafe {
        darwin_art_linker_namespace_find_file_scoped(registry, id, device, inode, offset, 1, output)
    }
}

/// # Safety
/// Same owned-output contract as find_file. search_links is 0 or 1; linked
/// namespace load attempts use 0 to prevent transitive re-export.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_find_file_scoped(
    registry: *const LinkerRegistry,
    id: u64,
    device: u64,
    inode: u64,
    offset: u64,
    search_links: u8,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    if search_links > 1 {
        return -1;
    }
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    match state.find_matching_scoped(NamespaceId::from_raw(id), search_links != 0, |image| {
        device != 0 && inode != 0 && image.lease.file_identity == Some((device, inode, offset))
    }) {
        Ok(Some(image)) => {
            unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(image, None))) };
            0
        }
        Ok(None) => 1,
        Err(_) => -1,
    }
}
