use darwin_art_engine_sys::NativeLoaderConfig;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Validation failures for the optional NativeLoader input are intentionally
/// separate from `ProcessRequestError`, so existing host error matching stays
/// source-compatible.
#[derive(Debug, Eq, PartialEq)]
pub enum InputError {
    NotAbsolute(&'static str),
    InteriorNul(&'static str),
}

/// Owns all strings referenced by one NativeLoader configuration and keeps the
/// boxed wire object at a stable address. Moving this value therefore cannot
/// invalidate any pointer in the configuration passed to native code.
pub struct NativeLoaderInput {
    _linker_config_path: CString,
    _executable_path: CString,
    _library_search_path: CString,
    _android_unwind_path: CString,
    wire: Box<NativeLoaderConfig>,
}

impl NativeLoaderInput {
    pub fn new(
        linker_config_path: &Path,
        executable_path: &Path,
        library_search_path: &OsStr,
        android_unwind_path: &Path,
    ) -> Result<Self, InputError> {
        let linker_config_path = absolute_c_string(linker_config_path, "linker_config_path")?;
        let executable_path = absolute_c_string(executable_path, "executable_path")?;
        let library_search_path = c_string(library_search_path, "library_search_path")?;
        let android_unwind_path = absolute_c_string(android_unwind_path, "android_unwind_path")?;
        let wire = Box::new(NativeLoaderConfig::new(
            linker_config_path.as_ptr(),
            executable_path.as_ptr(),
            library_search_path.as_ptr(),
            android_unwind_path.as_ptr(),
        ));
        Ok(Self {
            _linker_config_path: linker_config_path,
            _executable_path: executable_path,
            _library_search_path: library_search_path,
            _android_unwind_path: android_unwind_path,
            wire,
        })
    }

    pub(crate) fn as_ptr(&self) -> *const NativeLoaderConfig {
        self.wire.as_ref()
    }

    #[cfg(test)]
    fn config(&self) -> &NativeLoaderConfig {
        self.wire.as_ref()
    }
}

fn absolute_c_string(path: &Path, field: &'static str) -> Result<CString, InputError> {
    if !path.is_absolute() {
        return Err(InputError::NotAbsolute(field));
    }
    c_string(path.as_os_str(), field)
}

fn c_string(value: &OsStr, field: &'static str) -> Result<CString, InputError> {
    CString::new(value.as_bytes()).map_err(|_| InputError::InteriorNul(field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, OsString};
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    fn path(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    fn input() -> NativeLoaderInput {
        NativeLoaderInput::new(
            &path("/system/etc/ld.config.txt"),
            &path("/system/bin/app_process64"),
            OsStr::new("/system/lib64:/vendor/lib64"),
            &path("/runtime/libdarwin_art_android_unwind.so"),
        )
        .unwrap()
    }

    #[test]
    fn owns_wire_strings_and_move_preserves_pointers() {
        let value = input();
        let before = value.config();
        let pointers = (
            before.linker_config_path,
            before.executable_path,
            before.library_search_path,
            before.android_unwind_path,
            value.as_ptr(),
        );
        let moved = value;
        let after = moved.config();
        assert_eq!(moved.as_ptr(), pointers.4);
        assert_eq!(after.linker_config_path, pointers.0);
        assert_eq!(after.executable_path, pointers.1);
        assert_eq!(after.library_search_path, pointers.2);
        assert_eq!(after.android_unwind_path, pointers.3);
        assert_eq!(
            unsafe { CStr::from_ptr(after.linker_config_path) }.to_bytes(),
            b"/system/etc/ld.config.txt"
        );
        assert_eq!(
            unsafe { CStr::from_ptr(after.executable_path) }.to_bytes(),
            b"/system/bin/app_process64"
        );
        assert_eq!(
            unsafe { CStr::from_ptr(after.library_search_path) }.to_bytes(),
            b"/system/lib64:/vendor/lib64"
        );
        assert_eq!(
            unsafe { CStr::from_ptr(after.android_unwind_path) }.to_bytes(),
            b"/runtime/libdarwin_art_android_unwind.so"
        );
        assert_eq!(
            after.header.abi_version,
            darwin_art_engine_sys::NATIVE_LOADER_CONFIG_ABI_VERSION
        );
    }

    #[test]
    fn validates_absolute_required_paths_and_nul_search_path() {
        assert!(matches!(
            NativeLoaderInput::new(
                &path("relative/ld.config"),
                &path("/system/bin/app_process64"),
                OsStr::new("/system/lib64"),
                &path("/runtime/libdarwin_art_android_unwind.so"),
            ),
            Err(InputError::NotAbsolute("linker_config_path"))
        ));
        assert!(matches!(
            NativeLoaderInput::new(
                &path("/system/etc/ld.config.txt"),
                &path("app_process64"),
                OsStr::new("/system/lib64"),
                &path("/runtime/libdarwin_art_android_unwind.so"),
            ),
            Err(InputError::NotAbsolute("executable_path"))
        ));
        assert!(matches!(
            NativeLoaderInput::new(
                &path("/system/etc/ld.config.txt"),
                &path("/system/bin/app_process64"),
                OsStr::new("/system/lib64\0/vendor/lib64"),
                &path("/runtime/libdarwin_art_android_unwind.so"),
            ),
            Err(InputError::InteriorNul("library_search_path"))
        ));
        assert!(matches!(
            NativeLoaderInput::new(
                &path("/system/etc/ld.config.txt"),
                &path("/system/bin/app_process64"),
                OsStr::new("/system/lib64"),
                &path("libdarwin_art_android_unwind.so"),
            ),
            Err(InputError::NotAbsolute("android_unwind_path"))
        ));
    }

    #[test]
    fn preserves_non_utf8_path_bytes() {
        let executable = PathBuf::from(OsString::from_vec(b"/system/bin/app_process\xff".to_vec()));
        let value = NativeLoaderInput::new(
            &path("/system/etc/ld.config.txt"),
            &executable,
            OsStr::new("/system/lib64"),
            &path("/runtime/libdarwin_art_android_unwind.so"),
        )
        .unwrap();
        let bytes = unsafe { CStr::from_ptr(value.config().executable_path) };
        assert_eq!(bytes.to_bytes(), b"/system/bin/app_process\xff");
    }
}
