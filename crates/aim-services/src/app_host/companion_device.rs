//! `companiondevice`: CompanionDeviceManagerService for the app host
//! (`android-16.0.0_r1`), published as the original publishes it with
//! `FEATURE_COMPANION_DEVICE_SETUP`, for a user with no associations
//! persisted (`AssociationDiskStore`: `system_de/<user>/companion_device_manager.xml`,
//! else the legacy `system/users/<user>/companion_device_manager_associations.xml`):
//! `canPairWithoutPrompt`, which the Bluetooth app asks when pairing.
//! Persisted associations and every other call are not implemented
//! (#1790).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_companion_icompaniondevicemanager as cdm;

use super::{AppHost, reply};

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
