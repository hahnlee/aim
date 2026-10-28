//! SystemServer's PMS readiness precedes AMS process startup (Android 16 r1).
use super::AppHost;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

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
        let result = self
            .permission_manager
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permission owner unavailable"))
            .and_then(|owner| owner.system_ready());
        let _ = self.processes_ready.set(result.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(host.system_ready().is_err());
        assert!(host.processes_ready.wait().is_err());
        host.process.shutdown().unwrap();
    }
}
