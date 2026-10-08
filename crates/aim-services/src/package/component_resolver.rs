//! `ComponentResolver`: the intent filters of every package's activities,
//! receivers, services and providers, the providers by authority, and a
//! match's `ResolveInfo` (`android-16.0.0_r1`,
//! `services/core/java/com/android/server/pm/resolution`).
//!
//! The model builds it from a whole state. The original adds packages in
//! its scan's order, but results are sorted by package name after their
//! priority and match, and a package's own filters keep its manifest
//! order either way, so the order of the replies does not depend on it.
//! Activity priorities are adjusted at registration using the captured privilege,
//! factory activity filters and authoritative setup wizard selection.

#[path="component_resolver/raw.rs"] mod raw;
#[path="component_resolver/priority.rs"] mod priority;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use super::info::{
    ActivityInfo, ProviderInfo, ServiceInfo, Target, generate_activity_info,
    generate_application_info, generate_provider_info, generate_service_info,
    is_enabled_and_matches, user_state,
};
use super::intent::{Intent,ComponentName};
use super::intent_filter::{CATEGORY_BROWSABLE, IntentFilter};
use super::intent_resolver::{Build, Entry, IntentResolver, query_from_list};
use super::model::{PackageState, PackageUserState, State};
use super::pkg::{AndroidPackage, MainComponent};
use crate::clip::write_char_sequence;

/// `PackageManager.MATCH_*`, `GET_*` resolve flags.
pub const GET_RESOLVED_FILTER: i64 = 0x0000_0040;
pub const MATCH_DEFAULT_ONLY: i64 = 0x0001_0000;
pub const MATCH_INSTANT: i64 = 0x0080_0000;
pub const MATCH_VISIBLE_TO_INSTANT_APP_ONLY: i64 = 0x0100_0000;
pub const MATCH_EXPLICITLY_VISIBLE_ONLY: i64 = 0x0200_0000;

/// `ApplicationInfo.FLAG_SYSTEM`.
const FLAG_SYSTEM: i32 = 1;
/// `UserInfo.FLAG_MANAGED_PROFILE`: a profile whose results are badged.
const FLAG_MANAGED_PROFILE: i32 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Activity,
    Receiver,
    Service,
    Provider,
}

/// A component's filter in a resolver: where the component is in its
/// package, and the filter with the package's MIME groups applied
/// (`applyMimeGroups`).
#[derive(Clone, Debug)]
pub struct FilterEntry {
    pub kind: Kind,
    pub package: String,
    pub component: usize,
    pub intent: usize,
    filter: IntentFilter,
}

impl Entry for FilterEntry {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.package
    }
}

/// The component's part a result shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Info {
    Activity(ActivityInfo),
    Service(ServiceInfo),
    Provider(ProviderInfo),
}

#[derive(Clone,Debug,PartialEq)]
pub struct Auxiliary {
    pub failure:Option<ComponentName>,pub package:String,pub version:i64,pub split:String,
}
/// `android.content.pm.ResolveInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolveInfo {
    pub info: Info,
    pub auxiliary: Option<Auxiliary>,
    pub filter: Option<IntentFilter>,
    pub priority: i32,
    pub preferred_order: i32,
    pub match_: i32,
    pub specific_index: i32,
    pub is_default: bool,
    pub label_res: i32,
    pub non_localized_label: Option<String>,
    pub icon: i32,
    pub resolve_package_name: Option<String>,
    pub target_user_id: i32,
    pub system: bool,
    pub no_resource_id: bool,
    pub icon_resource_id: i32,
    pub handle_all_web_data_uri: bool,
    pub auto_resolution_allowed: bool,
    pub is_instant_app_available: bool,
    /// `UserHandle.USER_CURRENT` (-2) when the original sets none.
    pub user_handle: i32,
}

impl WriteParcelable for ResolveInfo {
    /// `ResolveInfo.writeToParcel`.
    fn write_to(&self, p: &mut Parcel) {
        match &self.info {
            Info::Activity(a) => {
                p.write_i32(1);
                a.write_to(p);
            }
            Info::Service(s) => {
                p.write_i32(2);
                s.write_to(p);
            }
            Info::Provider(i) => {
                p.write_i32(3);
                i.write_to(p);
            }
        }
        match &self.filter {
            Some(f) => {
                p.write_i32(1);
                f.write(p);
            }
            None => p.write_i32(0),
        }
        for v in [
            self.priority,
            self.preferred_order,
            self.match_,
            self.specific_index,
            self.label_res,
        ] {
            p.write_i32(v);
        }
        write_char_sequence(p, self.non_localized_label.as_deref());
        p.write_i32(self.icon);
        p.write_string8(self.resolve_package_name.as_deref());
        p.write_i32(self.target_user_id);
        p.write_i32(self.system as i32);
        p.write_i32(self.no_resource_id as i32);
        p.write_i32(self.icon_resource_id);
        p.write_i32(self.handle_all_web_data_uri as i32);
        p.write_i32(self.auto_resolution_allowed as i32);
        p.write_i32(self.is_instant_app_available as i32);
        p.write_i32(self.user_handle);
    }
}

/// `UserHandle.USER_CURRENT`.
pub const USER_CURRENT: i32 = -2;

impl ResolveInfo {
    /// `new ResolveInfo()` around a component.
    pub fn new(info: Info) -> ResolveInfo {
        ResolveInfo {
            info,
            auxiliary: None,
            filter: None,
            priority: 0,
            preferred_order: 0,
            match_: 0,
            specific_index: -1,
            is_default: false,
            label_res: 0,
            non_localized_label: None,
            icon: 0,
            resolve_package_name: None,
            target_user_id: USER_CURRENT,
            system: false,
            no_resource_id: false,
            icon_resource_id: 0,
            handle_all_web_data_uri: false,
            auto_resolution_allowed: false,
            is_instant_app_available: false,
            user_handle: USER_CURRENT,
        }
    }

    /// `getComponentInfo`'s package and class.
    pub fn component(&self) -> (&str, &str) {
        let c = match &self.info {
            Info::Activity(a) => &a.info,
            Info::Service(s) => &s.info,
            Info::Provider(p) => &p.info,
        };
        (
            c.item.package_name.as_deref().unwrap_or_default(),
            c.item.name.as_deref().unwrap_or_default(),
        )
    }
}

/// `ComponentResolver.RESOLVE_PRIORITY_SORTER`, for a stable sort.
pub fn resolve_priority_order(a: &ResolveInfo, b: &ResolveInfo) -> Ordering {
    b.priority
        .cmp(&a.priority)
        .then(b.preferred_order.cmp(&a.preferred_order))
        .then(b.is_default.cmp(&a.is_default))
        .then(b.match_.cmp(&a.match_))
        .then(b.system.cmp(&a.system))
        .then_with(|| utf16_cmp(a.component().0, b.component().0))
}

/// `String.compareTo`: by UTF-16 unit.
fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[derive(Clone, Debug, Default)]
pub struct ComponentResolver {
    pub activities: IntentResolver<FilterEntry>,
    pub receivers: IntentResolver<FilterEntry>,
    pub services: IntentResolver<FilterEntry>,
    pub providers: IntentResolver<FilterEntry>,
    /// The first package to claim an authority keeps it.
    providers_by_authority: HashMap<String, Registered>,
}

/// A provider `mProvidersByAuthority` holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Registered {
    pub package: String,
    /// The provider's index in the package.
    pub index: usize,
    /// For the copy registration makes of a syncable provider with
    /// several authorities: the copy's authorities (it is not syncable).
    pub copy: Option<String>,
}

/// `String.split(";")`: trailing empty names dropped.
fn authorities(s: &str) -> Vec<&str> {
    let mut names: Vec<&str> = s.split(';').collect();
    while names.last() == Some(&"") {
        names.pop();
    }
    names
}

/// A component's main part.
fn main_of(pkg: &AndroidPackage, kind: Kind, index: usize) -> &MainComponent {
    match kind {
        Kind::Activity => &pkg.activities[index].main,
        Kind::Receiver => &pkg.receivers[index].main,
        Kind::Service => &pkg.services[index].main,
        Kind::Provider => &pkg.providers[index].main,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MimeGroupError {
    MissingGroup,
    NullType,
    UriMatching(super::domain_verification::uri_parcel::MatchError),
}

impl MimeGroupError {
    pub fn reply(self) -> aim_binder_host::parcel::Result<Parcel> {
        let mut reply = Parcel::new();
        use aim_binder_host::parcel::{EX_NULL_POINTER, UNKNOWN_TRANSACTION, Exception};
        let exception = match self {
            Self::MissingGroup => Exception::new(EX_NULL_POINTER, "missing MIME group during component registration"),
            Self::NullType => Exception::new(EX_NULL_POINTER, "null MIME type during component registration"),
            Self::UriMatching(error) => error.binder_exception().ok_or(UNKNOWN_TRANSACTION)?,
        };
        reply.write_exception(&exception);
        Ok(reply)
    }
}

impl ComponentResolver {
    /// `addAllComponents` of every package with code.
    pub fn new(state: &State) -> std::result::Result<ComponentResolver, MimeGroupError> {
        let mut r = ComponentResolver::default();
        for ps in state.packages.values() {
            let Some(pkg) = ps.pkg.as_deref() else {
                continue;
            };
            for (kind, count) in [
                (Kind::Activity, pkg.activities.len()),
                (Kind::Receiver, pkg.receivers.len()),
                (Kind::Provider, pkg.providers.len()),
                (Kind::Service, pkg.services.len()),
            ] {
                for component in 0..count {
                    r.add(state, ps, pkg, kind, component)?;
                }
            }
            for (i, p) in pkg.providers.iter().enumerate() {
                for name in p.authority.iter().flat_map(|a| authorities(a)) {
                    r.providers_by_authority
                        .entry(name.to_owned())
                        .or_insert_with(|| Registered {
                            package: ps.name.clone(),
                            index: i,
                            copy: None,
                        });
                }
            }
        }
        for ps in state.packages.values() {
            if let Some(pkg) = ps.pkg.as_deref() {
                r.add_copies(ps, pkg);
            }
        }
        Ok(r)
    }

    /// `addProvidersLocked`'s copies: a syncable provider keeps its first
    /// authority (the fed package has what registration left it) and a
    /// copy that is not syncable takes the declared others that are
    /// free, after the authorities of the fed packages.
    fn add_copies(&mut self, ps: &PackageState, pkg: &AndroidPackage) {
        for (index, p) in pkg.providers.iter().enumerate() {
            let declared = ps
                .syncable_authorities
                .iter()
                .find(|(name, _)| *name == p.main.component.name);
            let Some((_, declared)) = declared.filter(|_| p.syncable) else {
                continue;
            };
            let mut names: Vec<&str> = p.authority.iter().map(String::as_str).collect();
            for name in authorities(declared).into_iter().skip(1) {
                if !self.providers_by_authority.contains_key(name) && !names.contains(&name) {
                    names.push(name);
                }
            }
            let copy = names.join(";");
            for name in names.into_iter().skip(p.authority.is_some() as usize) {
                self.providers_by_authority.insert(
                    name.to_owned(),
                    Registered {
                        package: ps.name.clone(),
                        index,
                        copy: Some(copy.clone()),
                    },
                );
            }
        }
    }

    fn add(
        &mut self,
        state: &State,
        ps: &PackageState,
        pkg: &AndroidPackage,
        kind: Kind,
        component: usize,
    ) -> std::result::Result<(), MimeGroupError> {
        let main = main_of(pkg, kind, component);
        for (intent, info) in main.component.intents.iter().enumerate() {
            let mut filter = info.filter.clone();
            if kind==Kind::Activity {
                filter.priority=priority::adjust(ps,state.disabled_system_packages.get(&ps.name),
                    &pkg.activities[component],&filter,state.system.roles.as_ref().and_then(|owner|owner.setup_wizard_priority_owner()));
            }
            for group in filter.mime_groups.clone().iter().flatten().rev() {
                let (_, types) = ps
                    .mime_groups
                    .iter()
                    .find(|(g, _)| g.as_deref() == Some(group.as_str()))
                    .ok_or(MimeGroupError::MissingGroup)?;
                for ty in types {
                    let ty = ty.as_deref().ok_or(MimeGroupError::NullType)?;
                    // A malformed type is skipped, as the original skips it.
                    let _ = filter.add_dynamic_data_type(ty);
                }
            }
            let entry = FilterEntry {
                kind,
                package: ps.name.clone(),
                component,
                intent,
                filter,
            };
            match kind {
                Kind::Activity => self.activities.add(entry),
                Kind::Receiver => self.receivers.add(entry),
                Kind::Service => self.services.add(entry),
                Kind::Provider => self.providers.add(entry),
            }
        }
        Ok(())
    }

    /// `mProvidersByAuthority.get`.
    pub fn provider_by_authority(&self, authority: &str) -> Option<&Registered> {
        self.providers_by_authority.get(authority)
    }
}

/// `newResult` of the four resolvers, and `isFilterStopped`.
pub struct Results<'a> {
    pub state: &'a State,
    pub user: i32,
    pub flags: i64,
}

impl Results<'_> {
    /// `MimeGroupsAwareIntentResolver.isFilterStopped`.
    fn is_stopped(&self, e: &FilterEntry) -> bool {
        if !self.state.users.contains_key(&self.user) {
            return true;
        }
        let Some(ps) = self.state.packages.get(&e.package) else {
            return false;
        };
        if ps.pkg.is_none() {
            return false;
        }
        let stopped = user_state(ps, self.user).stopped;
        if ps.is.system {
            // A system app is stopped only if it was scanned stopped.
            return ps.is.scanned_as_stopped_system_app && stopped;
        }
        stopped
    }

    /// The common part of `newResult`: the package, its user state, and
    /// whether the component is enabled and matches the flags.
    fn target(
        &self,
        e: &FilterEntry,
    ) -> Option<(&PackageState, &AndroidPackage, PackageUserState)> {
        if !self.state.users.contains_key(&self.user) {
            return None;
        }
        let ps = self.state.packages.get(&e.package)?;
        let pkg = ps.pkg.as_deref()?;
        let main = main_of(pkg, e.kind, e.component);
        if !is_enabled_and_matches(ps, main, self.flags, self.user) {
            return None;
        }
        Some((ps, pkg, user_state(ps, self.user)))
    }

    fn target_of<'b>(
        &'b self,
        ps: &'b PackageState,
        pkg: &'b AndroidPackage,
        us: &'b PackageUserState,
    ) -> Target<'b> {
        Target {
            sys: &self.state.system,
            pkg,
            ps,
            state: us,
            user: self.user,
        }
    }

    /// The instant-app checks every `newResult` makes after its own.
    fn instant_ok(&self, ps: &PackageState, us: &PackageUserState, visible: bool) -> bool {
        let visible_only = self.flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0;
        if visible_only && !(visible || us.instant_app) {
            return false;
        }
        if self.flags & MATCH_INSTANT == 0 && us.instant_app {
            return false;
        }
        !(us.instant_app && ps.is.update_available)
    }

    /// The fields every `newResult` sets from the filter.
    fn fill(
        &self,
        mut r: ResolveInfo,
        e: &FilterEntry,
        pkg: &AndroidPackage,
        matched: i32,
    ) -> ResolveInfo {
        let info = &main_of(pkg, e.kind, e.component).component.intents[e.intent];
        if self.flags & GET_RESOLVED_FILTER != 0 {
            r.filter = Some(e.filter.clone());
        }
        r.priority = e.filter.priority;
        r.match_ = matched;
        r.is_default = info.has_default;
        r.label_res = info.label_res;
        r.non_localized_label = info.non_localized_label.clone();
        r.icon = info.icon;
        r
    }

    fn activity(&self, e: &FilterEntry, matched: i32) -> Option<ResolveInfo> {
        let (ps, pkg, us) = self.target(e)?;
        let activity = match e.kind {
            Kind::Receiver => &pkg.receivers[e.component],
            _ => &pkg.activities[e.component],
        };
        let ai = generate_activity_info(&self.target_of(ps, pkg, &us), activity, self.flags, None)?;
        let filter = &e.filter;
        let visible = filter.is_visible_to_instant_app()
            && (self.flags & MATCH_EXPLICITLY_VISIBLE_ONLY == 0
                || filter.is_explicitly_visible_to_instant_app());
        if !self.instant_ok(ps, &us, visible) {
            return None;
        }
        let system = ai.info.application_info.flags & FLAG_SYSTEM != 0;
        let mut r = self.fill(ResolveInfo::new(Info::Activity(ai)), e, pkg, matched);
        r.auto_resolution_allowed = filter.has_category(CATEGORY_BROWSABLE);
        r.handle_all_web_data_uri = filter.handle_all_web_data_uri();
        r.icon_resource_id = r.icon;
        // A managed profile's results are badged: no icon of their own
        // (UserNeedsBadgingCache).
        if self
            .state
            .users
            .get(&self.user)
            .is_some_and(|u| u.flags & FLAG_MANAGED_PROFILE != 0)
        {
            r.no_resource_id = true;
            r.icon = 0;
        }
        r.system = system;
        r.is_instant_app_available = us.instant_app;
        r.user_handle = self.user;
        Some(r)
    }

    fn service(&self, e: &FilterEntry, matched: i32) -> Option<ResolveInfo> {
        let (ps, pkg, us) = self.target(e)?;
        let si = generate_service_info(
            &self.target_of(ps, pkg, &us),
            &pkg.services[e.component],
            self.flags,
            None,
        )?;
        if !self.instant_ok(ps, &us, e.filter.is_visible_to_instant_app()) {
            return None;
        }
        let system = si.info.application_info.flags & FLAG_SYSTEM != 0;
        let mut r = self.fill(ResolveInfo::new(Info::Service(si)), e, pkg, matched);
        r.system = system;
        Some(r)
    }

    fn provider(&self, e: &FilterEntry, matched: i32) -> Option<ResolveInfo> {
        let (ps, pkg, us) = self.target(e)?;
        if !self.instant_ok(ps, &us, e.filter.is_visible_to_instant_app()) {
            return None;
        }
        let t = self.target_of(ps, pkg, &us);
        let app = generate_application_info(&t, self.flags)?;
        let pi = generate_provider_info(
            &t,
            &pkg.providers[e.component],
            self.flags,
            Some(Arc::new(app)),
        )?;
        let system = pi.info.application_info.flags & FLAG_SYSTEM != 0;
        let mut r = self.fill(ResolveInfo::new(Info::Provider(pi)), e, pkg, matched);
        r.system = system;
        Some(r)
    }
}

impl Build<FilterEntry, ResolveInfo> for Results<'_> {
    fn stopped(&mut self, e: &FilterEntry) -> bool {
        self.is_stopped(e)
    }

    /// `allowFilterResult`: a component once.
    fn allow(&mut self, e: &FilterEntry, dest: &[ResolveInfo]) -> bool {
        let Some(pkg) = self
            .state
            .packages
            .get(&e.package)
            .and_then(|p| p.pkg.as_deref())
        else {
            return true;
        };
        let name = &main_of(pkg, e.kind, e.component).component.name;
        !dest
            .iter()
            .rev()
            .any(|r| r.component() == (e.package.as_str(), name.as_str()))
    }

    fn result(&mut self, e: &FilterEntry, matched: i32) -> Option<ResolveInfo> {
        match e.kind {
            Kind::Activity | Kind::Receiver => self.activity(e, matched),
            Kind::Service => self.service(e, matched),
            Kind::Provider => self.provider(e, matched),
        }
    }
}

impl ComponentResolver {
    fn resolver(&self, kind: Kind) -> &IntentResolver<FilterEntry> {
        match kind {
            Kind::Activity => &self.activities,
            Kind::Receiver => &self.receivers,
            Kind::Service => &self.services,
            Kind::Provider => &self.providers,
        }
    }

    /// `queryActivities`, `queryReceivers`, `queryServices`,
    /// `queryProviders`: every package's components, or with `package`,
    /// only its (`queryIntentForPackage`). Sorted.
    pub fn query(
        &self,
        kind: Kind,
        mut results: Results<'_>,
        intent: &Intent,
        resolved_type: Option<&str>,
        package: Option<&str>,
    ) -> std::result::Result<Option<Vec<ResolveInfo>>, super::domain_verification::uri_parcel::MatchError> {
        if !results.state.users.contains_key(&results.user) {
            return Ok(None);
        }
        let default_only = results.flags & MATCH_DEFAULT_ONLY != 0;
        let resolver = self.resolver(kind);
        let mut list = match package {
            None => resolver.query(intent, resolved_type, default_only, &mut results),
            Some(package) => {
                // The package's components in manifest order, each with
                // its filters in order.
                let mut lists: Vec<Vec<&FilterEntry>> = Vec::new();
                for e in resolver.entries().iter().filter(|e| e.package == package) {
                    match lists.last_mut() {
                        Some(last) if last[0].component == e.component => last.push(e),
                        _ => lists.push(vec![e]),
                    }
                }
                query_from_list(lists, intent, resolved_type, default_only, &mut results)
            }
        }?;
        list.sort_by(resolve_priority_order);
        Ok(Some(list))
    }
}
