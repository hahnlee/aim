//! `package`'s intent queries: `queryIntentActivities`, `queryIntentServices`,
//! `queryIntentReceivers`, `queryIntentContentProviders`, `resolveIntent`,
//! `resolveService` and `resolveContentProvider`, as `IPackageManagerBase`,
//! `ComputerEngine` and `ResolveIntentHelper` answer them at
//! `android-16.0.0_r1`, over the [`ComponentResolver`] and the
//! [`AppsFilter`] of one version of the state.
//!
//! Paths that need state the model does not have answer [`NotModelled`]
//! rather than a guess: instant apps (#725), web links' domain
//! verification (#726), the chooser's preferred and resolver activities
//! (#727), other profiles and persistent preferred activities (#715).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::apps_filter::{
    self, AppsFilter, NotModelled, Result, SYSTEM_UID, app_id, instant_app_package_name,
    should_filter_application, user_id,
};
use super::component_resolver::{
    ComponentResolver, Info, Kind, MATCH_DEFAULT_ONLY, MATCH_EXPLICITLY_VISIBLE_ONLY,
    MATCH_INSTANT, MATCH_VISIBLE_TO_INSTANT_APP_ONLY, MimeGroupError, ResolveInfo, Results,
    resolve_priority_order,
};
use super::info::{
    ActivityInfo, ProviderInfo, Target, generate_application_info, generate_provider_info,
    is_enabled_and_matches,
};
use super::intent::{
    ComponentName, FLAG_ACTIVITY_MATCH_EXTERNAL, FLAG_ACTIVITY_REQUIRE_NON_BROWSER,
    FLAG_IGNORE_EPHEMERAL, Intent,
};
use super::intent_filter::{IntentFilter, Plain};
use super::model::State;
use super::preferred::{self, Preferred};
use super::query::Query;
use super::reply;
use super::uri;
use crate::clip::char_sequence;
use crate::shadow::{Answer, IntoValue, ListSlice, ShadowCall, Value, decode};

/// `PackageManager` flags.
pub const MATCH_DIRECT_BOOT_UNAWARE: i64 = 0x0004_0000;
pub const MATCH_DIRECT_BOOT_AWARE: i64 = 0x0008_0000;
pub const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
pub const MATCH_QUARANTINED_COMPONENTS: i64 = 1 << 33;

/// `ActivityInfo`, `ServiceInfo` and `ProviderInfo.FLAG_*`.
const FLAG_VISIBLE_TO_INSTANT_APP: i32 = 0x0010_0000;
const FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP: i32 = 0x0020_0000;
const FLAG_SYSTEM_USER_ONLY: i32 = 0x2000_0000;
/// `ApplicationInfo.PRIVATE_FLAG_INSTANT`.
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;
/// `UserHandle.USER_SYSTEM`.
const USER_SYSTEM: i32 = 0;

/// Resolution over one version of the state.
pub struct Resolution {
    pub state: Arc<State>,
    pub components: ComponentResolver,
    pub apps_filter: AppsFilter,
    /// Each user's preferred activities.
    preferred: BTreeMap<i32, Preferred>,
    /// `getSetupWizardPackageNameImpl`, found once.
    setup_wizard: OnceLock<Option<String>>,
    /// Each user's legacy domain verification states, by package.
    legacy: BTreeMap<i32, HashMap<String, i32>>,
}

impl Resolution {
    pub fn new(
        state: Arc<State>,
        config: &apps_filter::Config,
    ) -> std::result::Result<Resolution, MimeGroupError> {
        let preferred = state
            .users
            .iter()
            .map(|(&id, u)| {
                let p =
                    Preferred::parse(u.preferred_activities.as_deref(), u.restrictions.as_deref());
                (id, p)
            })
            .collect();
        let legacy = state
            .users
            .iter()
            .map(|(&id, u)| (id, domains::legacy_domain_states(u)))
            .collect();
        Ok(Resolution {
            components: ComponentResolver::new(&state)?,
            apps_filter: AppsFilter::new(&state, config),
            state,
            preferred,
            setup_wizard: OnceLock::new(),
            legacy,
        })
    }

    /// m4model's queries over this state, for the caller.
    fn query(&self, calling_uid: i32) -> Query<'_> {
        Query {
            state: &self.state,
            filter: &self.apps_filter,
            calling_uid,
        }
    }

    /// Whether the package is known with its code (`mPackages.get`).
    fn has_code(&self, package: &str) -> bool {
        self.state
            .packages
            .get(package)
            .is_some_and(|ps| ps.pkg.is_some())
    }

    /// The component resolver's query over this state.
    fn find(
        &self,
        kind: Kind,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        package: Option<&str>,
    ) -> Option<Vec<ResolveInfo>> {
        let at = Results {
            state: &self.state,
            user,
            flags,
        };
        self.components
            .query(kind, at, intent, resolved_type, package)
    }

    fn user_exists(&self, user: i32) -> bool {
        self.state.users.contains_key(&user)
    }

    /// `enforceCrossUserPermission` without the shell check: a caller in
    /// another user needs INTERACT_ACROSS_USERS, which the model does not
    /// check.
    fn enforce_cross_user(&self, calling_uid: i32, user: i32) -> Result<()> {
        if user_id(calling_uid) == user || matches!(app_id(calling_uid), 0 | SYSTEM_UID) {
            Ok(())
        } else {
            Err(NotModelled("a query of another user's packages"))
        }
    }

    /// `ComputerEngine.updateFlags`: the direct boot match flags of the
    /// user's state unless the caller chose.
    fn update_flags(&self, flags: i64, user: i32) -> i64 {
        if flags & (MATCH_DIRECT_BOOT_UNAWARE | MATCH_DIRECT_BOOT_AWARE) != 0 {
            return flags;
        }
        let unlocked = self
            .state
            .users
            .get(&user)
            .is_some_and(|u| u.unlocking_or_unlocked);
        if unlocked {
            flags | MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE
        } else {
            flags | MATCH_DIRECT_BOOT_AWARE
        }
    }

    /// `updateFlagsForResolve`.
    fn update_flags_for_resolve(
        &self,
        mut flags: i64,
        user: i32,
        calling_uid: i32,
        want_instant_apps: bool,
        only_exposed_explicitly: bool,
        implicit_image_capture: bool,
    ) -> Result<i64> {
        if implicit_image_capture {
            flags |= MATCH_SYSTEM_ONLY;
        }
        if instant_app_package_name(&self.state, calling_uid)?.is_some() {
            if only_exposed_explicitly {
                flags |= MATCH_EXPLICITLY_VISIBLE_ONLY;
            }
            flags |= MATCH_VISIBLE_TO_INSTANT_APP_ONLY | MATCH_INSTANT;
        } else {
            let want_match_instant = flags & MATCH_INSTANT != 0;
            if !want_instant_apps
                && want_match_instant
                && calling_uid >= apps_filter::FIRST_APPLICATION_UID
            {
                return Err(NotModelled("whether the caller may see instant apps"));
            }
            let allow_match_instant = want_instant_apps || want_match_instant;
            flags &= !(MATCH_VISIBLE_TO_INSTANT_APP_ONLY | MATCH_EXPLICITLY_VISIBLE_ONLY);
            if !allow_match_instant {
                flags &= !MATCH_INSTANT;
            }
        }
        Ok(self.update_flags(flags, user))
    }

    /// `isImplicitImageCaptureIntentAndNotSetByDpc`: a camera intent no
    /// persistent preferred activity of a device policy answers.
    fn implicit_image_capture(
        &self,
        intent: &Intent,
        user: i32,
        resolved_type: Option<&str>,
        flags: i64,
    ) -> bool {
        intent.is_implicit_image_capture_intent()
            && !self.preferred(user).is_some_and(|p| {
                let default_only = flags & MATCH_DEFAULT_ONLY != 0;
                preferred::query(&p.persistent, intent, resolved_type, default_only)
                    .iter()
                    .any(|ppa| ppa.set_by_dpm)
            })
    }

    /// The user's preferred activities.
    fn preferred(&self, user: i32) -> Option<&Preferred> {
        self.preferred.get(&user)
    }

    /// The model's limits: other profiles' results.
    fn single_profile(&self, user: i32) -> Result<()> {
        if self.state.users.len() > 1 {
            return Err(NotModelled("cross-profile resolution"));
        }
        debug_assert!(self.user_exists(user));
        Ok(())
    }

    /// `queryIntentActivitiesInternal` of a binder call: not for a start,
    /// dynamic splits allowed, the caller's uid as both.
    pub fn query_intent_activities(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        let flags = flags | MATCH_QUARANTINED_COMPONENTS;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        self.enforce_cross_user(calling_uid, user)?;
        let package = intent.package.as_deref();
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let comp = intent.component.as_ref();
        let flags = self.update_flags_for_resolve(
            flags,
            user,
            calling_uid,
            false,
            comp.is_some() || package.is_some(),
            self.implicit_image_capture(intent, user, resolved_type, flags),
        )?;
        let mut list = match comp {
            Some(comp) => {
                let mut list = Vec::new();
                let found = self
                    .query(calling_uid)
                    .activity_info(comp, flags, calling_uid, user);
                if let Some(ai) = thrown(found)?
                    && !self.block_activity(&ai, comp, flags, instant_pkg, calling_uid, user)?
                {
                    let mut ri = ResolveInfo::new(Info::Activity(ai));
                    ri.user_handle = user;
                    list.push(ri);
                    self.enforce_intent_filter_matching(
                        Kind::Activity,
                        intent,
                        resolved_type,
                        calling_uid,
                        &mut list,
                    );
                }
                list
            }
            None => self.query_activities_body(
                intent,
                resolved_type,
                flags,
                calling_uid,
                user,
                package,
            )?,
        };
        // blockNullAction only reports unless the block_null_action_intents
        // flag is on, which it is not on this image.
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Activity,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            );
        }
        self.apply_post_resolution_filter(list, instant_pkg, true, calling_uid, user, intent)
    }

    /// The explicit component's instant-app and visibility blocks.
    fn block_activity(
        &self,
        ai: &ActivityInfo,
        comp: &ComponentName,
        flags: i64,
        instant_pkg: Option<&str>,
        calling_uid: i32,
        user: i32,
    ) -> Result<bool> {
        let caller_instant = instant_pkg.is_some();
        let target_instant = ai.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
        let visible = ai.flags & FLAG_VISIBLE_TO_INSTANT_APP != 0;
        let explicitly_visible = visible && ai.flags & FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP == 0;
        let hidden =
            !visible || (flags & MATCH_EXPLICITLY_VISIBLE_ONLY != 0 && !explicitly_visible);
        let block_instant = instant_pkg != Some(comp.package.as_str())
            && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0 && caller_instant && hidden));
        let block_normal = !target_instant
            && !caller_instant
            && should_filter_application(
                &self.state,
                &self.apps_filter,
                ai.info
                    .application_info
                    .item
                    .package_name
                    .as_deref()
                    .and_then(|p| self.state.packages.get(p)),
                calling_uid,
                user,
                false,
                true,
            )?;
        Ok(block_instant || block_normal)
    }

    /// `queryIntentActivitiesInternalBody` and the single profile's
    /// `combineFilterAndCreateQueryActivitiesResponse`.
    fn query_activities_body(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        calling_uid: i32,
        user: i32,
        package: Option<&str>,
    ) -> Result<Vec<ResolveInfo>> {
        self.single_profile(user)?;
        let mut result = Vec::new();
        match package {
            None => {
                let found = self.find(Kind::Activity, intent, resolved_type, flags, user, None);
                result.extend(filter_if_not_system_user(found.unwrap_or_default(), user));
                self.check_instant_resolution(intent, &result, false)?;
            }
            Some(package) => {
                let ps = self.state.packages.get(package);
                let visible = match ps {
                    Some(ps) if ps.pkg.is_some() => !should_filter_application(
                        &self.state,
                        &self.apps_filter,
                        Some(ps),
                        calling_uid,
                        user,
                        false,
                        true,
                    )?,
                    _ => false,
                };
                if visible {
                    let found = self.find(
                        Kind::Activity,
                        intent,
                        resolved_type,
                        flags,
                        user,
                        Some(package),
                    );
                    result.extend(filter_if_not_system_user(found.unwrap_or_default(), user));
                }
                if result.is_empty() {
                    self.check_instant_resolution(intent, &result, true)?;
                }
            }
        }
        if package.is_none() && intent.has_web_uri() {
            if result.len() > 1 {
                result = self.filter_web_candidates(intent, flags, result, user)?;
            }
            // sortResult.
            result.sort_by(resolve_priority_order);
        }
        Ok(result)
    }

    /// `isInstantAppResolutionAllowed` up to the domain checks: without
    /// an instant app resolver and installer it is never allowed; when
    /// its checks pass, the original asks its resolver, which the model
    /// does not (#725).
    fn check_instant_resolution(
        &self,
        intent: &Intent,
        resolved: &[ResolveInfo],
        skip_package_check: bool,
    ) -> Result<()> {
        let platform = &self.state.platform;
        if platform.instant_app_resolver.is_none()
            || platform.instant_app_installer.is_none()
            || intent.component.is_some()
            || intent.has_flag(FLAG_IGNORE_EPHEMERAL)
            || intent.has_flag(FLAG_ACTIVITY_REQUIRE_NON_BROWSER)
            || (!skip_package_check && intent.package.is_some())
        {
            return Ok(());
        }
        let possible = if intent.is_web_intent() {
            intent
                .data
                .as_ref()
                .and_then(|d| d.host())
                .is_some_and(|h| !h.is_empty())
        } else {
            resolved.is_empty() && intent.has_flag(FLAG_ACTIVITY_MATCH_EXTERNAL)
        };
        if possible {
            Err(NotModelled("instant app resolution"))
        } else {
            Ok(())
        }
    }

    /// `applyPostResolutionFilter` for a full or an instant app caller,
    /// not for a start.
    fn apply_post_resolution_filter(
        &self,
        mut list: Vec<ResolveInfo>,
        instant_pkg: Option<&str>,
        allow_dynamic_splits: bool,
        calling_uid: i32,
        user: i32,
        intent: &Intent,
    ) -> Result<Vec<ResolveInfo>> {
        if instant_pkg.is_some() {
            return Err(NotModelled("an instant app's results"));
        }
        let mut kept = Vec::with_capacity(list.len());
        for info in list.drain(..).rev() {
            if info.is_instant_app_available && intent.is_web_intent() {
                return Err(NotModelled("web instant apps' setting"));
            }
            if let Info::Activity(ai) = &info.info
                && allow_dynamic_splits
                && let Some(split) = &ai.info.split_name
                && !ai
                    .info
                    .application_info
                    .split_names
                    .iter()
                    .flatten()
                    .flatten()
                    .any(|s| s == split)
            {
                return Err(NotModelled("an activity in a split not installed"));
            }
            let (package, _) = info.component();
            let target = self.state.packages.get(package);
            let hidden = match target {
                Some(t) => self
                    .apps_filter
                    .should_filter(&self.state, calling_uid, t, user),
                None => return Err(NotModelled("a result without its package")),
            };
            if !hidden {
                kept.push(info);
            }
        }
        kept.reverse();
        Ok(kept)
    }

    /// `SaferIntentUtils.enforceIntentFilterMatching` with intent matching
    /// flags (`enable_intent_matching_flags` is on): a component that asks
    /// for it drops an intent none of its filters match.
    fn enforce_intent_filter_matching(
        &self,
        kind: Kind,
        intent: &Intent,
        resolved_type: Option<&str>,
        calling_uid: i32,
        list: &mut Vec<ResolveInfo>,
    ) {
        /// `ParsedMainComponentImpl.INTENT_MATCHING_FLAGS_*`.
        const NONE: i32 = 1;
        const ENFORCE_INTENT_FILTER: i32 = 1 << 1;
        const ALLOW_NULL_ACTION: i32 = 1 << 2;
        if matches!(app_id(calling_uid), 0 | SYSTEM_UID) {
            return;
        }
        list.retain(|ri| {
            let Info::Activity(ai) = &ri.info else {
                return true;
            };
            if app_id(ai.info.application_info.uid) == app_id(calling_uid)
                && user_id(ai.info.application_info.uid) == user_id(calling_uid)
            {
                return true;
            }
            let (package, class) = ri.component();
            let Some(main) = self.main_component(kind, package, class) else {
                return true;
            };
            if main.component.intents.is_empty() {
                return true;
            }
            let flags = main.intent_matching_flags;
            let enforce = flags != 0 && flags & NONE != NONE && flags & ENFORCE_INTENT_FILTER != 0;
            if !enforce {
                return true;
            }
            let null_action = intent.action.is_none();
            let matches = main.component.intents.iter().any(|i| {
                i.filter.matches(
                    intent.action.as_deref(),
                    resolved_type,
                    intent.scheme(),
                    intent.data.as_ref(),
                    intent.categories.as_deref(),
                    false,
                    None,
                ) >= 0
            });
            !((null_action && flags & ALLOW_NULL_ACTION == 0) || !matches)
        });
    }

    /// A component by kind, package and class (`infoToComponent`).
    fn main_component(
        &self,
        kind: Kind,
        package: &str,
        class: &str,
    ) -> Option<&super::pkg::MainComponent> {
        let pkg = self.state.packages.get(package)?.pkg.as_deref()?;
        match kind {
            Kind::Activity => pkg
                .activities
                .iter()
                .map(|a| &a.main)
                .find(|m| m.component.name == class),
            Kind::Receiver => pkg
                .receivers
                .iter()
                .map(|a| &a.main)
                .find(|m| m.component.name == class),
            Kind::Service => pkg
                .services
                .iter()
                .map(|s| &s.main)
                .find(|m| m.component.name == class),
            Kind::Provider => pkg
                .providers
                .iter()
                .map(|p| &p.main)
                .find(|m| m.component.name == class),
        }
    }

    /// `ResolveIntentHelper.resolveIntentInternal` of a binder call.
    pub fn resolve_intent(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Option<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(None);
        }
        let flags = self.update_flags_for_resolve(
            flags,
            user,
            calling_uid,
            false,
            false,
            self.implicit_image_capture(intent, user, resolved_type, flags),
        )?;
        self.enforce_cross_user(calling_uid, user)?;
        let mut query =
            self.query_intent_activities(intent, resolved_type, flags, user, calling_uid)?;
        // chooseBestActivity.
        match query.len() {
            0 => Ok(None),
            1 => Ok(query.pop()),
            _ => {
                let (r0, r1) = (&query[0], &query[1]);
                if r0.priority != r1.priority
                    || r0.preferred_order != r1.preferred_order
                    || r0.is_default != r1.is_default
                {
                    return Ok(Some(query.swap_remove(0)));
                }
                let preferred = self.find_preferred_activity(
                    intent,
                    resolved_type,
                    flags,
                    &query,
                    user,
                    calling_uid,
                )?;
                if let Some(ri) = preferred {
                    return Ok(Some(ri));
                }
                self.chooser(intent, &query, user).map(Some)
            }
        }
    }

    /// `queryIntentServicesInternal` of a binder call: no instant apps,
    /// not for a start.
    pub fn query_intent_services(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        self.enforce_cross_user(calling_uid, user)?;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        let flags = self.update_flags_for_resolve(flags, user, calling_uid, false, false, false)?;
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let mut list = match &intent.component {
            Some(comp) => {
                let mut list = Vec::new();
                if let Some(si) = thrown(self.query(calling_uid).service_info(comp, flags, user))? {
                    let caller_instant = instant_pkg.is_some();
                    let target_instant =
                        si.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
                    let hidden = si.flags & FLAG_VISIBLE_TO_INSTANT_APP == 0;
                    let block_instant = instant_pkg != Some(comp.package.as_str())
                        && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                            || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0
                                && caller_instant
                                && hidden));
                    let block_normal = !target_instant
                        && !caller_instant
                        && should_filter_application(
                            &self.state,
                            &self.apps_filter,
                            si.info
                                .application_info
                                .item
                                .package_name
                                .as_deref()
                                .and_then(|p| self.state.packages.get(p)),
                            calling_uid,
                            user,
                            false,
                            true,
                        )?;
                    if !block_instant && !block_normal {
                        list.push(ResolveInfo::new(Info::Service(si)));
                        self.enforce_intent_filter_matching(
                            Kind::Service,
                            intent,
                            resolved_type,
                            calling_uid,
                            &mut list,
                        );
                    }
                }
                list
            }
            None => {
                let package = intent.package.as_deref();
                if package.is_some_and(|p| !self.has_code(p)) {
                    Vec::new()
                } else {
                    let found =
                        self.find(Kind::Service, intent, resolved_type, flags, user, package);
                    self.post_filter_others(
                        found.unwrap_or_default(),
                        instant_pkg,
                        calling_uid,
                        user,
                    )?
                }
            }
        };
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Service,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            );
        }
        Ok(list)
    }

    /// `applyPostServiceResolutionFilter` and
    /// `applyPostContentProviderResolutionFilter` for a full app caller.
    fn post_filter_others(
        &self,
        list: Vec<ResolveInfo>,
        instant_pkg: Option<&str>,
        calling_uid: i32,
        user: i32,
    ) -> Result<Vec<ResolveInfo>> {
        let mut kept = Vec::with_capacity(list.len());
        for info in list.into_iter().rev() {
            if instant_pkg.is_some() {
                return Err(NotModelled("an instant app's results"));
            }
            let (package, _) = info.component();
            let Some(target) = self.state.packages.get(package) else {
                return Err(NotModelled("a result without its package"));
            };
            if !self
                .apps_filter
                .should_filter(&self.state, calling_uid, target, user)
            {
                kept.push(info);
                continue;
            }
            // Hidden, unless an exposed component of a full app (the
            // original keeps those for instant callers only, but asks
            // the same question).
            let (flags, instant) = match &info.info {
                Info::Service(s) => (
                    s.flags,
                    s.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0,
                ),
                Info::Provider(p) => (
                    p.flags,
                    p.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0,
                ),
                Info::Activity(_) => unreachable!("services and providers only"),
            };
            if instant {
                return Err(NotModelled("an instant app's results"));
            }
            if flags & FLAG_VISIBLE_TO_INSTANT_APP != 0 {
                kept.push(info);
            }
        }
        kept.reverse();
        Ok(kept)
    }

    /// `ResolveIntentHelper.resolveServiceInternal`.
    pub fn resolve_service(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Option<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(None);
        }
        let flags = self.update_flags_for_resolve(flags, user, calling_uid, false, false, false)?;
        let mut list =
            self.query_intent_services(intent, resolved_type, flags, user, calling_uid)?;
        Ok((!list.is_empty()).then(|| list.swap_remove(0)))
    }

    /// `ResolveIntentHelper.queryIntentReceiversInternal` of a binder
    /// call (not for a send).
    pub fn query_intent_receivers(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        self.enforce_cross_user(calling_uid, user)?;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        let flags = self.update_flags_for_resolve(
            flags,
            user,
            calling_uid,
            false,
            false,
            self.implicit_image_capture(intent, user, resolved_type, flags),
        )?;
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let mut list = match &intent.component {
            Some(comp) => {
                let mut list = Vec::new();
                if let Some(ai) = thrown(self.query(calling_uid).receiver_info(comp, flags, user))?
                {
                    let caller_instant = instant_pkg.is_some();
                    let target_instant =
                        ai.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
                    let visible = ai.flags & FLAG_VISIBLE_TO_INSTANT_APP != 0;
                    let explicitly_visible =
                        visible && ai.flags & FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP == 0;
                    let hidden = !visible
                        || (flags & MATCH_EXPLICITLY_VISIBLE_ONLY != 0 && !explicitly_visible);
                    let block = instant_pkg != Some(comp.package.as_str())
                        && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                            || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0
                                && caller_instant
                                && hidden));
                    if !block {
                        list.push(ResolveInfo::new(Info::Activity(ai)));
                        self.enforce_intent_filter_matching(
                            Kind::Receiver,
                            intent,
                            resolved_type,
                            calling_uid,
                            &mut list,
                        );
                    }
                }
                list
            }
            None => {
                let package = intent.package.as_deref();
                let mut list = Vec::new();
                if package.is_none() {
                    list = self
                        .find(Kind::Receiver, intent, resolved_type, flags, user, None)
                        .unwrap_or_default();
                }
                if let Some(package) = package.filter(|p| {
                    self.state
                        .packages
                        .get(*p)
                        .is_some_and(|ps| ps.pkg.is_some())
                }) {
                    list = self
                        .find(
                            Kind::Receiver,
                            intent,
                            resolved_type,
                            flags,
                            user,
                            Some(package),
                        )
                        .unwrap_or_default();
                }
                list
            }
        };
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Receiver,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            );
        }
        self.apply_post_resolution_filter(list, instant_pkg, false, calling_uid, user, intent)
    }

    /// `ResolveIntentHelper.queryIntentContentProvidersInternal`.
    pub fn query_intent_content_providers(
        &self,
        intent: &Intent,
        resolved_type: Option<&str>,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        let flags = self.update_flags_for_resolve(flags, user, calling_uid, false, false, false)?;
        let intent = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => &**selector,
            _ => intent,
        };
        if let Some(comp) = &intent.component {
            let mut list = Vec::new();
            if let Some(pi) = thrown(self.query(calling_uid).provider_info(comp, flags, user))? {
                let caller_instant = instant_pkg.is_some();
                let target_instant =
                    pi.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
                let hidden = pi.flags & FLAG_VISIBLE_TO_INSTANT_APP == 0;
                let block = instant_pkg != Some(comp.package.as_str())
                    && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                        || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0
                            && caller_instant
                            && hidden));
                let block_normal = !target_instant
                    && !caller_instant
                    && should_filter_application(
                        &self.state,
                        &self.apps_filter,
                        pi.info
                            .application_info
                            .item
                            .package_name
                            .as_deref()
                            .and_then(|p| self.state.packages.get(p)),
                        calling_uid,
                        user,
                        false,
                        true,
                    )?;
                if !block && !block_normal {
                    list.push(ResolveInfo::new(Info::Provider(pi)));
                }
            }
            return Ok(list);
        }
        let package = intent.package.as_deref();
        if package.is_some_and(|p| !self.has_code(p)) {
            return Ok(Vec::new());
        }
        let found = self.find(Kind::Provider, intent, resolved_type, flags, user, package);
        self.post_filter_others(found.unwrap_or_default(), instant_pkg, calling_uid, user)
    }

    /// `ComputerEngine.resolveContentProvider`.
    pub fn resolve_content_provider(
        &self,
        name: &str,
        flags: i64,
        user: i32,
        calling_uid: i32,
    ) -> Result<Option<ProviderInfo>> {
        if !self.user_exists(user) {
            return Ok(None);
        }
        let flags = self.update_flags(flags, user);
        // `userId@authority` names another user's provider
        // (`ContentProvider.getUserIdFromAuthority`; USER_NULL when not a
        // number).
        let (authority, user) = match name.rsplit_once('@') {
            Some((u, a)) => (a, uri::parse_int(u).unwrap_or(-10000)),
            None => (name, user),
        };
        let provider = self.components.provider_by_authority(authority);
        if provider.is_some() && user != user_id(calling_uid) {
            return Err(NotModelled("another user's provider (URI grants)"));
        }
        self.enforce_cross_user(calling_uid, user)?;
        let Some(registered) = provider else {
            return Ok(None);
        };
        let Some(ps) = self.state.packages.get(&registered.package) else {
            return Ok(None);
        };
        let Some(pkg) = ps.pkg.as_deref() else {
            return Ok(None);
        };
        let us = ps.users.get(&user).cloned().unwrap_or_default();
        let t = Target {
            sys: &self.state.system,
            pkg,
            ps,
            state: &us,
            user,
        };
        let Some(app) = generate_application_info(&t, flags) else {
            return Ok(None);
        };
        let mut provider = Cow::Borrowed(&pkg.providers[registered.index]);
        if let Some(copy) = &registered.copy {
            let p = provider.to_mut();
            p.authority = Some(copy.clone());
            p.syncable = false;
        }
        let Some(pi) = generate_provider_info(&t, &provider, flags, Some(Arc::new(app))) else {
            return Ok(None);
        };
        if !is_enabled_and_matches(ps, &provider.main, flags, user) {
            return Ok(None);
        }
        if should_filter_application(
            &self.state,
            &self.apps_filter,
            Some(ps),
            calling_uid,
            user,
            false,
            true,
        )? {
            return Ok(None);
        }
        Ok(Some(pi))
    }
}

impl Resolver {
    /// Decodes a reply of one of the intent queries, the original's and
    /// the model's alike; `None` for the other methods.
    pub fn decode_reply(&self, code: u32, reply: &mut Reader<'_>) -> Option<ParcelResult<Value>> {
        Some(match code {
            pm::QUERY_INTENT_ACTIVITIES
            | pm::QUERY_INTENT_SERVICES
            | pm::QUERY_INTENT_RECEIVERS
            | pm::QUERY_INTENT_CONTENT_PROVIDERS => decode(
                reply,
                pm::read_query_intent_activities_reply::<Decoded<ResolveInfoList>>,
            ),
            pm::RESOLVE_INTENT | pm::RESOLVE_SERVICE => decode(
                reply,
                pm::read_resolve_intent_reply::<Decoded<ResolveInfoValue>>,
            ),
            pm::RESOLVE_CONTENT_PROVIDER => decode(
                reply,
                pm::read_resolve_content_provider_reply::<Decoded<ProviderInfoValue>>,
            ),
            _ => return None,
        })
    }
}

/// A parcelable read straight into a [`Value`].
struct Decoded<T>(Value, std::marker::PhantomData<T>);

trait ValueReader {
    fn read(r: &mut Reader<'_>) -> ParcelResult<Value>;
}

impl<T: ValueReader> ReadParcelable for Decoded<T> {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Decoded(T::read(r)?, std::marker::PhantomData))
    }
}

impl<T> IntoValue for Decoded<T> {
    fn into_value(self) -> Value {
        self.0
    }
}

/// A `ParceledListSlice<ResolveInfo>` read whole (the framework stitched
/// its chunks into the reply).
struct ResolveInfoList;

impl ValueReader for ResolveInfoList {
    fn read(r: &mut Reader<'_>) -> ParcelResult<Value> {
        let count = r.read_i32()?;
        let mut items = Vec::new();
        if count > 0 {
            if r.read_string16()?.as_deref() != Some("android.content.pm.ResolveInfo") {
                return Err(BAD_VALUE);
            }
            for _ in 0..count {
                if r.read_i32()? == 0 {
                    items.push(Value::Null);
                    continue;
                }
                items.push(ResolveInfoValue::read(r)?);
            }
        }
        Ok(Value::List(items))
    }
}

/// `ResolveInfo(Parcel)` as fields.
struct ResolveInfoValue;

impl ValueReader for ResolveInfoValue {
    fn read(r: &mut Reader<'_>) -> ParcelResult<Value> {
        let mut f: Vec<(String, Value)> = Vec::new();
        let mut put = |name: &str, v: Value| f.push((name.into(), v));
        let info = match r.read_i32()? {
            1 => ("activityInfo", reply::activity_info(r)?),
            2 => ("serviceInfo", reply::service_info(r)?),
            3 => ("providerInfo", reply::provider_info(r)?),
            _ => ("info", Value::Null),
        };
        put(info.0, info.1);
        let filter = if r.read_i32()? != 0 {
            filter_value(&IntentFilter::read(r, &mut Plain)?)
        } else {
            Value::Null
        };
        put("filter", filter);
        for name in [
            "priority",
            "preferredOrder",
            "match",
            "specificIndex",
            "labelRes",
        ] {
            put(name, Value::Int(r.read_i32()?));
        }
        put("nonLocalizedLabel", char_sequence(r)?.into_value());
        put("icon", Value::Int(r.read_i32()?));
        put("resolvePackageName", r.read_string8()?.into_value());
        put("targetUserId", Value::Int(r.read_i32()?));
        for name in ["system", "noResourceId"] {
            put(name, Value::Bool(r.read_i32()? != 0));
        }
        put("iconResourceId", Value::Int(r.read_i32()?));
        for name in [
            "handleAllWebDataURI",
            "autoResolutionAllowed",
            "isInstantAppAvailable",
        ] {
            put(name, Value::Bool(r.read_i32()? != 0));
        }
        put("userHandle", Value::Int(r.read_i32()?));
        Ok(Value::Fields(f))
    }
}

struct ProviderInfoValue;

impl ValueReader for ProviderInfoValue {
    fn read(r: &mut Reader<'_>) -> ParcelResult<Value> {
        reply::provider_info(r)
    }
}

/// An intent filter's fields, as `writeToParcel` orders them.
fn filter_value(f: &IntentFilter) -> Value {
    let strings = |l: &Option<Vec<String>>| l.clone().into_value();
    let patterns = |l: &Option<Vec<super::intent_filter::PatternMatcher>>| {
        l.as_ref()
            .map(|l| {
                l.iter()
                    .map(|p| format!("{}:{}", p.kind, p.pattern))
                    .collect::<Vec<_>>()
            })
            .into_value()
    };
    let authorities = f.authorities.as_ref().map(|l| {
        l.iter()
            .map(|a| format!("{}:{}", a.orig_host, a.port))
            .collect::<Vec<_>>()
    });
    Value::Fields(vec![
        ("actions".into(), f.actions.clone().into_value()),
        ("categories".into(), strings(&f.categories)),
        ("schemes".into(), strings(&f.schemes)),
        ("staticTypes".into(), strings(&f.static_types)),
        ("types".into(), strings(&f.types)),
        ("mimeGroups".into(), strings(&f.mime_groups)),
        ("ssps".into(), patterns(&f.ssps)),
        ("authorities".into(), authorities.into_value()),
        ("paths".into(), patterns(&f.paths)),
        ("priority".into(), Value::Int(f.priority)),
        ("autoVerify".into(), Value::Bool(f.auto_verify)),
        (
            "instantAppVisibility".into(),
            Value::Int(f.instant_app_visibility),
        ),
        ("order".into(), Value::Int(f.order)),
        (
            "extras".into(),
            f.extras.clone().map_or(Value::Null, Value::Bytes),
        ),
        (
            "groups".into(),
            Value::Int(f.uri_relative_filter_groups.as_ref().map_or(0, Vec::len) as i32),
        ),
    ])
}

/// A component lookup's answer; the exception it may throw (a cross-user
/// check) is not modelled here.
fn thrown<T>(r: std::result::Result<std::result::Result<T, Exception>, NotModelled>) -> Result<T> {
    r?.map_err(|_| NotModelled("an exception from a component lookup"))
}

/// `filterIfNotSystemUser`: an activity for the system user only is not
/// another user's.
fn filter_if_not_system_user(mut list: Vec<ResolveInfo>, user: i32) -> Vec<ResolveInfo> {
    if user != USER_SYSTEM {
        list.retain(
            |ri| !matches!(&ri.info, Info::Activity(ai) if ai.flags & FLAG_SYSTEM_USER_ONLY != 0),
        );
    }
    list
}

/// The resolution of the latest state, rebuilt when the state changes.
#[derive(Default)]
pub struct Resolver {
    latest: Mutex<Option<Arc<Resolution>>>,
    /// Not-modelled reasons already reported, by method code.
    reported: Mutex<HashSet<(u32, &'static str)>>,
}

impl Resolver {
    /// The resolution of `state`, built once per state.
    pub fn resolution(
        &self,
        state: &Arc<State>,
    ) -> std::result::Result<Arc<Resolution>, MimeGroupError> {
        let mut latest = self.latest.lock().unwrap();
        match &*latest {
            Some(r) if Arc::ptr_eq(&r.state, state) => Ok(r.clone()),
            _ => {
                let config = apps_filter::Config {
                    force_system_packages_queryable: state.system.force_system_packages_queryable,
                    force_queryable_packages: state.system.force_queryable_packages.clone(),
                };
                let r = Arc::new(Resolution::new(state.clone(), &config)?);
                *latest = Some(r.clone());
                Ok(r)
            }
        }
    }

    /// Answers `call` if it is one of the intent queries; `None` for the
    /// other methods of `package`.
    pub fn answer(&self, state: &Arc<State>, call: &mut ShadowCall<'_>) -> Option<Answer> {
        if call.descriptor != pm::DESCRIPTOR {
            return None;
        }
        self.query(state, call.code, call.sender_euid as i32, &mut call.data)
            .map(|answer| match answer {
                Ok(reply) => Answer::Reply(reply),
                Err(NotModelled(reason)) => self.not_modelled(call.code, reason),
            })
    }

    /// Intent queries shared by the shadow and native service. The
    /// reader includes the AIDL interface token; `uid` is Binder's
    /// caller, never a package name supplied in a transaction.
    pub fn query(
        &self,
        state: &Arc<State>,
        code: u32,
        uid: i32,
        data: &mut Reader<'_>,
    ) -> Option<Result<Parcel>> {
        if !matches!(
            code,
            pm::QUERY_INTENT_ACTIVITIES
                | pm::QUERY_INTENT_SERVICES
                | pm::QUERY_INTENT_RECEIVERS
                | pm::QUERY_INTENT_CONTENT_PROVIDERS
                | pm::RESOLVE_INTENT
                | pm::RESOLVE_SERVICE
                | pm::RESOLVE_CONTENT_PROVIDER
        ) {
            return None;
        }
        let r = match self.resolution(state) {
            Ok(r) => r,
            Err(error) => return Some(Ok(error.reply())),
        };
        let mut p = Parcel::new();
        let slice = |items| ListSlice {
            creator: "android.content.pm.ResolveInfo".into(),
            items,
        };
        let done: Result<()> = match code {
            pm::QUERY_INTENT_ACTIVITIES
            | pm::QUERY_INTENT_SERVICES
            | pm::QUERY_INTENT_RECEIVERS
            | pm::QUERY_INTENT_CONTENT_PROVIDERS => {
                let Ok(a) = pm::QueryIntentActivities::<Intent>::read(data) else {
                    return Some(Err(NotModelled("a malformed intent query")));
                };
                let Some(intent) = &a.intent else {
                    return Some(Err(NotModelled("a null intent")));
                };
                let rt = a.resolved_type.as_deref();
                let query = match code {
                    pm::QUERY_INTENT_ACTIVITIES => Resolution::query_intent_activities,
                    pm::QUERY_INTENT_SERVICES => Resolution::query_intent_services,
                    pm::QUERY_INTENT_RECEIVERS => Resolution::query_intent_receivers,
                    _ => Resolution::query_intent_content_providers,
                };
                query(&r, intent, rt, a.flags, a.user_id, uid)
                    .map(|list| pm::write_query_intent_activities_reply(&mut p, Some(&slice(list))))
            }
            pm::RESOLVE_INTENT | pm::RESOLVE_SERVICE => {
                let Ok(a) = pm::ResolveIntent::<Intent>::read(data) else {
                    return Some(Err(NotModelled("a malformed intent resolution")));
                };
                let Some(intent) = &a.intent else {
                    return Some(Err(NotModelled("a null intent")));
                };
                let resolve = match code {
                    pm::RESOLVE_INTENT => Resolution::resolve_intent,
                    _ => Resolution::resolve_service,
                };
                resolve(
                    &r,
                    intent,
                    a.resolved_type.as_deref(),
                    a.flags,
                    a.user_id,
                    uid,
                )
                .map(|ri| pm::write_resolve_intent_reply(&mut p, ri.as_ref()))
            }
            pm::RESOLVE_CONTENT_PROVIDER => {
                let Ok(a) = pm::ResolveContentProvider::read(data) else {
                    return Some(Err(NotModelled("a malformed provider resolution")));
                };
                let Some(name) = &a.name else {
                    return Some(Err(NotModelled("a null authority")));
                };
                r.resolve_content_provider(name, a.flags, a.user_id, uid)
                    .map(|pi| pm::write_resolve_content_provider_reply(&mut p, pi.as_ref()))
            }
            _ => return None,
        };
        Some(done.map(|()| p))
    }

    /// Reports a reason the first time a method meets it.
    fn not_modelled(&self, code: u32, reason: &'static str) -> Answer {
        if self.reported.lock().unwrap().insert((code, reason)) {
            let method = pm::METHODS
                .iter()
                .find(|(c, _)| *c == code)
                .map_or("?", |(_, n)| n);
            eprintln!("package shadow: {method} not modelled: {reason}");
        }
        Answer::NotModelled
    }
}

mod chooser;
mod domains;
pub(crate) use domains::is_domain_name;

#[cfg(test)]
mod tests;
