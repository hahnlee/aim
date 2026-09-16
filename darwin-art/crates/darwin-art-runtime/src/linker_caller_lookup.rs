//! Published-image caller lookup. Representation-specific address tests execute
//! on retained originals outside the registry lock, never on name replacements.
use super::*;

pub type ContainsAddress = unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> i32;

/// # Safety
/// Live registry/output and callback context. Caller owns the encompassing
/// linker operation. Callback returns 1 match, 0 miss, negative failure and
/// must not mutate publication. Only the selected representation is inspected.
/// Returns 0 + owned non-counting image, 1 absent, -5 ambiguous, -3 changed,
/// -6 callback failure. No callback or native release runs under the mutex.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_find_image_address(
    registry: *const LinkerRegistry,
    kind: u32,
    address: usize,
    contains: Option<ContainsAddress>,
    context: *mut c_void,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let (Some(registry), Some(contains)) = (unsafe { registry.as_ref() }, contains) else {
        return -1;
    };
    let images = {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        let mut images = Vec::new();
        for image in state
            .resident_images()
            .filter(|image| image.lease.kind == kind)
        {
            if !images.iter().any(|old| Arc::ptr_eq(old, image)) {
                images.push(image.clone());
            }
        }
        images
    };
    let mut found = None;
    for image in &images {
        match unsafe { contains(image.lease.value as *mut c_void, address, context) } {
            0 => {}
            1 => {
                if found.is_some() {
                    return -5;
                }
                found = Some(image.clone());
            }
            _ => return -6,
        }
    }
    let Some(found) = found else {
        return 1;
    };
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        if !state
            .resident_images()
            .any(|image| Arc::ptr_eq(image, &found))
        {
            return -3;
        }
    }
    unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(found, None))) };
    0
}
