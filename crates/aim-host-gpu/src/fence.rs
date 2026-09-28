//! Native fences (`EGL_ANDROID_native_fence_sync`; docs/gles-driver.md,
//! "Synchronization"): a sync_file that signals when the GPU has finished
//! the commands the current context issued before it.
//!
//! Each fence is a new `MTLSharedEvent` that ANGLE signals after those
//! commands (an `EGL_ANGLE_metal_shared_event_sync` sync), in the command
//! buffer that the flush commits. One `MTLSharedEventListener` per process
//! calls back when an event reaches its value, and the callback signals the
//! fence's writer ([`aim_sync_file`]) with the time it ran.

use std::ffi::c_void;
use std::os::fd::OwnedFd;
use std::sync::{Mutex, OnceLock};

use aim_sync_file::Writer;

use crate::EINVAL;
use crate::metal::{Id, device, objc_getClass, objc_release, send};
use crate::present::entry;

const EGL_SYNC_METAL_SHARED_EVENT_ANGLE: u32 = 0x34d8;
const EGL_SYNC_METAL_SHARED_EVENT_OBJECT_ANGLE: isize = 0x34d9;
const EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_LO_ANGLE: isize = 0x34da;
const EGL_SYNC_METAL_SHARED_EVENT_SIGNAL_VALUE_HI_ANGLE: isize = 0x34db;
const EGL_NONE: isize = 0x3038;

/// The ANGLE entry points a fence uses.
struct Egl {
    current_context: unsafe extern "C" fn() -> usize,
    create_sync: unsafe extern "C" fn(usize, u32, *const isize) -> usize,
    destroy_sync: unsafe extern "C" fn(usize, usize) -> u32,
    flush: unsafe extern "C" fn(),
}

fn egl() -> Option<&'static Egl> {
    static EGL: OnceLock<Option<Egl>> = OnceLock::new();
    EGL.get_or_init(|| {
        // SAFETY: each field's type is the C signature of that entry point.
        unsafe {
            Some(Egl {
                current_context: entry(c"eglGetCurrentContext")?,
                create_sync: entry(c"eglCreateSync")?,
                destroy_sync: entry(c"eglDestroySync")?,
                flush: entry(c"glFlush")?,
            })
        }
    })
    .as_ref()
}

/// A block literal without captures (`_NSConcreteGlobalBlock`).
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*const Block, Id, u64),
    descriptor: &'static BlockDescriptor,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

// SAFETY: immutable after construction.
unsafe impl Send for Block {}
unsafe impl Sync for Block {}

#[link(name = "System")]
unsafe extern "C" {
    static _NSConcreteGlobalBlock: [*const c_void; 32];
}

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

/// Writers of the fences in flight, by their event.
static PENDING: Mutex<Vec<(usize, Writer)>> = Mutex::new(Vec::new());

/// The listener's callback: `event` reached its value.
unsafe extern "C" fn reached(_block: *const Block, event: Id, _value: u64) {
    let writer = {
        let mut pending = PENDING.lock().unwrap();
        pending
            .iter()
            .position(|(e, _)| *e == event as usize)
            .map(|i| pending.swap_remove(i).1)
    };
    if let Some(w) = writer {
        w.signal(1);
        // SAFETY: the reference `fence` made the event with.
        unsafe { objc_release(event) };
    }
}

/// The process's listener and its block.
fn listener() -> Option<(Id, &'static Block)> {
    static LISTENER: OnceLock<Option<(usize, Block)>> = OnceLock::new();
    LISTENER
        .get_or_init(|| {
            static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
                reserved: 0,
                size: size_of::<Block>(),
            };
            // SAFETY: a class name.
            let class = unsafe { objc_getClass(c"MTLSharedEventListener".as_ptr()) };
            if class.is_null() {
                return None;
            }
            let listener = send!(class, c"alloc" => Id);
            let listener = send!(listener, c"init" => Id);
            let block = Block {
                isa: (&raw const _NSConcreteGlobalBlock).cast(),
                flags: BLOCK_IS_GLOBAL,
                reserved: 0,
                invoke: reached,
                descriptor: &DESCRIPTOR,
            };
            (!listener.is_null()).then_some((listener as usize, block))
        })
        .as_ref()
        .map(|(l, b)| (*l as Id, b))
}

/// A fence for the commands the current context has issued on ANGLE's
/// `display`; the commands are flushed.
pub fn fence(display: usize) -> Result<OwnedFd, i64> {
    let (Some(egl), Some((listener, block))) = (egl(), listener()) else {
        return Err(EINVAL);
    };
    // SAFETY: a plain query of this thread's context.
    if unsafe { (egl.current_context)() } == 0 {
        return Err(EINVAL);
    }
    let device = device(display).ok_or(EINVAL)?;
    let (file, writer) = aim_sync_file::pair().map_err(|_| EINVAL)?;
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
    PENDING.lock().unwrap().push((event as usize, writer));
    send!(event, c"notifyListener:atValue:block:" => (),
        Id = listener, u64 = 1, *const Block = block);
    // SAFETY: ANGLE's glFlush and eglDestroySync on the sync made above;
    // the queued signal keeps its own reference to the event.
    unsafe {
        (egl.flush)();
        (egl.destroy_sync)(display, sync);
    }
    Ok(file)
}
