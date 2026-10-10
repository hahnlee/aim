//! Deterministic lease release for native installation's synchronous callback.
impl super::Endpoint {
    pub(crate) fn revoke_install_scope(&self) {
        let mut lease = self.lease.lock().unwrap();
        lease.snapshot = None;
        lease.code.clear();
        lease.users.clear();
        lease.settings.clear();
        lease.runtimes.clear();
        lease.libraries.clear();
        lease.shared_users.clear();
        self.computer.lock().unwrap().take();
    }
}
