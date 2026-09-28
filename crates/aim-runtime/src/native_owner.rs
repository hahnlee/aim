//! Opaque owner for native graph resources.
//!
//! C++ owns the resource implementations, but it no longer owns the release
//! order. Each resource is registered with a Rust slot and a typed C callback;
//! destruction walks slots in descending order and reports the first callback
//! failure without leaking the remaining resources.

use core::ffi::c_void;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

pub type RuntimeNativeOwnerDropFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;

fn owner_registry() -> &'static Mutex<HashSet<usize>> {
    static REGISTRY: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

fn owner_is_live(owner: *mut RuntimeNativeOwner) -> bool {
    !owner.is_null()
        && owner_registry()
            .lock()
            .is_ok_and(|registry| registry.contains(&(owner as usize)))
}

struct Slot {
    order: u32,
    value: *mut c_void,
    context: *mut c_void,
    drop_fn: RuntimeNativeOwnerDropFn,
}

/// Owner-thread-only native resource collection.
pub struct RuntimeNativeOwner {
    slots: Vec<Slot>,
    _owner_thread: PhantomData<Rc<()>>,
}

impl RuntimeNativeOwner {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            _owner_thread: PhantomData,
        }
    }

    fn attach(
        &mut self,
        order: u32,
        value: *mut c_void,
        context: *mut c_void,
        drop_fn: RuntimeNativeOwnerDropFn,
    ) -> i32 {
        if value.is_null() || self.slots.iter().any(|slot| slot.order == order) {
            return -1;
        }
        self.slots.push(Slot {
            order,
            value,
            context,
            drop_fn,
        });
        0
    }

    fn lookup(&self, order: u32) -> *mut c_void {
        self.slots
            .iter()
            .find(|slot| slot.order == order)
            .map_or(core::ptr::null_mut(), |slot| slot.value)
    }

    fn destroy(&mut self) -> i32 {
        self.slots.sort_unstable_by_key(|slot| slot.order);
        let mut first_error = 0;
        while let Some(slot) = self.slots.pop() {
            // SAFETY: callback/value/context were validated at attach and are
            // consumed exactly once by this owner-thread destruction path.
            let status = unsafe { (slot.drop_fn)(slot.value, slot.context) };
            if status != 0 && first_error == 0 {
                first_error = status;
            }
        }
        first_error
    }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// The returned owner must be used only on its creating owner thread and
/// destroyed exactly once with `aim_runtime_native_owner_destroy`.
pub unsafe extern "C" fn aim_runtime_native_owner_create() -> *mut RuntimeNativeOwner {
    let owner = Box::into_raw(Box::new(RuntimeNativeOwner::new()));
    let Ok(mut registry) = owner_registry().lock() else {
        // SAFETY: the allocation was created immediately above and has not
        // been published to another caller.
        unsafe { drop(Box::from_raw(owner)) };
        return core::ptr::null_mut();
    };
    registry.insert(owner as usize);
    owner
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `owner` must be a live handle returned by `create`; `value`, `context`, and
/// `drop_fn` must remain valid until the owner destroys the slot.
pub unsafe extern "C" fn aim_runtime_native_owner_attach(
    owner: *mut RuntimeNativeOwner,
    order: u32,
    value: *mut c_void,
    context: *mut c_void,
    drop_fn: Option<RuntimeNativeOwnerDropFn>,
) -> i32 {
    if !owner_is_live(owner) || value.is_null() {
        return -1;
    }
    let Some(drop_fn) = drop_fn else {
        return -1;
    };
    // SAFETY: the caller owns a live owner returned by create and serializes
    // attach with destruction on the owner thread.
    // SAFETY: the registry check above proves that `owner` was returned by
    // create and has not yet entered destruction.
    unsafe { &mut *owner }.attach(order, value, context, drop_fn)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `owner` must be a handle returned by `create`; an unknown or already
/// destroyed handle returns null without dereferencing it.
pub unsafe extern "C" fn aim_runtime_native_owner_lookup(
    owner: *mut RuntimeNativeOwner,
    order: u32,
) -> *mut c_void {
    if !owner_is_live(owner) {
        return core::ptr::null_mut();
    }
    // SAFETY: the registry check above proves that `owner` is live for this
    // owner-thread lookup.
    unsafe { (&*owner).lookup(order) }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `owner` must be a live handle returned by `create` and no other thread may
/// access it while destruction callbacks run.
pub unsafe extern "C" fn aim_runtime_native_owner_destroy(owner: *mut RuntimeNativeOwner) -> i32 {
    let Ok(mut registry) = owner_registry().lock() else {
        return -1;
    };
    if owner.is_null() {
        return 0;
    }
    if !registry.remove(&(owner as usize)) {
        return -1;
    }
    drop(registry);
    // SAFETY: ownership is uniquely transferred to this call. Slot callbacks
    // run before the allocation itself is reclaimed.
    let mut owner = unsafe { Box::from_raw(owner) };
    owner.destroy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    unsafe extern "C" fn record(_value: *mut c_void, context: *mut c_void) -> i32 {
        // SAFETY: test contexts are pointers to a live local mutex for the
        // duration of owner destruction.
        let order = unsafe { &*context.cast::<Mutex<Vec<u32>>>() };
        let value = unsafe { *_value.cast::<u32>() };
        order.lock().unwrap().push(value);
        0
    }

    unsafe extern "C" fn noop(_value: *mut c_void, _context: *mut c_void) -> i32 {
        0
    }

    #[test]
    fn owner_destroys_native_slots_in_reverse_order() {
        let order = Mutex::new(Vec::new());
        let mut owner = RuntimeNativeOwner::new();
        let values = [1_u32, 2, 3];
        for (index, value) in values.iter().enumerate() {
            let context = &order as *const Mutex<Vec<u32>> as *mut c_void;
            assert_eq!(
                owner.attach(
                    index as u32,
                    value as *const u32 as *mut c_void,
                    context,
                    record
                ),
                0
            );
        }
        assert_eq!(owner.destroy(), 0);
        assert_eq!(*order.lock().unwrap(), vec![3_u32, 2, 1]);
    }

    #[test]
    fn duplicate_orders_are_rejected_without_consuming_value() {
        let mut owner = RuntimeNativeOwner::new();
        let value = Box::into_raw(Box::new(1_u32)).cast::<c_void>();
        assert_eq!(owner.attach(7, value, value, noop), 0);
        assert_eq!(owner.attach(7, value, value, noop), -1);
        assert_eq!(owner.destroy(), 0);
        // The test callback does not own the allocation; reclaim it here.
        unsafe { drop(Box::from_raw(value.cast::<u32>())) };
    }

    #[test]
    fn published_owner_lookup_rejects_stale_handles() {
        let value = Box::into_raw(Box::new(7_u32)).cast::<c_void>();
        let owner = unsafe { aim_runtime_native_owner_create() };
        assert!(!owner.is_null());
        assert_eq!(
            unsafe { aim_runtime_native_owner_attach(owner, 120, value, value, Some(noop)) },
            0
        );
        assert_eq!(
            unsafe { aim_runtime_native_owner_lookup(owner, 120) },
            value
        );
        assert_eq!(unsafe { aim_runtime_native_owner_destroy(owner) }, 0);
        assert!(unsafe { aim_runtime_native_owner_lookup(owner, 120) }.is_null());
        // The no-op callback does not consume this test allocation.
        unsafe { drop(Box::from_raw(value.cast::<u32>())) };
    }
}
