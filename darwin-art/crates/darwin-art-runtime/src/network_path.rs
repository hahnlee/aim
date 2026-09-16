use std::ffi::c_void;
use std::sync::Mutex;

pub const NETWORK_PATH_ABI_VERSION: u32 = 1;
pub const NETWORK_PATH_STATUS_UNKNOWN: u32 = 0;
pub const NETWORK_PATH_STATUS_SATISFIED: u32 = 1;
pub const NETWORK_PATH_STATUS_UNSATISFIED: u32 = 2;
pub const NETWORK_PATH_STATUS_REQUIRES_CONNECTION: u32 = 3;

pub const NETWORK_PATH_FLAG_EXPENSIVE: u32 = 1 << 0;
pub const NETWORK_PATH_FLAG_CONSTRAINED: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkPathSnapshot {
    pub abi_version: u32,
    pub struct_size: u32,
    pub generation: u64,
    pub status: u32,
    pub flags: u32,
    pub interface_mask: u32,
    pub reserved: u32,
}

impl NetworkPathSnapshot {
    const fn initial() -> Self {
        Self {
            abi_version: NETWORK_PATH_ABI_VERSION,
            struct_size: std::mem::size_of::<Self>() as u32,
            generation: 0,
            status: NETWORK_PATH_STATUS_UNKNOWN,
            flags: 0,
            interface_mask: 0,
            reserved: 0,
        }
    }
}

struct NetworkPathMonitor {
    snapshot: Mutex<NetworkPathSnapshot>,
    platform: *mut c_void,
}

#[cfg(not(test))]
unsafe extern "C" {
    fn darwin_art_network_path_platform_start(
        context: *mut c_void,
        update: unsafe extern "C" fn(*mut c_void, u32, u32, u32),
    ) -> *mut c_void;
    fn darwin_art_network_path_platform_stop(monitor: *mut c_void);
}

unsafe extern "C" fn update_snapshot(
    context: *mut c_void,
    status: u32,
    flags: u32,
    interface_mask: u32,
) {
    if context.is_null() {
        return;
    }
    // SAFETY: the platform callback is drained before NetworkPathMonitor is
    // dropped. The platform trampoline retains no other Rust reference.
    let monitor = unsafe { &*(context.cast::<NetworkPathMonitor>()) };
    let mut snapshot = monitor
        .snapshot
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    snapshot.generation = snapshot.generation.wrapping_add(1);
    snapshot.status = match status {
        NETWORK_PATH_STATUS_SATISFIED
        | NETWORK_PATH_STATUS_UNSATISFIED
        | NETWORK_PATH_STATUS_REQUIRES_CONNECTION => status,
        _ => NETWORK_PATH_STATUS_UNKNOWN,
    };
    snapshot.flags = flags & (NETWORK_PATH_FLAG_EXPENSIVE | NETWORK_PATH_FLAG_CONSTRAINED);
    snapshot.interface_mask = interface_mask;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_network_path_create() -> *mut c_void {
    let monitor = Box::new(NetworkPathMonitor {
        snapshot: Mutex::new(NetworkPathSnapshot::initial()),
        platform: std::ptr::null_mut(),
    });
    let raw = Box::into_raw(monitor);
    #[cfg(not(test))]
    {
        // SAFETY: raw remains live until platform start fails or destroy drains
        // the callback queue. update_snapshot copies all callback facts.
        let platform =
            unsafe { darwin_art_network_path_platform_start(raw.cast(), update_snapshot) };
        if platform.is_null() {
            // SAFETY: platform start failed and therefore retained no callback.
            drop(unsafe { Box::from_raw(raw) });
            return std::ptr::null_mut();
        }
        // SAFETY: raw still denotes the allocation above and initialization is
        // single-threaded before the handle is published to Java.
        unsafe { (*raw).platform = platform };
        raw.cast()
    }
    #[cfg(test)]
    {
        raw.cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_network_path_snapshot(
    handle: *const c_void,
    output: *mut NetworkPathSnapshot,
) -> i32 {
    if handle.is_null() || output.is_null() {
        return libc::EINVAL;
    }
    // SAFETY: callers retain the opaque monitor until this copied query
    // completes. No borrowed Network.framework objects cross this boundary.
    let monitor = unsafe { &*(handle.cast::<NetworkPathMonitor>()) };
    let snapshot = *monitor
        .snapshot
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // SAFETY: output was validated and points to caller-owned writable storage.
    unsafe { output.write(snapshot) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_network_path_destroy(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let raw = handle.cast::<NetworkPathMonitor>();
    #[cfg(not(test))]
    {
        // SAFETY: the platform stop contract cancels and drains its serial
        // callback queue before returning, so raw is no longer observable.
        unsafe { darwin_art_network_path_platform_stop((*raw).platform) };
    }
    // SAFETY: this consumes the unique opaque handle exactly once.
    drop(unsafe { Box::from_raw(raw) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_snapshot_is_versioned_and_conservative() {
        let snapshot = NetworkPathSnapshot::initial();
        assert_eq!(snapshot.abi_version, NETWORK_PATH_ABI_VERSION);
        assert_eq!(
            snapshot.struct_size as usize,
            std::mem::size_of::<NetworkPathSnapshot>()
        );
        assert_eq!(snapshot.status, NETWORK_PATH_STATUS_UNKNOWN);
        assert_eq!(snapshot.generation, 0);
    }

    #[test]
    fn callback_copies_only_stable_facts() {
        let monitor = NetworkPathMonitor {
            snapshot: Mutex::new(NetworkPathSnapshot::initial()),
            platform: std::ptr::null_mut(),
        };
        unsafe {
            update_snapshot(
                (&monitor as *const NetworkPathMonitor).cast_mut().cast(),
                NETWORK_PATH_STATUS_SATISFIED,
                NETWORK_PATH_FLAG_EXPENSIVE | NETWORK_PATH_FLAG_CONSTRAINED | (1 << 31),
                0x15,
            )
        };
        let snapshot = *monitor.snapshot.lock().unwrap();
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.status, NETWORK_PATH_STATUS_SATISFIED);
        assert_eq!(
            snapshot.flags,
            NETWORK_PATH_FLAG_EXPENSIVE | NETWORK_PATH_FLAG_CONSTRAINED
        );
        assert_eq!(snapshot.interface_mask, 0x15);
    }
}
