use super::abi::{BinderBrokerSymbols, EngineSymbols, LoadedEngine, native_loader_config_allowed};
use super::graphics::GraphicsSession;
use super::process::ProcessRequest;
use super::process_filesystem::{ProcessFilesystemError, install_with};
use super::process_snapshot::{ProcessSnapshotError, ProcessSnapshotInputs};
use super::surface::SurfaceSession;
use core::ffi::c_void;
use darwin_art_engine_sys::{ProcessConfig, ProcessResult, ProviderAcquireFn, ProviderReleaseFn};
use darwin_art_runtime::{NativeResource, ProviderBridge};
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Process-scoped engine owner.  The dynamic library and its shutdown
/// callback share one Rust lifetime, so callers cannot accidentally drop
/// the symbol image before ART has been shut down.
pub struct EngineSession {
    engine: LoadedEngine,
    shutdown_taken: bool,
    shutdown_status: i32,
    process_entered: AtomicBool,
    process_snapshot_installed: bool,
    process_filesystem_installed: bool,
}

impl EngineSession {
    pub fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            engine: LoadedEngine::open(path)?,
            shutdown_taken: false,
            shutdown_status: 0,
            process_entered: AtomicBool::new(false),
            process_snapshot_installed: false,
            process_filesystem_installed: false,
        })
    }

    pub(crate) fn symbols(&self) -> EngineSymbols {
        self.engine.symbols()
    }

    /// Build the Rust-owned provider bridge directly from this image's ABI.
    /// The host does not copy or interpret the callback table.
    pub fn provider_bridge(&self) -> ProviderBridge {
        let symbols = self.engine.symbols();
        ProviderBridge::from_callbacks(
            symbols.provider.native_acquire,
            symbols.provider.native_release,
            symbols.provider.clear_hooks,
        )
    }

    /// Borrow the central Bionic Binder-FD publication ABI from this exact
    /// RuntimeEntry image. The returned function pointers remain valid while
    /// this `EngineSession` owns the dynamic image.
    pub fn binder_broker_symbols(&self) -> BinderBrokerSymbols {
        self.engine.symbols().binder_broker
    }

    /// Install the host/service-owned Android process snapshot through the
    /// engine's configured ABI. The native side copies every input before the
    /// call returns; this owner records the active lifetime only afterward.
    pub fn install_process_snapshot(
        &mut self,
        inputs: &ProcessSnapshotInputs,
    ) -> Result<(), ProcessSnapshotError> {
        if self.shutdown_taken {
            return Err(ProcessSnapshotError::EngineClosed);
        }
        if self.process_snapshot_installed {
            return Err(ProcessSnapshotError::AlreadyInstalled);
        }
        inputs.install_with(self.engine.symbols().process.install_process_snapshot)?;
        self.process_snapshot_installed = true;
        Ok(())
    }

    /// Atomically install a snapshot and explicit trusted Android credentials.
    /// A native image that rejects version 2 is an error, never a v1 fallback.
    pub fn install_process_snapshot_with_credentials(
        &mut self,
        inputs: &ProcessSnapshotInputs,
        credentials: &super::process_credentials::ProcessCredentialsInputs,
    ) -> Result<(), ProcessSnapshotError> {
        if self.shutdown_taken {
            return Err(ProcessSnapshotError::EngineClosed);
        }
        if self.process_snapshot_installed {
            return Err(ProcessSnapshotError::AlreadyInstalled);
        }
        inputs.install_with_credentials(
            credentials,
            self.engine.symbols().process.install_process_snapshot,
        )?;
        self.process_snapshot_installed = true;
        Ok(())
    }

    /// Install the process filesystem from a caller-owned authority
    /// descriptor. Native code duplicates the descriptor synchronously and
    /// copies both guest paths before this method returns.
    pub fn install_process_filesystem(
        &mut self,
        root_fd: BorrowedFd<'_>,
        guest_mount: &[u8],
        cwd: &[u8],
    ) -> Result<(), ProcessFilesystemError> {
        if self.shutdown_taken {
            return Err(ProcessFilesystemError::EngineClosed);
        }
        if self.process_filesystem_installed {
            return Err(ProcessFilesystemError::AlreadyInstalled);
        }
        install_with(
            root_fd,
            guest_mount,
            cwd,
            self.engine.symbols().process.install_process_filesystem,
        )?;
        self.process_filesystem_installed = true;
        Ok(())
    }

    /// Run one process through the versioned ABI and construct its result
    /// in the same crate that owns the raw function pointer. The caller
    /// receives no partially initialized result on a nonzero status.
    pub(crate) fn run_process(&self, config: &ProcessConfig) -> Result<ProcessResult, i32> {
        if self.shutdown_taken || !config.is_compatible() || !native_loader_config_allowed(config) {
            return Err(-1);
        }
        let mut result = ProcessResult::new();
        self.process_entered.store(true, Ordering::Release);
        // SAFETY: `config` and all callback state it references are owned
        // by the caller for this synchronous invocation; the function
        // pointer belongs to this live EngineSession image.
        let status = unsafe { (self.engine.symbols().process.run_process)(config, &mut result) };
        if status == 0 { Ok(result) } else { Err(status) }
    }

    /// Run an owned request without exposing the raw ABI struct to host
    /// orchestration. The request keeps all referenced strings and callback
    /// state alive until this synchronous call returns.
    pub fn run_request(&self, request: &ProcessRequest<'_>) -> Result<ProcessResult, i32> {
        let config = request.as_config();
        self.run_process(&config)
    }

    pub fn active_surface(&self) -> Option<SurfaceSession> {
        SurfaceSession::active(self.symbols())
    }

    pub fn create_graphics_session(&self) -> Result<GraphicsSession, i32> {
        GraphicsSession::create(self.symbols())
    }

    /// Reports whether the graphics flavor has published a drawable
    /// surface without taking ownership of it. The non-graphics probe
    /// intentionally has no active surface and uses the diagnostic path;
    /// production graphics always publishes one before the host enters
    /// its frame loop.
    pub fn has_active_surface(&self) -> bool {
        // SAFETY: this is a read-only query on the live engine image.
        unsafe { !(self.engine.symbols().surface.active)().is_null() }
    }

    /// Service AppKit while the ART owner is running on another thread.
    /// Callers use this from the host's main actor, never from the ART worker.
    pub fn pump_appkit_events(&self, seconds: f64) -> i32 {
        // SAFETY: the callback belongs to this live engine image and accepts
        // only a bounded duration value.
        unsafe { (self.engine.symbols().surface.appkit_pump_events)(seconds) }
    }

    /// Returns the main-actor callback without borrowing the engine owner.
    /// The image stays alive through `RuntimeSession`, so the host may call
    /// this function pointer while the owner thread is running.
    pub fn appkit_pump_callback(&self) -> darwin_art_engine_sys::AppKitPumpEventsFn {
        self.engine.symbols().surface.appkit_pump_events
    }

    pub fn create_surface(
        &self,
        info: &darwin_art_engine_sys::SurfaceCreateInfo,
    ) -> Result<SurfaceSession, i32> {
        SurfaceSession::create(self.symbols(), info)
    }

    /// # Safety
    ///
    /// `context` and both callbacks must remain valid until
    /// `clear_provider_hooks` is called. The callbacks may run on an ART
    /// thread during native-library graph loading.
    pub unsafe fn install_provider_hooks(
        &self,
        context: *mut c_void,
        acquire: Option<ProviderAcquireFn>,
        release: Option<ProviderReleaseFn>,
    ) {
        // SAFETY: the callback context is owned by the caller for the
        // entire engine session, and the function pointer table belongs
        // to this live dynamic image.
        unsafe { (self.engine.symbols().provider.install_hooks)(context, acquire, release) }
    }

    pub fn clear_provider_hooks(&self) {
        // SAFETY: the hook table is process-global and this owner is the
        // same image that installed it.
        unsafe { (self.engine.symbols().provider.clear_hooks)() }
    }

    /// Close the process-scoped engine at most once. The callback is kept
    /// behind this owner so its code image remains mapped for the entire
    /// call and until the owner is dropped afterward.
    pub fn close(&mut self) -> i32 {
        if self.shutdown_taken {
            return self.shutdown_status;
        }
        self.shutdown_taken = true;
        // Installing configuration/opening the image does not create a VM.
        // Failed pre-run bootstrap transfers must release their snapshot
        // without invoking the native shutdown state machine in kNotReady.
        if !self.process_entered.load(Ordering::Acquire) {
            return 0;
        }
        // SAFETY: the function pointer was resolved from this live,
        // version-checked engine image and takes no arguments.
        self.shutdown_status = unsafe { (self.engine.symbols().process.shutdown_process)() };
        self.shutdown_status
    }

    /// Unload guest NativeLoader DSOs before an Android process-style `_exit`.
    /// This preserves JNI_OnUnload without entering a competing VM teardown.
    pub fn prepare_process_exit(&self) -> i32 {
        // SAFETY: resolved from the live, version-checked engine image.
        unsafe { (self.engine.symbols().process.prepare_process_exit)() }
    }

    /// Backwards-compatible name for the explicit process close contract.
    pub fn shutdown_once(&mut self) -> i32 {
        self.close()
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        // A failed ownership transfer must not leave ART resident. Normal
        // RuntimeSession teardown marks this callback consumed first, so
        // Drop is idempotent in the successful path.
        let shutdown_status = self.close();
        if self.process_filesystem_installed {
            // A failed VM shutdown does not prove filesystem users have
            // quiesced. Never release the authority while native threads may
            // still use it.
            if shutdown_status != 0 {
                std::process::abort();
            }
            self.process_filesystem_installed = false;
            // SAFETY: close completed and the callback belongs to the live
            // engine image. Filesystem teardown precedes snapshot teardown.
            let status = unsafe { (self.engine.symbols().process.uninstall_process_filesystem)() };
            if status != 0 {
                std::process::abort();
            }
        }
        if self.process_snapshot_installed {
            // A failed VM shutdown does not prove its property users quiesced.
            // Retain the failure across repeated close calls; never unload
            // their backing snapshot/image under surviving native threads.
            if shutdown_status != 0 {
                std::process::abort();
            }
            // Mark the local owner consumed before calling out so a future
            // panic/unwind path cannot attempt a second native uninstall.
            self.process_snapshot_installed = false;
            // SAFETY: close has completed and the process-state callback is
            // still resolved from the live engine image. Do not dlclose an
            // image whose process snapshot could still reference its storage.
            let status = unsafe { (self.engine.symbols().process.uninstall_process_snapshot)() };
            if status != 0 {
                std::process::abort();
            }
        }
    }
}

impl NativeResource for EngineSession {
    fn close(&mut self) -> i32 {
        EngineSession::close(self)
    }
}

#[cfg(test)]
mod close_contract_tests {
    use super::{EngineSession, GraphicsSession, SurfaceSession};

    // Keep all three owner types on the same explicit close-shaped API.
    // This is intentionally a function-pointer check: changing a close
    // contract's receiver or status type fails at compile time here.
    fn assert_close_contracts(
        _engine: fn(&mut EngineSession) -> i32,
        _surface: fn(&mut SurfaceSession) -> i32,
        _graphics: fn(&mut GraphicsSession) -> i32,
    ) {
    }

    #[test]
    fn all_native_owners_expose_explicit_close_contracts() {
        assert_close_contracts(
            EngineSession::close,
            SurfaceSession::close,
            GraphicsSession::close,
        );
    }
}
