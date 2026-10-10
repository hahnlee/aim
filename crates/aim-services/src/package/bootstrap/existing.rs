use super::{Bridge, OwnerError, read_installer_user_policy_record};
use crate::package::installer::{existing::IntentSender, policy::UserPolicy};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
impl Bridge {
    pub(crate) fn existing_install_user_policy(
        &self,
        user: i32,
    ) -> Result<(bool, UserPolicy), OwnerError> {
        let mut data = Parcel::new();
        api::GetExistingPackageInstallUserPolicy { user_id: user }.write(&mut data);
        let reply = self
            .owner
            .transact(api::GET_EXISTING_PACKAGE_INSTALL_USER_POLICY, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = api::read_get_existing_package_install_user_policy_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        read_installer_user_policy_record(&bytes, user).map_err(OwnerError::Transport)
    }
    pub(crate) fn existing_package_installed(
        &self,
        record: aim_binder_host::parcel::Binder,
        user: i32,
        flags: i32,
    ) -> Result<[i64; 3], OwnerError> {
        let mut data = Parcel::new();
        api::OnExistingPackageInstalled {
            record: Some(record),
            user_id: user,
            install_flags: flags,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(api::ON_EXISTING_PACKAGE_INSTALLED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let values = api::read_on_existing_package_installed_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(BAD_VALUE))?;
        if reader.remaining() != 0 || values.len() != 3 || values[0] & !3 != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        values
            .try_into()
            .map_err(|_| OwnerError::Transport(BAD_VALUE))
    }
    pub(crate) fn restore_existing_install(
        &self,
        name: &str,
        user: i32,
        token: i32,
    ) -> Result<bool, OwnerError> {
        let mut data = Parcel::new();
        api::RestoreExistingPackageInstall {
            package_name: Some(name.into()),
            user_id: user,
            token,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(api::RESTORE_EXISTING_PACKAGE_INSTALL, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = api::read_restore_existing_package_install_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(value)
    }
    pub(crate) fn complete_existing_install(
        &self,
        name: Option<&str>,
        user: i32,
        receiver: Option<&IntentSender>,
        status: i32,
        restore: bool,
    ) -> Result<Option<String>, OwnerError> {
        let mut data = Parcel::new();
        api::CompleteExistingPackageInstall {
            package_name: name.map(str::to_owned),
            user_id: user,
            target: receiver.cloned(),
            status,
            restore_permissions: restore,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(api::COMPLETE_EXISTING_PACKAGE_INSTALL, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = api::read_complete_existing_package_install_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(value)
    }
}
