//! Concrete owner-thread runtime session.
//!
//! This is the single Rust value that joins lifecycle state with the native
//! resources it governs.  Keeping the slots here prevents a caller from
//! accidentally dropping a provider or surface before the lifecycle has
//! recorded its reverse-order lease release.

use crate::owners::RuntimeOwners;
use crate::{RuntimeError, RuntimeLifecycle, RuntimePhase, Subsystem, SubsystemLease};
use darwin_art_engine_sys::LifecycleHooks;

/// Native resource contract used by the Rust-owned shutdown coordinator.
///
/// Implementations are deliberately tiny ABI adapters: they close or clear
/// an already-borrowed native object and return its status.  The ordering,
/// rollback, and owner-thread checks remain in `RuntimeSession` rather than
/// being duplicated by each host frontend.
pub trait NativeResource {
    fn close(&mut self) -> i32;

    /// True only when native process teardown actually released providers.
    /// A successful pre-entry close is not that event. Default to normal
    /// counted provider release rather than discarding ownership records.
    fn providers_released_by_native_shutdown(&self) -> bool {
        false
    }

    /// Complete native destruction after the owning process has shut down but
    /// before the dynamic image containing the callback is released. Most
    /// resources have no second phase; resources whose destroy callback lives
    /// in the engine image override this hook.
    fn finalize(&mut self) -> i32 {
        0
    }

    fn clear(&mut self) -> i32 {
        0
    }

    /// Reconcile Rust lease bookkeeping after native process teardown has
    /// already released the corresponding owner resources.
    fn adopt_native_shutdown(&mut self) -> i32 {
        0
    }
}

/// Owns one runtime's lifecycle and its concrete native resources.
pub struct RuntimeSession<E, P, S, G = ()> {
    lifecycle: RuntimeLifecycle,
    owners: RuntimeOwners<E, P, S, G>,
}

impl<E, P, S, G> RuntimeSession<E, P, S, G> {
    pub fn new() -> Self {
        Self {
            lifecycle: RuntimeLifecycle::new(),
            owners: RuntimeOwners::new(),
        }
    }

    pub const fn phase(&self) -> RuntimePhase {
        self.lifecycle.phase()
    }

    pub const fn failure(&self) -> Option<RuntimeError> {
        self.lifecycle.failure()
    }

    /// Construct the synchronous native lifecycle bridge. The table is valid
    /// only while this session remains alive on its owner thread.
    pub fn native_lifecycle_hooks(&mut self) -> LifecycleHooks {
        self.lifecycle.native_hooks()
    }

    pub fn start(&mut self) -> Result<(), RuntimeError> {
        self.lifecycle.start()
    }

    pub fn mark_running(&mut self) -> Result<(), RuntimeError> {
        self.lifecycle.mark_running()
    }

    pub fn begin_shutdown(&mut self) -> Result<(), RuntimeError> {
        self.lifecycle.begin_shutdown()
    }

    pub fn finish_shutdown(&mut self) -> Result<(), RuntimeError> {
        self.lifecycle.finish_shutdown()
    }

    pub fn fail(&mut self, error: RuntimeError) {
        self.lifecycle.fail(error);
    }

    pub fn install_subsystem(
        &mut self,
        subsystem: Subsystem,
    ) -> Result<SubsystemLease, RuntimeError> {
        self.lifecycle.install_subsystem(subsystem)
    }

    pub fn uninstall_subsystem(&mut self, lease: SubsystemLease) -> Result<(), RuntimeError> {
        self.lifecycle.uninstall_subsystem(lease)
    }

    /// Uninstall the newest subsystem without exporting its lease token to the
    /// host orchestration layer. The lifecycle validates owner thread,
    /// generation, session identity, and reverse order internally.
    pub fn uninstall_latest_subsystem(&mut self) -> Result<Option<Subsystem>, RuntimeError> {
        self.lifecycle.uninstall_latest_subsystem()
    }

    pub fn subsystem_active(&self, subsystem: Subsystem) -> bool {
        self.lifecycle.subsystem_active(subsystem)
    }

    pub fn assert_owner(&self) -> Result<(), RuntimeError> {
        self.lifecycle.assert_owner()
    }

    /// Returns the attached engine without exposing the owner slots.
    pub fn engine(&self) -> Option<&E> {
        self.owners.engine()
    }

    /// Borrow the live engine through the session owner. This keeps the
    /// native engine inaccessible once the session has been dropped and makes
    /// the Rust session the sole owner even during bootstrap calls.
    pub fn engine_mut(&mut self) -> Option<&mut E> {
        self.owners.engine_mut()
    }

    /// Returns the attached provider without exposing the owner slots.
    pub fn provider(&self) -> Option<&P> {
        self.owners.provider()
    }

    pub fn provider_mut(&mut self) -> Option<&mut P> {
        self.owners.provider_mut()
    }

    /// Returns the attached surface without exposing the owner slots.
    pub fn surface(&self) -> Option<&S> {
        self.owners.surface()
    }

    pub fn surface_mut(&mut self) -> Option<&mut S> {
        self.owners.surface_mut()
    }

    /// Returns the attached graphics session without exposing the owner slots.
    pub fn graphics(&self) -> Option<&G> {
        self.owners.graphics()
    }

    pub fn graphics_for_shutdown_mut(&mut self) -> Result<Option<&mut G>, RuntimeError> {
        self.release_boundary(Subsystem::Graphics)?;
        Ok(self.owners.graphics_mut())
    }

    /// Stop ingress while retaining the allocation and subsystem lease.
    pub(crate) fn graphics_ingress_for_shutdown_mut(
        &mut self,
    ) -> Result<Option<&mut G>, RuntimeError> {
        self.lifecycle.assert_owner()?;
        if self.phase() != RuntimePhase::ShuttingDown {
            return Err(RuntimeError::InvalidTransition {
                from: self.phase(),
                to: RuntimePhase::ShuttingDown,
            });
        }
        Ok(self.owners.graphics_mut())
    }

    /// Run resource-specific destruction after the process owner has closed,
    /// while the engine image is still mapped.
    pub fn finalize_graphics(&mut self) -> Result<(), RuntimeError>
    where
        G: NativeResource,
    {
        if let Some(graphics) = self.owners.graphics_mut() {
            let status = graphics.finalize();
            if status != 0 {
                return Err(RuntimeError::EngineFailure { status });
            }
        }
        Ok(())
    }

    pub fn release_engine(&mut self) -> Result<Option<E>, RuntimeError> {
        self.release_boundary(Subsystem::Engine)?;
        Ok(self.owners.take_engine())
    }

    pub fn release_provider(&mut self) -> Result<Option<P>, RuntimeError> {
        self.release_boundary(Subsystem::ElfNamespace)?;
        Ok(self.owners.take_provider())
    }

    pub fn release_surface(&mut self) -> Result<Option<S>, RuntimeError> {
        self.release_boundary(Subsystem::Surface)?;
        Ok(self.owners.take_surface())
    }

    pub fn release_graphics(&mut self) -> Result<Option<G>, RuntimeError> {
        self.release_boundary(Subsystem::Graphics)?;
        Ok(self.owners.take_graphics())
    }

    pub fn attach_engine(&mut self, engine: E) -> Result<(), E> {
        self.owners.attach_engine(engine)
    }

    pub fn attach_provider(&mut self, provider: P) -> Result<(), P> {
        self.owners.attach_provider(provider)
    }

    pub fn attach_surface(&mut self, surface: S) -> Result<(), S> {
        self.owners.attach_surface(surface)
    }

    pub fn attach_graphics(&mut self, graphics: G) -> Result<(), G> {
        self.owners.attach_graphics(graphics)
    }

    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    /// Close and release every native resource in dependency order.
    ///
    /// Graphics ingress closes first, but every allocation, lease and provider
    /// remains live until ART shutdown succeeds. Native readiness defers are
    /// retried on this owner thread; failure does not authorize image unload.
    pub fn shutdown_native(&mut self) -> Result<(), RuntimeError>
    where
        E: NativeResource,
        P: NativeResource,
        S: NativeResource,
        G: NativeResource,
    {
        crate::shutdown::shutdown_native(self)
    }

    pub(crate) fn remove_expected_subsystem(
        &mut self,
        expected: Subsystem,
    ) -> Result<(), RuntimeError> {
        match self.uninstall_latest_subsystem()? {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(RuntimeError::InvalidShutdownOrder {
                expected,
                requested: actual,
            }),
            None => Err(RuntimeError::SubsystemNotActive {
                subsystem: expected,
            }),
        }
    }

    fn release_boundary(&self, subsystem: Subsystem) -> Result<(), RuntimeError> {
        self.lifecycle.assert_owner()?;
        if self.phase() != RuntimePhase::ShuttingDown {
            return Err(RuntimeError::InvalidTransition {
                from: self.phase(),
                to: RuntimePhase::ShuttingDown,
            });
        }
        if self.lifecycle.subsystem_active(subsystem) {
            return Err(RuntimeError::InvalidShutdownOrder {
                expected: subsystem,
                requested: subsystem,
            });
        }
        Ok(())
    }
}

impl<E, P, S, G> Default for RuntimeSession<E, P, S, G> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E, P, S, G> Drop for RuntimeSession<E, P, S, G> {
    fn drop(&mut self) {
        if self.engine().is_some()
            && matches!(
                self.phase(),
                RuntimePhase::ShuttingDown | RuntimePhase::Failed
            )
        {
            // A failed teardown leaves the VM's borrowed pointers and code
            // live. Abort before RuntimeOwners can drop any dependent resource.
            // Deferred (67) teardown normally stays inside the owner retry loop.
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeSession;
    use crate::{RuntimePhase, Subsystem};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug)]
    struct CloseProbe {
        name: &'static str,
        events: Rc<RefCell<Vec<&'static str>>>,
    }

    impl super::NativeResource for CloseProbe {
        fn close(&mut self) -> i32 {
            self.events.borrow_mut().push(self.name);
            0
        }

        fn clear(&mut self) -> i32 {
            self.events.borrow_mut().push("provider-clear");
            0
        }
    }

    #[test]
    fn session_keeps_resources_and_lifecycle_in_one_owner_value() {
        let mut session = RuntimeSession::<u8, u16, u32, u64>::new();
        session.start().unwrap();
        session.attach_engine(7).unwrap();
        session.attach_provider(8).unwrap();
        session.attach_surface(9).unwrap();
        session.attach_graphics(10).unwrap();
        let engine = session.install_subsystem(Subsystem::Engine).unwrap();
        let provider = session.install_subsystem(Subsystem::ElfNamespace).unwrap();
        let surface = session.install_subsystem(Subsystem::Surface).unwrap();
        let graphics = session.install_subsystem(Subsystem::Graphics).unwrap();
        session.mark_running().unwrap();
        session.begin_shutdown().unwrap();
        session.uninstall_subsystem(graphics).unwrap();
        assert_eq!(session.release_graphics().unwrap(), Some(10));
        session.uninstall_subsystem(surface).unwrap();
        assert_eq!(session.release_surface().unwrap(), Some(9));
        session.uninstall_subsystem(provider).unwrap();
        assert_eq!(session.release_provider().unwrap(), Some(8));
        session.uninstall_subsystem(engine).unwrap();
        assert_eq!(session.release_engine().unwrap(), Some(7));
        session.finish_shutdown().unwrap();
        assert_eq!(session.phase(), RuntimePhase::Stopped);
        assert!(session.is_empty());
    }

    #[test]
    fn resource_release_requires_shutdown_and_inactive_lease() {
        let mut session = RuntimeSession::<u8, u16, u32, u64>::new();
        session.start().unwrap();
        session.attach_engine(7).unwrap();
        let engine = session.install_subsystem(Subsystem::Engine).unwrap();
        assert!(session.release_engine().is_err());

        session.mark_running().unwrap();
        session.begin_shutdown().unwrap();
        assert!(session.release_engine().is_err());
        session.uninstall_subsystem(engine).unwrap();
        assert_eq!(session.release_engine().unwrap(), Some(7));
        session.finish_shutdown().unwrap();
    }

    #[test]
    fn native_shutdown_owns_reverse_close_and_provider_clear_order() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut session = RuntimeSession::<CloseProbe, CloseProbe, CloseProbe, CloseProbe>::new();
        session.start().unwrap();
        session
            .attach_engine(CloseProbe {
                name: "engine-close",
                events: Rc::clone(&events),
            })
            .unwrap();
        session
            .attach_provider(CloseProbe {
                name: "provider-close",
                events: Rc::clone(&events),
            })
            .unwrap();
        session
            .attach_graphics(CloseProbe {
                name: "graphics-close",
                events: Rc::clone(&events),
            })
            .unwrap();
        session
            .attach_surface(CloseProbe {
                name: "surface-close",
                events: Rc::clone(&events),
            })
            .unwrap();
        session.install_subsystem(Subsystem::Engine).unwrap();
        session.install_subsystem(Subsystem::ElfNamespace).unwrap();
        session.install_subsystem(Subsystem::Graphics).unwrap();
        session.install_subsystem(Subsystem::Surface).unwrap();
        session.mark_running().unwrap();

        session.shutdown_native().unwrap();

        assert_eq!(
            &*events.borrow(),
            &[
                "graphics-close",
                "engine-close",
                "surface-close",
                "provider-clear"
            ]
        );
        assert_eq!(session.phase(), RuntimePhase::Stopped);
        assert!(session.is_empty());
    }
}
