//! Byte-preserving default namespace path query for Bionic's linker ABI.
use super::*;
use std::os::unix::ffi::OsStrExt;

/// # Safety
/// Registry live, required writable; buffer writable for capacity bytes if nonnull.
/// Returns 0 copied, 1 size query/insufficient capacity, negative error. Required
/// includes NUL. No partial output on undersize; never consults host environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_default_paths(
    registry: *mut LinkerRegistry,
    id: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> i32 {
    if required.is_null() {
        return -1;
    }
    unsafe {
        *required = 0;
    }
    if registry.is_null() {
        return -1;
    }
    let paths = {
        let Ok(state) = (unsafe { &*registry }).0.lock() else {
            return -2;
        };
        match state.default_library_paths(NamespaceId::from_raw(id)) {
            Ok(paths) => paths,
            Err(_) => return -1,
        }
    };
    let mut bytes = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            bytes.push(b':');
        }
        bytes.extend_from_slice(path.as_os_str().as_bytes());
    }
    bytes.push(0);
    unsafe {
        *required = bytes.len();
    }
    if buffer.is_null() || capacity < bytes.len() {
        return 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_paths_exclude_search_paths_and_do_not_partially_write() {
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let mut id = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    0,
                    c"/override".as_ptr(),
                    c"/system/lib64:/vendor/lib64".as_ptr(),
                    std::ptr::null(),
                    0,
                    &mut id
                ),
                0
            );
            let mut needed = 0;
            assert_eq!(
                darwin_art_linker_default_paths(registry, id, std::ptr::null_mut(), 0, &mut needed),
                1
            );
            let mut output = vec![0x55; needed];
            assert_eq!(
                darwin_art_linker_default_paths(
                    registry,
                    id,
                    output.as_mut_ptr(),
                    needed - 1,
                    &mut needed
                ),
                1
            );
            assert!(output.iter().all(|b| *b == 0x55));
            assert_eq!(
                darwin_art_linker_default_paths(
                    registry,
                    id,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut needed
                ),
                0
            );
            assert_eq!(output, b"/system/lib64:/vendor/lib64\0");
            assert_eq!(
                darwin_art_linker_default_paths(
                    registry,
                    0,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut needed
                ),
                -1
            );
            assert_eq!(needed, 0);
            darwin_art_linker_registry_destroy(registry);
        }
    }
}
