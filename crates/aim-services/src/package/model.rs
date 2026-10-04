//! The package state the native queries read (M4 slice A,
//! docs/m4-packagemanager.md): the original's in-memory state as its
//! `PackageManagerLocal` snapshots give it (`PackageState`,
//! `PackageUserState`, `SharedUserApi`), with what the info generators
//! read beside it (the permission state's gids and grants, SystemConfig,
//! the device's constants). The feed fills it from the original
//! (`feed.rs`); field names follow the Java getters.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::intent_filter::UriRelativeFilterGroup;
use super::pkg::AndroidPackage;
use super::restrictions::ArchiveState;
use super::settings::{Signatures, UsesSdkLibrary};

/// One version of the state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Incremented by each batch the feed applies.
    pub generation: u64,
    /// The `package_info_cache` nonce read before the batch was asked for.
    pub nonce: Option<i64>,
    /// By package name (`getPackageStates`).
    pub packages: BTreeMap<String, PackageState>,
    /// The system packages an update replaces
    /// (`getDisabledSystemPackageStates`).
    pub disabled_system_packages: BTreeMap<String, PackageState>,
    /// By name.
    pub shared_users: BTreeMap<String, SharedUser>,
    /// Complete scoped runtime owners exported by the original snapshot.
    pub runtime_inputs: BTreeMap<(String, bool), super::scan::OriginalRuntime>,
    /// By user id.
    pub users: BTreeMap<i32, User>,
    pub system: System,
    pub platform: Platform,
}

/// What resolution reads beside the packages: what PackageManager chose
/// at boot and the platform's settings, as the feed gives them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Platform {
    /// The resolver activity's theme (`Theme.Material.Dialog.Alert`).
    pub resolver_theme: i32,
    /// `ResolverActivity.ActionTitle`'s labels (`getLabelRes`): action (`None` for the
    /// default) and string resource.
    pub resolver_titles: Vec<(Option<String>, i32)>,
    /// `config_customResolverActivity`, flattened, if set.
    pub custom_resolver: Option<String>,
    /// `Settings.Global.DEVICE_PROVISIONED`.
    pub device_provisioned: bool,
    /// The instant app resolver and installer PackageManager chose at
    /// boot, flattened.
    pub instant_app_resolver: Option<String>,
    pub instant_app_installer: Option<String>,
    /// AppsFilter's DeviceConfig flag `package_query_filtering_enabled`
    /// is off: no package is filtered.
    pub query_filtering_disabled: bool,
}

/// `PackageState` (`PackageStateInternal`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackageState {
    pub name: String,
    pub app_id: i32,
    /// The shared user's name (`getSharedUser`; `hasSharedUser`).
    pub shared_user: Option<String>,
    /// The code path.
    pub path: String,
    pub volume_uuid: Option<String>,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub cpu_abi_override: Option<String>,
    pub seinfo: Option<String>,
    pub version_code: i64,
    pub target_sdk_version: i32,
    /// `ApplicationInfo.CATEGORY_*`; `CATEGORY_UNDEFINED` without one.
    pub category_override: i32,
    pub hidden_api_enforcement_policy: i32,
    pub last_modified_time: i64,
    pub last_update_time: i64,
    pub restrict_update_hash: Option<Vec<u8>>,
    pub apex_module_name: Option<String>,
    /// MIME groups and their types.
    pub mime_groups: Vec<(Option<String>, Vec<Option<String>>)>,
    /// Static shared libraries used: name and version.
    pub uses_static_libraries: Vec<(String, i64)>,
    pub uses_sdk_libraries: Vec<UsesSdkLibrary>,
    /// `getUsesLibraryFiles`.
    pub uses_library_files: Vec<String>,
    /// `getSharedLibraryDependencies`, in order.
    pub uses_library_infos: Vec<SharedLibrary>,
    pub is: StateFlags,
    /// The signing details (`getSigningInfo`).
    pub signatures: Option<Signatures>,
    pub install_source: InstallSource,
    /// The permissions the package defines that are installed
    /// (`PermissionManagerServiceInternal.getInstalledPermissions`).
    pub installed_permissions: Vec<String>,
    /// The domain verification state (`getDomainVerificationInfo`): the
    /// domain set id and each host's state; `None` without web domains.
    pub domain_verification: Option<(String, Vec<(String, i32)>)>,
    /// Domain verification's URI relative filter groups, by web domain.
    pub uri_relative_filter_groups: Vec<(String, Vec<UriRelativeFilterGroup>)>,
    /// `FILTER_APPLICATION_QUERY` as platform compat gives it for the
    /// package (with its overrides); `None` where no feed gave it, which
    /// leaves the change's own rule (on from the package's target SDK 30).
    pub filter_application_query: Option<bool>,
    /// The authorities the manifest declares for each syncable provider
    /// with several, by provider class (registration leaves the package's
    /// provider only the first).
    pub syncable_authorities: Vec<(String, String)>,
    /// The `AndroidPackage` as `PackageCacher.toCacheEntryStatic` writes
    /// it (the parser cache's format); `None` without code. Shared across
    /// versions while the original cache bytes remain unchanged.
    pub parcel: Option<Arc<[u8]>>,
    /// `parcel`, read (`AndroidPackage::read_cache_entry`).
    pub pkg: Option<Arc<AndroidPackage>>,
    /// By user id.
    pub users: BTreeMap<i32, PackageUserState>,
}

/// `PackageState`'s `is*()` getters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateFlags {
    pub system: bool,
    pub privileged: bool,
    pub oem: bool,
    pub vendor: bool,
    pub product: bool,
    pub system_ext: bool,
    pub odm: bool,
    pub updated_system_app: bool,
    pub apex: bool,
    pub apk_in_updated_apex: bool,
    pub hidden_until_installed: bool,
    pub default_to_device_protected_storage: bool,
    pub force_queryable_override: bool,
    pub scanned_as_stopped_system_app: bool,
    pub update_available: bool,
    pub install_permissions_fixed: bool,
    pub pending_restore: bool,
    pub debuggable: bool,
    pub loading: bool,
}

/// `InstallSource`: unfiltered (`getInstallSourceInfo` filters it by the
/// caller's visibility).
#[derive(Clone, Debug, PartialEq)]
pub struct InstallSource {
    pub installer: Option<String>,
    pub installer_uid: i32,
    pub initiating_package: Option<String>,
    pub originating_package: Option<String>,
    pub update_owner: Option<String>,
    pub installer_attribution_tag: Option<String>,
    pub initiating_package_signatures: Option<Signatures>,
    /// `PackageInstaller.PACKAGE_SOURCE_*`.
    pub package_source: i32,
    pub is_orphaned: bool,
    pub initiating_package_uninstalled: bool,
}

impl Default for InstallSource {
    fn default() -> Self {
        InstallSource {
            installer: None,
            installer_uid: -1,
            initiating_package: None,
            originating_package: None,
            update_owner: None,
            installer_attribution_tag: None,
            initiating_package_signatures: None,
            package_source: 0,
            is_orphaned: false,
            initiating_package_uninstalled: false,
        }
    }
}

/// `SharedLibraryInfo` (`SharedLibrary`): a library a package uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedLibrary {
    pub path: Option<String>,
    pub package_name: Option<String>,
    pub code_paths: Option<Vec<Option<String>>>,
    pub name: Option<String>,
    pub version: i64,
    /// `SharedLibraryInfo.TYPE_*`.
    pub kind: i32,
    pub native: bool,
    /// The declaring package: name and version code.
    pub declaring: (String, i64),
    /// SDK dependency placeholders have no declaring package.
    pub declaring_absent: bool,
    pub dependents: Vec<Option<(String, i64)>>,
    pub dependencies: Vec<Option<SharedLibrary>>,
    pub dependents_initialized: bool,
    pub dependencies_initialized: bool,
    /// Raw Parcelable list and string list; null and allocated-empty are distinct.
    pub optional_dependents: Option<Vec<Option<(String, i64)>>>,
    pub cert_digests: Option<Vec<Option<String>>>,
}

/// `PackageUserState`: one package's state for one user.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageUserState {
    pub ce_data_inode: i64,
    pub de_data_inode: i64,
    pub installed: bool,
    pub stopped: bool,
    pub not_launched: bool,
    pub hidden: bool,
    pub instant_app: bool,
    pub virtual_preload: bool,
    pub quarantined: bool,
    pub distraction_flags: i32,
    /// The suspending packages.
    pub suspended_by: Vec<String>,
    /// `COMPONENT_ENABLED_STATE_*`.
    pub enabled: i32,
    pub last_disable_app_caller: Option<String>,
    pub enabled_components: Vec<String>,
    pub disabled_components: Vec<String>,
    pub install_reason: i32,
    pub uninstall_reason: i32,
    pub harmful_app_warning: Option<String>,
    pub splash_screen_theme: Option<String>,
    pub first_install_time: i64,
    pub min_aspect_ratio: i32,
    pub archive_state: Option<ArchiveState>,
    /// `getAllOverlayPaths`, the shared libraries' merged in.
    pub overlay_paths: Option<OverlayPaths>,
    /// Components' label and icon overrides: class name, label, icon.
    pub component_label_icon_overrides: Vec<(String, Option<String>, Option<i32>)>,
    /// `dataExists`.
    pub data_exists: bool,
    /// The permission state's gids of the package's uid in this user
    /// (`getGidsForUid`).
    pub gids: Vec<i32>,
    /// The permissions granted to the package in this user
    /// (`getGrantedPermissions`).
    pub granted_permissions: Vec<String>,
    /// The user's domain selection (`getDomainVerificationUserState`):
    /// whether link handling is allowed, and each host's state.
    pub domain_selection: Option<(bool, Vec<(String, i32)>)>,
}

impl Default for PackageUserState {
    /// `PackageUserStateDefault`: installed, enabled, not stopped.
    fn default() -> Self {
        PackageUserState {
            ce_data_inode: 0,
            de_data_inode: 0,
            installed: true,
            stopped: false,
            not_launched: false,
            hidden: false,
            instant_app: false,
            virtual_preload: false,
            quarantined: false,
            distraction_flags: 0,
            suspended_by: Vec::new(),
            enabled: 0,
            last_disable_app_caller: None,
            enabled_components: Vec::new(),
            disabled_components: Vec::new(),
            install_reason: 0,
            uninstall_reason: 0,
            harmful_app_warning: None,
            splash_screen_theme: None,
            first_install_time: 0,
            min_aspect_ratio: 0,
            archive_state: None,
            overlay_paths: None,
            component_label_icon_overrides: Vec::new(),
            data_exists: true,
            gids: Vec::new(),
            granted_permissions: Vec::new(),
            domain_selection: None,
        }
    }
}

/// `OverlayPaths`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayPaths {
    pub resource_dirs: Vec<String>,
    pub overlay_paths: Vec<String>,
}

/// `SharedUserApi`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedUser {
    pub name: String,
    pub app_id: i32,
    /// `ApplicationInfo.FLAG_*` and `PRIVATE_FLAG_*` of the shared user.
    pub flags: i32,
    pub private_flags: i32,
    pub seinfo_target_sdk_version: i32,
    /// Its packages' names.
    pub packages: Vec<String>,
    pub signatures: Option<Signatures>,
}

/// A user as PackageManager sees it (`UserManagerService`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct User {
    pub id: i32,
    /// `UserInfo.FLAG_*`.
    pub flags: i32,
    /// `UserInfo.profileGroupId`.
    pub profile_group_id: i32,
    /// `isUserUnlockingOrUnlocked`, which decides the direct boot match
    /// flags a query gets.
    pub unlocking_or_unlocked: bool,
    /// The preferred activities as `getPreferredActivityBackup` writes
    /// them (`<pa>`, in full); `None` without any.
    pub preferred_activities: Option<Vec<u8>>,
    /// `package-restrictions.xml` as the original last wrote it: the
    /// persistent preferred activities and cross-profile filters, which
    /// no API reads (#715).
    pub restrictions: Option<Vec<u8>>,
    /// The browser role's holder (`DefaultAppProvider.getDefaultBrowser`).
    pub default_browser: Option<String>,
}

/// SystemConfig and the device's constants the info generators read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct System {
    /// Native AppsFilter interaction grants, carried with each query snapshot.
    pub implicit_access: super::apps_filter::ImplicitAccess,
    /// `mAvailableFeatures`: name and version.
    pub features: Vec<(String, i32)>,
    /// `ro.opengles.version`, the `reqGlEsVersion` feature.
    pub gl_es_version: i32,
    /// `SystemConfig.getHiddenApiWhitelistedApps`.
    pub hidden_api_allowlist: Vec<String>,
    /// `ParsingPackageUtils.sUseRoundIcon`.
    pub use_round_icon: bool,
    /// `ParsingPackageUtils.sCompatibilityModeEnabled`.
    pub compatibility_mode: bool,
    /// `FallbackCategoryProvider`'s categories, by package.
    pub fallback_categories: Vec<(String, i32)>,
    /// The aconfig flags the generators read, by full name.
    pub flags: Vec<(String, bool)>,
    /// Isolated uids and their owners' (`mIsolatedOwners`).
    pub isolated_owners: Vec<(i32, i32)>,
    /// `config_forceSystemPackagesQueryable`.
    pub force_system_packages_queryable: bool,
    /// `config_forceQueryablePackages`.
    pub force_queryable_packages: Vec<String>,
    /// SystemConfig's named actors: namespace, actor name and package.
    pub named_actors: Vec<(String, String, String)>,
}
