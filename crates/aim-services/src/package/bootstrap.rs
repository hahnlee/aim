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
mod existing;
mod query;
mod runtime;
mod preferred;
mod scan;
pub use apex::{ActiveApex, ApexInventory, ApexPackage};
pub use query::QueryContextError;
pub use scan::{
    BootError, BootOwners, DataBootInputs, SavedBootScan, SavedSystemPhase, ScanPolicy,
};

pub struct Bridge {
    pub(crate) owner: Strong,
    test_base_on_bcp: bool,
    pub(crate) signing_debuggable: bool,
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

fn read_current_package_version(
    bytes: &[u8],
) -> aim_binder_host::parcel::Result<super::settings::Version> {
    let mut reader = aim_binder_host::parcel::Reader::new(bytes, &[]);
    let current = super::settings::Version {
        volume_uuid: None,
        sdk_version: reader.read_i32()?,
        database_version: reader.read_i32()?,
        build_fingerprint: reader.read_string16()?,
        fingerprint: reader.read_string16()?,
    };
    if reader.remaining() != 0 || bytes.len() % 4 != 0 {
        return Err(aim_binder_host::parcel::BAD_VALUE);
    }
    Ok(current)
}

fn read_installer_user_policy_record(
    bytes: &[u8],
    user: i32,
) -> aim_binder_host::parcel::Result<(bool, super::installer::policy::UserPolicy)> {
    use aim_binder_host::parcel::{BAD_VALUE, Reader};
    let mut reader = Reader::new(bytes, &[]);
    if reader.read_i32()? != user {
        return Err(BAD_VALUE);
    }
    let boolean = |r: &mut Reader<'_>| match r.read_i32()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(BAD_VALUE),
    };
    let exists = boolean(&mut reader)?;
    let policy = super::installer::policy::UserPolicy {
        disallow_install_apps: boolean(&mut reader)?,
        disallow_debugging_features: boolean(&mut reader)?,
        organization_managed: boolean(&mut reader)?,
    };
    if reader.remaining() != 0 {
        return Err(BAD_VALUE);
    }
    Ok((exists, policy))
}

#[cfg(test)]
fn read_installer_user_policy(
    bytes: &[u8],
    user: i32,
) -> aim_binder_host::parcel::Result<Option<super::installer::policy::UserPolicy>> {
    let (exists, policy) = read_installer_user_policy_record(bytes, user)?;
    Ok(exists.then_some(policy))
}

impl Bridge {
    pub fn format_package_timestamp(&self, millis: i64) -> Result<String, OwnerError> {
        let mut data = Parcel::new();
        bridge::FormatPackageTimestamp { millis }.write(&mut data);
        let reply = self.owner.transact(bridge::FORMAT_PACKAGE_TIMESTAMP, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = bridge::read_format_package_timestamp_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or_else(|| OwnerError::Code("package diagnostic timestamp absent".into()))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(value)
    }

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

    fn installer_user_policy_record(
        &self,
        user: i32,
    ) -> Result<(bool, super::installer::policy::UserPolicy), OwnerError> {
        let mut data = Parcel::new();
        bridge::GetInstallerUserPolicy { user_id: user }.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_INSTALLER_USER_POLICY, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_get_installer_user_policy_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        read_installer_user_policy_record(&bytes, user).map_err(OwnerError::Transport)
    }

    pub fn installer_user_policy(
        &self,
        user: i32,
    ) -> Result<Option<super::installer::policy::UserPolicy>, OwnerError> {
        let (exists, policy) = self.installer_user_policy_record(user)?;
        Ok(exists.then_some(policy))
    }

    pub fn allocate_installer_bytes(
        &self,
        file: &std::fs::File,
        length: i64,
        flags: i32,
    ) -> Result<(), OwnerError> {
        use std::os::fd::AsFd;
        struct Descriptor(aim_binder_driver::File);
        impl aim_service_aidl::WriteParcelable for Descriptor {
            fn write_to(&self, parcel: &mut Parcel) {
                parcel.write_i32(0);
                parcel.write_file(self.0.clone());
            }
        }
        let file = aim_binder_host::server::file_from_fd(file.as_fd())
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        let mut data = Parcel::new();
        bridge::AllocateInstallerBytes {
            file: Some(Descriptor(file)),
            length_bytes: length,
            install_flags: flags,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::ALLOCATE_INSTALLER_BYTES, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_allocate_installer_bytes_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(())
    }

    pub fn provider_authority_grants(
        &self, uid: i32, provider: &super::info::ProviderInfo, user: i32,
    ) -> Result<bool, OwnerError> {
        let mut request = Parcel::new();
        bridge::CheckProviderAuthorityGrants {
            calling_uid: uid, provider_info: Some(provider.clone()), user_id: user,
        }.write(&mut request);
        let reply = self.owner.transact(bridge::CHECK_PROVIDER_AUTHORITY_GRANTS, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let result = bridge::read_check_provider_authority_grants_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(result)
    }

    pub fn provider_clone_redirected(&self, authority: &str, uid: i32, user: i32) -> Result<bool, OwnerError> {
        let mut request = Parcel::new();
        bridge::IsProviderCloneRedirected {
            authority: Some(authority.into()), calling_uid: uid, user_id: user,
        }.write(&mut request);
        let reply = self.owner.transact(bridge::IS_PROVIDER_CLONE_REDIRECTED, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let result = bridge::read_is_provider_clone_redirected_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(result)
    }

    pub fn instant_cookie_limit(&self) -> Result<i32, OwnerError> {
        let mut request = Parcel::new(); bridge::GetInstantAppCookieLimit {}.write(&mut request);
        let reply = self.owner.transact(bridge::GET_INSTANT_APP_COOKIE_LIMIT, &request, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let limit = bridge::read_get_instant_app_cookie_limit_reply(&mut reader).map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(limit)
    }
    pub fn instant_icon_density(&self) -> Result<i32, OwnerError> {
        let mut request = Parcel::new(); bridge::GetInstantAppIconDensity {}.write(&mut request);
        let reply = self.owner.transact(bridge::GET_INSTANT_APP_ICON_DENSITY, &request, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let density = bridge::read_get_instant_app_icon_density_reply(&mut reader).map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(density)
    }

    pub fn instant_configuration(&self) -> Result<(i32, i32), OwnerError> {
        let limit = self.instant_cookie_limit()?;
        let density = self.instant_icon_density()?;
        Ok((limit, density))
    }

    pub fn package_profile_parent(&self, user: i32) -> Result<Option<i32>, OwnerError> {
        let mut request = Parcel::new(); bridge::GetPackageProfileParent { user_id: user }.write(&mut request);
        let reply = self.owner.transact(bridge::GET_PACKAGE_PROFILE_PARENT, &request, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let parent = bridge::read_get_package_profile_parent_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok((parent != -10000).then_some(parent))
    }

    pub fn parent_profile_app_linking(&self, user: i32) -> Result<bool, OwnerError> {
        let mut request = Parcel::new(); bridge::IsParentProfileAppLinkingAllowed { user_id: user }.write(&mut request);
        let reply = self.owner.transact(bridge::IS_PARENT_PROFILE_APP_LINKING_ALLOWED, &request, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let allowed = bridge::read_is_parent_profile_app_linking_allowed_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        Ok(allowed)
    }

    pub fn package_role_holders(&self, role: &str, user: i32) -> Result<Vec<String>, OwnerError> {
        let mut request = Parcel::new(); bridge::GetPackageRoleHolders { role: Some(role.into()), user_id: user }.write(&mut request);
        let reply = self.owner.transact(bridge::GET_PACKAGE_ROLE_HOLDERS, &request, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let holders = bridge::read_get_package_role_holders_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        holders.into_iter().map(|value| value.ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))).collect()
    }

    pub fn application_data(&self) -> Result<Strong, OwnerError> {
        let mut request = Parcel::new(); bridge::GetApplicationDataBridge {}.write(&mut request);
        let reply = self.owner.transact(bridge::GET_APPLICATION_DATA_BRIDGE, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_application_data_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn package_relocation(&self) -> Result<Strong, OwnerError> {
        let mut request = Parcel::new(); bridge::GetPackageRelocationBridge {}.write(&mut request);
        let reply = self.owner.transact(bridge::GET_PACKAGE_RELOCATION_BRIDGE, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_relocation_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn package_moves(&self) -> Result<Strong, OwnerError> {
        let mut request = Parcel::new(); bridge::GetPackageMoveBridge {}.write(&mut request);
        let reply = self.owner.transact(bridge::GET_PACKAGE_MOVE_BRIDGE, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_move_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn package_effects(&self) -> Result<Strong, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPackageMutationBridge {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_PACKAGE_MUTATION_BRIDGE, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_mutation_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn package_maintenance(&self) -> Result<Strong, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPackageMaintenanceBridge {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_PACKAGE_MAINTENANCE_BRIDGE, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_maintenance_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn installer_external(&self) -> Result<Strong, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetInstallerExternalBridge {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_INSTALLER_EXTERNAL_BRIDGE, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_installer_external_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn installer_files(&self) -> Result<Strong, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetPackageInstallerFiles {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_PACKAGE_INSTALLER_FILES, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_package_installer_files_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn staging_bridge(&self) -> Result<Strong, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetNativeStagingBridge {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_NATIVE_STAGING_BRIDGE, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_get_native_staging_bridge_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        reply.retain_remote_binder(binder).map_err(OwnerError::Transport)
    }

    pub fn package_monitor_result(&self, action: &str, package: &str, user: i32, uid: i32, replacing: bool) -> Result<Parcel, OwnerError> {
        let mut data = Parcel::new();
        bridge::PackageMonitorResult { action: Some(action.into()), package_name: Some(package.into()), user_id: user, uid, replacing }.write(&mut data);
        let reply = self.owner.transact(bridge::PACKAGE_MONITOR_RESULT, &data, false).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_package_monitor_result_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 { return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
        let mut payload = Parcel::new();
        payload.write_raw(&bytes, &[]);
        Ok(payload)
    }

    pub fn allocate_preferred_identity(&self) -> Result<super::preferred::registry::Identity, OwnerError> {
        use aim_service_aidl::dev_aim_server_ipackageresolveridentity as identity;
        let mut data = Parcel::new();
        bridge::AllocatePreferredResolverIdentity {}.write(&mut data);
        let reply = self.owner.transact(bridge::ALLOCATE_PREFERRED_RESOLVER_IDENTITY, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let binder = bridge::read_allocate_preferred_resolver_identity_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        let lease = reply.retain_remote_binder(binder).map_err(OwnerError::Transport)?;
        let mut request = Parcel::new();
        identity::GetIdentityHash {}.write(&mut request);
        let reply = lease.transact(identity::GET_IDENTITY_HASH, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let hash = identity::read_get_identity_hash_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(super::preferred::registry::Identity { hash, lease: std::sync::Arc::new(lease) })
    }

    pub fn installer_art_service_v3_enabled(&self) -> Result<bool, OwnerError> {
        let mut data = Parcel::new();
        bridge::IsInstallerArtServiceV3Enabled {}.write(&mut data);
        let reply = self.owner.transact(bridge::IS_INSTALLER_ART_SERVICE_V3ENABLED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_installer_art_service_v3enabled_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(enabled)
    }

    pub fn installer_domain_limits(&self) -> Result<(i64, i64), OwnerError> {
        let mut data = Parcel::new();
        bridge::GetInstallerDomainLimits {}.write(&mut data);
        let reply = self.owner.transact(bridge::GET_INSTALLER_DOMAIN_LIMITS, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let limits = bridge::read_get_installer_domain_limits_reply(&mut reader)
            .map_err(OwnerError::Transport)?.map_err(OwnerError::Owner)?;
        let Some(limits) = limits.filter(|limits| limits.len() == 2) else {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        };
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok((limits[0], limits[1]))
    }

    pub fn installer_revocable_fd_enabled(&self) -> Result<bool, OwnerError> {
        let mut data = Parcel::new();
        bridge::IsInstallerRevocableFdEnabled {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_INSTALLER_REVOCABLE_FD_ENABLED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        let mode = match reader.read_i32().map_err(OwnerError::Transport)? {
            0 => false,
            1 => true,
            _ => return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)),
        };
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(mode)
    }

    pub fn shell_debugging_restricted(&self, user: i32) -> Result<bool, OwnerError> {
        let mut data = Parcel::new();
        bridge::IsShellDebuggingRestricted { user_id: user }.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_SHELL_DEBUGGING_RESTRICTED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        let restricted = match reader.read_i32().map_err(OwnerError::Transport)? {
            0 => false,
            1 => true,
            _ => return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE)),
        };
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(restricted)
    }

    pub fn restore_installer_context(&self, path: &str) -> Result<(), OwnerError> {
        let mut data = Parcel::new();
        bridge::RestoreInstallerContext {
            path: Some(path.into()),
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::RESTORE_INSTALLER_CONTEXT, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_restore_installer_context_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(())
    }

    pub fn current_package_version(&self) -> Result<super::settings::Version, OwnerError> {
        let mut data = Parcel::new();
        bridge::GetCurrentPackageVersion {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::GET_CURRENT_PACKAGE_VERSION, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_get_current_package_version_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?
            .ok_or(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE))?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        read_current_package_version(&bytes).map_err(OwnerError::Transport)
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
        let mut data = Parcel::new();
        bridge::IsSigningDebuggable {}.write(&mut data);
        let reply = owner
            .transact(bridge::IS_SIGNING_DEBUGGABLE, &data, false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package signing build policy: status {status}"),
                )
            })?;
        let mut reader = reply.reader();
        let signing_debuggable =
            bridge::read_is_signing_debuggable_reply(&mut reader).map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package signing build policy reply: status {status}"),
                )
            })??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package signing build policy has trailing data",
            ));
        }
        Ok(Self {
            owner,
            test_base_on_bcp: on_bcp,
            signing_debuggable,
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
        let mut reader = reply.reader();
        let enforce_native_dependencies =
            bridge::read_are_native_library_dependencies_enforced_reply(&mut reader)
                .map_err(NativePolicyError::Transport)?
                .map_err(NativePolicyError::Owner)?;
        if reader.remaining() != 0 {
            return Err(NativePolicyError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        let mut data = Parcel::new();
        bridge::IsSdkLibraryIndependenceEnabled {}.write(&mut data);
        let reply = self.owner
            .transact(bridge::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED, &data, false)
            .map_err(NativePolicyError::Transport)?;
        let mut reader = reply.reader();
        let sdk_library_independence =
            bridge::read_is_sdk_library_independence_enabled_reply(&mut reader)
                .map_err(NativePolicyError::Transport)?
                .map_err(NativePolicyError::Owner)?;
        if reader.remaining() != 0 {
            return Err(NativePolicyError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        Ok(Policy { enforce_native_dependencies, sdk_library_independence })
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
            let mut reader = reply.reader();
            let result = bridge::read_get_permission_gids_for_uid_reply(&mut reader)
                .map_err(PermissionGidError::Transport)?
                .map_err(PermissionGidError::Owner)?;
            if reader.remaining() != 0 {
                return Err(PermissionGidError::Transport(
                    aim_binder_host::parcel::BAD_VALUE,
                ));
            }
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sdk_library_policy_reads_live_binder_flag_and_preserves_owner_failures() {
        use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
        use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
        use aim_binder_host::parcel::{Binder, UNKNOWN_TRANSACTION, BAD_VALUE};
        use std::sync::{Arc, atomic::{AtomicI32, Ordering}};
        struct NoMemory;
        impl GuestProcess for NoMemory {
            fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> { Err(errno::EFAULT) }
            fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> { Err(errno::EFAULT) }
            fn get_file(&mut self, _: u32) -> Result<File, Errno> { Err(errno::EBADF) }
            fn install_file(&mut self, _: File) -> Result<u32, Errno> { Err(errno::EBADF) }
            fn close_fd(&mut self, _: u32) { panic!("unexpected fd") }
        }
        struct Flags(Arc<AtomicI32>);
        impl Service for Flags {
            fn descriptor(&self) -> &str { bridge::DESCRIPTOR }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, 1000);
                let mut reply = Parcel::new();
                match call.code {
                    bridge::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED => {
                        let args = bridge::AreNativeLibraryDependenciesEnforced::read(&mut call.data)?;
                        assert_eq!(args.package_name.as_deref(), Some("consumer"));
                        assert_eq!(args.target_sdk, 36);
                        bridge::write_are_native_library_dependencies_enforced_reply(&mut reply, true);
                    }
                    bridge::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED => {
                        bridge::IsSdkLibraryIndependenceEnabled::read(&mut call.data)?;
                        match self.0.load(Ordering::Acquire) {
                            2 => reply.write_exception(&Exception::new(EX_ILLEGAL_STATE, "flag owner failed")),
                            3 => { reply.write_no_exception(); },
                            4 => { bridge::write_is_sdk_library_independence_enabled_reply(&mut reply, true); reply.write_i32(99); },
                            5 => return Err(BAD_VALUE),
                            enabled => bridge::write_is_sdk_library_independence_enabled_reply(&mut reply, enabled == 1),
                        }
                    }
                    _ => return Err(UNKNOWN_TRANSACTION),
                }
                assert_eq!(call.data.remaining(), 0);
                Ok(reply)
            }
        }
        struct Processes { driver: Arc<Driver>, server: Arc<LocalProcess>, client: Arc<LocalProcess> }
        impl Drop for Processes {
            fn drop(&mut self) {
                self.driver.release(self.client.proc_handle());
                self.driver.release(self.server.proc_handle());
            }
        }
        let driver = Driver::new();
        let open = |pid| LocalProcess::open(&driver, Device::Binder, Credentials { pid, euid: 1000, security_context: None });
        let server = open(99501);
        let client = open(99502);
        let _processes = Processes { driver: driver.clone(), server: server.clone(), client: client.clone() };
        let phase = Arc::new(AtomicI32::new(0));
        let Binder::Local(ptr) = server.add_service(Arc::new(Flags(phase.clone()))) else { unreachable!() };
        let mut object = FlatBinderObject { kind: BINDER_TYPE_BINDER, flags: 0, binder: ptr, cookie: ptr }.encode();
        driver.ioctl(server.proc_handle(), 99503, BINDER_SET_CONTEXT_MGR_EXT, &mut object, &mut NoMemory).unwrap();
        server.start();
        client.start();
        let owner = Bridge { owner: client.strong(0), test_base_on_bcp: false, signing_debuggable: false };
        let registry = crate::package::libraries::Registry::new(&SystemConfig::default());
        let available = std::collections::BTreeMap::new();
        let mut pkg = crate::package::pkg::AndroidPackage::default();
        pkg.uses_sdk_libraries = vec!["sdk".into()];
        pkg.uses_sdk_libraries_versions_major = Some(vec![1]);
        pkg.uses_sdk_libraries_optional = Some(vec![true]);
        for enabled in [false, true] {
            phase.store(i32::from(enabled), Ordering::Release);
            let policy = owner.library_policy("consumer", 36).unwrap();
            assert!(policy.enforce_native_dependencies);
            assert_eq!(policy.sdk_library_independence, enabled);
            let result = registry.collect(&pkg, &available, policy);
            if enabled { assert!(result.unwrap().libraries.is_empty()); }
            else { assert_eq!(result.unwrap_err(), crate::package::libraries::ResolveError::MissingLibrary("sdk".into())); }
        }
        pkg.uses_sdk_libraries_optional = Some(vec![false]);
        assert_eq!(registry.collect(&pkg, &available, owner.library_policy("consumer", 36).unwrap()).unwrap_err(),
            crate::package::libraries::ResolveError::MissingLibrary("sdk".into()));
        phase.store(2, Ordering::Release);
        assert!(matches!(owner.library_policy("consumer", 36), Err(NativePolicyError::Owner(error)) if error.code == EX_ILLEGAL_STATE && error.message == "flag owner failed"));
        for phase_value in [3, 4, 5] {
            phase.store(phase_value, Ordering::Release);
            assert!(matches!(owner.library_policy("consumer", 36), Err(NativePolicyError::Transport(_))));
        }
    }

    #[test]
    fn installer_policy_record_rejects_foreign_user_nonboolean_and_tail() {
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        parcel.write_i32(10);
        for value in [1, 1, 0, 1] {
            parcel.write_i32(value);
        }
        let policy = super::read_installer_user_policy(parcel.data(), 10)
            .unwrap()
            .unwrap();
        assert!(policy.disallow_install_apps && policy.organization_managed);
        assert!(!policy.disallow_debugging_features);
        assert!(super::read_installer_user_policy(parcel.data(), 0).is_err());
        for size in 0..parcel.data().len() {
            assert!(super::read_installer_user_policy(&parcel.data()[..size], 10).is_err());
        }
        parcel.set_i32_at(4, 0);
        assert!(
            super::read_installer_user_policy(parcel.data(), 10)
                .unwrap()
                .is_none()
        );
        assert!(
            !super::read_installer_user_policy_record(parcel.data(), 10)
                .unwrap()
                .1
                .disallow_debugging_features
        );
        parcel.set_i32_at(8, 2);
        assert!(super::read_installer_user_policy(parcel.data(), 10).is_err());
        parcel.set_i32_at(8, 1);
        parcel.write_i32(0);
        assert!(super::read_installer_user_policy(parcel.data(), 10).is_err());
    }

    use super::*;
    #[test]
    fn current_version_frame_retains_nulls_and_rejects_truncation_or_trailing_data() {
        let mut data = Parcel::new();
        data.write_i32(36);
        data.write_i32(3);
        data.write_string16(Some("build"));
        data.write_string16(None);
        let version = read_current_package_version(data.data()).unwrap();
        assert_eq!(version.sdk_version, 36);
        assert_eq!(version.database_version, 3);
        assert_eq!(version.build_fingerprint.as_deref(), Some("build"));
        assert!(version.fingerprint.is_none());
        for end in 0..data.data().len() {
            assert!(read_current_package_version(&data.data()[..end]).is_err());
        }
        data.write_i32(99);
        assert!(read_current_package_version(data.data()).is_err());
    }

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

mod visibility;
