//! Read a user's actual Settings permission file before original TEMP migration.
use crate::package::{permissions::RuntimePermissions, scan::SigningScan, system_config::SystemConfig};
impl super::super::Store {
    pub(crate) fn read_permission_user(&self, scan: &mut SigningScan, config: &SystemConfig,
        user: u32) -> Result<super::State, super::super::WriteError> {
        let mut state = self.state.clone();
        let entry = state.users.iter_mut().find(|(id, _)| *id == user)
            .ok_or_else(|| super::super::WriteError::before("permission read user is unregistered"))?;
        let path = self.data.join("misc_de").join(user.to_string())
            .join(crate::package::PERMISSION_DIR).join("runtime-permissions.xml");
        entry.1.runtime_permissions = crate::package::atomic(&path, RuntimePermissions::parse)
            .map_err(super::super::WriteError::before)?;
        scan.restore_permission_user_from_data(&self.data, &state, config, user as i32)
            .map_err(super::super::WriteError::before)
    }
}
