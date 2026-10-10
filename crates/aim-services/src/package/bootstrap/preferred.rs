//! Typed independent preferred policy/effect leaves of the current bridge.
use super::{Bridge, OwnerError, bridge};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader};
impl Bridge {
    pub fn preferred_cross_access_control(
        &self,
        source: i32,
        target: i32,
    ) -> Result<i32, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPreferredCrossProfileAccessControl {
            source_user_id: source,
            target_user_id: target,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(
                bridge::GET_PREFERRED_CROSS_PROFILE_ACCESS_CONTROL,
                &data,
                false,
            )
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = bridge::read_get_preferred_cross_profile_access_control_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(value)
    }
    pub fn preferred_role_holder(
        &self,
        role: &str,
        user: i32,
    ) -> Result<Option<String>, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPreferredRoleHolder {
            role: Some(role.into()),
            user_id: user,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_PREFERRED_ROLE_HOLDER, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = bridge::read_get_preferred_role_holder_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(value)
    }
    pub fn preferred_set_role(
        &self,
        role: &str,
        name: &str,
        user: i32,
        broadcast: bool,
    ) -> Result<(), OwnerError> {
        let mut data = Parcel::new();
        bridge::SetPreferredRoleHolder {
            role: Some(role.into()),
            package_name: Some(name.into()),
            user_id: user,
            broadcast_on_success: broadcast,
        }
        .write(&mut data);
        self.preferred_void(
            bridge::SET_PREFERRED_ROLE_HOLDER,
            &data,
            bridge::read_set_preferred_role_holder_reply,
        )
    }
    pub fn preferred_changed(&self, user: i32) -> Result<(), OwnerError> {
        let mut data = Parcel::new();
        bridge::SendPreferredActivityChanged { user_id: user }.write(&mut data);
        self.preferred_void(
            bridge::SEND_PREFERRED_ACTIVITY_CHANGED,
            &data,
            bridge::read_send_preferred_activity_changed_reply,
        )
    }
    pub fn preferred_reset_permissions(&self, user: i32) -> Result<(), OwnerError> {
        let mut data = Parcel::new();
        bridge::ResetPreferredRuntimePermissions { user_id: user }.write(&mut data);
        self.preferred_void(
            bridge::RESET_PREFERRED_RUNTIME_PERMISSIONS,
            &data,
            bridge::read_reset_preferred_runtime_permissions_reply,
        )
    }
    pub fn preferred_reset_network(&self, user: i32) -> Result<(), OwnerError> {
        let mut data = Parcel::new();
        bridge::ResetPreferredNetworkPolicies { user_id: user }.write(&mut data);
        self.preferred_void(
            bridge::RESET_PREFERRED_NETWORK_POLICIES,
            &data,
            bridge::read_reset_preferred_network_policies_reply,
        )
    }
    fn preferred_void(
        &self,
        code: u32,
        data: &Parcel,
        decode: impl FnOnce(
            &mut Reader<'_>,
        ) -> aim_binder_host::parcel::Result<aim_service_aidl::Returned<()>>,
    ) -> Result<(), OwnerError> {
        let reply = self
            .owner
            .transact(code, data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        decode(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(())
    }
}
