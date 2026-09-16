use crate::{Interest, Readiness, ReadyEvent};

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct NativeEvent {
    token: u64,
    flags: u32,
    reserved: u32,
}

// Android identifies a registration by its sequence number. A single kqueue
// batch may contain separate read/write filters for that same registration.
// Preserve first occurrence order and never merge different (including stale)
// sequences; original Looper owns stale sequence rejection.
fn merge_responses(ready: Vec<ReadyEvent>) -> Vec<NativeEvent> {
    let mut merged: Vec<NativeEvent> = Vec::with_capacity(ready.len());
    for event in ready {
        let flags = match event.interest {
            Interest::Read => 1,
            Interest::Write => 2,
            Interest::ErrorOnly => 0,
        } | if event.eof { 4 } else { 0 }
            | if event.error { 8 } else { 0 };
        if let Some(existing) = merged.iter_mut().find(|item| item.token == event.token) {
            existing.flags |= flags;
        } else {
            merged.push(NativeEvent {
                token: event.token,
                flags,
                reserved: 0,
            });
        }
    }
    merged
}

fn error(e: std::io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EIO)
}

/// # Safety
/// out is null or writable for one pointer; caller owns successful result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_queue_create(out: *mut *mut Readiness) -> i32 {
    if out.is_null() {
        return libc::EINVAL;
    }
    // SAFETY: caller supplies writable output.
    unsafe {
        *out = std::ptr::null_mut();
    }
    match Readiness::new() {
        Ok(queue) => {
            // SAFETY: transfer unique allocation ownership.
            unsafe {
                *out = Box::into_raw(Box::new(queue));
            }
            0
        }
        Err(e) => error(e),
    }
}

/// # Safety
/// queue is null or created by this ABI, destroyed exactly once after quiescence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_queue_destroy(queue: *mut Readiness) {
    if !queue.is_null() {
        // SAFETY: caller transfers the unique allocation back.
        drop(unsafe { Box::from_raw(queue) });
    }
}

/// # Safety
/// queue is null or live. fd is a host descriptor number, possibly already closed.
/// Registration borrows it; callers must serialize changes/close as appropriate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_queue_change(
    queue: *const Readiness,
    fd: i32,
    interest: u32,
    remove: u32,
    token: u64,
) -> i32 {
    if fd < 0 || remove > 2 {
        return libc::EINVAL;
    }
    let interest = match interest {
        1 => Interest::Read,
        2 => Interest::Write,
        _ => return libc::EINVAL,
    };
    // SAFETY: caller guarantees queue lifetime; null is explicitly rejected.
    let Some(queue) = (unsafe { queue.as_ref() }) else {
        return libc::EINVAL;
    };
    let result = if remove == 2 {
        queue.update_raw(fd, interest, token)
    } else if remove == 1 {
        queue.remove_raw(fd, interest)
    } else {
        queue.register_raw(fd, interest, token)
    };
    result.map_or_else(error, |()| 0)
}

/// # Safety
/// queue is null or live. events points to capacity writable NativeEvents and
/// count to a writable u32, neither aliasing queue or each other. Capacity is
/// bounded to 1024 for this native Looper ABI (Android currently requests 16).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_queue_wait(
    queue: *const Readiness,
    events: *mut NativeEvent,
    capacity: u32,
    timeout_ms: i32,
    count: *mut u32,
) -> i32 {
    if events.is_null() || count.is_null() || capacity == 0 || capacity > 1024 {
        return libc::EINVAL;
    }
    // SAFETY: caller guarantees queue lifetime.
    let Some(queue) = (unsafe { queue.as_ref() }) else {
        return libc::EINVAL;
    };
    // SAFETY: output count is writable, zero on kernel failure.
    unsafe {
        *count = 0;
    }
    match queue.wait(capacity as usize, timeout_ms) {
        Ok(ready) => {
            let merged = merge_responses(ready);
            let length = merged.len();
            for (i, event) in merged.into_iter().enumerate() {
                // SAFETY: merged length <= ready length <= caller capacity.
                unsafe {
                    events.add(i).write(event);
                }
            }
            // SAFETY: output count is writable.
            unsafe {
                *count = length as u32;
            }
            0
        }
        Err(e) => error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_preserves_masks_order_and_distinct_sequences() {
        let events = vec![
            ReadyEvent {
                token: 9,
                interest: Interest::Write,
                eof: false,
                error: false,
            },
            ReadyEvent {
                token: u64::MAX,
                interest: Interest::Read,
                eof: false,
                error: false,
            },
            ReadyEvent {
                token: 9,
                interest: Interest::Read,
                eof: true,
                error: true,
            },
        ];
        assert_eq!(
            merge_responses(events),
            vec![
                NativeEvent {
                    token: 9,
                    flags: 15,
                    reserved: 0
                },
                NativeEvent {
                    token: u64::MAX,
                    flags: 1,
                    reserved: 0
                },
            ]
        );
        assert!(merge_responses(vec![]).is_empty());
    }
}
