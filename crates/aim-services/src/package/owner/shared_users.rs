//! First-boot identity input, ported from pinned PackageManagerService's
//! constructor and Settings.add[Oem]SharedUserLPw (#803).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::app_ids::{AppIds, Error, Owner};
use crate::package::system_config::SystemConfig;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedUser {
    pub app_id: i32,
    pub flags: i32,
    pub private_flags: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    InvalidName,
    InvalidOemId,
    ConflictingName { existing_id: i32 },
    Slot(Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub name: String,
    pub app_id: i32,
    pub reason: Rejection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bootstrap {
    pub ids: AppIds,
    pub shared_users: BTreeMap<String, SharedUser>,
    pub rejected: Vec<Rejected>,
}

impl Bootstrap {
    /// Seed before Settings or APK reconciliation. No persistence or
    /// query snapshot is changed; rejected OEM records remain visible.
    pub fn new(config: &SystemConfig) -> Self {
        let mut boot = Self {
            ids: AppIds::default(),
            shared_users: BTreeMap::new(),
            rejected: Vec::new(),
        };
        for (name, id) in [
            ("android.uid.system", 1000),
            ("android.uid.phone", 1001),
            ("android.uid.log", 1007),
            ("android.uid.nfc", 1027),
            ("android.uid.bluetooth", 1002),
            ("android.uid.shell", 2000),
            ("android.uid.se", 1031),
            ("android.uid.networkstack", 1073),
            ("android.uid.uwb", 1083),
        ] {
            boot.register(name, id)
                .expect("distinct platform shared users");
        }
        for (name, id) in &config.oem_defined_uids {
            let result = if !name.starts_with("android.uid") {
                Err(Rejection::InvalidName)
            } else if !(2900..=2999).contains(id) {
                Err(Rejection::InvalidOemId)
            } else {
                boot.register(name, *id)
            };
            if let Err(reason) = result {
                boot.rejected.push(Rejected {
                    name: name.clone(),
                    app_id: *id,
                    reason,
                });
            }
        }
        boot
    }

    fn register(&mut self, name: &str, app_id: i32) -> Result<(), Rejection> {
        if let Some(old) = self.shared_users.get(name) {
            return if old.app_id == app_id {
                Ok(())
            } else {
                Err(Rejection::ConflictingName {
                    existing_id: old.app_id,
                })
            };
        }
        self.ids
            .register_existing(app_id, Owner::SharedUser(name.into()))
            .map_err(Rejection::Slot)?;
        self.shared_users.insert(
            name.into(),
            SharedUser {
                app_id,
                flags: 1,
                private_flags: 8,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeds_fixed_ids_and_retains_oem_rejections_without_reassigning() {
        let mut config = SystemConfig::default();
        config.oem_defined_uids = [
            ("android.uid.oem", 2900),
            ("android.uid.collision", 2900),
            ("android.uid.system", 2901),
            ("other", 2902),
            ("android.uid.low", 2899),
            ("android.uid.high", 3000),
            ("android.uid", 2999),
        ]
        .map(|(name, id)| (name.into(), id))
        .into();
        let mut boot = Bootstrap::new(&config);
        assert_eq!(boot.shared_users.len(), 11);
        assert_eq!(boot.shared_users["android.uid.system"].app_id, 1000);
        assert_eq!(
            boot.shared_users["android.uid.oem"],
            SharedUser {
                app_id: 2900,
                flags: 1,
                private_flags: 8
            }
        );
        assert_eq!(boot.rejected.len(), 5);
        assert!(matches!(
            boot.rejected[0].reason,
            Rejection::Slot(Error::Occupied { .. })
        ));
        assert_eq!(
            boot.rejected[1].reason,
            Rejection::ConflictingName { existing_id: 1000 }
        );
        assert_eq!(boot.rejected[2].reason, Rejection::InvalidName);
        assert_eq!(boot.rejected[3].reason, Rejection::InvalidOemId);
        assert_eq!(boot.rejected[4].reason, Rejection::InvalidOemId);
        assert_eq!(boot.ids.acquire(Owner::Package("new".into())), Ok(10000));
    }
}
