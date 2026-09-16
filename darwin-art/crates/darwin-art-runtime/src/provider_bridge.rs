//! Thin unsafe provider callback adapter.
//!
//! Lease accounting stays in `provider.rs`; this module only owns the fixed
//! C ABI function pointers and turns them into the Rust lease table callbacks.

use core::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use darwin_art_engine_sys::{
    ProviderAcquireFn, ProviderClearHooksFn, ProviderNativeAcquireFn, ProviderNativeReleaseFn,
    ProviderReleaseFn,
};

use super::{ProviderKind, ProviderLeaseError, ProviderLeaseTable};

/// Rust-owned bridge for the process-global provider callbacks.
pub struct ProviderBridge {
    leases: ProviderLeaseTable,
    process_leases: Mutex<Vec<ProviderKind>>,
}

/// A process- or subsystem-scoped lease on one process-global native
/// provider.  Keeping the release in Drop makes early-return paths obey the
/// same zero-to-one/one-to-zero transition contract as ELF-owned leases.
pub struct ProviderLease<'a> {
    bridge: &'a ProviderBridge,
    kind: ProviderKind,
}

impl ProviderBridge {
    pub fn from_callbacks(
        acquire: ProviderNativeAcquireFn,
        release: ProviderNativeReleaseFn,
        clear: ProviderClearHooksFn,
    ) -> Self {
        Self {
            leases: ProviderLeaseTable::new(
                move |kind, authority_fd| {
                    // SAFETY: the function pointer belongs to the live engine
                    // image held by RuntimeSession.
                    unsafe { acquire(kind.raw(), authority_fd) }
                },
                move |kind| {
                    // SAFETY: same image-lifetime invariant as acquire.
                    unsafe { release(kind.raw()) }
                },
                move || {
                    // SAFETY: clear runs only after the lease table is
                    // quiescent and before the engine owner is dropped.
                    unsafe { clear() }
                },
            ),
            process_leases: Mutex::new(Vec::new()),
        }
    }

    pub fn context(&self) -> *mut c_void {
        (self as *const Self).cast_mut().cast()
    }

    pub fn acquire_callback() -> ProviderAcquireFn {
        acquire_provider
    }

    pub fn release_callback() -> ProviderReleaseFn {
        release_provider
    }

    pub fn clear(&self) -> Result<(), ProviderLeaseError> {
        let mut process_leases = self
            .process_leases
            .lock()
            .map_err(|_| ProviderLeaseError::Poisoned)?;
        while let Some(kind) = process_leases.pop() {
            if self.leases.release(kind) != 0 {
                process_leases.push(kind);
                return Err(ProviderLeaseError::ActiveLeases);
            }
        }
        self.leases.clear()
    }

    pub fn adopt_native_shutdown(&self) -> Result<(), ProviderLeaseError> {
        self.process_leases
            .lock()
            .map_err(|_| ProviderLeaseError::Poisoned)?
            .clear();
        self.leases.adopt_native_shutdown()
    }

    /// Acquire a provider for the lifetime of the Android process.
    ///
    /// Unlike a temporary `ProviderLease`, this ownership is retained by the
    /// bridge itself and released by the runtime shutdown transaction after
    /// ART and its service processes have stopped using provider-backed
    /// objects.
    pub fn acquire_process_lease(&self, kind: ProviderKind, authority_fd: i32) -> Result<(), i32> {
        let mut process_leases = self.process_leases.lock().map_err(|_| -1)?;
        if process_leases.contains(&kind) {
            return Ok(());
        }
        let status = self.leases.acquire(kind, authority_fd);
        if status != 0 {
            return Err(status);
        }
        process_leases.push(kind);
        Ok(())
    }

    pub fn acquire_lease(
        &self,
        kind: ProviderKind,
        authority_fd: i32,
    ) -> Result<ProviderLease<'_>, i32> {
        let status = self.leases.acquire(kind, authority_fd);
        if status != 0 {
            return Err(status);
        }
        Ok(ProviderLease { bridge: self, kind })
    }
}

impl Drop for ProviderLease<'_> {
    fn drop(&mut self) {
        if self.bridge.leases.release(self.kind) != 0 {
            std::process::abort();
        }
    }
}

impl crate::NativeResource for ProviderBridge {
    fn close(&mut self) -> i32 {
        0
    }

    fn clear(&mut self) -> i32 {
        ProviderBridge::clear(self).map_or(-1, |_| 0)
    }

    fn adopt_native_shutdown(&mut self) -> i32 {
        ProviderBridge::adopt_native_shutdown(self).map_or(-1, |_| 0)
    }
}

impl crate::NativeResource for Box<ProviderBridge> {
    fn close(&mut self) -> i32 {
        0
    }

    fn clear(&mut self) -> i32 {
        ProviderBridge::clear(self).map_or(-1, |_| 0)
    }

    fn adopt_native_shutdown(&mut self) -> i32 {
        ProviderBridge::adopt_native_shutdown(self).map_or(-1, |_| 0)
    }
}

impl Drop for ProviderBridge {
    fn drop(&mut self) {
        // RuntimeOwners drops the provider slot before the engine slot, so
        // the callback image is still mapped here even on an early-return
        // path that never reached RuntimeSession::shutdown_native.  Leaving
        // hooks installed would let a later ART callback jump through a
        // dangling Rust context or an unmapped engine image.  A live lease
        // cannot be made safe by best-effort cleanup; fail-stop instead of
        // permitting that use-after-unload.
        if self.clear().is_err() {
            std::process::abort();
        }
    }
}

unsafe extern "C" fn acquire_provider(context: *mut c_void, kind: u32, authority_fd: i32) -> i32 {
    if context.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(kind) = ProviderKind::try_from(kind) else {
            return -1;
        };
        // SAFETY: RuntimeSession owns the bridge until provider hooks are
        // cleared, and no callback is retained after that boundary.
        unsafe { &*context.cast::<ProviderBridge>() }
            .leases
            .acquire(kind, authority_fd)
    }))
    .unwrap_or(-1)
}

unsafe extern "C" fn release_provider(context: *mut c_void, kind: u32) -> i32 {
    if context.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(kind) = ProviderKind::try_from(kind) else {
            return -1;
        };
        // SAFETY: same lifetime invariant as acquire_provider.
        unsafe { &*context.cast::<ProviderBridge>() }
            .leases
            .release(kind)
    }))
    .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static ACQUIRES: AtomicU32 = AtomicU32::new(0);
    static RELEASES: AtomicU32 = AtomicU32::new(0);
    static CLEARS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn acquire(_: u32, _: i32) -> i32 {
        ACQUIRES.fetch_add(1, Ordering::SeqCst);
        0
    }

    unsafe extern "C" fn release(_: u32) -> i32 {
        RELEASES.fetch_add(1, Ordering::SeqCst);
        0
    }

    unsafe extern "C" fn clear() {
        CLEARS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn process_lease_survives_temporary_borrows_until_bridge_clear() {
        for (kind, fd) in [(ProviderKind::Network, -1), (ProviderKind::Filesystem, 7)] {
            ACQUIRES.store(0, Ordering::SeqCst);
            RELEASES.store(0, Ordering::SeqCst);
            CLEARS.store(0, Ordering::SeqCst);
            let bridge = ProviderBridge::from_callbacks(acquire, release, clear);

            bridge.acquire_process_lease(kind, fd).unwrap();
            let temporary = bridge.acquire_lease(kind, fd).unwrap();
            drop(temporary);
            assert_eq!(ACQUIRES.load(Ordering::SeqCst), 1);
            assert_eq!(RELEASES.load(Ordering::SeqCst), 0);

            bridge.clear().unwrap();
            assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
            assert_eq!(CLEARS.load(Ordering::SeqCst), 1);
        }
    }
}
