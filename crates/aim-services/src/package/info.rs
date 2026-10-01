//! What PackageManager answers about a package: `PackageInfoUtils`'
//! generators (a `PackageInfo`, an `ApplicationInfo`, the component infos
//! and permission infos, all per user and by the query's flags) and
//! `PackageUserStateUtils`' matching, over the model's state, with the
//! parcels as their `writeToParcel` write them at `android-16.0.0_r1`.
//!
//! `ApplicationInfo.createTimestamp` (`SystemClock.uptimeMillis()` when
//! the original made the object) is written as 0: no two replies share
//! it, and the comparison leaves it out.

use std::sync::Arc;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use super::intent_filter::PatternMatcher;
use super::model::{PackageState, PackageUserState, SharedLibrary, System};
use super::pkg::{
    self, AndroidPackage, Component, MainComponent, MetaData, SplitDependencies, Value,
    WindowLayout, booleans, booleans2,
};

/// `PackageManager`'s query flags.
pub mod flags {
    pub const GET_ACTIVITIES: i64 = 0x1;
    pub const GET_RECEIVERS: i64 = 0x2;
    pub const GET_SERVICES: i64 = 0x4;
    pub const GET_PROVIDERS: i64 = 0x8;
    pub const GET_INSTRUMENTATION: i64 = 0x10;
    pub const GET_SIGNATURES: i64 = 0x40;
    pub const GET_META_DATA: i64 = 0x80;
    pub const GET_GIDS: i64 = 0x100;
    pub const MATCH_DISABLED_COMPONENTS: i64 = 0x200;
    pub const GET_SHARED_LIBRARY_FILES: i64 = 0x400;
    pub const GET_URI_PERMISSION_PATTERNS: i64 = 0x800;
    pub const GET_PERMISSIONS: i64 = 0x1000;
    pub const MATCH_UNINSTALLED_PACKAGES: i64 = 0x2000;
    pub const GET_CONFIGURATIONS: i64 = 0x4000;
    pub const MATCH_DISABLED_UNTIL_USED_COMPONENTS: i64 = 0x8000;
    pub const MATCH_DIRECT_BOOT_UNAWARE: i64 = 0x40000;
    pub const MATCH_DIRECT_BOOT_AWARE: i64 = 0x80000;
    pub const MATCH_SYSTEM_ONLY: i64 = 0x100000;
    pub const MATCH_FACTORY_ONLY: i64 = 0x200000;
    pub const MATCH_ANY_USER: i64 = 0x400000;
    pub const MATCH_KNOWN_PACKAGES: i64 = MATCH_UNINSTALLED_PACKAGES | MATCH_ANY_USER;
    pub const MATCH_INSTANT: i64 = 0x800000;
    pub const MATCH_STATIC_SHARED_AND_SDK_LIBRARIES: i64 = 0x4000000;
    pub const GET_SIGNING_CERTIFICATES: i64 = 0x8000000;
    pub const MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS: i64 = 0x20000000;
    pub const MATCH_APEX: i64 = 0x40000000;
    pub const GET_ATTRIBUTIONS_LONG: i64 = 0x80000000;
    pub const MATCH_ARCHIVED_PACKAGES: i64 = 1 << 32;
    pub const MATCH_QUARANTINED_COMPONENTS: i64 = 1 << 33;
}
use flags::*;

/// `PackageManager.COMPONENT_ENABLED_STATE_*`.
pub const COMPONENT_ENABLED_STATE_DEFAULT: i32 = 0;
pub const COMPONENT_ENABLED_STATE_ENABLED: i32 = 1;
pub const COMPONENT_ENABLED_STATE_DISABLED: i32 = 2;
pub const COMPONENT_ENABLED_STATE_DISABLED_USER: i32 = 3;
pub const COMPONENT_ENABLED_STATE_DISABLED_UNTIL_USED: i32 = 4;

/// `ApplicationInfo.FLAG_*`.
pub const FLAG_SYSTEM: i32 = 1;
const FLAG_DEBUGGABLE: i32 = 1 << 1;
const FLAG_HAS_CODE: i32 = 1 << 2;
const FLAG_PERSISTENT: i32 = 1 << 3;
const FLAG_FACTORY_TEST: i32 = 1 << 4;
const FLAG_ALLOW_TASK_REPARENTING: i32 = 1 << 5;
const FLAG_ALLOW_CLEAR_USER_DATA: i32 = 1 << 6;
pub const FLAG_UPDATED_SYSTEM_APP: i32 = 1 << 7;
const FLAG_TEST_ONLY: i32 = 1 << 8;
const FLAG_SUPPORTS_SMALL_SCREENS: i32 = 1 << 9;
const FLAG_SUPPORTS_NORMAL_SCREENS: i32 = 1 << 10;
const FLAG_SUPPORTS_LARGE_SCREENS: i32 = 1 << 11;
const FLAG_RESIZEABLE_FOR_SCREENS: i32 = 1 << 12;
const FLAG_SUPPORTS_SCREEN_DENSITIES: i32 = 1 << 13;
const FLAG_VM_SAFE_MODE: i32 = 1 << 14;
const FLAG_ALLOW_BACKUP: i32 = 1 << 15;
const FLAG_KILL_AFTER_RESTORE: i32 = 1 << 16;
const FLAG_RESTORE_ANY_VERSION: i32 = 1 << 17;
const FLAG_EXTERNAL_STORAGE: i32 = 1 << 18;
const FLAG_SUPPORTS_XLARGE_SCREENS: i32 = 1 << 19;
const FLAG_LARGE_HEAP: i32 = 1 << 20;
const FLAG_STOPPED: i32 = 1 << 21;
const FLAG_SUPPORTS_RTL: i32 = 1 << 22;
const FLAG_INSTALLED: i32 = 1 << 23;
const FLAG_IS_GAME: i32 = 1 << 25;
const FLAG_FULL_BACKUP_ONLY: i32 = 1 << 26;
const FLAG_USES_CLEARTEXT_TRAFFIC: i32 = 1 << 27;
const FLAG_EXTRACT_NATIVE_LIBS: i32 = 1 << 28;
const FLAG_HARDWARE_ACCELERATED: i32 = 1 << 29;
const FLAG_SUSPENDED: i32 = 1 << 30;
const FLAG_MULTIARCH: i32 = 1 << 31;

/// `ApplicationInfo.PRIVATE_FLAG_*`.
const PRIVATE_FLAG_HIDDEN: i32 = 1;
const PRIVATE_FLAG_CANT_SAVE_STATE: i32 = 1 << 1;
pub const PRIVATE_FLAG_PRIVILEGED: i32 = 1 << 3;
const PRIVATE_FLAG_HAS_DOMAIN_URLS: i32 = 1 << 4;
const PRIVATE_FLAG_DEFAULT_TO_DEVICE_PROTECTED_STORAGE: i32 = 1 << 5;
const PRIVATE_FLAG_DIRECT_BOOT_AWARE: i32 = 1 << 6;
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;
const PRIVATE_FLAG_PARTIALLY_DIRECT_BOOT_AWARE: i32 = 1 << 8;
const PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_RESIZEABLE: i32 = 1 << 10;
const PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_UNRESIZEABLE: i32 = 1 << 11;
const PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION: i32 = 1 << 12;
const PRIVATE_FLAG_BACKUP_IN_FOREGROUND: i32 = 1 << 13;
const PRIVATE_FLAG_STATIC_SHARED_LIBRARY: i32 = 1 << 14;
const PRIVATE_FLAG_ISOLATED_SPLIT_LOADING: i32 = 1 << 15;
const PRIVATE_FLAG_VIRTUAL_PRELOAD: i32 = 1 << 16;
const PRIVATE_FLAG_OEM: i32 = 1 << 17;
const PRIVATE_FLAG_VENDOR: i32 = 1 << 18;
const PRIVATE_FLAG_PRODUCT: i32 = 1 << 19;
const PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY: i32 = 1 << 20;
const PRIVATE_FLAG_SYSTEM_EXT: i32 = 1 << 21;
const PRIVATE_FLAG_USES_NON_SDK_API: i32 = 1 << 22;
const PRIVATE_FLAG_HAS_FRAGILE_USER_DATA: i32 = 1 << 24;
const PRIVATE_FLAG_ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE: i32 = 1 << 26;
const PRIVATE_FLAG_ALLOW_AUDIO_PLAYBACK_CAPTURE: i32 = 1 << 27;
const PRIVATE_FLAG_REQUEST_LEGACY_EXTERNAL_STORAGE: i32 = 1 << 29;
const PRIVATE_FLAG_ODM: i32 = 1 << 30;
const PRIVATE_FLAG_ALLOW_NATIVE_HEAP_POINTER_TAGGING: i32 = 1 << 31;
const PRIVATE_FLAG_USE_EMBEDDED_DEX: i32 = 1 << 25;
const PRIVATE_FLAG_IS_RESOURCE_OVERLAY: i32 = 1 << 28;
const PRIVATE_FLAG_PROFILEABLE_BY_SHELL: i32 = 1 << 23;

/// `ApplicationInfo.PRIVATE_FLAG_EXT_*`.
const PRIVATE_FLAG_EXT_PROFILEABLE: i32 = 1;
const PRIVATE_FLAG_EXT_REQUEST_FOREGROUND_SERVICE_EXEMPTION: i32 = 1 << 1;
const PRIVATE_FLAG_EXT_ATTRIBUTIONS_ARE_USER_VISIBLE: i32 = 1 << 2;
const PRIVATE_FLAG_EXT_ENABLE_ON_BACK_INVOKED_CALLBACK: i32 = 1 << 3;
const PRIVATE_FLAG_EXT_ALLOWLISTED_FOR_HIDDEN_APIS: i32 = 1 << 4;
const PRIVATE_FLAG_EXT_CPU_OVERRIDE: i32 = 1 << 5;
const PRIVATE_FLAG_EXT_NOT_LAUNCHED: i32 = 1 << 6;

/// `ApplicationInfo.CATEGORY_UNDEFINED`.
pub const CATEGORY_UNDEFINED: i32 = -1;
/// `UserHandle.PER_USER_RANGE`.
pub const PER_USER_RANGE: i32 = 100_000;
/// `UserHandle.USER_SYSTEM`, `USER_NULL`.
pub const USER_SYSTEM: i32 = 0;
const USER_NULL: i32 = -10000;
/// `PackageInfo.REQUESTED_PERMISSION_*`.
const REQUESTED_PERMISSION_REQUIRED: i32 = 0x1;
const REQUESTED_PERMISSION_GRANTED: i32 = 0x2;
const REQUESTED_PERMISSION_IMPLICIT: i32 = 0x4;
const REQUESTED_PERMISSION_NEVER_FOR_LOCATION: i32 = 0x10000;
/// `ParsedUsesPermission.FLAG_NEVER_FOR_LOCATION`.
const USES_PERMISSION_NEVER_FOR_LOCATION: i32 = 0x10000;
/// `PermissionInfo.FLAG_INSTALLED`.
const PERMISSION_FLAG_INSTALLED: i32 = 1 << 30;
/// `PackageManager.APP_DETAILS_ACTIVITY_CLASS_NAME`.
const APP_DETAILS_ACTIVITY_CLASS_NAME: &str = "android.app.AppDetailsActivity";
/// `SELinuxUtil`'s suffixes.
const SEINFO_COMPLETE: &str = ":complete";
const SEINFO_INSTANT_APP: &str = ":ephemeralapp";
/// `StorageManager`'s volume UUIDs (`convert`).
const UUID_DEFAULT: (i64, i64) = (0x4121_7664_9172_527a, 0xb3d5_edab_b50a_7d69_u64 as i64);
const UUID_PRIMARY_PHYSICAL: (i64, i64) = (0x0f95_a519_dae7_5abf, 0x9519_fbd6_209e_05fd_u64 as i64);
const UUID_SYSTEM: (i64, i64) = (0x5d25_8386_e60d_59e3, 0x826d_0089_cdd4_2cc0_u64 as i64);
/// The aconfig flag `android.content.pm.nullable_data_dir`.
pub const NULLABLE_DATA_DIR: &str = "android.content.pm.nullable_data_dir";

/// `UserHandle.getUid`.
pub fn uid(user: i32, app_id: i32) -> i32 {
    user * PER_USER_RANGE + app_id.rem_euclid(PER_USER_RANGE)
}

/// `String.hashCode`.
pub fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// The order an `ArraySet` or `ArrayMap` keeps its keys in: by hash,
/// then as added.
pub fn array_order<T>(mut items: Vec<T>, key: impl Fn(&T) -> &str) -> Vec<T> {
    items.sort_by_key(|i| java_hash(key(i)));
    items
}

/// `getUserStateOrDefault`.
pub fn user_state(ps: &PackageState, user: i32) -> PackageUserState {
    ps.users.get(&user).cloned().unwrap_or_default()
}

fn flag(set: bool, flag: i32) -> i32 {
    if set { flag } else { 0 }
}

/// `PackageItemInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageItemInfo {
    pub name: Option<String>,
    pub package_name: Option<String>,
    pub label_res: i32,
    pub non_localized_label: Option<String>,
    pub icon: i32,
    pub logo: i32,
    pub meta_data: Option<MetaData>,
    pub banner: i32,
    pub show_user_icon: i32,
    pub is_archived: bool,
}

impl Default for PackageItemInfo {
    fn default() -> Self {
        PackageItemInfo {
            name: None,
            package_name: None,
            label_res: 0,
            non_localized_label: None,
            icon: 0,
            logo: 0,
            meta_data: None,
            banner: 0,
            show_user_icon: USER_NULL,
            is_archived: false,
        }
    }
}

impl PackageItemInfo {
    fn write(&self, p: &mut Parcel) {
        p.write_string8(self.name.as_deref());
        p.write_string8(self.package_name.as_deref());
        p.write_i32(self.label_res);
        write_char_sequence(p, self.non_localized_label.as_deref());
        p.write_i32(self.icon);
        p.write_i32(self.logo);
        write_bundle(p, self.meta_data.as_ref());
        p.write_i32(self.banner);
        p.write_i32(self.show_user_icon);
        p.write_bool(self.is_archived);
    }

    /// `assignFieldsPackageItemInfoParsedComponent`, with the user's label
    /// and icon override (`ParsedComponentStateUtils`).
    fn of(c: &Component, state: &PackageUserState) -> PackageItemInfo {
        let mut item = PackageItemInfo {
            non_localized_label: c.non_localized_label.clone(),
            icon: c.icon,
            banner: c.banner,
            label_res: c.label_res,
            logo: c.logo,
            name: Some(c.name.clone()),
            package_name: Some(c.package_name.clone()),
            ..PackageItemInfo::default()
        };
        if let Some((_, label, icon)) = state
            .component_label_icon_overrides
            .iter()
            .find(|(class, _, _)| *class == c.name)
        {
            if label.is_some() {
                item.non_localized_label = label.clone();
            }
            if let Some(icon) = icon {
                item.icon = *icon;
            }
        }
        item
    }
}

/// `ApplicationInfo`; `max_aspect_ratio` and `min_aspect_ratio` are not
/// parceled.
#[derive(Clone, Debug, PartialEq)]
pub struct ApplicationInfo {
    pub item: PackageItemInfo,
    pub task_affinity: Option<String>,
    pub permission: Option<String>,
    pub process_name: Option<String>,
    pub class_name: Option<String>,
    pub theme: i32,
    pub flags: i32,
    pub private_flags: i32,
    pub private_flags_ext: i32,
    pub requires_smallest_width_dp: i32,
    pub compatible_width_limit_dp: i32,
    pub largest_width_limit_dp: i32,
    pub volume_uuid: Option<String>,
    pub scan_source_dir: Option<String>,
    pub scan_public_source_dir: Option<String>,
    pub source_dir: Option<String>,
    pub public_source_dir: Option<String>,
    pub split_names: Option<Vec<Option<String>>>,
    pub split_source_dirs: Option<Vec<Option<String>>>,
    pub split_public_source_dirs: Option<Vec<Option<String>>>,
    pub split_dependencies: Option<SplitDependencies>,
    pub native_library_dir: Option<String>,
    pub secondary_native_library_dir: Option<String>,
    pub native_library_root_dir: Option<String>,
    pub native_library_root_requires_isa: bool,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub resource_dirs: Option<Vec<String>>,
    pub overlay_paths: Option<Vec<String>>,
    pub se_info: Option<String>,
    pub se_info_user: Option<String>,
    pub shared_library_files: Option<Vec<String>>,
    pub shared_library_infos: Option<Vec<SharedLibrary>>,
    pub optional_shared_library_infos: Option<Vec<SharedLibrary>>,
    pub data_dir: Option<String>,
    pub device_protected_data_dir: Option<String>,
    pub credential_protected_data_dir: Option<String>,
    pub uid: i32,
    pub min_sdk_version: i32,
    pub target_sdk_version: i32,
    pub long_version_code: i64,
    pub enabled: bool,
    pub enabled_setting: i32,
    pub install_location: i32,
    pub manage_space_activity_name: Option<String>,
    pub backup_agent_name: Option<String>,
    pub description_res: i32,
    pub ui_options: i32,
    pub full_backup_content: i32,
    pub data_extraction_rules_res: i32,
    pub cross_profile: bool,
    pub network_security_config_res: i32,
    pub category: i32,
    pub target_sandbox_version: i32,
    pub class_loader_name: Option<String>,
    pub split_class_loader_names: Option<Vec<Option<String>>>,
    pub compile_sdk_version: i32,
    pub compile_sdk_version_codename: Option<String>,
    pub app_component_factory: Option<String>,
    pub icon_res: i32,
    pub round_icon_res: i32,
    pub hidden_api_policy: i32,
    pub hidden_until_installed: bool,
    pub zygote_preload_name: Option<String>,
    pub gwp_asan_mode: i32,
    pub memtag_mode: i32,
    pub native_heap_zero_initialized: i32,
    pub request_raw_external_storage_access: Option<bool>,
    pub app_class_names_by_process: Option<Vec<(String, String)>>,
    pub locale_config_res: i32,
    pub allow_cross_uid_activity_switch_from_below: bool,
    pub page_size_app_compat_flags: i32,
    pub known_activity_embedding_certs: Option<Vec<String>>,
}

impl Default for ApplicationInfo {
    /// `new ApplicationInfo()`.
    fn default() -> Self {
        ApplicationInfo {
            item: PackageItemInfo::default(),
            task_affinity: None,
            permission: None,
            process_name: None,
            class_name: None,
            theme: 0,
            flags: 0,
            private_flags: 0,
            private_flags_ext: 0,
            requires_smallest_width_dp: 0,
            compatible_width_limit_dp: 0,
            largest_width_limit_dp: 0,
            volume_uuid: None,
            scan_source_dir: None,
            scan_public_source_dir: None,
            source_dir: None,
            public_source_dir: None,
            split_names: None,
            split_source_dirs: None,
            split_public_source_dirs: None,
            split_dependencies: None,
            native_library_dir: None,
            secondary_native_library_dir: None,
            native_library_root_dir: None,
            native_library_root_requires_isa: false,
            primary_cpu_abi: None,
            secondary_cpu_abi: None,
            resource_dirs: None,
            overlay_paths: None,
            se_info: None,
            se_info_user: None,
            shared_library_files: None,
            shared_library_infos: None,
            optional_shared_library_infos: None,
            data_dir: None,
            device_protected_data_dir: None,
            credential_protected_data_dir: None,
            uid: 0,
            min_sdk_version: 0,
            target_sdk_version: 0,
            long_version_code: 0,
            enabled: true,
            enabled_setting: COMPONENT_ENABLED_STATE_DEFAULT,
            install_location: -1,
            manage_space_activity_name: None,
            backup_agent_name: None,
            description_res: 0,
            ui_options: 0,
            full_backup_content: 0,
            data_extraction_rules_res: 0,
            cross_profile: false,
            network_security_config_res: 0,
            category: CATEGORY_UNDEFINED,
            target_sandbox_version: 0,
            class_loader_name: None,
            split_class_loader_names: None,
            compile_sdk_version: 0,
            compile_sdk_version_codename: None,
            app_component_factory: None,
            icon_res: 0,
            round_icon_res: 0,
            hidden_api_policy: -1,
            hidden_until_installed: false,
            zygote_preload_name: None,
            gwp_asan_mode: -1,
            memtag_mode: -1,
            native_heap_zero_initialized: -1,
            request_raw_external_storage_access: None,
            app_class_names_by_process: None,
            locale_config_res: 0,
            allow_cross_uid_activity_switch_from_below: true,
            page_size_app_compat_flags: 0,
            known_activity_embedding_certs: None,
        }
    }
}

/// Where a parcel that allows squashing (`Parcel.allowSquashing`, which
/// `PackageInfo.writeToParcel` turns on) wrote its `ApplicationInfo`:
/// later writes of the same object are an offset back to it.
#[derive(Default)]
pub struct Squash(Option<usize>);

impl ApplicationInfo {
    /// `writeToParcel`, with `Parcel.maybeWriteSquashed` before it.
    pub fn write(&self, p: &mut Parcel, squash: Option<&mut Squash>) {
        match squash {
            Some(Squash(Some(first))) => {
                let at = p.position();
                p.write_i32((at - *first + 4) as i32);
                return;
            }
            Some(Squash(first)) => {
                p.write_i32(0);
                *first = Some(p.position());
            }
            None => p.write_i32(0),
        }
        self.item.write(p);
        p.write_string8(self.task_affinity.as_deref());
        p.write_string8(self.permission.as_deref());
        p.write_string8(self.process_name.as_deref());
        p.write_string8(self.class_name.as_deref());
        p.write_i32(self.theme);
        p.write_i32(self.flags);
        p.write_i32(self.private_flags);
        p.write_i32(self.private_flags_ext);
        p.write_i32(self.requires_smallest_width_dp);
        p.write_i32(self.compatible_width_limit_dp);
        p.write_i32(self.largest_width_limit_dp);
        let (msb, lsb) = storage_uuid(self.volume_uuid.as_deref());
        p.write_i32(1);
        p.write_i64(msb);
        p.write_i64(lsb);
        p.write_string8(self.scan_source_dir.as_deref());
        p.write_string8(self.scan_public_source_dir.as_deref());
        p.write_string8(self.source_dir.as_deref());
        p.write_string8(self.public_source_dir.as_deref());
        write_string8_array(p, self.split_names.as_deref());
        write_string8_array(p, self.split_source_dirs.as_deref());
        write_string8_array(p, self.split_public_source_dirs.as_deref());
        write_sparse_int_arrays(p, self.split_dependencies.as_ref());
        p.write_string8(self.native_library_dir.as_deref());
        p.write_string8(self.secondary_native_library_dir.as_deref());
        p.write_string8(self.native_library_root_dir.as_deref());
        p.write_bool(self.native_library_root_requires_isa);
        p.write_string8(self.primary_cpu_abi.as_deref());
        p.write_string8(self.secondary_cpu_abi.as_deref());
        write_strings8(p, self.resource_dirs.as_deref());
        write_strings8(p, self.overlay_paths.as_deref());
        p.write_string8(self.se_info.as_deref());
        p.write_string8(self.se_info_user.as_deref());
        write_strings8(p, self.shared_library_files.as_deref());
        write_libraries(p, self.shared_library_infos.as_deref());
        write_libraries(p, self.optional_shared_library_infos.as_deref());
        p.write_string8(self.data_dir.as_deref());
        p.write_string8(self.device_protected_data_dir.as_deref());
        p.write_string8(self.credential_protected_data_dir.as_deref());
        p.write_i32(self.uid);
        p.write_i32(self.min_sdk_version);
        p.write_i32(self.target_sdk_version);
        p.write_i64(self.long_version_code);
        p.write_bool(self.enabled);
        p.write_i32(self.enabled_setting);
        p.write_i32(self.install_location);
        p.write_string8(self.manage_space_activity_name.as_deref());
        p.write_string8(self.backup_agent_name.as_deref());
        p.write_i32(self.description_res);
        p.write_i32(self.ui_options);
        p.write_i32(self.full_backup_content);
        p.write_i32(self.data_extraction_rules_res);
        p.write_bool(self.cross_profile);
        p.write_i32(self.network_security_config_res);
        p.write_i32(self.category);
        p.write_i32(self.target_sandbox_version);
        p.write_string8(self.class_loader_name.as_deref());
        write_string8_array(p, self.split_class_loader_names.as_deref());
        p.write_i32(self.compile_sdk_version);
        p.write_string8(self.compile_sdk_version_codename.as_deref());
        p.write_string8(self.app_component_factory.as_deref());
        p.write_i32(self.icon_res);
        p.write_i32(self.round_icon_res);
        p.write_i32(self.hidden_api_policy);
        p.write_bool(self.hidden_until_installed);
        p.write_string8(self.zygote_preload_name.as_deref());
        p.write_i32(self.gwp_asan_mode);
        p.write_i32(self.memtag_mode);
        p.write_i32(self.native_heap_zero_initialized);
        write_for_boolean(p, self.request_raw_external_storage_access);
        // createTimestamp.
        p.write_i64(0);
        match &self.app_class_names_by_process {
            None => p.write_i32(0),
            Some(map) => {
                p.write_i32(map.len() as i32);
                for (process, class) in map {
                    p.write_string16(Some(process));
                    p.write_string16(Some(class));
                }
            }
        }
        p.write_i32(self.locale_config_res);
        p.write_bool(self.allow_cross_uid_activity_switch_from_below);
        p.write_i32(self.page_size_app_compat_flags);
        write_string_set(p, self.known_activity_embedding_certs.as_deref());
    }

    pub fn is_system_app(&self) -> bool {
        self.flags & FLAG_SYSTEM != 0
    }
}

impl WriteParcelable for ApplicationInfo {
    fn write_to(&self, p: &mut Parcel) {
        self.write(p, None);
    }
}

/// `ComponentInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentInfo {
    pub item: PackageItemInfo,
    pub application_info: Arc<ApplicationInfo>,
    pub process_name: Option<String>,
    pub split_name: Option<String>,
    pub attribution_tags: Option<Vec<Option<String>>>,
    pub description_res: i32,
    pub enabled: bool,
    pub exported: bool,
    pub direct_boot_aware: bool,
}

impl ComponentInfo {
    /// `assignFieldsComponentInfoParsedMainComponent`.
    fn of(c: &MainComponent, state: &PackageUserState, app: Arc<ApplicationInfo>) -> Self {
        ComponentInfo {
            item: PackageItemInfo::of(&c.component, state),
            application_info: app,
            process_name: c.process_name.clone(),
            split_name: c.split_name.clone(),
            attribution_tags: c.attribution_tags.clone(),
            description_res: c.component.description_res,
            enabled: c.enabled,
            exported: c.exported,
            direct_boot_aware: c.direct_boot_aware,
        }
    }

    fn write(&self, p: &mut Parcel, squash: Option<&mut Squash>) {
        self.item.write(p);
        self.application_info.write(p, squash);
        p.write_string8(self.process_name.as_deref());
        p.write_string8(self.split_name.as_deref());
        write_string8_array(p, self.attribution_tags.as_deref());
        p.write_i32(self.description_res);
        p.write_bool(self.enabled);
        p.write_bool(self.exported);
        p.write_bool(self.direct_boot_aware);
    }
}

/// `ActivityInfo`: an activity's or a receiver's.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityInfo {
    pub info: ComponentInfo,
    pub theme: i32,
    pub launch_mode: i32,
    pub document_launch_mode: i32,
    pub permission: Option<String>,
    pub task_affinity: Option<String>,
    pub target_activity: Option<String>,
    pub flags: i32,
    pub private_flags: i32,
    pub screen_orientation: i32,
    pub config_changes: i32,
    pub soft_input_mode: i32,
    pub ui_options: i32,
    pub parent_activity_name: Option<String>,
    pub persistable_mode: i32,
    pub max_recents: i32,
    pub lock_task_launch_mode: i32,
    pub window_layout: Option<WindowLayout>,
    pub resize_mode: i32,
    pub requested_vr_component: Option<String>,
    pub rotation_animation: i32,
    pub color_mode: i32,
    pub max_aspect_ratio: f32,
    pub min_aspect_ratio: f32,
    pub supports_size_changes: bool,
    pub known_activity_embedding_certs: Option<Vec<String>>,
    pub required_display_category: Option<String>,
    pub require_content_uri_permission_from_caller: i32,
}

impl ActivityInfo {
    pub fn write(&self, p: &mut Parcel, squash: Option<&mut Squash>) {
        self.info.write(p, squash);
        p.write_i32(self.theme);
        p.write_i32(self.launch_mode);
        p.write_i32(self.document_launch_mode);
        p.write_string8(self.permission.as_deref());
        p.write_string8(self.task_affinity.as_deref());
        p.write_string8(self.target_activity.as_deref());
        // launchToken.
        p.write_string8(None);
        p.write_i32(self.flags);
        p.write_i32(self.private_flags);
        p.write_i32(self.screen_orientation);
        p.write_i32(self.config_changes);
        p.write_i32(self.soft_input_mode);
        p.write_i32(self.ui_options);
        p.write_string8(self.parent_activity_name.as_deref());
        p.write_i32(self.persistable_mode);
        p.write_i32(self.max_recents);
        p.write_i32(self.lock_task_launch_mode);
        match &self.window_layout {
            Some(w) => {
                p.write_i32(1);
                p.write_i32(w.width);
                p.write_f32(w.width_fraction);
                p.write_i32(w.height);
                p.write_f32(w.height_fraction);
                p.write_i32(w.gravity);
                p.write_i32(w.min_width);
                p.write_i32(w.min_height);
                p.write_string8(w.affinity.as_deref());
            }
            None => p.write_i32(0),
        }
        p.write_i32(self.resize_mode);
        p.write_string8(self.requested_vr_component.as_deref());
        p.write_i32(self.rotation_animation);
        p.write_i32(self.color_mode);
        p.write_f32(self.max_aspect_ratio);
        p.write_f32(self.min_aspect_ratio);
        p.write_bool(self.supports_size_changes);
        write_string_set(p, self.known_activity_embedding_certs.as_deref());
        p.write_string8(self.required_display_category.as_deref());
        p.write_i32(self.require_content_uri_permission_from_caller);
    }
}

impl WriteParcelable for ActivityInfo {
    fn write_to(&self, p: &mut Parcel) {
        self.write(p, None);
    }
}

/// `ServiceInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceInfo {
    pub info: ComponentInfo,
    pub permission: Option<String>,
    pub flags: i32,
    pub foreground_service_type: i32,
}

impl ServiceInfo {
    pub fn write(&self, p: &mut Parcel, squash: Option<&mut Squash>) {
        self.info.write(p, squash);
        p.write_string8(self.permission.as_deref());
        p.write_i32(self.flags);
        p.write_i32(self.foreground_service_type);
    }
}

impl WriteParcelable for ServiceInfo {
    fn write_to(&self, p: &mut Parcel) {
        self.write(p, None);
    }
}

/// `ProviderInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderInfo {
    pub info: ComponentInfo,
    pub authority: Option<String>,
    pub read_permission: Option<String>,
    pub write_permission: Option<String>,
    pub grant_uri_permissions: bool,
    pub force_uri_permissions: bool,
    pub uri_permission_patterns: Option<Vec<PatternMatcher>>,
    pub path_permissions: Option<Vec<pkg::PathPermission>>,
    pub multiprocess: bool,
    pub init_order: i32,
    pub flags: i32,
    pub is_syncable: bool,
}

impl ProviderInfo {
    pub fn write(&self, p: &mut Parcel, squash: Option<&mut Squash>) {
        self.info.write(p, squash);
        p.write_string8(self.authority.as_deref());
        p.write_string8(self.read_permission.as_deref());
        p.write_string8(self.write_permission.as_deref());
        p.write_bool(self.grant_uri_permissions);
        p.write_bool(self.force_uri_permissions);
        write_typed_array(p, self.uri_permission_patterns.as_deref(), |p, m| {
            m.write(p)
        });
        write_typed_array(p, self.path_permissions.as_deref(), |p, m| {
            m.pattern.write(p);
            p.write_string16(m.read_permission.as_deref());
            p.write_string16(m.write_permission.as_deref());
        });
        p.write_bool(self.multiprocess);
        p.write_i32(self.init_order);
        p.write_i32(self.flags);
        p.write_bool(self.is_syncable);
    }
}

impl WriteParcelable for ProviderInfo {
    fn write_to(&self, p: &mut Parcel) {
        self.write(p, None);
    }
}

/// `InstrumentationInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentationInfo {
    pub item: PackageItemInfo,
    pub target_package: Option<String>,
    pub target_processes: Option<String>,
    pub source_dir: Option<String>,
    pub public_source_dir: Option<String>,
    pub split_names: Option<Vec<Option<String>>>,
    pub split_source_dirs: Option<Vec<Option<String>>>,
    pub split_public_source_dirs: Option<Vec<Option<String>>>,
    pub split_dependencies: Option<SplitDependencies>,
    pub data_dir: Option<String>,
    pub device_protected_data_dir: Option<String>,
    pub credential_protected_data_dir: Option<String>,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub native_library_dir: Option<String>,
    pub secondary_native_library_dir: Option<String>,
    pub handle_profiling: bool,
    pub functional_test: bool,
}

impl InstrumentationInfo {
    pub fn write(&self, p: &mut Parcel) {
        self.item.write(p);
        p.write_string8(self.target_package.as_deref());
        p.write_string8(self.target_processes.as_deref());
        p.write_string8(self.source_dir.as_deref());
        p.write_string8(self.public_source_dir.as_deref());
        write_string8_array(p, self.split_names.as_deref());
        write_string8_array(p, self.split_source_dirs.as_deref());
        write_string8_array(p, self.split_public_source_dirs.as_deref());
        write_sparse_int_arrays(p, self.split_dependencies.as_ref());
        p.write_string8(self.data_dir.as_deref());
        p.write_string8(self.device_protected_data_dir.as_deref());
        p.write_string8(self.credential_protected_data_dir.as_deref());
        p.write_string8(self.primary_cpu_abi.as_deref());
        p.write_string8(self.secondary_cpu_abi.as_deref());
        p.write_string8(self.native_library_dir.as_deref());
        p.write_string8(self.secondary_native_library_dir.as_deref());
        p.write_bool(self.handle_profiling);
        p.write_bool(self.functional_test);
    }
}

/// `PermissionInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionInfo {
    pub item: PackageItemInfo,
    pub protection_level: i32,
    pub flags: i32,
    pub group: Option<String>,
    pub background_permission: Option<String>,
    pub description_res: i32,
    pub request_res: i32,
    pub known_certs: Option<Vec<Option<String>>>,
}

impl PermissionInfo {
    fn write(&self, p: &mut Parcel) {
        self.item.write(p);
        p.write_i32(self.protection_level);
        p.write_i32(self.flags);
        p.write_string8(self.group.as_deref());
        p.write_string8(self.background_permission.as_deref());
        p.write_i32(self.description_res);
        p.write_i32(self.request_res);
        // nonLocalizedDescription.
        write_char_sequence(p, None);
        match &self.known_certs {
            None => p.write_i32(-1),
            Some(certs) => {
                p.write_i32(certs.len() as i32);
                for c in certs {
                    p.write_string16(c.as_deref());
                }
            }
        }
    }
}

/// `SigningDetails` as a reply carries it: the signatures, DER encoded,
/// and the past signing certificates; `None` is `UNKNOWN`.
#[derive(Clone, Debug, PartialEq)]
pub struct SigningInfo {
    pub scheme_version: i32,
    pub signatures: Vec<Vec<u8>>,
    /// The signers' public keys as the original serialized them; `None`
    /// where the model has none (#738).
    pub public_keys: Option<Vec<Option<pkg::Serialized>>>,
    pub past_signing_certificates: Option<Vec<Vec<u8>>>,
}

/// `PackageInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageInfo {
    pub package_name: Option<String>,
    pub split_names: Option<Vec<Option<String>>>,
    pub version_code: i32,
    pub version_code_major: i32,
    pub version_name: Option<String>,
    pub base_revision_code: i32,
    pub split_revision_codes: Option<Vec<i32>>,
    pub shared_user_id: Option<String>,
    pub shared_user_label: i32,
    pub application_info: Option<Arc<ApplicationInfo>>,
    pub first_install_time: i64,
    pub last_update_time: i64,
    pub gids: Option<Vec<i32>>,
    pub activities: Option<Vec<ActivityInfo>>,
    pub receivers: Option<Vec<ActivityInfo>>,
    pub services: Option<Vec<ServiceInfo>>,
    pub providers: Option<Vec<ProviderInfo>>,
    pub instrumentation: Option<Vec<InstrumentationInfo>>,
    pub permissions: Option<Vec<PermissionInfo>>,
    pub requested_permissions: Option<Vec<String>>,
    pub requested_permissions_flags: Option<Vec<i32>>,
    pub signatures: Option<Vec<Vec<u8>>>,
    pub config_preferences: Option<Vec<pkg::ConfigurationInfo>>,
    pub req_features: Option<Vec<pkg::FeatureInfo>>,
    pub feature_groups: Option<Vec<Option<Vec<pkg::FeatureInfo>>>>,
    /// Tag and label.
    pub attributions: Option<Vec<(Option<String>, i32)>>,
    pub install_location: i32,
    pub is_stub: bool,
    pub core_app: bool,
    pub required_for_all_users: bool,
    pub restricted_account_type: Option<String>,
    pub required_account_type: Option<String>,
    pub overlay_target: Option<String>,
    pub overlay_category: Option<String>,
    pub overlay_priority: i32,
    pub overlay_is_static: bool,
    pub compile_sdk_version: i32,
    pub compile_sdk_version_codename: Option<String>,
    pub signing_info: Option<SigningInfo>,
    pub is_apex: bool,
    pub is_active_apex: bool,
    pub archive_time_millis: i64,
    pub apex_package_name: Option<String>,
}

impl Default for PackageInfo {
    fn default() -> Self {
        PackageInfo {
            package_name: None,
            split_names: None,
            version_code: 0,
            version_code_major: 0,
            version_name: None,
            base_revision_code: 0,
            split_revision_codes: None,
            shared_user_id: None,
            shared_user_label: 0,
            application_info: None,
            first_install_time: 0,
            last_update_time: 0,
            gids: None,
            activities: None,
            receivers: None,
            services: None,
            providers: None,
            instrumentation: None,
            permissions: None,
            requested_permissions: None,
            requested_permissions_flags: None,
            signatures: None,
            config_preferences: None,
            req_features: None,
            feature_groups: None,
            attributions: None,
            // INSTALL_LOCATION_INTERNAL_ONLY.
            install_location: 1,
            is_stub: false,
            core_app: false,
            required_for_all_users: false,
            restricted_account_type: None,
            required_account_type: None,
            overlay_target: None,
            overlay_category: None,
            overlay_priority: 0,
            overlay_is_static: false,
            compile_sdk_version: 0,
            compile_sdk_version_codename: None,
            signing_info: None,
            is_apex: false,
            is_active_apex: false,
            archive_time_millis: 0,
            apex_package_name: None,
        }
    }
}

impl WriteParcelable for PackageInfo {
    /// `writeToParcel`, squashing its `ApplicationInfo`s.
    fn write_to(&self, p: &mut Parcel) {
        let mut squash = Squash::default();
        p.write_string8(self.package_name.as_deref());
        write_string8_array(p, self.split_names.as_deref());
        p.write_i32(self.version_code);
        p.write_i32(self.version_code_major);
        p.write_string8(self.version_name.as_deref());
        p.write_i32(self.base_revision_code);
        write_int_array(p, self.split_revision_codes.as_deref());
        p.write_string8(self.shared_user_id.as_deref());
        p.write_i32(self.shared_user_label);
        match &self.application_info {
            Some(a) => {
                p.write_i32(1);
                a.write(p, Some(&mut squash));
            }
            None => p.write_i32(0),
        }
        p.write_i64(self.first_install_time);
        p.write_i64(self.last_update_time);
        write_int_array(p, self.gids.as_deref());
        write_typed_array(p, self.activities.as_deref(), |p, a| {
            a.write(p, Some(&mut squash))
        });
        write_typed_array(p, self.receivers.as_deref(), |p, a| {
            a.write(p, Some(&mut squash))
        });
        write_typed_array(p, self.services.as_deref(), |p, s| {
            s.write(p, Some(&mut squash))
        });
        write_typed_array(p, self.providers.as_deref(), |p, pr| {
            pr.write(p, Some(&mut squash))
        });
        write_typed_array(p, self.instrumentation.as_deref(), |p, i| i.write(p));
        write_typed_array(p, self.permissions.as_deref(), |p, pi| pi.write(p));
        write_strings8(p, self.requested_permissions.as_deref());
        write_int_array(p, self.requested_permissions_flags.as_deref());
        write_typed_array(p, self.signatures.as_deref(), |p, s| write_bytes(p, s));
        write_typed_array(p, self.config_preferences.as_deref(), |p, c| {
            p.write_i32(c.req_touch_screen);
            p.write_i32(c.req_keyboard_type);
            p.write_i32(c.req_navigation);
            p.write_i32(c.req_input_features);
            p.write_i32(c.req_gl_es_version);
        });
        write_typed_array(p, self.req_features.as_deref(), write_feature_info);
        write_typed_array(p, self.feature_groups.as_deref(), |p, g| {
            write_typed_array(p, g.as_deref(), write_feature_info)
        });
        write_typed_array(p, self.attributions.as_deref(), |p, (tag, label)| {
            p.write_string16(tag.as_deref());
            p.write_i32(*label);
        });
        p.write_i32(self.install_location);
        p.write_bool(self.is_stub);
        p.write_bool(self.core_app);
        p.write_bool(self.required_for_all_users);
        p.write_string8(self.restricted_account_type.as_deref());
        p.write_string8(self.required_account_type.as_deref());
        p.write_string8(self.overlay_target.as_deref());
        p.write_string8(self.overlay_category.as_deref());
        p.write_i32(self.overlay_priority);
        p.write_bool(self.overlay_is_static);
        p.write_i32(self.compile_sdk_version);
        p.write_string8(self.compile_sdk_version_codename.as_deref());
        match &self.signing_info {
            Some(s) => {
                p.write_i32(1);
                write_signing_details(p, Some(s));
            }
            None => p.write_i32(0),
        }
        p.write_bool(self.is_apex);
        p.write_bool(self.is_active_apex);
        p.write_i64(self.archive_time_millis);
        match &self.apex_package_name {
            Some(name) => {
                p.write_i32(1);
                p.write_string8(Some(name));
            }
            None => p.write_i32(0),
        }
    }
}

/// `SigningDetails.writeToParcel`; `None` is `UNKNOWN`.
pub fn write_signing_details(p: &mut Parcel, s: Option<&SigningInfo>) {
    let Some(s) = s else {
        p.write_bool(true);
        return;
    };
    p.write_bool(false);
    write_typed_array(p, Some(&s.signatures), |p, s| write_bytes(p, s));
    p.write_i32(s.scheme_version);
    // writeArraySet: each key a length-prefixed VAL_SERIALIZABLE.
    match &s.public_keys {
        None => p.write_i32(-1),
        Some(keys) => {
            p.write_i32(keys.len() as i32);
            for key in keys {
                let Some(key) = key else {
                    p.write_i32(-1);
                    continue;
                };
                p.write_i32(21);
                let length = p.position();
                p.write_i32(-1);
                let start = p.position();
                p.write_string16(Some(&key.class));
                write_bytes(p, &key.bytes);
                let end = p.position();
                p.set_i32_at(length, (end - start) as i32);
            }
        }
    }
    write_typed_array(p, s.past_signing_certificates.as_deref(), |p, s| {
        write_bytes(p, s)
    });
}

fn write_feature_info(p: &mut Parcel, f: &pkg::FeatureInfo) {
    p.write_string8(f.name.as_deref());
    p.write_i32(f.version);
    p.write_i32(f.req_gl_es_version);
    p.write_i32(f.flags);
}

impl WriteParcelable for pkg::FeatureInfo {
    fn write_to(&self, p: &mut Parcel) {
        write_feature_info(p, self);
    }
}

/// `StorageManager.convert` of a volume UUID.
fn storage_uuid(volume: Option<&str>) -> (i64, i64) {
    match volume {
        None => UUID_DEFAULT,
        Some("primary_physical") => UUID_PRIMARY_PHYSICAL,
        Some("system") => UUID_SYSTEM,
        Some(uuid) => {
            let hex: String = uuid.chars().filter(|c| *c != '-').collect();
            let msb = u64::from_str_radix(hex.get(..16).unwrap_or("0"), 16).unwrap_or(0);
            let lsb = u64::from_str_radix(hex.get(16..).unwrap_or("0"), 16).unwrap_or(0);
            (msb as i64, lsb as i64)
        }
    }
}

/// `TextUtils.writeToParcel` of a plain string.
fn write_char_sequence(p: &mut Parcel, s: Option<&str>) {
    p.write_i32(1);
    p.write_string8(s);
}

/// `writeBundle` of meta-data: an empty one as its length 0.
fn write_bundle(p: &mut Parcel, b: Option<&MetaData>) {
    let Some(b) = b else {
        p.write_i32(-1);
        return;
    };
    if b.0.is_empty() {
        p.write_i32(0);
        return;
    }
    let length = p.position();
    p.write_i32(-1);
    p.write_i32(0x4C44_4E42);
    let start = p.position();
    p.write_i32(b.0.len() as i32);
    for (key, value) in &b.0 {
        p.write_string16(Some(key));
        match value {
            Value::Null => p.write_i32(-1),
            Value::String(s) => {
                p.write_i32(0);
                p.write_string16(s.as_deref());
            }
            Value::Int(v) => {
                p.write_i32(1);
                p.write_i32(*v);
            }
            Value::Long(v) => {
                p.write_i32(6);
                p.write_i64(*v);
            }
            Value::Float(v) => {
                p.write_i32(7);
                p.write_f32(*v);
            }
            Value::Double(v) => {
                p.write_i32(8);
                p.write_i64(v.to_bits() as i64);
            }
            Value::Bool(v) => {
                p.write_i32(9);
                p.write_bool(*v);
            }
        }
    }
    let end = p.position();
    p.set_i32_at(length, (end - start) as i32);
    // mHasIntent.
    p.write_bool(false);
}

fn write_bytes(p: &mut Parcel, b: &[u8]) {
    p.write_i32(b.len() as i32);
    p.write_raw(b, &[]);
}

fn write_int_array(p: &mut Parcel, v: Option<&[i32]>) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|x| p.write_i32(*x));
        }
    }
}

fn write_string8_array(p: &mut Parcel, v: Option<&[Option<String>]>) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|s| p.write_string8(s.as_deref()));
        }
    }
}

fn write_strings8(p: &mut Parcel, v: Option<&[String]>) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|s| p.write_string8(Some(s)));
        }
    }
}

/// `ForStringSet`.
fn write_string_set(p: &mut Parcel, v: Option<&[String]>) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            v.iter().for_each(|s| p.write_string16(Some(s)));
        }
    }
}

/// `ForBoolean`: 1 unset, 0 false, -1 true.
fn write_for_boolean(p: &mut Parcel, v: Option<bool>) {
    p.write_i32(match v {
        None => 1,
        Some(false) => 0,
        Some(true) => -1,
    });
}

/// `writeSparseArray` of `int[]` values (`writeValue`: `VAL_INTARRAY`).
fn write_sparse_int_arrays(p: &mut Parcel, v: Option<&SplitDependencies>) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            for (key, value) in v {
                p.write_i32(*key);
                match value {
                    None => p.write_i32(-1),
                    Some(a) => {
                        p.write_i32(18);
                        write_int_array(p, Some(a));
                    }
                }
            }
        }
    }
}

/// `writeTypedArray`/`writeTypedList`: each item preceded by 1.
fn write_typed_array<T>(p: &mut Parcel, v: Option<&[T]>, mut item: impl FnMut(&mut Parcel, &T)) {
    match v {
        None => p.write_i32(-1),
        Some(v) => {
            p.write_i32(v.len() as i32);
            for x in v {
                p.write_i32(1);
                item(p, x);
            }
        }
    }
}

/// `writeTypedList` of `SharedLibraryInfo`s.
fn write_libraries(p: &mut Parcel, v: Option<&[SharedLibrary]>) {
    write_typed_array(p, v, write_library);
}

/// `SharedLibraryInfo.writeToParcel`. Optional dependents and
/// certificate digests are not in the model: written empty.
fn write_library(p: &mut Parcel, l: &SharedLibrary) {
    const VERSIONED_PACKAGE: &str = "android.content.pm.VersionedPackage";
    p.write_string8(l.path.as_deref());
    p.write_string8(l.package_name.as_deref());
    match &l.code_paths {
        Some(paths) => {
            p.write_i32(1);
            write_strings8(p, Some(paths));
        }
        None => p.write_i32(0),
    }
    p.write_string8(l.name.as_deref());
    p.write_i64(l.version);
    p.write_i32(l.kind);
    p.write_string16(Some(VERSIONED_PACKAGE));
    p.write_string8(Some(&l.declaring.0));
    p.write_i64(l.declaring.1);
    // writeList of VersionedPackages: each a length-prefixed parcelable.
    p.write_i32(l.dependents.len() as i32);
    for (name, version) in &l.dependents {
        p.write_i32(4);
        let length = p.position();
        p.write_i32(-1);
        let start = p.position();
        p.write_string16(Some(VERSIONED_PACKAGE));
        p.write_string8(Some(name));
        p.write_i64(*version);
        let end = p.position();
        p.set_i32_at(length, (end - start) as i32);
    }
    write_libraries(p, Some(&l.dependencies));
    p.write_bool(l.native);
    p.write_i32(-1);
    p.write_i32(-1);
}

/// `PackageUserStateUtils.isAvailable`.
pub fn is_available(state: &PackageUserState, flags: i64) -> bool {
    let data_exists_matches = flags & (MATCH_UNINSTALLED_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0;
    if flags & MATCH_ANY_USER != 0 {
        return true;
    }
    if state.installed {
        !state.hidden || data_exists_matches
    } else {
        data_exists_matches && state.data_exists
    }
}

/// `PackageUserStateUtils.isEnabled`.
pub fn is_enabled(
    state: &PackageUserState,
    package_enabled: bool,
    component_enabled: bool,
    name: &str,
    flags: i64,
) -> bool {
    if flags & MATCH_DISABLED_COMPONENTS != 0 {
        return true;
    }
    if flags & MATCH_QUARANTINED_COMPONENTS == 0 && state.quarantined {
        return false;
    }
    match state.enabled {
        COMPONENT_ENABLED_STATE_DISABLED | COMPONENT_ENABLED_STATE_DISABLED_USER => return false,
        COMPONENT_ENABLED_STATE_DISABLED_UNTIL_USED
            if flags & MATCH_DISABLED_UNTIL_USED_COMPONENTS == 0 =>
        {
            return false;
        }
        COMPONENT_ENABLED_STATE_ENABLED => {}
        _ if !package_enabled => return false,
        _ => {}
    }
    if state.enabled_components.iter().any(|c| c == name) {
        true
    } else if state.disabled_components.iter().any(|c| c == name) {
        false
    } else {
        component_enabled
    }
}

/// `PackageUserStateUtils.isMatch`.
pub fn is_match(
    state: &PackageUserState,
    system: bool,
    package_enabled: bool,
    c: &MainComponent,
    flags: i64,
) -> bool {
    let available = is_available(state, flags) || (system && flags & MATCH_KNOWN_PACKAGES != 0);
    if !available {
        return false;
    }
    if !is_enabled(state, package_enabled, c.enabled, &c.component.name, flags) {
        return false;
    }
    if flags & MATCH_SYSTEM_ONLY != 0 && !system {
        return false;
    }
    (flags & MATCH_DIRECT_BOOT_UNAWARE != 0 && !c.direct_boot_aware)
        || (flags & MATCH_DIRECT_BOOT_AWARE != 0 && c.direct_boot_aware)
}

/// `PackageStateUtils.isEnabledAndMatches`: the component's package
/// state in `user`.
pub fn is_enabled_and_matches(ps: &PackageState, c: &MainComponent, flags: i64, user: i32) -> bool {
    let Some(pkg) = &ps.pkg else {
        return false;
    };
    let state = user_state(ps, user);
    is_match(&state, ps.is.system, pkg.is(booleans::ENABLED), c, flags)
}

/// `PackageInfoUtils.checkUseInstalledOrHidden`.
pub fn check_use_installed_or_hidden(
    ps: &PackageState,
    state: &PackageUserState,
    flags: i64,
) -> bool {
    if flags & MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS == 0
        && !state.installed
        && ps.is.hidden_until_installed
    {
        return false;
    }
    is_available(state, flags) || (ps.is.system && match_uninstalled_or_hidden(flags))
}

fn match_uninstalled_or_hidden(flags: i64) -> bool {
    flags
        & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES | MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS)
        != 0
}

fn aconfig(sys: &System, name: &str) -> bool {
    sys.flags.iter().any(|(n, on)| n == name && *on)
}

/// `AppInfoUtils.appInfoFlags` of the package (`PackageImpl`'s base
/// flags).
fn base_flags(pkg: &AndroidPackage) -> i32 {
    use booleans::*;
    let b = |f: i64| pkg.is(f);
    flag(b(EXTERNAL_STORAGE), FLAG_EXTERNAL_STORAGE)
        | flag(b(HARDWARE_ACCELERATED), FLAG_HARDWARE_ACCELERATED)
        | flag(b(ALLOW_BACKUP), FLAG_ALLOW_BACKUP)
        | flag(b(KILL_AFTER_RESTORE), FLAG_KILL_AFTER_RESTORE)
        | flag(b(RESTORE_ANY_VERSION), FLAG_RESTORE_ANY_VERSION)
        | flag(b(FULL_BACKUP_ONLY), FLAG_FULL_BACKUP_ONLY)
        | flag(b(PERSISTENT), FLAG_PERSISTENT)
        | flag(b(DEBUGGABLE), FLAG_DEBUGGABLE)
        | flag(b(VM_SAFE_MODE), FLAG_VM_SAFE_MODE)
        | flag(b(HAS_CODE), FLAG_HAS_CODE)
        | flag(b(ALLOW_TASK_REPARENTING), FLAG_ALLOW_TASK_REPARENTING)
        | flag(b(ALLOW_CLEAR_USER_DATA), FLAG_ALLOW_CLEAR_USER_DATA)
        | flag(b(LARGE_HEAP), FLAG_LARGE_HEAP)
        | flag(b(USES_CLEARTEXT_TRAFFIC), FLAG_USES_CLEARTEXT_TRAFFIC)
        | flag(b(SUPPORTS_RTL), FLAG_SUPPORTS_RTL)
        | flag(b(TEST_ONLY), FLAG_TEST_ONLY)
        | flag(b(MULTI_ARCH), FLAG_MULTIARCH)
        | flag(b(EXTRACT_NATIVE_LIBS), FLAG_EXTRACT_NATIVE_LIBS)
        | flag(b(GAME), FLAG_IS_GAME)
        | flag(
            screens(pkg.supports_small_screens, pkg),
            FLAG_SUPPORTS_SMALL_SCREENS,
        )
        | flag(
            pkg.supports_normal_screens != Some(false),
            FLAG_SUPPORTS_NORMAL_SCREENS,
        )
        | flag(
            screens(pkg.supports_large_screens, pkg),
            FLAG_SUPPORTS_LARGE_SCREENS,
        )
        | flag(
            screens_since(pkg.supports_extra_large_screens, pkg, 9),
            FLAG_SUPPORTS_XLARGE_SCREENS,
        )
        | flag(screens(pkg.resizeable, pkg), FLAG_RESIZEABLE_FOR_SCREENS)
        | flag(
            screens(pkg.any_density, pkg),
            FLAG_SUPPORTS_SCREEN_DENSITIES,
        )
        | flag(b(SYSTEM), FLAG_SYSTEM)
        | flag(b(FACTORY_TEST), FLAG_FACTORY_TEST)
}

/// `PackageImpl.isSmallScreensSupported` and the like: unset means
/// supported from Donut (SDK 4) on.
fn screens(v: Option<bool>, pkg: &AndroidPackage) -> bool {
    screens_since(v, pkg, 4)
}

fn screens_since(v: Option<bool>, pkg: &AndroidPackage, sdk: i32) -> bool {
    v.unwrap_or(pkg.target_sdk_version >= sdk)
}

/// `AppInfoUtils.appInfoPrivateFlags`.
fn base_private_flags(pkg: &AndroidPackage) -> i32 {
    use booleans::*;
    let b = |f: i64| pkg.is(f);
    let mut flags = flag(b(STATIC_SHARED_LIBRARY), PRIVATE_FLAG_STATIC_SHARED_LIBRARY)
        | flag(b(OVERLAY), PRIVATE_FLAG_IS_RESOURCE_OVERLAY)
        | flag(
            b(ISOLATED_SPLIT_LOADING),
            PRIVATE_FLAG_ISOLATED_SPLIT_LOADING,
        )
        | flag(b(HAS_DOMAIN_URLS), PRIVATE_FLAG_HAS_DOMAIN_URLS)
        | flag(
            !b(DISALLOW_PROFILING) && b(PROFILEABLE_BY_SHELL),
            PRIVATE_FLAG_PROFILEABLE_BY_SHELL,
        )
        | flag(b(BACKUP_IN_FOREGROUND), PRIVATE_FLAG_BACKUP_IN_FOREGROUND)
        | flag(b(USE_EMBEDDED_DEX), PRIVATE_FLAG_USE_EMBEDDED_DEX)
        | flag(
            b(DEFAULT_TO_DEVICE_PROTECTED_STORAGE),
            PRIVATE_FLAG_DEFAULT_TO_DEVICE_PROTECTED_STORAGE,
        )
        | flag(b(DIRECT_BOOT_AWARE), PRIVATE_FLAG_DIRECT_BOOT_AWARE)
        | flag(
            b(PARTIALLY_DIRECT_BOOT_AWARE),
            PRIVATE_FLAG_PARTIALLY_DIRECT_BOOT_AWARE,
        )
        | flag(
            b(ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE),
            PRIVATE_FLAG_ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE,
        )
        | flag(
            b(ALLOW_AUDIO_PLAYBACK_CAPTURE),
            PRIVATE_FLAG_ALLOW_AUDIO_PLAYBACK_CAPTURE,
        )
        | flag(
            b(REQUEST_LEGACY_EXTERNAL_STORAGE),
            PRIVATE_FLAG_REQUEST_LEGACY_EXTERNAL_STORAGE,
        )
        | flag(b(USES_NON_SDK_API), PRIVATE_FLAG_USES_NON_SDK_API)
        | flag(b(HAS_FRAGILE_USER_DATA), PRIVATE_FLAG_HAS_FRAGILE_USER_DATA)
        | flag(b(CANT_SAVE_STATE), PRIVATE_FLAG_CANT_SAVE_STATE)
        | flag(
            b(RESIZEABLE_ACTIVITY_VIA_SDK_VERSION),
            PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION,
        )
        | flag(
            b(ALLOW_NATIVE_HEAP_POINTER_TAGGING),
            PRIVATE_FLAG_ALLOW_NATIVE_HEAP_POINTER_TAGGING,
        )
        | flag(b(SYSTEM_EXT), PRIVATE_FLAG_SYSTEM_EXT)
        | flag(b(PRIVILEGED), PRIVATE_FLAG_PRIVILEGED)
        | flag(b(OEM), PRIVATE_FLAG_OEM)
        | flag(b(VENDOR), PRIVATE_FLAG_VENDOR)
        | flag(b(PRODUCT), PRIVATE_FLAG_PRODUCT)
        | flag(b(ODM), PRIVATE_FLAG_ODM)
        | flag(
            b(SIGNED_WITH_PLATFORM_KEY),
            PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY,
        );
    match pkg.resizeable_activity {
        Some(true) => flags |= PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_RESIZEABLE,
        Some(false) => flags |= PRIVATE_FLAG_ACTIVITIES_RESIZE_MODE_UNRESIZEABLE,
        None => {}
    }
    flags
}

/// `AppInfoUtils.appInfoPrivateFlagsExt`.
fn base_private_flags_ext(pkg: &AndroidPackage, sys: &System) -> i32 {
    use booleans::*;
    let profileable = !pkg.is(DISALLOW_PROFILING);
    flag(profileable, PRIVATE_FLAG_EXT_PROFILEABLE)
        | flag(
            pkg.is(REQUEST_FOREGROUND_SERVICE_EXEMPTION),
            PRIVATE_FLAG_EXT_REQUEST_FOREGROUND_SERVICE_EXEMPTION,
        )
        | flag(
            pkg.is(ATTRIBUTIONS_ARE_USER_VISIBLE),
            PRIVATE_FLAG_EXT_ATTRIBUTIONS_ARE_USER_VISIBLE,
        )
        | flag(
            pkg.is(ENABLE_ON_BACK_INVOKED_CALLBACK),
            PRIVATE_FLAG_EXT_ENABLE_ON_BACK_INVOKED_CALLBACK,
        )
        | flag(
            sys.hidden_api_allowlist.contains(&pkg.package_name),
            PRIVATE_FLAG_EXT_ALLOWLISTED_FOR_HIDDEN_APIS,
        )
}

/// `PackageImpl.getProcessName`: the application's own process.
fn process_name(pkg: &AndroidPackage) -> Option<String> {
    pkg.process_name
        .clone()
        .or_else(|| Some(pkg.package_name.clone()))
}

/// `PackageImpl.buildAppClassNamesByProcess`.
fn app_class_names_by_process(pkg: &AndroidPackage) -> Option<Vec<(String, String)>> {
    let processes = pkg.processes.as_ref()?;
    let mut map = Vec::new();
    for process in processes {
        for (package, class) in &process.app_class_names_by_package {
            if *package == pkg.package_name
                && let (Some(name), Some(class)) = (&process.name, class)
                && !class.is_empty()
            {
                map.retain(|(n, _): &(String, String)| n != name);
                map.push((name.clone(), class.clone()));
            }
        }
    }
    (!map.is_empty()).then(|| array_order(map, |(n, _)| n))
}

/// `Environment.getDataDirectoryPath(volumeUuid)`.
fn data_directory(volume: Option<&str>) -> String {
    match volume {
        None => "/data".into(),
        Some(uuid) => format!("/mnt/expand/{uuid}"),
    }
}

/// `PackageImpl.toAppInfoWithoutState`.
fn app_info_without_state(pkg: &AndroidPackage, sys: &System) -> ApplicationInfo {
    let split_paths = pkg.split_code_paths.clone().filter(|p| !p.is_empty());
    let use_round = sys.use_round_icon && pkg.round_icon_res != 0;
    ApplicationInfo {
        item: PackageItemInfo {
            name: pkg.class_name.clone(),
            package_name: Some(pkg.package_name.clone()),
            label_res: pkg.label_res,
            non_localized_label: pkg.non_localized_label.clone(),
            icon: if use_round {
                pkg.round_icon_res
            } else {
                pkg.icon_res
            },
            logo: pkg.logo,
            meta_data: pkg.meta_data.clone(),
            banner: pkg.banner,
            ..PackageItemInfo::default()
        },
        app_component_factory: pkg.app_component_factory.clone(),
        backup_agent_name: pkg.backup_agent_name.clone(),
        category: pkg.category,
        class_loader_name: pkg.class_loader_name.clone(),
        class_name: pkg.class_name.clone(),
        compatible_width_limit_dp: pkg.compatible_width_limit_dp,
        compile_sdk_version: pkg.compile_sdk_version,
        compile_sdk_version_codename: pkg.compile_sdk_version_code_name.clone(),
        cross_profile: pkg.is(booleans::CROSS_PROFILE),
        description_res: pkg.description_res,
        enabled: pkg.is(booleans::ENABLED),
        full_backup_content: pkg.full_backup_content,
        data_extraction_rules_res: pkg.data_extraction_rules,
        icon_res: pkg.icon_res,
        round_icon_res: pkg.round_icon_res,
        install_location: pkg.install_location,
        largest_width_limit_dp: pkg.largest_width_limit_dp,
        manage_space_activity_name: pkg.manage_space_activity_name.clone(),
        min_sdk_version: pkg.min_sdk_version,
        network_security_config_res: pkg.network_security_config_res,
        permission: pkg.permission.clone(),
        process_name: process_name(pkg),
        requires_smallest_width_dp: pkg.requires_smallest_width_dp,
        split_class_loader_names: pkg.split_class_loader_names.clone(),
        split_dependencies: pkg.split_dependencies.clone().filter(|d| !d.is_empty()),
        split_names: pkg.split_names.clone(),
        target_sandbox_version: pkg.target_sandbox_version,
        target_sdk_version: pkg.target_sdk_version,
        task_affinity: pkg.task_affinity.clone(),
        theme: pkg.theme,
        ui_options: pkg.ui_options,
        volume_uuid: pkg.volume_uuid.clone(),
        zygote_preload_name: pkg.zygote_preload_name.clone(),
        gwp_asan_mode: pkg.gwp_asan_mode,
        memtag_mode: pkg.memtag_mode,
        native_heap_zero_initialized: pkg.native_heap_zero_initialized,
        request_raw_external_storage_access: pkg.request_raw_external_storage_access,
        source_dir: pkg.base_apk_path.clone(),
        public_source_dir: pkg.base_apk_path.clone(),
        scan_source_dir: pkg.path.clone(),
        scan_public_source_dir: pkg.path.clone(),
        split_source_dirs: split_paths.clone(),
        split_public_source_dirs: split_paths,
        long_version_code: (i64::from(pkg.version_code_major) << 32)
            | i64::from(pkg.version_code as u32),
        app_class_names_by_process: app_class_names_by_process(pkg),
        locale_config_res: pkg.locale_config_res,
        known_activity_embedding_certs: pkg
            .known_activity_embedding_certs
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| upper_case_set(c)),
        allow_cross_uid_activity_switch_from_below: pkg.allow_cross_uid_activity_switch_from_below,
        page_size_app_compat_flags: pkg.page_size_app_compat_flags,
        flags: base_flags(pkg),
        private_flags: base_private_flags(pkg),
        private_flags_ext: base_private_flags_ext(pkg, sys),
        native_library_dir: pkg.native_library_dir.clone(),
        native_library_root_dir: pkg.native_library_root_dir.clone(),
        native_library_root_requires_isa: pkg.native_library_root_requires_isa,
        primary_cpu_abi: pkg.primary_cpu_abi.clone(),
        secondary_cpu_abi: pkg.secondary_cpu_abi.clone(),
        secondary_native_library_dir: pkg.secondary_native_library_dir.clone(),
        se_info_user: Some(SEINFO_COMPLETE.into()),
        uid: pkg.uid,
        ..ApplicationInfo::default()
    }
}

/// `setKnownActivityEmbeddingCerts`: an `ArraySet` of the upper-case
/// digests.
fn upper_case_set(certs: &[Option<String>]) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for c in certs.iter().flatten() {
        let upper = c.to_uppercase();
        if !set.contains(&upper) {
            set.push(upper);
        }
    }
    array_order(set, |s| s)
}

/// `PackageInfoUtils.updateApplicationInfo`.
fn update_application_info(
    ai: &mut ApplicationInfo,
    flags: i64,
    state: &PackageUserState,
    sys: &System,
) {
    if flags & GET_META_DATA == 0 {
        ai.item.meta_data = None;
    }
    if flags & GET_SHARED_LIBRARY_FILES == 0 {
        ai.shared_library_files = None;
        ai.shared_library_infos = None;
    }
    if !sys.compatibility_mode {
        ai.flags |= FLAG_SUPPORTS_LARGE_SCREENS
            | FLAG_SUPPORTS_NORMAL_SCREENS
            | FLAG_SUPPORTS_SMALL_SCREENS
            | FLAG_RESIZEABLE_FOR_SCREENS
            | FLAG_SUPPORTS_SCREEN_DENSITIES
            | FLAG_SUPPORTS_XLARGE_SCREENS;
    }
    ai.flags |= flag(state.stopped, FLAG_STOPPED)
        | flag(state.installed, FLAG_INSTALLED)
        | flag(!state.suspended_by.is_empty(), FLAG_SUSPENDED);
    ai.private_flags |= flag(state.instant_app, PRIVATE_FLAG_INSTANT)
        | flag(state.virtual_preload, PRIVATE_FLAG_VIRTUAL_PRELOAD)
        | flag(state.hidden, PRIVATE_FLAG_HIDDEN);
    ai.private_flags_ext |= flag(state.not_launched, PRIVATE_FLAG_EXT_NOT_LAUNCHED);
    match state.enabled {
        COMPONENT_ENABLED_STATE_ENABLED => ai.enabled = true,
        COMPONENT_ENABLED_STATE_DISABLED_UNTIL_USED => {
            ai.enabled = flags & MATCH_DISABLED_UNTIL_USED_COMPONENTS != 0
        }
        COMPONENT_ENABLED_STATE_DISABLED | COMPONENT_ENABLED_STATE_DISABLED_USER => {
            ai.enabled = false
        }
        _ => {}
    }
    ai.enabled_setting = state.enabled;
    if ai.category == CATEGORY_UNDEFINED
        && let Some(package) = &ai.item.package_name
        && let Some((_, category)) = sys.fallback_categories.iter().find(|(p, _)| p == package)
    {
        ai.category = *category;
    }
    ai.se_info_user = Some(if state.instant_app {
        format!("{SEINFO_INSTANT_APP}{SEINFO_COMPLETE}")
    } else {
        SEINFO_COMPLETE.into()
    });
    if let Some(o) = &state.overlay_paths {
        ai.resource_dirs = Some(o.resource_dirs.clone());
        ai.overlay_paths = Some(o.overlay_paths.clone());
    }
    ai.item.is_archived = is_archived(state);
    if ai.item.is_archived
        && let Some(a) = state
            .archive_state
            .as_ref()
            .and_then(|a| a.activities.first())
    {
        ai.item.non_localized_label = Some(a.title.clone());
    }
    if !state.installed && !state.data_exists && aconfig(sys, NULLABLE_DATA_DIR) {
        ai.data_dir = None;
    }
}

/// `PackageArchiver.isArchived`.
fn is_archived(state: &PackageUserState) -> bool {
    !state.installed && state.archive_state.is_some()
}

/// A package as a query sees it in one user: its parsed package, its
/// state, the user's state of it, and the system's constants.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub sys: &'a System,
    pub pkg: &'a AndroidPackage,
    pub ps: &'a PackageState,
    pub state: &'a PackageUserState,
    pub user: i32,
}

/// `PackageInfoUtils.initForUser`: an application's or an
/// instrumentation's data directories in the user.
struct DataDirs {
    data: Option<String>,
    device_protected: Option<String>,
    credential_protected: Option<String>,
}

fn data_dirs(t: &Target<'_>) -> DataDirs {
    let none = DataDirs {
        data: None,
        device_protected: None,
        credential_protected: None,
    };
    if t.pkg.package_name == "android" {
        return DataDirs {
            data: Some("/data/system".into()),
            ..none
        };
    }
    if !t.state.installed && !t.state.data_exists && aconfig(t.sys, NULLABLE_DATA_DIR) {
        return none;
    }
    let base = data_directory(t.pkg.volume_uuid.as_deref());
    let (name, user) = (&t.pkg.package_name, t.user);
    let ce = format!("{base}/user/{user}/{name}");
    let de = format!("{base}/user_de/{user}/{name}");
    let data = if t.pkg.is(booleans::DEFAULT_TO_DEVICE_PROTECTED_STORAGE) {
        de.clone()
    } else {
        ce.clone()
    };
    DataDirs {
        data: Some(data),
        device_protected: Some(de),
        credential_protected: Some(ce),
    }
}

/// `PackageInfoUtils.generateApplicationInfo`.
pub fn generate_application_info(t: &Target<'_>, flags: i64) -> Option<ApplicationInfo> {
    let (pkg, ps, state) = (t.pkg, t.ps, t.state);
    if !check_use_installed_or_hidden(ps, state, flags)
        || (flags & MATCH_SYSTEM_ONLY != 0 && !ps.is.system)
    {
        return None;
    }
    let mut info = app_info_without_state(pkg, t.sys);
    update_application_info(&mut info, flags, state, t.sys);
    info.uid = uid(t.user, pkg.uid);
    let dirs = data_dirs(t);
    if pkg.package_name == "android" || dirs.data.is_none() {
        info.data_dir = dirs.data;
    } else {
        info.credential_protected_data_dir = dirs.credential_protected;
        info.device_protected_data_dir = dirs.device_protected;
        info.data_dir = dirs.data;
    }
    info.hidden_until_installed = ps.is.hidden_until_installed;
    info.shared_library_files =
        (!ps.uses_library_files.is_empty()).then(|| ps.uses_library_files.clone());
    info.shared_library_infos =
        (!ps.uses_library_infos.is_empty()).then(|| ps.uses_library_infos.clone());
    // sdk_lib_independence: the SDK libraries the package marks optional.
    let optional: Vec<SharedLibrary> = ps
        .uses_library_infos
        .iter()
        .filter(|l| {
            l.kind == SHARED_LIBRARY_TYPE_SDK_PACKAGE
                && ps
                    .uses_sdk_libraries
                    .iter()
                    .any(|s| s.optional && l.name.as_deref() == Some(&s.name))
        })
        .cloned()
        .collect();
    info.optional_shared_library_infos = (!optional.is_empty()).then_some(optional);
    if info.category == CATEGORY_UNDEFINED {
        info.category = ps.category_override;
    }
    info.se_info = ps.seinfo.clone();
    info.primary_cpu_abi = ps.primary_cpu_abi.clone();
    info.secondary_cpu_abi = ps.secondary_cpu_abi.clone();
    info.flags |= flag(ps.is.updated_system_app, FLAG_UPDATED_SYSTEM_APP);
    info.private_flags_ext |= flag(ps.cpu_abi_override.is_some(), PRIVATE_FLAG_EXT_CPU_OVERRIDE);
    Some(info)
}

/// `SharedLibraryInfo.TYPE_SDK_PACKAGE`.
const SHARED_LIBRARY_TYPE_SDK_PACKAGE: i32 = 3;

/// The application's info a component's shares: the caller's, or made
/// now (when the caller has none, or one of another package).
fn app_of(
    t: &Target<'_>,
    flags: i64,
    app: Option<Arc<ApplicationInfo>>,
) -> Option<Arc<ApplicationInfo>> {
    match app {
        Some(app) if app.item.package_name.as_deref() == Some(&t.pkg.package_name) => Some(app),
        _ => generate_application_info(t, flags).map(Arc::new),
    }
}

/// The meta-data a query returns: none without `GET_META_DATA`, and an
/// empty bundle as none.
fn meta_data(c: &Component, flags: i64) -> Option<MetaData> {
    if flags & GET_META_DATA == 0 {
        return None;
    }
    c.meta_data.clone().filter(|m| !m.0.is_empty())
}

/// `PackageInfoUtils.generateActivityInfo`, sharing the application's
/// info `app` when the caller made it.
pub fn generate_activity_info(
    t: &Target<'_>,
    a: &pkg::Activity,
    flags: i64,
    app: Option<Arc<ApplicationInfo>>,
) -> Option<ActivityInfo> {
    if !check_use_installed_or_hidden(t.ps, t.state, flags) {
        return None;
    }
    let app = app_of(t, flags, app)?;
    let mut info = ComponentInfo::of(&a.main, t.state, app);
    info.item.meta_data = meta_data(&a.main.component, flags);
    Some(ActivityInfo {
        info,
        target_activity: a.target_activity.clone(),
        theme: a.theme,
        ui_options: a.ui_options,
        parent_activity_name: a.parent_activity_name.clone(),
        permission: a.permission.clone(),
        task_affinity: a.task_affinity.clone(),
        flags: a.main.component.flags,
        private_flags: a.private_flags,
        launch_mode: a.launch_mode,
        document_launch_mode: a.document_launch_mode,
        max_recents: a.max_recents,
        config_changes: a.config_changes,
        soft_input_mode: a.soft_input_mode,
        persistable_mode: a.persistable_mode,
        lock_task_launch_mode: a.lock_task_launch_mode,
        screen_orientation: a.screen_orientation,
        resize_mode: a.resize_mode,
        max_aspect_ratio: a.max_aspect_ratio.unwrap_or(0.0).max(0.0),
        min_aspect_ratio: a.min_aspect_ratio.unwrap_or(0.0).max(0.0),
        supports_size_changes: a.supports_size_changes,
        requested_vr_component: a.requested_vr_component.clone(),
        rotation_animation: a.rotation_animation,
        color_mode: a.color_mode,
        window_layout: a.window_layout.clone(),
        required_display_category: a.required_display_category.clone(),
        require_content_uri_permission_from_caller: a.require_content_uri_permission_from_caller,
        known_activity_embedding_certs: Some(upper_case_set(
            a.known_activity_embedding_certs
                .as_deref()
                .unwrap_or_default(),
        )),
    })
}

/// `PackageInfoUtils.generateServiceInfo`.
pub fn generate_service_info(
    t: &Target<'_>,
    s: &pkg::Service,
    flags: i64,
    app: Option<Arc<ApplicationInfo>>,
) -> Option<ServiceInfo> {
    if !check_use_installed_or_hidden(t.ps, t.state, flags) {
        return None;
    }
    let app = app_of(t, flags, app)?;
    let mut info = ComponentInfo::of(&s.main, t.state, app);
    info.item.meta_data = meta_data(&s.main.component, flags);
    Some(ServiceInfo {
        info,
        permission: s.permission.clone(),
        flags: s.main.component.flags,
        foreground_service_type: s.foreground_service_type,
    })
}

/// `PackageInfoUtils.generateProviderInfo`.
pub fn generate_provider_info(
    t: &Target<'_>,
    pr: &pkg::Provider,
    flags: i64,
    app: Option<Arc<ApplicationInfo>>,
) -> Option<ProviderInfo> {
    if !check_use_installed_or_hidden(t.ps, t.state, flags) {
        return None;
    }
    let app = app_of(t, flags, app)?;
    let mut info = ComponentInfo::of(&pr.main, t.state, app);
    info.item.meta_data = meta_data(&pr.main.component, flags);
    Some(ProviderInfo {
        info,
        authority: pr.authority.clone(),
        read_permission: pr.read_permission.clone(),
        write_permission: pr.write_permission.clone(),
        grant_uri_permissions: pr.grant_uri_permissions,
        force_uri_permissions: pr.force_uri_permissions,
        uri_permission_patterns: (flags & GET_URI_PERMISSION_PATTERNS != 0)
            .then(|| pr.uri_permission_patterns.clone().unwrap_or_default()),
        path_permissions: Some(pr.path_permissions.clone().unwrap_or_default()),
        multiprocess: pr.multi_process,
        init_order: pr.init_order,
        flags: pr.main.component.flags,
        is_syncable: pr.syncable,
    })
}

/// `PackageInfoUtils.generateInstrumentationInfo`.
fn generate_instrumentation_info(
    t: &Target<'_>,
    i: &pkg::Instrumentation,
    flags: i64,
) -> Option<InstrumentationInfo> {
    if !check_use_installed_or_hidden(t.ps, t.state, flags) {
        return None;
    }
    let pkg = t.pkg;
    let splits = pkg.split_code_paths.clone().filter(|p| !p.is_empty());
    let dirs = data_dirs(t);
    let mut item = PackageItemInfo::of(&i.component, t.state);
    item.meta_data = meta_data(&i.component, flags);
    Some(InstrumentationInfo {
        item,
        target_package: i.target_package.clone(),
        target_processes: i.target_processes.clone(),
        handle_profiling: i.handle_profiling,
        functional_test: i.functional_test,
        source_dir: pkg.base_apk_path.clone(),
        public_source_dir: pkg.base_apk_path.clone(),
        split_names: pkg.split_names.clone(),
        split_source_dirs: splits.clone(),
        split_public_source_dirs: splits,
        split_dependencies: pkg.split_dependencies.clone().filter(|d| !d.is_empty()),
        data_dir: dirs.data,
        device_protected_data_dir: dirs.device_protected,
        credential_protected_data_dir: dirs.credential_protected,
        primary_cpu_abi: t.ps.primary_cpu_abi.clone(),
        secondary_cpu_abi: t.ps.secondary_cpu_abi.clone(),
        native_library_dir: pkg.native_library_dir.clone(),
        secondary_native_library_dir: pkg.secondary_native_library_dir.clone(),
    })
}

/// `PackageInfoUtils.generatePermissionInfo`.
fn generate_permission_info(p: &pkg::Permission, flags: i64) -> PermissionInfo {
    let mut item = PackageItemInfo::of(&p.component, &PackageUserState::default());
    item.meta_data = meta_data(&p.component, flags);
    PermissionInfo {
        item,
        protection_level: p.protection_level,
        flags: p.component.flags,
        group: p.group.clone(),
        background_permission: p.background_permission.clone(),
        description_res: p.component.description_res,
        request_res: p.request_res,
        known_certs: p.known_certs.clone(),
    }
}

/// The signing details' signatures as a reply's `SigningInfo` carries
/// them.
pub fn signing_info(pkg: &AndroidPackage) -> Option<SigningInfo> {
    let s = pkg.signing_details.as_ref()?;
    Some(SigningInfo {
        scheme_version: s.scheme_version,
        signatures: s.signatures.clone().unwrap_or_default(),
        public_keys: s.public_keys.clone(),
        past_signing_certificates: s.past_signing_certificates.clone(),
    })
}

/// `getDeprecatedSignatures`: the oldest certificate of a rotation, else
/// the signers.
fn deprecated_signatures(s: Option<&SigningInfo>, flags: i64) -> Option<Vec<Vec<u8>>> {
    if flags & GET_SIGNATURES == 0 {
        return None;
    }
    let s = s?;
    match &s.past_signing_certificates {
        Some(past) if !past.is_empty() => Some(vec![past[0].clone()]),
        _ if !s.signatures.is_empty() => Some(s.signatures.clone()),
        _ => None,
    }
}

/// What `PackageInfoUtils.generate` takes beside the package: the times,
/// the permission state's gids and the installed and granted
/// permissions, and the name the caller knows the package by
/// (`resolveExternalPackageName`).
pub struct Extras<'a> {
    pub first_install_time: i64,
    pub last_update_time: i64,
    pub gids: &'a [i32],
    pub installed_permissions: &'a [String],
    pub granted_permissions: &'a [String],
    pub external_name: &'a str,
}

/// `PackageInfoUtils.generate` (`generateWithComponents`).
pub fn generate_package_info(t: &Target<'_>, x: &Extras<'_>, flags: i64) -> Option<PackageInfo> {
    let (pkg, ps, state) = (t.pkg, t.ps, t.state);
    let mut app_info = generate_application_info(t, flags)?;
    let mut info = PackageInfo {
        package_name: Some(x.external_name.to_string()),
        split_names: pkg.split_names.clone(),
        version_code: pkg.version_code,
        version_code_major: pkg.version_code_major,
        base_revision_code: pkg.base_revision_code,
        split_revision_codes: pkg.split_revision_codes.clone(),
        version_name: pkg.version_name.clone(),
        shared_user_id: pkg.shared_user_id.clone(),
        shared_user_label: pkg.shared_user_label,
        install_location: pkg.install_location,
        required_for_all_users: app_info.flags & (FLAG_SYSTEM | FLAG_UPDATED_SYSTEM_APP) != 0
            && pkg.is(booleans::REQUIRED_FOR_ALL_USERS),
        restricted_account_type: pkg.restricted_account_type.clone(),
        required_account_type: pkg.required_account_type.clone(),
        overlay_target: pkg.overlay_target.clone(),
        overlay_category: pkg.overlay_category.clone(),
        overlay_priority: pkg.overlay_priority,
        overlay_is_static: pkg.is(booleans::OVERLAY_IS_STATIC),
        compile_sdk_version: pkg.compile_sdk_version,
        compile_sdk_version_codename: pkg.compile_sdk_version_code_name.clone(),
        first_install_time: x.first_install_time,
        last_update_time: x.last_update_time,
        archive_time_millis: state.archive_state.as_ref().map_or(0, |a| a.archive_time),
        gids: (flags & GET_GIDS != 0).then(|| x.gids.to_vec()),
        ..PackageInfo::default()
    };
    if flags & GET_CONFIGURATIONS != 0 {
        info.config_preferences = pkg.config_preferences.clone().filter(|v| !v.is_empty());
        info.req_features = pkg.req_features.clone().filter(|v| !v.is_empty());
        info.feature_groups = pkg.feature_groups.clone().filter(|v| !v.is_empty());
    }
    if flags & GET_PERMISSIONS != 0 {
        if !pkg.permissions.is_empty() {
            info.permissions = Some(
                pkg.permissions
                    .iter()
                    .map(|p| {
                        let mut pi = generate_permission_info(p, flags);
                        if x.installed_permissions.contains(&p.component.name) {
                            pi.flags |= PERMISSION_FLAG_INSTALLED;
                        }
                        pi
                    })
                    .collect(),
            );
        }
        if !pkg.uses_permissions.is_empty() {
            let (names, flags): (Vec<String>, Vec<i32>) = pkg
                .uses_permissions
                .iter()
                .map(|u| {
                    let name = u.name.clone().unwrap_or_default();
                    let f = REQUESTED_PERMISSION_REQUIRED
                        | flag(
                            x.granted_permissions.contains(&name),
                            REQUESTED_PERMISSION_GRANTED,
                        )
                        | flag(
                            u.flags & USES_PERMISSION_NEVER_FOR_LOCATION != 0,
                            REQUESTED_PERMISSION_NEVER_FOR_LOCATION,
                        )
                        | flag(
                            pkg.implicit_permissions.contains(&name),
                            REQUESTED_PERMISSION_IMPLICIT,
                        );
                    (name, f)
                })
                .unzip();
            info.requested_permissions = Some(names);
            info.requested_permissions_flags = Some(flags);
        }
    }
    if flags & GET_ATTRIBUTIONS_LONG != 0 && !pkg.attributions.is_empty() {
        info.attributions = Some(
            pkg.attributions
                .iter()
                .map(|a| (a.tag.clone(), a.label))
                .collect(),
        );
    }
    if flags & GET_ATTRIBUTIONS_LONG != 0 && pkg.is(booleans::ATTRIBUTIONS_ARE_USER_VISIBLE) {
        app_info.private_flags_ext |= PRIVATE_FLAG_EXT_ATTRIBUTIONS_ARE_USER_VISIBLE;
    } else {
        app_info.private_flags_ext &= !PRIVATE_FLAG_EXT_ATTRIBUTIONS_ARE_USER_VISIBLE;
    }
    // The components share this object, as the original's do.
    let app = Arc::new(app_info);
    let signing = signing_info(pkg);
    info.signatures = deprecated_signatures(signing.as_ref(), flags);
    if flags & GET_SIGNING_CERTIFICATES != 0 {
        info.signing_info = signing;
    }
    info.is_stub = pkg.is2(booleans2::STUB);
    info.core_app = pkg.is(booleans::CORE_APP);
    info.is_apex = pkg.is2(booleans2::APEX);
    if ps.shared_user.is_none() {
        info.shared_user_id = None;
        info.shared_user_label = 0;
    }
    let enabled = pkg.is(booleans::ENABLED);
    let matches = |c: &MainComponent, flags: i64| is_match(state, ps.is.system, enabled, c, flags);
    if flags & GET_ACTIVITIES != 0 && !pkg.activities.is_empty() {
        let aflags = flags | MATCH_QUARANTINED_COMPONENTS;
        info.activities = Some(
            pkg.activities
                .iter()
                .filter(|a| matches(&a.main, aflags))
                .filter(|a| a.main.component.name != APP_DETAILS_ACTIVITY_CLASS_NAME)
                .filter_map(|a| generate_activity_info(t, a, aflags, Some(app.clone())))
                .collect(),
        );
    }
    if flags & GET_RECEIVERS != 0 && !pkg.receivers.is_empty() {
        info.receivers = Some(
            pkg.receivers
                .iter()
                .filter(|a| matches(&a.main, flags))
                .filter_map(|a| generate_activity_info(t, a, flags, Some(app.clone())))
                .collect(),
        );
    }
    if flags & GET_SERVICES != 0 && !pkg.services.is_empty() {
        info.services = Some(
            pkg.services
                .iter()
                .filter(|s| matches(&s.main, flags))
                .filter_map(|s| generate_service_info(t, s, flags, Some(app.clone())))
                .collect(),
        );
    }
    if flags & GET_PROVIDERS != 0 && !pkg.providers.is_empty() {
        info.providers = Some(
            pkg.providers
                .iter()
                .filter(|pr| matches(&pr.main, flags))
                .filter_map(|pr| generate_provider_info(t, pr, flags, Some(app.clone())))
                .collect(),
        );
    }
    if flags & GET_INSTRUMENTATION != 0 && !pkg.instrumentations.is_empty() {
        info.instrumentation = pkg
            .instrumentations
            .iter()
            .map(|i| generate_instrumentation_info(t, i, flags))
            .collect();
    }
    // The external name is set on the shared object once the components
    // are made, as the original renames it after `generate`.
    let mut renamed = (*app).clone();
    renamed.item.package_name = Some(x.external_name.to_string());
    let app = Arc::new(renamed);
    let share = |c: &mut ComponentInfo| c.application_info = app.clone();
    info.activities
        .iter_mut()
        .flatten()
        .for_each(|a| share(&mut a.info));
    info.receivers
        .iter_mut()
        .flatten()
        .for_each(|a| share(&mut a.info));
    info.services
        .iter_mut()
        .flatten()
        .for_each(|s| share(&mut s.info));
    info.providers
        .iter_mut()
        .flatten()
        .for_each(|p| share(&mut p.info));
    info.application_info = Some(app);
    Some(info)
}
/// `InstallSourceInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstallSourceInfo {
    pub initiating_package_name: Option<String>,
    pub initiating_package_signing_info: Option<SigningInfo>,
    pub originating_package_name: Option<String>,
    pub installing_package_name: Option<String>,
    pub update_owner_package_name: Option<String>,
    pub package_source: i32,
}

impl WriteParcelable for InstallSourceInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.initiating_package_name.as_deref());
        match &self.initiating_package_signing_info {
            Some(s) => {
                p.write_string16(Some("android.content.pm.SigningInfo"));
                write_signing_details(p, Some(s));
            }
            None => p.write_string16(None),
        }
        p.write_string16(self.originating_package_name.as_deref());
        p.write_string16(self.installing_package_name.as_deref());
        p.write_string8(self.update_owner_package_name.as_deref());
        p.write_i32(self.package_source);
    }
}
