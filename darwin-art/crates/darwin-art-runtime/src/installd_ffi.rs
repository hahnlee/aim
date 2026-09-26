//! installd transport FFI for the system process. The Java `IInstalld`
//! binder decodes AIDL; the profile daemon performs the filesystem work.
use darwin_art_profile::{InstalldRequest, installd_at};
use std::ffi::{CStr, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

// Return values: 0 success, 1 installd failure (`errno` set to the Android
// errno), -1 invalid arguments, -2 transport failure, -4 internal panic.
const FAILED: i32 = 1;
const INVALID: i32 = -1;
const TRANSPORT: i32 = -2;
const PANIC: i32 = -4;

unsafe fn text<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .filter(|s| !s.is_empty())
}

fn run(
    socket: *const c_char,
    request: Option<InstalldRequest>,
    errno: *mut u32,
    reply: impl FnOnce(&[u8]) -> bool,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (Some(socket), Some(request)) = (unsafe { text(socket) }, request) else {
            return INVALID;
        };
        let socket = Path::new(std::ffi::OsStr::from_bytes(socket.as_bytes()));
        match installd_at(socket, &request) {
            Ok(Ok(payload)) => {
                if reply(&payload) {
                    0
                } else {
                    TRANSPORT
                }
            }
            Ok(Err((code, _))) => {
                unsafe { errno.write(code) };
                FAILED
            }
            Err(_) => TRANSPORT,
        }
    }))
    .unwrap_or(PANIC)
}

/// installd createAppData for internal storage.
///
/// # Safety
/// Strings must be NUL-terminated; output pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_installd_create_app_data(
    socket: *const c_char,
    package: *const c_char,
    user: u32,
    flags: u32,
    ce_inode: *mut u64,
    de_inode: *mut u64,
    errno: *mut u32,
) -> i32 {
    if ce_inode.is_null() || de_inode.is_null() || errno.is_null() {
        return INVALID;
    }
    let request = unsafe { text(package) }.map(|package| InstalldRequest::CreateAppData {
        package: package.to_owned(),
        user,
        flags,
    });
    run(socket, request, errno, |payload| {
        if payload.len() != 16 {
            return false;
        }
        unsafe {
            ce_inode.write(u64::from_le_bytes(payload[..8].try_into().unwrap()));
            de_inode.write(u64::from_le_bytes(payload[8..].try_into().unwrap()));
        }
        true
    })
}

/// installd destroyAppData (`clear` = 0) or clearAppData (`clear` = 1).
///
/// # Safety
/// Strings must be NUL-terminated; `errno` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_installd_app_data(
    socket: *const c_char,
    package: *const c_char,
    user: u32,
    flags: u32,
    clear: i32,
    errno: *mut u32,
) -> i32 {
    if errno.is_null() || !(0..=1).contains(&clear) {
        return INVALID;
    }
    let request = unsafe { text(package) }.map(|package| {
        let package = package.to_owned();
        if clear == 1 {
            InstalldRequest::ClearAppData {
                package,
                user,
                flags,
            }
        } else {
            InstalldRequest::DestroyAppData {
                package,
                user,
                flags,
            }
        }
    });
    run(socket, request, errno, |payload| payload.is_empty())
}

/// installd rmPackageDir for a guest `/data/app` code path.
///
/// # Safety
/// Strings must be NUL-terminated; `errno` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_installd_rm_package_dir(
    socket: *const c_char,
    code_path: *const c_char,
    errno: *mut u32,
) -> i32 {
    if errno.is_null() {
        return INVALID;
    }
    let request = unsafe { text(code_path) }.map(|code_path| InstalldRequest::RmPackageDir {
        code_path: code_path.to_owned(),
    });
    run(socket, request, errno, |payload| payload.is_empty())
}
