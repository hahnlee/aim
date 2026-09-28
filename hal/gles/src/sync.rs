//! EGL syncs the guest implements (docs/gles-driver.md,
//! "Synchronization"):
//!
//! - `EGL_ANDROID_native_fence_sync`: a native fence sync is a sync_file.
//!   One made without an fd is the host's fence for the commands the current
//!   context has issued (`FN_FENCE`, which flushes them); one made from an
//!   fd takes it over. Waiting polls the fd; a server wait
//!   (`eglWaitSync`) waits on the CPU, which orders later commands after
//!   the fence as the GPU wait would.
//! - Client waits on the host's syncs without a current context, which
//!   ANGLE's Metal backend refuses.
//!
//! Every other sync is ANGLE's.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, MutexGuard, OnceLock};

use aim_hostcall::gpu::Fence;

use crate::egl::fail;
use crate::thunks::host;
use crate::types::*;

const EGL_FALSE: EGLBoolean = 0;
const EGL_TRUE: EGLBoolean = 1;
const EGL_NONE: EGLint = 0x3038;
const EGL_BAD_ATTRIBUTE: EGLint = 0x3004;
const EGL_BAD_MATCH: EGLint = 0x3009;
const EGL_BAD_PARAMETER: EGLint = 0x300c;
const EGL_SYNC_STATUS: EGLint = 0x30f1;
const EGL_SIGNALED: EGLint = 0x30f2;
const EGL_UNSIGNALED: EGLint = 0x30f3;
const EGL_TIMEOUT_EXPIRED: EGLint = 0x30f5;
const EGL_CONDITION_SATISFIED: EGLint = 0x30f6;
const EGL_SYNC_TYPE: EGLint = 0x30f7;
const EGL_SYNC_CONDITION: EGLint = 0x30f8;
const EGL_SYNC_NATIVE_FENCE_ANDROID: EGLenum = 0x3144;
const EGL_SYNC_NATIVE_FENCE_FD_ANDROID: EGLint = 0x3145;
const EGL_SYNC_NATIVE_FENCE_SIGNALED_ANDROID: EGLint = 0x3146;
const EGL_NO_NATIVE_FENCE_FD_ANDROID: EGLint = -1;
/// `EGL_FOREVER` in nanoseconds.
const FOREVER: u64 = u64::MAX;

/// A native fence sync: its sync_file.
struct NativeSync(OwnedFd);

/// Native fence syncs, by their handle (the address of their box).
fn natives() -> MutexGuard<'static, HashMap<usize, Box<NativeSync>>> {
    static NATIVES: OnceLock<Mutex<HashMap<usize, Box<NativeSync>>>> = OnceLock::new();
    NATIVES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The sync_file of a native fence sync (borrowed while `natives` is not
/// locked: a sync is not destroyed while it is used, per EGL).
fn native(sync: EGLSync) -> Option<i32> {
    natives().get(&(sync as usize)).map(|s| s.0.as_raw_fd())
}

/// Wait up to `timeout` nanoseconds for a sync_file; whether it signaled.
fn poll(fd: i32, timeout: u64) -> bool {
    let ms = if timeout == FOREVER {
        -1
    } else {
        timeout.div_ceil(1_000_000).min(i32::MAX as u64) as i32
    };
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one pollfd on our stack.
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n >= 0 {
            return n > 0;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

fn create_native(dpy: EGLDisplay, attribs: &[(i64, i64)]) -> EGLSync {
    let mut fd = EGL_NO_NATIVE_FENCE_FD_ANDROID;
    for &(k, v) in attribs {
        if k != EGL_SYNC_NATIVE_FENCE_FD_ANDROID as i64 {
            return fail(EGL_BAD_ATTRIBUTE, std::ptr::null_mut());
        }
        fd = v as EGLint;
    }
    let fd = if fd == EGL_NO_NATIVE_FENCE_FD_ANDROID {
        match aim_hostcall::guest::gpu_fence(&mut Fence {
            display: dpy as u64,
        }) {
            Ok(fd) => fd,
            // No current context, or none on this display.
            Err(_) => return fail(EGL_BAD_MATCH, std::ptr::null_mut()),
        }
    } else {
        fd
    };
    // SAFETY: EGL takes over the fd it is given, and the host's is new.
    let sync = Box::new(NativeSync(unsafe { OwnedFd::from_raw_fd(fd) }));
    let handle = &*sync as *const NativeSync as usize;
    natives().insert(handle, sync);
    handle as EGLSync
}

/// Attribute pairs of an `EGLint` or `EGLAttrib` list up to `EGL_NONE`.
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

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreateSyncKHR(
    dpy: EGLDisplay,
    ty: EGLenum,
    attribs: *const EGLint,
) -> EGLSyncKHR {
    if ty == EGL_SYNC_NATIVE_FENCE_ANDROID {
        // SAFETY: EGL's contract for the list.
        return create_native(dpy, &unsafe { pairs(attribs) });
    }
    // SAFETY: forwarded.
    unsafe { host::eglCreateSyncKHR(dpy, ty, attribs) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglCreateSync(
    dpy: EGLDisplay,
    ty: EGLenum,
    attribs: *const EGLAttrib,
) -> EGLSync {
    if ty == EGL_SYNC_NATIVE_FENCE_ANDROID {
        // SAFETY: EGL's contract for the list.
        return create_native(dpy, &unsafe { pairs(attribs.cast::<i64>()) });
    }
    // SAFETY: forwarded.
    unsafe { host::eglCreateSync(dpy, ty, attribs) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDestroySyncKHR(dpy: EGLDisplay, sync: EGLSyncKHR) -> EGLBoolean {
    if natives().remove(&(sync as usize)).is_some() {
        return EGL_TRUE;
    }
    // SAFETY: forwarded.
    unsafe { host::eglDestroySyncKHR(dpy, sync) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDestroySync(dpy: EGLDisplay, sync: EGLSync) -> EGLBoolean {
    // SAFETY: the same call.
    unsafe { eglDestroySyncKHR(dpy, sync) }
}

/// The value of a native fence sync's attribute.
fn native_attrib(fd: i32, attr: EGLint) -> Option<EGLint> {
    Some(match attr {
        EGL_SYNC_TYPE => EGL_SYNC_NATIVE_FENCE_ANDROID as EGLint,
        EGL_SYNC_CONDITION => EGL_SYNC_NATIVE_FENCE_SIGNALED_ANDROID,
        EGL_SYNC_STATUS if poll(fd, 0) => EGL_SIGNALED,
        EGL_SYNC_STATUS => EGL_UNSIGNALED,
        _ => return None,
    })
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetSyncAttribKHR(
    dpy: EGLDisplay,
    sync: EGLSyncKHR,
    attr: EGLint,
    value: *mut EGLint,
) -> EGLBoolean {
    let Some(fd) = native(sync) else {
        // SAFETY: forwarded.
        return unsafe { host::eglGetSyncAttribKHR(dpy, sync, attr, value) };
    };
    match native_attrib(fd, attr) {
        None => fail(EGL_BAD_ATTRIBUTE, EGL_FALSE),
        Some(_) if value.is_null() => fail(EGL_BAD_PARAMETER, EGL_FALSE),
        Some(v) => {
            // SAFETY: checked non-null.
            unsafe { *value = v };
            EGL_TRUE
        }
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglGetSyncAttrib(
    dpy: EGLDisplay,
    sync: EGLSync,
    attr: EGLint,
    value: *mut EGLAttrib,
) -> EGLBoolean {
    let Some(fd) = native(sync) else {
        // SAFETY: forwarded.
        return unsafe { host::eglGetSyncAttrib(dpy, sync, attr, value) };
    };
    match native_attrib(fd, attr) {
        None => fail(EGL_BAD_ATTRIBUTE, EGL_FALSE),
        Some(_) if value.is_null() => fail(EGL_BAD_PARAMETER, EGL_FALSE),
        Some(v) => {
            // SAFETY: checked non-null.
            unsafe { *value = v as EGLAttrib };
            EGL_TRUE
        }
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglDupNativeFenceFDANDROID(_dpy: EGLDisplay, sync: EGLSyncKHR) -> EGLint {
    let Some(fd) = native(sync) else {
        return fail(EGL_BAD_PARAMETER, EGL_NO_NATIVE_FENCE_FD_ANDROID);
    };
    // SAFETY: duplicating the sync's fd for the caller.
    unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglWaitSyncKHR(
    dpy: EGLDisplay,
    sync: EGLSyncKHR,
    flags: EGLint,
) -> EGLint {
    match native(sync) {
        Some(_) if flags != 0 => fail(EGL_BAD_PARAMETER, EGL_FALSE as EGLint),
        Some(fd) => poll(fd, FOREVER) as EGLint,
        // SAFETY: forwarded.
        None => unsafe { host::eglWaitSyncKHR(dpy, sync, flags) },
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglWaitSync(dpy: EGLDisplay, sync: EGLSync, flags: EGLint) -> EGLBoolean {
    match native(sync) {
        Some(_) if flags != 0 => fail(EGL_BAD_PARAMETER, EGL_FALSE),
        Some(fd) => poll(fd, FOREVER) as EGLBoolean,
        // SAFETY: forwarded.
        None => unsafe { host::eglWaitSync(dpy, sync, flags) },
    }
}

/// How often a client wait without a context looks at a host sync.
const POLL: std::time::Duration = std::time::Duration::from_micros(100);

/// `eglClientWaitSync(KHR)`. A native fence sync waits for its sync_file;
/// its commands were flushed when it was made. EGL needs no current
/// context for a wait, but ANGLE's Metal backend fails it with
/// `EGL_BAD_CONTEXT` without one (HWUI waits for its upload thread's fence
/// from another thread). Without a context the wait polls the sync's
/// status, which ANGLE answers from the display; the flush flag has
/// nothing to flush then.
unsafe fn client_wait(
    dpy: EGLDisplay,
    sync: EGLSync,
    timeout: u64,
    with_context: impl FnOnce() -> EGLint,
) -> EGLint {
    if let Some(fd) = native(sync) {
        return if poll(fd, timeout) {
            EGL_CONDITION_SATISFIED
        } else {
            EGL_TIMEOUT_EXPIRED
        };
    }
    // SAFETY: a plain query of this thread's context.
    if !unsafe { host::eglGetCurrentContext() }.is_null() {
        return with_context();
    }
    let start = std::time::Instant::now();
    loop {
        let mut status: EGLint = 0;
        // SAFETY: forwarded, into a local.
        if unsafe { host::eglGetSyncAttribKHR(dpy, sync, EGL_SYNC_STATUS, &mut status) } != EGL_TRUE
        {
            return EGL_FALSE as EGLint;
        }
        if status == EGL_SIGNALED {
            return EGL_CONDITION_SATISFIED;
        }
        let left = timeout.saturating_sub(start.elapsed().as_nanos() as u64);
        if left == 0 {
            return EGL_TIMEOUT_EXPIRED;
        }
        std::thread::sleep(POLL.min(std::time::Duration::from_nanos(left)));
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglClientWaitSync(
    dpy: EGLDisplay,
    sync: EGLSync,
    flags: EGLint,
    timeout: EGLTime,
) -> EGLint {
    // SAFETY: forwarded.
    unsafe {
        client_wait(dpy, sync, timeout, || {
            host::eglClientWaitSync(dpy, sync, flags, timeout)
        })
    }
}

/// # Safety
/// EGL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eglClientWaitSyncKHR(
    dpy: EGLDisplay,
    sync: EGLSyncKHR,
    flags: EGLint,
    timeout: EGLTimeKHR,
) -> EGLint {
    // SAFETY: forwarded.
    unsafe {
        client_wait(dpy, sync, timeout, || {
            host::eglClientWaitSyncKHR(dpy, sync, flags, timeout)
        })
    }
}
