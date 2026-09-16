//! Borrow the placement decided by namespace admission, without re-searching.
use super::*;

/// # Safety
/// Context is live and not concurrently mutated. Outputs are writable and
/// non-aliasing. Path is borrowed until context destruction; copy before then.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_file_placement(
    context: *const LinkerDiscovery,
    image: u64,
    namespace: *mut u64,
    path: *mut *const c_char,
) -> i32 {
    if namespace.is_null() || path.is_null() {
        return -1;
    }
    unsafe {
        *namespace = 0;
        *path = std::ptr::null();
    }
    let Some(state) = (unsafe { context.as_ref() }) else {
        return -1;
    };
    let Some(index) = image.checked_sub(1).and_then(|id| usize::try_from(id).ok()) else {
        return -1;
    };
    let Some(entry) = state.images.get(index) else {
        return -1;
    };
    if entry.resident.is_some() {
        return -1;
    }
    unsafe {
        *namespace = entry.namespace;
        *path = entry.path.as_ptr();
    }
    0
}
