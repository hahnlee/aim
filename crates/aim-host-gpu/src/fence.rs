//! Native fences (`EGL_ANDROID_native_fence_sync`; docs/gles-driver.md,
//! "Synchronization"): a sync_file that signals when the GPU has finished
//! the commands the current context issued before it.
//!
//! Each fence is a new `MTLSharedEvent` that ANGLE signals after those
//! commands (an `EGL_ANGLE_metal_shared_event_sync` sync), in the command
//! buffer that the flush commits; [`aim_sync_file::metal`] signals the
//! sync_file when the event reaches its value.

use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::OnceLock;

use crate::EINVAL;
use crate::metal::{Id, device, objc_release, send};
use crate::present::entry;

const EGL_SYNC_METAL_SHARED_EVENT_ANGLE: u32 = 0x34d8;
const EGL_SYNC_METAL_SHARED_EVENT_OBJECT_ANGLE: isize = 0x34d9;
const EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_LO_ANGLE: isize = 0x34da;
const EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_HI_ANGLE: isize = 0x34db;
const EGL_SYNC_METAL_SHARED_EVENT_SIGNALED_ANGLE: isize = 0x34dc;
const EGL_SYNC_CONDITION: isize = 0x30f8;
const EGL_NONE: isize = 0x3038;

/// The ANGLE entry points a fence uses.
struct Egl {
    current_context: unsafe extern "C" fn() -> usize,
    current_display: unsafe extern "C" fn() -> usize,
    create_sync: unsafe extern "C" fn(usize, u32, *const isize) -> usize,
    destroy_sync: unsafe extern "C" fn(usize, usize) -> u32,
    wait_sync: unsafe extern "C" fn(usize, usize, i32) -> u32,
    flush: unsafe extern "C" fn(),
}

fn egl() -> Option<&'static Egl> {
    static EGL: OnceLock<Option<Egl>> = OnceLock::new();
    EGL.get_or_init(|| {
        // SAFETY: each field's type is the C signature of that entry point.
        unsafe {
            Some(Egl {
                current_context: entry(c"eglGetCurrentContext")?,
                current_display: entry(c"eglGetCurrentDisplay")?,
                create_sync: entry(c"eglCreateSync")?,
                destroy_sync: entry(c"eglDestroySync")?,
                wait_sync: entry(c"eglWaitSync")?,
                flush: entry(c"glFlush")?,
            })
        }
    })
    .as_ref()
}

/// Import a sync_file as a GPU dependency, as Vulkan's signal_after does.
/// ANGLE's SIGNALED condition imports the event without enqueueing a signal
/// of its own; only the sync_file's completion may advance it.
pub fn wait(display: usize, fd: BorrowedFd<'_>) -> Result<(), i64> {
    let egl = egl().ok_or(EINVAL)?;
    // SAFETY: a query of this thread's current context.
    if unsafe { (egl.current_context)() == 0 || (egl.current_display)() != display }
        || !aim_sync_file::is_sync_file(fd)
    {
        return Err(EINVAL);
    }
    if aim_sync_file::state(fd) != aim_sync_file::State::Active {
        return Ok(());
    }
    let device = device(display).ok_or(EINVAL)?;
    let event = send!(device, c"newSharedEvent" => Id);
    if event.is_null() {
        return Err(EINVAL);
    }
    let attribs = [
        EGL_SYNC_METAL_SHARED_EVENT_OBJECT_ANGLE,
        event as isize,
        EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_LO_ANGLE,
        1,
        EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_HI_ANGLE,
        0,
        EGL_SYNC_CONDITION,
        EGL_SYNC_METAL_SHARED_EVENT_SIGNALED_ANGLE,
        EGL_NONE,
    ];
    // SAFETY: the event and NONE-terminated attributes remain live.
    let sync =
        unsafe { (egl.create_sync)(display, EGL_SYNC_METAL_SHARED_EVENT_ANGLE, attribs.as_ptr()) };
    let result = if sync == 0 {
        Err(EINVAL)
    } else {
        // SAFETY: a live MTLSharedEvent. The waiter retains the event and
        // duplicates the fd before returning; destroying the EGLSync after
        // serverWait leaves the queued GPU dependency intact.
        unsafe { aim_sync_file::metal::signal_when(fd, event, 1) }
            .map_err(|_| EINVAL)
            .and_then(|()| {
                // SAFETY: a sync on this display and its current context.
                (unsafe { (egl.wait_sync)(display, sync, 0) } != 0)
                    .then_some(())
                    .ok_or(EINVAL)
            })
    };
    // SAFETY: references created above; the GPU and waiter keep theirs.
    unsafe {
        if sync != 0 {
            (egl.destroy_sync)(display, sync);
        }
        objc_release(event);
    }
    result
}

/// A fence for the commands the current context has issued on ANGLE's
/// `display`; the commands are flushed.
pub fn fence(display: usize) -> Result<OwnedFd, i64> {
    let Some(egl) = egl() else {
        return Err(EINVAL);
    };
    // SAFETY: a plain query of this thread's context.
    if unsafe { (egl.current_context)() } == 0 {
        return Err(EINVAL);
    }
    let device = device(display).ok_or(EINVAL)?;
    let event = send!(device, c"newSharedEvent" => Id);
    if event.is_null() {
        return Err(EINVAL);
    }
    let attribs = [
        EGL_SYNC_METAL_SHARED_EVENT_OBJECT_ANGLE,
        event as isize,
        EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_LO_ANGLE,
        1,
        EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_HI_ANGLE,
        0,
        EGL_NONE,
    ];
    // SAFETY: ANGLE's eglCreateSync with a NONE-terminated list; the sync
    // queues the event's signal after the context's commands.
    let sync =
        unsafe { (egl.create_sync)(display, EGL_SYNC_METAL_SHARED_EVENT_ANGLE, attribs.as_ptr()) };
    if sync == 0 {
        // SAFETY: our reference from newSharedEvent.
        unsafe { objc_release(event) };
        return Err(EINVAL);
    }
    // SAFETY: a new MTLSharedEvent.
    let file = unsafe { aim_sync_file::metal::fence(event, 1) };
    // SAFETY: ANGLE's glFlush and eglDestroySync on the sync made above;
    // the queued signal and the fence keep their own references to the
    // event.
    unsafe {
        objc_release(event);
        (egl.flush)();
        (egl.destroy_sync)(display, sync);
    }
    file.map_err(|_| EINVAL)
}
