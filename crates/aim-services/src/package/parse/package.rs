//! A parsed package (`PackageImpl`) and its `writeToParcel`, the parser
//! cache's format.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `com.android.internal.pm.parsing.pkg.PackageImpl`), Copyright (C) The
//! Android Open Source Project, Licensed under the Apache License, Version
//! 2.0.

use super::super::intent_filter::ParsedIntentInfo;
use super::component::{
    Activity, ApexSystemService, Attribution, Instrumentation, PROCESS_CLASS, PROPERTY_CLASS,
    Permission, PermissionGroup, Process, Property, Provider, Service, UsesPermission,
    write_intent_info,
};
use super::parcel::{ArrayMap, ArraySet, Bundle, Entry, Writer};

/// `PackageImpl.Booleans`.
pub mod booleans {
    pub const EXTERNAL_STORAGE: u64 = 1;
    pub const HARDWARE_ACCELERATED: u64 = 1 << 1;
    pub const ALLOW_BACKUP: u64 = 1 << 2;
    pub const KILL_AFTER_RESTORE: u64 = 1 << 3;
    pub const RESTORE_ANY_VERSION: u64 = 1 << 4;
    pub const FULL_BACKUP_ONLY: u64 = 1 << 5;
    pub const PERSISTENT: u64 = 1 << 6;
    pub const DEBUGGABLE: u64 = 1 << 7;
    pub const VM_SAFE_MODE: u64 = 1 << 8;
    pub const HAS_CODE: u64 = 1 << 9;
    pub const ALLOW_TASK_REPARENTING: u64 = 1 << 10;
    pub const ALLOW_CLEAR_USER_DATA: u64 = 1 << 11;
    pub const LARGE_HEAP: u64 = 1 << 12;
    pub const USES_CLEARTEXT_TRAFFIC: u64 = 1 << 13;
    pub const SUPPORTS_RTL: u64 = 1 << 14;
    pub const TEST_ONLY: u64 = 1 << 15;
    pub const MULTI_ARCH: u64 = 1 << 16;
    pub const EXTRACT_NATIVE_LIBS: u64 = 1 << 17;
    pub const GAME: u64 = 1 << 18;
    pub const STATIC_SHARED_LIBRARY: u64 = 1 << 19;
    pub const OVERLAY: u64 = 1 << 20;
    pub const ISOLATED_SPLIT_LOADING: u64 = 1 << 21;
    pub const HAS_DOMAIN_URLS: u64 = 1 << 22;
    pub const PROFILEABLE_BY_SHELL: u64 = 1 << 23;
    pub const BACKUP_IN_FOREGROUND: u64 = 1 << 24;
    pub const USE_EMBEDDED_DEX: u64 = 1 << 25;
    pub const DEFAULT_TO_DEVICE_PROTECTED_STORAGE: u64 = 1 << 26;
    pub const DIRECT_BOOT_AWARE: u64 = 1 << 27;
    pub const PARTIALLY_DIRECT_BOOT_AWARE: u64 = 1 << 28;
    pub const RESIZEABLE_ACTIVITY_VIA_SDK_VERSION: u64 = 1 << 29;
    pub const ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE: u64 = 1 << 30;
    pub const ALLOW_AUDIO_PLAYBACK_CAPTURE: u64 = 1 << 31;
    pub const REQUEST_LEGACY_EXTERNAL_STORAGE: u64 = 1 << 32;
    pub const USES_NON_SDK_API: u64 = 1 << 33;
    pub const HAS_FRAGILE_USER_DATA: u64 = 1 << 34;
    pub const CANT_SAVE_STATE: u64 = 1 << 35;
    pub const ALLOW_NATIVE_HEAP_POINTER_TAGGING: u64 = 1 << 36;
    pub const PRESERVE_LEGACY_EXTERNAL_STORAGE: u64 = 1 << 37;
    pub const REQUIRED_FOR_ALL_USERS: u64 = 1 << 38;
    pub const OVERLAY_IS_STATIC: u64 = 1 << 39;
    pub const USE_32_BIT_ABI: u64 = 1 << 40;
    pub const VISIBLE_TO_INSTANT_APPS: u64 = 1 << 41;
    pub const FORCE_QUERYABLE: u64 = 1 << 42;
    pub const CROSS_PROFILE: u64 = 1 << 43;
    pub const ENABLED: u64 = 1 << 44;
    pub const DISALLOW_PROFILING: u64 = 1 << 45;
    pub const REQUEST_FOREGROUND_SERVICE_EXEMPTION: u64 = 1 << 46;
    pub const ATTRIBUTIONS_ARE_USER_VISIBLE: u64 = 1 << 47;
    pub const RESET_ENABLED_SETTINGS_ON_APP_DATA_CLEARED: u64 = 1 << 48;
    pub const SDK_LIBRARY: u64 = 1 << 49;
    pub const ENABLE_ON_BACK_INVOKED_CALLBACK: u64 = 1 << 50;
    pub const LEAVING_SHARED_UID: u64 = 1 << 51;
    pub const CORE_APP: u64 = 1 << 52;
}

/// `PackageImpl.Booleans2`.
pub mod booleans2 {
    pub const UPDATABLE_SYSTEM: u64 = 1 << 2;
}

/// `FeatureInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureInfo {
    pub name: Option<String>,
    pub version: i32,
    pub req_gl_es_version: i32,
    pub flags: i32,
}

impl FeatureInfo {
    fn write(&self, w: &mut Writer) {
        w.string(self.name.as_deref());
        w.int(self.version);
        w.int(self.req_gl_es_version);
        w.int(self.flags);
    }
}

/// `ConfigurationInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigurationInfo {
    pub req_touch_screen: i32,
    pub req_keyboard_type: i32,
    pub req_navigation: i32,
    pub req_input_features: i32,
    pub req_gl_es_version: i32,
}

/// An `Intent` of `<queries>`: what the parser sets of one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueriesIntent {
    pub action: Option<String>,
    /// `Uri.toString` of the data, a `HierarchicalUri`.
    pub data: Option<String>,
    pub data_type: Option<String>,
    pub categories: Option<ArraySet>,
}

/// `Uri.HierarchicalUri.TYPE_ID`, `UserHandle.USER_CURRENT`.
const HIERARCHICAL_URI: i32 = 3;
const USER_CURRENT: i32 = -2;

/// The parsed package. Fields keep `PackageImpl`'s names; a `None` is
/// Java's null where the original keeps one.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub feature_flag_state: ArrayMap<Option<bool>>,
    pub supports_small_screens: Option<bool>,
    pub supports_normal_screens: Option<bool>,
    pub supports_large_screens: Option<bool>,
    pub supports_extra_large_screens: Option<bool>,
    pub resizeable: Option<bool>,
    pub any_density: Option<bool>,
    pub version_code: i32,
    pub version_code_major: i32,
    pub base_revision_code: i32,
    pub version_name: Option<String>,
    pub compile_sdk_version: i32,
    pub compile_sdk_version_code_name: Option<String>,
    pub package_name: String,
    pub base_apk_path: String,
    pub restricted_account_type: Option<String>,
    pub required_account_type: Option<String>,
    pub emergency_installer: Option<String>,
    pub overlay_target: Option<String>,
    pub overlay_target_overlayable_name: Option<String>,
    pub overlay_category: Option<String>,
    pub overlay_priority: i32,
    pub overlayables: ArrayMap<String>,
    pub sdk_library_name: Option<String>,
    pub sdk_lib_version_major: i32,
    pub static_shared_library_name: Option<String>,
    pub static_shared_lib_version: i64,
    pub library_names: Vec<String>,
    pub uses_libraries: Vec<String>,
    pub uses_optional_libraries: Vec<String>,
    pub uses_native_libraries: Vec<String>,
    pub uses_optional_native_libraries: Vec<String>,
    pub uses_static_libraries: Vec<String>,
    pub uses_static_libraries_versions: Option<Vec<i64>>,
    pub uses_static_libraries_cert_digests: Option<Vec<Vec<String>>>,
    pub uses_sdk_libraries: Vec<String>,
    pub uses_sdk_libraries_versions_major: Option<Vec<i64>>,
    pub uses_sdk_libraries_cert_digests: Option<Vec<Vec<String>>>,
    pub uses_sdk_libraries_optional: Option<Vec<bool>>,
    pub shared_user_id: Option<String>,
    pub shared_user_label: i32,
    pub config_preferences: Vec<ConfigurationInfo>,
    pub req_features: Vec<FeatureInfo>,
    pub feature_groups: Vec<Option<Vec<FeatureInfo>>>,
    pub restrict_update_hash: Option<Vec<u8>>,
    pub original_packages: Vec<String>,
    pub adopt_permissions: Vec<String>,
    pub requested_permissions: ArraySet,
    pub uses_permissions: Vec<UsesPermission>,
    pub implicit_permissions: ArraySet,
    pub upgrade_key_sets: ArraySet,
    pub key_set_mapping: ArrayMap<Vec<crate::package::pkg::Serialized>>,
    pub protected_broadcasts: Vec<String>,
    pub activities: Vec<Activity>,
    pub apex_system_services: Vec<ApexSystemService>,
    pub receivers: Vec<Activity>,
    pub services: Vec<Service>,
    pub providers: Vec<Provider>,
    pub attributions: Vec<Attribution>,
    pub permissions: Vec<Permission>,
    pub permission_groups: Vec<PermissionGroup>,
    pub instrumentations: Vec<Instrumentation>,
    pub preferred_activity_filters: Vec<(String, ParsedIntentInfo)>,
    pub processes: ArrayMap<Process>,
    pub meta_data: Option<Bundle>,
    pub volume_uuid: Option<String>,
    pub path: String,
    pub queries_intents: Vec<QueriesIntent>,
    pub queries_packages: Vec<String>,
    pub queries_providers: ArraySet,
    pub app_component_factory: Option<String>,
    pub backup_agent_name: Option<String>,
    pub banner: i32,
    pub category: i32,
    pub class_loader_name: Option<String>,
    pub class_name: Option<String>,
    pub compatible_width_limit_dp: i32,
    pub description_res: i32,
    pub full_backup_content: i32,
    pub data_extraction_rules: i32,
    pub icon_res: i32,
    pub install_location: i32,
    pub label_res: i32,
    pub largest_width_limit_dp: i32,
    pub logo: i32,
    pub manage_space_activity_name: Option<String>,
    pub max_aspect_ratio: f32,
    pub min_aspect_ratio: f32,
    pub min_sdk_version: i32,
    pub max_sdk_version: i32,
    pub network_security_config_res: i32,
    pub non_localized_label: Option<String>,
    pub permission: Option<String>,
    pub process_name: Option<String>,
    pub requires_smallest_width_dp: i32,
    pub round_icon_res: i32,
    pub target_sandbox_version: i32,
    pub target_sdk_version: i32,
    pub task_affinity: Option<String>,
    pub theme: i32,
    pub ui_options: i32,
    pub zygote_preload_name: Option<String>,
    pub split_class_loader_names: Option<Vec<Option<String>>>,
    pub split_code_paths: Option<Vec<String>>,
    pub split_dependencies: Option<std::collections::BTreeMap<i32, Vec<i32>>>,
    pub split_flags: Option<Vec<i32>>,
    pub split_names: Option<Vec<String>>,
    pub split_revision_codes: Option<Vec<i32>>,
    pub resizeable_activity: Option<bool>,
    pub auto_revoke_permissions: i32,
    pub mime_groups: ArraySet,
    pub gwp_asan_mode: i32,
    pub min_extension_versions: Option<Vec<(i32, i32)>>,
    pub properties: ArrayMap<Property>,
    pub memtag_mode: i32,
    pub native_heap_zero_initialized: i32,
    pub request_raw_external_storage_access: Option<bool>,
    pub locale_config_res: i32,
    pub known_activity_embedding_certs: ArraySet,
    pub manifest_package_name: String,
    pub booleans: u64,
    pub booleans2: u64,
    pub allow_cross_uid_activity_switch_from_below: bool,
    pub intent_matching_flags: i32,
    pub alternate_launcher_icon_res_ids: Option<Vec<i32>>,
    pub alternate_launcher_label_res_ids: Option<Vec<i32>>,
    pub page_size_app_compat_flags: i32,
}

/// `ApplicationInfo.CATEGORY_UNDEFINED`,
/// `PAGE_SIZE_APP_COMPAT_FLAG_UNDEFINED`; `ParsingUtils`' defaults.
pub const CATEGORY_UNDEFINED: i32 = -1;
pub const PAGE_SIZE_APP_COMPAT_FLAG_UNDEFINED: i32 = 0;
pub const DEFAULT_MIN_SDK_VERSION: i32 = 1;
pub const DEFAULT_MAX_SDK_VERSION: i32 = i32::MAX;
pub const DEFAULT_TARGET_SDK_VERSION: i32 = 0;

impl Package {
    /// `PackageImpl`'s constructor, before the manifest's values.
    pub fn new(package_name: &str, base_apk_path: &str, path: &str) -> Package {
        Package {
            feature_flag_state: ArrayMap::default(),
            supports_small_screens: None,
            supports_normal_screens: None,
            supports_large_screens: None,
            supports_extra_large_screens: None,
            resizeable: None,
            any_density: None,
            version_code: 0,
            version_code_major: 0,
            base_revision_code: 0,
            version_name: None,
            compile_sdk_version: 0,
            compile_sdk_version_code_name: None,
            package_name: package_name.to_owned(),
            base_apk_path: base_apk_path.to_owned(),
            restricted_account_type: None,
            required_account_type: None,
            emergency_installer: None,
            overlay_target: None,
            overlay_target_overlayable_name: None,
            overlay_category: None,
            overlay_priority: 0,
            overlayables: ArrayMap::default(),
            sdk_library_name: None,
            sdk_lib_version_major: 0,
            static_shared_library_name: None,
            static_shared_lib_version: 0,
            library_names: Vec::new(),
            uses_libraries: Vec::new(),
            uses_optional_libraries: Vec::new(),
            uses_native_libraries: Vec::new(),
            uses_optional_native_libraries: Vec::new(),
            uses_static_libraries: Vec::new(),
            uses_static_libraries_versions: None,
            uses_static_libraries_cert_digests: None,
            uses_sdk_libraries: Vec::new(),
            uses_sdk_libraries_versions_major: None,
            uses_sdk_libraries_cert_digests: None,
            uses_sdk_libraries_optional: None,
            shared_user_id: None,
            shared_user_label: 0,
            config_preferences: Vec::new(),
            req_features: Vec::new(),
            feature_groups: Vec::new(),
            restrict_update_hash: None,
            original_packages: Vec::new(),
            adopt_permissions: Vec::new(),
            requested_permissions: ArraySet::default(),
            uses_permissions: Vec::new(),
            implicit_permissions: ArraySet::default(),
            upgrade_key_sets: ArraySet::default(),
            key_set_mapping: ArrayMap::default(),
            protected_broadcasts: Vec::new(),
            activities: Vec::new(),
            apex_system_services: Vec::new(),
            receivers: Vec::new(),
            services: Vec::new(),
            providers: Vec::new(),
            attributions: Vec::new(),
            permissions: Vec::new(),
            permission_groups: Vec::new(),
            instrumentations: Vec::new(),
            preferred_activity_filters: Vec::new(),
            processes: ArrayMap::default(),
            meta_data: None,
            volume_uuid: None,
            path: path.to_owned(),
            queries_intents: Vec::new(),
            queries_packages: Vec::new(),
            queries_providers: ArraySet::default(),
            app_component_factory: None,
            backup_agent_name: None,
            banner: 0,
            category: CATEGORY_UNDEFINED,
            class_loader_name: None,
            class_name: None,
            compatible_width_limit_dp: 0,
            description_res: 0,
            full_backup_content: 0,
            data_extraction_rules: 0,
            icon_res: 0,
            install_location: -1,
            label_res: 0,
            largest_width_limit_dp: 0,
            logo: 0,
            manage_space_activity_name: None,
            max_aspect_ratio: 0.0,
            min_aspect_ratio: 0.0,
            min_sdk_version: DEFAULT_MIN_SDK_VERSION,
            max_sdk_version: DEFAULT_MAX_SDK_VERSION,
            network_security_config_res: 0,
            non_localized_label: None,
            permission: None,
            process_name: None,
            requires_smallest_width_dp: 0,
            round_icon_res: 0,
            target_sandbox_version: 0,
            target_sdk_version: DEFAULT_TARGET_SDK_VERSION,
            task_affinity: None,
            theme: 0,
            ui_options: 0,
            zygote_preload_name: None,
            split_class_loader_names: None,
            split_code_paths: None,
            split_flags: None,
            split_names: None,
            split_dependencies: None,
            split_revision_codes: None,
            resizeable_activity: None,
            auto_revoke_permissions: 0,
            mime_groups: ArraySet::default(),
            gwp_asan_mode: 0,
            min_extension_versions: None,
            properties: ArrayMap::default(),
            memtag_mode: 0,
            native_heap_zero_initialized: 0,
            request_raw_external_storage_access: None,
            locale_config_res: 0,
            known_activity_embedding_certs: ArraySet::default(),
            manifest_package_name: package_name.to_owned(),
            booleans: booleans::ENABLED,
            booleans2: booleans2::UPDATABLE_SYSTEM,
            allow_cross_uid_activity_switch_from_below: false,
            intent_matching_flags: 0,
            alternate_launcher_icon_res_ids: None,
            alternate_launcher_label_res_ids: None,
            page_size_app_compat_flags: PAGE_SIZE_APP_COMPAT_FLAG_UNDEFINED,
        }
    }

    pub fn get(&self, flag: u64) -> bool {
        self.booleans & flag != 0
    }

    pub fn set(&mut self, flag: u64, value: bool) {
        if value {
            self.booleans |= flag;
        } else {
            self.booleans &= !flag;
        }
    }

    /// `addUsesPermission`.
    pub fn add_uses_permission(&mut self, p: UsesPermission) {
        self.requested_permissions.add(&p.name);
        self.uses_permissions.push(p);
    }

    /// `addImplicitPermission`.
    pub fn add_implicit_permission(&mut self, name: &str) {
        self.add_uses_permission(UsesPermission {
            name: name.to_owned(),
            flags: 0,
        });
        self.implicit_permissions.add(name);
    }

    /// `addMimeGroupsFromComponent`, for each intent filter last to first.
    pub fn add_mime_groups(&mut self, intents: &[ParsedIntentInfo]) {
        for i in intents.iter().rev() {
            for g in i.filter.mime_groups.iter().flatten().rev() {
                self.mime_groups.add(g);
            }
        }
    }

    /// The cache entry: `PackageCacher.toCacheEntryStatic`.
    pub fn to_cache_entry(&self) -> Entry {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    fn write(&self, w: &mut Writer) {
        w.field("mFeatureFlagState");
        let flags: Vec<String> = self
            .feature_flag_state
            .iter()
            .map(|(k, v)| match v {
                None => format!("{k}=?"),
                Some(true) => format!("{k}=1"),
                Some(false) => format!("{k}=0"),
            })
            .collect();
        w.strings(Some(&flags));
        for (name, v) in [
            ("supportsSmallScreens", self.supports_small_screens),
            ("supportsNormalScreens", self.supports_normal_screens),
            ("supportsLargeScreens", self.supports_large_screens),
            (
                "supportsExtraLargeScreens",
                self.supports_extra_large_screens,
            ),
            ("resizeable", self.resizeable),
            ("anyDensity", self.any_density),
        ] {
            w.field(name);
            for_boolean(w, v);
        }
        w.field("versionCode");
        w.int(self.version_code);
        w.field("versionCodeMajor");
        w.int(self.version_code_major);
        w.field("baseRevisionCode");
        w.int(self.base_revision_code);
        w.field("versionName");
        w.string(self.version_name.as_deref());
        w.field("compileSdkVersion");
        w.int(self.compile_sdk_version);
        w.field("compileSdkVersionCodeName");
        w.string(self.compile_sdk_version_code_name.as_deref());
        w.field("packageName");
        w.string(Some(&self.package_name));
        w.field("mBaseApkPath");
        w.string(Some(&self.base_apk_path));
        w.field("restrictedAccountType");
        w.string(self.restricted_account_type.as_deref());
        w.field("requiredAccountType");
        w.string(self.required_account_type.as_deref());
        w.field("mEmergencyInstaller");
        w.string(self.emergency_installer.as_deref());
        w.field("overlayTarget");
        w.string(self.overlay_target.as_deref());
        w.field("overlayTargetOverlayableName");
        w.string(self.overlay_target_overlayable_name.as_deref());
        w.field("overlayCategory");
        w.string(self.overlay_category.as_deref());
        w.field("overlayPriority");
        w.int(self.overlay_priority);
        w.field("overlayables");
        w.string_map(&self.overlayables);
        w.field("sdkLibraryName");
        w.string(self.sdk_library_name.as_deref());
        w.field("sdkLibVersionMajor");
        w.int(self.sdk_lib_version_major);
        w.field("staticSharedLibraryName");
        w.string(self.static_shared_library_name.as_deref());
        w.field("staticSharedLibVersion");
        w.long(self.static_shared_lib_version);
        for (name, v) in [
            ("libraryNames", &self.library_names),
            ("usesLibraries", &self.uses_libraries),
            ("usesOptionalLibraries", &self.uses_optional_libraries),
            ("usesNativeLibraries", &self.uses_native_libraries),
            (
                "usesOptionalNativeLibraries",
                &self.uses_optional_native_libraries,
            ),
            ("usesStaticLibraries", &self.uses_static_libraries),
        ] {
            w.field(name);
            w.strings(Some(v));
        }
        w.field("usesStaticLibrariesVersions");
        w.longs(self.uses_static_libraries_versions.as_deref());
        w.field("usesStaticLibrariesCertDigests");
        digests(w, self.uses_static_libraries_cert_digests.as_deref());
        w.field("usesSdkLibraries");
        w.strings(Some(&self.uses_sdk_libraries));
        w.field("usesSdkLibrariesVersionsMajor");
        w.longs(self.uses_sdk_libraries_versions_major.as_deref());
        w.field("usesSdkLibrariesCertDigests");
        digests(w, self.uses_sdk_libraries_cert_digests.as_deref());
        w.field("usesSdkLibrariesOptional");
        w.bools(self.uses_sdk_libraries_optional.as_deref());
        w.field("sharedUserId");
        w.string(self.shared_user_id.as_deref());
        w.field("sharedUserLabel");
        w.int(self.shared_user_label);
        w.field("configPreferences");
        w.list("configPreferences", &self.config_preferences, |w, c| {
            w.int(c.req_touch_screen);
            w.int(c.req_keyboard_type);
            w.int(c.req_navigation);
            w.int(c.req_input_features);
            w.int(c.req_gl_es_version);
        });
        w.field("reqFeatures");
        w.list("reqFeatures", &self.req_features, |w, f| f.write(w));
        w.field("featureGroups");
        w.list("featureGroups", &self.feature_groups, |w, g| match g {
            // `writeTypedArray`.
            Some(features) => w.list("features", features, |w, f| f.write(w)),
            None => w.int(-1),
        });
        w.field("restrictUpdateHash");
        w.bytes(self.restrict_update_hash.as_deref());
        w.field("originalPackages");
        w.strings(Some(&self.original_packages));
        w.field("adoptPermissions");
        w.strings(Some(&self.adopt_permissions));
        w.field("requestedPermissions");
        w.set(&self.requested_permissions);
        w.field("usesPermissions");
        w.list("usesPermissions", &self.uses_permissions, |w, p| p.write(w));
        w.field("implicitPermissions");
        w.set(&self.implicit_permissions);
        w.field("upgradeKeySets");
        w.set(&self.upgrade_key_sets);
        w.field("keySetMapping");
        w.int(self.key_set_mapping.len() as i32);
        for (alias, keys) in self.key_set_mapping.iter() {
            w.string(Some(alias));
            w.int(keys.len() as i32);
            for key in keys {
                w.string(Some(&key.class));
                w.bytes(Some(&key.bytes));
            }
        }
        w.field("protectedBroadcasts");
        w.strings(Some(&self.protected_broadcasts));
        w.field("activities");
        w.list("activities", &self.activities, |w, a| a.write(w));
        w.field("apexSystemServices");
        w.list("apexSystemServices", &self.apex_system_services, |w, s| {
            s.write(w)
        });
        w.field("receivers");
        w.list("receivers", &self.receivers, |w, a| a.write(w));
        w.field("services");
        w.list("services", &self.services, |w, s| s.write(w));
        w.field("providers");
        w.list("providers", &self.providers, |w, p| p.write(w));
        w.field("attributions");
        w.list("attributions", &self.attributions, |w, a| a.write(w));
        w.field("permissions");
        w.list("permissions", &self.permissions, |w, p| p.write(w));
        w.field("permissionGroups");
        w.list("permissionGroups", &self.permission_groups, |w, g| {
            g.write(w)
        });
        w.field("instrumentations");
        w.list("instrumentations", &self.instrumentations, |w, i| {
            i.write(w)
        });
        w.field("preferredActivityFilters");
        w.int(self.preferred_activity_filters.len() as i32);
        for (i, (class, info)) in self.preferred_activity_filters.iter().enumerate() {
            w.scope(format!("preferredActivityFilters[{i}]"), |w| {
                w.string(Some(class));
                w.string(Some(
                    "com.android.internal.pm.pkg.component.ParsedIntentInfoImpl",
                ));
                write_intent_info(w, info);
            });
        }
        w.field("processes");
        w.parcelable_map(PROCESS_CLASS, &self.processes, |w, p| p.write(w));
        w.field("metaData");
        w.bundle(self.meta_data.as_ref());
        w.field("volumeUuid");
        w.string(self.volume_uuid.as_deref());
        // `SigningDetails.UNKNOWN`: the parser collects no certificates for
        // the cache.
        w.field("signingDetails");
        w.string(Some("android.content.pm.SigningDetails"));
        w.bool(true);
        w.field("mPath");
        w.string(Some(&self.path));
        w.field("queriesIntents");
        w.list("queriesIntents", &self.queries_intents, |w, i| {
            write_intent(w, i)
        });
        w.field("queriesPackages");
        w.strings(Some(&self.queries_packages));
        w.field("queriesProviders");
        w.set(&self.queries_providers);
        w.field("appComponentFactory");
        w.string(self.app_component_factory.as_deref());
        w.field("backupAgentName");
        w.string(self.backup_agent_name.as_deref());
        w.field("banner");
        w.int(self.banner);
        w.field("category");
        w.int(self.category);
        w.field("classLoaderName");
        w.string(self.class_loader_name.as_deref());
        w.field("className");
        w.string(self.class_name.as_deref());
        for (name, v) in [
            ("compatibleWidthLimitDp", self.compatible_width_limit_dp),
            ("descriptionRes", self.description_res),
            ("fullBackupContent", self.full_backup_content),
            ("dataExtractionRules", self.data_extraction_rules),
            ("iconRes", self.icon_res),
            ("installLocation", self.install_location),
            ("labelRes", self.label_res),
            ("largestWidthLimitDp", self.largest_width_limit_dp),
            ("logo", self.logo),
        ] {
            w.field(name);
            w.int(v);
        }
        w.field("manageSpaceActivityName");
        w.string(self.manage_space_activity_name.as_deref());
        w.field("maxAspectRatio");
        w.float(self.max_aspect_ratio);
        w.field("minAspectRatio");
        w.float(self.min_aspect_ratio);
        w.field("minSdkVersion");
        w.int(self.min_sdk_version);
        w.field("maxSdkVersion");
        w.int(self.max_sdk_version);
        w.field("networkSecurityConfigRes");
        w.int(self.network_security_config_res);
        w.field("nonLocalizedLabel");
        w.char_sequence(self.non_localized_label.as_deref());
        w.field("permission");
        w.string(self.permission.as_deref());
        w.field("processName");
        w.string(self.process_name.as_deref());
        w.field("requiresSmallestWidthDp");
        w.int(self.requires_smallest_width_dp);
        w.field("roundIconRes");
        w.int(self.round_icon_res);
        w.field("targetSandboxVersion");
        w.int(self.target_sandbox_version);
        w.field("targetSdkVersion");
        w.int(self.target_sdk_version);
        w.field("taskAffinity");
        w.string(self.task_affinity.as_deref());
        w.field("theme");
        w.int(self.theme);
        w.field("uiOptions");
        w.int(self.ui_options);
        w.field("zygotePreloadName");
        w.string(self.zygote_preload_name.as_deref());
        w.field("splitClassLoaderNames");
        match &self.split_class_loader_names {
            Some(v) => {
                w.int(v.len() as i32);
                v.iter().for_each(|s| w.string(s.as_deref()));
            }
            None => w.int(-1),
        }
        w.field("splitCodePaths");
        w.strings(self.split_code_paths.as_deref());
        w.field("splitDependencies");
        match &self.split_dependencies {
            None => w.int(-1),
            Some(deps) => {
                w.int(deps.len() as i32);
                for (index, values) in deps {
                    w.int(*index);
                    w.int(18); // Parcel.VAL_INTARRAY
                    w.ints(Some(values));
                }
            }
        }
        w.field("splitFlags");
        w.ints(self.split_flags.as_deref());
        w.field("splitNames");
        w.strings(self.split_names.as_deref());
        w.field("splitRevisionCodes");
        w.ints(self.split_revision_codes.as_deref());
        w.field("resizeableActivity");
        for_boolean(w, self.resizeable_activity);
        w.field("autoRevokePermissions");
        w.int(self.auto_revoke_permissions);
        w.field("mimeGroups");
        w.set(&self.mime_groups);
        w.field("gwpAsanMode");
        w.int(self.gwp_asan_mode);
        w.field("minExtensionVersions");
        match &self.min_extension_versions {
            Some(v) => {
                w.int(v.len() as i32);
                for &(k, m) in v {
                    w.int(k);
                    w.int(m);
                }
            }
            None => w.int(-1),
        }
        w.field("mProperties");
        w.parcelable_map(PROPERTY_CLASS, &self.properties, |w, p| p.write(w));
        w.field("memtagMode");
        w.int(self.memtag_mode);
        w.field("nativeHeapZeroInitialized");
        w.int(self.native_heap_zero_initialized);
        w.field("requestRawExternalStorageAccess");
        for_boolean(w, self.request_raw_external_storage_access);
        w.field("mLocaleConfigRes");
        w.int(self.locale_config_res);
        w.field("mKnownActivityEmbeddingCerts");
        w.set(&self.known_activity_embedding_certs);
        w.field("manifestPackageName");
        w.string(Some(&self.manifest_package_name));
        // What the scan sets later: native library paths, ABIs, the uid.
        w.field("nativeLibraryDir");
        w.string(None);
        w.field("nativeLibraryRootDir");
        w.string(None);
        w.field("nativeLibraryRootRequiresIsa");
        w.bool(false);
        w.field("primaryCpuAbi");
        w.string(None);
        w.field("secondaryCpuAbi");
        w.string(None);
        w.field("secondaryNativeLibraryDir");
        w.string(None);
        w.field("uid");
        w.int(-1);
        w.field("mBooleans");
        w.long(self.booleans as i64);
        w.field("mBooleans2");
        w.long(self.booleans2 as i64);
        w.field("mAllowCrossUidActivitySwitchFromBelow");
        w.bool(self.allow_cross_uid_activity_switch_from_below);
        w.field("mIntentMatchingFlags");
        w.int(self.intent_matching_flags);
        w.field("mAlternateLauncherIconResIds");
        w.ints(self.alternate_launcher_icon_res_ids.as_deref());
        w.field("mAlternateLauncherLabelResIds");
        w.ints(self.alternate_launcher_label_res_ids.as_deref());
        w.field("mPageSizeAppCompatFlags");
        w.int(self.page_size_app_compat_flags);
    }
}

/// `Parcelling.BuiltIn.ForBoolean`: null is 1, false 0, true -1.
fn for_boolean(w: &mut Writer, v: Option<bool>) {
    w.int(match v {
        None => 1,
        Some(false) => 0,
        Some(true) => -1,
    });
}

/// The certificate digests of each used library, `writeStringArray` each.
fn digests(w: &mut Writer, v: Option<&[Vec<String>]>) {
    let Some(v) = v else { return w.int(-1) };
    w.int(v.len() as i32);
    for d in v {
        w.strings(Some(d));
    }
}

/// `Intent.writeToParcel` of a `<queries>` intent.
fn write_intent(w: &mut Writer, i: &QueriesIntent) {
    w.field("mAction");
    w.string(i.action.as_deref());
    w.field("mData");
    match &i.data {
        Some(uri) => {
            w.int(HIERARCHICAL_URI);
            w.string(Some(uri));
        }
        None => w.int(0),
    }
    w.field("mType");
    w.string(i.data_type.as_deref());
    w.field("mIdentifier");
    w.string(None);
    w.field("mFlags");
    w.int(0);
    w.field("mExtendedFlags");
    w.int(0);
    w.field("mPackage");
    w.string(None);
    w.field("mComponent");
    w.string(None);
    w.field("mSourceBounds");
    w.int(0);
    w.field("mCategories");
    match &i.categories {
        Some(c) => w.set(c),
        None => w.int(0),
    }
    w.field("mSelector");
    w.int(0);
    w.field("mClipData");
    w.int(0);
    w.field("mContentUserHint");
    w.int(USER_CURRENT);
    w.field("mExtras");
    w.bundle(None);
    w.field("mOriginalIntent");
    w.int(0);
    w.field("mCreatorTokenInfo");
    w.int(0);
}
