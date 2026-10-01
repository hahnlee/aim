//! Installs, updates and removals, found as the original's state changes
//! from one observed state to the next: `pm install` and `pm uninstall`
//! run inside system_server and never cross binder, and an install
//! session's commit ends on PackageManager's own threads. What the model
//! makes of the change, from the state before it and the package the
//! original parsed, is compared with what the original made of it, up to
//! the values docs/first-boot.md lists as the device's own (times and
//! code path names).
//!
//! As `Settings.createNewSetting`, `updatePackageSetting`,
//! `removePackageAndAppIdLPw` and `InstallPackageHelper.
//! updateSettingsInternalLI` do at `android-16.0.0_r1`. The install
//! session's parameters (its user, installer, install reason, flags) are
//! not fed (#759), so what they decide is left out of the comparison:
//! the users' install reasons and the installer recorded as the last
//! enabler. A removal is taken as one without `DELETE_KEEP_DATA`.

use std::collections::BTreeSet;

use super::session::Session;
use crate::package::apps_filter::{FIRST_APPLICATION_UID, NotModelled};
use crate::package::model::{PackageState, State};
use crate::package::pkg::AndroidPackage;
use crate::shadow::Value;

/// What happened to a package between two states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Install,
    Update,
    Removal,
    /// An updated system package's update removed: the system package is
    /// the package again.
    UpdateRemoval,
    /// A package uninstalled for some users, kept for the others or as a
    /// system package.
    UserRemoval,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Install => "install",
            Kind::Update => "update",
            Kind::Removal => "removal",
            Kind::UpdateRemoval => "update removal",
            Kind::UserRemoval => "user removal",
        }
    }
}

/// The packages installed, updated or removed from `pre` to `post`.
pub(super) fn changes(pre: &State, post: &State) -> Vec<(String, Kind)> {
    let names: BTreeSet<&String> = pre.packages.keys().chain(post.packages.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let kind = match (pre.packages.get(name), post.packages.get(name)) {
                (None, Some(_)) => Kind::Install,
                (Some(_), None) => Kind::Removal,
                (Some(a), Some(b)) if a.path != b.path => {
                    let restored = pre
                        .disabled_system_packages
                        .get(name)
                        .is_some_and(|d| d.path == b.path);
                    if restored {
                        Kind::UpdateRemoval
                    } else {
                        Kind::Update
                    }
                }
                (Some(a), Some(b)) if uninstalled(a, b) => Kind::UserRemoval,
                _ => return None,
            };
            Some((name.clone(), kind))
        })
        .collect()
}

/// Whether a user that had the package installed no longer has.
fn uninstalled(pre: &PackageState, post: &PackageState) -> bool {
    pre.users
        .iter()
        .any(|(id, u)| u.installed && post.users.get(id).is_some_and(|v| !v.installed))
}

/// The part of a package's state a change decides, as compared.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct View {
    pub app_id: i32,
    pub shared_user: Option<String>,
    /// The code path, its random names masked.
    pub path: String,
    pub version_code: i64,
    pub target_sdk_version: i32,
    pub system: bool,
    pub updated_system_app: bool,
    /// The system package an update replaces: its code path.
    pub disabled_system_path: Option<String>,
    pub users: Vec<(i32, UserView)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct UserView {
    pub installed: bool,
    pub stopped: bool,
    pub not_launched: bool,
    pub hidden: bool,
    pub enabled: i32,
    pub enabled_components: Vec<String>,
    pub disabled_components: Vec<String>,
    pub uninstall_reason: i32,
    /// Whether there is a first install time (its value is the clock's).
    pub first_install_time: bool,
    pub suspended: bool,
    pub distraction_flags: i32,
    pub instant_app: bool,
    pub virtual_preload: bool,
    /// The install reason and the last enabler, which the install
    /// session decides: `None` when the model has not got it (#759).
    pub install_reason: Option<i32>,
    pub last_disable_app_caller: Option<Option<String>>,
}

/// A shared user as compared: its app id and packages.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SharedView {
    pub name: String,
    pub app_id: i32,
    pub packages: BTreeSet<String>,
}

/// Everything a change decides: the package, its shared user, and the
/// install sources of the packages that name it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Outcome {
    pub package: Option<View>,
    pub shared_user: Option<SharedView>,
    /// Packages whose installer, originating package or update owner was
    /// the removed package: those three of each.
    pub sources: Vec<(String, [Option<String>; 3])>,
}

/// `/data/app/~~<random>/<package>-<random>` with each random name
/// masked (`PackageManagerServiceUtils.getNextCodePath`).
fn mask(path: &str, package: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["", "data", "app", dir, leaf]
            if dir.starts_with("~~") && leaf.starts_with(&format!("{package}-")) =>
        {
            format!("/data/app/~~*/{package}-*")
        }
        _ => path.to_string(),
    }
}

/// Each user's view; with `session`, the session's fields too.
fn user_view(ps: &PackageState, session: bool) -> Vec<(i32, UserView)> {
    ps.users
        .iter()
        .map(|(&id, u)| {
            (
                id,
                UserView {
                    installed: u.installed,
                    stopped: u.stopped,
                    not_launched: u.not_launched,
                    hidden: u.hidden,
                    enabled: u.enabled,
                    enabled_components: u.enabled_components.clone(),
                    disabled_components: u.disabled_components.clone(),
                    uninstall_reason: u.uninstall_reason,
                    first_install_time: u.first_install_time != 0,
                    suspended: !u.suspended_by.is_empty(),
                    distraction_flags: u.distraction_flags,
                    instant_app: u.instant_app,
                    virtual_preload: u.virtual_preload,
                    install_reason: session.then_some(u.install_reason),
                    last_disable_app_caller: session.then(|| u.last_disable_app_caller.clone()),
                },
            )
        })
        .collect()
}

fn view(state: &State, name: &str, session: bool) -> Option<View> {
    let ps = state.packages.get(name)?;
    Some(View {
        app_id: ps.app_id,
        shared_user: ps.shared_user.clone(),
        path: mask(&ps.path, name),
        version_code: ps.version_code,
        target_sdk_version: ps.target_sdk_version,
        system: ps.is.system,
        updated_system_app: ps.is.updated_system_app,
        disabled_system_path: state
            .disabled_system_packages
            .get(name)
            .map(|d| d.path.clone()),
        users: user_view(ps, session),
    })
}

fn shared_view(state: &State, name: Option<&str>) -> Option<SharedView> {
    let su = state.shared_users.get(name?)?;
    Some(SharedView {
        name: su.name.clone(),
        app_id: su.app_id,
        packages: su.packages.iter().cloned().collect(),
    })
}

/// The packages of `state` whose install source names `package`.
fn sources(pre: &State, state: &State, package: &str) -> Vec<(String, [Option<String>; 3])> {
    pre.packages
        .values()
        .filter(|ps| {
            let s = &ps.install_source;
            [&s.installer, &s.originating_package, &s.update_owner]
                .iter()
                .any(|n| n.as_deref() == Some(package))
        })
        .filter_map(|ps| state.packages.get(&ps.name))
        .map(|ps| {
            let s = &ps.install_source;
            (
                ps.name.clone(),
                [
                    s.installer.clone(),
                    s.originating_package.clone(),
                    s.update_owner.clone(),
                ],
            )
        })
        .collect()
}

/// What the original made of the change of `name`; `session`: with the
/// fields an install session decides.
pub(super) fn original(
    kind: Kind,
    pre: &State,
    post: &State,
    name: &str,
    session: bool,
) -> Outcome {
    let package = view(post, name, session || kind == Kind::UserRemoval);
    let shared = match kind {
        Kind::Removal => pre.packages.get(name).and_then(|p| p.shared_user.clone()),
        _ => post.packages.get(name).and_then(|p| p.shared_user.clone()),
    };
    Outcome {
        package,
        shared_user: shared_view(post, shared.as_deref()),
        sources: match kind {
            Kind::Removal => sources(pre, post, name),
            _ => Vec::new(),
        },
    }
}

/// The app id `Settings` acquires for a new package or shared user: the
/// first free one from `first_available` (`AppIdSettingMap`).
pub(super) fn new_app_id(pre: &State, first_available: i32) -> i32 {
    let used: BTreeSet<i32> = pre
        .packages
        .values()
        .map(|p| p.app_id)
        .chain(pre.shared_users.values().map(|s| s.app_id))
        .collect();
    (first_available.max(FIRST_APPLICATION_UID)..)
        .find(|id| !used.contains(id))
        .unwrap_or(first_available)
}

/// What the model makes of the change of `name` from `pre`, with `pkg`
/// the package the original installed (none for a removal) and `session`
/// the install session that did, if the bridge told of it.
pub(super) fn model(
    kind: Kind,
    pre: &State,
    post: &State,
    name: &str,
    pkg: Option<&AndroidPackage>,
    session: Option<&Session>,
    first_available: i32,
) -> Result<Outcome, NotModelled> {
    if pre.users.len() > 1 && kind != Kind::Removal {
        return Err(NotModelled(
            "the install session's user, on a device of several users",
        ));
    }
    match kind {
        Kind::Install => {
            let pkg = pkg.ok_or(NotModelled("an installed package that does not read"))?;
            install(pre, name, pkg, session, first_available)
        }
        Kind::Update => {
            let pkg = pkg.ok_or(NotModelled("an installed package that does not read"))?;
            update(pre, name, pkg, session)
        }
        Kind::Removal => removal(pre, name),
        Kind::UpdateRemoval => Err(NotModelled("restoring a system package")),
        Kind::UserRemoval => user_removal(pre, post, name),
    }
}

/// `PackageImpl.getLongVersionCode`.
fn long_version(pkg: &AndroidPackage) -> i64 {
    (i64::from(pkg.version_code_major) << 32) | i64::from(pkg.version_code as u32)
}

/// The installer an install records as the last enabler
/// (`setEnabled(DEFAULT, user, installer)`), unless the session keeps the
/// enabled state as it was (`isApplicationEnabledSettingPersistent`).
fn enabler(s: &Session) -> Option<Option<String>> {
    (!s.enabled_setting_persistent).then(|| s.installer.clone())
}

/// A new package, not a system one (`createNewSetting`): stopped and not
/// launched in every user, installed in them (one user), enabled by
/// default by its installer, with the session's install reason
/// (`updateSettingsInternalLI`).
fn install(
    pre: &State,
    name: &str,
    pkg: &AndroidPackage,
    session: Option<&Session>,
    first_available: i32,
) -> Result<Outcome, NotModelled> {
    if pre.disabled_system_packages.contains_key(name) {
        return Err(NotModelled(
            "a system package's update installed after its removal",
        ));
    }
    let shared = pkg.shared_user_id.clone();
    let existing = shared.as_ref().and_then(|s| pre.shared_users.get(s));
    let app_id = match existing {
        Some(su) => su.app_id,
        None => new_app_id(pre, first_available),
    };
    let users = pre
        .users
        .keys()
        .map(|&id| {
            (
                id,
                UserView {
                    installed: true,
                    stopped: true,
                    not_launched: true,
                    hidden: false,
                    enabled: 0,
                    enabled_components: Vec::new(),
                    disabled_components: Vec::new(),
                    uninstall_reason: 0,
                    first_install_time: true,
                    suspended: false,
                    distraction_flags: 0,
                    instant_app: false,
                    virtual_preload: false,
                    install_reason: session.map(|s| s.install_reason),
                    last_disable_app_caller: session.map(|s| enabler(s).flatten()),
                },
            )
        })
        .collect();
    let shared_user = shared.as_ref().map(|s| {
        let mut packages: BTreeSet<String> =
            existing.map_or_else(BTreeSet::new, |su| su.packages.iter().cloned().collect());
        packages.insert(name.to_string());
        SharedView {
            name: s.clone(),
            app_id,
            packages,
        }
    });
    Ok(Outcome {
        package: Some(View {
            app_id,
            shared_user: shared,
            path: format!("/data/app/~~*/{name}-*"),
            version_code: long_version(pkg),
            target_sdk_version: pkg.target_sdk_version,
            system: false,
            updated_system_app: false,
            disabled_system_path: None,
            users,
        }),
        shared_user,
        sources: Vec::new(),
    })
}

/// A package replaced by a new version of itself (`updatePackageSetting`
/// and `updateSettingsInternalLI`): its app id, shared user and users'
/// state stay; a system package's first update keeps it as the disabled
/// system package; installed and enabled by default in its users, whose
/// install reason stays where it was installed and is the session's
/// where it was not.
fn update(
    pre: &State,
    name: &str,
    pkg: &AndroidPackage,
    session: Option<&Session>,
) -> Result<Outcome, NotModelled> {
    let ps = pre.packages.get(name).expect("an update of a package");
    if pkg.shared_user_id != ps.shared_user {
        return Err(NotModelled("an update that changes the shared user"));
    }
    let disabled_system_path = match pre.disabled_system_packages.get(name) {
        Some(d) => Some(d.path.clone()),
        None if ps.is.system => Some(ps.path.clone()),
        None => None,
    };
    let users = user_view(ps, session.is_some())
        .into_iter()
        .map(|(id, mut u)| {
            if let Some(s) = session
                && !u.installed
            {
                u.install_reason = Some(s.install_reason);
            }
            u.installed = true;
            u.uninstall_reason = 0;
            match session.map(enabler) {
                Some(None) => {}
                Some(Some(installer)) => {
                    u.enabled = 0;
                    u.last_disable_app_caller = Some(installer);
                }
                None => u.enabled = 0,
            }
            (id, u)
        })
        .collect();
    Ok(Outcome {
        package: Some(View {
            app_id: ps.app_id,
            shared_user: ps.shared_user.clone(),
            path: format!("/data/app/~~*/{name}-*"),
            version_code: long_version(pkg),
            target_sdk_version: pkg.target_sdk_version,
            system: ps.is.system,
            updated_system_app: ps.is.system,
            disabled_system_path,
            users,
        }),
        shared_user: shared_view(pre, ps.shared_user.as_deref()),
        sources: Vec::new(),
    })
}

/// A package removed for every user (`removePackageAndAppIdLPw`): gone
/// from its shared user, which goes too when it has no packages left;
/// the packages it installed lose it as their installer, originating
/// package and update owner (`removeInstallerPackageStatus`).
fn removal(pre: &State, name: &str) -> Result<Outcome, NotModelled> {
    let ps = pre.packages.get(name).expect("a removal of a package");
    if ps.is.system {
        return Err(NotModelled("a system package's removal"));
    }
    let shared_user = shared_view(pre, ps.shared_user.as_deref()).and_then(|mut su| {
        su.packages.remove(name);
        let disabled = pre
            .disabled_system_packages
            .values()
            .any(|d| d.shared_user.as_deref() == Some(&su.name));
        (!su.packages.is_empty() || disabled).then_some(su)
    });
    let clear = |n: &Option<String>| n.clone().filter(|n| n != name);
    let sources = sources(pre, pre, name)
        .into_iter()
        .map(|(q, [i, o, u])| (q, [clear(&i), clear(&o), clear(&u)]))
        .collect();
    Ok(Outcome {
        package: None,
        shared_user,
        sources,
    })
}

/// A package uninstalled for the users that had it and no longer do
/// (`markPackageUninstalledForUserLPw`, without `DELETE_KEEP_DATA`): the
/// rest of the package stays.
fn user_removal(pre: &State, post: &State, name: &str) -> Result<Outcome, NotModelled> {
    let ps = pre.packages.get(name).expect("a user removal of a package");
    let after = post
        .packages
        .get(name)
        .ok_or(NotModelled("a package removed again before it settled"))?;
    let mut package = view(pre, name, true).expect("a package");
    for (id, u) in &mut package.users {
        if u.installed && after.users.get(id).is_some_and(|v| !v.installed) {
            *u = UserView {
                installed: false,
                stopped: true,
                not_launched: true,
                hidden: false,
                enabled: 0,
                enabled_components: Vec::new(),
                disabled_components: Vec::new(),
                uninstall_reason: 0,
                first_install_time: false,
                suspended: false,
                distraction_flags: 0,
                instant_app: false,
                virtual_preload: false,
                install_reason: Some(0),
                last_disable_app_caller: Some(None),
            };
        }
    }
    Ok(Outcome {
        package: Some(package),
        shared_user: shared_view(pre, ps.shared_user.as_deref()),
        sources: Vec::new(),
    })
}

/// The app id a removal frees, if any: the package's own, or its shared
/// user's when that goes too (`AppIdSettingMap.removeSetting` raises the
/// first available app id past it).
pub(super) fn freed_app_id(pre: &State, post: &State, name: &str) -> Option<i32> {
    let ps = pre.packages.get(name)?;
    match &ps.shared_user {
        Some(s) => (!post.shared_users.contains_key(s))
            .then(|| pre.shared_users.get(s).map(|su| su.app_id))
            .flatten(),
        None => Some(ps.app_id),
    }
}

impl Outcome {
    pub fn value(&self) -> Value {
        let s = |v: &Option<String>| v.clone().map_or(Value::Null, Value::Str);
        let list = |v: &[String]| Value::List(v.iter().cloned().map(Value::Str).collect());
        let package = self.package.as_ref().map_or(Value::Null, |p| {
            Value::Fields(vec![
                ("appId".into(), Value::Int(p.app_id)),
                ("sharedUser".into(), s(&p.shared_user)),
                ("path".into(), Value::Str(p.path.clone())),
                ("versionCode".into(), Value::Long(p.version_code)),
                ("targetSdkVersion".into(), Value::Int(p.target_sdk_version)),
                ("system".into(), Value::Bool(p.system)),
                ("updatedSystemApp".into(), Value::Bool(p.updated_system_app)),
                ("disabledSystemPath".into(), s(&p.disabled_system_path)),
                (
                    "users".into(),
                    Value::List(
                        p.users
                            .iter()
                            .map(|(id, u)| {
                                let mut fields = vec![
                                    ("id".into(), Value::Int(*id)),
                                    ("installed".into(), Value::Bool(u.installed)),
                                    ("stopped".into(), Value::Bool(u.stopped)),
                                    ("notLaunched".into(), Value::Bool(u.not_launched)),
                                    ("hidden".into(), Value::Bool(u.hidden)),
                                    ("enabled".into(), Value::Int(u.enabled)),
                                    ("enabledComponents".into(), list(&u.enabled_components)),
                                    ("disabledComponents".into(), list(&u.disabled_components)),
                                    ("uninstallReason".into(), Value::Int(u.uninstall_reason)),
                                    ("firstInstallTime".into(), Value::Bool(u.first_install_time)),
                                    ("suspended".into(), Value::Bool(u.suspended)),
                                    ("distractionFlags".into(), Value::Int(u.distraction_flags)),
                                    ("instantApp".into(), Value::Bool(u.instant_app)),
                                    ("virtualPreload".into(), Value::Bool(u.virtual_preload)),
                                ];
                                if let Some(reason) = u.install_reason {
                                    fields.push(("installReason".into(), Value::Int(reason)));
                                }
                                if let Some(caller) = &u.last_disable_app_caller {
                                    fields.push(("lastDisableAppCaller".into(), s(caller)));
                                }
                                Value::Fields(fields)
                            })
                            .collect(),
                    ),
                ),
            ])
        });
        let shared = self.shared_user.as_ref().map_or(Value::Null, |su| {
            Value::Fields(vec![
                ("name".into(), Value::Str(su.name.clone())),
                ("appId".into(), Value::Int(su.app_id)),
                (
                    "packages".into(),
                    Value::List(su.packages.iter().cloned().map(Value::Str).collect()),
                ),
            ])
        });
        let sources = Value::List(
            self.sources
                .iter()
                .map(|(q, [i, o, u])| {
                    Value::Fields(vec![
                        ("package".into(), Value::Str(q.clone())),
                        ("installer".into(), s(i)),
                        ("originating".into(), s(o)),
                        ("updateOwner".into(), s(u)),
                    ])
                })
                .collect(),
        );
        Value::Fields(vec![
            ("package".into(), package),
            ("sharedUser".into(), shared),
            ("installSources".into(), sources),
        ])
    }
}
