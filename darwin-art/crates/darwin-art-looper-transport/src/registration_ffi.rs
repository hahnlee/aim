use crate::{Readiness, Registration};

#[repr(C)]
pub struct NativeRegistration {
    token: u64,
    mask: u32,
    reserved: u32,
}

#[repr(C)]
pub struct NativeTransitionResult {
    operation_error: i32,
    rollback_error: i32,
}

impl NativeRegistration {
    fn decode(self) -> Option<Registration> {
        if !matches!(self.mask, 0..=4) || self.reserved != 0 {
            return None;
        }
        Some(Registration {
            read: self.mask & 1 != 0,
            write: self.mask & 2 != 0,
            error_only: self.mask == 4,
            token: self.token,
        })
    }
}

/// # Safety
/// queue is null or live. fd is a host FD number, possibly already closed. The caller
/// supplies authoritative previous state and serializes transitions and close.
/// A nonzero rollback_error requires discarding/rebuilding the kernel set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_queue_transition(
    queue: *const Readiness,
    fd: i32,
    previous: NativeRegistration,
    next: NativeRegistration,
) -> NativeTransitionResult {
    let invalid = NativeTransitionResult {
        operation_error: libc::EINVAL,
        rollback_error: 0,
    };
    if fd < 0 {
        return invalid;
    }
    let (Some(previous), Some(next)) = (previous.decode(), next.decode()) else {
        return invalid;
    };
    // SAFETY: caller guarantees pointer lifetime; null is explicitly handled.
    let Some(queue) = (unsafe { queue.as_ref() }) else {
        return invalid;
    };
    match queue.transition_raw(fd, previous, next) {
        Ok(()) => NativeTransitionResult {
            operation_error: 0,
            rollback_error: 0,
        },
        Err(error) => NativeTransitionResult {
            operation_error: error.operation.raw_os_error().unwrap_or(libc::EIO),
            rollback_error: error
                .rollback
                .map_or(0, |e| e.raw_os_error().unwrap_or(libc::EIO)),
        },
    }
}
