//! The feed's records, as `PackageFeed.java` writes them: Parcels of
//! strings (`writeString`), ints, longs, booleans (`writeBoolean`), byte
//! and int arrays, lists as their size (-1 for null) and elements, and
//! boolean getters packed into flag words in the order below.

use aim_binder_host::parcel::{Reader, Result};
use aim_service_aidl::{read_byte_array, read_int_array, read_string_list};

use crate::package::intent_filter::UriRelativeFilterGroup;
use crate::package::model::{
    InstallSource, OverlayPaths, PackageState, PackageUserState, Platform, SharedLibrary,
    SharedUser, StateFlags, User,
};
use crate::package::restrictions::{ArchiveActivity, ArchiveState};
use crate::package::settings::{PRIVATE_FLAG_PRIVILEGED, Signatures, UsesSdkLibrary};

/// A package state's record: `PackageState`'s getters, its signing,
/// install source, installed permissions, domain verification state and
/// each user's state, and its shared user's app id (the shared user's
/// name is in that shared user's record). Its parcel comes in a record
/// of its own.
pub fn package(bytes: &[u8]) -> Result<(PackageState, Option<i32>)> {
    let r = &mut Reader::new(bytes, &[]);
    let mut s = PackageState {
        name: string(r)?.unwrap_or_default(),
        app_id: r.read_i32()?,
        ..PackageState::default()
    };
    let shared_user_app_id = r.read_i32()?;
    s.path = string(r)?.unwrap_or_default();
    s.volume_uuid = string(r)?;
    s.primary_cpu_abi = string(r)?;
    s.secondary_cpu_abi = string(r)?;
    s.cpu_abi_override = string(r)?;
    s.seinfo = string(r)?;
    s.apex_module_name = string(r)?;
    s.version_code = r.read_i64()?;
    s.target_sdk_version = r.read_i32()?;
    s.category_override = r.read_i32()?;
    s.hidden_api_enforcement_policy = r.read_i32()?;
    s.last_modified_time = r.read_i64()?;
    s.last_update_time = r.read_i64()?;
    s.restrict_update_hash = read_byte_array(r)?;
    let f = r.read_i32()?;
    let bit = |i: u32| f & (1 << i) != 0;
    // hasSharedUser, isApex, isApkInUpdatedApex, isDebuggable,
    // isDefaultToDeviceProtectedStorage, isExternalStorage,
    // isForceQueryableOverride, isHiddenUntilInstalled,
    // isInstallPermissionsFixed, isLeavingSharedUser, isOdm, isOem,
    // isPageSizeAppCompatEnabled, isPendingRestore, isPersistent,
    // isPrivileged, isProduct, isRequiredForSystemUser,
    // isScannedAsStoppedSystemApp, isSystem, isSystemExt,
    // isUpdateAvailable, isUpdatedSystemApp, isVendor.
    s.is = StateFlags {
        apex: bit(1),
        apk_in_updated_apex: bit(2),
        debuggable: bit(3),
        default_to_device_protected_storage: bit(4),
        force_queryable_override: bit(6),
        hidden_until_installed: bit(7),
        install_permissions_fixed: bit(8),
        odm: bit(10),
        oem: bit(11),
        pending_restore: bit(13),
        privileged: bit(15),
        product: bit(16),
        scanned_as_stopped_system_app: bit(18),
        system: bit(19),
        system_ext: bit(20),
        update_available: bit(21),
        updated_system_app: bit(22),
        vendor: bit(23),
        // Only PackageStateInternal has isLoading (#716).
        loading: false,
    };
    let groups = r.read_i32()?;
    for _ in 0..groups.max(0) {
        let name = string(r)?.unwrap_or_default();
        s.mime_groups.push((name, strings(r)?));
    }
    for _ in 0..count(r)? {
        s.uses_static_libraries
            .push((string(r)?.unwrap_or_default(), r.read_i64()?));
    }
    for _ in 0..count(r)? {
        s.uses_sdk_libraries.push(UsesSdkLibrary {
            name: string(r)?.unwrap_or_default(),
            version_major: r.read_i64()?,
            optional: r.read_bool()?,
        });
    }
    s.uses_library_files = strings(r)?;
    for _ in 0..count(r)? {
        s.uses_library_infos.push(shared_library(r)?);
    }
    s.installed_permissions = strings(r)?;
    s.signatures = signing(r)?;
    if r.read_bool()? {
        s.install_source = InstallSource {
            installer: string(r)?,
            initiating_package: string(r)?,
            originating_package: string(r)?,
            update_owner: string(r)?,
            package_source: r.read_i32()?,
            initiating_package_signatures: signing(r)?,
            // No API gives the rest (#714).
            ..InstallSource::default()
        };
    }
    let domains = r.read_bool()?;
    if domains {
        let id = string(r)?.unwrap_or_default();
        s.domain_verification = Some((id, host_states(r)?));
    }
    for _ in 0..count(r)? {
        let user = r.read_i32()?;
        s.users.insert(user, user_state(r)?);
    }
    for _ in 0..count(r)? {
        let domain = string(r)?.unwrap_or_default();
        let mut groups = Vec::new();
        for _ in 0..count(r)? {
            let mut g = UriRelativeFilterGroup::new(r.read_i32()?);
            for _ in 0..count(r)? {
                let (part, kind) = (r.read_i32()?, r.read_i32()?);
                g.add(part, kind, &string(r)?.unwrap_or_default());
            }
            groups.push(g);
        }
        s.uri_relative_filter_groups.push((domain, groups));
    }
    s.filter_application_query = Some(r.read_bool()?);
    Ok((s, bit(0).then_some(shared_user_app_id)))
}

fn user_state(r: &mut Reader<'_>) -> Result<PackageUserState> {
    let ce_data_inode = r.read_i64()?;
    let de_data_inode = r.read_i64()?;
    let f = r.read_i32()?;
    let bit = |i: u32| f & (1 << i) != 0;
    // isInstalled, isStopped, isNotLaunched, isHidden, isSuspended,
    // isInstantApp, isVirtualPreload, isQuarantined, dataExists.
    let mut u = PackageUserState {
        ce_data_inode,
        de_data_inode,
        installed: bit(0),
        stopped: bit(1),
        not_launched: bit(2),
        hidden: bit(3),
        instant_app: bit(5),
        virtual_preload: bit(6),
        quarantined: bit(7),
        data_exists: bit(8),
        distraction_flags: r.read_i32()?,
        enabled: r.read_i32()?,
        last_disable_app_caller: string(r)?,
        enabled_components: strings(r)?,
        disabled_components: strings(r)?,
        install_reason: r.read_i32()?,
        uninstall_reason: r.read_i32()?,
        harmful_app_warning: string(r)?,
        splash_screen_theme: string(r)?,
        first_install_time: r.read_i64()?,
        min_aspect_ratio: r.read_i32()?,
        ..PackageUserState::default()
    };
    if r.read_bool()? {
        let installer_title = string(r)?.unwrap_or_default();
        let archive_time = r.read_i64()?;
        let mut activities = Vec::new();
        for _ in 0..count(r)? {
            activities.push(ArchiveActivity {
                title: string(r)?.unwrap_or_default(),
                original_component_name: string(r)?.unwrap_or_default(),
                icon_path: string(r)?.unwrap_or_default(),
                monochrome_icon_path: string(r)?,
            });
        }
        u.archive_state = Some(ArchiveState {
            installer_title,
            archive_time,
            activities,
        });
    }
    if r.read_bool()? {
        u.overlay_paths = Some(OverlayPaths {
            overlay_paths: strings(r)?,
            resource_dirs: strings(r)?,
        });
    }
    let suspending = string(r)?;
    if bit(4) {
        // The original's own suspension when the API names nobody.
        u.suspended_by = vec![suspending.unwrap_or_else(|| "android".into())];
    }
    u.gids = read_int_array(r)?.unwrap_or_default();
    u.granted_permissions = strings(r)?;
    if r.read_bool()? {
        let allowed = r.read_bool()?;
        u.domain_selection = Some((allowed, host_states(r)?));
    }
    Ok(u)
}

/// A `SharedLibrary` or `SharedLibraryInfo` with its dependencies.
fn shared_library(r: &mut Reader<'_>) -> Result<SharedLibrary> {
    let mut l = SharedLibrary {
        name: string(r)?,
        path: string(r)?,
        package_name: string(r)?,
        code_paths: read_string_list(r)?.map(|v| v.into_iter().flatten().collect()),
        version: r.read_i64()?,
        kind: r.read_i32()?,
        native: r.read_bool()?,
        declaring: (string(r)?.unwrap_or_default(), r.read_i64()?),
        ..SharedLibrary::default()
    };
    for _ in 0..count(r)? {
        l.dependents
            .push((string(r)?.unwrap_or_default(), r.read_i64()?));
    }
    for _ in 0..count(r)? {
        l.dependencies.push(shared_library(r)?);
    }
    Ok(l)
}

/// `SharedUserApi`'s getters.
pub fn shared_user(bytes: &[u8]) -> Result<SharedUser> {
    let r = &mut Reader::new(bytes, &[]);
    Ok(SharedUser {
        name: string(r)?.unwrap_or_default(),
        app_id: r.read_i32()?,
        private_flags: if r.read_bool()? {
            PRIVATE_FLAG_PRIVILEGED
        } else {
            0
        },
        seinfo_target_sdk_version: r.read_i32()?,
        packages: strings(r)?,
        signatures: signing(r)?,
        ..SharedUser::default()
    })
}

/// A user's record: its id, its preferred activities as
/// `getPreferredActivityBackup` writes them, its
/// `package-restrictions.xml` and its default browser.
pub fn user(bytes: &[u8]) -> Result<User> {
    let r = &mut Reader::new(bytes, &[]);
    let id = r.read_i32()?;
    Ok(User {
        id,
        preferred_activities: read_byte_array(r)?,
        restrictions: read_byte_array(r)?,
        default_browser: string(r)?,
        ..User::default()
    })
}

/// The system record: `config_forceSystemPackagesQueryable`,
/// `config_forceQueryablePackages`, and what resolution reads (the
/// resolver activity's theme and titles, the custom resolver, device
/// provisioning, the instant app resolver and installer, whether package
/// query filtering is on).
pub fn system(bytes: &[u8]) -> Result<(bool, Vec<String>, Platform)> {
    let r = &mut Reader::new(bytes, &[]);
    let all = r.read_bool()?;
    let packages = strings(r)?;
    let mut p = Platform {
        resolver_theme: r.read_i32()?,
        ..Platform::default()
    };
    for _ in 0..count(r)? {
        p.resolver_titles.push((string(r)?, r.read_i32()?));
    }
    p.custom_resolver = string(r)?.filter(|s| !s.is_empty());
    p.device_provisioned = r.read_i32()? == 1;
    p.instant_app_resolver = string(r)?;
    p.instant_app_installer = string(r)?;
    p.query_filtering_disabled = !r.read_bool()?;
    Ok((all, packages, p))
}

/// `SigningDetails`: the scheme (-1 for none), the signers, then the past
/// signers with their capabilities.
fn signing(r: &mut Reader<'_>) -> Result<Option<Signatures>> {
    let scheme_version = r.read_i32()?;
    if scheme_version < 0 {
        return Ok(None);
    }
    let current = signatures(r)?.unwrap_or_default();
    Ok(Some(Signatures {
        scheme_version,
        signatures: current.into_iter().map(|(der, _)| der).collect(),
        past_signatures: signatures(r)?,
    }))
}

/// Signers, DER encoded, with their capabilities.
type Signers = Vec<(Vec<u8>, i32)>;

fn signatures(r: &mut Reader<'_>) -> Result<Option<Signers>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    (0..n)
        .map(|_| Ok((read_byte_array(r)?.unwrap_or_default(), r.read_i32()?)))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn host_states(r: &mut Reader<'_>) -> Result<Vec<(String, i32)>> {
    (0..count(r)?)
        .map(|_| Ok((string(r)?.unwrap_or_default(), r.read_i32()?)))
        .collect()
}

fn string(r: &mut Reader<'_>) -> Result<Option<String>> {
    r.read_string16()
}

/// A list's size; none for a null list.
fn count(r: &mut Reader<'_>) -> Result<i32> {
    Ok(r.read_i32()?.max(0))
}

/// A list of strings; empty for a null list.
fn strings(r: &mut Reader<'_>) -> Result<Vec<String>> {
    Ok(read_string_list(r)?
        .unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}
