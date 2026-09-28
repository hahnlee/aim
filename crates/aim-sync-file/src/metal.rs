//! Fences of Metal work, for the GPU modules: a sync_file that signals when
//! an `MTLSharedEvent` reaches a value ([`fence`]), and an `MTLSharedEvent`
//! set to a value once a sync_file has signaled ([`signal_when`]).
//!
//! One `MTLSharedEventListener` per process calls back when an event
//! reaches a value it was asked about, and the callback signals the
//! fences waiting for that event and value with the time it ran.

use std::ffi::{CStr, c_char, c_void};
use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::{Mutex, OnceLock};

use crate::{State, Writer, is_sync_file, on_signal, pair};

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static METAL = c"/System/Library/Frameworks/Metal.framework/Metal" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_retain(obj: Id) -> Id;
        fn objc_release(obj: Id);
    }
}

fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature.
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute(objc_msgSend()) };
        unsafe { f($obj, sel($sel) $(, $a)*) }
    }};
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

/// Fences in flight: their event (a reference of ours), value and writer.
static PENDING: Mutex<Vec<(usize, u64, Writer)>> = Mutex::new(Vec::new());

/// The listener's callback: `event` reached `value`.
unsafe extern "C" fn reached(_block: *const Block, event: Id, value: u64) {
    let done: Vec<_> = {
        let mut pending = PENDING.lock().unwrap();
        let (done, left) = std::mem::take(&mut *pending)
            .into_iter()
            .partition(|(e, v, _)| *e == event as usize && *v <= value);
        *pending = left;
        done
    };
    for (_, _, writer) in done {
        writer.signal(1);
        // SAFETY: the reference `fence` took.
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

/// A sync_file that signals when `event` reaches `value`.
///
/// # Safety
/// `event` must be an `MTLSharedEvent`.
pub unsafe fn fence(event: Id, value: u64) -> io::Result<OwnedFd> {
    let (listener, block) = listener().ok_or(io::ErrorKind::Unsupported)?;
    let (file, writer) = pair()?;
    // SAFETY: caller contract; the reference lasts until the event
    // reaches `value`.
    unsafe { objc_retain(event) };
    PENDING
        .lock()
        .unwrap()
        .push((event as usize, value, writer));
    send!(event, c"notifyListener:atValue:block:" => (),
        Id = listener, u64 = value, *const Block = block);
    Ok(file)
}

/// Set `event` to `value` once the sync_file `fd` has signaled.
///
/// # Safety
/// `event` must be an `MTLSharedEvent`.
pub unsafe fn signal_when(fd: BorrowedFd, event: Id, value: u64) -> io::Result<()> {
    if !is_sync_file(fd) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: caller contract; the reference lasts until it is set.
    let event = unsafe { objc_retain(event) } as usize;
    on_signal(fd, move |_: State| {
        let event = event as Id;
        send!(event, c"setSignaledValue:" => (), u64 = value);
        // SAFETY: the reference taken above.
        unsafe { objc_release(event) };
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait;
    use std::os::fd::AsFd;
    use std::time::{Duration, Instant};

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> Id;
    }

    fn event() -> Id {
        // SAFETY: no preconditions.
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        assert!(!device.is_null());
        send!(device, c"newSharedEvent" => Id)
    }

    #[test]
    fn a_fence_signals_when_its_event_reaches_the_value() {
        let event = event();
        // SAFETY: an MTLSharedEvent.
        let (one, two) = unsafe { (fence(event, 1).unwrap(), fence(event, 2).unwrap()) };
        send!(event, c"setSignaledValue:" => (), u64 = 1);
        assert!(wait(one.as_fd(), 5000));
        assert!(!wait(two.as_fd(), 50));
        send!(event, c"setSignaledValue:" => (), u64 = 3);
        assert!(wait(two.as_fd(), 5000));
        // SAFETY: our reference from newSharedEvent.
        unsafe { objc_release(event) };
    }

    #[test]
    fn a_signaled_fence_sets_the_event() {
        let event = event();
        let (file, writer) = pair().unwrap();
        // SAFETY: an MTLSharedEvent.
        unsafe { signal_when(file.as_fd(), event, 7).unwrap() };
        let value = || send!(event, c"signaledValue" => u64);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(value(), 0);
        writer.signal(1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while value() != 7 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(value(), 7);
        // Not a sync_file.
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        // SAFETY: as above.
        assert!(unsafe { signal_when(a.as_fd(), event, 8) }.is_err());
        // SAFETY: our reference from newSharedEvent.
        unsafe { objc_release(event) };
    }
}
