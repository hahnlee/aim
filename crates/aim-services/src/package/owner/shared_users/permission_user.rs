//! Original shared UID-owner projection for retained member settings.
impl super::SharedUser {
    pub(crate) fn capture_retained_permission_inventory(&mut self,
        bridge: &crate::package::bootstrap::Bridge, users: &[i32]) -> Result<(), String> {
        if self.retained.is_empty() { return Ok(()); }
        let state = bridge.legacy_permissions(self.app_id, users)
            .map_err(|error| format!("shared retained permission inventory owner: {error:?}"))?;
        for setting in self.retained.values_mut() {
            std::sync::Arc::make_mut(setting).legacy = Some(state.clone());
        }
        Ok(())
    }
}
