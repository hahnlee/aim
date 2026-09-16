//! Guest handle identity and owned opens. Numeric tokens are never pointers to
//! Rust/native objects. Closing requires the separate finalization coordinator.
use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Entry {
    image: Arc<NamespaceImage<ImageOwner>>,
    opens: Vec<Box<LinkerImageLease>>,
}
#[derive(Default)]
pub(super) struct Handles {
    entries: BTreeMap<usize, Entry>,
}

/// # Safety
/// Caller holds linker operation. source owns an exclusive opened lease; output
/// is writable. Success consumes source, error preserves it. Registry must own
/// the exact resident image. Repeated opens share a handle, not their open refs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_handle_adopt(
    registry: *const LinkerRegistry,
    source: *mut *mut LinkerImageLease,
    output: *mut usize,
) -> i32 {
    if output.is_null() || source.is_null() || source.cast::<usize>() == output {
        return -1;
    }
    unsafe { *output = 0 };
    let (Some(registry), Some(image)) =
        (unsafe { registry.as_ref() }, unsafe { (*source).as_ref() })
    else {
        return -1;
    };
    if image.0.lease.kind == 1 {
        let Some(open) = &image.1 else {
            return -4;
        };
        if Arc::strong_count(open) != 1 {
            return -7;
        }
    } else if !matches!(image.0.lease.kind, 2 | 3 | 4 | 5 | 6 | 7 | 8) {
        return -4;
    }
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        if !state
            .resident_images()
            .any(|value| Arc::ptr_eq(value, &image.0))
        {
            return -1;
        }
    }
    let Ok(mut handles) = registry.3.lock() else {
        return -2;
    };
    let key = if let Some((&key, _)) = handles
        .entries
        .iter()
        .find(|(_, entry)| Arc::ptr_eq(&entry.image, &image.0))
    {
        key
    } else {
        let Ok(key) =
            NEXT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(2))
        else {
            return -5;
        };
        handles.entries.insert(
            key,
            Entry {
                image: image.0.clone(),
                opens: Vec::new(),
            },
        );
        key
    };
    let owned = unsafe { Box::from_raw(*source) };
    unsafe {
        *source = std::ptr::null_mut();
        *output = key;
    }
    handles
        .entries
        .get_mut(&key)
        .expect("inserted handle")
        .opens
        .push(owned);
    0
}

/// # Safety
/// Live registry/output under linker operation. Returns a non-counting image
/// view for symbol/caller work. A closed-but-retained image keeps its identity
/// until eligible retirement removes the entry. Invalid tokens are not read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_handle_image(
    registry: *const LinkerRegistry,
    handle: usize,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    let Ok(handles) = registry.3.lock() else {
        return -2;
    };
    let Some(entry) = handles.entries.get(&handle) else {
        return -1;
    };
    unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(entry.image.clone(), None))) };
    0
}

/// # Safety
/// Live registry/output under linker operation. Transfer one open to close
/// coordination WITHOUT decrementing it. On pre-finalizer failure, adopt it
/// back to the same identity. This alone does not perform Android dlclose.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_handle_take_open(
    registry: *const LinkerRegistry,
    handle: usize,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    let Ok(mut handles) = registry.3.lock() else {
        return -2;
    };
    let Some(open) = handles
        .entries
        .get_mut(&handle)
        .and_then(|entry| entry.opens.pop())
    else {
        return -1;
    };
    unsafe { *output = Box::into_raw(open) };
    0
}

/// # Safety
/// Live registry under the whole close operation, after successful group
/// finalization/detachment. Removes stale guest handles; physical resource
/// release runs outside the table mutex. Cannot erase a group with opens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_handles_retire_group(
    registry: *const LinkerRegistry,
    group: u64,
) -> i32 {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    {
        let Ok(retired) = registry.2.lock() else {
            return -2;
        };
        if !retired.contains(&group) {
            return -1;
        }
    }
    let removed = {
        let Ok(mut handles) = registry.3.lock() else {
            return -2;
        };
        let keys: Vec<_> = handles
            .entries
            .iter()
            .filter(|(_, entry)| entry.image.lease.group.is_some_and(|id| id.id == group))
            .map(|(&key, _)| key)
            .collect();
        if keys
            .iter()
            .any(|key| !handles.entries[key].opens.is_empty())
        {
            return -7;
        }
        keys.into_iter()
            .map(|key| handles.entries.remove(&key).expect("selected entry"))
            .collect::<Vec<_>>()
    };
    drop(removed);
    0
}
