//! Android libdl request dispatch. Namespace/file/flag policy belongs to the
//! bound linker owner; a platform provider must not erase that request.
use super::*;

// Android arm64 bionic libdl ABI (libc/include/dlfcn.h), not Darwin values.
const RTLD_LAZY: c_int = 0x1;
const RTLD_NOW: c_int = 0x2;
const RTLD_GLOBAL: c_int = 0x100;
const RTLD_NODELETE: c_int = 0x1000;

/// Flags a resident built-in provider satisfies without linker state: any
/// binding mode (bionic always binds eagerly), RTLD_LOCAL/RTLD_GLOBAL (the
/// provider is already visible to every image naming it) and RTLD_NODELETE
/// (it lives for the process). RTLD_NOLOAD and unknown bits are not provider
/// policy; they reach the bound linker, which validates or rejects them.
fn resident_provider_flags(flags: c_int) -> bool {
    flags & !(RTLD_LAZY | RTLD_NOW | RTLD_GLOBAL | RTLD_NODELETE) == 0
}

fn legacy_provider_request(name: &[u8], flags: c_int, extension_flags: u64) -> bool {
    // Existing bare provider tokens remain migration debt. Explicit paths,
    // namespace/FD/reservation requests and flags a resident provider cannot
    // satisfy must reach the linker with their original identity.
    !name.contains(&b'/') && resident_provider_flags(flags) && extension_flags == 0
}

fn canonical_system_graphics_provider(name: &[u8]) -> Option<*mut c_void> {
    match name {
        b"/system/lib64/libEGL.so" => virtual_graphics_handle(b"libEGL.so"),
        b"/system/lib64/libGLESv2.so" | b"/system/lib64/libGLESv3.so" => {
            virtual_graphics_handle(b"libGLESv2.so")
        }
        _ => None,
    }
}

/// Opens an Android DSO with the original extended-loader parameters.
/// # Safety
/// filename is null or a C string; extinfo is null or readable during callback.
#[no_mangle]
pub unsafe extern "C" fn darwin_art_bionic_android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const AndroidDlExtInfo,
) -> *mut c_void {
    let extension_flags = unsafe { extinfo.as_ref() }.map_or(0, |info| info.flags);
    if !filename.is_null() {
        let name = unsafe { CStr::from_ptr(filename) }.to_bytes();
        if legacy_provider_request(name, flags, extension_flags) {
            if name == b"libandroid.so" {
                return libandroid_handle();
            }
            if name == b"libnativewindow.so" {
                return libnativewindow_handle();
            }
            if let Some(handle) = virtual_graphics_handle(name) {
                return handle;
            }
        }
        // Ported system graphics images have an authoritative Android path but
        // no guest inode. Keep the exact canonical path inside the Android
        // libdl provider instead of passing it to a transitional callback that
        // may eventually reach macOS dyld. Aliases and extended requests stay
        // on the regular linker path.
        if resident_provider_flags(flags) && extension_flags == 0 {
            if let Some(handle) = canonical_system_graphics_provider(name) {
                return handle;
            }
        }
    }
    let Some(loader) = loader() else {
        return ptr::null_mut();
    };
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let result = unsafe {
        (loader.open.expect("validated"))(
            loader.context,
            filename,
            flags,
            extinfo,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if result.is_null() {
        let message = call_error(&error);
        set_error(if message.is_empty() {
            "Android dlopen failed"
        } else {
            &message
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_loader_contract_never_becomes_a_bare_provider() {
        // bionic RTLD_LOCAL/LAZY/NOW combinations plus GLOBAL/NODELETE.
        for flags in [0, 1, 2, 3, 0x100, 0x102, 0x1002, 0x1102] {
            assert!(legacy_provider_request(b"libEGL.so", flags, 0));
        }
        // RTLD_NOLOAD and unknown bits are linker policy, never provider hits.
        for flags in [4, 0x104, 0x8, 0x200, 0x2002, -1] {
            assert!(!legacy_provider_request(b"libEGL.so", flags, 0));
        }
        for extensions in [1, 0x10, 0x200, u64::MAX] {
            assert!(!legacy_provider_request(b"libEGL.so", 2, extensions));
        }
        for path in [b"/private/libEGL.so".as_slice(), b"other/libEGL.so"] {
            assert!(!legacy_provider_request(path, 2, 0));
        }
    }

    #[test]
    fn only_canonical_system_graphics_paths_have_virtual_identity() {
        assert!(!canonical_system_graphics_provider(b"/system/lib64/libEGL.so").is_none());
        assert!(!canonical_system_graphics_provider(b"/system/lib64/libGLESv3.so").is_none());
        for path in [
            b"/system/lib64/./libEGL.so".as_slice(),
            b"/system/lib64/libEGL.so/",
            b"/vendor/lib64/libEGL.so",
            b"/private/libEGL.so",
        ] {
            assert!(canonical_system_graphics_provider(path).is_none());
        }
    }
}
