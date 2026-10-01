//! A package's components and the parcelables they hold, as the parser
//! makes them (`com.android.internal.pm.pkg.component.Parsed*Impl`,
//! `PackageManager.Property`; intent filters are
//! [`super::super::intent_filter`]'s), with each one's `writeToParcel` into
//! the cache's parcel.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`),
//! Copyright (C) The Android Open Source Project, Licensed under the
//! Apache License, Version 2.0.

use super::super::intent_filter::{IntentFilter, ParsedIntentInfo, PatternMatcher};
use super::parcel::{ArrayMap, ArraySet, Bundle, Writer};

/// `ParsingUtils.NOT_SET`.
pub const NOT_SET: f32 = -1.0;

/// `ParsedComponentImpl`.
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
    pub meta_data: Option<Bundle>,
    pub properties: ArrayMap<Property>,
}

impl Component {
    fn write(&self, w: &mut Writer) {
        w.field("name");
        w.string(Some(&self.name));
        w.field("icon");
        w.int(self.icon);
        w.field("labelRes");
        w.int(self.label_res);
        w.field("nonLocalizedLabel");
        w.char_sequence(self.non_localized_label.as_deref());
        w.field("logo");
        w.int(self.logo);
        w.field("banner");
        w.int(self.banner);
        w.field("descriptionRes");
        w.int(self.description_res);
        w.field("flags");
        w.int(self.flags);
        w.field("packageName");
        w.string(Some(&self.package_name));
        w.field("intents");
        w.list("intents", &self.intents, write_intent_info);
        w.field("metaData");
        w.bundle(self.meta_data.as_ref());
        w.field("mProperties");
        w.parcelable_map(PROPERTY_CLASS, &self.properties, |w, p| p.write(w));
    }

    /// `getMetaData`: an empty bundle for none.
    pub fn meta_data(&self) -> Bundle {
        self.meta_data.clone().unwrap_or_default()
    }
}

/// `ParsedMainComponentImpl`.
#[derive(Clone, Debug, PartialEq)]
pub struct MainComponent {
    pub component: Component,
    pub process_name: Option<String>,
    pub direct_boot_aware: bool,
    pub enabled: bool,
    pub exported: bool,
    pub order: i32,
    pub split_name: Option<String>,
    pub attribution_tags: Option<Vec<String>>,
    pub intent_matching_flags: i32,
}

impl Default for MainComponent {
    fn default() -> Self {
        MainComponent {
            component: Component::default(),
            process_name: None,
            direct_boot_aware: false,
            enabled: true,
            exported: false,
            order: 0,
            split_name: None,
            attribution_tags: None,
            intent_matching_flags: 0,
        }
    }
}

impl MainComponent {
    fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("processName");
        w.string(self.process_name.as_deref());
        w.field("directBootAware");
        w.bool(self.direct_boot_aware);
        w.field("enabled");
        w.bool(self.enabled);
        w.field("exported");
        w.bool(self.exported);
        w.field("order");
        w.int(self.order);
        w.field("splitName");
        w.string(self.split_name.as_deref());
        w.field("attributionTags");
        w.strings(self.attribution_tags.as_deref());
        w.field("mIntentMatchingFlags");
        w.int(self.intent_matching_flags);
    }
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

/// `ParsedActivityImpl`, for activities and receivers.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub main: MainComponent,
    pub theme: i32,
    pub ui_options: i32,
    pub target_activity: Option<String>,
    pub parent_activity_name: Option<String>,
    pub task_affinity: Option<String>,
    pub private_flags: i32,
    pub permission: Option<String>,
    pub known_activity_embedding_certs: Option<ArraySet>,
    pub launch_mode: i32,
    pub document_launch_mode: i32,
    pub max_recents: i32,
    pub config_changes: i32,
    pub soft_input_mode: i32,
    pub persistable_mode: i32,
    pub lock_task_launch_mode: i32,
    pub screen_orientation: i32,
    pub resize_mode: i32,
    pub max_aspect_ratio: f32,
    pub min_aspect_ratio: f32,
    pub supports_size_changes: bool,
    pub requested_vr_component: Option<String>,
    pub rotation_animation: i32,
    pub color_mode: i32,
    pub window_layout: Option<WindowLayout>,
    pub required_display_category: Option<String>,
    pub require_content_uri_permission_from_caller: i32,
}

/// `ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED`, `RESIZE_MODE_RESIZEABLE`.
pub const SCREEN_ORIENTATION_UNSPECIFIED: i32 = -1;
pub const RESIZE_MODE_RESIZEABLE: i32 = 2;

impl Default for Activity {
    fn default() -> Self {
        Activity {
            main: MainComponent::default(),
            theme: 0,
            ui_options: 0,
            target_activity: None,
            parent_activity_name: None,
            task_affinity: None,
            private_flags: 0,
            permission: None,
            known_activity_embedding_certs: None,
            launch_mode: 0,
            document_launch_mode: 0,
            max_recents: 0,
            config_changes: 0,
            soft_input_mode: 0,
            persistable_mode: 0,
            lock_task_launch_mode: 0,
            screen_orientation: SCREEN_ORIENTATION_UNSPECIFIED,
            resize_mode: RESIZE_MODE_RESIZEABLE,
            max_aspect_ratio: NOT_SET,
            min_aspect_ratio: NOT_SET,
            supports_size_changes: false,
            requested_vr_component: None,
            rotation_animation: -1,
            color_mode: 0,
            window_layout: None,
            required_display_category: None,
            require_content_uri_permission_from_caller: 0,
        }
    }
}

impl Activity {
    pub fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("theme");
        w.int(self.theme);
        w.field("uiOptions");
        w.int(self.ui_options);
        w.field("targetActivity");
        w.string(self.target_activity.as_deref());
        w.field("parentActivityName");
        w.string(self.parent_activity_name.as_deref());
        w.field("taskAffinity");
        w.string(self.task_affinity.as_deref());
        w.field("privateFlags");
        w.int(self.private_flags);
        w.field("permission");
        w.string(self.permission.as_deref());
        for (name, v) in [
            ("launchMode", self.launch_mode),
            ("documentLaunchMode", self.document_launch_mode),
            ("maxRecents", self.max_recents),
            ("configChanges", self.config_changes),
            ("softInputMode", self.soft_input_mode),
            ("persistableMode", self.persistable_mode),
            ("lockTaskLaunchMode", self.lock_task_launch_mode),
            ("screenOrientation", self.screen_orientation),
            ("resizeMode", self.resize_mode),
        ] {
            w.field(name);
            w.int(v);
        }
        w.field("maxAspectRatio");
        w.float_value(self.max_aspect_ratio);
        w.field("minAspectRatio");
        w.float_value(self.min_aspect_ratio);
        w.field("supportsSizeChanges");
        w.bool(self.supports_size_changes);
        w.field("requestedVrComponent");
        w.string(self.requested_vr_component.as_deref());
        w.field("rotationAnimation");
        w.int(self.rotation_animation);
        w.field("colorMode");
        w.int(self.color_mode);
        // `getMetaData` a second time.
        w.field("metaData2");
        w.bundle(Some(&self.main.component.meta_data()));
        w.field("windowLayout");
        match &self.window_layout {
            Some(l) => {
                w.int(1);
                w.int(l.width);
                w.float(l.width_fraction);
                w.int(l.height);
                w.float(l.height_fraction);
                w.int(l.gravity);
                w.int(l.min_width);
                w.int(l.min_height);
                w.string(l.affinity.as_deref());
            }
            None => w.bool(false),
        }
        w.field("mKnownActivityEmbeddingCerts");
        match &self.known_activity_embedding_certs {
            Some(s) => w.set(s),
            None => w.int(-1),
        }
        w.field("mRequiredDisplayCategory");
        w.string(self.required_display_category.as_deref());
        w.field("mRequireContentUriPermissionFromCaller");
        w.int(self.require_content_uri_permission_from_caller);
    }
}

/// `ParsedServiceImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub main: MainComponent,
    pub foreground_service_type: i32,
    pub permission: Option<String>,
}

impl Service {
    pub fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("foregroundServiceType");
        w.int(self.foreground_service_type);
        w.field("permission");
        w.string(self.permission.as_deref());
    }
}

/// `PathPermission`: a `PatternMatcher` and its permissions.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPermission {
    pub pattern: PatternMatcher,
    pub read_permission: Option<String>,
    pub write_permission: Option<String>,
}

/// `ParsedProviderImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Provider {
    pub main: MainComponent,
    pub authority: Option<String>,
    pub syncable: bool,
    pub read_permission: Option<String>,
    pub write_permission: Option<String>,
    pub grant_uri_permissions: bool,
    pub force_uri_permissions: bool,
    pub multi_process: bool,
    pub init_order: i32,
    pub uri_permission_patterns: Vec<PatternMatcher>,
    pub path_permissions: Vec<PathPermission>,
}

impl Provider {
    pub fn write(&self, w: &mut Writer) {
        self.main.write(w);
        w.field("authority");
        w.string(self.authority.as_deref());
        w.field("syncable");
        w.bool(self.syncable);
        w.field("readPermission");
        w.string(self.read_permission.as_deref());
        w.field("writePermission");
        w.string(self.write_permission.as_deref());
        w.field("grantUriPermissions");
        w.bool(self.grant_uri_permissions);
        w.field("forceUriPermissions");
        w.bool(self.force_uri_permissions);
        w.field("multiProcess");
        w.bool(self.multi_process);
        w.field("initOrder");
        w.int(self.init_order);
        w.field("uriPermissionPatterns");
        w.list(
            "uriPermissionPatterns",
            &self.uri_permission_patterns,
            write_pattern,
        );
        w.field("pathPermissions");
        w.list("pathPermissions", &self.path_permissions, |w, p| {
            write_pattern(w, &p.pattern);
            w.string(p.read_permission.as_deref());
            w.string(p.write_permission.as_deref());
        });
    }
}

/// `PatternMatcher.writeToParcel`. The parser makes no advanced glob
/// (it refuses one), so none has a parsed form.
fn write_pattern(w: &mut Writer, p: &PatternMatcher) {
    w.string(Some(&p.pattern));
    w.int(p.kind);
    w.ints(None);
}

/// `IntentFilter.writeToParcel` of a filter the parser made: no extras,
/// no relative filter groups.
fn write_filter(w: &mut Writer, f: &IntentFilter) {
    w.field("mActions");
    let mut actions = ArraySet::default();
    f.actions.iter().for_each(|a| actions.add(a));
    let actions: Vec<&str> = actions.iter().collect();
    w.strings(Some(&actions));
    for (name, list) in [
        ("mCategories", &f.categories),
        ("mDataSchemes", &f.schemes),
        ("mStaticDataTypes", &f.static_types),
        ("mDataTypes", &f.types),
        ("mMimeGroups", &f.mime_groups),
    ] {
        w.field(name);
        match list {
            Some(l) => {
                w.int(1);
                w.strings(Some(l));
            }
            None => w.int(0),
        }
    }
    w.field("mDataSchemeSpecificParts");
    let ssps = f.ssps.as_deref().unwrap_or(&[]);
    w.int(ssps.len() as i32);
    ssps.iter().for_each(|p| write_pattern(w, p));
    w.field("mDataAuthorities");
    let authorities = f.authorities.as_deref().unwrap_or(&[]);
    w.int(authorities.len() as i32);
    for a in authorities {
        w.string(Some(&a.orig_host));
        w.string(Some(&a.host));
        w.bool(a.wild);
        w.int(a.port);
    }
    w.field("mDataPaths");
    let paths = f.paths.as_deref().unwrap_or(&[]);
    w.int(paths.len() as i32);
    paths.iter().for_each(|p| write_pattern(w, p));
    w.field("mPriority");
    w.int(f.priority);
    w.field("mHasStaticPartialTypes");
    w.bool(f.has_static_partial_types);
    w.field("mHasDynamicPartialTypes");
    w.bool(f.has_dynamic_partial_types);
    w.field("autoVerify");
    w.bool(f.auto_verify);
    w.field("mInstantAppVisibility");
    w.int(f.instant_app_visibility);
    w.field("mOrder");
    w.int(f.order);
    w.field("mExtras");
    w.int(0);
    w.field("mUriRelativeFilterGroups");
    w.int(0);
}

/// `ParsedIntentInfoImpl.writeToParcel`.
pub fn write_intent_info(w: &mut Writer, i: &ParsedIntentInfo) {
    let mut flg = 0;
    if i.has_default {
        flg |= 0x1;
    }
    if i.non_localized_label.is_some() {
        flg |= 0x4;
    }
    w.field("flg");
    w.int(flg);
    w.field("mLabelRes");
    w.int(i.label_res);
    if let Some(l) = &i.non_localized_label {
        w.field("mNonLocalizedLabel");
        w.char_sequence(Some(l));
    }
    w.field("mIcon");
    w.int(i.icon);
    w.field("mIntentFilter");
    w.int(1);
    write_filter(w, &i.filter);
}

/// `PackageManager.Property`.
pub const PROPERTY_CLASS: &str = "android.content.pm.PackageManager$Property";

#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    Resource(i32),
    String(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: PropertyValue,
    pub package_name: String,
    pub class_name: Option<String>,
}

impl Property {
    pub fn write(&self, w: &mut Writer) {
        w.string(Some(&self.name));
        let kind = match self.value {
            PropertyValue::Bool(_) => 1,
            PropertyValue::Float(_) => 2,
            PropertyValue::Int(_) => 3,
            PropertyValue::Resource(_) => 4,
            PropertyValue::String(_) => 5,
        };
        w.int(kind);
        w.string(Some(&self.package_name));
        w.string(self.class_name.as_deref());
        match &self.value {
            PropertyValue::Bool(b) => w.bool(*b),
            PropertyValue::Float(f) => w.float(*f),
            PropertyValue::Int(i) | PropertyValue::Resource(i) => w.int(*i),
            PropertyValue::String(s) => w.string(s.as_deref()),
        }
    }

    /// `toBundle`: `bundle` with this property put in it.
    pub fn put_in(&self, bundle: &mut Bundle) {
        use super::parcel::BundleValue as V;
        let v = match &self.value {
            PropertyValue::Bool(b) => V::Bool(*b),
            PropertyValue::Float(f) => V::Float(*f),
            PropertyValue::Int(i) | PropertyValue::Resource(i) => V::Int(*i),
            PropertyValue::String(s) => V::String(s.clone()),
        };
        bundle.put(&self.name, v);
    }
}

/// `ParsedPermissionGroupImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionGroup {
    pub component: Component,
    pub request_detail_res: i32,
    pub background_request_res: i32,
    pub background_request_detail_res: i32,
    pub request_res: i32,
    pub priority: i32,
}

pub const PERMISSION_GROUP_CLASS: &str =
    "com.android.internal.pm.pkg.component.ParsedPermissionGroupImpl";

impl PermissionGroup {
    pub fn write(&self, w: &mut Writer) {
        self.component.write(w);
        for (name, v) in [
            ("requestDetailRes", self.request_detail_res),
            ("backgroundRequestRes", self.background_request_res),
            (
                "backgroundRequestDetailRes",
                self.background_request_detail_res,
            ),
            ("requestRes", self.request_res),
            ("priority", self.priority),
        ] {
            w.field(name);
            w.int(v);
        }
    }
}

/// `ParsedPermissionImpl`, for permissions and permission trees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Permission {
    pub component: Component,
    pub background_permission: Option<String>,
    pub group: Option<String>,
    pub request_res: i32,
    pub protection_level: i32,
    pub tree: bool,
    pub known_certs: Option<ArraySet>,
}

impl Permission {
    pub fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("backgroundPermission");
        w.string(self.background_permission.as_deref());
        w.field("group");
        w.string(self.group.as_deref());
        w.field("requestRes");
        w.int(self.request_res);
        w.field("protectionLevel");
        w.int(self.protection_level);
        w.field("tree");
        w.bool(self.tree);
        // The parser never links a permission to its group.
        w.field("parsedPermissionGroup");
        w.string(None);
        w.field("knownCerts");
        match &self.known_certs {
            Some(s) => w.set(s),
            None => w.int(-1),
        }
    }
}

/// `ParsedInstrumentationImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instrumentation {
    pub component: Component,
    pub target_package: Option<String>,
    pub target_processes: Option<String>,
    pub handle_profiling: bool,
    pub functional_test: bool,
}

impl Instrumentation {
    pub fn write(&self, w: &mut Writer) {
        self.component.write(w);
        w.field("targetPackage");
        w.string(self.target_package.as_deref());
        w.field("targetProcesses");
        w.string(self.target_processes.as_deref());
        w.field("handleProfiling");
        w.bool(self.handle_profiling);
        w.field("functionalTest");
        w.bool(self.functional_test);
    }
}

/// `ParsedAttributionImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attribution {
    pub tag: String,
    pub label: i32,
    pub inherit_from: Vec<String>,
}

impl Attribution {
    pub fn write(&self, w: &mut Writer) {
        w.field("tag");
        w.string(Some(&self.tag));
        w.field("label");
        w.int(self.label);
        w.field("inheritFrom");
        w.strings(Some(&self.inherit_from));
    }
}

/// `ParsedProcessImpl`.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub name: String,
    pub app_class_names_by_package: ArrayMap<Option<String>>,
    pub denied_permissions: ArraySet,
    pub gwp_asan_mode: i32,
    pub memtag_mode: i32,
    pub native_heap_zero_initialized: i32,
    pub use_embedded_dex: bool,
}

pub const PROCESS_CLASS: &str = "com.android.internal.pm.pkg.component.ParsedProcessImpl";

impl Process {
    pub fn write(&self, w: &mut Writer) {
        w.field("flg");
        w.int(if self.use_embedded_dex { 0x40 } else { 0 });
        w.field("name");
        w.string(Some(&self.name));
        w.field("appClassNamesByPackage");
        w.int(self.app_class_names_by_package.len() as i32);
        for (k, v) in self.app_class_names_by_package.iter() {
            w.string_value(k);
            match v {
                Some(v) => w.string_value(v),
                None => w.null_value(),
            }
        }
        w.field("deniedPermissions");
        w.set(&self.denied_permissions);
        w.field("gwpAsanMode");
        w.int(self.gwp_asan_mode);
        w.field("memtagMode");
        w.int(self.memtag_mode);
        w.field("nativeHeapZeroInitialized");
        w.int(self.native_heap_zero_initialized);
    }
}

/// `ParsedApexSystemServiceImpl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApexSystemService {
    pub name: String,
    pub jar_path: Option<String>,
    pub min_sdk_version: Option<String>,
    pub max_sdk_version: Option<String>,
    pub init_order: i32,
}

impl ApexSystemService {
    pub fn write(&self, w: &mut Writer) {
        let mut flg = 0;
        if self.jar_path.is_some() {
            flg |= 0x2;
        }
        if self.min_sdk_version.is_some() {
            flg |= 0x4;
        }
        if self.max_sdk_version.is_some() {
            flg |= 0x8;
        }
        w.field("flg");
        w.int(flg);
        w.field("name");
        w.string(Some(&self.name));
        for (name, v) in [
            ("jarPath", &self.jar_path),
            ("minSdkVersion", &self.min_sdk_version),
            ("maxSdkVersion", &self.max_sdk_version),
        ] {
            w.field(name);
            w.string(v.as_deref());
        }
        w.field("initOrder");
        w.int(self.init_order);
    }
}

/// `ParsedUsesPermissionImpl`.
#[derive(Clone, Debug, PartialEq)]
pub struct UsesPermission {
    pub name: String,
    pub flags: i32,
}

impl UsesPermission {
    pub fn write(&self, w: &mut Writer) {
        w.field("name");
        w.string(Some(&self.name));
        w.field("usesPermissionFlags");
        w.int(self.flags);
    }
}
