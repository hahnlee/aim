//! SystemServer's PMS readiness precedes AMS process startup (Android 16 r1).
use super::AppHost;
use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::sync::{Arc, Condvar, Mutex};

/// Early boot failure also releases pending starts, with the boot failure.
pub(super) struct BootReady<'a>(pub &'a AppHost);
impl Drop for BootReady<'_> {
    fn drop(&mut self) {
        let _ = self.0.processes_ready.set(Err(Exception::new(
            EX_ILLEGAL_STATE,
            "boot stopped before system ready",
        )));
    }
}
impl AppHost {
    pub(super) fn system_ready(&self) -> Result<(), Exception> {
        self.complete_system_ready(|| self.package_system()?.run_native_package_lifecycle(1))
    }

    pub(super) fn complete_system_ready(
        &self,
        packages: impl FnOnce() -> Result<(), Exception>,
    ) -> Result<(), Exception> {
        self.processes_ready
            .get_or_init(|| {
                // SystemServer calls PMS.systemReady before AMS.systemReady:
                // package prepare-ready (including SettingsProvider access)
                // and default grants must both precede application forks.
                packages()?;
                self.permission_manager
                    .get()
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "permission owner unavailable")
                    })?
                    .system_ready()
            })
            .clone()?;
        self.start_pending_services()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accessibility_startup_failure_releases_waiters() {
        let ready = Arc::new(AccessibilityReady::default());
        let pending = ready.clone();
        let waiter = std::thread::spawn(move || pending.wait().err().unwrap());
        let failure = "construction failed (#2529)";
        ready.failed(failure);
        assert_eq!(waiter.join().unwrap().message, failure);
    }
    #[test]
    fn boot_failure_releases_pending_starts_with_an_error() {
        let host = AppHost::new(crate::test_process(1000));
        assert!(host.processes_ready.get().is_none());
        drop(BootReady(&host));
        assert!(host.processes_ready.wait().is_err());
        host.process.shutdown().unwrap();
    }
    #[test]
    fn readiness_requires_a_permission_owner() {
        let host = AppHost::new(crate::test_process(1000));
        assert!(host.complete_system_ready(|| Ok(())).is_err());
        assert!(host.processes_ready.wait().is_err());
        host.process.shutdown().unwrap();
    }

    #[test]
    fn readiness_requires_the_package_lifecycle_before_default_grants() {
        crate::system::with_boot_discovery_binders(|host, _, _, _| {
            let error = host.system_ready().unwrap_err();
            assert_eq!(error.message, "Boot lifecycle runtime unavailable");
            assert_eq!(host.processes_ready.wait(), &Err(error));
        });
    }
}

/// Publication barrier for the original service, including startup failure and later death.
#[derive(Default)]
pub(crate) struct AccessibilityReady {
    result: Mutex<Option<Result<Arc<Strong>, Exception>>>,
    changed: Condvar,
}
impl AccessibilityReady {
    pub(super) fn failed(&self, message: &str) {
        *self.result.lock().unwrap() = Some(Err(Exception::new(EX_ILLEGAL_STATE, message)));
        self.changed.notify_all();
    }
    pub(crate) fn publish(&self, service: Arc<Strong>, observe: impl FnOnce(&Strong)) {
        let mut result = self.result.lock().unwrap();
        observe(&service);
        *result = Some(Ok(service));
        self.changed.notify_all();
    }
    pub(super) fn wait(&self) -> Result<Arc<Strong>, Exception> {
        let result = self.result.lock().unwrap();
        let (mut result, _) = self
            .changed
            .wait_timeout_while(result, std::time::Duration::from_secs(60), |r| r.is_none())
            .unwrap();
        if result.is_none() {
            let failure = Exception::new(EX_ILLEGAL_STATE, "accessibility start timed out (#2529)");
            *result = Some(Err(failure));
            self.changed.notify_all();
        }
        result.as_ref().unwrap().clone()
    }
}
