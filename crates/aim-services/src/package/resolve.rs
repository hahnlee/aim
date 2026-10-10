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
    self, AppsFilter, NotModelled, SYSTEM_UID, app_id, instant_app_package_name,
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

#[derive(Debug, PartialEq, Eq)]
pub enum ResolutionError {
    Original(Exception),
    NotModelled(NotModelled),
    UriMatching(super::domain_verification::uri_parcel::MatchError),
}
impl From<NotModelled> for ResolutionError {
    fn from(error: NotModelled) -> Self {
        Self::NotModelled(error)
    }
}
type Result<T> = std::result::Result<T, ResolutionError>;
#[derive(Debug, PartialEq, Eq)]
pub enum QueryError {
    Original(Exception),
    NotModelled(NotModelled),
    Transport(aim_binder_host::parcel::StatusCode),
}
impl From<NotModelled> for QueryError {
    fn from(error: NotModelled) -> Self {
        Self::NotModelled(error)
    }
}

struct FilterParcelable(IntentFilter);
impl aim_service_aidl::WriteParcelable for FilterParcelable {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}

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
#[derive(Clone)]
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
        let preferred = if let Some(owner) = &state.system.preferred_owner {
            owner.captured.user_states().into_iter()
                .map(|(user, preferred)| (user, preferred.as_ref().clone())).collect()
        } else {
            state.users.iter().map(|(&id, user)| {
                let preferred = Preferred::parse(user.preferred_activities.as_deref(), user.restrictions.as_deref());
                (id, preferred)
            }).collect()
        };
        let legacy = state
            .users
            .iter()
            .map(|(&id, u)| (id, domains::legacy_domain_states(u)))
            .collect();
        Ok(Resolution {
            components: ComponentResolver::new(&state)?,
            apps_filter: AppsFilter::new(&state, config).map_err(MimeGroupError::UriMatching)?,
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
    ) -> Result<Option<Vec<ResolveInfo>>> {
        let at = Results {
            state: &self.state,
            user,
            flags,
        };
        self.components
            .query(kind, at, intent, resolved_type, package)
            .map_err(ResolutionError::UriMatching)
    }

    fn user_exists(&self, user: i32) -> bool {
        self.state.users.contains_key(&user)
    }

    /// The original query cross-user permission check, without the shell check.
    fn enforce_cross_user(&self, calling_uid: i32, user: i32) -> Result<()> {
        match self.query(calling_uid).enforce_cross_user(user, false, false, "query intent")? {
            Ok(()) => Ok(()),
            Err(_) => Err(NotModelled("query cross-user permission denied").into()),
        }
    }

    /// `ComputerEngine.updateFlags`: the direct boot match flags of the
    /// user's state unless the caller chose.
    fn update_flags(&self, flags: i64, user: i32) -> Result<i64> {
        self.query(1000).update_flags(flags,user).map_err(Into::into)
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
            let allow_match_instant = want_instant_apps
                || (want_match_instant
                    && self.query(calling_uid).internal_can_view_instant(calling_uid, user)?);
            flags &= !(MATCH_VISIBLE_TO_INSTANT_APP_ONLY | MATCH_EXPLICITLY_VISIBLE_ONLY);
            if !allow_match_instant {
                flags &= !MATCH_INSTANT;
            }
        }
        self.update_flags(flags, user)
    }

    /// `isImplicitImageCaptureIntentAndNotSetByDpc`: a camera intent no
    /// persistent preferred activity of a device policy answers.
    fn implicit_image_capture(
        &self,
        intent: &Intent,
        user: i32,
        resolved_type: Option<&str>,
        flags: i64,
    ) -> Result<bool> {
        if !intent.is_implicit_image_capture_intent() {
            return Ok(false);
        }
        let Some(preferred) = self.preferred(user) else {
            return Ok(true);
        };
        let entries = preferred::query(
            &preferred.persistent,
            intent,
            resolved_type,
            flags & MATCH_DEFAULT_ONLY != 0,
        )
        .map_err(ResolutionError::UriMatching)?;
        Ok(!entries.iter().any(|entry| entry.set_by_dpm))
    }

    /// The user's preferred activities.
    fn preferred(&self, user: i32) -> Option<&Preferred> {
        self.preferred.get(&user)
    }

    /// The model's limits: other profiles' results.
    fn single_profile(&self, user: i32) -> Result<()> {
        if self.state.users.len() > 1 {
            return Err(NotModelled("cross-profile resolution").into());
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
        self.query_activities_with_splits(intent, resolved_type, flags | MATCH_QUARANTINED_COMPONENTS,
            user, calling_uid, true)
    }

    fn query_activities_with_splits(&self, intent: &Intent, resolved_type: Option<&str>, flags: i64,
        user: i32, calling_uid: i32, allow_dynamic_splits: bool) -> Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
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
            self.implicit_image_capture(intent, user, resolved_type, flags)?,
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
                    )?;
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
            )?;
        }
        self.apply_post_resolution_filter(list, instant_pkg, allow_dynamic_splits, calling_uid, user, intent)
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
                let found = self.find(Kind::Activity, intent, resolved_type, flags, user, None)?;
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
                    )?;
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
            Err(NotModelled("instant app resolution").into())
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
        // A missing policy matters only for results that actually consult it;
        // ordinary full-app results do not acquire an unrelated owner dependency.
        let block_instant = || -> Result<bool> {
            if !intent.is_web_intent() { return Ok(false); }
            Ok(self.state.system.web_instant_policy.as_ref()
                .ok_or(NotModelled("captured web instant policy unavailable"))?.is_disabled(user))
        };
        let mut kept = Vec::with_capacity(list.len());
        for info in list.drain(..).rev() {
            if info.is_instant_app_available && block_instant()? { continue; }
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
                let installer = self.state.system.instant_components.as_ref()
                    .ok_or(NotModelled("captured instant installer owner unavailable"))?;
                let Some(template) = installer.installer_resolve_info() else { continue; };
                let package = ai.info.item.package_name.as_deref()
                    .ok_or(NotModelled("split activity package identity unavailable"))?;
                if self.state.packages.get(package)
                    .is_some_and(|package| super::info::user_state(package, user).instant_app)
                    && block_instant()? { continue; }
                let mut replacement = template.clone();
                replacement.auxiliary = Some(super::component_resolver::Auxiliary {
                    failure: self.find_install_failure_activity(package, calling_uid, user)?,
                    package: package.into(), version: ai.info.application_info.long_version_code,
                    split: split.clone(),
                });
                replacement.filter = Some(IntentFilter::default());
                replacement.resolve_package_name = Some(package.into());
                replacement.label_res = if info.label_res != 0 {info.label_res} else if ai.info.item.label_res != 0 {ai.info.item.label_res} else {ai.info.application_info.item.label_res};
                replacement.icon = if info.icon != 0 {info.icon} else if ai.info.item.icon != 0 {ai.info.item.icon} else {ai.info.application_info.item.icon};
                replacement.is_instant_app_available = true;
                // The original returns its installer before AppsFilter checks.
                kept.push(replacement);
                continue;
            }
            let (package, _) = info.component();
            if let Some(caller) = instant_pkg {
                let exposed_full_app = matches!(&info.info, Info::Activity(activity)
                    if activity.flags & FLAG_VISIBLE_TO_INSTANT_APP != 0
                        && activity.info.application_info.private_flags & PRIVATE_FLAG_INSTANT == 0);
                if caller == package || exposed_full_app { kept.push(info); }
                continue;
            }
            let target = self.state.packages.get(package);
            let hidden = match target {
                Some(t) => self
                    .apps_filter
                    .should_filter(&self.state, calling_uid, t, user),
                None => return Err(NotModelled("a result without its package").into()),
            };
            if !hidden {
                kept.push(info);
            }
        }
        kept.reverse();
        Ok(kept)
    }

    fn find_install_failure_activity(&self, package: &str, caller: i32, user: i32) -> Result<Option<ComponentName>> {
        let intent = Intent { action: Some("android.intent.action.INSTALL_FAILURE".into()),
            package: Some(package.into()), ..Default::default() };
        let matches = self.query_activities_with_splits(&intent, None, 0, user, caller, false)?;
        Ok(matches.into_iter().find_map(|result| match result.info {
            Info::Activity(activity) if activity.info.split_name.is_none() => Some(ComponentName {
                package: package.into(), class: activity.info.item.name?,
            }), _ => None,
        }))
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
    ) -> Result<()> {
        /// `ParsedMainComponentImpl.INTENT_MATCHING_FLAGS_*`.
        const NONE: i32 = 1;
        const ENFORCE_INTENT_FILTER: i32 = 1 << 1;
        const ALLOW_NULL_ACTION: i32 = 1 << 2;
        if matches!(app_id(calling_uid), 0 | SYSTEM_UID) {
            return Ok(());
        }
        let mut failure = None;
        list.retain(|ri| {
            if failure.is_some() {
                return false;
            }
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
                if failure.is_some() {
                    return false;
                }
                i.filter
                    .matches(
                        intent.action.as_deref(),
                        resolved_type,
                        intent.scheme(),
                        intent.data.as_ref(),
                        intent.categories.as_deref(),
                        false,
                        None,
                    )
                    .map_or_else(
                        |error| {
                            failure = Some(error);
                            false
                        },
                        |matched| matched >= 0,
                    )
            });
            !((null_action && flags & ALLOW_NULL_ACTION == 0) || !matches)
        });
        if let Some(error) = failure {
            return Err(ResolutionError::UriMatching(error));
        }
        Ok(())
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
            self.implicit_image_capture(intent, user, resolved_type, flags)?,
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

    /// ResolveIntentHelper's specific activity options and generic action deduplication.
    pub fn query_activity_options(&self, caller: Option<&ComponentName>,
        specifics: Option<&[Option<Intent>]>, specific_types: Option<&[Option<String>]>,
        intent: &Intent, resolved_type: Option<&str>, flags: i64, user: i32,
        calling_uid: i32) -> Result<Vec<ResolveInfo>> {
        use super::component_resolver::GET_RESOLVED_FILTER;
        if !self.user_exists(user) { return Ok(Vec::new()); }
        let capture = self.implicit_image_capture(intent, user, resolved_type, flags)?;
        let flags = self.update_flags_for_resolve(flags, user, calling_uid, false, false, capture)?;
        self.enforce_cross_user(calling_uid, user)?;
        let mut results = self.query_intent_activities(intent, resolved_type,
            flags | GET_RESOLVED_FILTER, user, calling_uid)?;
        let mut position = 0;
        for (index, specific) in specifics.unwrap_or_default().iter().enumerate() {
            let Some(specific) = specific else { continue; };
            let action = specific.action.as_deref().filter(|action| intent.action.as_deref() != Some(*action));
            let (mut selected, activity, component) = if let Some(component) = &specific.component {
                let Some(activity) = thrown(self.query(calling_uid).activity_info(component, flags, calling_uid, user))? else { continue; };
                (None, activity, component.clone())
            } else {
                let ty = match specific_types {
                    Some(types) => types.get(index).ok_or(NotModelled("specific activity type array index outside original Java array"))?.as_deref(),
                    None => None,
                };
                let Some(selected) = self.resolve_intent(specific, ty, flags, user, calling_uid)? else { continue; };
                let Info::Activity(activity) = &selected.info else { return Err(NotModelled("specific result is not an activity").into()); };
                let (package, class) = selected.component();
                let component = ComponentName { package: package.into(), class: class.into() };
                (Some(selected.clone()), activity.clone(), component)
            };
            let mut cursor = position;
            while cursor < results.len() {
                let result = &results[cursor];
                let (package, class) = result.component();
                let duplicate = package == component.package && class == component.class
                    || action.is_some_and(|action| result.filter.as_ref().is_some_and(|filter| filter.actions.iter().any(|entry| entry == action)));
                if duplicate {
                    let removed = results.remove(cursor);
                    if selected.is_none() { selected = Some(removed); }
                } else { cursor += 1; }
            }
            let mut selected = selected.unwrap_or_else(|| ResolveInfo::new(Info::Activity(activity)));
            selected.specific_index = index as i32;
            results.insert(position, selected);
            position += 1;
        }
        let mut index = position;
        while index + 1 < results.len() {
            let actions = results[index].filter.as_ref().map(|filter| filter.actions.clone()).unwrap_or_default();
            for action in actions {
                if intent.action.as_deref() == Some(action.as_str()) { continue; }
                let mut cursor = index + 1;
                while cursor < results.len() {
                    if results[cursor].filter.as_ref().is_some_and(|filter| filter.actions.contains(&action)) {
                        results.remove(cursor);
                    } else { cursor += 1; }
                }
            }
            index += 1;
        }
        if let Some(caller) = caller {
            if let Some(index) = results.iter().position(|entry| {
                let (package, class) = entry.component();
                package == caller.package && class == caller.class
            }) { results.remove(index); }
        }
        if flags & GET_RESOLVED_FILTER == 0 {
            for entry in &mut results { entry.filter = None; }
        }
        Ok(results)
    }

    pub fn can_forward_to(&self, intent: &Intent, resolved_type: Option<&str>, source: i32, target: i32,
        calling_uid: i32) -> Result<bool> {
        fn reachable(resolution: &Resolution, intent: &Intent, ty: Option<&str>, source: i32, target: i32,
            visited: &mut HashSet<i32>) -> Result<bool> {
            if source == target { return Ok(true); }
            visited.insert(source);
            let Some(owner) = resolution.preferred(source) else { return Ok(false); };
            let matches = preferred::query(&owner.cross_profile, intent, ty, false).map_err(ResolutionError::UriMatching)?;
            for filter in matches {
                if filter.target_user_id == target { return Ok(true); }
                if visited.contains(&filter.target_user_id) { continue; }
                if filter.flags & 0x10 != 0 {
                    visited.insert(filter.target_user_id);
                    if reachable(resolution, intent, ty, filter.target_user_id, target, visited)? { return Ok(true); }
                }
            }
            Ok(false)
        }
        if reachable(self, intent, resolved_type, source, target, &mut HashSet::new())? { return Ok(true); }
        if !intent.has_web_uri() { return Ok(false); }
        let policy = self.state.system.user_policy.as_ref().ok_or(NotModelled("actual UserManager cross-profile owner unavailable"))?;
        let Some(parent) = policy.profile_parent(source).map_err(ResolutionError::Original)? else { return Ok(false); };
        let capture = self.implicit_image_capture(intent, parent, resolved_type, 0)?;
        let flags = self.update_flags_for_resolve(0, parent, calling_uid, false, false, capture)? | MATCH_DEFAULT_ONLY;
        if !policy.parent_app_linking(source).map_err(ResolutionError::Original)? { return Ok(false); }
        let matches = self.find(Kind::Activity, intent, resolved_type, flags, parent, None)?.unwrap_or_default();
        let Some(host) = intent.data.as_ref().and_then(|data| data.host()) else { return Ok(false); };
        for matched in matches {
            if matched.handle_all_web_data_uri { continue; }
            let (package, _) = matched.component();
            if let Some(package) = self.state.packages.get(package) {
                let groups = package.uri_relative_filter_groups.iter().find(|(domain, _)| domain == &host)
                    .map_or(&[][..], |(_, groups)| groups.as_slice());
                if !groups.is_empty() && !super::intent_filter::UriRelativeFilterGroup::match_groups(groups, intent.data.as_ref().unwrap())
                    .map_err(ResolutionError::UriMatching)? { continue; }
                if self.approval_level(package, &host, parent)? > 0 { return Ok(true); }
            }
        }
        Ok(false)
    }

    pub(crate) fn launch_candidates(&self, package: &str, category: &str, user: i32, caller: i32) -> std::result::Result<Vec<ResolveInfo>, NotModelled> {
        if !self.user_exists(user) || !self.has_code(package) { return Ok(Vec::new()); }
        let intent = Intent { action: Some("android.intent.action.MAIN".into()), categories: Some(vec![category.into()]), package: Some(package.into()), ..Default::default() };
        let flags = self.update_flags_for_resolve(MATCH_QUARANTINED_COMPONENTS, user, caller, false, true, false)
            .map_err(|_| NotModelled("launch resolution flag owner unavailable"))?;
        // ResolveIntentHelper uses resolveForStart=true and allowDynamicSplits=false:
        // the native component match is retained without application-query filtering.
        self.find(Kind::Activity, &intent, None, flags, user, Some(package))
            .map(|results| results.unwrap_or_default())
            .map_err(|_| NotModelled("launch activity component match owner unavailable"))
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
                        )?;
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
                        self.find(Kind::Service, intent, resolved_type, flags, user, package)?;
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
            )?;
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
                return Err(NotModelled("an instant app's results").into());
            }
            let (package, _) = info.component();
            let Some(target) = self.state.packages.get(package) else {
                return Err(NotModelled("a result without its package").into());
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
                return Err(NotModelled("an instant app's results").into());
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
            self.implicit_image_capture(intent, user, resolved_type, flags)?,
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
                        )?;
                    }
                }
                list
            }
            None => {
                let package = intent.package.as_deref();
                let mut list = Vec::new();
                if package.is_none() {
                    list = self
                        .find(Kind::Receiver, intent, resolved_type, flags, user, None)?
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
                        )?
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
            )?;
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
        let found = self.find(Kind::Provider, intent, resolved_type, flags, user, package)?;
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
        self.resolve_content_provider_for_query(name, flags, user, calling_uid, calling_uid)
    }
    pub fn resolve_content_provider_for_query(&self, name: &str, flags: i64, user: i32,
        calling_uid: i32, actual_uid: i32) -> Result<Option<ProviderInfo>> {
        if !self.user_exists(user) {
            return Ok(None);
        }
        let flags = self.update_flags(flags, user)?;
        // `userId@authority` names another user's provider
        // (`ContentProvider.getUserIdFromAuthority`; USER_NULL when not a
        // number).
        let (authority, user) = match name.rsplit_once('@') {
            Some((u, a)) => (a, uri::parse_int(u).unwrap_or(-10000)),
            None => (name, user),
        };
        let native = self
            .state
            .package_registry
            .as_ref()
            .map(|r| r.authority(authority));
        let legacy = if native.is_none() {
            self.components.provider_by_authority(authority)
        } else {
            None
        };

        let package = match native {
            Some(Some(row)) => row.package.as_str(),
            Some(None) => { self.enforce_provider_scope(authority, calling_uid, user, None)?; return Ok(None); },
            None => match legacy {
                Some(row) => row.package.as_str(),
                None => { self.enforce_provider_scope(authority, calling_uid, user, None)?; return Ok(None); },
            },
        };
        let Some(ps) = self.state.packages.get(package) else {
            self.enforce_provider_scope(authority, calling_uid, user, None)?;
            return Ok(None);
        };
        let Some(pkg) = ps.pkg.as_deref() else {
            self.enforce_provider_scope(authority, calling_uid, user, None)?;
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
            self.enforce_provider_scope(authority, calling_uid, user, None)?;
            return Ok(None);
        };
        let mut provider = if let Some(Some(row)) = native {
            Cow::Borrowed(&row.value)
        } else {
            Cow::Borrowed(&pkg.providers[legacy.unwrap().index])
        };
        if let Some(copy) = legacy.and_then(|registered| registered.copy.as_ref()) {
            let p = provider.to_mut();
            p.authority = Some(copy.clone());
            p.syncable = false;
        }
        let Some(pi) = generate_provider_info(&t, &provider, flags, Some(Arc::new(app))) else {
            self.enforce_provider_scope(authority, calling_uid, user, None)?;
            return Ok(None);
        };
        self.enforce_provider_scope(authority, calling_uid, user, Some(&pi))?;
        if !is_enabled_and_matches(ps, &provider.main, flags, user) { return Ok(None); }
        let component = ComponentName { package: package.into(), class: provider.main.component.name.clone() };
        let query = Query { state: &self.state, filter: &self.apps_filter, calling_uid: actual_uid };
        if query.filtered_component(Some(ps), &component, 4, calling_uid, user)? { return Ok(None); }
        Ok(Some(pi))
    }
    fn enforce_provider_scope(&self, authority: &str, calling_uid: i32, user: i32,
        provider: Option<&ProviderInfo>) -> Result<()> {
        let cross_user = user != user_id(calling_uid);
        let checked = if let Some(provider) = provider.filter(|_| cross_user) {
            self.state.system.uri_access.as_ref().ok_or(NotModelled("native provider URI grants owner unavailable"))?
                .check(calling_uid, provider, user).map_err(ResolutionError::Original)?
        } else { false };
        if checked { return Ok(()); }
        let redirected = if cross_user {
            self.state.system.uri_access.as_ref().ok_or(NotModelled("native clone provider redirection owner unavailable"))?
                .clone_redirected(authority, calling_uid, user).map_err(ResolutionError::Original)?
        } else { false };
        if !redirected {
            if let Err(exception) = self.query(calling_uid).internal_enforce_cross_user(calling_uid,
                user, false, false, "resolveContentProvider")? { return Err(ResolutionError::Original(exception)); }
        }
        Ok(())
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
    r?.map_err(|_| NotModelled("an exception from a component lookup").into())
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
    /// Only the dynamic implicit-access owner changed; package relations,
    /// components, preferences and domains remain the same validated inputs.
    pub(crate) fn implicit_access_view(&self, before: &Arc<State>, state: Arc<State>)
        -> std::result::Result<Self, MimeGroupError> {
        let mut resolution = (*self.resolution(before)?).clone();
        resolution.state = state;
        Ok(Self { latest: Mutex::new(Some(Arc::new(resolution))), reported: Mutex::new(HashSet::new()) })
    }

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
                Err(QueryError::NotModelled(NotModelled(reason))) => {
                    self.not_modelled(call.code, reason)
                }
                Err(QueryError::Original(error)) => { let mut reply = Parcel::new(); reply.write_exception(&error); Answer::Reply(reply) },
                Err(QueryError::Transport(status)) => Answer::Status(status),
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
    ) -> Option<std::result::Result<Parcel, QueryError>> {
        if !super::query::preferred::MUTATION_METHODS.contains(&code) && !matches!(
            code,
            pm::QUERY_INTENT_ACTIVITIES
                | pm::QUERY_INTENT_SERVICES
                | pm::QUERY_INTENT_RECEIVERS
                | pm::QUERY_INTENT_CONTENT_PROVIDERS
                | pm::RESOLVE_INTENT
                | pm::RESOLVE_SERVICE
                | pm::RESOLVE_CONTENT_PROVIDER
                | pm::RESOLVE_CONTENT_PROVIDER_FOR_UID
                | pm::GET_ALL_INTENT_FILTERS
                | pm::FIND_PERSISTENT_PREFERRED_ACTIVITY
                | pm::QUERY_INTENT_ACTIVITY_OPTIONS
                | pm::CAN_FORWARD_TO
                | pm::GET_LAST_CHOSEN_ACTIVITY
                | pm::GET_HOME_ACTIVITIES
        ) {
            return None;
        }
        let r = match self.resolution(state) {
            Ok(r) => r,
            Err(error) => return Some(error.reply().map_err(QueryError::Transport)),
        };
        if super::query::preferred::MUTATION_METHODS.contains(&code) {
            let Some(owner) = state.system.preferred_owner.as_deref() else {
                return Some(Err(NotModelled("native preferred action owners unavailable").into()));
            };
            return super::query::preferred::mutation(&r.query(uid), &r, code, data, owner);
        }
        if code == pm::GET_HOME_ACTIVITIES {
            return Some((|| {
                let owner = state.system.preferred_owner.as_deref()
                    .ok_or(NotModelled("native preferred home owner unavailable"))?;
                super::query::preferred::home_activities(&r.query(uid), &r, data, owner)
            })());
        }
        if code == pm::GET_LAST_CHOSEN_ACTIVITY {
            return Some((|| {
                let owner = state.system.preferred_owner.as_deref()
                    .ok_or(NotModelled("native preferred owner unavailable"))?;
                super::query::preferred::last_chosen(&r.query(uid), &r, data, owner)
            })());
        }
        if code == pm::CAN_FORWARD_TO {
            return Some((|| {
                let a = pm::CanForwardTo::<Intent>::read(data).map_err(QueryError::Transport)?;
                if data.remaining() != 0 { return Err(QueryError::Transport(BAD_VALUE)); }
                let query = r.query(uid); let mut reply = Parcel::new();
                if !query.uid_has_permission(uid, "android.permission.INTERACT_ACROSS_USERS_FULL")? {
                    reply.write_exception(&Exception::security("android.permission.INTERACT_ACROSS_USERS_FULL required")); return Ok(reply);
                }
                if a.source_user_id == a.target_user_id {
                    pm::write_can_forward_to_reply(&mut reply, true); return Ok(reply);
                }
                let Some(intent) = a.intent.as_ref() else { reply.write_exception(&Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "intent is null")); return Ok(reply); };
                match r.can_forward_to(intent, a.resolved_type.as_deref(), a.source_user_id, a.target_user_id, uid) {
                    Ok(value) => pm::write_can_forward_to_reply(&mut reply, value),
                    Err(ResolutionError::Original(exception)) => reply.write_exception(&exception),
                    Err(ResolutionError::NotModelled(error)) => return Err(error.into()),
                    Err(ResolutionError::UriMatching(error)) => match error.binder_exception() {
                        Some(exception) => reply.write_exception(&exception),
                        None => return Err(QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION)),
                    },
                }
                Ok(reply)
            })());
        }
        if code == pm::RESOLVE_CONTENT_PROVIDER_FOR_UID {
            return Some((|| {
                let a = pm::ResolveContentProviderForUid::read(data).map_err(QueryError::Transport)?;
                if data.remaining() != 0 { return Err(QueryError::Transport(BAD_VALUE)); }
                let query = r.query(uid);
                let mut reply = Parcel::new();
                if !matches!(app_id(uid), 0 | SYSTEM_UID)
                    && !query.uid_has_permission(uid, "android.permission.RESOLVE_COMPONENT_FOR_UID")? {
                    reply.write_exception(&Exception::security("resolveContentProviderForUid: requires android.permission.RESOLVE_COMPONENT_FOR_UID"));
                    return Ok(reply);
                }
                if let Err(error) = query.enforce_cross_user(user_id(a.calling_uid), false, false,
                    "resolveContentProviderForUid")? {
                    reply.write_exception(&error);
                    return Ok(reply);
                }
                let filter_uid = a.calling_uid;
                let hidden = if apps_filter::is_sdk_sandbox(filter_uid) {
                    uid != filter_uid
                } else {
                    match apps_filter::setting(state, app_id(filter_uid)) {
                        Some(apps_filter::Setting::Package(package)) => query.filtered_including_uninstalled(Some(package), user_id(filter_uid))?,
                        Some(apps_filter::Setting::Shared(shared)) => query.shared_filtered(shared, user_id(filter_uid), true)?,
                        None => true,
                    }
                };
                if hidden {
                    pm::write_resolve_content_provider_for_uid_reply::<ProviderInfo>(&mut reply, None);
                    return Ok(reply);
                }
                let Some(name) = a.authority.as_deref() else {
                    reply.write_exception(&Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "name is null"));
                    return Ok(reply);
                };
                macro_rules! provider_result {
                    ($expression:expr) => { match $expression {
                        Ok(value) => value,
                        Err(ResolutionError::Original(exception)) => { reply.write_exception(&exception); return Ok(reply); },
                        Err(ResolutionError::NotModelled(error)) => return Err(error.into()),
                        Err(ResolutionError::UriMatching(error)) => {
                            if let Some(exception) = error.binder_exception() { reply.write_exception(&exception); return Ok(reply); }
                            return Err(QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION));
                        }
                    } }
                }
                let first = provider_result!(r.resolve_content_provider_for_query(name, a.flags, a.user_id, filter_uid, uid));
                let second = if first.is_some() { provider_result!(r.resolve_content_provider_for_query(name, a.flags, a.user_id, uid, uid)) } else { None };
                let result = first.filter(|first| second.as_ref().is_some_and(|second|
                    first.info.item.name == second.info.item.name && first.authority == second.authority));
                pm::write_resolve_content_provider_for_uid_reply(&mut reply, result.as_ref());
                Ok(reply)
            })());
        }
        if code == pm::FIND_PERSISTENT_PREFERRED_ACTIVITY {
            return Some((|| {
                let a = pm::FindPersistentPreferredActivity::<Intent>::read(data)
                    .map_err(QueryError::Transport)?;
                if data.remaining() != 0 { return Err(QueryError::Transport(BAD_VALUE)); }
                let mut reply = Parcel::new();
                if app_id(uid) != SYSTEM_UID {
                    reply.write_exception(&Exception::security("findPersistentPreferredActivity can only be run by the system"));
                    return Ok(reply);
                }
                if !state.users.contains_key(&a.user_id) {
                    pm::write_find_persistent_preferred_activity_reply::<ResolveInfo>(&mut reply, None);
                    return Ok(reply);
                }
                let Some(intent) = a.intent.as_ref() else {
                    reply.write_exception(&Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "intent is null"));
                    return Ok(reply);
                };
                let intent = intent.selector.as_deref().unwrap_or(intent);
                if intent.ty.is_none() && intent.data.as_ref().and_then(|data| data.scheme()) == Some("content") {
                    return Err(NotModelled("persistent preferred content-provider MIME owner").into());
                }
                let resolved = if intent.component.is_some() { None } else { intent.ty.as_deref() };
                macro_rules! resolved {
                    ($expression:expr) => {
                        match $expression {
                            Ok(value) => value,
                            Err(ResolutionError::Original(exception)) => { reply.write_exception(&exception); return Ok(reply); }
                            Err(ResolutionError::NotModelled(error)) => return Err(error.into()),
                            Err(ResolutionError::UriMatching(error)) => {
                                let Some(exception) = error.binder_exception() else {
                                    return Err(QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION));
                                };
                                reply.write_exception(&exception);
                                return Ok(reply);
                            }
                        }
                    }
                }
                let capture = resolved!(r.implicit_image_capture(intent, a.user_id, resolved, 0));
                let flags = resolved!(r.update_flags_for_resolve(0, a.user_id, uid, false, false, capture));
                let entries = resolved!(r.query_intent_activities(intent, resolved, flags, a.user_id, uid));
                let chosen = resolved!(r.find_persistent(intent, resolved, flags, &entries, a.user_id, uid));
                pm::write_find_persistent_preferred_activity_reply(&mut reply, chosen.as_ref());
                Ok(reply)
            })());
        }
        if code == pm::GET_ALL_INTENT_FILTERS {
            return Some((|| {
                let a = pm::GetAllIntentFilters::read(data).map_err(QueryError::Transport)?;
                if data.remaining() != 0 {
                    return Err(QueryError::Transport(BAD_VALUE));
                }
                let query = Query {
                    state,
                    filter: &r.apps_filter,
                    calling_uid: uid,
                };
                let package = state
                    .packages
                    .get(a.package_name.as_deref().unwrap_or_default());
                let mut filters = Vec::new();
                if let Some(package) = package.filter(|package| package.pkg.is_some()) {
                    if !query.filtered_including_uninstalled(Some(package), user_id(uid))? {
                        use super::intent_resolver::Entry;
                        for entry in r
                            .components
                            .activities
                            .entries()
                            .iter()
                            .filter(|entry| entry.package == package.name)
                        {
                            filters.push(FilterParcelable(entry.filter().clone()));
                        }
                    }
                }
                let slice = ListSlice {
                    creator: "android.content.IntentFilter".into(),
                    items: filters,
                };
                let mut reply = Parcel::new();
                pm::write_get_all_intent_filters_reply(&mut reply, Some(&slice));
                Ok(reply)
            })());
        }
        let mut p = Parcel::new();
        let slice = |items| ListSlice {
            creator: "android.content.pm.ResolveInfo".into(),
            items,
        };
        let done: Result<()> = match code {
            pm::QUERY_INTENT_ACTIVITY_OPTIONS => {
                let Ok(a) = pm::QueryIntentActivityOptions::<ComponentName, Intent>::read(data) else {
                    return Some(Err(NotModelled("malformed activity options query").into()));
                };
                let Some(intent) = a.intent.as_ref() else {
                    return Some(Err(NotModelled("null activity options intent").into()));
                };
                r.query_activity_options(a.caller.as_ref(), a.specifics.as_deref(),
                    a.specific_types.as_deref(), intent, a.resolved_type.as_deref(), a.flags,
                    a.user_id, uid).map(|items| pm::write_query_intent_activity_options_reply(&mut p,
                        Some(&slice(items))))
            }
            pm::QUERY_INTENT_ACTIVITIES
            | pm::QUERY_INTENT_SERVICES
            | pm::QUERY_INTENT_RECEIVERS
            | pm::QUERY_INTENT_CONTENT_PROVIDERS => {
                let Ok(a) = pm::QueryIntentActivities::<Intent>::read(data) else {
                    return Some(Err(NotModelled("a malformed intent query").into()));
                };
                let Some(intent) = &a.intent else {
                    return Some(Err(NotModelled("a null intent").into()));
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
                    return Some(Err(NotModelled("a malformed intent resolution").into()));
                };
                let Some(intent) = &a.intent else {
                    return Some(Err(NotModelled("a null intent").into()));
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
                    return Some(Err(NotModelled("a malformed provider resolution").into()));
                };
                let Some(name) = &a.name else {
                    return Some(Err(NotModelled("a null authority").into()));
                };
                r.resolve_content_provider(name, a.flags, a.user_id, uid)
                    .map(|pi| pm::write_resolve_content_provider_reply(&mut p, pi.as_ref()))
            }
            _ => return None,
        };
        Some(match done {
            Ok(()) => Ok(p),
            Err(ResolutionError::Original(exception)) => {
                let mut reply = Parcel::new(); reply.write_exception(&exception); Ok(reply)
            }
            Err(ResolutionError::NotModelled(error)) => Err(QueryError::NotModelled(error)),
            Err(ResolutionError::UriMatching(error)) => {
                let Some(exception) = error.binder_exception() else {
                    return Some(Err(QueryError::Transport(
                        aim_binder_host::parcel::UNKNOWN_TRANSACTION,
                    )));
                };
                let mut reply = Parcel::new();
                reply.write_exception(&exception);
                Ok(reply)
            }
        })
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
mod internal_records;
pub mod policy;
pub mod settings;
mod domains;
pub(crate) use domains::is_domain_name;

#[cfg(test)]
mod tests;

pub mod preferred_owner;
mod facade;
