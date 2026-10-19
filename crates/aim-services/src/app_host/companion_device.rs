//! `companiondevice`: CompanionDeviceManagerService for the app host
//! (`android-16.0.0_r1`), published as the original publishes it with
//! `FEATURE_COMPANION_DEVICE_SETUP`, for a user with no associations
//! persisted (`AssociationDiskStore`: `system_de/<user>/companion_device_manager.xml`,
//! else the legacy `system/users/<user>/companion_device_manager_associations.xml`):
//! `canPairWithoutPrompt`, which the Bluetooth app asks when pairing,
//! and `getAllAssociationsForUser`. Persisted associations and every
//! other call are not implemented (#1790).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_companion_icompaniondevicemanager as cdm;

use super::{AppHost, reply};

const MANAGE_COMPANION_DEVICES: &str = "android.permission.MANAGE_COMPANION_DEVICES";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const SYSTEM_UID: i32 = 1000;
const PER_USER_RANGE: i32 = 100_000;
/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;
/// The app host runs user 0 alone.
const USERS: [i32; 1] = [0];

/// An `AssociationInfo`, of which there are none.
enum NoAssociation {}

impl WriteParcelable for NoAssociation {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

pub struct CompanionDevice {
    host: Arc<AppHost>,
}

impl CompanionDevice {
    pub fn new(host: Arc<AppHost>) -> Self {
        CompanionDevice { host }
    }

    /// `AssociationStore`'s associations of `user`: none unless a store
    /// file persists some.
    fn no_associations(&self, user: i32) -> Result<(), Exception> {
        let data = self.host.data()?;
        let files = [
            data.join(format!("system_de/{user}/companion_device_manager.xml")),
            data.join(format!(
                "system/users/{user}/companion_device_manager_associations.xml"
            )),
        ];
        if files.iter().any(|f| f.exists()) {
            return Err(self
                .host
                .unimplemented_named("companiondevice", "persisted associations (#1790)"));
        }
        Ok(())
    }
}

impl CompanionDevice {
    /// `getAllAssociationsForUser`: MANAGE_COMPANION_DEVICES, then
    /// `enforceCallerIsSystemOrCanInteractWithUserId`; the user's active
    /// associations, every user's for `USER_ALL`.
    fn all_associations_for_user(&self, uid: i32, user: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(MANAGE_COMPANION_DEVICES), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "Access denied, requires: {MANAGE_COMPANION_DEVICES}"
            )));
        }
        if uid != SYSTEM_UID
            && uid / PER_USER_RANGE != user
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "uid {uid} does not have {INTERACT_ACROSS_USERS}."
            )));
        }
        if user == USER_ALL {
            return USERS.iter().try_for_each(|&u| self.no_associations(u));
        }
        self.no_associations(user)
    }
}

impl Service for CompanionDevice {
    fn descriptor(&self) -> &str {
        cdm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            cdm::CAN_PAIR_WITHOUT_PROMPT => {
                let a = cdm::CanPairWithoutPrompt::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `getFirstAssociationByAddress`: none, so not without a
                // prompt.
                self.no_associations(a.user_id)
                    .map(|()| cdm::write_can_pair_without_prompt_reply(&mut p, false))
            }
            cdm::GET_ALL_ASSOCIATIONS_FOR_USER => {
                let a =
                    cdm::GetAllAssociationsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.all_associations_for_user(call.sender_euid as i32, a.user_id)
                    .map(|()| {
                        cdm::write_get_all_associations_for_user_reply::<NoAssociation>(
                            &mut p,
                            Some(&[]),
                        )
                    })
            }
            code => {
                let name = cdm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("companiondevice", &format!("{name} (#1790)")))
            }
        };
        reply(result.map(|()| p))
    }
}
