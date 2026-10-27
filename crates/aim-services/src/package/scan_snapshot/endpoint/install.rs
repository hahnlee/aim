//! Deterministic lease release for native installation's synchronous callback.
impl super::Endpoint {
    pub(crate) fn revoke_install_scope(&self) {
        {
            let mut lease = self.lease.lock().unwrap();
            lease.snapshot = None;
            lease.records.clear();
        }
        self.computer.lock().unwrap().take();
        let retired = self.install_retirement.lock().unwrap().take();
        if let Some(retired) = retired {
            retired();
        }
    }
}
impl super::Endpoint {
    pub(crate) fn on_install_retired(&self, retired: Box<dyn FnOnce() + Send>) {
        *self.install_retirement.lock().unwrap() = Some(retired);
    }
    pub(crate) fn install_capture(
        &self,
    ) -> Result<std::sync::Arc<super::super::query_state::Capture>, String> {
        self.computer
            .lock()
            .unwrap()
            .as_ref()
            .map(|owner| owner.capture.clone())
            .ok_or_else(|| "permission scope is retired".into())
    }
    pub(crate) fn pin_install_access(
        &self,
        access: std::sync::Arc<crate::package::permissions::AccessView>,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<(), String> {
        let mut computer = self.computer.lock().unwrap();
        let owner = computer.as_mut().ok_or("permission scope is retired")?;
        let mut context = (**owner.capture.context()).clone();
        access.project_context(&mut context, owner.capture.state(), config);
        context.access = Some(access);
        owner.capture =
            super::super::query_state::Capture::new(owner.capture.scan().clone(), context)?;
        Ok(())
    }
}
