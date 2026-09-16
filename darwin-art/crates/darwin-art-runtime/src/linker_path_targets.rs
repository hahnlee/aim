//! Direct path-link policy, separate from guest filesystem FD admission.
use super::*;
/// # Safety
/// Registry and NUL-terminated path are live; output writable. Caller holds
/// its root operation scope while iterating and trying the returned targets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_path_target(
    registry: *const LinkerRegistry,
    id: u64,
    path: *const c_char,
    index: usize,
    output: *mut u64,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = 0;
    }
    let (Some(registry), Some(path)) = (unsafe { registry.as_ref() }, unsafe { text(path) }) else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    match state.path_target(NamespaceId::from_raw(id), &path, index) {
        Ok(Some(target)) => {
            unsafe {
                *output = target.raw();
            }
            0
        }
        Ok(None) => 1,
        Err(_) => -1,
    }
}
