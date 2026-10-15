//! Original UID-owner projection for retained settings after a UM transition.
impl super::AppIds {
    pub(crate) fn capture_retained_permission_inventory(&mut self,
        bridge: &crate::package::bootstrap::Bridge, users: &[i32]) -> Result<(), String> {
        let mut captured = self.detached.clone();
        for (app_id, setting) in &mut captured {
            let state = bridge.legacy_permissions(*app_id, users)
                .map_err(|error| format!("detached permission inventory owner: {error:?}"))?;
            std::sync::Arc::make_mut(setting).legacy = Some(state);
        }
        self.detached = captured;
        Ok(())
    }
}
