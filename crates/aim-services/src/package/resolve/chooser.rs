//! `resolveIntent` among activities of equal standing (`chooseBestActivity`
//! at `android-16.0.0_r1`): a persistent preferred activity, else a
//! preferred one (`findPreferredActivityBody`), else the resolver
//! activity's `ResolveInfo` (the chooser), as PackageManager builds it
//! from the `android` package at boot (`setPlatformPackage`).

use std::sync::Arc;

use super::super::info::flags::{
    MATCH_DIRECT_BOOT_AWARE, MATCH_DIRECT_BOOT_UNAWARE, MATCH_DISABLED_COMPONENTS,
};
use super::super::info::{
    ActivityInfo, ComponentInfo, PackageItemInfo, Target, generate_application_info,
};
use super::super::intent::{ComponentName, Intent};
use super::super::intent_filter::MATCH_CATEGORY_MASK;
use super::super::model::PackageUserState;
use super::super::pkg::{MetaData, Value};
use super::super::preferred;
use super::*;

const ACTION_MAIN: &str = "android.intent.action.MAIN";
const CATEGORY_HOME: &str = "android.intent.category.HOME";
const CATEGORY_DEFAULT: &str = "android.intent.category.DEFAULT";
const CATEGORY_SETUP_WIZARD: &str = "android.intent.category.SETUP_WIZARD";
/// `PackageManager.INSTALL_REASON_DEVICE_SETUP`.
const INSTALL_REASON_DEVICE_SETUP: i32 = 2;
/// `ApplicationInfo.PRIVATE_FLAG_INSTANT`.
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;
/// The aconfig flag `android.content.pm.improve_home_app_behavior`.
const IMPROVE_HOME_APP_BEHAVIOR: &str = "android.content.pm.improve_home_app_behavior";
/// `Intent.METADATA_DOCK_HOME`.
const METADATA_DOCK_HOME: &str = "android.dock_home";
/// The resolver activity (`ResolverActivity.class.getName()`).
const RESOLVER_ACTIVITY: &str = "com.android.internal.app.ResolverActivity";

/// `ActivityInfo` constants of the resolver activity.
const LAUNCH_MULTIPLE: i32 = 0;
const DOCUMENT_LAUNCH_NEVER: i32 = 3;
const FLAG_EXCLUDE_FROM_RECENTS: i32 = 0x20;
const FLAG_HARDWARE_ACCELERATED: i32 = 0x200;
const FLAG_RELINQUISH_TASK_IDENTITY: i32 = 0x1000;
const FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES: i32 = 0x10000;
const RESIZE_MODE_RESIZEABLE: i32 = 2;
const SCREEN_ORIENTATION_UNSPECIFIED: i32 = -1;
const CONFIG_KEYBOARD: i32 = 0x10;
const CONFIG_KEYBOARD_HIDDEN: i32 = 0x20;
const CONFIG_ORIENTATION: i32 = 0x80;
const CONFIG_SCREEN_LAYOUT: i32 = 0x100;
const CONFIG_SCREEN_SIZE: i32 = 0x400;
const CONFIG_SMALLEST_SCREEN_SIZE: i32 = 0x800;

fn same(ri: &ResolveInfo, c: &ComponentName) -> bool {
    ri.component() == (c.package.as_str(), c.class.as_str())
}

impl Resolution {
    /// `getSetupWizardPackageNameImpl`: the one system activity for
    /// MAIN/SETUP_WIZARD, else none.
    fn setup_wizard(&self) -> Result<Option<&str>> {
        if self.setup_wizard.get().is_none() {
            let intent = Intent {
                action: Some(ACTION_MAIN.into()),
                categories: Some(vec![CATEGORY_SETUP_WIZARD.into()]),
                ..Intent::default()
            };
            let flags = MATCH_SYSTEM_ONLY
                | MATCH_DIRECT_BOOT_AWARE
                | MATCH_DIRECT_BOOT_UNAWARE
                | MATCH_DISABLED_COMPONENTS;
            let found = self.query_intent_activities(&intent, None, flags, 0, SYSTEM_UID)?;
            let name = (found.len() == 1).then(|| found[0].component().0.to_owned());
            let _ = self.setup_wizard.set(name);
        }
        Ok(self.setup_wizard.get().and_then(|n| n.as_deref()))
    }

    fn activity(&self, c: &ComponentName, flags: i64, calling_uid: i32, user: i32) -> Result<bool> {
        let found = self
            .query(calling_uid)
            .activity_info(c, flags, calling_uid, user);
        Ok(thrown(found)?.is_some())
    }

    /// `findPersistentPreferredActivity`.
    fn find_persistent(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        query: &[ResolveInfo],
        user: i32,
        calling_uid: i32,
    ) -> Result<Option<ResolveInfo>> {
        let Some(p) = self.preferred(user) else {
            return Ok(None);
        };
        let default_only = flags & MATCH_DEFAULT_ONLY != 0;
        for ppa in preferred::query(&p.persistent, intent, resolved_type, default_only).map_err(|_| NotModelled("URI filter matching exception"))? {
            let c = &ppa.component;
            if !self.activity(c, flags | MATCH_DISABLED_COMPONENTS, calling_uid, user)? {
                continue;
            }
            if let Some(ri) = query.iter().find(|ri| same(ri, c)) {
                return Ok(Some(ri.clone()));
            }
        }
        Ok(None)
    }

    /// `findPreferredActivityBody` for `chooseBestActivity` (always, no
    /// removal). The original's bookkeeping (dropping a dangling or
    /// outdated choice, which `queryMayBeFiltered` and the setup wizard's
    /// home allow) changes what it writes, not what it answers.
    pub(super) fn find_preferred_activity(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        query: &[ResolveInfo],
        user: i32,
        calling_uid: i32,
    ) -> Result<Option<ResolveInfo>> {
        let capture = self.implicit_image_capture(intent, user, resolved_type, flags)?;
        let flags =
            self.update_flags_for_resolve(flags, user, calling_uid, false, false, capture)?;
        let intent = intent.selector.as_deref().unwrap_or(intent);
        if let Some(ri) =
            self.find_persistent(intent, resolved_type, flags, query, user, calling_uid)?
        {
            return Ok(Some(ri));
        }
        let Some(p) = self.preferred(user) else {
            return Ok(None);
        };
        let default_only = flags & MATCH_DEFAULT_ONLY != 0;
        let prefs = preferred::query(&p.preferred, intent, resolved_type, default_only).map_err(|_| NotModelled("URI filter matching exception"))?;
        if prefs.is_empty() {
            return Ok(None);
        }
        let best = query.iter().map(|ri| ri.match_).fold(0, i32::max) & MATCH_CATEGORY_MASK;
        let has = |c: &str| intent.categories.iter().flatten().any(|x| x == c);
        let home = intent.action.as_deref() == Some(ACTION_MAIN) && has(CATEGORY_HOME);
        let exclude_setup_wizard =
            home && has(CATEGORY_DEFAULT) && !self.state.platform.device_provisioned;
        for pa in prefs {
            if pa.match_ != best || !pa.always {
                continue;
            }
            let lookup = flags
                | MATCH_DISABLED_COMPONENTS
                | MATCH_DIRECT_BOOT_AWARE
                | MATCH_DIRECT_BOOT_UNAWARE;
            if !self.activity(&pa.component, lookup, calling_uid, user)? {
                continue;
            }
            let Some(ri) = query.iter().find(|ri| same(ri, &pa.component)) else {
                continue;
            };
            if !self.same_set(&pa, query, exclude_setup_wizard, user)?
                && !self.is_superset(&pa, query, exclude_setup_wizard)?
            {
                let improve = self
                    .state
                    .system
                    .flags
                    .iter()
                    .any(|(n, v)| n == IMPROVE_HOME_APP_BEHAVIOR && *v);
                if !improve || !home {
                    return Ok(None);
                }
            }
            return Ok(Some(ri.clone()));
        }
        Ok(None)
    }

    /// `PreferredComponent.sameSet(query, excludeSetupWizardPackage, user)`:
    /// every result is in the set (but the setup wizard's, and packages
    /// not installed or installed for device setup), and all of the set
    /// is.
    fn same_set(
        &self,
        pa: &preferred::PreferredActivity,
        query: &[ResolveInfo],
        exclude_setup_wizard: bool,
        user: i32,
    ) -> Result<bool> {
        let Some(set) = &pa.set else {
            return Ok(false);
        };
        let wizard = self.setup_wizard()?;
        let mut matched = 0;
        for ri in query {
            let (package, _) = ri.component();
            if exclude_setup_wizard && Some(package) == wizard {
                continue;
            }
            let Some(us) = self
                .state
                .packages
                .get(package)
                .and_then(|ps| ps.users.get(&user))
            else {
                continue;
            };
            if us.install_reason == INSTALL_REASON_DEVICE_SETUP {
                continue;
            }
            if !set.iter().any(|c| same(ri, c)) {
                return Ok(false);
            }
            matched += 1;
        }
        Ok(matched == set.len())
    }

    /// `PreferredComponent.isSuperset`.
    fn is_superset(
        &self,
        pa: &preferred::PreferredActivity,
        query: &[ResolveInfo],
        exclude_setup_wizard: bool,
    ) -> Result<bool> {
        let Some(set) = &pa.set else {
            return Ok(false);
        };
        if !exclude_setup_wizard && set.len() < query.len() {
            return Ok(false);
        }
        let wizard = self.setup_wizard()?;
        Ok(query.iter().all(|ri| {
            (exclude_setup_wizard && Some(ri.component().0) == wizard)
                || set.iter().any(|c| same(ri, c))
        }))
    }

    /// The end of `chooseBestActivity`: the resolver activity's
    /// `ResolveInfo`, showing the intent's package when all results are
    /// its.
    pub(super) fn chooser(
        &self,
        intent: &Intent,
        query: &[ResolveInfo],
        user: i32,
    ) -> Result<ResolveInfo> {
        let mut browsers = 0;
        for ri in query {
            browsers += ri.handle_all_web_data_uri as usize;
            if let Info::Activity(ai) = &ri.info
                && ai.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0
            {
                return Err(NotModelled("an instant app's domain approval"));
            }
        }
        let mut ai = self.resolver_activity()?;
        let action = intent.action.as_deref();
        let titles = &self.state.platform.resolver_titles;
        ai.info.item.label_res = titles
            .iter()
            .find(|(a, _)| a.is_some() && a.as_deref() == action)
            .or_else(|| titles.iter().find(|(a, _)| a.is_none()))
            .map_or(0, |(_, id)| *id);
        ai.info.item.meta_data = Some(MetaData(vec![(
            METADATA_DOCK_HOME.into(),
            Value::Bool(true),
        )]));
        let mut ri = ResolveInfo::new(Info::Activity(ai));
        ri.handle_all_web_data_uri = browsers == query.len();
        ri.user_handle = user;
        let package = intent.package.as_deref().filter(|p| !p.is_empty());
        if let Some(package) = package
            && query.iter().all(|r| r.component().0 == package)
            && let Info::Activity(first) = &query[0].info
        {
            let app = &first.info.application_info;
            ri.resolve_package_name = Some(package.to_owned());
            ri.icon = app.item.icon;
            ri.icon_resource_id = app.item.icon;
            ri.label_res = app.item.label_res;
        }
        if user != 0
            && let Info::Activity(ai) = &mut ri.info
        {
            let mut app = (*ai.info.application_info).clone();
            app.uid = apps_filter::uid(user, app_id(app.uid));
            ai.info.application_info = Arc::new(app);
        }
        Ok(ri)
    }

    /// `mResolveActivity` as `setPlatformPackage` builds it: the `android`
    /// application of the system user, with its overlays.
    fn resolver_activity(&self) -> Result<ActivityInfo> {
        if self.state.platform.custom_resolver.is_some() {
            return Err(NotModelled("a custom resolver activity"));
        }
        let ps = self
            .state
            .packages
            .get("android")
            .ok_or(NotModelled("no android package"))?;
        let pkg = ps.pkg.as_deref().ok_or(NotModelled("no android package"))?;
        let t = Target {
            sys: &self.state.system,
            pkg,
            ps,
            state: &PackageUserState::default(),
            user: 0,
        };
        let mut app =
            generate_application_info(&t, 0).ok_or(NotModelled("no android application"))?;
        if let Some(o) = ps.users.get(&0).and_then(|u| u.overlay_paths.as_ref()) {
            app.overlay_paths = Some(o.overlay_paths.clone());
            app.resource_dirs = Some(o.resource_dirs.clone());
        }
        let package_name = app.item.package_name.clone();
        Ok(ActivityInfo {
            info: ComponentInfo {
                item: PackageItemInfo {
                    name: Some(RESOLVER_ACTIVITY.into()),
                    package_name,
                    ..PackageItemInfo::default()
                },
                application_info: Arc::new(app),
                process_name: Some("system:ui".into()),
                split_name: None,
                attribution_tags: None,
                description_res: 0,
                enabled: true,
                exported: true,
                direct_boot_aware: false,
            },
            theme: self.state.platform.resolver_theme,
            launch_mode: LAUNCH_MULTIPLE,
            document_launch_mode: DOCUMENT_LAUNCH_NEVER,
            permission: None,
            task_affinity: None,
            target_activity: None,
            flags: FLAG_EXCLUDE_FROM_RECENTS
                | FLAG_RELINQUISH_TASK_IDENTITY
                | FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES
                | FLAG_HARDWARE_ACCELERATED,
            private_flags: 0,
            screen_orientation: SCREEN_ORIENTATION_UNSPECIFIED,
            config_changes: CONFIG_SCREEN_SIZE
                | CONFIG_SMALLEST_SCREEN_SIZE
                | CONFIG_SCREEN_LAYOUT
                | CONFIG_ORIENTATION
                | CONFIG_KEYBOARD
                | CONFIG_KEYBOARD_HIDDEN,
            soft_input_mode: 0,
            ui_options: 0,
            parent_activity_name: None,
            persistable_mode: 0,
            max_recents: 0,
            lock_task_launch_mode: 0,
            window_layout: None,
            resize_mode: RESIZE_MODE_RESIZEABLE,
            requested_vr_component: None,
            rotation_animation: -1,
            color_mode: 0,
            max_aspect_ratio: 0.0,
            min_aspect_ratio: 0.0,
            supports_size_changes: false,
            known_activity_embedding_certs: None,
            required_display_category: None,
            require_content_uri_permission_from_caller: 0,
        })
    }
}
