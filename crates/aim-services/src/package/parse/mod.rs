//! The package parser: an APK's manifest, and its resources where the
//! manifest's values reference them, made into the `PackageImpl` the
//! original's `ParsingPackageUtils` makes, written in the parser cache's
//! format (docs/m4-packagemanager.md, D4). The image's parser cache is the
//! oracle: the entry for each system package must equal the original's
//! byte for byte (`aim-package-parse`).
//!
//! This parser makes what `PackageParser2.parsePackage` caches during a
//! scan: no certificates (`PARSE_COLLECT_CERTIFICATES` is not set), and
//! nothing the scan sets afterwards. What it does not port yet it refuses
//! with [`Error::Unsupported`] rather than guess, among them split APKs,
//! `<key-sets>`, `<install-constraints>`, `<extension-sdk>` and advanced
//! glob patterns; none of the image's packages has them.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `com.android.internal.pm.pkg.parsing.ParsingPackageUtils`,
//! `android.content.pm.parsing.ApkLiteParseUtils` and
//! `FrameworkParsingPackageUtils`), Copyright (C) The Android Open Source
//! Project, Licensed under the Apache License, Version 2.0.

mod attrs;
pub mod component;
mod components;
pub mod package;
pub mod parcel;
pub mod platform;
pub mod resources;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use aim_apps::apk::Apk;
use aim_apps::res::Element;

use attrs::{ANDROID, Ctx, NATIVE_CONFIG_VERSION, TypedArray, attr_bool, attr_value};
use component::{Property, PropertyValue};
use package::{ConfigurationInfo, FeatureInfo, Package, QueriesIntent, booleans as b};
use parcel::{ArraySet, Bundle, BundleValue};
use resources::{
    Config, Resources, TYPE_FIRST_INT, TYPE_FLOAT, TYPE_INT_BOOLEAN, TYPE_LAST_INT, TYPE_STRING,
    Table,
};

/// `ParsingPackageUtils.PARSE_*`.
pub const PARSE_IS_SYSTEM_DIR: i32 = 1 << 4;
pub const PARSE_APK_IN_APEX: i32 = 1 << 9;
const PARSE_IGNORE_PROCESSES: i32 = 1 << 1;
const PARSE_EXTERNAL_STORAGE: i32 = 1 << 3;
const PARSE_APEX: i32 = 1 << 10;

// `Build.VERSION_CODES`.
const DONUT: i32 = 4;
const FROYO: i32 = 8;
const ICE_CREAM_SANDWICH: i32 = 14;
const N: i32 = 24;
const O: i32 = 26;
const O_MR1: i32 = 27;
const P: i32 = 28;
const Q: i32 = 29;
const R: i32 = 30;
const VANILLA_ICE_CREAM: i32 = 35;
/// `Build.VERSION_CODES.CUR_DEVELOPMENT`.
const CUR_DEVELOPMENT: i32 = 10000;

/// `ParsingPackageUtils.DEFAULT_PRE_O_MAX_ASPECT_RATIO`.
const DEFAULT_PRE_O_MAX_ASPECT_RATIO: f32 = 1.86;
const APP_DETAILS_ACTIVITY_CLASS_NAME: &str = "android.app.AppDetailsActivity";

/// What a parse depends on besides the APK: the platform it runs on.
pub struct Platform {
    /// `Build.VERSION.SDK_INT` (and `RESOURCES_SDK_INT`, the same on a
    /// release build).
    pub sdk: i32,
    /// `Build.VERSION.ACTIVE_CODENAMES`.
    pub codenames: Vec<String>,
    /// The system features (`hasSystemFeature`).
    pub features: HashSet<String>,
    /// The aconfig flags' values, by `package.name`, and the packages that
    /// declare flags.
    pub flags: HashMap<String, bool>,
    pub flag_packages: HashSet<String>,
    /// `PermissionManager.getSplitPermissions`.
    pub split_permissions: Vec<SplitPermission>,
    /// The default locale's language and region (the device's, which
    /// `persist.sys.locale` holds at run time).
    pub locale: ([u8; 2], [u8; 2]),
    /// `config_useRoundIcon`.
    pub use_round_icon: bool,
    /// `ActivityTaskManager.getDefaultAppRecentsLimitStatic`.
    pub recents_limit: i32,
    /// framework-res.apk's resource table, and its static overlays in the
    /// order the zygote loads them.
    pub framework: Table,
    pub framework_overlays: Vec<resources::Overlay>,
    /// The framework's attributes by name (`android:<name>`).
    pub framework_attrs: HashMap<String, u32>,
    /// The default display's density (`DisplayMetrics.densityDpi`), which
    /// sizes in a `<layout>` are converted with.
    pub density_dpi: Option<i32>,
}

impl Platform {
    fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    /// `AconfigFlags.getFlagValue` over the new storage: none for a
    /// package no container declares, false for a flag its package lacks.
    fn flag_value(&self, name: &str) -> Option<bool> {
        let (package, _) = name.rsplit_once('.')?;
        self.flag_packages
            .contains(package)
            .then(|| self.flag(name))
    }

    /// The resources' configuration while parsing (`ResourcesImpl` for
    /// PackageManager's `Resources`).
    fn config(&self) -> Config {
        Config {
            language: self.locale.0,
            country: self.locale.1,
            sdk_version: self.sdk as u16,
            ..Config::default()
        }
    }
}

/// `PermissionManager.SplitPermissionInfo`.
pub struct SplitPermission {
    pub name: String,
    pub new_permissions: Vec<String>,
    pub target_sdk: i32,
}

#[derive(Debug)]
pub enum Error {
    /// The original fails too (`INSTALL_PARSE_FAILED_*`), or skips it.
    Parse(String),
    /// Something this parser does not port yet.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(s) => write!(f, "parse failed: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Parse(msg.into()))
}

/// `ParseInput`'s deferred errors (`ParseTypeImpl`): each change id is an
/// error when the package targets a level after the one it is enabled
/// after.
#[derive(Default)]
struct Input {
    target_sdk: Option<i32>,
    deferred: Vec<(i32, Option<String>)>,
}

/// `ParseInput.DeferredError`: `@EnabledAfter` of each.
const MISSING_APP_TAG: i32 = Q;
const EMPTY_INTENT_ACTION_CATEGORY: i32 = Q;
const MISSING_EXPORTED_FLAG: i32 = R;

impl Input {
    fn defer(&mut self, msg: String, enabled_after: i32) -> Result<()> {
        match self.target_sdk {
            Some(t) if t > enabled_after => fail(msg),
            Some(_) => Ok(()),
            None => {
                if !self.deferred.iter().any(|(e, _)| *e == enabled_after) {
                    self.deferred.push((enabled_after, Some(msg)));
                }
                Ok(())
            }
        }
    }

    /// `enableDeferredError`.
    fn enable(&mut self, target_sdk: i32) -> Result<()> {
        self.target_sdk = Some(target_sdk);
        for (after, msg) in self.deferred.iter_mut().rev() {
            if target_sdk > *after {
                return fail(msg.take().unwrap_or_default());
            }
        }
        Ok(())
    }
}

/// One parse: the platform, the resources, the parse flags.
struct Parser<'a> {
    platform: &'a Platform,
    ctx: Ctx<'a>,
    flags: i32,
    input: RefCell<Input>,
}

/// Parses the package at `host` (an APK or a directory holding one) that
/// the guest sees at `path`, with the scan's `flags`.
pub fn parse(host: &Path, path: &str, flags: i32, platform: &Platform) -> Result<Package> {
    let (host, path) = descend(host, path)?;
    let (apk_host, apk_path, code_path) = if host.is_dir() {
        let apks: Vec<_> = std::fs::read_dir(&host)
            .map_err(|e| Error::Parse(format!("{}: {e}", host.display())))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".apk"))
            .collect();
        match apks.as_slice() {
            [one] => {
                let name = one.file_name().to_string_lossy().into_owned();
                (one.path(), format!("{path}/{name}"), path.clone())
            }
            [] => return fail("No packages found in split"),
            _ => return Err(Error::Unsupported("split APKs".into())),
        }
    } else {
        (host.clone(), path.clone(), path.clone())
    };
    let apk = Apk::open(&apk_host).map_err(|e| Error::Parse(e.to_string()))?;
    let manifest = apk.manifest().map_err(|e| Error::Parse(e.to_string()))?;
    let app_table = apk.file("resources.arsc").ok().map(|b| Table::parse(&b));
    let app_table = app_table
        .transpose()
        .map_err(|e| Error::Parse(e.to_string()))?;
    let mut tables = vec![&platform.framework];
    if let Some(t) = &app_table {
        tables.push(t);
    }
    let res = Resources {
        tables,
        overlays: &platform.framework_overlays,
        config: platform.config(),
    };
    let parser = Parser {
        platform,
        ctx: Ctx {
            res: &res,
            attrs: &platform.framework_attrs,
            error: RefCell::new(None),
        },
        flags,
        input: RefCell::new(Input::default()),
    };
    let mut pkg = parser.parse_base_apk(&manifest, &apk_path, &code_path)?;
    if let Some(e) = parser.ctx.error.take() {
        return fail(e);
    }
    // `set32BitAbiPreferred(lite.isUse32bitAbi())`.
    let use32 = manifest
        .children
        .iter()
        .find(|c| c.name == "application")
        .is_some_and(|a| attr_bool(a, ANDROID, "use32bitAbi", false));
    pkg.set(b::USE_32_BIT_ABI, use32);
    Ok(pkg)
}

/// `parsePackage`: a directory holding a single directory is that one.
fn descend(host: &Path, path: &str) -> Result<(std::path::PathBuf, String)> {
    if host.is_dir() {
        let entries: Vec<_> = std::fs::read_dir(host)
            .map_err(|e| Error::Parse(format!("{}: {e}", host.display())))?
            .filter_map(|e| e.ok())
            .collect();
        if let [one] = entries.as_slice()
            && one.path().is_dir()
        {
            let name = one.file_name().to_string_lossy().into_owned();
            return descend(&one.path(), &format!("{path}/{name}"));
        }
    }
    Ok((host.to_owned(), path.to_owned()))
}

/// `FrameworkParsingPackageUtils.validateName`.
fn validate_name(name: &str, require_separator: bool) -> Option<String> {
    let mut has_sep = false;
    let mut front = true;
    for c in name.chars() {
        if c.is_ascii_alphabetic() {
            front = false;
            continue;
        }
        if !front && (c.is_ascii_digit() || c == '_') {
            continue;
        }
        if c == '.' {
            has_sep = true;
            front = true;
            continue;
        }
        return Some(format!("bad character '{c}'"));
    }
    (!has_sep && require_separator).then(|| "must have at least one '.' separator".into())
}

/// `ParsingUtils.buildClassName`.
fn build_class_name(pkg: &str, cls: Option<&str>) -> Option<String> {
    let cls = cls.filter(|c| !c.is_empty())?;
    Some(if cls.starts_with('.') {
        format!("{pkg}{cls}")
    } else if !cls.contains('.') {
        format!("{pkg}.{cls}")
    } else {
        cls.to_owned()
    })
}

/// `ComponentParseUtils.buildCompoundName`.
fn build_compound_name(pkg: &str, proc: &str, kind: &str) -> Result<String> {
    if let Some(sub) = proc.strip_prefix(':') {
        if sub.is_empty() {
            return fail(format!("Bad {kind} name {proc} in package {pkg}"));
        }
        if let Some(e) = validate_name(sub, false) {
            return fail(format!("Invalid {kind} name {proc} in package {pkg}: {e}"));
        }
        return Ok(format!("{pkg}{proc}"));
    }
    if proc != "system"
        && let Some(e) = validate_name(proc, true)
    {
        return fail(format!("Invalid {kind} name {proc} in package {pkg}: {e}"));
    }
    Ok(proc.to_owned())
}

/// `ComponentParseUtils.buildProcessName` (no separate processes).
fn build_process_name(
    pkg: &str,
    def_proc: Option<&str>,
    proc: Option<&str>,
    flags: i32,
) -> Result<Option<String>> {
    if flags & PARSE_IGNORE_PROCESSES != 0 && proc != Some("system") {
        return Ok(Some(def_proc.unwrap_or(pkg).to_owned()));
    }
    match proc.filter(|p| !p.is_empty()) {
        None => Ok(def_proc.map(str::to_owned)),
        Some(p) => build_compound_name(pkg, p, "process").map(Some),
    }
}

/// `ComponentParseUtils.buildTaskAffinityName`.
fn build_task_affinity_name(
    pkg: &str,
    def: Option<&str>,
    affinity: Option<&str>,
) -> Result<Option<String>> {
    match affinity {
        None => Ok(def.map(str::to_owned)),
        Some("") => Ok(None),
        Some(a) => build_compound_name(pkg, a, "taskAffinity").map(Some),
    }
}

impl Parser<'_> {
    fn obtain<'e>(&'e self, e: &'e Element) -> TypedArray<'e> {
        self.ctx.obtain(e)
    }

    /// `AconfigFlags.skipCurrentElement`: an element behind a disabled
    /// feature flag, the flag's value recorded in the package.
    fn skip(&self, pkg: &mut Package, e: &Element) -> bool {
        if !self.platform.flag("android.content.res.manifest_flagging") {
            return false;
        }
        let Some(flag) = attr_value(e, ANDROID, "featureFlag") else {
            return false;
        };
        let flag = flag.trim();
        let (negated, flag) = match flag.strip_prefix('!') {
            Some(f) => (true, f.trim()),
            None => (false, flag),
        };
        let value = self.platform.flag_value(flag);
        if self
            .platform
            .flag("android.content.pm.include_feature_flags_in_package_cacher")
        {
            pkg.feature_flag_state.put(flag, value);
        }
        value.unwrap_or(false) == negated
    }

    fn parse_base_apk(
        &self,
        manifest: &Element,
        apk_path: &str,
        code_path: &str,
    ) -> Result<Package> {
        if manifest.name != "manifest" {
            return fail("No <manifest> tag");
        }
        let package_name = attr_value(manifest, "", "package").unwrap_or_default();
        let split = attr_value(manifest, "", "split");
        if package_name.is_empty() {
            return fail("<manifest> has no package");
        }
        if package_name != "android"
            && let Some(e) = validate_name(&package_name, true)
        {
            return fail(format!("Invalid manifest package: {e}"));
        }
        if split.is_some_and(|s| !s.is_empty()) {
            return fail("Expected base APK, but found split");
        }
        let sa = self.obtain(manifest);
        let mut pkg = Package::new(&package_name, apk_path, code_path);
        pkg.version_code = sa.integer("versionCode", 0);
        pkg.version_code_major = sa.integer("versionCodeMajor", 0);
        pkg.base_revision_code = sa.integer("revisionCode", 0);
        pkg.version_name = sa.non_config_string("versionName", 0);
        pkg.compile_sdk_version = sa.integer("compileSdkVersion", 0);
        pkg.compile_sdk_version_code_name = sa.non_config_string("compileSdkVersionCodename", 0);
        pkg.set(
            b::ISOLATED_SPLIT_LOADING,
            sa.boolean("isolatedSplits", false),
        );
        pkg.set(b::CORE_APP, attr_bool(manifest, "", "coreApp", false));
        self.parse_base_apk_tags(&mut pkg, manifest)?;
        // `parseBaseApk`: when the APK's resources define overlayables,
        // those of every package its resources hold, the framework's too.
        let res = self.ctx.res;
        if res.tables.last().is_some_and(|t| t.defines_overlayable()) {
            for t in &res.tables {
                for (name, actor) in t.overlayables() {
                    pkg.overlayables.put(name, actor.to_owned());
                }
            }
        }
        Ok(pkg)
    }

    fn parse_base_apk_tags(&self, pkg: &mut Package, manifest: &Element) -> Result<()> {
        let sa = self.obtain(manifest);
        // `parseSharedUser`.
        if let Some(id) = sa
            .non_config_string("sharedUserId", 0)
            .filter(|s| !s.is_empty())
        {
            if pkg.package_name != "android"
                && let Some(e) = validate_name(&id, true)
            {
                return fail(format!(
                    "<manifest> specifies bad sharedUserId name \"{id}\": {e}"
                ));
            }
            let max = sa.integer("sharedUserMaxSdkVersion", 0);
            pkg.set(b::LEAVING_SHARED_UID, max != 0 && max < self.platform.sdk);
            pkg.shared_user_id = Some(id);
            pkg.shared_user_label = sa.resource_id("sharedUserLabel", 0);
        }
        pkg.install_location = sa.integer("installLocation", -1);
        pkg.target_sandbox_version = sa.integer("targetSandboxVersion", 1);
        pkg.set(
            b::EXTERNAL_STORAGE,
            self.flags & PARSE_EXTERNAL_STORAGE != 0,
        );
        let updatable = attr_bool(manifest, "", "updatableSystem", true);
        if updatable {
            pkg.booleans2 |= package::booleans2::UPDATABLE_SYSTEM;
        } else {
            pkg.booleans2 &= !package::booleans2::UPDATABLE_SYSTEM;
        }
        pkg.emergency_installer = attr_value(manifest, "", "emergencyInstaller");

        let mut found_app = false;
        for e in &manifest.children {
            if self.skip(pkg, e) {
                continue;
            }
            if e.name == "application" {
                if !found_app {
                    found_app = true;
                    self.parse_base_application(pkg, e)?;
                }
            } else {
                self.parse_base_apk_tag(pkg, e)?;
            }
        }
        if !found_app && pkg.instrumentations.is_empty() {
            self.input.borrow_mut().defer(
                "<manifest> does not contain an <application> or <instrumentation>".into(),
                MISSING_APP_TAG,
            )?;
        }
        self.validate_base_apk_tags(pkg)
    }

    fn validate_base_apk_tags(&self, pkg: &mut Package) -> Result<()> {
        // `ParsedAttributionUtils.isCombinationValid`.
        let mut tags = ArraySet::default();
        let mut inherited = ArraySet::default();
        if pkg.attributions.len() > 10_000 {
            return fail("Combination <attribution> tags are not valid");
        }
        for a in &pkg.attributions {
            if tags.contains(&a.tag) {
                return fail("Combination <attribution> tags are not valid");
            }
            tags.add(&a.tag);
        }
        for a in &pkg.attributions {
            for i in &a.inherit_from {
                if tags.contains(i) || inherited.contains(i) {
                    return fail("Combination <attribution> tags are not valid");
                }
                inherited.add(i);
            }
        }
        // `ParsedPermissionUtils.declareDuplicatePermission`.
        let mut seen: HashMap<&str, &component::Permission> = HashMap::new();
        for p in &pkg.permissions {
            if let Some(q) = seen.get(p.component.name.as_str())
                && !p.tree
                && !q.tree
                && (p.protection_level != q.protection_level || p.group != q.group)
            {
                return fail("Found duplicate permission with a different attribute value.");
            }
            seen.insert(&p.component.name, p);
        }
        // `convertCompatPermissions`.
        for (name, sdk) in [
            ("android.permission.POST_NOTIFICATIONS", 33),
            ("android.permission.WRITE_EXTERNAL_STORAGE", DONUT),
            ("android.permission.READ_PHONE_STATE", DONUT),
        ] {
            if pkg.target_sdk_version >= sdk {
                break;
            }
            if !pkg.requested_permissions.contains(name) {
                pkg.add_implicit_permission(name);
            }
        }
        // `convertSplitPermissions`.
        for spi in &self.platform.split_permissions {
            if pkg.target_sdk_version >= spi.target_sdk
                || !pkg.requested_permissions.contains(&spi.name)
            {
                continue;
            }
            for perm in &spi.new_permissions {
                if !pkg.requested_permissions.contains(perm) {
                    pkg.add_implicit_permission(perm);
                }
            }
        }
        if pkg.target_sdk_version < DONUT
            || (pkg.supports_small_screens == Some(false)
                && pkg.supports_normal_screens == Some(false)
                && pkg.supports_large_screens == Some(false)
                && pkg.supports_extra_large_screens == Some(false)
                && pkg.resizeable == Some(false)
                && pkg.any_density == Some(false))
        {
            // `adjustPackageToBeUnresizeableAndUnpipable`.
            for a in &mut pkg.activities {
                a.resize_mode = 0;
                a.main.component.flags &= !components::FLAG_SUPPORTS_PICTURE_IN_PICTURE;
            }
        }
        if self.flags & PARSE_APEX != 0 && !pkg.permissions.is_empty() {
            return fail(format!(
                "{} is an APEX package and shouldn't declare permissions.",
                pkg.package_name
            ));
        }
        Ok(())
    }

    fn parse_base_apk_tag(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        match e.name.as_str() {
            "overlay" => self.parse_overlay(pkg, e),
            "key-sets" => Err(Error::Unsupported("<key-sets>".into())),
            "feature" | "attribution" => {
                let a = self.parse_attribution(e)?;
                pkg.attributions.push(a);
                Ok(())
            }
            "permission-group" => {
                let g = self.parse_permission_group(pkg, e)?;
                pkg.permission_groups.push(g);
                Ok(())
            }
            "permission" => {
                if let Some(p) = self.parse_permission(pkg, e)? {
                    pkg.permissions.push(p);
                }
                Ok(())
            }
            "permission-tree" => {
                let p = self.parse_permission_tree(pkg, e)?;
                pkg.permissions.push(p);
                Ok(())
            }
            "uses-permission" | "uses-permission-sdk-m" | "uses-permission-sdk-23" => {
                self.parse_uses_permission(pkg, e)
            }
            "uses-configuration" => {
                let sa = self.obtain(e);
                let mut c = ConfigurationInfo {
                    req_touch_screen: sa.int("reqTouchScreen", 0),
                    req_keyboard_type: sa.int("reqKeyboardType", 0),
                    ..ConfigurationInfo::default()
                };
                if sa.boolean("reqHardKeyboard", false) {
                    c.req_input_features |= 1;
                }
                c.req_navigation = sa.int("reqNavigation", 0);
                if sa.boolean("reqFiveWayNav", false) {
                    c.req_input_features |= 2;
                }
                pkg.config_preferences.push(c);
                Ok(())
            }
            "uses-feature" => {
                let fi = self.parse_feature_info(e);
                if fi.name.is_none() {
                    pkg.config_preferences.push(ConfigurationInfo {
                        req_gl_es_version: fi.req_gl_es_version,
                        ..ConfigurationInfo::default()
                    });
                }
                pkg.req_features.push(fi);
                Ok(())
            }
            "feature-group" => {
                let mut features: Option<Vec<FeatureInfo>> = None;
                for c in &e.children {
                    if self.skip(pkg, c) {
                        continue;
                    }
                    if c.name == "uses-feature" {
                        let mut fi = self.parse_feature_info(c);
                        fi.flags |= 1;
                        features.get_or_insert_with(Vec::new).push(fi);
                    }
                }
                pkg.feature_groups.push(features);
                Ok(())
            }
            "uses-sdk" => self.parse_uses_sdk(pkg, e),
            "supports-screens" => {
                let sa = self.obtain(e);
                pkg.requires_smallest_width_dp = sa.int("requiresSmallestWidthDp", 0);
                pkg.compatible_width_limit_dp = sa.int("compatibleWidthLimitDp", 0);
                pkg.largest_width_limit_dp = sa.int("largestWidthLimitDp", 0);
                // `setXxx(int)`: 1 leaves the default, else negative is true.
                let tri = |v: i32, f: &mut Option<bool>| {
                    if v != 1 {
                        *f = Some(v < 0);
                    }
                };
                tri(sa.int("smallScreens", 1), &mut pkg.supports_small_screens);
                tri(sa.int("normalScreens", 1), &mut pkg.supports_normal_screens);
                tri(sa.int("largeScreens", 1), &mut pkg.supports_large_screens);
                tri(
                    sa.int("xlargeScreens", 1),
                    &mut pkg.supports_extra_large_screens,
                );
                tri(sa.int("resizeable", 1), &mut pkg.resizeable);
                tri(sa.int("anyDensity", 1), &mut pkg.any_density);
                Ok(())
            }
            "protected-broadcast" => {
                if let Some(name) = self.obtain(e).non_resource_string("name")
                    && !pkg.protected_broadcasts.contains(&name)
                {
                    pkg.protected_broadcasts.push(name);
                }
                Ok(())
            }
            "instrumentation" => {
                let i = self.parse_instrumentation(pkg, e)?;
                pkg.instrumentations.push(i);
                Ok(())
            }
            "original-package" => {
                let orig = self.obtain(e).non_config_string("name", 0);
                match orig {
                    Some(o) if o == pkg.package_name => {}
                    // `addOriginalPackage(null)` adds a null.
                    o => pkg.original_packages.push(o.ok_or_else(|| {
                        Error::Unsupported("<original-package> without a name".into())
                    })?),
                }
                Ok(())
            }
            "adopt-permissions" => {
                if let Some(name) = self.obtain(e).non_config_string("name", 0) {
                    pkg.adopt_permissions.push(name);
                }
                Ok(())
            }
            "uses-gl-texture" | "compatible-screens" | "supports-input" | "eat-comment" => Ok(()),
            "restrict-update" => {
                if self.flags & PARSE_IS_SYSTEM_DIR != 0 {
                    pkg.restrict_update_hash = self
                        .obtain(e)
                        .non_config_string("hash", 0)
                        .map(|h| hex_bytes(&h));
                }
                Ok(())
            }
            "install-constraints" => Err(Error::Unsupported("<install-constraints>".into())),
            "queries" => self.parse_queries(pkg, e),
            _ => Ok(()),
        }
    }

    fn parse_feature_info(&self, e: &Element) -> FeatureInfo {
        let sa = self.obtain(e);
        let name = sa.non_resource_string("name");
        FeatureInfo {
            version: sa.int("version", 0),
            req_gl_es_version: if name.is_none() {
                sa.int("glEsVersion", 0)
            } else {
                0
            },
            flags: if sa.boolean("required", true) { 1 } else { 0 },
            name,
        }
    }

    fn parse_overlay(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let sa = self.obtain(e);
        let target = sa.string("targetPackage");
        let priority = sa.int("priority", 0);
        let Some(target) = target else {
            return fail("<overlay> does not specify a target package");
        };
        if !(0..=9999).contains(&priority) {
            return fail("<overlay> priority must be between 0 and 9999");
        }
        let prop_name = sa.string("requiredSystemPropertyName");
        let prop_value = sa.string("requiredSystemPropertyValue");
        if prop_name.is_some_and(|s| !s.is_empty()) || prop_value.is_some_and(|s| !s.is_empty()) {
            return Err(Error::Unsupported(
                "<overlay> with a required system property".into(),
            ));
        }
        pkg.set(b::OVERLAY, true);
        pkg.overlay_target = Some(target);
        pkg.overlay_priority = priority;
        pkg.overlay_target_overlayable_name = sa.string("targetName");
        pkg.overlay_category = sa.string("category");
        pkg.set(b::OVERLAY_IS_STATIC, sa.boolean("isStatic", false));
        Ok(())
    }

    fn parse_uses_permission(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let sa = self.obtain(e);
        let name = sa.non_resource_string("name");
        let min_or_max = |attr: &str, default: i32| match sa.peek(attr) {
            Some(v) if (TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&v.kind) => v.data as i32,
            _ => default,
        };
        let min_sdk = min_or_max("minSdkVersion", i32::MIN);
        let max_sdk = min_or_max("maxSdkVersion", i32::MAX);
        let mut required = Vec::new();
        let mut required_not = Vec::new();
        if let Some(f) = sa.non_config_string("requiredFeature", 0) {
            required.push(f);
        }
        if let Some(f) = sa.non_config_string("requiredNotFeature", 0) {
            required_not.push(f);
        }
        let flags = sa.int("usesPermissionFlags", 0);
        for c in &e.children {
            match c.name.as_str() {
                "required-feature" | "required-not-feature" => {
                    let f = self.obtain(c).string("name").filter(|s| !s.is_empty());
                    let Some(f) = f else {
                        return fail(format!("Feature name is missing from <{}> tag.", c.name));
                    };
                    if c.name == "required-feature" {
                        required.push(f);
                    } else {
                        required_not.push(f);
                    }
                }
                _ => {}
            }
        }
        let Some(name) = name else { return Ok(()) };
        if self.platform.sdk < min_sdk || self.platform.sdk > max_sdk {
            return Ok(());
        }
        if required.iter().any(|f| !self.platform.features.contains(f))
            || required_not
                .iter()
                .any(|f| self.platform.features.contains(f))
        {
            return Ok(());
        }
        if let Some(existing) = pkg.uses_permissions.iter().find(|p| p.name == name) {
            if existing.flags != flags {
                return fail(format!(
                    "Conflicting uses-permissions flags: {name} in package: {}",
                    pkg.package_name
                ));
            }
            return Ok(());
        }
        pkg.add_uses_permission(component::UsesPermission { name, flags });
        Ok(())
    }

    fn parse_uses_sdk(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let sa = self.obtain(e);
        let apk_in_apex = self.flags & PARSE_APK_IN_APEX != 0;
        let mut min_vers = package::DEFAULT_MIN_SDK_VERSION;
        let mut min_code: Option<String> = None;
        let mut min_assigned = false;
        let mut target_vers = package::DEFAULT_TARGET_SDK_VERSION;
        let mut target_code: Option<String> = None;
        let mut max_vers = i32::MAX;
        if let Some(v) = sa.peek("minSdkVersion") {
            if v.kind == TYPE_STRING && v.string.is_some() {
                min_code = v.string;
                min_assigned = min_code.as_deref().is_some_and(|s| !s.is_empty());
            } else {
                min_vers = v.data as i32;
                min_assigned = true;
            }
        }
        match sa.peek("targetSdkVersion") {
            Some(v) if v.kind == TYPE_STRING && v.string.is_some() => {
                target_code = v.string;
                if !min_assigned {
                    min_code = target_code.clone();
                }
            }
            Some(v) => target_vers = v.data as i32,
            None => {
                target_vers = min_vers;
                target_code = min_code.clone();
            }
        }
        if apk_in_apex && let Some(v) = sa.peek("maxSdkVersion") {
            max_vers = v.data as i32;
        }
        let matches_code = |code: &str| {
            let name = code.split('.').next().unwrap_or(code);
            self.platform.codenames.iter().any(|c| c == name)
        };
        // `computeTargetSdkVersion`.
        let target_sdk = match &target_code {
            None => target_vers,
            Some(code) if apk_in_apex => {
                return Err(Error::Unsupported(format!(
                    "a codename target ({code}) in an APEX's APK"
                )));
            }
            Some(code) if matches_code(code) => CUR_DEVELOPMENT,
            Some(code) => return fail(format!("Requires development platform {code}")),
        };
        self.input.borrow_mut().enable(target_sdk)?;
        // `computeMinSdkVersion`.
        let min_sdk = match &min_code {
            None if min_vers <= self.platform.sdk => min_vers,
            None => return fail(format!("Requires newer sdk version #{min_vers}")),
            Some(code) if matches_code(code) => CUR_DEVELOPMENT,
            Some(code) => return fail(format!("Requires development platform {code}")),
        };
        pkg.min_sdk_version = min_sdk;
        pkg.target_sdk_version = target_sdk;
        if apk_in_apex {
            // `computeMaxSdkVersion`.
            if self.platform.sdk > max_vers {
                return fail(format!("Requires max SDK version {max_vers}"));
            }
            pkg.max_sdk_version = max_vers;
        }
        let mut min_extensions: Option<Vec<(i32, i32)>> = None;
        for c in &e.children {
            if c.name == "extension-sdk" {
                return Err(Error::Unsupported("<extension-sdk>".into()));
            }
        }
        if let Some(v) = &mut min_extensions {
            v.sort();
        }
        pkg.min_extension_versions = min_extensions;
        Ok(())
    }

    fn parse_queries(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "intent" => {
                    let info = self.parse_intent_info(pkg, c, true, true)?;
                    let f = &info.filter;
                    let actions = &f.actions;
                    let schemes = f.schemes.as_deref().unwrap_or(&[]);
                    let types = f.types.as_deref().unwrap_or(&[]);
                    // `getHosts`: the hosts as written.
                    let hosts: Vec<&str> = f
                        .authorities
                        .iter()
                        .flatten()
                        .map(|a| a.orig_host.as_str())
                        .collect();
                    if schemes.is_empty() && types.is_empty() && actions.is_empty() {
                        return fail("intent tags must contain either an action or data.");
                    }
                    if actions.len() > 1 || types.len() > 1 || schemes.len() > 1 || hosts.len() > 1
                    {
                        return fail(
                            "intent tag may have at most one action, type, scheme and host.",
                        );
                    }
                    let mut intent = QueriesIntent::default();
                    if let Some(cats) = &f.categories {
                        let mut set = ArraySet::default();
                        cats.iter().for_each(|c| set.add(c));
                        intent.categories = Some(set);
                    }
                    let host = hosts.first().copied();
                    if let Some(scheme) = schemes.first() {
                        intent.data = Some(hierarchical_uri(scheme, host, "*"));
                    }
                    if let Some(t) = types.first() {
                        intent.data_type = Some(if t.contains('/') {
                            t.clone()
                        } else {
                            format!("{t}/*")
                        });
                        if intent.data.is_none() {
                            intent.data = Some(hierarchical_uri("content", Some("*"), "*"));
                        }
                    }
                    intent.action = actions.first().cloned();
                    pkg.queries_intents.push(intent);
                }
                "package" => {
                    let name = self.obtain(c).non_config_string("name", 0);
                    match name.filter(|s| !s.is_empty()) {
                        Some(n) => pkg.queries_packages.push(n),
                        None => return fail("Package name is missing from package tag."),
                    }
                }
                "provider" => {
                    let auth = self.obtain(c).non_config_string("authorities", 0);
                    match auth.filter(|s| !s.is_empty()) {
                        Some(a) => a
                            .split(';')
                            .filter(|t| !t.is_empty())
                            .for_each(|t| pkg.queries_providers.add(t)),
                        None => return fail("Authority missing from provider tag."),
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_base_application(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let pkg_name = pkg.package_name.clone();
        let target_sdk = pkg.target_sdk_version;
        let sa = self.obtain(e);
        if let Some(name) = sa.non_config_string("name", 0) {
            let Some(cls) = build_class_name(&pkg_name, Some(&name)) else {
                return fail(format!("Empty class name in package {pkg_name}"));
            };
            if cls == APP_DETAILS_ACTIVITY_CLASS_NAME {
                return fail("<application> invalid android:name");
            }
            pkg.class_name = Some(cls);
        }
        if let Some(label) = sa.peek("label") {
            pkg.label_res = label.resource_id as i32;
            if label.resource_id == 0 {
                pkg.non_localized_label = label.coerce_to_string();
            }
        }
        self.parse_base_app_basic_flags(pkg, &sa);
        if let Some(m) = sa.non_config_string("manageSpaceActivity", NATIVE_CONFIG_VERSION) {
            pkg.manage_space_activity_name =
                Some(build_class_name(&pkg_name, Some(&m)).ok_or_else(|| {
                    Error::Parse(format!("Empty class name in package {pkg_name}"))
                })?);
        }
        if self
            .platform
            .flag("android.content.pm.change_launcher_badging")
        {
            return Err(Error::Unsupported("alternate launcher icons".into()));
        }
        if pkg.get(b::ALLOW_BACKUP) {
            if let Some(agent) = sa.non_config_string("backupAgent", NATIVE_CONFIG_VERSION) {
                pkg.backup_agent_name =
                    Some(build_class_name(&pkg_name, Some(&agent)).ok_or_else(|| {
                        Error::Parse(format!("Empty class name in package {pkg_name}"))
                    })?);
                pkg.set(b::KILL_AFTER_RESTORE, sa.boolean("killAfterRestore", true));
                pkg.set(
                    b::RESTORE_ANY_VERSION,
                    sa.boolean("restoreAnyVersion", false),
                );
                pkg.set(b::FULL_BACKUP_ONLY, sa.boolean("fullBackupOnly", false));
                pkg.set(
                    b::BACKUP_IN_FOREGROUND,
                    sa.boolean("backupInForeground", false),
                );
            }
            if let Some(v) = sa.peek("fullBackupContent") {
                pkg.full_backup_content = if v.resource_id != 0 {
                    v.resource_id as i32
                } else if v.data == 0 {
                    -1
                } else {
                    0
                };
            }
        }
        if sa.boolean("persistent", false) {
            let feature = sa.non_resource_string("persistentWhenFeatureAvailable");
            pkg.set(
                b::PERSISTENT,
                feature.is_none_or(|f| self.platform.features.contains(&f)),
            );
        }
        if sa.has_value_or_empty("resizeableActivity") {
            pkg.resizeable_activity = Some(sa.boolean("resizeableActivity", true));
        } else {
            pkg.set(b::RESIZEABLE_ACTIVITY_VIA_SDK_VERSION, target_sdk >= N);
        }
        let affinity = if target_sdk >= FROYO {
            sa.non_config_string("taskAffinity", NATIVE_CONFIG_VERSION)
        } else {
            sa.non_resource_string("taskAffinity")
        };
        pkg.task_affinity =
            build_task_affinity_name(&pkg_name, Some(&pkg_name), affinity.as_deref())?;
        if let Some(factory) = sa.non_resource_string("appComponentFactory") {
            pkg.app_component_factory =
                Some(build_class_name(&pkg_name, Some(&factory)).ok_or_else(|| {
                    Error::Parse(format!("Empty class name in package {pkg_name}"))
                })?);
        }
        let pname = if target_sdk >= FROYO {
            sa.non_config_string("process", NATIVE_CONFIG_VERSION)
        } else {
            sa.non_resource_string("process")
        };
        let process = build_process_name(&pkg_name, None, pname.as_deref(), self.flags)?;
        if pkg.get(b::CANT_SAVE_STATE) && process.as_ref().is_some_and(|p| *p != pkg_name) {
            return fail("cantSaveState applications can not use custom processes");
        }
        pkg.process_name = process;
        if let Some(cl) = &pkg.class_loader_name
            && cl != "dalvik.system.PathClassLoader"
            && cl != "dalvik.system.DelegateLastClassLoader"
        {
            return fail(format!("Invalid class loader name: {cl}"));
        }
        pkg.gwp_asan_mode = sa.int("gwpAsanMode", -1);
        pkg.memtag_mode = sa.int("memtagMode", -1);
        if self
            .platform
            .flag("android.content.pm.app_compat_option_16kb")
        {
            pkg.page_size_app_compat_flags = sa.int("pageSizeCompat", 0);
        }
        if sa.has_value("nativeHeapZeroInitialized") {
            pkg.native_heap_zero_initialized = if sa.boolean("nativeHeapZeroInitialized", false) {
                1
            } else {
                0
            };
        }
        if sa.has_value("requestRawExternalStorageAccess") {
            pkg.request_raw_external_storage_access =
                Some(sa.boolean("requestRawExternalStorageAccess", false));
        }
        if sa.has_value("requestForegroundServiceExemption") {
            pkg.set(
                b::REQUEST_FOREGROUND_SERVICE_EXEMPTION,
                sa.boolean("requestForegroundServiceExemption", false),
            );
        }
        if let Some(certs) = self.known_activity_embedding_certs(&sa)? {
            for c in certs {
                pkg.known_activity_embedding_certs.add(&c.to_uppercase());
            }
        }
        pkg.intent_matching_flags = sa.int("intentMatchingFlags", 0);

        let mut has_activity_order = false;
        let mut has_receiver_order = false;
        let mut has_service_order = false;
        for c in &e.children {
            if self.skip(pkg, c) {
                continue;
            }
            match c.name.as_str() {
                "activity" | "receiver" => {
                    let receiver = c.name == "receiver";
                    let a = self.parse_activity_or_receiver(pkg, c, None)?;
                    if receiver {
                        has_receiver_order |= a.main.order != 0;
                        pkg.add_mime_groups(&a.main.component.intents);
                        pkg.receivers.push(a);
                    } else {
                        has_activity_order |= a.main.order != 0;
                        pkg.add_mime_groups(&a.main.component.intents);
                        pkg.activities.push(a);
                    }
                }
                "service" => {
                    let s = self.parse_service(pkg, c, None)?;
                    has_service_order |= s.main.order != 0;
                    pkg.add_mime_groups(&s.main.component.intents);
                    pkg.services.push(s);
                }
                "provider" => {
                    let p = self.parse_provider(pkg, c, None)?;
                    pkg.add_mime_groups(&p.main.component.intents);
                    pkg.providers.push(p);
                }
                "activity-alias" => {
                    let a = self.parse_activity_alias(pkg, c, None)?;
                    has_activity_order |= a.main.order != 0;
                    pkg.add_mime_groups(&a.main.component.intents);
                    pkg.activities.push(a);
                }
                "apex-system-service" => {
                    let s = self.parse_apex_system_service(c)?;
                    pkg.apex_system_services.push(s);
                }
                _ => self.parse_base_app_child_tag(pkg, c)?,
            }
        }
        if pkg
            .static_shared_library_name
            .as_deref()
            .is_none_or(str::is_empty)
            && pkg.sdk_library_name.as_deref().is_none_or(str::is_empty)
        {
            let affinity =
                build_task_affinity_name(&pkg_name, Some(&pkg_name), Some(":app_details"))?;
            let a = self.app_details_activity(pkg, affinity);
            pkg.activities.push(a);
        }
        // `sortActivities` and the rest: stable, by descending order.
        if has_activity_order {
            pkg.activities
                .sort_by_key(|a| std::cmp::Reverse(a.main.order));
        }
        if has_receiver_order {
            pkg.receivers
                .sort_by_key(|a| std::cmp::Reverse(a.main.order));
        }
        if has_service_order {
            pkg.services
                .sort_by_key(|s| std::cmp::Reverse(s.main.order));
        }
        self.after_parse_base_application(pkg);
        Ok(())
    }

    /// `ParsingUtils.parseKnownActivityEmbeddingCerts`.
    fn known_activity_embedding_certs(&self, sa: &TypedArray) -> Result<Option<Vec<String>>> {
        if !sa.has_value("knownActivityEmbeddingCerts") {
            return Ok(None);
        }
        let id = sa.resource_id("knownActivityEmbeddingCerts", 0);
        let certs: Vec<String> = if id != 0 {
            let res = self.ctx.res;
            if res.type_name(id as u32) == Some("array") {
                res.string_array(id as u32)
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect()
            } else {
                res.resource_string(id as u32).into_iter().collect()
            }
        } else {
            sa.string("knownActivityEmbeddingCerts")
                .filter(|s| !s.is_empty())
                .into_iter()
                .collect()
        };
        if certs.is_empty() {
            return fail("Defined a knownActivityEmbeddingCerts attribute but it is empty");
        }
        Ok(Some(certs))
    }

    fn parse_base_app_basic_flags(&self, pkg: &mut Package, sa: &TypedArray) {
        let t = pkg.target_sdk_version;
        for (flag, attr, default) in [
            (b::ALLOW_BACKUP, "allowBackup", true),
            (b::ALLOW_CLEAR_USER_DATA, "allowClearUserData", true),
            (
                b::ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE,
                "allowClearUserDataOnFailedRestore",
                true,
            ),
            (
                b::ALLOW_NATIVE_HEAP_POINTER_TAGGING,
                "allowNativeHeapPointerTagging",
                true,
            ),
            (b::ENABLED, "enabled", true),
            (b::EXTRACT_NATIVE_LIBS, "extractNativeLibs", true),
            (b::HAS_CODE, "hasCode", true),
            (b::ALLOW_TASK_REPARENTING, "allowTaskReparenting", false),
            (b::CANT_SAVE_STATE, "cantSaveState", false),
            (b::CROSS_PROFILE, "crossProfile", false),
            (b::DEBUGGABLE, "debuggable", false),
            (
                b::DEFAULT_TO_DEVICE_PROTECTED_STORAGE,
                "defaultToDeviceProtectedStorage",
                false,
            ),
            (b::DIRECT_BOOT_AWARE, "directBootAware", false),
            (b::FORCE_QUERYABLE, "forceQueryable", false),
            (b::GAME, "isGame", false),
            (b::HAS_FRAGILE_USER_DATA, "hasFragileUserData", false),
            (b::LARGE_HEAP, "largeHeap", false),
            (b::MULTI_ARCH, "multiArch", false),
            (
                b::PRESERVE_LEGACY_EXTERNAL_STORAGE,
                "preserveLegacyExternalStorage",
                false,
            ),
            (b::REQUIRED_FOR_ALL_USERS, "requiredForAllUsers", false),
            (b::SUPPORTS_RTL, "supportsRtl", false),
            (b::TEST_ONLY, "testOnly", false),
            (b::USE_EMBEDDED_DEX, "useEmbeddedDex", false),
            (b::USES_NON_SDK_API, "usesNonSdkApi", false),
            (b::VM_SAFE_MODE, "vmSafeMode", false),
        ] {
            pkg.set(flag, sa.boolean(attr, default));
        }
        pkg.auto_revoke_permissions = sa.int("autoRevokePermissions", 0);
        pkg.set(
            b::ATTRIBUTIONS_ARE_USER_VISIBLE,
            sa.boolean("attributionsAreUserVisible", false),
        );
        pkg.set(
            b::RESET_ENABLED_SETTINGS_ON_APP_DATA_CLEARED,
            sa.boolean("resetEnabledSettingsOnAppDataCleared", false),
        );
        pkg.set(
            b::ALLOW_AUDIO_PLAYBACK_CAPTURE,
            sa.boolean("allowAudioPlaybackCapture", t >= Q),
        );
        pkg.set(
            b::HARDWARE_ACCELERATED,
            sa.boolean("hardwareAccelerated", t >= ICE_CREAM_SANDWICH),
        );
        pkg.set(
            b::REQUEST_LEGACY_EXTERNAL_STORAGE,
            sa.boolean("requestLegacyExternalStorage", t < Q),
        );
        pkg.set(
            b::USES_CLEARTEXT_TRAFFIC,
            sa.boolean("usesCleartextTraffic", t < P),
        );
        let back_default = self
            .platform
            .flag("com.android.window.flags.predictive_back_default_enable_sdk_36")
            && t > VANILLA_ICE_CREAM;
        pkg.set(
            b::ENABLE_ON_BACK_INVOKED_CALLBACK,
            sa.boolean("enableOnBackInvokedCallback", back_default),
        );
        pkg.ui_options = sa.int("uiOptions", 0);
        pkg.category = sa.int("appCategory", package::CATEGORY_UNDEFINED);
        pkg.max_aspect_ratio = sa.float("maxAspectRatio", 0.0);
        pkg.min_aspect_ratio = sa.float("minAspectRatio", 0.0);
        pkg.banner = sa.resource_id("banner", 0);
        pkg.description_res = sa.resource_id("description", 0);
        pkg.icon_res = sa.resource_id("icon", 0);
        pkg.logo = sa.resource_id("logo", 0);
        pkg.network_security_config_res = sa.resource_id("networkSecurityConfig", 0);
        pkg.round_icon_res = sa.resource_id("roundIcon", 0);
        pkg.theme = sa.resource_id("theme", 0);
        pkg.data_extraction_rules = sa.resource_id("dataExtractionRules", 0);
        pkg.locale_config_res = sa.resource_id("localeConfig", 0);
        pkg.class_loader_name = sa.string("classLoader");
        pkg.required_account_type = sa.string("requiredAccountType");
        pkg.restricted_account_type = sa.string("restrictedAccountType");
        pkg.zygote_preload_name = sa.string("zygotePreloadName");
        pkg.permission = sa.non_config_string("permission", 0);
        pkg.allow_cross_uid_activity_switch_from_below =
            sa.boolean("allowCrossUidActivitySwitchFromBelow", true);
    }

    fn parse_base_app_child_tag(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        match e.name.as_str() {
            "meta-data" => {
                if let Some(p) = self.parse_meta_data(pkg, None, e, "<meta-data>")? {
                    p.put_in(pkg.meta_data.get_or_insert_with(Bundle::default));
                }
                Ok(())
            }
            "property" => {
                if let Some(p) = self.parse_meta_data(pkg, None, e, "<property>")? {
                    pkg.properties.put(&p.name.clone(), p);
                }
                Ok(())
            }
            "sdk-library" => {
                let sa = self.obtain(e);
                let name = sa.non_resource_string("name");
                let major = sa.int("versionMajor", -1);
                match name {
                    Some(n) if major >= 0 => {
                        if pkg.shared_user_id.is_some() {
                            return fail("sharedUserId not allowed in SDK library");
                        }
                        if pkg.sdk_library_name.is_some() {
                            return fail(format!("Multiple SDKs for package {}", pkg.package_name));
                        }
                        pkg.sdk_library_name = Some(n);
                        pkg.sdk_lib_version_major = major;
                        pkg.set(b::SDK_LIBRARY, true);
                        Ok(())
                    }
                    n => fail(format!(
                        "Bad sdk-library declaration name: {n:?} version: {major}"
                    )),
                }
            }
            "static-library" => {
                let sa = self.obtain(e);
                let name = sa.non_resource_string("name");
                let version = sa.int("version", -1);
                let major = sa.int("versionMajor", 0);
                match name {
                    Some(n) if version >= 0 => {
                        if pkg.shared_user_id.is_some() {
                            return fail("sharedUserId not allowed in static shared library");
                        }
                        if pkg.static_shared_library_name.is_some() {
                            return fail(format!(
                                "Multiple static-shared libs for package {}",
                                pkg.package_name
                            ));
                        }
                        pkg.static_shared_library_name = Some(n);
                        pkg.static_shared_lib_version =
                            (major as i64) << 32 | (version as u32 as i64);
                        pkg.set(b::STATIC_SHARED_LIBRARY, true);
                        Ok(())
                    }
                    n => fail(format!(
                        "Bad static-library declaration name: {n:?} version: {version}"
                    )),
                }
            }
            "library" => {
                if let Some(n) = self.obtain(e).non_resource_string("name")
                    && !pkg.library_names.contains(&n)
                {
                    pkg.library_names.push(n);
                }
                Ok(())
            }
            "uses-sdk-library" => Err(Error::Unsupported("<uses-sdk-library>".into())),
            "uses-static-library" => self.parse_uses_static_library(pkg, e),
            "uses-library" | "uses-native-library" => {
                let sa = self.obtain(e);
                let Some(name) = sa.non_resource_string("name") else {
                    return Ok(());
                };
                let req = sa.boolean("required", true);
                let (required, optional) = if e.name == "uses-library" {
                    (&mut pkg.uses_libraries, &mut pkg.uses_optional_libraries)
                } else {
                    (
                        &mut pkg.uses_native_libraries,
                        &mut pkg.uses_optional_native_libraries,
                    )
                };
                if req {
                    if !required.contains(&name) {
                        required.push(name.clone());
                    }
                    optional.retain(|o| *o != name);
                } else if !required.contains(&name) && !optional.contains(&name) {
                    optional.push(name);
                }
                Ok(())
            }
            "processes" => self.parse_processes(pkg, e),
            "uses-package" => Ok(()),
            "profileable" => {
                let sa = self.obtain(e);
                let by_shell = pkg.get(b::PROFILEABLE_BY_SHELL) && !pkg.get(b::DISALLOW_PROFILING)
                    || sa.boolean("shell", false);
                pkg.set(b::PROFILEABLE_BY_SHELL, by_shell);
                let profileable = !pkg.get(b::DISALLOW_PROFILING) && sa.boolean("enabled", true);
                pkg.set(b::DISALLOW_PROFILING, !profileable);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn parse_uses_static_library(&self, pkg: &mut Package, e: &Element) -> Result<()> {
        let sa = self.obtain(e);
        let name = sa.non_resource_string("name");
        let version = sa.int("version", -1);
        let digest = sa.non_resource_string("certDigest");
        let (Some(name), Some(digest)) = (name, digest) else {
            return fail("Bad uses-static-library declaration");
        };
        if version < 0 {
            return fail("Bad uses-static-library declaration");
        }
        if pkg.uses_static_libraries.contains(&name) {
            return fail(format!(
                "Depending on multiple versions of static library {name}"
            ));
        }
        let mut digests = vec![digest.replace(':', "").to_lowercase()];
        if pkg.target_sdk_version >= O_MR1 {
            for c in &e.children {
                if self.skip(pkg, c) {
                    continue;
                }
                if c.name == "additional-certificate" {
                    let d = self
                        .obtain(c)
                        .non_resource_string("certDigest")
                        .filter(|d| !d.is_empty());
                    let Some(d) = d else {
                        return fail(
                            "Bad additional-certificate declaration with empty certDigest",
                        );
                    };
                    digests.push(d.replace(':', "").to_lowercase());
                }
            }
        }
        pkg.uses_static_libraries.push(name);
        pkg.uses_static_libraries_versions
            .get_or_insert_with(Vec::new)
            .push(version as i64);
        pkg.uses_static_libraries_cert_digests
            .get_or_insert_with(Vec::new)
            .push(digests);
        Ok(())
    }

    fn after_parse_base_application(&self, pkg: &mut Package) {
        // `setMaxAspectRatio`.
        let mut max = if pkg.target_sdk_version < O {
            DEFAULT_PRE_O_MAX_ASPECT_RATIO
        } else {
            0.0
        };
        if pkg.max_aspect_ratio != 0.0 {
            max = pkg.max_aspect_ratio;
        } else if let Some(BundleValue::Float(f)) = pkg
            .meta_data
            .as_ref()
            .and_then(|m| m.get("android.max_aspect"))
        {
            max = *f;
        }
        for a in &mut pkg.activities {
            if a.max_aspect_ratio != component::NOT_SET {
                continue;
            }
            let ratio = match a.main.component.meta_data().get("android.max_aspect") {
                Some(BundleValue::Float(f)) => *f,
                _ => max,
            };
            components::set_max_aspect_ratio(a, ratio);
        }
        // `setMinAspectRatio`.
        let min = pkg.min_aspect_ratio;
        for a in &mut pkg.activities {
            if a.min_aspect_ratio == component::NOT_SET {
                components::set_min_aspect_ratio(a, min);
            }
        }
        // `setSupportsSizeChanges`.
        let supports = matches!(
            pkg.meta_data
                .as_ref()
                .and_then(|m| m.get("android.supports_size_changes")),
            Some(BundleValue::Bool(true))
        );
        for a in &mut pkg.activities {
            if supports
                || matches!(
                    a.main
                        .component
                        .meta_data()
                        .get("android.supports_size_changes"),
                    Some(BundleValue::Bool(true))
                )
            {
                a.supports_size_changes = true;
            }
        }
        // `hasDomainURLs`.
        let domain_urls = pkg.activities.iter().any(|a| {
            a.main.component.intents.iter().any(|i| {
                let f = &i.filter;
                f.has_action("android.intent.action.VIEW")
                    && (f.has_data_scheme("http") || f.has_data_scheme("https"))
            })
        });
        pkg.set(b::HAS_DOMAIN_URLS, domain_urls);
    }

    /// `ParsingPackageUtils.parseMetaData`: a `<meta-data>` or
    /// `<property>`; `None` for a value of a type it does not keep.
    fn parse_meta_data(
        &self,
        pkg: &Package,
        component: Option<&str>,
        e: &Element,
        tag: &str,
    ) -> Result<Option<Property>> {
        let sa = self.obtain(e);
        let Some(name) = sa.non_config_string("name", 0) else {
            return fail(format!("{tag} requires an android:name attribute"));
        };
        let value = match sa.peek("resource") {
            Some(v) if v.resource_id != 0 => PropertyValue::Resource(v.resource_id as i32),
            _ => match sa.peek("value") {
                None => {
                    return fail(format!(
                        "{tag} requires an android:value or android:resource attribute"
                    ));
                }
                Some(v) if v.kind == TYPE_STRING => PropertyValue::String(v.coerce_to_string()),
                Some(v) if v.kind == TYPE_INT_BOOLEAN => PropertyValue::Bool(v.data != 0),
                Some(v) if (TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&v.kind) => {
                    PropertyValue::Int(v.data as i32)
                }
                Some(v) if v.kind == TYPE_FLOAT => PropertyValue::Float(f32::from_bits(v.data)),
                Some(_) => return Ok(None),
            },
        };
        Ok(Some(Property {
            name,
            value,
            package_name: pkg.package_name.clone(),
            class_name: component.map(str::to_owned),
        }))
    }
}

/// `Uri.Builder().scheme(scheme).authority(host).path(path).build()`'s
/// `toString`.
fn hierarchical_uri(scheme: &str, host: Option<&str>, path: &str) -> String {
    let mut s = format!("{scheme}:");
    if let Some(h) = host {
        s.push_str("//");
        s.push_str(&uri_encode(h, ""));
    }
    s.push_str(&uri_encode(&format!("/{path}"), "/"));
    s
}

/// `Uri.encode(s, allow)`.
fn uri_encode(s: &str, allow: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "_-!.~'()*".contains(c) || allow.contains(c) {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `parseRestrictUpdateHash`'s hex decoding (`Character.digit`).
fn hex_bytes(h: &str) -> Vec<u8> {
    let d: Vec<i32> = h
        .chars()
        .map(|c| c.to_digit(16).map_or(-1, |v| v as i32))
        .collect();
    (0..d.len() / 2)
        .map(|i| ((d[2 * i] << 4) + d[2 * i + 1]) as u8)
        .collect()
}
