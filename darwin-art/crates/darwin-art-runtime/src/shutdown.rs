//! Rust-owned native shutdown transaction.
//!
//! The session API exposes lifecycle/owner operations, while this module
//! contains the one dependency-ordered transaction that closes them. Keeping
//! the transaction separate prevents new runtime states from silently
//! acquiring a second teardown path in the host or engine crates.

use crate::{NativeResource, RuntimeError, RuntimeSession, Subsystem};

/// Owns the final shutdown obligation for one concrete `RuntimeSession`.
///
/// The guard lives in the ownership crate rather than a host frontend, so a
/// new launcher cannot accidentally invent a second armed/rollback state
/// machine. Dropping an armed guard invokes the same dependency-ordered
/// transaction as an explicit shutdown.
pub struct ShutdownGuard<'a, E, P, S, G>
where
    E: NativeResource,
    P: NativeResource,
    S: NativeResource,
    G: NativeResource,
{
    session: Option<&'a mut RuntimeSession<E, P, S, G>>,
}

impl<'a, E, P, S, G> ShutdownGuard<'a, E, P, S, G>
where
    E: NativeResource,
    P: NativeResource,
    S: NativeResource,
    G: NativeResource,
{
    pub fn new(session: &'a mut RuntimeSession<E, P, S, G>) -> Self {
        Self {
            session: Some(session),
        }
    }

    pub fn session(&mut self) -> &mut RuntimeSession<E, P, S, G> {
        self.session
            .as_deref_mut()
            .expect("shutdown guard already consumed")
    }

    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        let session = self
            .session
            .take()
            .expect("shutdown guard already consumed");
        shutdown_native(session)
    }
}

impl<E, P, S, G> Drop for ShutdownGuard<'_, E, P, S, G>
where
    E: NativeResource,
    P: NativeResource,
    S: NativeResource,
    G: NativeResource,
{
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = shutdown_native(session);
        }
    }
}

pub(crate) fn shutdown_native<E, P, S, G>(
    session: &mut RuntimeSession<E, P, S, G>,
) -> Result<(), RuntimeError>
where
    E: NativeResource,
    P: NativeResource,
    S: NativeResource,
    G: NativeResource,
{
    session.begin_shutdown()?;
    // Stop ingress only. Native teardown still borrows every allocation.
    if let Some(graphics) = session.graphics_ingress_for_shutdown_mut()? {
        let status = graphics.close();
        if status != 0 {
            return Err(RuntimeError::EngineFailure { status });
        }
    }
    let mut waits = 0_u64;
    let mut native_providers_released = false;
    if let Some(engine) = session.engine_mut() {
        loop {
            let status = engine.close();
            if status == darwin_art_engine_sys::PROCESS_SHUTDOWN_NOT_READY {
                if waits % 1000 == 0 {
                    eprintln!("ART shutdown pending: waiting for native teardown readiness");
                }
                waits = waits.saturating_add(1);
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            if status != 0 {
                // Failure is not permission to unload a live VM's providers.
                return Err(RuntimeError::EngineFailure { status });
            }
            break;
        }
        native_providers_released = engine.providers_released_by_native_shutdown();
    }
    let mut first_status = None;
    let mut remember = |status: i32| {
        if status != 0 && first_status.is_none() {
            first_status = Some(status);
        }
    };

    // Native shutdown has succeeded. Retire leases in installation-reverse
    // order; only now may the borrowed surface allocation be destroyed.
    if session.surface().is_some() {
        if let Err(error) = session.remove_expected_subsystem(Subsystem::Surface) {
            remember(error.status() as i32);
        }
    }

    if session.graphics().is_some() {
        if let Err(error) = session.remove_expected_subsystem(Subsystem::Graphics) {
            remember(error.status() as i32);
        }
    }

    // Surface destruction follows the ART graphics callback so every native
    // borrower observes a live surface for the complete shutdown boundary.
    if let Some(surface) = session.surface_mut() {
        remember(surface.close());
    }
    if let Err(error) = session.release_surface() {
        remember(error.status() as i32);
    }

    if session.provider().is_some()
        && let Err(error) = session.remove_expected_subsystem(Subsystem::ElfNamespace)
    {
        remember(error.status() as i32);
    }

    if session.engine().is_some()
        && let Err(error) = session.remove_expected_subsystem(Subsystem::Engine)
    {
        remember(error.status() as i32);
    }

    // The ART shutdown callback finalizes the bound graphics session while
    // the VM is still attached. Only after that callback returns may the
    // Rust owner invoke its destroy function. Keep the engine image mapped
    // until this second phase has completed.
    if let Err(error) = session.finalize_graphics() {
        remember(error.status() as i32);
    }
    if let Err(error) = session.release_graphics() {
        remember(error.status() as i32);
    }

    // ProviderBridge's clear callback belongs to the engine dylib. Clear all
    // provider hooks while that image is still owned; releasing the engine
    // first would turn this callback into a use-after-unload.
    if let Some(provider) = session.provider_mut() {
        if native_providers_released {
            remember(provider.adopt_native_shutdown());
        }
        remember(provider.clear());
    }
    if let Err(error) = session.release_provider() {
        remember(error.status() as i32);
    }
    if let Err(error) = session.release_engine() {
        remember(error.status() as i32);
    }
    if first_status.is_none()
        && let Err(error) = session.finish_shutdown()
    {
        first_status = Some(error.status() as i32);
    }
    if let Some(status) = first_status {
        session.fail(RuntimeError::EngineFailure { status });
        Err(RuntimeError::EngineFailure { status })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ShutdownGuard;
    use crate::{NativeResource, RuntimePhase, RuntimeSession, Subsystem};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Debug)]
    struct Probe(Arc<AtomicUsize>);

    impl NativeResource for Probe {
        fn close(&mut self) -> i32 {
            self.0.fetch_add(1, Ordering::SeqCst);
            0
        }

        fn clear(&mut self) -> i32 {
            self.0.fetch_add(1, Ordering::SeqCst);
            0
        }
    }

    #[derive(Debug)]
    struct OrderedProbe {
        name: &'static str,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl NativeResource for OrderedProbe {
        fn close(&mut self) -> i32 {
            self.events.lock().unwrap().push(self.name);
            0
        }

        fn finalize(&mut self) -> i32 {
            if self.name == "graphics" {
                self.events.lock().unwrap().push("finalize-graphics");
            }
            0
        }

        fn clear(&mut self) -> i32 {
            self.events.lock().unwrap().push("provider-clear");
            0
        }
    }

    impl Drop for OrderedProbe {
        fn drop(&mut self) {
            self.events.lock().unwrap().push(match self.name {
                "graphics" => "drop-graphics",
                "surface" => "drop-surface",
                "engine" => "drop-engine",
                "provider" => "drop-provider",
                other => other,
            });
        }
    }

    #[test]
    fn pre_entry_close_releases_real_provider_bridge_lease() {
        use crate::{ProviderBridge, ProviderKind};
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static RELEASES: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn acquire(_: u32, _: i32) -> i32 {
            LIVE.fetch_add(1, Ordering::SeqCst);
            0
        }
        unsafe extern "C" fn release(_: u32) -> i32 {
            assert_eq!(LIVE.fetch_sub(1, Ordering::SeqCst), 1);
            RELEASES.fetch_add(1, Ordering::SeqCst);
            0
        }
        unsafe extern "C" fn clear() {
            assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        }
        struct PreEntryEngine;
        impl NativeResource for PreEntryEngine {
            fn close(&mut self) -> i32 {
                0
            }
            // No native process teardown: default false preserves leases.
        }
        let mut session = RuntimeSession::<PreEntryEngine, ProviderBridge, Probe, Probe>::new();
        session.start().unwrap();
        assert!(session.attach_engine(PreEntryEngine).is_ok());
        let provider = ProviderBridge::from_callbacks(acquire, release, clear);
        provider
            .acquire_process_lease(ProviderKind::Network, -1)
            .unwrap();
        assert!(session.attach_provider(provider).is_ok());
        session.install_subsystem(Subsystem::Engine).unwrap();
        session.install_subsystem(Subsystem::ElfNamespace).unwrap();
        session.shutdown_native().unwrap();
        assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
        assert!(session.is_empty());
    }

    #[test]
    fn dropped_guard_runs_the_same_reverse_shutdown_transaction() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut session = RuntimeSession::<Probe, Probe, Probe, Probe>::new();
        session.start().unwrap();
        session.attach_engine(Probe(Arc::clone(&calls))).unwrap();
        session.attach_provider(Probe(Arc::clone(&calls))).unwrap();
        session.attach_surface(Probe(Arc::clone(&calls))).unwrap();
        session.attach_graphics(Probe(Arc::clone(&calls))).unwrap();
        session.install_subsystem(Subsystem::Engine).unwrap();
        session.install_subsystem(Subsystem::ElfNamespace).unwrap();
        session.install_subsystem(Subsystem::Graphics).unwrap();
        session.install_subsystem(Subsystem::Surface).unwrap();
        session.mark_running().unwrap();

        {
            let _guard = ShutdownGuard::new(&mut session);
        }

        assert_eq!(session.phase(), RuntimePhase::Stopped);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn graphics_owner_drops_before_engine_image_owner() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut session =
            RuntimeSession::<OrderedProbe, OrderedProbe, OrderedProbe, OrderedProbe>::new();
        session.start().unwrap();
        session
            .attach_engine(OrderedProbe {
                name: "engine",
                events: Arc::clone(&events),
            })
            .unwrap();
        session
            .attach_provider(OrderedProbe {
                name: "provider",
                events: Arc::clone(&events),
            })
            .unwrap();
        session
            .attach_surface(OrderedProbe {
                name: "surface",
                events: Arc::clone(&events),
            })
            .unwrap();
        session
            .attach_graphics(OrderedProbe {
                name: "graphics",
                events: Arc::clone(&events),
            })
            .unwrap();
        session.install_subsystem(Subsystem::Engine).unwrap();
        session.install_subsystem(Subsystem::ElfNamespace).unwrap();
        session.install_subsystem(Subsystem::Graphics).unwrap();
        session.install_subsystem(Subsystem::Surface).unwrap();
        session.mark_running().unwrap();

        session.shutdown_native().unwrap();
        assert_eq!(
            &*events.lock().unwrap(),
            &[
                "graphics",
                "engine",
                "surface",
                "drop-surface",
                "finalize-graphics",
                "drop-graphics",
                "provider-clear",
                "drop-provider",
                "drop-engine",
            ]
        );
    }
}
