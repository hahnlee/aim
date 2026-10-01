//! The component parsers: activities and receivers, aliases, services,
//! providers, intent filters, permissions, instrumentation, attributions,
//! processes and APEX system services.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `com.android.internal.pm.pkg.component.Parsed*Utils`,
//! `ComponentParseUtils`, `ParsedActivityImpl`), Copyright (C) The Android
//! Open Source Project, Licensed under the Apache License, Version 2.0.

use aim_apps::res::Element;

use super::super::intent_filter::{
    PATTERN_ADVANCED_GLOB, PATTERN_LITERAL, PATTERN_PREFIX, PATTERN_SIMPLE_GLOB, PATTERN_SUFFIX,
    ParsedIntentInfo, PatternMatcher,
};
use super::super::uri::parse_int;
use super::attrs::{ANDROID, NATIVE_CONFIG_VERSION, TypedArray, attr_value};
use super::component::{
    Activity, ApexSystemService, Attribution, Component, Instrumentation, MainComponent,
    PathPermission, Permission, PermissionGroup, Process, Provider, Service, WindowLayout,
};
use super::package::{Package, booleans as b};
use super::parcel::{ArrayMap, ArraySet, Bundle, BundleValue};
use super::resources::{TYPE_DIMENSION, TYPE_FLOAT, TYPE_FRACTION};
use super::{
    APP_DETAILS_ACTIVITY_CLASS_NAME, EMPTY_INTENT_ACTION_CATEGORY, Error, FROYO,
    MISSING_EXPORTED_FLAG, Parser, Result, build_class_name, build_process_name,
    build_task_affinity_name, fail, validate_name,
};

// `ActivityInfo` flags.
const FLAG_MULTIPROCESS: i32 = 0x0001;
const FLAG_FINISH_ON_TASK_LAUNCH: i32 = 0x0002;
const FLAG_CLEAR_TASK_ON_LAUNCH: i32 = 0x0004;
const FLAG_ALWAYS_RETAIN_TASK_STATE: i32 = 0x0008;
const FLAG_STATE_NOT_NEEDED: i32 = 0x0010;
const FLAG_EXCLUDE_FROM_RECENTS: i32 = 0x0020;
const FLAG_ALLOW_TASK_REPARENTING: i32 = 0x0040;
const FLAG_NO_HISTORY: i32 = 0x0080;
const FLAG_FINISH_ON_CLOSE_SYSTEM_DIALOGS: i32 = 0x0100;
const FLAG_HARDWARE_ACCELERATED: i32 = 0x0200;
const FLAG_SHOW_FOR_ALL_USERS: i32 = 0x0400;
const FLAG_IMMERSIVE: i32 = 0x0800;
const FLAG_RELINQUISH_TASK_IDENTITY: i32 = 0x1000;
const FLAG_AUTO_REMOVE_FROM_RECENTS: i32 = 0x2000;
const FLAG_RESUME_WHILE_PAUSING: i32 = 0x4000;
const FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES: i32 = 0x10000;
const FLAG_ALWAYS_FOCUSABLE: i32 = 0x40000;
const FLAG_VISIBLE_TO_INSTANT_APP: i32 = 0x100000;
const FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP: i32 = 0x200000;
pub const FLAG_SUPPORTS_PICTURE_IN_PICTURE: i32 = 0x400000;
const FLAG_SHOW_WHEN_LOCKED: i32 = 0x800000;
const FLAG_TURN_SCREEN_ON: i32 = 0x1000000;
const FLAG_PREFER_MINIMAL_POST_PROCESSING: i32 = 0x2000000;
const FLAG_ALLOW_UNTRUSTED_ACTIVITY_EMBEDDING: i32 = 0x10000000;
const FLAG_SYSTEM_USER_ONLY: i32 = 0x20000000;
const FLAG_SINGLE_USER: i32 = 0x40000000;
const FLAG_ALLOW_EMBEDDED: i32 = 0x80000000u32 as i32;
const FLAG_INHERIT_SHOW_WHEN_LOCKED: i32 = 1;
const PRIVATE_FLAG_HOME_TRANSITION_SOUND: i32 = 1 << 1;
const PRIVATE_FLAG_ENABLE_ON_BACK_INVOKED_CALLBACK: i32 = 1 << 2;
const PRIVATE_FLAG_DISABLE_ON_BACK_INVOKED_CALLBACK: i32 = 1 << 3;
const LAUNCH_SINGLE_INSTANCE_PER_TASK: i32 = 4;
const PERSIST_ROOT_ONLY: i32 = 0;
const PERSIST_NEVER: i32 = 1;
const RESIZE_MODE_UNRESIZEABLE: i32 = 0;
const RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION: i32 = 1;
const RESIZE_MODE_RESIZEABLE: i32 = 2;
const RESIZE_MODE_FORCE_RESIZEABLE: i32 = 4;
const RESIZE_MODE_FORCE_RESIZABLE_LANDSCAPE_ONLY: i32 = 5;
const RESIZE_MODE_FORCE_RESIZABLE_PORTRAIT_ONLY: i32 = 6;
const RESIZE_MODE_FORCE_RESIZABLE_PRESERVE_ORIENTATION: i32 = 7;
const SCREEN_ORIENTATION_LOCKED: i32 = 14;
/// `ActivityInfo.CONFIG_MCC | CONFIG_MNC`.
const RECREATE_ON_CONFIG_CHANGES_MASK: i32 = 0x3;
/// `Gravity.CENTER`, `Gravity.NO_GRAVITY`.
const GRAVITY_CENTER: i32 = 0x11;
const GRAVITY_NO_GRAVITY: i32 = 0;

// `ServiceInfo` and `ProviderInfo` flags.
const SERVICE_STOP_WITH_TASK: i32 = 0x1;
const SERVICE_ISOLATED_PROCESS: i32 = 0x2;
const SERVICE_EXTERNAL_SERVICE: i32 = 0x4;
const SERVICE_USE_APP_ZYGOTE: i32 = 0x8;
const SERVICE_ALLOW_SHARED_ISOLATED_PROCESS: i32 = 0x10;

// `IntentFilter` instant app visibility.
const VISIBILITY_EXPLICIT: i32 = 1;
const VISIBILITY_IMPLICIT: i32 = 2;

// `PermissionInfo`.
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_SIGNATURE: i32 = 2;
const PROTECTION_SIGNATURE_OR_SYSTEM: i32 = 3;
const PROTECTION_INTERNAL: i32 = 4;
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_FLAG_PRIVILEGED: i32 = 0x10;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
const PROTECTION_FLAG_INSTANT: i32 = 0x1000;
const PROTECTION_FLAG_RUNTIME_ONLY: i32 = 0x2000;
const PROTECTION_FLAG_VENDOR_PRIVILEGED: i32 = 0x8000;
const PERMISSION_FLAG_HARD_RESTRICTED: i32 = 1 << 2;
const PERMISSION_FLAG_SOFT_RESTRICTED: i32 = 1 << 3;

/// `ComponentParseUtils.flag`.
fn flag(f: i32, attr: &str, default: bool, sa: &TypedArray) -> i32 {
    if sa.boolean(attr, default) { f } else { 0 }
}

/// `ParsedActivityImpl.setMaxAspectRatio(resizeMode, ratio)`.
pub fn set_max_aspect_ratio(a: &mut Activity, ratio: f32) {
    if a.resize_mode == RESIZE_MODE_RESIZEABLE
        || a.resize_mode == RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION
        || (ratio < 1.0 && ratio != 0.0)
    {
        return;
    }
    a.max_aspect_ratio = ratio;
}

/// `ParsedActivityImpl.setMinAspectRatio(resizeMode, ratio)`.
pub fn set_min_aspect_ratio(a: &mut Activity, ratio: f32) {
    if a.resize_mode == RESIZE_MODE_RESIZEABLE
        || a.resize_mode == RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION
        || (ratio < 1.0 && ratio != 0.0)
    {
        return;
    }
    a.min_aspect_ratio = ratio;
}

/// `ParsedActivityUtils.getActivityConfigChanges`.
fn config_changes(changes: i32, recreate: i32) -> i32 {
    changes | (!recreate & RECREATE_ON_CONFIG_CHANGES_MASK)
}

/// A setter that turns an empty permission into none.
fn permission(p: Option<String>) -> Option<String> {
    p.filter(|p| !p.is_empty())
}

/// How `parseIntentFilter` treats a component's filters.
struct FilterRules {
    visible_to_ephemeral: bool,
    allow_auto_verify: bool,
    allow_implicit_ephemeral_visibility: bool,
    fail_on_no_actions: bool,
}

impl FilterRules {
    /// A service's or a provider's.
    fn plain(visible_to_ephemeral: bool) -> FilterRules {
        FilterRules {
            visible_to_ephemeral,
            allow_auto_verify: false,
            allow_implicit_ephemeral_visibility: false,
            fail_on_no_actions: false,
        }
    }
}

/// Which of `parseMainComponent`'s attributes a component has.
struct MainAttrs {
    tag: &'static str,
    direct_boot_aware: bool,
    process: bool,
    split_name: bool,
    description: bool,
}

impl Parser<'_> {
    /// `ParsedComponentUtils.parseComponent`.
    fn parse_component(
        &self,
        c: &mut Component,
        tag: &str,
        pkg: &Package,
        sa: &TypedArray,
        description: bool,
    ) -> Result<()> {
        let name = sa.non_config_string("name", 0).filter(|n| !n.is_empty());
        let Some(name) = name else {
            return fail(format!("{tag} does not specify android:name"));
        };
        let class = build_class_name(&pkg.package_name, Some(&name)).unwrap_or_default();
        if class == APP_DETAILS_ACTIVITY_CLASS_NAME {
            return fail(format!("{tag} invalid android:name"));
        }
        c.name = class;
        c.package_name = pkg.package_name.clone();
        let round = if self.platform.use_round_icon {
            sa.resource_id("roundIcon", 0)
        } else {
            0
        };
        if round != 0 {
            c.icon = round;
            c.non_localized_label = None;
        } else {
            let icon = sa.resource_id("icon", 0);
            if icon != 0 {
                c.icon = icon;
                c.non_localized_label = None;
            }
        }
        let logo = sa.resource_id("logo", 0);
        if logo != 0 {
            c.logo = logo;
        }
        let banner = sa.resource_id("banner", 0);
        if banner != 0 {
            c.banner = banner;
        }
        if description {
            c.description_res = sa.resource_id("description", 0);
        }
        if let Some(v) = sa.peek("label") {
            c.label_res = v.resource_id as i32;
            if v.resource_id == 0 {
                c.non_localized_label = v.coerce_to_string();
            }
        }
        Ok(())
    }

    /// `ParsedMainComponentUtils.parseMainComponent`.
    fn parse_main_component(
        &self,
        m: &mut MainComponent,
        attrs: &MainAttrs,
        pkg: &mut Package,
        sa: &TypedArray,
        default_split: Option<&str>,
    ) -> Result<()> {
        self.parse_component(&mut m.component, attrs.tag, pkg, sa, attrs.description)?;
        if attrs.direct_boot_aware {
            m.direct_boot_aware = sa.boolean("directBootAware", false);
            if m.direct_boot_aware {
                pkg.set(b::PARTIALLY_DIRECT_BOOT_AWARE, true);
            }
        }
        m.enabled = sa.boolean("enabled", true);
        if attrs.process {
            let name = if pkg.target_sdk_version >= FROYO {
                sa.non_config_string("process", NATIVE_CONFIG_VERSION)
            } else {
                sa.non_resource_string("process")
            };
            let def = pkg.process_name.clone().unwrap_or(pkg.package_name.clone());
            m.process_name =
                build_process_name(&pkg.package_name, Some(&def), name.as_deref(), self.flags)?;
        }
        if attrs.split_name {
            m.split_name = sa.non_config_string("splitName", 0);
        }
        if m.split_name.is_none() {
            m.split_name = default_split.map(str::to_owned);
        }
        if let Some(tags) = sa.non_config_string("attributionTags", 0) {
            m.attribution_tags = Some(tags.split('|').map(str::to_owned).collect());
        }
        if self
            .platform
            .flag("android.security.enable_intent_matching_flags")
        {
            m.intent_matching_flags = match sa.int("intentMatchingFlags", 0) {
                0 => pkg.intent_matching_flags,
                f => f,
            };
        }
        Ok(())
    }

    /// `ParsedIntentInfoUtils.parseIntentInfo`.
    pub(super) fn parse_intent_info(
        &self,
        pkg: &mut Package,
        e: &Element,
        allow_globs: bool,
        allow_auto_verify: bool,
    ) -> Result<ParsedIntentInfo> {
        let mut info = ParsedIntentInfo::default();
        {
            let sa = self.obtain(e);
            info.filter.priority = sa.int("priority", 0);
            info.filter.order = sa.int("order", 0);
            if let Some(v) = sa.peek("label") {
                info.label_res = v.resource_id as i32;
                if v.resource_id == 0 {
                    info.non_localized_label = v.coerce_to_string();
                }
            }
            if self.platform.use_round_icon {
                info.icon = sa.resource_id("roundIcon", 0);
            }
            if info.icon == 0 {
                info.icon = sa.resource_id("icon", 0);
            }
            if allow_auto_verify {
                info.filter.auto_verify = sa.boolean("autoVerify", false);
            }
        }
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "action" | "category" => {
                    let Some(v) = attr_value(c, ANDROID, "name") else {
                        return fail("No value supplied for <android:name>");
                    };
                    if c.name == "action" {
                        info.filter.add_action(&v);
                    } else {
                        info.filter.add_category(&v);
                    }
                    if v.is_empty() {
                        self.input.borrow_mut().defer(
                            "No value supplied for <android:name>".into(),
                            EMPTY_INTENT_ACTION_CATEGORY,
                        )?;
                    }
                }
                "data" => self.parse_data(&mut info, c, allow_globs)?,
                "uri-relative-filter-group"
                    if self
                        .platform
                        .flag("android.content.pm.relative_reference_intent_filters") =>
                {
                    return Err(Error::Unsupported("<uri-relative-filter-group>".into()));
                }
                _ => {}
            }
        }
        info.has_default = info.filter.has_category("android.intent.category.DEFAULT");
        Ok(info)
    }

    fn parse_data(
        &self,
        info: &mut ParsedIntentInfo,
        e: &Element,
        allow_globs: bool,
    ) -> Result<()> {
        let sa = self.obtain(e);
        let f = &mut info.filter;
        let s = |attr: &str| sa.non_config_string(attr, 0);
        if let Some(t) = s("mimeType") {
            f.add_data_type(&t)
                .map_err(|_| Error::Parse(format!("MalformedMimeTypeException: {t}")))?;
        }
        if let Some(g) = s("mimeGroup") {
            f.add_mime_group(&g);
        }
        if let Some(scheme) = s("scheme") {
            f.add_data_scheme(&scheme);
        }
        for (attr, kind, glob) in [
            ("ssp", PATTERN_LITERAL, false),
            ("sspPrefix", PATTERN_PREFIX, false),
            ("sspPattern", PATTERN_SIMPLE_GLOB, true),
            ("sspAdvancedPattern", PATTERN_ADVANCED_GLOB, true),
            ("sspSuffix", PATTERN_SUFFIX, false),
        ] {
            if let Some(v) = s(attr) {
                if glob && !allow_globs {
                    return fail(format!("{attr} not allowed here; ssp must be literal"));
                }
                f.add_data_scheme_specific_part(pattern(v, kind)?);
            }
        }
        if let Some(host) = s("host") {
            let port = s("port");
            // `Integer.parseInt` of the port throws.
            if port.as_deref().is_some_and(|p| parse_int(p).is_none()) {
                return fail(format!("NumberFormatException: port {port:?}"));
            }
            f.add_data_authority(&host, port.as_deref());
        }
        for (attr, kind, glob) in [
            ("path", PATTERN_LITERAL, false),
            ("pathPrefix", PATTERN_PREFIX, false),
            ("pathPattern", PATTERN_SIMPLE_GLOB, true),
            ("pathAdvancedPattern", PATTERN_ADVANCED_GLOB, true),
            ("pathSuffix", PATTERN_SUFFIX, false),
        ] {
            if let Some(v) = s(attr) {
                if glob && !allow_globs {
                    return fail(format!("{attr} not allowed here; path must be literal"));
                }
                f.add_data_path(pattern(v, kind)?);
            }
        }
        Ok(())
    }

    /// `ParsedMainComponentUtils.parseIntentFilter`; `None` for a filter
    /// without actions where that is dropped.
    fn parse_main_intent_filter(
        &self,
        pkg: &mut Package,
        e: &Element,
        rules: FilterRules,
    ) -> Result<Option<ParsedIntentInfo>> {
        let mut info = self.parse_intent_info(pkg, e, true, rules.allow_auto_verify)?;
        if info.filter.actions.is_empty() && rules.fail_on_no_actions {
            return Ok(None);
        }
        let f = &info.filter;
        let implicit = f.has_category("android.intent.category.BROWSABLE")
            || f.has_action("android.intent.action.SEND")
            || f.has_action("android.intent.action.SENDTO")
            || f.has_action("android.intent.action.SEND_MULTIPLE");
        info.filter.instant_app_visibility = if rules.visible_to_ephemeral {
            VISIBILITY_EXPLICIT
        } else if rules.allow_implicit_ephemeral_visibility && implicit {
            VISIBILITY_IMPLICIT
        } else {
            0
        };
        Ok(Some(info))
    }

    /// `ParsedComponentUtils.addMetaData`.
    fn add_meta_data(&self, pkg: &Package, c: &mut Component, e: &Element) -> Result<()> {
        if let Some(p) = self.parse_meta_data(pkg, Some(&c.name), e, "<meta-data>")? {
            p.put_in(c.meta_data.get_or_insert_with(Bundle::default));
        }
        Ok(())
    }

    /// `ParsedComponentUtils.addProperty`.
    fn add_property(&self, pkg: &Package, c: &mut Component, e: &Element) -> Result<()> {
        if let Some(p) = self.parse_meta_data(pkg, Some(&c.name), e, "<property>")? {
            c.properties.put(&p.name.clone(), p);
        }
        Ok(())
    }

    /// `ComponentParseUtils.parseAllMetaData`.
    fn parse_all_meta_data(&self, pkg: &mut Package, c: &mut Component, e: &Element) -> Result<()> {
        for child in &e.children {
            if self.skip(pkg, child) {
                continue;
            }
            if child.name == "meta-data" {
                self.add_meta_data(pkg, c, child)?;
            }
        }
        Ok(())
    }

    /// `ParsedActivityUtils.parseActivityOrReceiver`.
    pub(super) fn parse_activity_or_receiver(
        &self,
        pkg: &mut Package,
        e: &Element,
        default_split: Option<&str>,
    ) -> Result<Activity> {
        let receiver = e.name == "receiver";
        let mut a = Activity::default();
        let sa = self.obtain(e);
        let tag = if receiver { "<receiver>" } else { "<activity>" };
        self.parse_main_component(
            &mut a.main,
            &MainAttrs {
                tag,
                direct_boot_aware: true,
                process: true,
                split_name: true,
                description: true,
            },
            pkg,
            &sa,
            default_split,
        )?;
        if receiver
            && pkg.get(b::CANT_SAVE_STATE)
            && a.main.process_name.as_deref() == Some(pkg.package_name.as_str())
        {
            return fail("Heavy-weight applications can not have receivers in main process");
        }
        a.theme = sa.resource_id("theme", 0);
        a.ui_options = sa.int("uiOptions", pkg.ui_options);
        a.main.component.flags |=
            flag(
                FLAG_ALLOW_TASK_REPARENTING,
                "allowTaskReparenting",
                pkg.get(b::ALLOW_TASK_REPARENTING),
                &sa,
            ) | flag(
                FLAG_ALWAYS_RETAIN_TASK_STATE,
                "alwaysRetainTaskState",
                false,
                &sa,
            ) | flag(FLAG_CLEAR_TASK_ON_LAUNCH, "clearTaskOnLaunch", false, &sa)
                | flag(FLAG_EXCLUDE_FROM_RECENTS, "excludeFromRecents", false, &sa)
                | flag(
                    FLAG_FINISH_ON_CLOSE_SYSTEM_DIALOGS,
                    "finishOnCloseSystemDialogs",
                    false,
                    &sa,
                )
                | flag(FLAG_FINISH_ON_TASK_LAUNCH, "finishOnTaskLaunch", false, &sa)
                | flag(FLAG_IMMERSIVE, "immersive", false, &sa)
                | flag(FLAG_MULTIPROCESS, "multiprocess", false, &sa)
                | flag(FLAG_NO_HISTORY, "noHistory", false, &sa)
                | flag(FLAG_SHOW_FOR_ALL_USERS, "showForAllUsers", false, &sa)
                | flag(FLAG_SHOW_FOR_ALL_USERS, "showOnLockScreen", false, &sa)
                | flag(FLAG_STATE_NOT_NEEDED, "stateNotNeeded", false, &sa)
                | flag(FLAG_SYSTEM_USER_ONLY, "systemUserOnly", false, &sa);
        if !receiver {
            a.main.component.flags |= flag(
                FLAG_HARDWARE_ACCELERATED,
                "hardwareAccelerated",
                pkg.get(b::HARDWARE_ACCELERATED),
                &sa,
            ) | flag(FLAG_ALLOW_EMBEDDED, "allowEmbedded", false, &sa)
                | flag(FLAG_ALWAYS_FOCUSABLE, "alwaysFocusable", false, &sa)
                | flag(
                    FLAG_AUTO_REMOVE_FROM_RECENTS,
                    "autoRemoveFromRecents",
                    false,
                    &sa,
                )
                | flag(
                    FLAG_RELINQUISH_TASK_IDENTITY,
                    "relinquishTaskIdentity",
                    false,
                    &sa,
                )
                | flag(FLAG_RESUME_WHILE_PAUSING, "resumeWhilePausing", false, &sa)
                | flag(FLAG_SHOW_WHEN_LOCKED, "showWhenLocked", false, &sa)
                | flag(
                    FLAG_SUPPORTS_PICTURE_IN_PICTURE,
                    "supportsPictureInPicture",
                    false,
                    &sa,
                )
                | flag(FLAG_TURN_SCREEN_ON, "turnScreenOn", false, &sa)
                | flag(
                    FLAG_PREFER_MINIMAL_POST_PROCESSING,
                    "preferMinimalPostProcessing",
                    false,
                    &sa,
                )
                | flag(
                    FLAG_ALLOW_UNTRUSTED_ACTIVITY_EMBEDDING,
                    "allowUntrustedActivityEmbedding",
                    false,
                    &sa,
                );
            a.private_flags |= flag(
                FLAG_INHERIT_SHOW_WHEN_LOCKED,
                "inheritShowWhenLocked",
                false,
                &sa,
            ) | flag(
                PRIVATE_FLAG_HOME_TRANSITION_SOUND,
                "playHomeTransitionSound",
                true,
                &sa,
            );
            a.color_mode = sa.int("colorMode", 0);
            a.document_launch_mode = sa.int("documentLaunchMode", 0);
            a.launch_mode = sa.int("launchMode", 0);
            a.lock_task_launch_mode = sa.int("lockTaskMode", 0);
            a.max_recents = sa.int("maxRecents", self.platform.recents_limit);
            a.persistable_mode = sa.integer("persistableMode", PERSIST_ROOT_ONLY);
            a.requested_vr_component = sa.string("enableVrMode");
            a.rotation_animation = sa.int("rotationAnimation", -1);
            a.soft_input_mode = sa.int("windowSoftInputMode", 0);
            a.config_changes = config_changes(
                sa.int("configChanges", 0),
                sa.int("recreateOnConfigChanges", 0),
            );
            let orientation = sa.int("screenOrientation", -1);
            let resize_mode = activity_resize_mode(pkg, &sa, orientation);
            a.screen_orientation = orientation;
            a.resize_mode = resize_mode;
            if sa.has_value("maxAspectRatio") && sa.kind("maxAspectRatio") == TYPE_FLOAT {
                set_max_aspect_ratio(&mut a, sa.float("maxAspectRatio", 0.0));
            }
            if sa.has_value("minAspectRatio") && sa.kind("minAspectRatio") == TYPE_FLOAT {
                set_min_aspect_ratio(&mut a, sa.float("minAspectRatio", 0.0));
            }
            if sa.has_value("enableOnBackInvokedCallback") {
                a.private_flags |= if sa.boolean("enableOnBackInvokedCallback", false) {
                    PRIVATE_FLAG_ENABLE_ON_BACK_INVOKED_CALLBACK
                } else {
                    PRIVATE_FLAG_DISABLE_ON_BACK_INVOKED_CALLBACK
                };
            }
        } else {
            a.launch_mode = 0;
            a.config_changes = 0;
            a.main.component.flags |= flag(FLAG_SINGLE_USER, "singleUser", false, &sa);
        }
        let affinity = sa.non_config_string("taskAffinity", NATIVE_CONFIG_VERSION);
        a.task_affinity = build_task_affinity_name(
            &pkg.package_name,
            pkg.task_affinity.as_deref(),
            affinity.as_deref(),
        )?;
        let visible = sa.boolean("visibleToInstantApps", false);
        if visible {
            a.main.component.flags |= FLAG_VISIBLE_TO_INSTANT_APP;
            pkg.set(b::VISIBLE_TO_INSTANT_APPS, true);
        }
        let category = sa.non_config_string("requiredDisplayCategory", 0);
        if let Some(c) = &category
            && validate_name(c, false).is_some()
        {
            return fail(
                "requiredDisplayCategory attribute can only consist of alphanumeric characters, '_', and '.'",
            );
        }
        a.required_display_category = category;
        a.require_content_uri_permission_from_caller =
            sa.int("requireContentUriPermissionFromCaller", 0);
        self.parse_activity_or_alias(a, pkg, e, &sa, receiver, false, visible)
    }

    /// `ParsedActivityUtils.parseActivityAlias`.
    pub(super) fn parse_activity_alias(
        &self,
        pkg: &mut Package,
        e: &Element,
        default_split: Option<&str>,
    ) -> Result<Activity> {
        let sa = self.obtain(e);
        let Some(target) = sa.non_config_string("targetActivity", NATIVE_CONFIG_VERSION) else {
            return fail("<activity-alias> does not specify android:targetActivity");
        };
        let Some(target) = build_class_name(&pkg.package_name, Some(&target)) else {
            return fail(format!("Empty class name in package {}", pkg.package_name));
        };
        let Some(t) = pkg
            .activities
            .iter()
            .find(|a| a.main.component.name == target)
        else {
            return fail(format!(
                "<activity-alias> target activity {target} not found in manifest"
            ));
        };
        // `ParsedActivityImpl.makeAlias`.
        let tc = &t.main.component;
        let mut a = Activity {
            target_activity: Some(target.clone()),
            config_changes: t.config_changes,
            private_flags: t.private_flags,
            launch_mode: t.launch_mode,
            lock_task_launch_mode: t.lock_task_launch_mode,
            document_launch_mode: t.document_launch_mode,
            screen_orientation: t.screen_orientation,
            task_affinity: t.task_affinity.clone(),
            theme: t.theme,
            soft_input_mode: t.soft_input_mode,
            ui_options: t.ui_options,
            parent_activity_name: t.parent_activity_name.clone(),
            max_recents: t.max_recents,
            window_layout: t.window_layout.clone(),
            resize_mode: t.resize_mode,
            max_aspect_ratio: t.max_aspect_ratio,
            min_aspect_ratio: t.min_aspect_ratio,
            supports_size_changes: t.supports_size_changes,
            requested_vr_component: t.requested_vr_component.clone(),
            required_display_category: t.required_display_category.clone(),
            require_content_uri_permission_from_caller: t
                .require_content_uri_permission_from_caller,
            ..Activity::default()
        };
        a.main.component = Component {
            package_name: tc.package_name.clone(),
            flags: tc.flags,
            icon: tc.icon,
            logo: tc.logo,
            banner: tc.banner,
            label_res: tc.label_res,
            non_localized_label: tc.non_localized_label.clone(),
            description_res: tc.description_res,
            ..Component::default()
        };
        a.main.direct_boot_aware = t.main.direct_boot_aware;
        a.main.process_name = t.main.process_name.clone();
        self.parse_main_component(
            &mut a.main,
            &MainAttrs {
                tag: "<activity-alias>",
                direct_boot_aware: false,
                process: false,
                split_name: false,
                description: true,
            },
            pkg,
            &sa,
            default_split,
        )?;
        let visible = a.main.component.flags & FLAG_VISIBLE_TO_INSTANT_APP != 0;
        self.parse_activity_or_alias(a, pkg, e, &sa, false, true, visible)
    }

    /// `ParsedActivityUtils.parseActivityOrAlias`.
    #[allow(clippy::too_many_arguments)]
    fn parse_activity_or_alias(
        &self,
        mut a: Activity,
        pkg: &mut Package,
        e: &Element,
        sa: &TypedArray,
        receiver: bool,
        alias: bool,
        visible: bool,
    ) -> Result<Activity> {
        if let Some(parent) = sa.non_config_string("parentActivityName", NATIVE_CONFIG_VERSION)
            && let Some(cls) = build_class_name(&pkg.package_name, Some(&parent))
        {
            a.parent_activity_name = Some(cls);
        }
        let p = sa.non_config_string("permission", 0);
        a.permission = permission(if alias {
            p
        } else {
            p.or(pkg.permission.clone())
        });
        if let Some(certs) = self.known_activity_embedding_certs(sa)? {
            let mut set = ArraySet::default();
            certs.iter().for_each(|c| set.add(&c.to_uppercase()));
            a.known_activity_embedding_certs = Some(set);
        }
        let set_exported = sa.has_value("exported");
        if set_exported {
            a.main.exported = sa.boolean("exported", false);
        }
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "intent-filter" => {
                    let info =
                        self.parse_activity_intent_filter(pkg, c, &mut a, !receiver, visible)?;
                    if let Some(info) = info {
                        a.main.order = a.main.order.max(info.filter.order);
                        a.main.component.intents.push(info);
                    }
                }
                "meta-data" => self.add_meta_data(pkg, &mut a.main.component, c)?,
                "property" => self.add_property(pkg, &mut a.main.component, c)?,
                "preferred" if !receiver && !alias => {
                    if let Some(info) =
                        self.parse_activity_intent_filter(pkg, c, &mut a, true, visible)?
                    {
                        pkg.preferred_activity_filters
                            .push((a.main.component.name.clone(), info));
                    }
                }
                "layout" if !receiver && !alias => {
                    a.window_layout = Some(self.parse_window_layout(c)?);
                }
                _ => {}
            }
        }
        let meta = a.main.component.meta_data();
        if !alias
            && a.launch_mode != LAUNCH_SINGLE_INSTANCE_PER_TASK
            && let Some(BundleValue::String(Some(mode))) = meta.get("android.activity.launch_mode")
            && mode == "singleInstancePerTask"
        {
            a.launch_mode = LAUNCH_SINGLE_INSTANCE_PER_TASK;
        }
        if !alias {
            let mut remote = sa.boolean("canDisplayOnRemoteDevices", true);
            if matches!(
                meta.get("android.can_display_on_remote_devices"),
                Some(BundleValue::Bool(false))
            ) {
                remote = false;
            }
            if remote {
                a.main.component.flags |= FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES;
            }
        }
        // `resolveActivityWindowLayout`.
        if meta.contains("android.activity_window_layout_affinity")
            && a.window_layout
                .as_ref()
                .is_none_or(|l| l.affinity.is_none())
        {
            let affinity = match meta.get("android.activity_window_layout_affinity") {
                Some(BundleValue::String(s)) => s.clone(),
                _ => None,
            };
            match &mut a.window_layout {
                Some(l) => l.affinity = affinity,
                None => {
                    a.window_layout = Some(WindowLayout {
                        width: -1,
                        width_fraction: -1.0,
                        height: -1,
                        height_fraction: -1.0,
                        gravity: GRAVITY_NO_GRAVITY,
                        min_width: -1,
                        min_height: -1,
                        affinity,
                    })
                }
            }
        }
        if !set_exported {
            let has_filters = !a.main.component.intents.is_empty();
            if has_filters {
                self.input.borrow_mut().defer(
                    format!(
                        "{}: Targeting S+ requires that an explicit value for android:exported be defined when intent filters are present",
                        a.main.component.name
                    ),
                    MISSING_EXPORTED_FLAG,
                )?;
            }
            a.main.exported = has_filters;
        }
        Ok(a)
    }

    /// `ParsedActivityUtils.parseIntentFilter`.
    fn parse_activity_intent_filter(
        &self,
        pkg: &mut Package,
        e: &Element,
        a: &mut Activity,
        allow_implicit: bool,
        visible: bool,
    ) -> Result<Option<ParsedIntentInfo>> {
        let rules = FilterRules {
            visible_to_ephemeral: visible,
            allow_auto_verify: true,
            allow_implicit_ephemeral_visibility: allow_implicit,
            fail_on_no_actions: true,
        };
        let info = self.parse_main_intent_filter(pkg, e, rules)?;
        if let Some(i) = &info {
            match i.filter.instant_app_visibility {
                0 => {}
                v => {
                    a.main.component.flags |= FLAG_VISIBLE_TO_INSTANT_APP;
                    if v == VISIBILITY_IMPLICIT {
                        a.main.component.flags |= FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP;
                    }
                }
            }
        }
        Ok(info)
    }

    /// `ParsedActivityUtils.parseActivityWindowLayout`. Sizes are in the
    /// default display's pixels.
    fn parse_window_layout(&self, e: &Element) -> Result<WindowLayout> {
        let sa = self.obtain(e);
        let unsupported = |what: &str| Error::Unsupported(format!("<layout android:{what}>"));
        let density = || {
            self.platform
                .density_dpi
                .map(|d| d as f32 * (1.0 / 160.0))
                .ok_or_else(|| unsupported("a size without the display's density"))
        };
        let mut l = WindowLayout {
            width: -1,
            width_fraction: -1.0,
            height: -1,
            height_fraction: -1.0,
            gravity: GRAVITY_CENTER,
            min_width: -1,
            min_height: -1,
            affinity: None,
        };
        for (attr, size, fraction) in [
            ("defaultWidth", &mut l.width, &mut l.width_fraction),
            ("defaultHeight", &mut l.height, &mut l.height_fraction),
        ] {
            match sa.kind(attr) {
                TYPE_FRACTION => *fraction = sa.fraction(attr, -1.0),
                TYPE_DIMENSION => {
                    *size = sa
                        .dimension_pixel_size(attr, -1, density()?)
                        .ok_or_else(|| unsupported(attr))?
                }
                _ => {}
            }
        }
        l.gravity = sa.int("gravity", GRAVITY_CENTER);
        for (attr, size) in [
            ("minWidth", &mut l.min_width),
            ("minHeight", &mut l.min_height),
        ] {
            if sa.has_value(attr) {
                *size = sa
                    .dimension_pixel_size(attr, -1, density()?)
                    .ok_or_else(|| unsupported(attr))?;
            }
        }
        l.affinity = sa.non_config_string("windowLayoutAffinity", 0);
        Ok(l)
    }

    /// `ParsedActivityImpl.makeAppDetailsActivity`.
    pub(super) fn app_details_activity(&self, pkg: &Package, affinity: Option<String>) -> Activity {
        let mut a = Activity {
            theme: self
                .platform
                .framework
                .id("style", "Theme.NoDisplay")
                .unwrap_or_default() as i32,
            ui_options: pkg.ui_options,
            task_affinity: affinity,
            max_recents: self.platform.recents_limit,
            config_changes: config_changes(0, 0),
            persistable_mode: PERSIST_NEVER,
            resize_mode: RESIZE_MODE_FORCE_RESIZEABLE,
            ..Activity::default()
        };
        a.main.component.package_name = pkg.package_name.clone();
        a.main.component.name = APP_DETAILS_ACTIVITY_CLASS_NAME.into();
        a.main.exported = true;
        a.main.process_name = Some(
            pkg.process_name
                .clone()
                .unwrap_or_else(|| pkg.package_name.clone()),
        );
        if pkg.get(b::HARDWARE_ACCELERATED) {
            a.main.component.flags |= FLAG_HARDWARE_ACCELERATED;
        }
        a
    }

    /// `ParsedServiceUtils.parseService`.
    pub(super) fn parse_service(
        &self,
        pkg: &mut Package,
        e: &Element,
        default_split: Option<&str>,
    ) -> Result<Service> {
        let mut s = Service::default();
        let sa = self.obtain(e);
        self.parse_main_component(
            &mut s.main,
            &MainAttrs {
                tag: "service",
                direct_boot_aware: true,
                process: true,
                split_name: true,
                description: true,
            },
            pkg,
            &sa,
            default_split,
        )?;
        let set_exported = sa.has_value("exported");
        if set_exported {
            s.main.exported = sa.boolean("exported", false);
        }
        let p = sa.non_config_string("permission", 0);
        s.permission = permission(p.or(pkg.permission.clone()));
        s.foreground_service_type = sa.int("foregroundServiceType", 0);
        s.main.component.flags |= flag(SERVICE_STOP_WITH_TASK, "stopWithTask", false, &sa)
            | flag(SERVICE_ISOLATED_PROCESS, "isolatedProcess", false, &sa)
            | flag(SERVICE_EXTERNAL_SERVICE, "externalService", false, &sa)
            | flag(SERVICE_USE_APP_ZYGOTE, "useAppZygote", false, &sa)
            | flag(
                SERVICE_ALLOW_SHARED_ISOLATED_PROCESS,
                "allowSharedIsolatedProcess",
                false,
                &sa,
            )
            | flag(FLAG_SINGLE_USER, "singleUser", false, &sa);
        if self
            .platform
            .flag("android.multiuser.enable_system_user_only_for_services_and_providers")
        {
            s.main.component.flags |= flag(FLAG_SYSTEM_USER_ONLY, "systemUserOnly", false, &sa);
        }
        let visible = sa.boolean("visibleToInstantApps", false);
        if visible {
            s.main.component.flags |= FLAG_VISIBLE_TO_INSTANT_APP;
            pkg.set(b::VISIBLE_TO_INSTANT_APPS, true);
        }
        if pkg.get(b::CANT_SAVE_STATE)
            && s.main.process_name.as_deref() == Some(pkg.package_name.as_str())
        {
            return fail("Heavy-weight applications can not have services in main process");
        }
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "intent-filter" => {
                    if let Some(info) =
                        self.parse_main_intent_filter(pkg, c, FilterRules::plain(visible))?
                    {
                        s.main.order = s.main.order.max(info.filter.order);
                        s.main.component.intents.push(info);
                    }
                }
                "meta-data" => self.add_meta_data(pkg, &mut s.main.component, c)?,
                "property" => self.add_property(pkg, &mut s.main.component, c)?,
                _ => {}
            }
        }
        if !set_exported {
            let has_filters = !s.main.component.intents.is_empty();
            if has_filters {
                self.input.borrow_mut().defer(
                    format!(
                        "{}: Targeting S+ requires that an explicit value for android:exported be defined when intent filters are present",
                        s.main.component.name
                    ),
                    MISSING_EXPORTED_FLAG,
                )?;
            }
            s.main.exported = has_filters;
        }
        Ok(s)
    }

    /// `ParsedProviderUtils.parseProvider`.
    pub(super) fn parse_provider(
        &self,
        pkg: &mut Package,
        e: &Element,
        default_split: Option<&str>,
    ) -> Result<Provider> {
        let mut p = Provider::default();
        let sa = self.obtain(e);
        self.parse_main_component(
            &mut p.main,
            &MainAttrs {
                tag: "provider",
                direct_boot_aware: true,
                process: true,
                split_name: true,
                description: true,
            },
            pkg,
            &sa,
            default_split,
        )?;
        let authority = sa.non_config_string("authorities", 0);
        p.syncable = sa.boolean("syncable", false);
        p.main.exported = sa.boolean("exported", pkg.target_sdk_version < 17);
        let perm = sa.non_config_string("permission", 0);
        let read = sa.non_config_string("readPermission", 0).or(perm.clone());
        p.read_permission = permission(read.or(pkg.permission.clone()));
        let write = sa.non_config_string("writePermission", 0).or(perm);
        p.write_permission = permission(write.or(pkg.permission.clone()));
        p.grant_uri_permissions = sa.boolean("grantUriPermissions", false);
        p.force_uri_permissions = sa.boolean("forceUriPermissions", false);
        p.multi_process = sa.boolean("multiprocess", false);
        p.init_order = sa.int("initOrder", 0);
        p.main.component.flags |= flag(FLAG_SINGLE_USER, "singleUser", false, &sa);
        if self
            .platform
            .flag("android.multiuser.enable_system_user_only_for_services_and_providers")
        {
            p.main.component.flags |= flag(FLAG_SYSTEM_USER_ONLY, "systemUserOnly", false, &sa);
        }
        let visible = sa.boolean("visibleToInstantApps", false);
        if visible {
            p.main.component.flags |= FLAG_VISIBLE_TO_INSTANT_APP;
            pkg.set(b::VISIBLE_TO_INSTANT_APPS, true);
        }
        if pkg.get(b::CANT_SAVE_STATE)
            && p.main.process_name.as_deref() == Some(pkg.package_name.as_str())
        {
            return fail("Heavy-weight applications can not have providers in main process");
        }
        match authority {
            None => return fail("<provider> does not include authorities attribute"),
            Some(a) if a.is_empty() => return fail("<provider> has empty authorities attribute"),
            Some(a) => p.authority = Some(a),
        }
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "intent-filter" => {
                    if let Some(info) =
                        self.parse_main_intent_filter(pkg, c, FilterRules::plain(visible))?
                    {
                        p.main.order = p.main.order.max(info.filter.order);
                        p.main.component.intents.push(info);
                    }
                }
                "meta-data" => self.add_meta_data(pkg, &mut p.main.component, c)?,
                "property" => self.add_property(pkg, &mut p.main.component, c)?,
                "grant-uri-permission" => {
                    let sa = self.obtain(c);
                    if let Some(pa) = first_pattern(&sa)? {
                        p.uri_permission_patterns.push(pa);
                        p.grant_uri_permissions = true;
                    }
                }
                "path-permission" => {
                    let sa = self.obtain(c);
                    let perm = sa.non_config_string("permission", 0);
                    let read = sa.non_config_string("readPermission", 0).or(perm.clone());
                    let write = sa.non_config_string("writePermission", 0).or(perm);
                    if read.is_none() && write.is_none() {
                        continue;
                    }
                    if let Some(pattern) = first_pattern(&sa)? {
                        p.path_permissions.push(PathPermission {
                            pattern,
                            read_permission: read,
                            write_permission: write,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(p)
    }

    /// `ParsedPermissionUtils.parsePermission`; `None` for a permission
    /// past its `maxSdkVersion`.
    pub(super) fn parse_permission(
        &self,
        pkg: &mut Package,
        e: &Element,
    ) -> Result<Option<Permission>> {
        let mut p = Permission::default();
        {
            let sa = self.obtain(e);
            self.parse_component(&mut p.component, "<permission>", pkg, &sa, true)?;
            let max = sa.int("maxSdkVersion", -1);
            if max != -1 && max < self.platform.sdk {
                return Ok(None);
            }
            if sa.has_value("backgroundPermission") {
                let apk_in_apex = self.flags & super::PARSE_APK_IN_APEX != 0;
                if pkg.package_name == "android"
                    || (self
                        .platform
                        .flag("android.permission.flags.replace_body_sensor_permission_enabled")
                        && apk_in_apex)
                {
                    p.background_permission = sa.non_resource_string("backgroundPermission");
                }
            }
            p.group = sa.non_resource_string("permissionGroup");
            p.request_res = sa.resource_id("request", 0);
            p.protection_level = sa.int("protectionLevel", 0);
            p.component.flags = sa.int("permissionFlags", 0);
            let known = sa.resource_id("knownCerts", 0);
            if known != 0 {
                let res = self.ctx.res;
                let mut set = ArraySet::default();
                if res.type_name(known as u32) == Some("array") {
                    if let Some(certs) = res.string_array(known as u32) {
                        for c in certs {
                            let c =
                                c.ok_or_else(|| Error::Unsupported("a null in knownCerts".into()))?;
                            set.add(&c.to_uppercase());
                        }
                        p.known_certs = Some(set);
                    }
                } else if let Some(c) = res.resource_string(known as u32) {
                    set.add(&c.to_uppercase());
                    p.known_certs = Some(set);
                }
            } else if let Some(c) = sa.string("knownCerts") {
                let mut set = ArraySet::default();
                set.add(&c.to_uppercase());
                p.known_certs = Some(set);
            }
            let protection = p.protection_level & PROTECTION_MASK_BASE;
            if protection != PROTECTION_DANGEROUS || p.component.package_name != "android" {
                p.component.flags &=
                    !(PERMISSION_FLAG_HARD_RESTRICTED | PERMISSION_FLAG_SOFT_RESTRICTED);
            } else if p.component.flags & PERMISSION_FLAG_HARD_RESTRICTED != 0
                && p.component.flags & PERMISSION_FLAG_SOFT_RESTRICTED != 0
            {
                return fail(format!(
                    "Permission cannot be both soft and hard restricted: {}",
                    p.component.name
                ));
            }
        }
        p.protection_level = fix_protection_level(p.protection_level);
        let other = p.protection_level
            & !PROTECTION_MASK_BASE
            & !(PROTECTION_FLAG_APPOP | PROTECTION_FLAG_INSTANT | PROTECTION_FLAG_RUNTIME_ONLY);
        let base = p.protection_level & PROTECTION_MASK_BASE;
        if other != 0 && base != PROTECTION_SIGNATURE && base != PROTECTION_INTERNAL {
            return fail(
                "<permission> protectionLevel specifies a non-instant, non-appop, non-runtimeOnly flag but is not based on signature or internal type",
            );
        }
        self.parse_all_meta_data(pkg, &mut p.component, e)?;
        Ok(Some(p))
    }

    /// `ParsedPermissionUtils.parsePermissionTree`.
    pub(super) fn parse_permission_tree(
        &self,
        pkg: &mut Package,
        e: &Element,
    ) -> Result<Permission> {
        let mut p = Permission::default();
        let sa = self.obtain(e);
        self.parse_component(&mut p.component, "<permission-tree>", pkg, &sa, false)?;
        let name = &p.component.name;
        let segments = name
            .find('.')
            .filter(|&i| i > 0)
            .and_then(|i| name[i + 1..].find('.'));
        if segments.is_none() {
            return fail(format!(
                "<permission-tree> name has less than three segments: {name}"
            ));
        }
        p.protection_level = 0;
        p.tree = true;
        self.parse_all_meta_data(pkg, &mut p.component, e)?;
        Ok(p)
    }

    /// `ParsedPermissionUtils.parsePermissionGroup`.
    pub(super) fn parse_permission_group(
        &self,
        pkg: &mut Package,
        e: &Element,
    ) -> Result<PermissionGroup> {
        let mut g = PermissionGroup::default();
        let sa = self.obtain(e);
        self.parse_component(&mut g.component, "<permission-group>", pkg, &sa, true)?;
        g.request_detail_res = sa.resource_id("requestDetail", 0);
        g.background_request_res = sa.resource_id("backgroundRequest", 0);
        g.background_request_detail_res = sa.resource_id("backgroundRequestDetail", 0);
        g.request_res = sa.resource_id("request", 0);
        g.priority = sa.int("priority", 0);
        g.component.flags = sa.int("permissionGroupFlags", 0);
        self.parse_all_meta_data(pkg, &mut g.component, e)?;
        Ok(g)
    }

    /// `ParsedInstrumentationUtils.parseInstrumentation`.
    pub(super) fn parse_instrumentation(
        &self,
        pkg: &mut Package,
        e: &Element,
    ) -> Result<Instrumentation> {
        let mut i = Instrumentation::default();
        let sa = self.obtain(e);
        self.parse_component(&mut i.component, "<instrumentation>", pkg, &sa, false)?;
        i.target_package = sa.non_resource_string("targetPackage");
        i.target_processes = sa.non_resource_string("targetProcesses");
        i.handle_profiling = sa.boolean("handleProfiling", false);
        i.functional_test = sa.boolean("functionalTest", false);
        self.parse_all_meta_data(pkg, &mut i.component, e)?;
        Ok(i)
    }

    /// `ParsedAttributionUtils.parseAttribution`.
    pub(super) fn parse_attribution(&self, e: &Element) -> Result<Attribution> {
        let sa = self.obtain(e);
        let Some(tag) = sa.non_config_string("tag", 0) else {
            return fail("<attribution> does not specify android:tag");
        };
        if tag.encode_utf16().count() > 50 {
            return fail("android:tag is too long. Max length is 50");
        }
        let label = sa.resource_id("label", 0);
        if label == 0 {
            return fail("<attribution> does not specify android:label");
        }
        let mut inherit_from = Vec::new();
        for c in &e.children {
            if c.name != "inherit-from" {
                return fail(format!("Bad element under <attribution>: {}", c.name));
            }
            let t = self
                .obtain(c)
                .non_config_string("tag", 0)
                .ok_or_else(|| Error::Unsupported("<inherit-from> without a tag".into()))?;
            inherit_from.push(t);
        }
        Ok(Attribution {
            tag,
            label,
            inherit_from,
        })
    }

    /// `ParsedProcessUtils.parseProcesses`.
    pub(super) fn parse_processes(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let mut denied: Option<ArraySet> = None;
        let mut processes = ArrayMap::default();
        for c in &e.children {
            match c.name.as_str() {
                "deny-permission" | "allow-permission" => {
                    self.deny_or_allow(&mut denied, c);
                }
                "process" => {
                    let proc = self.parse_process(pkg, denied.as_ref(), c)?;
                    if processes.contains(&proc.name) {
                        return fail(format!("<process> specified existing name '{}'", proc.name));
                    }
                    processes.put(&proc.name.clone(), proc);
                }
                _ => {}
            }
        }
        pkg.processes = processes;
        Ok(())
    }

    /// `parseDenyPermission` and `parseAllowPermission`: only INTERNET.
    fn deny_or_allow(&self, perms: &mut Option<ArraySet>, e: &Element) {
        let perm = self.obtain(e).non_config_string("name", 0);
        if perm.as_deref() != Some("android.permission.INTERNET") {
            return;
        }
        if e.name == "deny-permission" {
            perms
                .get_or_insert_with(ArraySet::default)
                .add("android.permission.INTERNET");
        } else if let Some(p) = perms {
            let mut rest = ArraySet::default();
            p.iter()
                .filter(|s| *s != "android.permission.INTERNET")
                .for_each(|s| rest.add(s));
            *p = rest;
        }
    }

    fn parse_process(
        &self,
        pkg: &Package,
        denied: Option<&ArraySet>,
        e: &Element,
    ) -> Result<Process> {
        let sa = self.obtain(e);
        let name = sa.non_config_string("process", 0);
        let name = build_process_name(
            &pkg.package_name,
            Some(&pkg.package_name),
            name.as_deref(),
            self.flags,
        )?
        .unwrap_or_default();
        let class = build_class_name(
            &pkg.package_name,
            sa.non_config_string("name", 0).as_deref(),
        );
        let mut by_package = ArrayMap::default();
        by_package.put(&pkg.package_name, class);
        let mut proc = Process {
            name,
            app_class_names_by_package: by_package,
            denied_permissions: denied.cloned().unwrap_or_default(),
            gwp_asan_mode: sa.int("gwpAsanMode", -1),
            memtag_mode: sa.int("memtagMode", -1),
            native_heap_zero_initialized: -1,
            use_embedded_dex: false,
        };
        if sa.has_value("nativeHeapZeroInitialized") {
            proc.native_heap_zero_initialized = if sa.boolean("nativeHeapZeroInitialized", false) {
                1
            } else {
                0
            };
        }
        if self.platform.flag(
            "com.android.internal.pm.pkg.component.flags.enable_per_process_use_embedded_dex_attr",
        ) {
            proc.use_embedded_dex = sa.boolean("useEmbeddedDex", false);
        }
        let mut denied = Some(proc.denied_permissions.clone());
        for c in &e.children {
            if c.name == "deny-permission" || c.name == "allow-permission" {
                self.deny_or_allow(&mut denied, c);
            }
        }
        proc.denied_permissions = denied.unwrap_or_default();
        Ok(proc)
    }

    /// `ParsedApexSystemServiceUtils.parseApexSystemService`.
    pub(super) fn parse_apex_system_service(&self, e: &Element) -> Result<ApexSystemService> {
        let sa = self.obtain(e);
        let Some(name) = sa.string("name").filter(|n| !n.is_empty()) else {
            return fail("<apex-system-service> does not have name attribute");
        };
        Ok(ApexSystemService {
            name,
            jar_path: sa.string("path").filter(|p| !p.is_empty()),
            min_sdk_version: sa.string("minSdkVersion"),
            max_sdk_version: sa.string("maxSdkVersion"),
            init_order: sa.int("initOrder", 0),
        })
    }
}

/// `getActivityResizeMode`.
fn activity_resize_mode(pkg: &Package, sa: &TypedArray, orientation: i32) -> i32 {
    let app = pkg.resizeable_activity;
    if sa.has_value("resizeableActivity") || app.is_some() {
        return if sa.boolean("resizeableActivity", app == Some(true)) {
            RESIZE_MODE_RESIZEABLE
        } else {
            RESIZE_MODE_UNRESIZEABLE
        };
    }
    if pkg.get(b::RESIZEABLE_ACTIVITY_VIA_SDK_VERSION) {
        return RESIZE_MODE_RESIZEABLE_VIA_SDK_VERSION;
    }
    match orientation {
        1 | 7 | 9 | 12 => RESIZE_MODE_FORCE_RESIZABLE_PORTRAIT_ONLY,
        0 | 6 | 8 | 11 => RESIZE_MODE_FORCE_RESIZABLE_LANDSCAPE_ONLY,
        SCREEN_ORIENTATION_LOCKED => RESIZE_MODE_FORCE_RESIZABLE_PRESERVE_ORIENTATION,
        _ => RESIZE_MODE_FORCE_RESIZEABLE,
    }
}

/// `PermissionInfo.fixProtectionLevel`.
fn fix_protection_level(mut level: i32) -> i32 {
    if level == PROTECTION_SIGNATURE_OR_SYSTEM {
        level = PROTECTION_SIGNATURE | PROTECTION_FLAG_PRIVILEGED;
    }
    if level & PROTECTION_FLAG_VENDOR_PRIVILEGED != 0 && level & PROTECTION_FLAG_PRIVILEGED == 0 {
        level &= !PROTECTION_FLAG_VENDOR_PRIVILEGED;
    }
    level
}

/// A `PatternMatcher`. An advanced glob's parsed form is not written yet.
fn pattern(p: String, kind: i32) -> Result<PatternMatcher> {
    if kind == PATTERN_ADVANCED_GLOB {
        return Err(Error::Unsupported("an advanced glob pattern".into()));
    }
    PatternMatcher::new(&p, kind).map_err(Error::Parse)
}

/// A grant or path permission's pattern: advanced, simple glob, prefix,
/// suffix, then literal.
fn first_pattern(sa: &TypedArray) -> Result<Option<PatternMatcher>> {
    for (attr, kind) in [
        ("pathAdvancedPattern", PATTERN_ADVANCED_GLOB),
        ("pathPattern", PATTERN_SIMPLE_GLOB),
        ("pathPrefix", PATTERN_PREFIX),
        ("pathSuffix", PATTERN_SUFFIX),
        ("path", PATTERN_LITERAL),
    ] {
        if let Some(v) = sa.non_config_string(attr, 0) {
            return pattern(v, kind).map(Some);
        }
    }
    Ok(None)
}
