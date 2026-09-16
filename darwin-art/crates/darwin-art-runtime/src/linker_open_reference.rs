//! Logical ELF opens, distinct from lookup leases and mapping Arc counts.
//! Dependency references and eligible unpublication are separate linker work;
//! reaching zero here does not finalize or unmap namespace images.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct OpenReference(Arc<AtomicUsize>);
impl OpenReference {
    fn acquire(count: &Arc<AtomicUsize>) -> Option<Arc<Self>> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .ok()?;
        Some(Arc::new(Self(count.clone())))
    }
}
impl Drop for OpenReference {
    fn drop(&mut self) {
        let previous = self.0.fetch_sub(1, Ordering::AcqRel);
        assert!(previous != 0, "unbalanced local-group open reference");
    }
}

/// # Safety
/// Live admitted lease, writable output; caller owns linker operation ordering.
/// Acquires one logical ELF open. Cloning the returned lease shares that open,
/// not an additional dlopen. Last release balances it. Unknown groups return -4.
/// Does not grant namespace visibility, perform constructors or unload a group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_acquire_open(
    lease: *const LinkerImageLease,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = std::ptr::null_mut();
    }
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    let Some(count) = &lease.0.lease.group_opens else {
        return -4;
    };
    let Some(retired) = &lease.0.lease.group_retired else {
        return -4;
    };
    if retired.load(Ordering::Acquire) {
        return -1;
    }
    let Some(reference) = OpenReference::acquire(count) else {
        return -5;
    };
    if retired.load(Ordering::Acquire) {
        return -1;
    }
    unsafe {
        *output = Box::into_raw(Box::new(LinkerImageLease(lease.0.clone(), Some(reference))));
    }
    0
}

/// # Safety
/// Both slots are exclusively owned, writable and distinct. `source` owns a
/// live lease; `output` is an empty destination. Caller serializes linker work
/// and drains internal aliases before close. This consumes one logical open
/// while retaining the exact image as a non-counting finalization view.
/// Returns -7 for shared opens, -4 for a non-open view. Errors preserve source.
/// Does not finalize, unpublish, or release dependencies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_consume_open(
    source: *mut *mut LinkerImageLease,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if source.is_null() || output.is_null() || source == output {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(lease) = (unsafe { (*source).as_ref() }) else {
        return -1;
    };
    let Some(reference) = &lease.1 else {
        return -4;
    };
    if Arc::strong_count(reference) != 1 {
        return -7;
    }
    let view = Box::new(LinkerImageLease(lease.0.clone(), None));
    unsafe {
        let owned = Box::from_raw(*source);
        *source = std::ptr::null_mut();
        drop(owned);
        *output = Box::into_raw(view);
    }
    0
}

/// # Safety
/// Live lease, writable count. Counts explicit opens only, NOT dependency refs,
/// Arc owners or unload eligibility. Unknown legacy groups return -4 and zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_open_count(
    lease: *const LinkerImageLease,
    output: *mut usize,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = 0;
    }
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    let Some(count) = &lease.0.lease.group_opens else {
        return -4;
    };
    unsafe {
        *output = count.load(Ordering::Acquire);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counter_exhaustion_cannot_wrap_or_change_state() {
        let count = Arc::new(AtomicUsize::new(usize::MAX));
        assert!(OpenReference::acquire(&count).is_none());
        assert_eq!(count.load(Ordering::Acquire), usize::MAX);
    }
}
