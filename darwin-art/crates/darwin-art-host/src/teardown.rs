//! Single owner-thread shutdown path for graphics and headless runs.

use crate::ExecutionLifetime;
use crate::config::HostError;
use crate::process_exit::exit_android_process;
use crate::runtime::HostRuntime;
use darwin_art_engine::{EngineSession, GraphicsSession, SurfaceSession};
use darwin_art_runtime::{ProviderBridge, RuntimeError, ShutdownGuard as RuntimeOwnerGuard};

/// Owns the final shutdown obligation after the runtime has entered its
/// running phase.  Moving this obligation into a guard makes every later
/// error path (surface attach, graphics install, or frame loop) use the same
/// reverse-order teardown, including newly added paths.
pub(super) struct RuntimeShutdownGuard<'a> {
    inner: Option<
        RuntimeOwnerGuard<'a, EngineSession, Box<ProviderBridge>, SurfaceSession, GraphicsSession>,
    >,
    execution_lifetime: ExecutionLifetime,
}

impl<'a> RuntimeShutdownGuard<'a> {
    pub(super) fn new(runtime: &'a mut HostRuntime, execution_lifetime: ExecutionLifetime) -> Self {
        Self {
            inner: Some(RuntimeOwnerGuard::new(runtime)),
            execution_lifetime,
        }
    }

    pub(super) fn runtime(&mut self) -> &mut HostRuntime {
        self.inner
            .as_mut()
            .expect("shutdown guard already consumed")
            .session()
    }

    pub(super) fn shutdown(mut self) -> Result<(), HostError> {
        if self.execution_lifetime == ExecutionLifetime::AndroidProcess {
            // APK processes must never enter DestroyJavaVM/ELF teardown,
            // including explicit error cleanup paths. The caller has already
            // recorded the failure; preserve it at the OS process boundary.
            // Do not let the temporary guard returned by `take()` run its
            // destructor before `_exit`; APK process boundaries must bypass
            // DestroyJavaVM/ELF teardown entirely.
            std::mem::forget(self.inner.take());
            exit_android_process(1);
        }
        self.inner
            .take()
            .expect("shutdown guard already consumed")
            .shutdown()
            .map_err(map_shutdown_error)
    }
}

impl Drop for RuntimeShutdownGuard<'_> {
    fn drop(&mut self) {
        if self.execution_lifetime == ExecutionLifetime::AndroidProcess && self.inner.is_some() {
            // An Android APK process is an OS lifetime boundary.  If an
            // in-process error reaches this guard, unloading live Chromium
            // DSOs/DestroyJavaVM is unsafe and unlike AOSP.  Let the kernel
            // reclaim the process instead of running the host teardown path.
            exit_android_process(1);
        }
    }
}

fn map_shutdown_error(error: RuntimeError) -> HostError {
    match error {
        RuntimeError::EngineFailure { status } => HostError::ShutdownFailed(status),
        other => HostError::RuntimeFailed(other.status() as i32),
    }
}
