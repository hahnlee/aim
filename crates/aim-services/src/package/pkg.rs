//! `AndroidPackage` (`PackageImpl`) as its parcel carries it, with its
//! components: what the original parsed from an APK and adjusted at its
//! scan. The feed hands the native model each package as
//! `PackageCacher.toCacheEntryStatic` writes it (the parser cache's
//! format, strings through `PackageParserCacheHelper`'s pool), and the
//! parser cache under `/data/system/package_cache` holds the same.
//! `PackageImpl(Parcel)` and its components' parcel constructors at
//! `android-16.0.0_r1` are the reference. Every field the parcel holds
//! is kept, so the package can be written back as it was read (#723).

use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};

use super::intent::Intent;
use super::intent_filter::{ParsedIntentInfo, PatternMatcher, Strings, read_char_sequence};

/// `Parcel.VAL_*`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;
const VAL_PARCELABLE: i32 = 4;
const VAL_LONG: i32 = 6;
const VAL_FLOAT: i32 = 7;
const VAL_DOUBLE: i32 = 8;
const VAL_BOOLEAN: i32 = 9;
const VAL_INTARRAY: i32 = 18;
const VAL_SERIALIZABLE: i32 = 21;

/// `BaseBundle.BUNDLE_MAGIC` and `BUNDLE_MAGIC_NATIVE`.
const BUNDLE_MAGIC: i32 = 0x4C44_4E42;
const BUNDLE_MAGIC_NATIVE: i32 = 0x4C44_4E44;

/// `PackageImpl.Booleans`: bits of [`AndroidPackage::booleans`].
pub mod booleans {
    pub const EXTERNAL_STORAGE: i64 = 1;
    pub const HARDWARE_ACCELERATED: i64 = 1 << 1;
    pub const ALLOW_BACKUP: i64 = 1 << 2;
    pub const KILL_AFTER_RESTORE: i64 = 1 << 3;
    pub const RESTORE_ANY_VERSION: i64 = 1 << 4;
    pub const FULL_BACKUP_ONLY: i64 = 1 << 5;
    pub const PERSISTENT: i64 = 1 << 6;
    pub const DEBUGGABLE: i64 = 1 << 7;
    pub const VM_SAFE_MODE: i64 = 1 << 8;
    pub const HAS_CODE: i64 = 1 << 9;
    pub const ALLOW_TASK_REPARENTING: i64 = 1 << 10;
    pub const ALLOW_CLEAR_USER_DATA: i64 = 1 << 11;
    pub const LARGE_HEAP: i64 = 1 << 12;
    pub const USES_CLEARTEXT_TRAFFIC: i64 = 1 << 13;
    pub const SUPPORTS_RTL: i64 = 1 << 14;
    pub const TEST_ONLY: i64 = 1 << 15;
    pub const MULTI_ARCH: i64 = 1 << 16;
    pub const EXTRACT_NATIVE_LIBS: i64 = 1 << 17;
    pub const GAME: i64 = 1 << 18;
    pub const STATIC_SHARED_LIBRARY: i64 = 1 << 19;
    pub const OVERLAY: i64 = 1 << 20;
    pub const ISOLATED_SPLIT_LOADING: i64 = 1 << 21;
    pub const HAS_DOMAIN_URLS: i64 = 1 << 22;
    pub const PROFILEABLE_BY_SHELL: i64 = 1 << 23;
    pub const BACKUP_IN_FOREGROUND: i64 = 1 << 24;
    pub const USE_EMBEDDED_DEX: i64 = 1 << 25;
    pub const DEFAULT_TO_DEVICE_PROTECTED_STORAGE: i64 = 1 << 26;
    pub const DIRECT_BOOT_AWARE: i64 = 1 << 27;
    pub const PARTIALLY_DIRECT_BOOT_AWARE: i64 = 1 << 28;
    pub const RESIZEABLE_ACTIVITY_VIA_SDK_VERSION: i64 = 1 << 29;
    pub const ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE: i64 = 1 << 30;
    pub const ALLOW_AUDIO_PLAYBACK_CAPTURE: i64 = 1 << 31;
    pub const REQUEST_LEGACY_EXTERNAL_STORAGE: i64 = 1 << 32;
    pub const USES_NON_SDK_API: i64 = 1 << 33;
    pub const HAS_FRAGILE_USER_DATA: i64 = 1 << 34;
    pub const CANT_SAVE_STATE: i64 = 1 << 35;
    pub const ALLOW_NATIVE_HEAP_POINTER_TAGGING: i64 = 1 << 36;
    pub const PRESERVE_LEGACY_EXTERNAL_STORAGE: i64 = 1 << 37;
    pub const REQUIRED_FOR_ALL_USERS: i64 = 1 << 38;
    pub const OVERLAY_IS_STATIC: i64 = 1 << 39;
    pub const USE_32_BIT_ABI: i64 = 1 << 40;
    pub const VISIBLE_TO_INSTANT_APPS: i64 = 1 << 41;
    pub const FORCE_QUERYABLE: i64 = 1 << 42;
    pub const CROSS_PROFILE: i64 = 1 << 43;
    pub const ENABLED: i64 = 1 << 44;
    pub const DISALLOW_PROFILING: i64 = 1 << 45;
    pub const REQUEST_FOREGROUND_SERVICE_EXEMPTION: i64 = 1 << 46;
    pub const ATTRIBUTIONS_ARE_USER_VISIBLE: i64 = 1 << 47;
    pub const RESET_ENABLED_SETTINGS_ON_APP_DATA_CLEARED: i64 = 1 << 48;
    pub const SDK_LIBRARY: i64 = 1 << 49;
    pub const ENABLE_ON_BACK_INVOKED_CALLBACK: i64 = 1 << 50;
    pub const LEAVING_SHARED_UID: i64 = 1 << 51;
    pub const CORE_APP: i64 = 1 << 52;
    pub const SYSTEM: i64 = 1 << 53;
    pub const FACTORY_TEST: i64 = 1 << 54;
    pub const SYSTEM_EXT: i64 = 1 << 56;
    pub const PRIVILEGED: i64 = 1 << 57;
    pub const OEM: i64 = 1 << 58;
    pub const VENDOR: i64 = 1 << 59;
    pub const PRODUCT: i64 = 1 << 60;
    pub const ODM: i64 = 1 << 61;
    pub const SIGNED_WITH_PLATFORM_KEY: i64 = 1 << 62;
    pub const NATIVE_LIBRARY_ROOT_REQUIRES_ISA: i64 = 1 << 63;
}

/// `PackageImpl.Booleans2`: bits of [`AndroidPackage::booleans2`].
pub mod booleans2 {
    pub const STUB: i64 = 1;
    pub const APEX: i64 = 1 << 1;
    pub const UPDATABLE_SYSTEM: i64 = 1 << 2;
}

/// The strings of a parser cache entry: `PackageParserCacheHelper`'s
/// pool, each string written as its index.
pub struct Pool(Vec<Option<String>>);

impl Strings for Pool {
    fn string16(&mut self, r: &mut Reader<'_>) -> Result<Option<String>> {
        let index = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
        self.0.get(index).cloned().ok_or(BAD_VALUE)
    }

    fn string8(&mut self, r: &mut Reader<'_>) -> Result<Option<String>> {
        self.string16(r)
    }
}

/// A `Bundle` of meta-data, in its entries' order: the values a manifest
/// gives (`<meta-data>`: a string, an int or a resource id, a boolean, a
/// float).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaData(pub Vec<(String, Value)>);

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    String(Option<String>),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
}

/// `splitDependencies`: a split's index and the indices it depends on.
pub type SplitDependencies = Vec<(i32, Option<Vec<i32>>)>;

/// `ParsedComponent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub name: String,
    pub icon: i32,
    pub label_res: i32,
    pub non_localized_label: Option<String>,
    pub logo: i32,
    pub banner: i32,
    pub description_res: i32,
    pub flags: i32,
    pub package_name: String,
    pub intents: Vec<ParsedIntentInfo>,
    /// `None`: none written; an empty bundle is `Some` with no entries.
    pub meta_data: Option<MetaData>,
    pub properties: Option<Vec<(String, Property)>>,
}

/// `ParsedMainComponent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MainComponent {
    pub component: Component,
    pub process_name: Option<String>,
    pub direct_boot_aware: bool,
    pub enabled: bool,
    pub exported: bool,
    pub order: i32,
    pub split_name: Option<String>,
    pub attribution_tags: Option<Vec<Option<String>>>,
    pub intent_matching_flags: i32,
}

/// `ActivityInfo.WindowLayout`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowLayout {
    pub width: i32,
    pub width_fraction: f32,
    pub height: i32,
    pub height_fraction: f32,
    pub gravity: i32,
    pub min_width: i32,
    pub min_height: i32,
    pub affinity: Option<String>,
}

/// `ParsedActivity`: an activity or a receiver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    pub main: MainComponent,
    pub theme: i32,
    pub ui_options: i32,
    pub target_activity: Option<String>,
    pub parent_activity_name: Option<String>,
    pub task_affinity: Option<String>,
    pub private_flags: i32,
    pub permission: Option<String>,
    pub launch_mode: i32,
    pub document_launch_mode: i32,
    pub max_recents: i32,
    pub config_changes: i32,
    pub soft_input_mode: i32,
    pub persistable_mode: i32,
    pub lock_task_launch_mode: i32,
    pub screen_orientation: i32,
    pub resize_mode: i32,
    pub max_aspect_ratio: Option<f32>,
    pub min_aspect_ratio: Option<f32>,
    pub supports_size_changes: bool,
    pub requested_vr_component: Option<String>,
    pub rotation_animation: i32,
    pub color_mode: i32,
    pub window_layout: Option<WindowLayout>,
    pub known_activity_embedding_certs: Option<Vec<Option<String>>>,
    pub required_display_category: Option<String>,
    pub require_content_uri_permission_from_caller: i32,
}

/// `ParsedService`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub main: MainComponent,
    pub foreground_service_type: i32,
    pub permission: Option<String>,
}

/// `PathPermission`.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPermission {
    pub pattern: PatternMatcher,
    pub read_permission: Option<String>,
    pub write_permission: Option<String>,
}

/// `ParsedProvider`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Provider {
    pub main: MainComponent,
    /// The authorities, separated by `;`.
    pub authority: Option<String>,
    pub syncable: bool,
    pub read_permission: Option<String>,
    pub write_permission: Option<String>,
    pub grant_uri_permissions: bool,
    pub force_uri_permissions: bool,
    pub multi_process: bool,
    pub init_order: i32,
    pub uri_permission_patterns: Option<Vec<PatternMatcher>>,
    pub path_permissions: Option<Vec<PathPermission>>,
}

/// `ParsedPermissionGroup`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionGroup {
    pub component: Component,
    pub request_detail_res: i32,
    pub background_request_res: i32,
    pub background_request_detail_res: i32,
    pub request_res: i32,
    pub priority: i32,
}

/// `ParsedPermission`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Permission {
    pub component: Component,
    pub background_permission: Option<String>,
    pub group: Option<String>,
    pub request_res: i32,
    pub protection_level: i32,
    pub tree: bool,
    pub parsed_permission_group: Option<PermissionGroup>,
    pub known_certs: Option<Vec<Option<String>>>,
}

/// `ParsedInstrumentation`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instrumentation {
    pub component: Component,
    pub target_package: Option<String>,
    pub target_processes: Option<String>,
    pub handle_profiling: bool,
    pub functional_test: bool,
}

/// `ParsedAttribution`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attribution {
    pub tag: Option<String>,
    pub label: i32,
    pub inherit_from: Option<Vec<Option<String>>>,
}

/// `ParsedUsesPermission`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsesPermission {
    pub name: Option<String>,
    pub flags: i32,
}

/// `ParsedProcess`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Process {
    pub use_embedded_dex: bool,
    pub name: Option<String>,
    /// The application class by package (`getAppClassNamesByPackage`).
    pub app_class_names_by_package: Vec<(String, Option<String>)>,
    pub denied_permissions: Vec<String>,
    pub gwp_asan_mode: i32,
    pub memtag_mode: i32,
    pub native_heap_zero_initialized: i32,
}

/// `PackageManager.Property`.
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub name: Option<String>,
    pub package_name: Option<String>,
    pub class_name: Option<String>,
    pub value: PropertyValue,
}

/// A property's value by `Property.TYPE_*`.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    Resource(i32),
    String(Option<String>),
    /// A type the original reads as no property.
    Unknown(i32),
}

/// A `Serializable` as a parcel carries it (`writeSerializable`): its
/// class and its Java serialization.
#[derive(Clone, Debug, PartialEq)]
pub struct Serialized {
    pub class: String,
    pub bytes: Vec<u8>,
}

/// Key set aliases and their public keys (`readKeySetMapping`).
pub type KeySetMapping = Vec<(Option<String>, Option<Vec<Option<Serialized>>>)>;

/// `ParsedApexSystemService`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApexSystemService {
    pub name: Option<String>,
    pub jar_path: Option<String>,
    pub min_sdk_version: Option<String>,
    pub max_sdk_version: Option<String>,
    pub init_order: i32,
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

/// `FeatureInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureInfo {
    pub name: Option<String>,
    pub version: i32,
    pub req_gl_es_version: i32,
    pub flags: i32,
}

/// `SigningDetails` as a parcel carries it: the capabilities of past
/// certificates are not in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigningDetails {
    pub signatures: Option<Vec<Vec<u8>>>,
    pub scheme_version: i32,
    /// The signers' public keys (`writeArraySet` of `PublicKey`s).
    pub public_keys: Option<Vec<Option<Serialized>>>,
    pub past_signing_certificates: Option<Vec<Vec<u8>>>,
}

/// `PackageImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AndroidPackage {
    /// `mFeatureFlagState` as written: `flag=1`, `flag=0` or `flag=?`.
    pub feature_flag_state: Option<Vec<Option<String>>>,
    /// `supportsSmallScreens` and the rest: `None` unset (`ForBoolean`).
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
    pub base_apk_path: Option<String>,
    pub restricted_account_type: Option<String>,
    pub required_account_type: Option<String>,
    pub emergency_installer: Option<String>,
    pub overlay_target: Option<String>,
    pub overlay_target_overlayable_name: Option<String>,
    pub overlay_category: Option<String>,
    pub overlay_priority: i32,
    /// Overlayable names and their actors.
    pub overlayables: Option<Vec<(String, Option<String>)>>,
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
    pub uses_static_libraries_cert_digests: Option<Vec<Option<Vec<Option<String>>>>>,
    pub uses_sdk_libraries: Vec<String>,
    pub uses_sdk_libraries_versions_major: Option<Vec<i64>>,
    pub uses_sdk_libraries_cert_digests: Option<Vec<Option<Vec<Option<String>>>>>,
    pub uses_sdk_libraries_optional: Option<Vec<bool>>,
    pub shared_user_id: Option<String>,
    pub shared_user_label: i32,
    pub config_preferences: Option<Vec<ConfigurationInfo>>,
    pub req_features: Option<Vec<FeatureInfo>>,
    pub feature_groups: Option<Vec<Option<Vec<FeatureInfo>>>>,
    pub restrict_update_hash: Option<Vec<u8>>,
    pub original_packages: Option<Vec<Option<String>>>,
    pub adopt_permissions: Vec<String>,
    pub requested_permissions: Vec<String>,
    pub uses_permissions: Vec<UsesPermission>,
    pub implicit_permissions: Vec<String>,
    pub upgrade_key_sets: Vec<String>,
    pub key_set_mapping: Option<KeySetMapping>,
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
    /// `<preferred>` filters: the activity's class name and its filter.
    pub preferred_activity_filters: Vec<(Option<String>, ParsedIntentInfo)>,
    pub processes: Option<Vec<Process>>,
    pub meta_data: Option<MetaData>,
    pub volume_uuid: Option<String>,
    /// `None`: `SigningDetails.UNKNOWN`.
    pub signing_details: Option<SigningDetails>,
    pub path: Option<String>,
    /// `<queries>` intents.
    pub queries_intents: Vec<Intent>,
    pub queries_packages: Vec<String>,
    pub queries_providers: Vec<String>,
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
    pub split_code_paths: Option<Vec<Option<String>>>,
    pub split_dependencies: Option<SplitDependencies>,
    pub split_flags: Option<Vec<i32>>,
    pub split_names: Option<Vec<Option<String>>>,
    pub split_revision_codes: Option<Vec<i32>>,
    pub resizeable_activity: Option<bool>,
    pub auto_revoke_permissions: i32,
    pub mime_groups: Vec<String>,
    pub gwp_asan_mode: i32,
    pub min_extension_versions: Option<Vec<(i32, i32)>>,
    pub properties: Option<Vec<(String, Property)>>,
    pub memtag_mode: i32,
    pub native_heap_zero_initialized: i32,
    pub request_raw_external_storage_access: Option<bool>,
    pub locale_config_res: i32,
    pub known_activity_embedding_certs: Option<Vec<Option<String>>>,
    pub manifest_package_name: Option<String>,
    pub native_library_dir: Option<String>,
    pub native_library_root_dir: Option<String>,
    pub native_library_root_requires_isa: bool,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub secondary_native_library_dir: Option<String>,
    pub uid: i32,
    /// [`booleans`] and [`booleans2`].
    pub booleans: i64,
    pub booleans2: i64,
    pub allow_cross_uid_activity_switch_from_below: bool,
    pub intent_matching_flags: i32,
    pub alternate_launcher_icon_res_ids: Option<Vec<i32>>,
    pub alternate_launcher_label_res_ids: Option<Vec<i32>>,
    pub page_size_app_compat_flags: i32,
}

impl AndroidPackage {
    /// A parser cache entry (`PackageCacher.toCacheEntryStatic`): the
    /// pool's position, the package, the pool.
    pub fn read_cache_entry(bytes: &[u8]) -> Result<AndroidPackage> {
        let mut r = Reader::new(bytes, &[]);
        let pool = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
        let start = r.position();
        r.set_position(pool);
        let n = r.read_i32()?;
        let strings = (0..n.max(0))
            .map(|_| r.read_string16())
            .collect::<Result<_>>()?;
        r.set_position(start);
        let pkg = AndroidPackage::read(&mut r, &mut Pool(strings))?;
        // The package ends where the pool starts.
        if r.position() != pool {
            return Err(BAD_VALUE);
        }
        Ok(pkg)
    }

    pub fn is(&self, flag: i64) -> bool {
        self.booleans & flag != 0
    }

    pub fn is2(&self, flag: i64) -> bool {
        self.booleans2 & flag != 0
    }

    /// `PackageImpl(Parcel)`.
    pub fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<AndroidPackage> {
        Ok(AndroidPackage {
            feature_flag_state: string_array(r, s)?,
            supports_small_screens: for_boolean(r)?,
            supports_normal_screens: for_boolean(r)?,
            supports_large_screens: for_boolean(r)?,
            supports_extra_large_screens: for_boolean(r)?,
            resizeable: for_boolean(r)?,
            any_density: for_boolean(r)?,
            version_code: r.read_i32()?,
            version_code_major: r.read_i32()?,
            base_revision_code: r.read_i32()?,
            version_name: s.string16(r)?,
            compile_sdk_version: r.read_i32()?,
            compile_sdk_version_code_name: s.string16(r)?,
            package_name: s.string16(r)?.ok_or(BAD_VALUE)?,
            base_apk_path: s.string16(r)?,
            restricted_account_type: s.string16(r)?,
            required_account_type: s.string16(r)?,
            emergency_installer: s.string16(r)?,
            overlay_target: s.string16(r)?,
            overlay_target_overlayable_name: s.string16(r)?,
            overlay_category: s.string16(r)?,
            overlay_priority: r.read_i32()?,
            overlayables: array(r, |r| {
                let key = string_value(r, s)?.ok_or(BAD_VALUE)?;
                Ok((key, string_value(r, s)?))
            })?,
            sdk_library_name: s.string16(r)?,
            sdk_lib_version_major: r.read_i32()?,
            static_shared_library_name: s.string16(r)?,
            static_shared_lib_version: r.read_i64()?,
            library_names: string_list(r, s)?,
            uses_libraries: string_list(r, s)?,
            uses_optional_libraries: string_list(r, s)?,
            uses_native_libraries: string_list(r, s)?,
            uses_optional_native_libraries: string_list(r, s)?,
            uses_static_libraries: string_list(r, s)?,
            uses_static_libraries_versions: long_array(r)?,
            uses_static_libraries_cert_digests: array(r, |r| string_array(r, s))?,
            uses_sdk_libraries: string_list(r, s)?,
            uses_sdk_libraries_versions_major: long_array(r)?,
            uses_sdk_libraries_cert_digests: array(r, |r| string_array(r, s))?,
            uses_sdk_libraries_optional: array(r, |r| r.read_bool())?,
            shared_user_id: s.string16(r)?,
            shared_user_label: r.read_i32()?,
            config_preferences: typed_array(r, |r| {
                Ok(ConfigurationInfo {
                    req_touch_screen: r.read_i32()?,
                    req_keyboard_type: r.read_i32()?,
                    req_navigation: r.read_i32()?,
                    req_input_features: r.read_i32()?,
                    req_gl_es_version: r.read_i32()?,
                })
            })?,
            req_features: typed_array(r, |r| feature_info(r, s))?,
            feature_groups: typed_array(r, |r| typed_array(r, |r| feature_info(r, s)))?,
            restrict_update_hash: byte_array(r)?,
            original_packages: string_array(r, s)?,
            adopt_permissions: string_list(r, s)?,
            requested_permissions: string_list(r, s)?,
            uses_permissions: interface_list(r, |r| {
                Ok(UsesPermission {
                    name: s.string16(r)?,
                    flags: r.read_i32()?,
                })
            })?,
            implicit_permissions: string_list(r, s)?,
            upgrade_key_sets: string_list(r, s)?,
            key_set_mapping: array(r, |r| {
                let alias = s.string16(r)?;
                Ok((alias, array(r, |r| serializable(r, s))?))
            })?,
            protected_broadcasts: string_list(r, s)?,
            activities: interface_list(r, |r| Activity::read(r, s))?,
            apex_system_services: interface_list(r, |r| {
                // A flag byte (which of the strings are set), then all four.
                r.read_i32()?;
                Ok(ApexSystemService {
                    name: s.string16(r)?,
                    jar_path: s.string16(r)?,
                    min_sdk_version: s.string16(r)?,
                    max_sdk_version: s.string16(r)?,
                    init_order: r.read_i32()?,
                })
            })?,
            receivers: interface_list(r, |r| Activity::read(r, s))?,
            services: interface_list(r, |r| {
                Ok(Service {
                    main: MainComponent::read(r, s)?,
                    foreground_service_type: r.read_i32()?,
                    permission: s.string16(r)?,
                })
            })?,
            providers: interface_list(r, |r| Provider::read(r, s))?,
            attributions: interface_list(r, |r| {
                Ok(Attribution {
                    tag: s.string16(r)?,
                    label: r.read_i32()?,
                    inherit_from: string_array(r, s)?,
                })
            })?,
            permissions: interface_list(r, |r| Permission::read(r, s))?,
            permission_groups: interface_list(r, |r| PermissionGroup::read(r, s))?,
            instrumentations: interface_list(r, |r| {
                Ok(Instrumentation {
                    component: Component::read(r, s)?,
                    target_package: s.string16(r)?,
                    target_processes: s.string16(r)?,
                    handle_profiling: r.read_bool()?,
                    functional_test: r.read_bool()?,
                })
            })?,
            preferred_activity_filters: {
                let n = r.read_i32()?;
                (0..n.max(0))
                    .map(|_| {
                        let name = s.string16(r)?;
                        // `writeParcelable`: the creator's class, then the filter.
                        s.string16(r)?.ok_or(BAD_VALUE)?;
                        Ok((name, ParsedIntentInfo::read(r, s)?))
                    })
                    .collect::<Result<_>>()?
            },
            processes: processes(r, s)?,
            meta_data: bundle(r, s)?,
            volume_uuid: s.string16(r)?,
            signing_details: signing_details(r, s)?,
            path: s.string16(r)?,
            queries_intents: typed_array(r, |r| Intent::read(r, s))?.unwrap_or_default(),
            queries_packages: string_list(r, s)?,
            queries_providers: string_list(r, s)?,
            app_component_factory: s.string16(r)?,
            backup_agent_name: s.string16(r)?,
            banner: r.read_i32()?,
            category: r.read_i32()?,
            class_loader_name: s.string16(r)?,
            class_name: s.string16(r)?,
            compatible_width_limit_dp: r.read_i32()?,
            description_res: r.read_i32()?,
            full_backup_content: r.read_i32()?,
            data_extraction_rules: r.read_i32()?,
            icon_res: r.read_i32()?,
            install_location: r.read_i32()?,
            label_res: r.read_i32()?,
            largest_width_limit_dp: r.read_i32()?,
            logo: r.read_i32()?,
            manage_space_activity_name: s.string16(r)?,
            max_aspect_ratio: r.read_f32()?,
            min_aspect_ratio: r.read_f32()?,
            min_sdk_version: r.read_i32()?,
            max_sdk_version: r.read_i32()?,
            network_security_config_res: r.read_i32()?,
            non_localized_label: read_char_sequence(r, s)?,
            permission: s.string16(r)?,
            process_name: s.string16(r)?,
            requires_smallest_width_dp: r.read_i32()?,
            round_icon_res: r.read_i32()?,
            target_sandbox_version: r.read_i32()?,
            target_sdk_version: r.read_i32()?,
            task_affinity: s.string16(r)?,
            theme: r.read_i32()?,
            ui_options: r.read_i32()?,
            zygote_preload_name: s.string16(r)?,
            split_class_loader_names: string_array(r, s)?,
            split_code_paths: string_array(r, s)?,
            split_dependencies: sparse_int_arrays(r)?,
            split_flags: int_array(r)?,
            split_names: string_array(r, s)?,
            split_revision_codes: int_array(r)?,
            resizeable_activity: for_boolean(r)?,
            auto_revoke_permissions: r.read_i32()?,
            mime_groups: string_list(r, s)?,
            gwp_asan_mode: r.read_i32()?,
            min_extension_versions: array(r, |r| Ok((r.read_i32()?, r.read_i32()?)))?,
            properties: properties(r, s)?,
            memtag_mode: r.read_i32()?,
            native_heap_zero_initialized: r.read_i32()?,
            request_raw_external_storage_access: for_boolean(r)?,
            locale_config_res: r.read_i32()?,
            known_activity_embedding_certs: string_array(r, s)?,
            manifest_package_name: s.string16(r)?,
            native_library_dir: s.string16(r)?,
            native_library_root_dir: s.string16(r)?,
            native_library_root_requires_isa: r.read_bool()?,
            primary_cpu_abi: s.string16(r)?,
            secondary_cpu_abi: s.string16(r)?,
            secondary_native_library_dir: s.string16(r)?,
            uid: r.read_i32()?,
            booleans: r.read_i64()?,
            booleans2: r.read_i64()?,
            allow_cross_uid_activity_switch_from_below: r.read_bool()?,
            intent_matching_flags: r.read_i32()?,
            alternate_launcher_icon_res_ids: int_array(r)?,
            alternate_launcher_label_res_ids: int_array(r)?,
            page_size_app_compat_flags: r.read_i32()?,
        })
    }
}
impl Component {
    /// `ParsedComponentImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Component> {
        let component = Component {
            name: s.string16(r)?.ok_or(BAD_VALUE)?,
            icon: r.read_i32()?,
            label_res: r.read_i32()?,
            non_localized_label: read_char_sequence(r, s)?,
            logo: r.read_i32()?,
            banner: r.read_i32()?,
            description_res: r.read_i32()?,
            flags: r.read_i32()?,
            package_name: s.string16(r)?.ok_or(BAD_VALUE)?,
            intents: interface_list(r, |r| ParsedIntentInfo::read(r, s))?,
            meta_data: bundle(r, s)?,
            properties: properties(r, s)?,
        };
        Ok(component)
    }
}

impl MainComponent {
    /// `ParsedMainComponentImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<MainComponent> {
        Ok(MainComponent {
            component: Component::read(r, s)?,
            process_name: s.string16(r)?,
            direct_boot_aware: r.read_bool()?,
            enabled: r.read_bool()?,
            exported: r.read_bool()?,
            order: r.read_i32()?,
            split_name: s.string16(r)?,
            attribution_tags: string8_array(r, s)?,
            intent_matching_flags: r.read_i32()?,
        })
    }
}

impl Activity {
    /// `ParsedActivityImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Activity> {
        let mut main = MainComponent::read(r, s)?;
        let mut a = Activity {
            theme: r.read_i32()?,
            ui_options: r.read_i32()?,
            target_activity: s.string16(r)?,
            parent_activity_name: s.string16(r)?,
            task_affinity: s.string16(r)?,
            private_flags: r.read_i32()?,
            permission: s.string16(r)?,
            launch_mode: r.read_i32()?,
            document_launch_mode: r.read_i32()?,
            max_recents: r.read_i32()?,
            config_changes: r.read_i32()?,
            soft_input_mode: r.read_i32()?,
            persistable_mode: r.read_i32()?,
            lock_task_launch_mode: r.read_i32()?,
            screen_orientation: r.read_i32()?,
            resize_mode: r.read_i32()?,
            max_aspect_ratio: float_value(r)?,
            min_aspect_ratio: float_value(r)?,
            supports_size_changes: r.read_bool()?,
            requested_vr_component: s.string16(r)?,
            rotation_animation: r.read_i32()?,
            color_mode: r.read_i32()?,
            ..Activity::default()
        };
        // The meta-data again (`setMetaData`).
        main.component.meta_data = bundle(r, s)?;
        a.main = main;
        if r.read_bool()? {
            a.window_layout = Some(WindowLayout {
                width: r.read_i32()?,
                width_fraction: r.read_f32()?,
                height: r.read_i32()?,
                height_fraction: r.read_f32()?,
                gravity: r.read_i32()?,
                min_width: r.read_i32()?,
                min_height: r.read_i32()?,
                affinity: s.string8(r)?,
            });
        }
        a.known_activity_embedding_certs = string_array(r, s)?;
        a.required_display_category = s.string8(r)?;
        a.require_content_uri_permission_from_caller = r.read_i32()?;
        Ok(a)
    }
}

impl Provider {
    /// `ParsedProviderImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Provider> {
        Ok(Provider {
            main: MainComponent::read(r, s)?,
            authority: s.string16(r)?,
            syncable: r.read_bool()?,
            read_permission: s.string16(r)?,
            write_permission: s.string16(r)?,
            grant_uri_permissions: r.read_bool()?,
            force_uri_permissions: r.read_bool()?,
            multi_process: r.read_bool()?,
            init_order: r.read_i32()?,
            uri_permission_patterns: typed_array(r, |r| PatternMatcher::read(r, s))?,
            path_permissions: typed_array(r, |r| {
                Ok(PathPermission {
                    pattern: PatternMatcher::read(r, s)?,
                    read_permission: s.string16(r)?,
                    write_permission: s.string16(r)?,
                })
            })?,
        })
    }
}

impl PermissionGroup {
    /// `ParsedPermissionGroupImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<PermissionGroup> {
        Ok(PermissionGroup {
            component: Component::read(r, s)?,
            request_detail_res: r.read_i32()?,
            background_request_res: r.read_i32()?,
            background_request_detail_res: r.read_i32()?,
            request_res: r.read_i32()?,
            priority: r.read_i32()?,
        })
    }
}

impl Permission {
    /// `ParsedPermissionImpl(Parcel)`.
    fn read(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Permission> {
        Ok(Permission {
            component: Component::read(r, s)?,
            background_permission: s.string16(r)?,
            group: s.string16(r)?,
            request_res: r.read_i32()?,
            protection_level: r.read_i32()?,
            tree: r.read_bool()?,
            // `readParcelable`: the creator's class (null for none).
            parsed_permission_group: match s.string16(r)? {
                Some(_) => Some(PermissionGroup::read(r, s)?),
                None => None,
            },
            known_certs: string_array(r, s)?,
        })
    }
}

/// `ForBoolean`: 1 unset, 0 false, -1 true.
fn for_boolean(r: &mut Reader<'_>) -> Result<Option<bool>> {
    match r.read_i32()? {
        1 => Ok(None),
        0 => Ok(Some(false)),
        -1 => Ok(Some(true)),
        _ => Err(BAD_VALUE),
    }
}

/// A `create*Array`/`readTypedArray`-style array: a count (-1 null),
/// then the items.
fn array<T>(
    r: &mut Reader<'_>,
    mut item: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Option<Vec<T>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    (0..n).map(|_| item(r)).collect::<Result<_>>().map(Some)
}

fn int_array(r: &mut Reader<'_>) -> Result<Option<Vec<i32>>> {
    array(r, |r| r.read_i32())
}

fn long_array(r: &mut Reader<'_>) -> Result<Option<Vec<i64>>> {
    array(r, |r| r.read_i64())
}

/// `createStringArray` (and `createStringArrayList`, the same form).
fn string_array(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<Vec<Option<String>>>> {
    array(r, |r| s.string16(r))
}

fn string8_array(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<Vec<Option<String>>>> {
    array(r, |r| s.string8(r))
}

/// An interned list or set of strings (`ForInternedStringList`,
/// `ForInternedStringSet`, `ForStringSet`): null reads as empty.
fn string_list(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Vec<String>> {
    Ok(array(r, |r| s.string16(r)?.ok_or(BAD_VALUE))?.unwrap_or_default())
}

/// `createTypedArray`/`createTypedArrayList`: each item preceded by
/// whether it is there; a missing one is refused (the original writes
/// none).
fn typed_array<T>(
    r: &mut Reader<'_>,
    mut item: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Option<Vec<T>>> {
    array(r, |r| {
        if r.read_bool()? {
            item(r)
        } else {
            Err(BAD_VALUE)
        }
    })
}

/// `ParsingUtils.createTypedInterfaceList`: null reads as empty.
fn interface_list<T>(
    r: &mut Reader<'_>,
    item: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    Ok(typed_array(r, item)?.unwrap_or_default())
}

/// `createByteArray`.
fn byte_array(r: &mut Reader<'_>) -> Result<Option<Vec<u8>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    let start = r.position();
    r.skip(n as usize)?;
    let (bytes, _) = r.since(start);
    Ok(Some(bytes[..n as usize].to_vec()))
}

fn feature_info(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<FeatureInfo> {
    Ok(FeatureInfo {
        name: s.string8(r)?,
        version: r.read_i32()?,
        req_gl_es_version: r.read_i32()?,
        flags: r.read_i32()?,
    })
}

/// `readSerializable`: its class (null for none), then its bytes.
fn serializable(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<Serialized>> {
    let Some(class) = s.string16(r)? else {
        return Ok(None);
    };
    Ok(Some(Serialized {
        class,
        bytes: byte_array(r)?.ok_or(BAD_VALUE)?,
    }))
}

/// `readValue` of a `Float` (`writeValue`).
fn float_value(r: &mut Reader<'_>) -> Result<Option<f32>> {
    match r.read_i32()? {
        VAL_NULL => Ok(None),
        VAL_FLOAT => Ok(Some(r.read_f32()?)),
        _ => Err(BAD_VALUE),
    }
}

/// `readValue` of a value a manifest gives; a length-prefixed one is read
/// past by its length and refused.
fn value(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Value> {
    Ok(match r.read_i32()? {
        VAL_NULL => Value::Null,
        VAL_STRING => Value::String(s.string16(r)?),
        VAL_INTEGER => Value::Int(r.read_i32()?),
        VAL_LONG => Value::Long(r.read_i64()?),
        VAL_FLOAT => Value::Float(r.read_f32()?),
        VAL_DOUBLE => Value::Double(f64::from_bits(r.read_i64()? as u64)),
        VAL_BOOLEAN => Value::Bool(r.read_bool()?),
        _ => return Err(BAD_VALUE),
    })
}

/// `readValue` of a string (or null).
fn string_value(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<String>> {
    match value(r, s)? {
        Value::String(v) => Ok(v),
        Value::Null => Ok(None),
        _ => Err(BAD_VALUE),
    }
}

/// `readValue` of a parcelable (`VAL_PARCELABLE`, length-prefixed): its
/// creator's class read, then `read` reads it; `None` for null.
fn parcelable<T>(
    r: &mut Reader<'_>,
    s: &mut dyn Strings,
    read: impl FnOnce(&mut Reader<'_>, &mut dyn Strings) -> Result<T>,
) -> Result<Option<T>> {
    match r.read_i32()? {
        VAL_NULL => Ok(None),
        VAL_PARCELABLE => {
            let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
            let end = r.position() + len;
            s.string16(r)?.ok_or(BAD_VALUE)?;
            let v = read(r, s)?;
            if r.position() != end {
                return Err(BAD_VALUE);
            }
            Ok(Some(v))
        }
        _ => Err(BAD_VALUE),
    }
}

/// `readHashMap` of `Property`s (`mProperties`).
fn properties(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<Vec<(String, Property)>>> {
    array(r, |r| {
        let key = string_value(r, s)?.ok_or(BAD_VALUE)?;
        let property = parcelable(r, s, |r, s| {
            let name = s.string16(r)?;
            let kind = r.read_i32()?;
            let package_name = s.string16(r)?;
            let class_name = s.string16(r)?;
            let value = match kind {
                1 => PropertyValue::Bool(r.read_bool()?),
                2 => PropertyValue::Float(r.read_f32()?),
                3 => PropertyValue::Int(r.read_i32()?),
                4 => PropertyValue::Resource(r.read_i32()?),
                5 => PropertyValue::String(s.string16(r)?),
                other => PropertyValue::Unknown(other),
            };
            Ok(Property {
                name,
                package_name,
                class_name,
                value,
            })
        })?
        .ok_or(BAD_VALUE)?;
        Ok((key, property))
    })
}

/// `readBundle`: `None` for null.
fn bundle(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<MetaData>> {
    let len = r.read_i32()?;
    if len < 0 {
        return Ok(None);
    }
    if len == 0 {
        return Ok(Some(MetaData::default()));
    }
    let magic = r.read_i32()?;
    if magic != BUNDLE_MAGIC && magic != BUNDLE_MAGIC_NATIVE {
        return Err(BAD_VALUE);
    }
    let end = r.position() + len as usize;
    let n = r.read_i32()?;
    let entries = (0..n.max(0))
        .map(|_| Ok((s.string16(r)?.ok_or(BAD_VALUE)?, value(r, s)?)))
        .collect::<Result<_>>()?;
    if r.position() != end {
        return Err(BAD_VALUE);
    }
    // mHasIntent.
    r.read_bool()?;
    Ok(Some(MetaData(entries)))
}

/// `readSparseArray` of `int[]` values.
fn sparse_int_arrays(r: &mut Reader<'_>) -> Result<Option<SplitDependencies>> {
    array(r, |r| {
        let key = r.read_i32()?;
        let value = match r.read_i32()? {
            VAL_NULL => None,
            VAL_INTARRAY => int_array(r)?,
            _ => return Err(BAD_VALUE),
        };
        Ok((key, value))
    })
}

/// The processes (`writeMap` of `ParsedProcessImpl`s by name).
fn processes(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<Vec<Process>>> {
    array(r, |r| {
        string_value(r, s)?;
        parcelable(r, s, |r, s| {
            let flags = r.read_i32()?;
            Ok(Process {
                use_embedded_dex: flags & 0x40 != 0,
                name: s.string16(r)?,
                app_class_names_by_package: array(r, |r| {
                    let key = string_value(r, s)?.ok_or(BAD_VALUE)?;
                    Ok((key, string_value(r, s)?))
                })?
                .unwrap_or_default(),
                denied_permissions: string_list(r, s)?,
                gwp_asan_mode: r.read_i32()?,
                memtag_mode: r.read_i32()?,
                native_heap_zero_initialized: r.read_i32()?,
            })
        })?
        .ok_or(BAD_VALUE)
    })
}

/// `readParcelable` of `SigningDetails`: `None` for `UNKNOWN` (or null).
fn signing_details(r: &mut Reader<'_>, s: &mut dyn Strings) -> Result<Option<SigningDetails>> {
    if s.string16(r)?.is_none() || r.read_bool()? {
        return Ok(None);
    }
    Ok(Some(SigningDetails {
        signatures: typed_array(r, |r| byte_array(r)?.ok_or(BAD_VALUE))?,
        scheme_version: r.read_i32()?,
        public_keys: array(r, |r| match r.read_i32()? {
            VAL_NULL => Ok(None),
            VAL_SERIALIZABLE => {
                let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
                let end = r.position() + len;
                let key = serializable(r, s)?;
                if r.position() != end {
                    return Err(BAD_VALUE);
                }
                Ok(key)
            }
            _ => Err(BAD_VALUE),
        })?,
        past_signing_certificates: typed_array(r, |r| byte_array(r)?.ok_or(BAD_VALUE))?,
    }))
}

#[cfg(test)]
mod tests {
    use aim_binder_host::parcel::Parcel;

    use super::*;

    /// A parcel written as `PackageCacher.toCacheEntryStatic` writes one:
    /// every string an index into the pool at its end.
    #[derive(Default)]
    struct Cache {
        p: Parcel,
        pool: Vec<Option<String>>,
    }

    impl Cache {
        fn new() -> Cache {
            let mut c = Cache::default();
            c.p.write_i32(0);
            c
        }

        fn i(&mut self, v: i32) -> &mut Self {
            self.p.write_i32(v);
            self
        }

        fn s(&mut self, s: Option<&str>) -> &mut Self {
            let s = s.map(str::to_owned);
            let i = match self.pool.iter().position(|p| *p == s) {
                Some(i) => i,
                None => {
                    self.pool.push(s);
                    self.pool.len() - 1
                }
            };
            self.i(i as i32)
        }

        /// Strings, an array or list of them.
        fn ss(&mut self, v: &[&str]) -> &mut Self {
            self.i(v.len() as i32);
            for s in v {
                self.s(Some(s));
            }
            self
        }

        fn finish(mut self) -> Vec<u8> {
            let at = self.p.position();
            self.p.set_i32_at(0, at as i32);
            self.p.write_i32(self.pool.len() as i32);
            for s in &self.pool {
                self.p.write_string16(s.as_deref());
            }
            self.p.data().to_vec()
        }
    }

    /// A component with meta-data: the class, label `label`, and a string
    /// and an int.
    fn component(c: &mut Cache, class: &str) {
        c.s(Some(class)).i(7).i(0);
        // nonLocalizedLabel: a plain CharSequence.
        c.i(1).s(Some("label"));
        c.i(0).i(0).i(0).i(0x4).s(Some("org.example.app"));
        // No intents, the bundle, no properties.
        c.i(0);
        bundle(c);
        c.i(-1);
    }

    fn bundle(c: &mut Cache) {
        let length = c.p.position();
        c.i(-1).i(BUNDLE_MAGIC);
        let start = c.p.position();
        c.i(2);
        c.s(Some("k")).i(VAL_STRING).s(Some("v"));
        c.s(Some("n")).i(VAL_INTEGER).i(3);
        let end = c.p.position();
        c.p.set_i32_at(length, (end - start) as i32);
        c.i(0);
    }

    fn package() -> Vec<u8> {
        let mut c = Cache::new();
        c.ss(&["flag.a=1"]);
        // Screens: unset, false, true, unset, unset, unset.
        c.i(1).i(0).i(-1).i(1).i(1).i(1);
        c.i(7).i(1).i(0).s(Some("1.0")).i(36).s(None);
        c.s(Some("org.example.app")).s(Some("/data/app/a/base.apk"));
        c.s(None).s(None).s(None).s(None).s(None).s(None).i(0);
        // overlayables, sdk library, static library.
        c.i(0).s(None).i(0).s(None).i(0).i(0);
        // library names, uses-libraries (and optional, native, optional native).
        c.i(-1).ss(&["org.apache.http.legacy"]).i(-1).i(-1).i(-1);
        // static libraries with versions and digests; SDK libraries.
        c.ss(&["org.example.lib"])
            .i(1)
            .i(12)
            .i(0)
            .i(1)
            .i(1)
            .s(Some("ab"));
        c.i(-1).i(-1).i(-1).i(-1);
        c.s(Some("org.example.shared")).i(0);
        // config preferences, features, feature groups.
        c.i(0)
            .i(1)
            .i(1)
            .s(Some("android.hardware.camera"))
            .i(0)
            .i(0)
            .i(1)
            .i(-1);
        // restrictUpdateHash, original packages, adopt, requested, uses.
        c.i(-1).i(-1).i(-1).ss(&["android.permission.INTERNET"]);
        c.i(1).i(1).s(Some("android.permission.INTERNET")).i(0);
        // implicit, upgrade key sets, key set mapping, protected broadcasts.
        c.i(-1)
            .i(-1)
            .i(1)
            .s(Some("a"))
            .i(1)
            .s(Some("java.security.PublicKey"));
        c.i(2).i(0x0102);
        c.i(-1);
        // One activity.
        c.i(1).i(1);
        component(&mut c, "org.example.app.Main");
        c.s(None).i(1).i(1).i(1).i(0).s(None).i(-1).i(0);
        c.i(0).i(0).s(None).s(None).s(None).i(0).s(None);
        c.i(5).i(0).i(0).i(0).i(0).i(0).i(0).i(-1).i(0);
        c.i(VAL_FLOAT).p.write_f32(1.5);
        c.i(VAL_NULL).i(0).s(None).i(-1).i(0);
        bundle(&mut c);
        c.i(0).i(-1).s(None).i(0);
        // apex system services, receivers, services, providers.
        c.i(0).i(0).i(0).i(0);
        // attributions, permissions, groups, instrumentations, preferred.
        c.i(0).i(0).i(0).i(0).i(0);
        // processes: one, with the application class.
        c.i(1).i(VAL_STRING).s(Some("p"));
        let length = {
            c.i(VAL_PARCELABLE);
            c.p.position()
        };
        c.i(-1);
        let start = c.p.position();
        c.s(Some(
            "com.android.internal.pm.pkg.component.ParsedProcessImpl",
        ));
        c.i(0).s(Some("org.example.app:p")).i(1);
        c.i(VAL_STRING).s(Some("org.example.app"));
        c.i(VAL_STRING).s(Some("org.example.App"));
        c.i(-1).i(-1).i(-1).i(-1);
        let end = c.p.position();
        c.p.set_i32_at(length, (end - start) as i32);
        // meta-data: none; volume; signing details with one signature.
        c.i(-1).s(None);
        c.s(Some("android.content.pm.SigningDetails")).i(0);
        c.i(1).i(1).i(3).p.write_raw(&[1, 2, 3], &[]);
        c.i(3).i(1).i(VAL_NULL).i(-1);
        c.s(Some("/data/app/a"));
        // queries.
        c.i(-1).i(-1).i(-1);
        c.s(None)
            .s(None)
            .i(0)
            .i(-1)
            .s(None)
            .s(Some("org.example.App"));
        for _ in 0..9 {
            c.i(0);
        }
        c.s(None).p.write_f32(0.0);
        c.p.write_f32(0.0);
        c.i(24).i(0).i(0);
        c.i(1).s(Some("Example"));
        c.s(None)
            .s(None)
            .i(0)
            .i(0)
            .i(0)
            .i(36)
            .s(None)
            .i(0)
            .i(0)
            .s(None);
        // Splits: class loaders, code paths, one dependency, flags, names,
        // revision codes.
        c.i(-1).i(-1).i(1).i(0).i(VAL_NULL).i(-1).i(-1).i(-1);
        // resizeableActivity, autoRevoke, MIME groups, GWP-ASan, extension
        // versions, properties, memtag, zero-init, raw storage, locale
        // config, embedding certificates.
        c.i(1)
            .i(0)
            .i(-1)
            .i(-1)
            .i(-1)
            .i(-1)
            .i(-1)
            .i(-1)
            .i(1)
            .i(0)
            .i(-1);
        c.s(Some("org.example.app"))
            .s(Some("/data/app/a/lib/arm64"));
        c.s(None).i(0).s(Some("arm64-v8a")).s(None).s(None).i(10123);
        c.p.write_i64(booleans::HAS_CODE | booleans::ENABLED);
        c.p.write_i64(0);
        c.i(1).i(0).i(-1).i(-1).i(0);
        c.finish()
    }

    #[test]
    fn reads_a_cache_entry() {
        let pkg = AndroidPackage::read_cache_entry(&package()).unwrap();
        assert_eq!(pkg.package_name, "org.example.app");
        assert_eq!(
            (pkg.supports_small_screens, pkg.supports_normal_screens),
            (None, Some(false))
        );
        assert_eq!(pkg.supports_large_screens, Some(true));
        assert_eq!((pkg.version_code, pkg.version_code_major), (7, 1));
        assert_eq!(pkg.uses_libraries, ["org.apache.http.legacy"]);
        assert_eq!(pkg.uses_static_libraries_versions, Some(vec![12]));
        assert_eq!(pkg.shared_user_id.as_deref(), Some("org.example.shared"));
        let features = pkg.req_features.as_ref().unwrap();
        assert_eq!(features[0].name.as_deref(), Some("android.hardware.camera"));
        assert_eq!(
            pkg.uses_permissions[0].name.as_deref(),
            Some("android.permission.INTERNET")
        );
        let a = &pkg.activities[0];
        assert_eq!(a.main.component.name, "org.example.app.Main");
        assert_eq!(
            a.main.component.non_localized_label.as_deref(),
            Some("label")
        );
        assert!(a.main.enabled && a.main.exported && a.main.direct_boot_aware);
        assert_eq!(a.launch_mode, 5);
        assert_eq!((a.max_aspect_ratio, a.min_aspect_ratio), (Some(1.5), None));
        let meta = a.main.component.meta_data.as_ref().unwrap();
        assert_eq!(
            meta.0,
            [
                ("k".to_string(), Value::String(Some("v".into()))),
                ("n".to_string(), Value::Int(3))
            ]
        );
        let process = &pkg.processes.as_ref().unwrap()[0];
        assert_eq!(process.name.as_deref(), Some("org.example.app:p"));
        assert_eq!(
            (process.gwp_asan_mode, process.denied_permissions.len()),
            (-1, 0)
        );
        let keys = pkg.key_set_mapping.as_ref().unwrap();
        assert_eq!(keys[0].0.as_deref(), Some("a"));
        let key = keys[0].1.as_ref().unwrap()[0].as_ref().unwrap();
        assert_eq!(
            (key.class.as_str(), key.bytes.as_slice()),
            ("java.security.PublicKey", &[2, 1][..])
        );
        assert_eq!(
            process.app_class_names_by_package,
            [("org.example.app".into(), Some("org.example.App".into()))]
        );
        assert_eq!(pkg.meta_data, None);
        let signing = pkg.signing_details.as_ref().unwrap();
        assert_eq!(signing.signatures, Some(vec![vec![1, 2, 3]]));
        assert_eq!(signing.scheme_version, 3);
        assert_eq!(pkg.class_name.as_deref(), Some("org.example.App"));
        assert_eq!(pkg.non_localized_label.as_deref(), Some("Example"));
        assert_eq!((pkg.min_sdk_version, pkg.target_sdk_version), (24, 36));
        assert_eq!(pkg.split_dependencies, Some(vec![(0, None)]));
        assert_eq!(pkg.primary_cpu_abi.as_deref(), Some("arm64-v8a"));
        assert_eq!(pkg.uid, 10123);
        assert!(pkg.is(booleans::HAS_CODE) && !pkg.is(booleans::DEBUGGABLE));

        // A parcel that does not end where its pool starts is refused.
        let mut bytes = package();
        let pool = i32::from_le_bytes(bytes[..4].try_into().unwrap());
        bytes[..4].copy_from_slice(&(pool + 4).to_le_bytes());
        assert!(AndroidPackage::read_cache_entry(&bytes).is_err());
    }
}
