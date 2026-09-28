//! The EGL entry points the guest implements: EGL on the Android platform
//! over the host's ANGLE display (docs/gles-driver.md).
//!
//! - Displays: `EGL_PLATFORM_ANDROID_KHR` / the default display is ANGLE's
//!   Metal display on the host.
//! - Window surfaces: an `ANativeWindow` gets a host pbuffer as its default
//!   framebuffer. `eglSwapBuffers` dequeues a buffer, has the host blit the
//!   pbuffer into it (top row first) and queues it with the blit's fence.
//! - `EGL_NATIVE_BUFFER_ANDROID` images: the buffer's memory, imported by the
//!   host as a Metal texture ([`crate::buffers`]).
//! - Android config attributes (`EGL_NATIVE_VISUAL_ID`,
//!   `EGL_RECORDABLE_ANDROID`, `EGL_FRAMEBUFFER_TARGET_ANDROID`).
//! - Callbacks (blob cache, debug) are accepted and never called: host code
//!   does not call guest code.
//!
//! Syncs are [`crate::sync`]; every other EGL call goes straight to the
//! host (`crate::thunks`).

use core::ffi::{CStr, c_char, c_int, c_void};
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use aim_hostcall::gpu::{Init, Present};

use crate::buffers::{self, NativeBuffer};
use crate::thunks::{PROCS, TABLE_HASH, TABLE_LEN, host};
use crate::types::*;

const EGL_FALSE: EGLBoolean = 0;
const EGL_TRUE: EGLBoolean = 1;
const EGL_NONE: EGLint = 0x3038;
const EGL_SUCCESS: EGLint = 0x3000;
const EGL_BAD_ALLOC: EGLint = 0x3003;
const EGL_BAD_ATTRIBUTE: EGLint = 0x3004;
const EGL_BAD_NATIVE_WINDOW: EGLint = 0x300b;
const EGL_BAD_PARAMETER: EGLint = 0x300c;
const EGL_BAD_SURFACE: EGLint = 0x300d;
const EGL_NOT_INITIALIZED: EGLint = 0x3001;
const EGL_ALPHA_SIZE: EGLint = 0x3021;
const EGL_BLUE_SIZE: EGLint = 0x3022;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_NATIVE_VISUAL_ID: EGLint = 0x302e;
const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_HEIGHT: EGLint = 0x3056;
const EGL_WIDTH: EGLint = 0x3057;
const EGL_DRAW: EGLint = 0x3059;
const EGL_GL_COLORSPACE: EGLint = 0x309d;
const EGL_PROTECTED_CONTENT_EXT: EGLint = 0x32c0;
const EGL_COLOR_COMPONENT_TYPE_EXT: EGLint = 0x3339;
const EGL_COLOR_COMPONENT_TYPE_FLOAT_EXT: EGLint = 0x333b;
const EGL_NATIVE_BUFFER_ANDROID: EGLenum = 0x3140;
const EGL_PLATFORM_ANDROID_KHR: EGLenum = 0x3141;
const EGL_RECORDABLE_ANDROID: EGLint = 0x3142;
const EGL_FRAMEBUFFER_TARGET_ANDROID: EGLint = 0x3147;
const EGL_PLATFORM_ANGLE_ANGLE: EGLenum = 0x3202;
const EGL_PLATFORM_ANGLE_TYPE_ANGLE: EGLAttrib = 0x3203;
const EGL_PLATFORM_ANGLE_TYPE_METAL_ANGLE: EGLAttrib = 0x3489;

/// `ANDROID_NATIVE_WINDOW_MAGIC`: `'_wnd'`.
const NATIVE_WINDOW_MAGIC: i32 = 0x5f77_6e64;

/// `ANativeWindow`, only its magic is read.
#[repr(C)]
pub struct NativeWindow {
    magic: i32,
}

#[link(name = "nativewindow")]
unsafe extern "C" {
    fn ANativeWindow_acquire(w: *mut NativeWindow);
    fn ANativeWindow_release(w: *mut NativeWindow);
    fn ANativeWindow_getWidth(w: *mut NativeWindow) -> i32;
    fn ANativeWindow_getHeight(w: *mut NativeWindow) -> i32;
    fn ANativeWindow_dequeueBuffer(
        w: *mut NativeWindow,
        buffer: *mut *mut NativeBuffer,
        fence: *mut c_int,
    ) -> c_int;
    fn ANativeWindow_queueBuffer(
        w: *mut NativeWindow,
        buffer: *mut NativeBuffer,
        fence: c_int,
    ) -> c_int;
    fn ANativeWindow_cancelBuffer(
        w: *mut NativeWindow,
        buffer: *mut NativeBuffer,
        fence: c_int,
    ) -> c_int;
    fn ANativeWindow_setSwapInterval(w: *mut NativeWindow, interval: c_int) -> c_int;
    fn ANativeWindow_setBuffersTimestamp(w: *mut NativeWindow, timestamp: i64) -> c_int;
}

// Both are `const`; clippy misreads the Android thread_local expansion.
thread_local! {
    /// An error raised here rather than by the host, for `eglGetError`.
    #[allow(clippy::missing_const_for_thread_local)]
    static ERROR: Cell<EGLint> = const { Cell::new(EGL_SUCCESS) };
    /// The guest draw and read surfaces made current on this thread.
    #[allow(clippy::missing_const_for_thread_local)]
    static CURRENT: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

pub(crate) fn fail<T>(error: EGLint, ret: T) -> T {
    ERROR.set(error);
    ret
}

/// Bitmap of the host table entries ANGLE provides, after [`init`].
fn resolved() -> &'static [u64] {
    static RESOLVED: OnceLock<Vec<u64>> = OnceLock::new();
    RESOLVED.get_or_init(|| {
        let mut bits = vec![0u64; TABLE_LEN.div_ceil(64)];
        let mut args = Init {
            table_hash: TABLE_HASH,
            table_len: TABLE_LEN as u64,
            resolved: bits.as_mut_ptr() as u64,
            resolved_words: bits.len() as u64,
        };
        if let Err(e) = aim_hostcall::guest::gpu_init(&mut args) {
            eprintln!("libGLES_aim: host GPU unavailable (errno {})", e.0);
            bits.fill(0);
        }
        bits
    })
}

fn init() -> bool {
    resolved().iter().any(|&w| w != 0)
}

// Displays.

fn display() -> EGLDisplay {
    if !init() {
        return fail(EGL_NOT_INITIALIZED, std::ptr::null_mut());
    }
    let attribs = [
        EGL_PLATFORM_ANGLE_TYPE_ANGLE,
        EGL_PLATFORM_ANGLE_TYPE_METAL_ANGLE,
        EGL_NONE as EGLAttrib,
    ];
    // SAFETY: a NONE-terminated attribute list.
    unsafe {
        host::eglGetPlatformDisplay(
            EGL_PLATFORM_ANGLE_ANGLE,
            std::ptr::null_mut(),
            attribs.as_ptr(),
        )
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetDisplay(native: EGLNativeDisplayType) -> EGLDisplay {
    if !native.is_null() {
        return fail(EGL_BAD_PARAMETER, std::ptr::null_mut());
    }
    display()
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetPlatformDisplay(
    platform: EGLenum,
    native: *mut c_void,
    _attribs: *const EGLAttrib,
) -> EGLDisplay {
    if platform != EGL_PLATFORM_ANDROID_KHR || !native.is_null() {
        return fail(EGL_BAD_PARAMETER, std::ptr::null_mut());
    }
    display()
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetPlatformDisplayEXT(
    platform: EGLenum,
    native: *mut c_void,
    _attribs: *const EGLint,
) -> EGLDisplay {
    // SAFETY: the attributes are not read.
    unsafe { eglGetPlatformDisplay(platform, native, std::ptr::null()) }
}

/// Extensions implemented here, added to the host's.
const OURS: &[&str] = &[
    "EGL_ANDROID_image_native_buffer",
    "EGL_ANDROID_recordable",
    "EGL_ANDROID_framebuffer_target",
    "EGL_ANDROID_presentation_time",
    "EGL_KHR_swap_buffers_with_damage",
    "EGL_ANDROID_native_fence_sync",
];
/// Host extensions that need callbacks into the guest.
const WITHHELD: &[&str] = &["EGL_ANDROID_blob_cache", "EGL_KHR_debug"];

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const c_char {
    // SAFETY: forwarded.
    let s = unsafe { host::eglQueryString(dpy, name) };
    if dpy.is_null() || name != EGL_EXTENSIONS || s.is_null() {
        return s;
    }
    static STRINGS: OnceLock<Mutex<HashMap<usize, CString>>> = OnceLock::new();
    let mut strings = STRINGS.get_or_init(Default::default).lock().unwrap();
    let merged = strings.entry(dpy as usize).or_insert_with(|| {
        // SAFETY: the host's NUL-terminated extension string.
        let host = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        let mut list: Vec<&str> = host
            .split(' ')
            .filter(|e| !e.is_empty() && !WITHHELD.contains(e))
            .collect();
        for e in OURS {
            if !list.contains(e) {
                list.push(e);
            }
        }
        CString::new(list.join(" ")).unwrap()
    });
    // Kept for the life of the process, as EGL requires.
    merged.as_ptr()
}

// Configs.

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglChooseConfig(
    dpy: EGLDisplay,
    attribs: *const EGLint,
    configs: *mut EGLConfig,
    size: EGLint,
    count: *mut EGLint,
) -> EGLBoolean {
    let mut list = Vec::new();
    // SAFETY: a NONE-terminated list of pairs, or null.
    unsafe {
        let mut p = attribs;
        while !p.is_null() && *p != EGL_NONE {
            let (k, v) = (*p, *p.add(1));
            if !matches!(
                k,
                EGL_RECORDABLE_ANDROID | EGL_FRAMEBUFFER_TARGET_ANDROID | EGL_NATIVE_VISUAL_ID
            ) {
                list.extend([k, v]);
            }
            p = p.add(2);
        }
    }
    list.push(EGL_NONE);
    // SAFETY: forwarded with the filtered list.
    unsafe { host::eglChooseConfig(dpy, list.as_ptr(), configs, size, count) }
}

fn config_attrib(dpy: EGLDisplay, config: EGLConfig, attr: EGLint) -> Option<EGLint> {
    let mut v = 0;
    // SAFETY: forwarded.
    (unsafe { host::eglGetConfigAttrib(dpy, config, attr, &mut v) } == EGL_TRUE).then_some(v)
}

/// The `PixelFormat` of a config's color buffer (the window's buffers).
fn native_visual(dpy: EGLDisplay, config: EGLConfig) -> EGLint {
    let size = |a| config_attrib(dpy, config, a).unwrap_or(0);
    let float = config_attrib(dpy, config, EGL_COLOR_COMPONENT_TYPE_EXT)
        == Some(EGL_COLOR_COMPONENT_TYPE_FLOAT_EXT);
    match (
        size(EGL_RED_SIZE),
        size(EGL_GREEN_SIZE),
        size(EGL_BLUE_SIZE),
        size(EGL_ALPHA_SIZE),
    ) {
        (16, 16, 16, _) if float => 0x16,
        (8, 8, 8, 8) => 0x1,
        (8, 8, 8, 0) => 0x2,
        (5, 6, 5, 0) => 0x4,
        (10, 10, 10, 2) => 0x2b,
        _ => 0,
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetConfigAttrib(
    dpy: EGLDisplay,
    config: EGLConfig,
    attr: EGLint,
    value: *mut EGLint,
) -> EGLBoolean {
    let v = match attr {
        EGL_RECORDABLE_ANDROID | EGL_FRAMEBUFFER_TARGET_ANDROID => EGL_TRUE as EGLint,
        EGL_NATIVE_VISUAL_ID => native_visual(dpy, config),
        // SAFETY: forwarded.
        _ => return unsafe { host::eglGetConfigAttrib(dpy, config, attr, value) },
    };
    if value.is_null() {
        return fail(EGL_BAD_PARAMETER, EGL_FALSE);
    }
    // SAFETY: checked non-null.
    unsafe { *value = v };
    EGL_TRUE
}

// Window surfaces.

struct Window {
    display: EGLDisplay,
    config: EGLConfig,
    window: *mut NativeWindow,
    /// The host pbuffer that is the surface's default framebuffer.
    pbuffer: EGLSurface,
    width: i32,
    height: i32,
    colorspace: Option<EGLint>,
    /// Buffers of the window imported so far, by buffer id.
    images: HashMap<u64, EGLImage>,
}

// SAFETY: EGL surfaces may be used from any thread; access is serialized by
// the surface's mutex.
unsafe impl Send for Window {}

/// Guest window surfaces, by their handle.
fn windows() -> MutexGuard<'static, HashMap<usize, Arc<Mutex<Window>>>> {
    static WINDOWS: OnceLock<Mutex<HashMap<usize, Arc<Mutex<Window>>>>> = OnceLock::new();
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn window(surface: EGLSurface) -> Option<Arc<Mutex<Window>>> {
    windows().get(&(surface as usize)).cloned()
}

/// The host surface of a guest surface.
fn host_surface(surface: EGLSurface) -> EGLSurface {
    match window(surface) {
        Some(w) => w.lock().unwrap().pbuffer,
        None => surface,
    }
}

impl Window {
    fn create_pbuffer(&self, width: i32, height: i32) -> EGLSurface {
        let mut attribs = vec![EGL_WIDTH, width, EGL_HEIGHT, height];
        if let Some(c) = self.colorspace {
            attribs.extend([EGL_GL_COLORSPACE, c]);
        }
        attribs.push(EGL_NONE);
        // SAFETY: a NONE-terminated list.
        unsafe { host::eglCreatePbufferSurface(self.display, self.config, attribs.as_ptr()) }
    }

    fn release_images(&mut self) {
        for (id, image) in self.images.drain() {
            // SAFETY: an image this surface imported.
            unsafe { host::eglDestroyImageKHR(self.display, image) };
            buffers::unmap(id);
        }
    }
}

unsafe fn create_window(
    dpy: EGLDisplay,
    config: EGLConfig,
    native: *mut NativeWindow,
    colorspace: Option<EGLint>,
) -> EGLSurface {
    // SAFETY: checked non-null; the magic is the struct's first field.
    if native.is_null() || unsafe { (*native).magic } != NATIVE_WINDOW_MAGIC {
        return fail(EGL_BAD_NATIVE_WINDOW, std::ptr::null_mut());
    }
    // SAFETY: a valid window.
    let (width, height) = unsafe {
        (
            ANativeWindow_getWidth(native),
            ANativeWindow_getHeight(native),
        )
    };
    if width <= 0 || height <= 0 {
        return fail(EGL_BAD_NATIVE_WINDOW, std::ptr::null_mut());
    }
    let mut w = Window {
        display: dpy,
        config,
        window: native,
        pbuffer: std::ptr::null_mut(),
        width,
        height,
        colorspace,
        images: HashMap::new(),
    };
    w.pbuffer = w.create_pbuffer(width, height);
    if w.pbuffer.is_null() {
        // The host's error stands.
        return std::ptr::null_mut();
    }
    // SAFETY: a valid window; released in eglDestroySurface.
    unsafe { ANativeWindow_acquire(native) };
    let w = Arc::new(Mutex::new(w));
    let handle = Arc::as_ptr(&w) as usize;
    windows().insert(handle, w);
    handle as EGLSurface
}

/// Attribute pairs to (key, value) until `EGL_NONE`.
unsafe fn pairs<T: Copy + Into<i64>>(mut p: *const T) -> Vec<(i64, i64)> {
    let mut v = Vec::new();
    // SAFETY: caller contract: a NONE-terminated list or null.
    unsafe {
        while !p.is_null() && (*p).into() != EGL_NONE as i64 {
            v.push(((*p).into(), (*p.add(1)).into()));
            p = p.add(2);
        }
    }
    v
}

fn colorspace(attribs: &[(i64, i64)]) -> Option<EGLint> {
    attribs
        .iter()
        .find(|&&(k, _)| k == EGL_GL_COLORSPACE as i64)
        .map(|&(_, v)| v as EGLint)
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreateWindowSurface(
    dpy: EGLDisplay,
    config: EGLConfig,
    win: EGLNativeWindowType,
    attribs: *const EGLint,
) -> EGLSurface {
    // SAFETY: EGL's contract for the attribute list and window.
    unsafe {
        let attribs: Vec<_> = pairs(attribs).into_iter().collect();
        create_window(dpy, config, win.cast(), colorspace(&attribs))
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreatePlatformWindowSurface(
    dpy: EGLDisplay,
    config: EGLConfig,
    win: *mut c_void,
    attribs: *const EGLAttrib,
) -> EGLSurface {
    // SAFETY: EGL's contract.
    unsafe {
        let attribs: Vec<_> = pairs(attribs.cast::<i64>()).into_iter().collect();
        create_window(dpy, config, win.cast(), colorspace(&attribs))
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreatePlatformWindowSurfaceEXT(
    dpy: EGLDisplay,
    config: EGLConfig,
    win: *mut c_void,
    attribs: *const EGLint,
) -> EGLSurface {
    // SAFETY: EGL's contract.
    unsafe { eglCreateWindowSurface(dpy, config, win, attribs) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDestroySurface(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean {
    let Some(w) = windows().remove(&(surface as usize)) else {
        // SAFETY: forwarded.
        return unsafe { host::eglDestroySurface(dpy, surface) };
    };
    let mut w = w.lock().unwrap();
    w.release_images();
    // SAFETY: the surface's pbuffer and window reference.
    unsafe {
        host::eglDestroySurface(dpy, w.pbuffer);
        ANativeWindow_release(w.window);
    }
    EGL_TRUE
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglMakeCurrent(
    dpy: EGLDisplay,
    draw: EGLSurface,
    read: EGLSurface,
    ctx: EGLContext,
) -> EGLBoolean {
    // SAFETY: forwarded with the host surfaces.
    let ok = unsafe { host::eglMakeCurrent(dpy, host_surface(draw), host_surface(read), ctx) };
    if ok == EGL_TRUE {
        CURRENT.set((draw as usize, read as usize));
    }
    ok
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetCurrentSurface(which: EGLint) -> EGLSurface {
    let (draw, read) = CURRENT.get();
    (if which == EGL_DRAW { draw } else { read }) as EGLSurface
}

/// Wait for and close a fence fd.
fn wait(fence: c_int) {
    if fence < 0 {
        return;
    }
    let mut p = libc::pollfd {
        fd: fence,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd; the fence is ours to close.
    unsafe {
        while libc::poll(&mut p, 1, -1) < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
        {}
        libc::close(fence);
    }
}

/// Present a window surface: its pbuffer into the next buffer of the window.
fn swap(surface: EGLSurface, w: &mut Window) -> EGLBoolean {
    if CURRENT.get().0 != surface as usize {
        return fail(EGL_BAD_SURFACE, EGL_FALSE);
    }
    let (mut buffer, mut fence) = (std::ptr::null_mut(), -1);
    // SAFETY: the surface's window.
    if unsafe { ANativeWindow_dequeueBuffer(w.window, &mut buffer, &mut fence) } != 0 {
        return fail(EGL_BAD_NATIVE_WINDOW, EGL_FALSE);
    }
    wait(fence);
    // SAFETY: a buffer the window just gave us.
    let b = unsafe { &*buffer };
    // SAFETY: the buffer's handle.
    let id = unsafe { aim_gralloc::handle::parse(b.handle) }.map(|(_, h)| h.id);
    let image = match id.and_then(|id| w.images.get(&id).copied()) {
        Some(image) => Some(image),
        None => {
            // SAFETY: a valid ANativeWindowBuffer.
            unsafe { buffers::import(w.display, buffer) }.map(|i| {
                w.images.insert(i.id, i.image);
                i.image
            })
        }
    };
    let Some(image) = image else {
        // SAFETY: returning the dequeued buffer unused.
        unsafe { ANativeWindow_cancelBuffer(w.window, buffer, -1) };
        return fail(EGL_BAD_ALLOC, EGL_FALSE);
    };
    let mut present = Present {
        image: image as u64,
        src_width: w.width as u32,
        src_height: w.height as u32,
        dst_width: b.width as u32,
        dst_height: b.height as u32,
        fence: -1,
        _reserved: 0,
    };
    let presented = aim_hostcall::guest::gpu_present(&mut present).is_ok();
    // SAFETY: the dequeued buffer and the fence of the copy into it, which
    // the queue takes over.
    let queued = unsafe { ANativeWindow_queueBuffer(w.window, buffer, present.fence) } == 0;
    if !presented || !queued {
        return fail(EGL_BAD_SURFACE, EGL_FALSE);
    }
    // Follow the window's size: a new pbuffer takes the old one's place.
    // SAFETY: the surface's window.
    let (width, height) = unsafe {
        (
            ANativeWindow_getWidth(w.window),
            ANativeWindow_getHeight(w.window),
        )
    };
    if (width, height) != (w.width, w.height) && width > 0 && height > 0 {
        let pbuffer = w.create_pbuffer(width, height);
        if !pbuffer.is_null() {
            let (_, read) = CURRENT.get();
            let read = if read == surface as usize {
                pbuffer
            } else {
                host_surface(read as EGLSurface)
            };
            // SAFETY: rebinding this thread's context to the new pbuffer.
            unsafe {
                let ctx = host::eglGetCurrentContext();
                host::eglMakeCurrent(w.display, pbuffer, read, ctx);
                host::eglDestroySurface(w.display, w.pbuffer);
            }
            w.pbuffer = pbuffer;
            w.width = width;
            w.height = height;
            // The queue reallocates its buffers for the new size.
            w.release_images();
        }
    }
    EGL_TRUE
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean {
    match window(surface) {
        Some(w) => swap(surface, &mut w.lock().unwrap()),
        // SAFETY: forwarded.
        None => unsafe { host::eglSwapBuffers(dpy, surface) },
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSwapBuffersWithDamageKHR(
    dpy: EGLDisplay,
    surface: EGLSurface,
    _rects: *const EGLint,
    _count: EGLint,
) -> EGLBoolean {
    // SAFETY: EGL's contract; the whole surface is presented.
    unsafe { eglSwapBuffers(dpy, surface) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSwapBuffersWithDamageEXT(
    dpy: EGLDisplay,
    surface: EGLSurface,
    _rects: *const EGLint,
    _count: EGLint,
) -> EGLBoolean {
    // SAFETY: EGL's contract; the whole surface is presented.
    unsafe { eglSwapBuffers(dpy, surface) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSetDamageRegionKHR(
    dpy: EGLDisplay,
    surface: EGLSurface,
    rects: *mut EGLint,
    count: EGLint,
) -> EGLBoolean {
    if window(surface).is_some() {
        // Every swap presents the whole surface.
        return EGL_TRUE;
    }
    // SAFETY: forwarded.
    unsafe { host::eglSetDamageRegionKHR(dpy, surface, rects, count) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSwapInterval(dpy: EGLDisplay, interval: EGLint) -> EGLBoolean {
    match window(CURRENT.get().0 as EGLSurface) {
        Some(w) => {
            // SAFETY: the current surface's window.
            unsafe { ANativeWindow_setSwapInterval(w.lock().unwrap().window, interval) };
            EGL_TRUE
        }
        // SAFETY: forwarded.
        None => unsafe { host::eglSwapInterval(dpy, interval) },
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglQuerySurface(
    dpy: EGLDisplay,
    surface: EGLSurface,
    attr: EGLint,
    value: *mut EGLint,
) -> EGLBoolean {
    // SAFETY: forwarded with the host surface.
    unsafe { host::eglQuerySurface(dpy, host_surface(surface), attr, value) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSurfaceAttrib(
    dpy: EGLDisplay,
    surface: EGLSurface,
    attr: EGLint,
    value: EGLint,
) -> EGLBoolean {
    // SAFETY: forwarded with the host surface.
    unsafe { host::eglSurfaceAttrib(dpy, host_surface(surface), attr, value) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglPresentationTimeANDROID(
    _dpy: EGLDisplay,
    surface: EGLSurface,
    time: EGLnsecsANDROID,
) -> EGLBoolean {
    match window(surface) {
        Some(w) => {
            // SAFETY: the surface's window.
            unsafe { ANativeWindow_setBuffersTimestamp(w.lock().unwrap().window, time) };
            EGL_TRUE
        }
        None => fail(EGL_BAD_SURFACE, EGL_FALSE),
    }
}

// Images.

/// Host images of Android native buffers, and their buffer ids.
fn native_images() -> MutexGuard<'static, HashMap<usize, u64>> {
    static IMAGES: OnceLock<Mutex<HashMap<usize, u64>>> = OnceLock::new();
    IMAGES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn native_image(dpy: EGLDisplay, buffer: EGLClientBuffer, attribs: &[(i64, i64)]) -> EGLImage {
    if attribs
        .iter()
        .any(|&(k, v)| k == EGL_PROTECTED_CONTENT_EXT as i64 && v != 0)
    {
        return fail(EGL_BAD_ATTRIBUTE, std::ptr::null_mut());
    }
    // SAFETY: EGL_NATIVE_BUFFER_ANDROID buffers are ANativeWindowBuffers.
    match unsafe { buffers::import(dpy, buffer.cast()) } {
        Some(i) => {
            native_images().insert(i.image as usize, i.id);
            i.image
        }
        None => fail(EGL_BAD_PARAMETER, std::ptr::null_mut()),
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreateImageKHR(
    dpy: EGLDisplay,
    ctx: EGLContext,
    target: EGLenum,
    buffer: EGLClientBuffer,
    attribs: *const EGLint,
) -> EGLImageKHR {
    if target == EGL_NATIVE_BUFFER_ANDROID {
        // SAFETY: EGL's contract for the list.
        return native_image(dpy, buffer, &unsafe { pairs(attribs) });
    }
    // SAFETY: forwarded.
    unsafe { host::eglCreateImageKHR(dpy, ctx, target, buffer, attribs) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreateImage(
    dpy: EGLDisplay,
    ctx: EGLContext,
    target: EGLenum,
    buffer: EGLClientBuffer,
    attribs: *const EGLAttrib,
) -> EGLImage {
    if target == EGL_NATIVE_BUFFER_ANDROID {
        // SAFETY: EGL's contract for the list.
        return native_image(dpy, buffer, &unsafe { pairs(attribs.cast::<i64>()) });
    }
    // SAFETY: forwarded.
    unsafe { host::eglCreateImage(dpy, ctx, target, buffer, attribs) }
}

fn destroyed(image: EGLImage) {
    if let Some(id) = native_images().remove(&(image as usize)) {
        buffers::unmap(id);
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDestroyImageKHR(dpy: EGLDisplay, image: EGLImageKHR) -> EGLBoolean {
    // SAFETY: forwarded.
    let ok = unsafe { host::eglDestroyImageKHR(dpy, image) };
    if ok == EGL_TRUE {
        destroyed(image);
    }
    ok
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDestroyImage(dpy: EGLDisplay, image: EGLImage) -> EGLBoolean {
    // SAFETY: forwarded.
    let ok = unsafe { host::eglDestroyImage(dpy, image) };
    if ok == EGL_TRUE {
        destroyed(image);
    }
    ok
}

// Errors, procedures and callbacks.

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetError() -> EGLint {
    match ERROR.replace(EGL_SUCCESS) {
        // SAFETY: forwarded.
        EGL_SUCCESS => unsafe { host::eglGetError() },
        e => e,
    }
}

/// # Safety
/// `name` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetProcAddress(name: *const c_char) -> *const c_void {
    if name.is_null() {
        return std::ptr::null();
    }
    // SAFETY: caller contract.
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let Ok(i) = PROCS.binary_search_by(|p| p.name.as_bytes().cmp(name)) else {
        return std::ptr::null();
    };
    let p = &PROCS[i];
    let available = p.id < 0 || {
        let bits = resolved();
        bits[p.id as usize / 64] & (1 << (p.id % 64)) != 0
    };
    if available { p.addr } else { std::ptr::null() }
}

/// # Safety
/// Any arguments; the functions are never called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglSetBlobCacheFuncsANDROID(
    _dpy: EGLDisplay,
    _set: *const c_void,
    _get: *const c_void,
) {
}

/// # Safety
/// Any arguments; the callback is never called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDebugMessageControlKHR(
    _callback: *const c_void,
    _attribs: *const EGLAttrib,
) -> EGLint {
    EGL_SUCCESS
}
