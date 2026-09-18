//! Installation lifetime at the native SCM/code-image boundary, not guest policy.
use super::abi::ProviderSymbols;
use darwin_art_engine_sys::ScmEndpointProviderV1;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(crate) struct ScmEndpointInstallation {
    installed: AtomicBool,
}

impl ScmEndpointInstallation {
    /// Caller owns valid callback code/context until native retain completes.
    pub(crate) unsafe fn install(
        &self,
        symbols: ProviderSymbols,
        table: &ScmEndpointProviderV1,
    ) -> Result<(), String> {
        self.installed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "SCM provider already installed in this engine".to_owned())?;
        let status = unsafe { (symbols.install_scm_endpoint)(table) };
        if status != 0 {
            self.installed.store(false, Ordering::Release);
            return Err(format!(
                "install native SCM provider: {}",
                std::io::Error::from_raw_os_error(status)
            ));
        }
        Ok(())
    }

    /// Called on the owner thread AFTER provider teardown and actual broker
    /// deactivation. Never infer code-image quiescence from refcount alone.
    pub(crate) fn uninstall_before_unload(&self, symbols: ProviderSymbols) -> Result<(), String> {
        if !self.installed.load(Ordering::Acquire) {
            return Ok(());
        }
        if unsafe { (symbols.socket_broker_is_active)() } != 0 {
            return Err("cannot unload SCM provider while native socket broker is active".into());
        }
        let status = unsafe { (symbols.uninstall_scm_endpoint)() };
        if status != 0 {
            return Err(format!(
                "uninstall native SCM provider: {}",
                std::io::Error::from_raw_os_error(status)
            ));
        }
        self.installed.store(false, Ordering::Release);
        Ok(())
    }
}
