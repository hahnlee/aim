//! Deterministic lease release for native installation's synchronous callback.
impl super::Endpoint {
    pub(crate) fn revoke_install_scope(&self) {
        {
            let mut lease = self.lease.lock().unwrap();
            lease.snapshot = None;
            lease.records.clear();
        }
        self.computer.lock().unwrap().take();
    }
}
