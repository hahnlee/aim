//! Package visibility (`<queries>`, Android 11's app enumeration
//! filtering): `AppsFilterImpl`'s relations between app ids and
//! `ComputerEngine`'s `shouldFilterApplication`, which every query of a
//! package or component passes through (`android-16.0.0_r1`).
//!
//! The original grows its relations as packages are added and settles
//! them once boot completes (`onSystemReady` recomputes the component
//! queries with every protected broadcast known); this model computes
//! the settled relations from a whole state. Implicit grants (an app
//! that started, bound or was sent to another, `grantImplicitAccess`)
//! are runtime state and are not in the original feed. Native interaction
//! grants live in the snapshot's ImplicitAccess;
//! connecting ActivityManager/WindowManager producers is tracked in #724.

use std::collections::{HashMap, HashSet};

use super::model::{PackageState, SharedUser, State};
use super::pkg::{AndroidPackage, MainComponent, booleans};
use super::uri::Uri;

mod implicit;
pub use implicit::ImplicitAccess;

/// A path of the original that depends on state the model does not
/// have; the query is reported as not modelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotModelled(pub &'static str);

pub type Result<T> = std::result::Result<T, NotModelled>;

/// `Process` and `UserHandle`'s uid ranges.
pub const ROOT_UID: i32 = 0;
pub const SYSTEM_UID: i32 = 1000;
pub const SHELL_UID: i32 = 2000;
pub const FIRST_APPLICATION_UID: i32 = 10000;
const FIRST_SDK_SANDBOX_UID: i32 = 20000;
const LAST_SDK_SANDBOX_UID: i32 = 29999;
const FIRST_APP_ZYGOTE_ISOLATED_UID: i32 = 90000;
const LAST_ISOLATED_UID: i32 = 99999;
const PER_USER_RANGE: i32 = 100000;

pub fn app_id(uid: i32) -> i32 {
    uid % PER_USER_RANGE
}

pub fn user_id(uid: i32) -> i32 {
    uid / PER_USER_RANGE
}

/// `UserHandle.getUid`.
pub fn uid(user: i32, app_id: i32) -> i32 {
    user * PER_USER_RANGE + app_id % PER_USER_RANGE
}

/// `Process.isIsolated`: an isolated or app zygote's isolated process.
pub fn is_isolated(uid: i32) -> bool {
    (FIRST_APP_ZYGOTE_ISOLATED_UID..=LAST_ISOLATED_UID).contains(&app_id(uid))
}

/// `Process.isSdkSandboxUid`.
pub fn is_sdk_sandbox(uid: i32) -> bool {
    (FIRST_SDK_SANDBOX_UID..=LAST_SDK_SANDBOX_UID).contains(&app_id(uid))
}

/// `PackageManagerServiceUtils.isSystemOrRootOrShell`.
pub fn is_system_or_root_or_shell(uid: i32) -> bool {
    matches!(app_id(uid), ROOT_UID | SYSTEM_UID | SHELL_UID)
}

/// `PackageManager.FILTER_APPLICATION_QUERY`: on from Android 11 (R),
/// unless platform compat overrides it for the package.
const FILTER_APPLICATION_QUERY_SINCE_SDK: i32 = 30;
const QUERY_ALL_PACKAGES: &str = "android.permission.QUERY_ALL_PACKAGES";

/// What the apps filter reads from framework-res.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// `config_forceSystemPackagesQueryable`.
    pub force_system_packages_queryable: bool,
    /// `config_forceQueryablePackages`, unless all system packages are.
    pub force_queryable_packages: Vec<String>,
}

/// `mSettings.getSettingBase(appId)`: a package, or a shared user.
#[derive(Clone, Copy)]
pub enum Setting<'a> {
    Package(&'a PackageState),
    Shared(&'a SharedUser),
}

/// The setting of an app id.
pub fn setting(state: &State, app_id: i32) -> Option<Setting<'_>> {
    if let Some(su) = state.shared_users.values().find(|s| s.app_id == app_id) {
        return Some(Setting::Shared(su));
    }
    state
        .packages
        .values()
        .find(|p| p.app_id == app_id)
        .map(Setting::Package)
}

/// A shared user's packages.
fn shared_packages<'a>(
    state: &'a State,
    su: &'a SharedUser,
) -> impl Iterator<Item = &'a PackageState> {
    su.packages
        .iter()
        .filter_map(|name| state.packages.get(name))
}

/// `AppsFilterImpl`'s settled relations, by app id.
#[derive(Clone, Debug, Default)]
pub struct AppsFilter {
    force_queryable: HashSet<i32>,
    queries_via_package: HashSet<(i32, i32)>,
    queries_via_component: HashSet<(i32, i32)>,
    queryable_via_uses_library: HashSet<(i32, i32)>,
    queryable_via_uses_permission: HashSet<(i32, i32)>,
    /// Packages that target Android 10 or lower (`FeatureConfig`'s
    /// disabled packages).
    disabled: HashSet<String>,
    /// `OverlayReferenceMapper`'s actor packages and the targets and
    /// overlays they act on.
    actors: HashMap<String, HashSet<String>>,
}

fn pkg(ps: &PackageState) -> Option<&AndroidPackage> {
    ps.pkg.as_deref()
}

/// `AppsFilterUtils.requestsQueryAllPackages`.
fn requests_query_all_packages(p: &AndroidPackage) -> bool {
    p.requested_permissions
        .iter()
        .any(|r| r == QUERY_ALL_PACKAGES)
}

/// `AppsFilterUtils.canQueryViaComponents`.
fn can_query_via_components(
    querying: &AndroidPackage,
    target: &AndroidPackage,
    protected: &HashSet<&str>,
) -> bool {
    querying.queries_intents.iter().any(|intent| {
        let matches = |c: &MainComponent, protected: Option<&HashSet<&str>>| {
            let ignored: Option<Vec<&str>> = protected.map(|p| p.iter().copied().collect());
            c.exported
                && c.component.intents.iter().rev().any(|info| {
                    info.filter.matches(
                        intent.action.as_deref(),
                        intent.ty.as_deref(),
                        intent.scheme(),
                        intent.data.as_ref(),
                        intent.categories.as_deref(),
                        true,
                        ignored.as_deref(),
                    ) > 0
                })
        };
        target.services.iter().rev().any(|s| matches(&s.main, None))
            || target
                .activities
                .iter()
                .rev()
                .any(|a| matches(&a.main, None))
            || target
                .receivers
                .iter()
                .rev()
                .any(|r| matches(&r.main, Some(protected)))
            || target
                .providers
                .iter()
                .rev()
                .any(|p| matches(&p.main, None))
    }) || (!querying.queries_providers.is_empty()
        && target.providers.iter().any(|p| {
            p.main.exported
                && p.authority.as_deref().is_some_and(|a| {
                    a.split(';')
                        .filter(|s| !s.is_empty())
                        .any(|a| querying.queries_providers.iter().any(|q| q == a))
                })
        }))
}

/// `canQueryViaPackage`, `canQueryAsInstaller`, `canQueryAsUpdateOwner`.
fn can_query_via_package(
    querying: &PackageState,
    q: &AndroidPackage,
    target: &AndroidPackage,
) -> bool {
    let name = Some(target.package_name.as_str());
    let source = &querying.install_source;
    q.queries_packages.iter().any(|p| p == &target.package_name)
        || source.installer.as_deref() == name
        || (!source.initiating_package_uninstalled && source.initiating_package.as_deref() == name)
        || source.update_owner.as_deref() == name
}

/// `canQueryViaUsesLibrary`.
fn can_query_via_uses_library(querying: &AndroidPackage, target: &AndroidPackage) -> bool {
    target.library_names.iter().any(|lib| {
        querying.uses_libraries.contains(lib) || querying.uses_optional_libraries.contains(lib)
    })
}

/// `pkgInstruments`.
fn instruments(source: &AndroidPackage, target: &AndroidPackage) -> bool {
    source
        .instrumentations
        .iter()
        .any(|i| i.target_package.as_deref() == Some(target.package_name.as_str()))
}

/// `OverlayActorEnforcer.getPackageNameForActor`: the package a named
/// actor (`overlay://namespace/name`) stands for.
fn actor_package<'a>(actor: &str, named: &'a [(String, String, String)]) -> Option<&'a str> {
    let uri = Uri::parse(actor);
    let segments = uri.path_segments();
    if uri.scheme() != Some("overlay") || segments.len() != 1 {
        return None;
    }
    let namespace = uri.authority()?;
    named
        .iter()
        .find(|(ns, name, _)| *ns == namespace && *name == segments[0])
        .map(|(_, _, package)| package.as_str())
}

/// `String.hashCode`, which orders an `ArrayMap`'s keys.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// `OverlayReferenceMapper`'s rebuilt map: each overlayable's actor acts
/// on its target and the overlays of that overlayable. An actor package
/// named by several actors keeps the last actor's packages in the
/// original's `ArrayMap` order.
fn overlay_actors(
    packages: &[(&PackageState, &AndroidPackage)],
    named: &[(String, String, String)],
) -> HashMap<String, HashSet<String>> {
    let mut by_actor: Vec<(&str, HashSet<String>)> = Vec::new();
    for (_, target) in packages {
        for (overlayable, actor) in target.overlayables.iter().flatten() {
            let Some(actor) = actor.as_deref() else {
                continue;
            };
            let i = match by_actor.iter().position(|(a, _)| *a == actor) {
                Some(i) => i,
                None => {
                    by_actor.push((actor, HashSet::new()));
                    by_actor.len() - 1
                }
            };
            let acted = &mut by_actor[i].1;
            acted.insert(target.package_name.clone());
            acted.extend(
                packages
                    .iter()
                    .filter(|(_, o)| {
                        o.overlay_target.as_deref() == Some(target.package_name.as_str())
                            && o.overlay_target_overlayable_name.as_deref()
                                == Some(overlayable.as_str())
                    })
                    .map(|(_, o)| o.package_name.clone()),
            );
        }
    }
    by_actor.sort_by_key(|(a, _)| java_hash(a));
    by_actor
        .into_iter()
        .filter_map(|(a, acted)| Some((actor_package(a, named)?.to_owned(), acted)))
        .collect()
}

/// `SigningDetails.signaturesMatchExactly`: the same set of signers.
fn signatures_match_exactly(a: &PackageState, b: &PackageState) -> bool {
    let (Some(a), Some(b)) = (&a.signatures, &b.signatures) else {
        return false;
    };
    a.signatures.len() == b.signatures.len()
        && a.signatures.iter().all(|s| b.signatures.contains(s))
        && b.signatures.iter().all(|s| a.signatures.contains(s))
}

impl AppsFilter {
    /// The relations of every package in `state`, as `addPackage` of each
    /// and the recomputation at boot completion leave them.
    pub fn new(state: &State, config: &Config) -> AppsFilter {
        let mut f = AppsFilter::default();
        let packages: Vec<(&PackageState, &AndroidPackage)> = state
            .packages
            .values()
            .filter_map(|ps| pkg(ps).map(|p| (ps, p)))
            .collect();
        let protected: HashSet<&str> = packages
            .iter()
            .flat_map(|(_, p)| p.protected_broadcasts.iter().map(String::as_str))
            .collect();
        let android = state.packages.get("android");
        let forced_by_device: &[String] = if config.force_system_packages_queryable {
            &[]
        } else {
            &config.force_queryable_packages
        };
        for ps in state.packages.values() {
            let system_signed =
                ps.is.system && android.is_some_and(|a| signatures_match_exactly(ps, a));
            let forced = ps.is.force_queryable_override
                || (ps.is.system
                    && pkg(ps).is_some_and(|p| {
                        config.force_system_packages_queryable
                            || p.booleans & booleans::FORCE_QUERYABLE != 0
                            || forced_by_device.contains(&p.package_name)
                    }));
            if system_signed || forced {
                f.force_queryable.insert(ps.app_id);
            }
            let filtered = ps
                .filter_application_query
                .unwrap_or(ps.target_sdk_version >= FILTER_APPLICATION_QUERY_SINCE_SDK);
            if pkg(ps).is_some() && !filtered {
                f.disabled.insert(ps.name.clone());
            }
        }
        f.actors = overlay_actors(&packages, &state.system.named_actors);
        for &(qs, q) in &packages {
            for &(ts, t) in &packages {
                if qs.app_id == ts.app_id {
                    continue;
                }
                let pair = (qs.app_id, ts.app_id);
                if !f.force_queryable.contains(&ts.app_id) {
                    if !requests_query_all_packages(q) && can_query_via_components(q, t, &protected)
                    {
                        f.queries_via_component.insert(pair);
                    }
                    if can_query_via_package(qs, q, t) {
                        f.queries_via_package.insert(pair);
                    }
                    if can_query_via_uses_library(q, t) {
                        f.queryable_via_uses_library.insert(pair);
                    }
                }
                if instruments(q, t) || instruments(t, q) {
                    f.queries_via_package.insert(pair);
                }
                let defines = |name: &str| t.permissions.iter().any(|p| p.component.name == name);
                if q.uses_permissions
                    .iter()
                    .any(|u| u.name.as_deref().is_some_and(defines))
                {
                    f.queryable_via_uses_permission.insert(pair);
                }
            }
        }
        f
    }

    /// `AppsFilterBase.shouldFilterApplication`: whether the target is
    /// hidden from the caller.
    pub fn should_filter(
        &self,
        state: &State,
        calling_uid: i32,
        target: &PackageState,
        user: i32,
    ) -> bool {
        let calling_app_id = app_id(calling_uid);
        if calling_app_id < FIRST_APPLICATION_UID
            || target.app_id < FIRST_APPLICATION_UID
            || calling_app_id == target.app_id
        {
            return false;
        }
        if is_sdk_sandbox(calling_app_id) {
            // The sandbox sees force-queryable packages and its client
            // (allow_sdk_sandbox_query_intent_activities is on).
            let target_uid = uid(user, target.app_id);
            return !self.force_queryable.contains(&target.app_id)
                && !state
                    .system
                    .implicit_access
                    .transient(calling_uid, target_uid)
                && target_uid != calling_uid - (FIRST_SDK_SANDBOX_UID - FIRST_APPLICATION_UID);
        }
        self.should_filter_internal(state, calling_uid, target, user)
    }

    /// `shouldFilterApplicationInternal`, which the original's cache holds
    /// for every pair.
    fn should_filter_internal(
        &self,
        state: &State,
        calling_uid: i32,
        target: &PackageState,
        user: i32,
    ) -> bool {
        let calling_app_id = app_id(calling_uid);
        // FeatureConfig's DeviceConfig flag. The original's cache keeps
        // what it computed before the flag changed until something
        // recomputes it; the model follows the flag at once.
        if state.platform.query_filtering_disabled {
            return false;
        }
        let Some(calling) = setting(state, calling_app_id) else {
            return true;
        };
        let callers: Vec<&PackageState> = match calling {
            Setting::Package(ps) => vec![ps],
            Setting::Shared(su) => shared_packages(state, su).collect(),
        };
        let acts_on_target = |t: &AndroidPackage| {
            callers.iter().any(|c| {
                self.actors
                    .get(&c.name)
                    .is_some_and(|acted| acted.contains(&t.package_name))
            })
        };
        let callers: Vec<&AndroidPackage> = callers.iter().copied().filter_map(pkg).collect();
        if callers
            .iter()
            .any(|p| self.disabled.contains(&p.package_name))
        {
            return false;
        }
        if callers.iter().any(|p| requests_query_all_packages(p)) {
            return false;
        }
        let Some(t) = pkg(target) else {
            return true;
        };
        if t.static_shared_library_name.is_some() {
            return false;
        }
        let pair = (calling_app_id, target.app_id);
        !(self.force_queryable.contains(&target.app_id)
            || self.queries_via_package.contains(&pair)
            || self.queries_via_component.contains(&pair)
            || self.queryable_via_uses_library.contains(&pair)
            || self.queryable_via_uses_permission.contains(&pair)
            || state
                .system
                .implicit_access
                .visible(calling_uid, uid(user, target.app_id))
            || acts_on_target(t))
    }
}

/// `ComputerEngine.getIsolatedOwner`.
fn isolated_owner(state: &State, uid: i32) -> Result<i32> {
    state
        .system
        .isolated_owners
        .iter()
        .find(|(isolated, _)| *isolated == uid)
        .map(|(_, owner)| *owner)
        .ok_or(NotModelled("an isolated uid without a known owner"))
}

/// `ComputerEngine.getInstantAppPackageName`: the calling instant app's
/// package, if the caller is one.
pub fn instant_app_package_name(state: &State, mut calling_uid: i32) -> Result<Option<&str>> {
    if is_isolated(calling_uid) {
        calling_uid = isolated_owner(state, calling_uid)?;
    }
    let user = user_id(calling_uid);
    Ok(match setting(state, app_id(calling_uid)) {
        Some(Setting::Package(ps)) => ps
            .users
            .get(&user)
            .is_some_and(|u| u.instant_app)
            .then_some(ps.name.as_str()),
        _ => None,
    })
}

/// `ComputerEngine.isCallerSameApp`.
pub fn is_caller_same_app(state: &State, package: Option<&str>, uid: i32) -> Result<bool> {
    if is_sdk_sandbox(uid) {
        let selected = state
            .system
            .sdk_sandbox_package
            .as_ref()
            .ok_or(NotModelled("the SDK sandbox's package"))?;
        return Ok(package.is_some() && package == selected.as_deref());
    }
    Ok(package
        .and_then(|p| state.packages.get(p))
        .is_some_and(|ps| ps.pkg.is_some() && app_id(uid) == ps.app_id))
}

/// `ComputerEngine.shouldFilterApplication`: whether `ps` is hidden from
/// the caller. With `filter_uninstall`, a package not installed for the
/// user is hidden too, an archived one unless `filter_archived` is off.
pub fn should_filter_application(
    state: &State,
    filter: &AppsFilter,
    ps: Option<&PackageState>,
    mut calling_uid: i32,
    user: i32,
    filter_uninstall: bool,
    filter_archived: bool,
) -> Result<bool> {
    if is_sdk_sandbox(calling_uid) {
        if ps.is_some_and(|target| {
            uid(user, target.app_id)
                == calling_uid - (FIRST_SDK_SANDBOX_UID - FIRST_APPLICATION_UID)
        }) {
            return Ok(false);
        }
        if ps.is_none() {
            return Ok(true);
        }
    }
    if is_isolated(calling_uid) {
        calling_uid = isolated_owner(state, calling_uid)?;
    }
    let caller_is_instant = instant_app_package_name(state, calling_uid)?.is_some();
    let Some(ps) = ps else {
        return Ok(caller_is_instant || filter_uninstall);
    };
    let us = ps.users.get(&user).cloned().unwrap_or_default();
    let archived = us.archive_state.is_some() && !us.installed;
    // Hidden-until-installed packages stay visible: the phone app reads
    // the carrier apps' details.
    if filter_uninstall
        && !is_system_or_root_or_shell(calling_uid)
        && !ps.is.hidden_until_installed
        && !us.installed
        && (!archived || filter_archived)
    {
        return Ok(true);
    }
    if is_caller_same_app(state, Some(&ps.name), calling_uid)? {
        return Ok(false);
    }
    if caller_is_instant || us.instant_app {
        return Err(NotModelled("instant apps' visibility"));
    }
    Ok(filter.should_filter(state, calling_uid, ps, user))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::intent::Intent;
    use super::super::intent_filter::{IntentFilter, ParsedIntentInfo};
    use super::super::model::{PackageUserState, User};
    use super::super::pkg::{Activity, Component, Permission, UsesPermission};
    use super::super::settings::Signatures;
    use super::super::uri::Uri;
    use super::*;

    const VIEW: &str = "android.intent.action.VIEW";

    fn package(
        name: &str,
        app_id: i32,
        edit: impl FnOnce(&mut PackageState, &mut AndroidPackage),
    ) -> PackageState {
        let mut ps = PackageState {
            name: name.into(),
            app_id,
            target_sdk_version: 35,
            ..PackageState::default()
        };
        let mut pkg = AndroidPackage {
            package_name: name.into(),
            ..AndroidPackage::default()
        };
        edit(&mut ps, &mut pkg);
        ps.pkg = Some(Arc::new(pkg));
        ps
    }

    fn signed(ps: &mut PackageState, key: u8) {
        ps.signatures = Some(Signatures {
            current_flags: Vec::new(),
            scheme_version: 3,
            signatures: vec![vec![key]],
            public_keys: None,
            past_signatures: None,
        });
    }

    fn browsable_activity(name: &str, exported: bool) -> Activity {
        let mut filter = IntentFilter::default();
        filter.add_action(VIEW);
        filter.add_category("android.intent.category.BROWSABLE");
        filter.add_data_scheme("https");
        let mut a = Activity::default();
        a.main.exported = exported;
        a.main.component = Component {
            name: name.into(),
            intents: vec![ParsedIntentInfo {
                filter,
                ..ParsedIntentInfo::default()
            }],
            ..Component::default()
        };
        a
    }

    fn state() -> State {
        let packages = [
            package("android", 1000, |ps, _| {
                ps.is.system = true;
                signed(ps, 1);
            }),
            package("a", 10001, |_, p| p.queries_packages = vec!["b".into()]),
            package("b", 10002, |_, _| {}),
            package("c", 10003, |_, p| {
                p.queries_intents = vec![Intent {
                    action: Some(VIEW.into()),
                    data: Some(Uri::parse("https://")),
                    categories: Some(vec!["android.intent.category.BROWSABLE".into()]),
                    ..Intent::default()
                }];
            }),
            package("d", 10004, |_, p| {
                p.activities = vec![browsable_activity("d.A", true)]
            }),
            package("d2", 10014, |_, p| {
                p.activities = vec![browsable_activity("d2.A", false)]
            }),
            package("e", 10005, |_, p| {
                p.requested_permissions = vec![QUERY_ALL_PACKAGES.into()]
            }),
            package("f", 10006, |ps, _| {
                ps.is.system = true;
                signed(ps, 1);
            }),
            package("f2", 10016, |ps, _| signed(ps, 1)),
            package("g", 10007, |ps, _| ps.target_sdk_version = 29),
            package("h", 10008, |_, p| {
                p.permissions = vec![Permission {
                    component: Component {
                        name: "h.P".into(),
                        ..Component::default()
                    },
                    ..Permission::default()
                }];
            }),
            package("i", 10009, |_, p| {
                p.uses_permissions = vec![UsesPermission {
                    name: Some("h.P".into()),
                    flags: 0,
                }];
            }),
            package("j", 10010, |_, p| {
                p.instrumentations = vec![super::super::pkg::Instrumentation {
                    target_package: Some("b".into()),
                    ..Default::default()
                }];
            }),
            package("k", 10011, |ps, _| {
                ps.install_source.installer = Some("b".into())
            }),
            package("s", 10012, |ps, p| {
                ps.is.system = true;
                p.booleans = booleans::FORCE_QUERYABLE;
            }),
            package("settings", 10013, |ps, _| ps.is.system = true),
        ];
        State {
            packages: packages.into_iter().map(|p| (p.name.clone(), p)).collect(),
            users: [(0, User::default())].into(),
            ..State::default()
        }
    }

    fn filtered(f: &AppsFilter, s: &State, caller: i32, target: &str) -> bool {
        f.should_filter(s, caller, &s.packages[target], 0)
    }

    #[test]
    fn visibility() {
        let s = state();
        let config = Config {
            force_system_packages_queryable: false,
            force_queryable_packages: vec!["settings".into()],
        };
        let f = AppsFilter::new(&s, &config);
        // <queries><package>, and not the other way.
        assert!(!filtered(&f, &s, 10001, "b"));
        assert!(filtered(&f, &s, 10002, "a"));
        // <queries><intent> matches exported components only.
        assert!(!filtered(&f, &s, 10003, "d"));
        assert!(filtered(&f, &s, 10003, "d2"));
        // QUERY_ALL_PACKAGES; a target before Android 11 sees all.
        assert!(!filtered(&f, &s, 10005, "a") && !filtered(&f, &s, 10007, "a"));
        // Force queryable: system and signed with the platform key, the
        // manifest's flag, the device's list; a signed app off the system
        // image is not.
        for t in ["f", "s", "settings"] {
            assert!(!filtered(&f, &s, 10002, t), "{t}");
        }
        assert!(filtered(&f, &s, 10002, "f2"));
        // A permission's user sees its definer; instrumentation both ways;
        // an installer is seen by what it installed.
        assert!(!filtered(&f, &s, 10009, "h") && filtered(&f, &s, 10008, "i"));
        assert!(!filtered(&f, &s, 10010, "b") && !filtered(&f, &s, 10002, "j"));
        assert!(!filtered(&f, &s, 10011, "b") && filtered(&f, &s, 10002, "k"));
        // System uids and the same app see everything.
        assert!(!filtered(&f, &s, 1000, "b") && !filtered(&f, &s, 10002, "b"));
        // An app id with no package is filtered.
        assert!(filtered(&f, &s, 10099, "b"));
        let all = Config {
            force_system_packages_queryable: true,
            force_queryable_packages: Vec::new(),
        };
        let f = AppsFilter::new(&s, &all);
        assert!(!filtered(&f, &s, 10002, "settings"));
    }

    #[test]
    fn interaction_grants_are_directional_user_scoped_and_snapshot_owned() {
        let mut s = state();
        let f = AppsFilter::new(&s, &Config::default());
        assert!(filtered(&f, &s, 10002, "a"));
        assert!(!s.system.implicit_access.grant(10002, 10002, false));
        assert!(s.system.implicit_access.grant(10002, 10001, false));
        assert!(!s.system.implicit_access.grant(10002, 10001, false));
        assert!(!filtered(&f, &s, 10002, "a"));
        assert!(f.should_filter(&s, uid(10, 10002), &s.packages["a"], 10));
        assert!(f.should_filter(&s, 10002, &s.packages["a"], 10));
        assert!(s.system.implicit_access.grant(10002, 10001, true));
        let old = s.clone();
        s.system
            .implicit_access
            .replace_package(10001, &[0, 10], false);
        assert!(!filtered(&f, &s, 10002, "a"));
        s.system.implicit_access.remove_package(10001, &[0, 10]);
        assert!(filtered(&f, &s, 10002, "a"));
        assert!(!filtered(&f, &old, 10002, "a"));
        // SDK sandboxes use only ordinary grants, even when retained access
        // makes the same target visible to an ordinary application UID.
        assert!(s.system.implicit_access.grant(20002, 10001, true));
        assert!(filtered(&f, &s, 20002, "a"));
        assert!(s.system.implicit_access.grant(20002, 10001, false));
        assert!(!filtered(&f, &s, 20002, "a"));
    }

    #[test]
    fn interaction_cleanup_removes_both_directions_for_resolved_users() {
        let mut grants = ImplicitAccess::default();
        for user in [0, 10, 11] {
            for retain in [false, true] {
                assert!(grants.grant(uid(user, 10001), uid(user, 10002), retain));
                assert!(grants.grant(uid(user, 10003), uid(user, 10001), retain));
                assert!(grants.grant(uid(user, 10002), uid(user, 10003), retain));
            }
        }
        let original = grants.clone();
        grants.replace_package(10001, &[0, 10], true);
        assert_eq!(grants, original);
        grants.replace_package(10001, &[0, 10], false);
        assert!(!grants.transient(10001, 10002));
        assert!(grants.visible(10001, 10002));
        assert!(!grants.transient(uid(10, 10003), uid(10, 10001)));
        grants.remove_package(10001, &[0, 10]);
        for user in [0, 10] {
            assert!(!grants.visible(uid(user, 10001), uid(user, 10002)));
            assert!(!grants.visible(uid(user, 10003), uid(user, 10001)));
            assert!(grants.visible(uid(user, 10002), uid(user, 10003)));
        }
        assert!(grants.transient(uid(11, 10001), uid(11, 10002)));
        assert!(grants.visible(uid(11, 10003), uid(11, 10001)));
    }

    #[test]
    fn sandbox_same_app_uses_the_selected_owner_without_package_inventory() {
        let mut state = State::default();
        for user in [0, 10] {
            for id in [FIRST_SDK_SANDBOX_UID, LAST_SDK_SANDBOX_UID] {
                let caller = uid(user, id);
                assert_eq!(
                    is_caller_same_app(&state, Some("selected.sdk"), caller),
                    Err(NotModelled("the SDK sandbox's package"))
                );
                state.system.sdk_sandbox_package = Some(None);
                assert_eq!(is_caller_same_app(&state, None, caller), Ok(false));
                assert_eq!(
                    is_caller_same_app(&state, Some("selected.sdk"), caller),
                    Ok(false)
                );
                state.system.sdk_sandbox_package = Some(Some("selected.sdk".into()));
                assert_eq!(
                    is_caller_same_app(&state, Some("selected.sdk"), caller),
                    Ok(true)
                );
                assert_eq!(
                    is_caller_same_app(&state, Some("other.sdk"), caller),
                    Ok(false)
                );
                assert_eq!(is_caller_same_app(&state, None, caller), Ok(false));
                // Ordinary callers still require parsed package ownership.
                assert_eq!(
                    is_caller_same_app(&state, Some("selected.sdk"), uid(user, 10001)),
                    Ok(false)
                );
                state.system.sdk_sandbox_package = None;
            }
        }
    }

    #[test]
    fn sandbox_non_client_visibility_follows_uninstall_same_app_and_owned_grants() {
        for user in [0, 10] {
            let sandbox = uid(user, FIRST_SDK_SANDBOX_UID);
            let mut state = State::default();
            let mut target = PackageState {
                name: "selected.sdk".into(),
                app_id: 10005,
                users: [(
                    user,
                    PackageUserState {
                        installed: true,
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            };
            let filter = AppsFilter::new(&state, &Config::default());
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    sandbox,
                    user,
                    false,
                    true
                ),
                Err(NotModelled("the SDK sandbox's package"))
            );
            state.system.sdk_sandbox_package = Some(Some(target.name.clone()));
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Ok(false)
            );
            // Uninstall/archived filtering precedes even the selected owner's same-app check.
            target.users.get_mut(&user).unwrap().installed = false;
            for uninstall in [false, true] {
                for archived_filter in [false, true] {
                    assert_eq!(
                        should_filter_application(
                            &state,
                            &filter,
                            Some(&target),
                            sandbox,
                            user,
                            uninstall,
                            archived_filter
                        ),
                        Ok(uninstall)
                    );
                    target.users.get_mut(&user).unwrap().archive_state =
                        Some(super::super::restrictions::ArchiveState {
                            installer_title: String::new(),
                            archive_time: 0,
                            activities: vec![],
                        });
                    assert_eq!(
                        should_filter_application(
                            &state,
                            &filter,
                            Some(&target),
                            sandbox,
                            user,
                            uninstall,
                            archived_filter
                        ),
                        Ok(uninstall && archived_filter)
                    );
                    target.users.get_mut(&user).unwrap().archive_state = None;
                }
            }
            target.users.get_mut(&user).unwrap().installed = true;
            // A captured null selection still permits AppsFilter-owned relations.
            state.system.sdk_sandbox_package = Some(None);
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Ok(true)
            );
            state
                .system
                .implicit_access
                .grant(sandbox, uid(user, target.app_id), true);
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Ok(true)
            );
            state
                .system
                .implicit_access
                .grant(sandbox, uid(user, target.app_id), false);
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Ok(false)
            );
            assert_eq!(
                should_filter_application(
                    &state,
                    &filter,
                    Some(&target),
                    uid(user + 1, FIRST_SDK_SANDBOX_UID),
                    user,
                    false,
                    true
                ),
                Ok(true)
            );
            state
                .system
                .implicit_access
                .remove_package(target.app_id, &[user]);
            target.is.force_queryable_override = true;
            state.packages.insert(target.name.clone(), target.clone());
            let forced = AppsFilter::new(&state, &Config::default());
            assert_eq!(
                should_filter_application(
                    &state,
                    &forced,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Ok(false)
            );
            target.users.get_mut(&user).unwrap().instant_app = true;
            assert_eq!(
                should_filter_application(
                    &state,
                    &forced,
                    Some(&target),
                    sandbox,
                    user,
                    true,
                    true
                ),
                Err(NotModelled("instant apps' visibility"))
            );
        }
    }

    #[test]
    fn sandbox_clients_are_visible_before_code_user_and_instant_checks() {
        let state = State::default();
        let filter = AppsFilter::new(&state, &Config::default());
        for user in [0, 10] {
            for client_id in [FIRST_APPLICATION_UID, FIRST_SDK_SANDBOX_UID - 1] {
                let sandbox = uid(
                    user,
                    client_id + FIRST_SDK_SANDBOX_UID - FIRST_APPLICATION_UID,
                );
                let client = PackageState {
                    name: "sandbox.client".into(),
                    app_id: client_id,
                    users: [(
                        user,
                        PackageUserState {
                            installed: false,
                            instant_app: true,
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                };
                for uninstall in [false, true] {
                    for archived in [false, true] {
                        assert_eq!(
                            should_filter_application(
                                &state,
                                &filter,
                                Some(&client),
                                sandbox,
                                user,
                                uninstall,
                                archived
                            ),
                            Ok(false)
                        );
                        assert_eq!(
                            should_filter_application(
                                &state, &filter, None, sandbox, user, uninstall, archived
                            ),
                            Ok(true)
                        );
                    }
                }
                assert_eq!(
                    should_filter_application(
                        &state,
                        &filter,
                        Some(&client),
                        sandbox,
                        user + 1,
                        false,
                        true
                    ),
                    Err(NotModelled("the SDK sandbox's package"))
                );
                let mut other = client.clone();
                other.app_id += 1;
                assert_eq!(
                    should_filter_application(
                        &state,
                        &filter,
                        Some(&other),
                        sandbox,
                        user,
                        false,
                        true
                    ),
                    Err(NotModelled("the SDK sandbox's package"))
                );
            }
        }
    }

    #[test]
    fn uids() {
        assert_eq!((app_id(1_010_001), user_id(1_010_001)), (10001, 10));
        assert_eq!(uid(10, 10001), 1_010_001);
        assert!(is_isolated(99_005) && is_isolated(1_090_000) && !is_isolated(10001));
        assert!(is_sdk_sandbox(20_001) && !is_sdk_sandbox(10001));
    }
}
