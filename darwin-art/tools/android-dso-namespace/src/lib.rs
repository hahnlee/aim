//! A deliberately closed Android DSO namespace for the Darwin ELF loader.
//!
//! This is not a Darwin `dlsym(RTLD_DEFAULT, ...)` adapter. Every visible
//! SONAME and symbol is listed below, including its Android GNU symbol version.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::OnceLock;

mod loader_open;
mod vulkan_acquire_fences;
#[path = "vulkan/android_memory.rs"]
mod vulkan_android_memory;
#[path = "vulkan/android_sync.rs"]
mod vulkan_android_sync;
#[path = "vulkan/backend.rs"]
mod vulkan_backend;
#[path = "vulkan/device_owner.rs"]
mod vulkan_device_owner;
mod vulkan_library;
#[path = "vulkan/provider.rs"]
mod vulkan_provider;
#[path = "vulkan/sync_fd_import.rs"]
mod vulkan_sync_fd_import;
#[path = "vulkan/types.rs"]
mod vulkan_types;
mod vulkan_wsi;
mod vulkan_wsi_backend;
pub use loader_open::darwin_art_bionic_android_dlopen_ext;

pub const PROVIDER_LOADER_LIBDL: u32 = 1;
pub const PROVIDER_AOSP_LIBLOG: u32 = 2;

#[repr(C)]
pub struct AndroidDlExtInfo {
    pub flags: u64,
    pub reserved_addr: *mut c_void,
    pub reserved_size: usize,
    pub relro_fd: c_int,
    pub library_fd: c_int,
    pub library_fd_offset: i64,
    pub library_namespace: *mut c_void,
}

pub type OpenCallback = unsafe extern "C" fn(
    context: *mut c_void,
    filename: *const c_char,
    flags: c_int,
    extinfo: *const AndroidDlExtInfo,
    error: *mut c_char,
    error_capacity: usize,
) -> *mut c_void;
pub type LookupCallback = unsafe extern "C" fn(
    context: *mut c_void,
    handle: *mut c_void,
    symbol: *const c_char,
    version: *const c_char,
    error: *mut c_char,
    error_capacity: usize,
) -> *mut c_void;
pub type CloseCallback = unsafe extern "C" fn(
    context: *mut c_void,
    handle: *mut c_void,
    error: *mut c_char,
    error_capacity: usize,
) -> c_int;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoaderCallbacks {
    pub context: *mut c_void,
    pub open: Option<OpenCallback>,
    pub lookup: Option<LookupCallback>,
    pub close: Option<CloseCallback>,
}

// The embedding loader owns the context lifetime and binds exactly once before
// any app thread can enter libdl. Access is read-only after OnceLock publication.
unsafe impl Send for LoaderCallbacks {}
unsafe impl Sync for LoaderCallbacks {}

static LOADER: OnceLock<LoaderCallbacks> = OnceLock::new();
static mut LIBANDROID_HANDLE_TOKEN: u8 = 0;
static mut LIBNATIVEWINDOW_HANDLE_TOKEN: u8 = 0;
static mut LIBEGL_HANDLE_TOKEN: u8 = 0;
static mut LIBGLESV2_HANDLE_TOKEN: u8 = 0;
static mut LIBVULKAN_HANDLE_TOKEN: u8 = 0;

#[cfg(not(test))]
unsafe extern "C" {
    #[link_name = "darwin_art_android_ANativeWindow_fromSurface"]
    fn host_anative_window_from_surface(env: *mut c_void, surface: *mut c_void) -> *mut c_void;
    #[link_name = "darwin_art_android_ANativeWindow_release"]
    fn host_anative_window_release(window: *mut c_void);
    #[link_name = "darwin_art_android_ANativeWindow_lock"]
    fn host_anative_window_lock(
        window: *mut c_void,
        buffer: *mut c_void,
        bounds: *mut c_void,
    ) -> c_int;
    #[link_name = "darwin_art_android_ANativeWindow_unlockAndPost"]
    fn host_anative_window_unlock_and_post(window: *mut c_void) -> c_int;
    #[link_name = "darwin_art_android_ANativeWindow_setBuffersGeometry"]
    fn host_anative_window_set_buffers_geometry(
        window: *mut c_void,
        width: c_int,
        height: c_int,
        format: c_int,
    ) -> c_int;
    #[link_name = "ASharedMemory_create"]
    fn host_ashared_memory_create(name: *const c_char, size: usize) -> c_int;
    #[link_name = "ASharedMemory_setProt"]
    fn host_ashared_memory_set_prot(fd: c_int, protection: c_int) -> c_int;
    fn darwin_art_android_platform_symbol(symbol: *const c_char) -> *mut c_void;
    fn darwin_art_angle_dso_symbol(soname: *const c_char, symbol: *const c_char) -> *mut c_void;
}

#[cfg(test)]
unsafe fn darwin_art_android_platform_symbol(_symbol: *const c_char) -> *mut c_void {
    ptr::null_mut()
}

fn libandroid_handle() -> *mut c_void {
    ptr::addr_of_mut!(LIBANDROID_HANDLE_TOKEN).cast()
}

fn libnativewindow_handle() -> *mut c_void {
    ptr::addr_of_mut!(LIBNATIVEWINDOW_HANDLE_TOKEN).cast()
}

fn virtual_graphics_handle(name: &[u8]) -> Option<*mut c_void> {
    match name {
        b"libEGL.so" => Some(ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast()),
        b"libGLESv2.so" => Some(ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast()),
        // Vulkan remains a virtual Android DSO even when MoltenVK is present:
        // callers must never receive a Darwin dlopen handle directly.
        b"libvulkan.so" => Some(ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast()),
        _ => None,
    }
}

unsafe fn libandroid_symbol(name: &CStr) -> *mut c_void {
    #[cfg(test)]
    {
        let _ = name;
        ptr::dangling_mut::<c_void>()
    }
    #[cfg(not(test))]
    {
        match name.to_bytes() {
            b"ANativeWindow_fromSurface" => host_anative_window_from_surface as *mut c_void,
            b"ANativeWindow_release" => host_anative_window_release as *mut c_void,
            b"ANativeWindow_lock" => host_anative_window_lock as *mut c_void,
            b"ANativeWindow_unlockAndPost" => host_anative_window_unlock_and_post as *mut c_void,
            b"ANativeWindow_setBuffersGeometry" => {
                host_anative_window_set_buffers_geometry as *mut c_void
            }
            b"ASharedMemory_create" => host_ashared_memory_create as *mut c_void,
            b"ASharedMemory_setProt" => host_ashared_memory_set_prot as *mut c_void,
            // Android feature detection commonly dlopen()s libandroid.so and
            // resolves newer NDK APIs lazily. Keep that virtual DSO backed by
            // the same capability table used for regular ELF imports.
            _ => darwin_art_android_platform_symbol(name.as_ptr()),
        }
    }
}

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
    static ERROR_RETURN: RefCell<Option<CString>> = const { RefCell::new(None) };
}

const ERROR_CAPACITY: usize = 512;

fn set_error(message: impl AsRef<str>) {
    let bytes = message.as_ref().as_bytes();
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = CString::new(&bytes[..end]).expect("interior NUL removed");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(value));
}

fn call_error(buffer: &[c_char]) -> String {
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[no_mangle]
/// Binds the process-wide loader callbacks exactly once.
///
/// # Safety
///
/// `callbacks` must point to a readable `LoaderCallbacks`. Its context must
/// remain valid for the process lifetime and its function pointers must obey
/// their declared C ABIs.
pub unsafe extern "C" fn darwin_art_loader_bind(callbacks: *const LoaderCallbacks) -> c_int {
    if callbacks.is_null() {
        return -1;
    }
    // SAFETY: null was rejected and the value is copied immediately.
    let callbacks = unsafe { *callbacks };
    if callbacks.open.is_none() || callbacks.lookup.is_none() || callbacks.close.is_none() {
        return -1;
    }
    if LOADER.set(callbacks).is_err() {
        -1
    } else {
        0
    }
}

fn loader() -> Option<&'static LoaderCallbacks> {
    let value = LOADER.get();
    if value.is_none() {
        set_error("Android ELF loader is not bound");
    }
    value
}

#[no_mangle]
/// Opens an Android DSO through the bound loader.
///
/// # Safety
///
/// `filename` must be null or point to a valid NUL-terminated C string.
pub unsafe extern "C" fn darwin_art_bionic_dlopen(
    filename: *const c_char,
    flags: c_int,
) -> *mut c_void {
    darwin_art_bionic_android_dlopen_ext(filename, flags, ptr::null())
}

#[no_mangle]
/// Looks up an exported symbol in a loader-owned handle.
///
/// # Safety
///
/// `handle` must be accepted by the bound loader and `symbol` must point to a
/// valid NUL-terminated C string.
pub unsafe extern "C" fn darwin_art_bionic_dlsym(
    handle: *mut c_void,
    symbol: *const c_char,
) -> *mut c_void {
    let egl_handle = ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast();
    let gles_handle = ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast();
    let vulkan_handle = ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast();
    if handle == egl_handle || handle == gles_handle {
        if symbol.is_null() {
            set_error("null Android graphics DSO symbol");
            return ptr::null_mut();
        }
        let soname = if handle == egl_handle {
            c"libEGL.so"
        } else {
            c"libGLESv2.so"
        };
        #[cfg(not(test))]
        let result = unsafe { darwin_art_angle_dso_symbol(soname.as_ptr(), symbol) };
        #[cfg(test)]
        let result = {
            let _ = soname;
            ptr::dangling_mut::<c_void>()
        };
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            let name = unsafe { CStr::from_ptr(symbol) };
            eprintln!(
                "ART Android libdl: {} dlsym {} resolved={}",
                soname.to_string_lossy(),
                name.to_string_lossy(),
                !result.is_null()
            );
        }
        if result.is_null() {
            set_error("Android graphics DSO symbol is unavailable");
        }
        return result;
    }
    if handle == vulkan_handle {
        return unsafe { vulkan_library::legacy_lookup(symbol) };
    }
    if symbol.is_null() {
        set_error("dlsym symbol is null");
        return ptr::null_mut();
    }
    if handle == libandroid_handle() || handle == libnativewindow_handle() {
        let name = unsafe { CStr::from_ptr(symbol) };
        let result = unsafe { libandroid_symbol(name) };
        if std::env::var_os("DARWIN_ART_DEBUG_ANATIVEWINDOW").is_some() {
            let soname = if handle == libandroid_handle() {
                "libandroid.so"
            } else {
                "libnativewindow.so"
            };
            eprintln!(
                "ART Android libdl: {soname} dlsym {} resolved={}",
                name.to_string_lossy(),
                !result.is_null()
            );
        }
        if result.is_null() {
            set_error("Android platform DSO symbol is unsupported");
        }
        return result;
    }
    let Some(loader) = loader() else {
        return ptr::null_mut();
    };
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let result = unsafe {
        (loader.lookup.expect("validated"))(
            loader.context,
            handle,
            symbol,
            ptr::null(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if result.is_null() {
        let message = call_error(&error);
        set_error(if message.is_empty() {
            "Android dlsym failed"
        } else {
            &message
        });
    }
    result
}

#[no_mangle]
/// Releases a loader-owned DSO handle.
///
/// # Safety
///
/// `handle` must be null or a handle accepted by the bound loader.
pub unsafe extern "C" fn darwin_art_bionic_dlclose(handle: *mut c_void) -> c_int {
    if handle == libandroid_handle()
        || handle == libnativewindow_handle()
        || handle == ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast()
        || handle == ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast()
        || handle == ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast()
    {
        return 0;
    }
    let Some(loader) = loader() else { return -1 };
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let result = unsafe {
        (loader.close.expect("validated"))(loader.context, handle, error.as_mut_ptr(), error.len())
    };
    if result != 0 {
        let message = call_error(&error);
        set_error(if message.is_empty() {
            "Android dlclose failed"
        } else {
            &message
        });
        -1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn darwin_art_bionic_dlerror() -> *mut c_char {
    let pending = LAST_ERROR.with(|slot| slot.borrow_mut().take());
    ERROR_RETURN.with(|slot| {
        *slot.borrow_mut() = pending;
        slot.borrow()
            .as_ref()
            .map_or(ptr::null_mut(), |message| message.as_ptr() as *mut c_char)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub provider: u32,
    pub ordinal: u32,
    pub address: usize,
}

const LIBDL: &[(&str, &str)] = &[
    ("android_dlopen_ext", "LIBC"),
    ("dlclose", "LIBC"),
    ("dlerror", "LIBC"),
    ("dlopen", "LIBC"),
    ("dlsym", "LIBC"),
];

// Exact NDK r28 API 35 liblog stub surface. An address of zero means that the
// ELF loader must obtain the address from the linked AOSP liblog provider table,
// never from Darwin's process-global namespace.
const LIBLOG: &[&str] = &[
    "__android_log_assert",
    "__android_log_buf_print",
    "__android_log_buf_write",
    "__android_log_call_aborter",
    "__android_log_default_aborter",
    "__android_log_get_minimum_priority",
    "__android_log_is_loggable",
    "__android_log_is_loggable_len",
    "__android_log_logd_logger",
    "__android_log_print",
    "__android_log_set_aborter",
    "__android_log_set_default_tag",
    "__android_log_set_logger",
    "__android_log_set_minimum_priority",
    "__android_log_stderr_logger",
    "__android_log_vprint",
    "__android_log_write",
    "__android_log_write_log_message",
];

pub fn resolve(soname: &str, symbol: &str, version: Option<&str>) -> Result<Resolution, u32> {
    match soname {
        "libdl.so" => {
            let Some((ordinal, (_, expected))) = LIBDL
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.0 == symbol)
            else {
                return Err(2);
            };
            if version.is_some_and(|actual| actual != *expected) {
                return Err(3);
            }
            let address = match ordinal {
                0 => darwin_art_bionic_android_dlopen_ext as usize,
                1 => darwin_art_bionic_dlclose as usize,
                2 => darwin_art_bionic_dlerror as usize,
                3 => darwin_art_bionic_dlopen as usize,
                4 => darwin_art_bionic_dlsym as usize,
                _ => unreachable!(),
            };
            Ok(Resolution {
                provider: PROVIDER_LOADER_LIBDL,
                ordinal: ordinal as u32,
                address,
            })
        }
        "liblog.so" => {
            let Some(ordinal) = LIBLOG.iter().position(|candidate| *candidate == symbol) else {
                return Err(2);
            };
            // The API 35 NDK stub deliberately has no GNU version definitions.
            if version.is_some() {
                return Err(3);
            }
            Ok(Resolution {
                provider: PROVIDER_AOSP_LIBLOG,
                ordinal: ordinal as u32,
                address: 0,
            })
        }
        _ => Err(1),
    }
}

#[repr(C)]
pub struct CResolution {
    pub provider: u32,
    pub ordinal: u32,
    pub address: usize,
}

#[no_mangle]
/// Resolves an allowlisted virtual Android DSO symbol.
///
/// # Safety
///
/// `soname` and `symbol` must be valid C strings, `version` must be null or a
/// valid C string, and `output` must point to writable `CResolution` storage.
pub unsafe extern "C" fn darwin_art_dso_resolve(
    soname: *const c_char,
    symbol: *const c_char,
    version: *const c_char,
    output: *mut CResolution,
) -> c_int {
    if soname.is_null() || symbol.is_null() || output.is_null() {
        return 4;
    }
    let Ok(soname) = unsafe { CStr::from_ptr(soname) }.to_str() else {
        return 4;
    };
    let Ok(symbol) = unsafe { CStr::from_ptr(symbol) }.to_str() else {
        return 4;
    };
    let version = if version.is_null() {
        None
    } else {
        let Ok(value) = unsafe { CStr::from_ptr(version) }.to_str() else {
            return 4;
        };
        Some(value)
    };
    match resolve(soname, symbol, version) {
        Ok(result) => {
            unsafe {
                *output = CResolution {
                    provider: result.provider,
                    ordinal: result.ordinal,
                    address: result.address,
                }
            };
            0
        }
        Err(code) => code as c_int,
    }
}

pub fn manifest() -> String {
    let mut output = String::new();
    for (symbol, version) in LIBDL {
        output.push_str(&format!("libdl.so\t{symbol}\t{version}\tloader\n"));
    }
    for symbol in LIBLOG {
        output.push_str(&format!("liblog.so\t{symbol}\t-\taosp-liblog\n"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_is_closed_and_versioned() {
        unsafe extern "C" fn open(
            _: *mut c_void,
            filename: *const c_char,
            flags: c_int,
            extinfo: *const AndroidDlExtInfo,
            error: *mut c_char,
            _: usize,
        ) -> *mut c_void {
            if let Some(info) = unsafe { extinfo.as_ref() } {
                if info.flags != 0 {
                    assert_eq!(flags, 2);
                    assert_eq!(info.library_namespace as usize, 0xfeed);
                    assert_eq!(info.library_fd, 73);
                    let message = b"explicit request rejected by linker\0";
                    unsafe {
                        ptr::copy_nonoverlapping(message.as_ptr().cast(), error, message.len());
                    }
                    return ptr::null_mut();
                }
            }
            if flags & 0x4 != 0 {
                // RTLD_NOLOAD is linker policy; the original flags arrive intact.
                assert_eq!(flags, 0x106);
                let message = b"RTLD_NOLOAD reached linker\0";
                unsafe {
                    ptr::copy_nonoverlapping(message.as_ptr().cast(), error, message.len());
                }
                return ptr::null_mut();
            }
            if !filename.is_null() && unsafe { CStr::from_ptr(filename) }.to_bytes() == b"bad.so" {
                let message = b"virtual SONAME denied\0";
                unsafe {
                    ptr::copy_nonoverlapping(
                        message.as_ptr() as *const c_char,
                        error,
                        message.len(),
                    )
                };
                ptr::null_mut()
            } else {
                0x1234usize as *mut c_void
            }
        }
        unsafe extern "C" fn lookup(
            _: *mut c_void,
            _: *mut c_void,
            _: *const c_char,
            _: *const c_char,
            _: *mut c_char,
            _: usize,
        ) -> *mut c_void {
            0x5678usize as *mut c_void
        }
        unsafe extern "C" fn close(
            _: *mut c_void,
            _: *mut c_void,
            _: *mut c_char,
            _: usize,
        ) -> c_int {
            0
        }

        assert_eq!(
            resolve("libdl.so", "dlopen", Some("LIBC"))
                .unwrap()
                .provider,
            1
        );
        assert_eq!(resolve("libdl.so", "dlopen", Some("GLIBC_2.0")), Err(3));
        assert_eq!(resolve("libc.so", "malloc", Some("LIBC")), Err(1));
        assert_eq!(
            resolve("liblog.so", "__android_log_print", None)
                .unwrap()
                .provider,
            2
        );
        assert_eq!(
            resolve("liblog.so", "__android_log_print", Some("LIBLOG")),
            Err(3)
        );
        assert_eq!(resolve("liblog.so", "android_log_private", None), Err(2));
        assert_eq!(std::mem::size_of::<AndroidDlExtInfo>(), 48);

        let callbacks = LoaderCallbacks {
            context: ptr::null_mut(),
            open: Some(open),
            lookup: Some(lookup),
            close: Some(close),
        };
        assert_eq!(unsafe { darwin_art_loader_bind(&callbacks) }, 0);
        let explicit = AndroidDlExtInfo {
            flags: 0x200,
            reserved_addr: ptr::null_mut(),
            reserved_size: 0,
            relro_fd: -1,
            library_fd: 73,
            library_fd_offset: 0,
            library_namespace: 0xfeedusize as *mut c_void,
        };
        for name in [
            c"libandroid.so",
            c"libnativewindow.so",
            c"libEGL.so",
            c"libGLESv2.so",
            c"libvulkan.so",
        ] {
            assert!(
                unsafe { darwin_art_bionic_android_dlopen_ext(name.as_ptr(), 2, &explicit) }
                    .is_null()
            );
            assert_eq!(
                unsafe { CStr::from_ptr(darwin_art_bionic_dlerror()) }.to_bytes(),
                b"explicit request rejected by linker"
            );
            assert!(darwin_art_bionic_dlerror().is_null());
        }
        // An explicit file is not substituted by a basename-matching token.
        assert_eq!(
            unsafe { darwin_art_bionic_dlopen(c"/private/libEGL.so".as_ptr(), 2) } as usize,
            0x1234
        );
        let good = CString::new("good.so").unwrap();
        let symbol = CString::new("fixture_symbol").unwrap();
        let handle = unsafe { darwin_art_bionic_dlopen(good.as_ptr(), 2) };
        assert_eq!(handle as usize, 0x1234);
        assert_eq!(
            unsafe { darwin_art_bionic_dlsym(handle, symbol.as_ptr()) } as usize,
            0x5678
        );
        assert_eq!(unsafe { darwin_art_bionic_dlclose(handle) }, 0);
        assert!(darwin_art_bionic_dlerror().is_null());

        let vulkan = CString::new("libvulkan.so").unwrap();
        let vulkan_handle = unsafe { darwin_art_bionic_dlopen(vulkan.as_ptr(), 2) };
        assert!(!vulkan_handle.is_null());
        let get_proc_name = CString::new("vkGetInstanceProcAddr").unwrap();
        let get_proc = unsafe { darwin_art_bionic_dlsym(vulkan_handle, get_proc_name.as_ptr()) };
        assert!(!get_proc.is_null());
        let enumerate_name = CString::new("vkEnumerateInstanceVersion").unwrap();
        let enumerate = unsafe { darwin_art_bionic_dlsym(vulkan_handle, enumerate_name.as_ptr()) };
        assert!(!enumerate.is_null());
        let enumerate: unsafe extern "C" fn(*mut u32) -> i32 =
            unsafe { std::mem::transmute(enumerate) };
        assert_eq!(unsafe { enumerate(ptr::null_mut()) }, -9);
        assert_eq!(unsafe { darwin_art_bionic_dlclose(vulkan_handle) }, 0);

        let native_window = CString::new("libnativewindow.so").unwrap();
        let native_window_handle = unsafe { darwin_art_bionic_dlopen(native_window.as_ptr(), 2) };
        assert!(!native_window_handle.is_null());
        let describe_name = CString::new("AHardwareBuffer_describe").unwrap();
        assert!(
            !unsafe { darwin_art_bionic_dlsym(native_window_handle, describe_name.as_ptr()) }
                .is_null()
        );
        // bionic RTLD_NOW|RTLD_GLOBAL on a resident provider is its handle.
        assert_eq!(
            unsafe { darwin_art_bionic_dlopen(native_window.as_ptr(), 0x102) },
            native_window_handle
        );
        assert!(unsafe { darwin_art_bionic_dlopen(native_window.as_ptr(), 0x106) }.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(darwin_art_bionic_dlerror()) }.to_bytes(),
            b"RTLD_NOLOAD reached linker"
        );
        assert_eq!(
            unsafe { darwin_art_bionic_dlclose(native_window_handle) },
            0
        );

        let bad = CString::new("bad.so").unwrap();
        assert!(unsafe { darwin_art_bionic_dlopen(bad.as_ptr(), 2) }.is_null());
        let error = darwin_art_bionic_dlerror();
        assert_eq!(
            unsafe { CStr::from_ptr(error) }.to_bytes(),
            b"virtual SONAME denied"
        );
        assert!(darwin_art_bionic_dlerror().is_null());
    }
}
