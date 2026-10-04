//! Original package owners available before native PMS scanning (#834).
use super::{
    libraries::{NativePolicyError, Policy},
    owner::permission_gids::{self, PermissionGidError},
    scan::{LibraryCompatibility, PolicyBridgeError},
    system_config::SystemConfig,
};
use aim_binder_host::{
    local::Strong,
    parcel::{EX_ILLEGAL_STATE, Exception, Parcel},
};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;

mod apex;
mod scan;
pub use apex::{ActiveApex, ApexInventory, ApexPackage};
pub use scan::{
    BootError, BootOwners, DataBootInputs, SavedBootScan, SavedSystemPhase, ScanPolicy,
};

pub struct Bridge {
    pub(crate) owner: Strong,
    test_base_on_bcp: bool,
}

#[derive(Debug)]
pub enum SeInfoError {
    Code(String),
    Transport(i32),
    Owner(Exception),
}

#[derive(Debug)]
pub enum OwnerError {
    Code(String),
    Transport(i32),
    Owner(Exception),
}

/// None is the original uninitialized UserManager, distinct from an empty owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanUsers {
    pub users: Option<Vec<super::scan::User>>,
}

impl ScanUsers {
    pub fn read_original_record(bytes: &[u8]) -> aim_binder_host::parcel::Result<Self> {
        use aim_binder_host::parcel::{BAD_VALUE, Reader};
        let mut reader = Reader::new(bytes, &[]);
        let boolean = |r: &mut Reader<'_>| match r.read_i32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(BAD_VALUE),
        };
        let users = if boolean(&mut reader)? {
            let count = reader.read_i32()?;
            if count < 0 || count as usize > reader.remaining() / 12 {
                return Err(BAD_VALUE);
            }
            let mut users = Vec::new();
            let mut previous = -1;
            for _ in 0..count {
                let id = reader.read_i32()?;
                if id <= previous {
                    return Err(BAD_VALUE);
                }
                users.push(super::scan::User {
                    id,
                    pre_created: boolean(&mut reader)?,
                    adb_install_disallowed: boolean(&mut reader)?,
                });
                previous = id;
            }
            Some(users)
        } else {
            None
        };
        if reader.remaining() != 0 || bytes.len() % 4 != 0 {
            return Err(BAD_VALUE);
        }
        Ok(Self { users })
    }
}

fn read_shared_uid_migration(
    reader: &mut aim_binder_host::parcel::Reader<'_>,
) -> Result<super::scan::SharedUidMigration, OwnerError> {
    let best_effort = bridge::read_is_shared_uid_migration_best_effort_reply(reader)
        .map_err(OwnerError::Transport)?
        .map_err(OwnerError::Owner)?;
    if reader.remaining() != 0 {
        return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
    }
    Ok(if best_effort {
        super::scan::SharedUidMigration::BestEffort
    } else {
        super::scan::SharedUidMigration::NewInstallOnly
    })
}

impl Bridge {
    /// System packages and a boot classpath containing test.base require no
    /// PlatformCompat query, as in android-16.0.0_r1 AndroidTestBaseUpdater.
    pub fn remove_test_base(
        &self,
        package: &super::pkg::AndroidPackage,
        is_system: bool,
    ) -> Result<Option<bool>, OwnerError> {
        if self.test_base_on_bcp || is_system {
            return Ok(None);
        }
        self.test_base_change(package).map(Some)
    }

    pub fn test_base_change(
        &self,
        package: &super::pkg::AndroidPackage,
    ) -> Result<bool, OwnerError> {
        let cache = package
            .to_cache_entry()
            .map_err(|error| OwnerError::Code(format!("{error:?}")))?;
        let mut data = Parcel::new();
        bridge::IsTestBaseLibraryChangeEnabled {
            package_cache: Some(cache.bytes),
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_TEST_BASE_LIBRARY_CHANGE_ENABLED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_test_base_library_change_enabled_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(enabled)
    }

    pub fn shared_uid_migration(&self) -> Result<super::scan::SharedUidMigration, OwnerError> {
        let mut data = Parcel::new();
        bridge::IsSharedUidMigrationBestEffort {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_SHARED_UID_MIGRATION_BEST_EFFORT, &data, false)
            .map_err(OwnerError::Transport)?;
        read_shared_uid_migration(&mut reply.reader())
    }

    pub fn notify_apex_scan(
        &self,
        results: &[super::scan::ApexScanResult],
    ) -> Result<(), OwnerError> {
        let payload =
            super::scan::ApexScanResult::notification_payload(results).map_err(OwnerError::Code)?;
        let mut data = Parcel::new();
        bridge::NotifyApexScanResults {
            scan_results: Some(payload),
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::NOTIFY_APEX_SCAN_RESULTS, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_notify_apex_scan_results_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(())
    }

    pub fn apex_inventory(&self) -> Result<ApexInventory, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetApexBootInventory {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_APEX_BOOT_INVENTORY, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_get_apex_boot_inventory_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        ApexInventory::read_original_record(&bytes).map_err(OwnerError::Transport)
    }

    pub fn scan_users(&self) -> Result<ScanUsers, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPackageScanUsers {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_PACKAGE_SCAN_USERS, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_get_package_scan_users_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        ScanUsers::read_original_record(&bytes).map_err(OwnerError::Transport)
    }

    pub fn new_domain_id(&self) -> Result<[u8; 16], OwnerError> {
        let mut data = Parcel::new();
        bridge::GenerateNewDomainId {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GENERATE_NEW_DOMAIN_ID, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_generate_new_domain_id_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        bytes
            .try_into()
            .map_err(|_| OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))
    }

    /// SELinuxMMAC's non-shared decision uses the original ApplicationInfo
    /// generated from parsed code, including flags rather than name/SDK alone.
    /// A nonempty shared UID uses its own boot-fixed SDK instead (#838).
    pub fn seinfo_target_sdk(
        &self,
        package: &super::pkg::AndroidPackage,
    ) -> Result<i32, SeInfoError> {
        let cache = package
            .to_cache_entry()
            .map_err(|error| SeInfoError::Code(format!("{error:?}")))?;
        let mut data = Parcel::new();
        bridge::GetSeInfoTargetSdkVersion {
            package_cache: Some(cache.bytes),
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_SE_INFO_TARGET_SDK_VERSION, &data, false)
            .map_err(SeInfoError::Transport)?;
        let mut reader = reply.reader();
        let target = bridge::read_get_se_info_target_sdk_version_reply(&mut reader)
            .map_err(SeInfoError::Transport)?
            .map_err(SeInfoError::Owner)?;
        if reader.remaining() != 0 {
            return Err(SeInfoError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(target)
    }

    pub(crate) fn new(owner: Strong) -> Result<Self, Exception> {
        // Binder's standard descriptor handshake, before retaining an endpoint.
        let reply = owner
            .transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package bootstrap bridge descriptor: status {status}"),
                )
            })?;
        let mut reader = reply.reader();
        if reader.read_string16().ok().flatten().as_deref() != Some(bridge::DESCRIPTOR)
            || reader.remaining() != 0
        {
            return Err(Exception::illegal_argument(
                "wrong package bootstrap bridge interface",
            ));
        }
        let mut data = Parcel::new();
        bridge::IsTestBaseOnBootclasspath {}.write(&mut data);
        let reply = owner
            .transact(bridge::IS_TEST_BASE_ON_BOOTCLASSPATH, &data, false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package bootstrap classpath policy: status {status}"),
                )
            })?;
        let mut reader = reply.reader();
        let on_bcp =
            bridge::read_is_test_base_on_bootclasspath_reply(&mut reader).map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package bootstrap classpath reply: status {status}"),
                )
            })??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package bootstrap classpath reply has trailing data",
            ));
        }
        Ok(Self {
            owner,
            test_base_on_bcp: on_bcp,
        })
    }

    pub fn library_compatibility(
        &self,
        config: &SystemConfig,
        prop: &dyn Fn(&str) -> Option<String>,
    ) -> Result<LibraryCompatibility, PolicyBridgeError> {
        // This is a pinned image/build property, read before attachment succeeds.
        LibraryCompatibility::new(config, prop, self.test_base_on_bcp)
            .map_err(PolicyBridgeError::Policy)
    }

    pub fn library_policy(
        &self,
        package_name: &str,
        target_sdk: i32,
    ) -> Result<Policy, NativePolicyError> {
        let mut data = Parcel::new();
        bridge::AreNativeLibraryDependenciesEnforced {
            package_name: Some(package_name.into()),
            target_sdk,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(
                bridge::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED,
                &data,
                false,
            )
            .map_err(NativePolicyError::Transport)?;
        bridge::read_are_native_library_dependencies_enforced_reply(&mut reply.reader())
            .map_err(NativePolicyError::Transport)?
            .map_err(NativePolicyError::Owner)
            .map(Policy::pinned)
    }

    pub fn permission_gids(
        &self,
        app_id: i32,
        users: &[i32],
    ) -> Result<Vec<u32>, PermissionGidError> {
        permission_gids::query_with(app_id, users, |uid| {
            let mut data = Parcel::new();
            bridge::GetPermissionGidsForUid { uid }.write(&mut data);
            let reply = self
                .owner
                .transact(bridge::GET_PERMISSION_GIDS_FOR_UID, &data, false)
                .map_err(PermissionGidError::Transport)?;
            bridge::read_get_permission_gids_for_uid_reply(&mut reply.reader())
                .map_err(PermissionGidError::Transport)?
                .map_err(PermissionGidError::Owner)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_policy_reply_preserves_owner_failure_and_rejects_truncation_or_tails() {
        use super::super::scan::SharedUidMigration;
        for (enabled, expected) in [
            (false, SharedUidMigration::NewInstallOnly),
            (true, SharedUidMigration::BestEffort),
        ] {
            let mut reply = Parcel::new();
            reply.write_no_exception();
            reply.write_bool(enabled);
            assert_eq!(
                read_shared_uid_migration(&mut aim_binder_host::parcel::Reader::new(
                    reply.data(),
                    reply.objects()
                ))
                .unwrap(),
                expected
            );
            reply.write_i32(0);
            assert!(matches!(
                read_shared_uid_migration(&mut aim_binder_host::parcel::Reader::new(
                    reply.data(),
                    reply.objects()
                )),
                Err(OwnerError::Transport(_))
            ));
        }
        let mut missing = Parcel::new();
        missing.write_no_exception();
        assert!(matches!(
            read_shared_uid_migration(&mut aim_binder_host::parcel::Reader::new(
                missing.data(),
                missing.objects()
            )),
            Err(OwnerError::Transport(_))
        ));
        let mut denied = Parcel::new();
        denied.write_exception(&Exception::security("policy caller denied"));
        assert!(
            matches!(read_shared_uid_migration(&mut aim_binder_host::parcel::Reader::new(denied.data(), denied.objects())), Err(OwnerError::Owner(e)) if e.message == "policy caller denied")
        );
    }

    #[test]
    fn original_scan_users_distinguish_missing_owner_and_reject_invalid_frames() {
        let frame = |words: &[i32]| {
            let mut p = Parcel::new();
            for word in words {
                p.write_i32(*word);
            }
            p.data().to_vec()
        };
        assert_eq!(
            ScanUsers::read_original_record(&frame(&[0])).unwrap().users,
            None
        );
        assert_eq!(
            ScanUsers::read_original_record(&frame(&[1, 0]))
                .unwrap()
                .users,
            Some(Vec::new())
        );
        let original = frame(&[1, 2, 0, 0, 1, 10, 1, 0]);
        let users = ScanUsers::read_original_record(&original).unwrap();
        assert_eq!(
            users
                .users
                .unwrap()
                .iter()
                .map(|u| (u.id, u.pre_created, u.adb_install_disallowed))
                .collect::<Vec<_>>(),
            [(0, false, true), (10, true, false)]
        );
        for words in [
            vec![],
            vec![2],
            vec![0, 0],
            vec![1, -1],
            vec![1, 2],
            vec![1, 1, -1, 0, 0],
            vec![1, 1, 0, 2, 0],
            vec![1, 1, 0, 0, 2],
            vec![1, 2, 0, 0, 0, 0, 0, 0],
            vec![1, 2, 10, 0, 0, 0, 0, 0],
            vec![1, 0, 99],
        ] {
            assert!(
                ScanUsers::read_original_record(&frame(&words)).is_err(),
                "{words:?}"
            );
        }
        let mut bad = original.clone();
        bad.push(0);
        assert!(ScanUsers::read_original_record(&bad).is_err());
        assert!(ScanUsers::read_original_record(&original[..original.len() - 1]).is_err());
    }
}
