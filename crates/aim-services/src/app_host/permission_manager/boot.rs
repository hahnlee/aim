//! Boot volume admissions, AppIdPermissionPolicy at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use crate::package::scan::permission_admissions::Admission;
mod signature;
use signature::Policy;

type Evaluate<'a> = dyn Fn(&Permission, i32, i32, i32, i32) -> Result<i32, Exception> + 'a;

fn unsupported(branch: &str) -> Exception {
    Exception::new(
        EX_UNSUPPORTED_OPERATION,
        format!("permission boot: {branch} (#2169)"),
    )
}

impl PermissionManager {
    /// PermissionService queues onPackageAdded until onStorageVolumeMounted.
    /// Commit the batch through the sole access owner before query publication.
    pub(crate) fn reconcile_boot(
        &self,
        state: &State,
        admissions: &[&Admission],
        is_system_updated: bool,
    ) -> Result<Arc<permissions::AccessView>, Exception> {
        self.retained_access()?;
        let mut owner = self.owner.lock().unwrap();
        let base = permissions::AccessView {
            system: owner.system.clone().expect("loaded"),
            user0: owner.user0.clone().expect("loaded"),
        };
        let config =
            crate::package::system_config::SystemConfig::read(self.host.root()?, &|name| {
                self.host.property(name)
            });
        let policy = Policy {
            allowlist: config.permission_allowlist,
            debuggable: self.host.property("ro.debuggable").as_deref() == Some("1"),
            control_privapp_permissions: self
                .host
                .property("ro.control_privapp_permissions")
                .unwrap_or_default(),
        };
        let after = reconcile(
            &base,
            state,
            admissions,
            is_system_updated,
            &policy,
            &|p, info, target, old, new| Ok(evaluate_permission(p, info, target, old, new)),
        )?;
        if *after.system != *base.system {
            self.write_system(&after.system)?;
            owner.system = Some(after.system.clone());
        }
        if *after.user0 != *base.user0 {
            self.write_user0(&after.user0)?;
            owner.user0 = Some(after.user0.clone());
        }
        Ok(Arc::new(permissions::AccessView {
            system: owner.system.clone().expect("loaded"),
            user0: owner.user0.clone().expect("loaded"),
        }))
    }
}

fn clear(user: &mut AccessUser, name: &str) {
    for (_, flags) in &mut user.app_id_permissions {
        flags.retain(|(permission, _)| permission != name);
    }
}

fn declared(state: &State, package: &str, name: &str, tree: bool) -> bool {
    [
        state.packages.get(package),
        state.disabled_system_packages.get(package),
    ]
    .into_iter()
    .flatten()
    .filter_map(|ps| ps.pkg.as_deref())
    .any(|code| {
        code.permissions
            .iter()
            .any(|p| p.tree == tree && p.component.name == name)
    })
}

/// trimPermissions of one owner, retaining the disabled system declaration.
fn trim(
    system: &mut AccessSystem,
    user: &mut AccessUser,
    state: &State,
    package: &str,
    changed: &mut BTreeSet<String>,
) {
    system.permission_trees.retain(|p| {
        p.package != package
            || state
                .packages
                .get(&p.package)
                .is_some_and(|ps| ps.pkg.is_none())
            || declared(state, &p.package, &p.name, true)
    });
    for permission in &mut system.permissions {
        if permission.dynamic.is_some() {
            if let Some(tree) = system
                .permission_trees
                .iter()
                .find(|tree| permission.name.starts_with(&format!("{}.", tree.name)))
            {
                permission.package = tree.package.clone();
            }
        }
    }
    system.permissions.retain(|p| {
        let keep = p.package != package
            || state
                .packages
                .get(&p.package)
                .is_some_and(|ps| ps.pkg.is_none())
            || declared(state, &p.package, &p.name, false);
        if !keep {
            clear(user, &p.name);
            changed.insert(p.name.clone());
        }
        keep
    });
}

fn reconcile(
    base: &permissions::AccessView,
    state: &State,
    admissions: &[&Admission],
    is_system_updated: bool,
    policy: &Policy,
    evaluate: &Evaluate<'_>,
) -> Result<permissions::AccessView, Exception> {
    let mut system = (*base.system).clone();
    let mut user = (*base.user0).clone();
    let mut changed = BTreeSet::new();
    let mut added = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let removed: BTreeSet<_> = system
        .permissions
        .iter()
        .chain(&system.permission_trees)
        .filter(|p| !state.packages.contains_key(&p.package))
        .map(|p| p.package.clone())
        .collect();
    for package in removed {
        trim(&mut system, &mut user, state, &package, &mut changed);
    }
    for admission in admissions {
        let ps = state.packages.get(&admission.name).ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "boot permission admission package absent")
        })?;
        if !seen.insert(&ps.name) {
            return Err(Exception::illegal_argument(
                "duplicate boot permission admission",
            ));
        }
        if ps.is.apex {
            continue;
        }
        let code = ps.pkg.as_deref().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "boot permission admission code absent")
        })?;
        if admission
            .old_package
            .as_ref()
            .is_some_and(|old| old.shared_user_id != code.shared_user_id)
        {
            return Err(unsupported("shared UID migration"));
        }
        let previous = admission.old_package.as_ref();
        let package_changed = previous.is_some_and(|old| {
            old.requested_permissions != code.requested_permissions
                || old.permissions != code.permissions
                || old.signing_details != code.signing_details
                || old.target_sdk_version != code.target_sdk_version
        });
        let version = user
            .package_versions
            .iter()
            .find(|(name, _)| name == &ps.name)
            .map(|(_, version)| *version);
        if version.is_some_and(|version| version < 17) {
            return Err(unsupported("access package-state upgrade"));
        }
        if package_changed || version.is_none() {
            added.insert(ps.name.clone());
        }
        // adoptPermissions: only a missing system package can transfer ownership.
        for original in &code.adopt_permissions {
            let eligible = state
                .packages
                .get(original)
                .is_some_and(|p| p.is.system && p.pkg.is_none());
            if eligible {
                for permission in &mut system.permissions {
                    if permission.package == *original {
                        permission.package = ps.name.clone();
                        changed.insert(permission.name.clone());
                    }
                }
            }
        }
        for declaration in &code.permissions {
            let name = &declaration.component.name;
            if system
                .permission_trees
                .iter()
                .any(|tree| name.starts_with(&format!("{}.", tree.name)) && tree.package != ps.name)
            {
                continue;
            }
            let definitions = if declaration.tree {
                &mut system.permission_trees
            } else {
                &mut system.permissions
            };
            let old = definitions.iter().position(|p| p.name == *name);
            let mut reset = false;
            if let Some(index) = old {
                let permission = &definitions[index];
                if permission.package != ps.name {
                    // First system declaration wins; a system app may override a non-system app.
                    if !ps.is.system
                        || state
                            .packages
                            .get(&permission.package)
                            .is_some_and(|p| p.is.system)
                    {
                        continue;
                    }
                    reset = true;
                } else {
                    let old_declaration = previous.and_then(|code| {
                        code.permissions.iter().find(|p| p.component.name == *name)
                    });
                    reset = old_declaration.is_some_and(|p| {
                        let runtime = declaration.protection_level & PROTECTION_MASK_BASE
                            == PROTECTION_DANGEROUS;
                        (runtime
                            && p.protection_level & PROTECTION_MASK_BASE != PROTECTION_DANGEROUS)
                            || (runtime
                                && declaration.group.is_some()
                                && declaration.group != p.group)
                            || (declaration.protection_level & PROTECTION_MASK_BASE == 4
                                && p.protection_level & PROTECTION_MASK_BASE != 4)
                    });
                }
            }
            let value = Permission {
                name: name.clone(),
                package: ps.name.clone(),
                protection_level: declaration.protection_level,
                dynamic: None,
            };
            let differs = old.is_none_or(|i| definitions[i] != value);
            match old {
                Some(i) => definitions[i] = value,
                None => {
                    definitions.push(value);
                    *definitions = info::array_order(std::mem::take(definitions), |p| &p.name);
                }
            }
            if reset {
                clear(&mut user, name);
            }
            let signing_changed =
                previous.is_some_and(|old| old.signing_details != code.signing_details);
            if !declaration.tree
                && (differs
                    || reset
                    || (declaration.protection_level & PROTECTION_MASK_BASE
                        == PROTECTION_SIGNATURE
                        && signing_changed))
            {
                changed.insert(name.clone());
            }
        }
        trim(&mut system, &mut user, state, &ps.name, &mut changed);
    }
    user.package_versions
        .retain(|(name, _)| state.packages.contains_key(name));
    // trimPermissionStates uses the union of the UID's active packages, not its factory requests.
    let mut requests: BTreeMap<i32, Vec<&PackageState>> = BTreeMap::new();
    for ps in state.packages.values().filter(|p| !p.is.apex) {
        requests
            .entry(ps.shared_user_app_id.unwrap_or(ps.app_id))
            .or_default()
            .push(ps);
    }
    user.app_id_permissions
        .retain(|(id, _)| requests.contains_key(id));
    for (id, packages) in requests {
        if packages.iter().all(|p| p.pkg.is_none()) {
            continue;
        }
        let requested: BTreeSet<_> = packages
            .iter()
            .filter_map(|p| p.pkg.as_deref())
            .flat_map(|p| p.requested_permissions.iter())
            .collect();
        if let Some((_, flags)) = user
            .app_id_permissions
            .iter_mut()
            .find(|(app_id, _)| *app_id == id)
        {
            flags.retain(|(name, _)| requested.contains(name));
        }
        let installing = is_system_updated || packages.iter().any(|p| added.contains(&p.name));
        for name in requested {
            let affected = installing || changed.contains(name);
            let old = user.permission_flags(id, name);
            let Some(permission) = system.permissions.iter().find(|p| p.name == *name) else {
                if old == 0 {
                    user.set_permission_flags(id, name, INSTALL_REVOKED);
                }
                continue;
            };
            let declaration = state
                .packages
                .get(&permission.package)
                .and_then(|p| p.pkg.as_deref())
                .and_then(|p| {
                    p.permissions
                        .iter()
                        .find(|p| !p.tree && p.component.name == *name)
                });
            let restricted = declaration.is_some_and(|p| {
                p.component.flags & (FLAG_HARD_RESTRICTED | FLAG_SOFT_RESTRICTED) != 0
            });
            if !affected && (restricted || is_role(permission)) {
                continue;
            }
            let requesters: Vec<_> = packages
                .iter()
                .copied()
                .filter(|p| {
                    p.pkg
                        .as_ref()
                        .is_some_and(|p| p.requested_permissions.contains(name))
                })
                .collect();
            let flags = match permission.protection_level & PROTECTION_MASK_BASE {
                0 => {
                    if old & INSTALL_GRANTED != 0 {
                        old
                    } else if old & INSTALL_REVOKED == 0
                        || installing
                        || requesters.iter().any(|p| p.is.system)
                    {
                        INSTALL_GRANTED
                    } else {
                        INSTALL_REVOKED
                    }
                }
                PROTECTION_SIGNATURE | 4
                    if is_role(permission)
                        || permission.protection_level & PROTECTION_MASK_BASE
                            == PROTECTION_SIGNATURE =>
                {
                    // Constructor role selections are made after this capture.
                    // Existing grants need no role decision on an unchanged mount.
                    if !affected
                        && permission.protection_level
                            & (0x100
                                | 0x200
                                | 0x800
                                | 0x10000
                                | 0x80000
                                | 0x100000
                                | 0x200000
                                | 0x800000
                                | 0x2000000)
                            != 0
                    {
                        continue;
                    }
                    let granted = policy.grant(state, &requesters, permission, declaration)?;
                    let flags = if granted { PROTECTION_GRANTED } else { 0 };
                    if is_development(permission) {
                        flags | (old & RUNTIME_GRANTED)
                    } else {
                        flags
                    }
                }
                PROTECTION_DANGEROUS => {
                    if requesters.iter().any(|p| {
                        p.pkg
                            .as_ref()
                            .is_some_and(|p| p.implicit_permissions.contains(name))
                    }) {
                        if affected {
                            return Err(unsupported("implicit runtime permission inheritance"));
                        }
                        continue;
                    }
                    runtime_flags(base, &user, id, name, permission, old, &requesters)
                }
                _ => {
                    if affected {
                        return Err(unsupported("internal protection evaluation"));
                    }
                    continue;
                }
            };
            let flags = if permission.protection_level & 0x40 != 0 && !is_runtime(permission) {
                flags | (old & (ROLE | USER_SET))
            } else {
                flags
            };
            let flags = if restricted
                || is_role(permission)
                || old & (RESTRICTION_REVOKED | SOFT_RESTRICTED) != 0
            {
                let target = requesters
                    .iter()
                    .filter_map(|p| p.pkg.as_ref())
                    .map(|p| p.target_sdk_version)
                    .min()
                    .unwrap_or(CUR_DEVELOPMENT);
                evaluate(
                    permission,
                    declaration.map_or(0, |p| p.component.flags),
                    target,
                    old,
                    flags,
                )?
            } else {
                flags
            };
            user.set_permission_flags(id, name, flags);
        }
    }
    for name in added {
        if !user.package_versions.iter().any(|(p, _)| *p == name) {
            user.package_versions.push((name, 17));
            user.package_versions = info::array_order(user.package_versions, |(name, _)| name);
        }
    }
    user.app_id_permissions
        .retain(|(_, flags)| !flags.is_empty());
    Ok(permissions::AccessView {
        system: Arc::new(system),
        user0: Arc::new(user),
    })
}

/// AppIdPermissionPolicy.kt at android-16.0.0_r1, services/permission/java/
/// com/android/server/permission/access/permission/ (Apache-2.0).
/// Keep this mount-time evaluation before publication, not at systemReady:
/// PackageManagerService.java:2368-2376; PermissionService.kt:2394-2404;
/// AppIdPermissionPolicy.kt:101-127. No Java state or late policy owner is needed.
fn evaluate_permission(
    permission: &Permission,
    info_flags: i32,
    target: i32,
    old: i32,
    mut new: i32,
) -> i32 {
    match permission.protection_level & PROTECTION_MASK_BASE {
        PROTECTION_SIGNATURE | 4 => {
            // AppIdPermissionPolicy.kt:1066-1070: retain dynamic role grants.
            if is_role(permission) {
                new |= old & (ROLE | RUNTIME_GRANTED);
            }
        }
        PROTECTION_DANGEROUS => {
            let hard = info_flags & FLAG_HARD_RESTRICTED != 0;
            let soft = info_flags & FLAG_SOFT_RESTRICTED != 0;
            // AppIdPermissionPolicy.kt:1183-1205: all three persisted exemptions;
            // previously unevaluated restricted permissions start upgrade-exempt.
            let mut exempt = new & (INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT) != 0;
            if (hard || soft) && !exempt && new & (RESTRICTION_REVOKED | SOFT_RESTRICTED) == 0 {
                new |= UPGRADE_EXEMPT;
                exempt = true;
            }
            // AppIdPermissionPolicy.kt:1206-1211: hard restriction.
            if hard && !exempt {
                new |= RESTRICTION_REVOKED;
            } else {
                new &= !RESTRICTION_REVOKED;
            }
            // AppIdPermissionPolicy.kt:1212-1227,1538-1548: soft restriction;
            // storage uses the minimum target SDK of the UID's requesters.
            let storage_exempt = target >= 29
                && matches!(
                    permission.name.as_str(),
                    "android.permission.READ_EXTERNAL_STORAGE"
                        | "android.permission.WRITE_EXTERNAL_STORAGE"
                );
            if soft && !exempt && !storage_exempt {
                new |= SOFT_RESTRICTED;
            } else {
                new &= !SOFT_RESTRICTED;
            }
        }
        _ => {}
    }
    new
}

/// Existing runtime transitions; restrictions are evaluated before publication.
fn runtime_flags(
    base: &permissions::AccessView,
    user: &AccessUser,
    id: i32,
    name: &str,
    permission: &Permission,
    old: i32,
    requesters: &[&PackageState],
) -> i32 {
    let mut flags =
        old & (((1 << 24) - 1) & !(INSTALL_GRANTED | INSTALL_REVOKED | PROTECTION_GRANTED));
    let revoked = flags != 0 && !permission_granted(flags);
    let target = requesters
        .iter()
        .filter_map(|p| p.pkg.as_ref())
        .map(|p| p.target_sdk_version)
        .min()
        .unwrap_or(CUR_DEVELOPMENT);
    if target < M {
        if permission.protection_level & 0x2000 != 0 {
            flags &= INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT;
        } else {
            flags |= LEGACY_GRANTED;
            if revoked {
                flags |= APP_OP_REVOKED;
            }
            if base.user0.permission_flags(id, name) == 0 {
                flags |= IMPLICIT;
            }
        }
    } else {
        let legacy = flags & LEGACY_GRANTED != 0;
        let implicit = flags & IMPLICIT != 0;
        let implicit_grant = flags & IMPLICIT_GRANTED != 0;
        if legacy {
            flags &= !LEGACY_GRANTED;
            if !implicit {
                flags |= RUNTIME_GRANTED;
            }
        }
        flags &= !IMPLICIT_GRANTED;
        if (legacy || implicit_grant) && flags & APP_OP_REVOKED != 0 {
            flags &= !(RUNTIME_GRANTED | APP_OP_REVOKED);
        }
        if implicit {
            flags &= !IMPLICIT;
            let background = user.permission_flags(id, ACCESS_BACKGROUND_LOCATION);
            let nearby = matches!(
                name,
                "android.permission.BLUETOOTH_ADVERTISE"
                    | "android.permission.BLUETOOTH_CONNECT"
                    | "android.permission.BLUETOOTH_SCAN"
                    | "android.permission.NEARBY_WIFI_DEVICES"
            ) && permission_granted(background)
                && background & (RESTRICTION_REVOKED | SOFT_RESTRICTED | APP_OP_REVOKED | IMPLICIT)
                    == 0;
            if nearby || flags & (SYSTEM_FIXED | POLICY_FIXED) != 0 {
                if implicit_grant {
                    flags |= RUNTIME_GRANTED;
                }
            } else {
                flags &= !(RUNTIME_GRANTED | USER_SET | USER_FIXED);
            }
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::super::tests::PersistedAccess;
    use super::*;
    use crate::package::{
        model::PackageUserState,
        pkg::{AndroidPackage, Component, Permission as Declaration},
        settings::Signatures,
    };

    fn declaration(name: &str, level: i32, tree: bool) -> Declaration {
        Declaration {
            component: Component {
                name: name.into(),
                package_name: "owner".into(),
                ..Default::default()
            },
            protection_level: level,
            tree,
            ..Default::default()
        }
    }
    fn package(
        name: &str,
        id: i32,
        definitions: Vec<Declaration>,
        requests: &[&str],
    ) -> PackageState {
        PackageState {
            name: name.into(),
            app_id: id,
            target_sdk_version: 36,
            signatures: Some(Signatures {
                signatures: vec![vec![id as u8]],
                ..Default::default()
            }),
            pkg: Some(Arc::new(AndroidPackage {
                package_name: name.into(),
                target_sdk_version: 36,
                permissions: definitions,
                requested_permissions: requests.iter().map(|s| (*s).into()).collect(),
                ..Default::default()
            })),
            users: [(0, PackageUserState::default())].into(),
            ..Default::default()
        }
    }
    fn state() -> State {
        State {
            packages: [
                (
                    "owner".into(),
                    package(
                        "owner",
                        10001,
                        vec![declaration("owner.TEST", 0, false)],
                        &[],
                    ),
                ),
                (
                    "client".into(),
                    package("client", 10002, vec![], &["owner.TEST"]),
                ),
            ]
            .into(),
            ..Default::default()
        }
    }
    fn admissions(state: &State) -> Vec<Admission> {
        state
            .packages
            .keys()
            .map(|name| Admission {
                name: name.clone(),
                scan_as_instant: false,
                old_package: None,
            })
            .collect()
    }
    fn base() -> permissions::AccessView {
        permissions::AccessView {
            system: Arc::new(AccessSystem {
                permissions: vec![Permission {
                    name: "owner.TEST".into(),
                    package: "owner".into(),
                    protection_level: 0,
                    dynamic: None,
                }],
                ..Default::default()
            }),
            user0: Arc::new(AccessUser {
                package_versions: info::array_order(
                    vec![("owner".into(), 17), ("client".into(), 17)],
                    |(name, _)| name,
                ),
                app_id_permissions: vec![(10002, vec![("owner.TEST".into(), INSTALL_GRANTED)])],
                ..Default::default()
            }),
        }
    }
    fn no_dynamic(_: &Permission, _: i32, _: i32, _: i32, _: i32) -> Result<i32, Exception> {
        panic!("unexpected cold policy evaluation")
    }
    fn run(
        base: &permissions::AccessView,
        state: &State,
        updated: bool,
    ) -> Result<permissions::AccessView, Exception> {
        reconcile(
            base,
            state,
            &admissions(state).iter().collect::<Vec<_>>(),
            updated,
            &Policy::default(),
            &no_dynamic,
        )
    }
    #[test]
    fn unchanged_mount_keeps_the_access_owner_and_writes_nothing() {
        let fixture = PersistedAccess::new();
        fixture.write(SYSTEM_ACCESS, br#"<access><permissions><permission name="owner.TEST" packageName="owner" protectionLevel="0" type="0"/></permissions><permission-trees/></access>"#);
        fixture.write(USER0_ACCESS, br#"<access><package-versions><package name="owner" version="17"/><package name="client" version="17"/></package-versions><app-id-permissions><app-id id="10002"><permission name="owner.TEST" flags="1"/></app-id></app-id-permissions><app-id-app-ops><app-id id="10002"><app-op name="android:camera" mode="2"/></app-id></app-id-app-ops></access>"#);
        let manager = PermissionManager::new(fixture.host.clone());
        let before = manager.retained_access().unwrap();
        let state = state();
        let receipts = admissions(&state);
        let after = manager
            .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), false)
            .unwrap();
        assert!(before.same_base(&after));
        assert!(manager.access_writes.lock().unwrap().is_empty());
        assert_eq!(*after.user0, *before.user0);
    }
    #[test]
    fn changed_declaration_new_tree_and_removed_owner_commit_once() {
        let fixture = PersistedAccess::new();
        fixture.write(SYSTEM_ACCESS, br#"<access><permissions><permission name="owner.TEST" packageName="owner" protectionLevel="0" type="0"/><permission name="gone.TEST" packageName="gone" protectionLevel="0" type="0"/></permissions><permission-trees><permission name="gone.tree" packageName="gone" protectionLevel="0" type="0"/></permission-trees></access>"#);
        fixture.write(USER0_ACCESS, br#"<access><package-versions><package name="owner" version="17"/><package name="client" version="17"/><package name="gone" version="17"/></package-versions><app-id-permissions><app-id id="10002"><permission name="owner.TEST" flags="1"/><permission name="gone.TEST" flags="1"/></app-id></app-id-permissions><package-app-ops><package name="client"><app-op name="android:camera" mode="2"/></package></package-app-ops></access>"#);
        let manager = PermissionManager::new(fixture.host.clone());
        let before = manager.retained_access().unwrap();
        let mut state = state();
        let owner = Arc::make_mut(
            state
                .packages
                .get_mut("owner")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        );
        owner.permissions[0].protection_level = PROTECTION_DANGEROUS;
        owner.permissions.push(declaration("owner.tree", 0, true));
        let receipts = admissions(&state);
        let after = manager
            .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), true)
            .unwrap();
        assert_eq!(
            after.system.permissions[0].protection_level,
            PROTECTION_DANGEROUS
        );
        assert!(after.system.permissions.iter().all(|p| p.package != "gone"));
        assert_eq!(
            after
                .system
                .permission_trees
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["owner.tree"]
        );
        assert_eq!(after.user0.permission_flags(10002, "owner.TEST"), 0);
        assert_eq!(after.user0.permission_flags(10002, "gone.TEST"), 0);
        assert!(
            !after
                .user0
                .package_versions
                .iter()
                .any(|(name, _)| name == "gone")
        );
        assert_eq!(after.user0.package_app_ops, before.user0.package_app_ops);
        assert_eq!(
            *manager.access_writes.lock().unwrap(),
            [SYSTEM_ACCESS, USER0_ACCESS]
        );
        assert_eq!(before.system.permissions[0].protection_level, 0);
        let reload = PermissionManager::new(fixture.host.clone())
            .retained_access()
            .unwrap();
        assert_eq!(*reload.system, *after.system);
        assert_eq!(*reload.user0, *after.user0);
        let repeat = manager
            .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), false)
            .unwrap();
        assert!(after.same_base(&repeat));
        assert_eq!(manager.access_writes.lock().unwrap().len(), 2);
    }
    #[test]
    fn system_owner_precedence_tree_boundaries_factory_and_dynamic_pruning() {
        let mut state = state();
        let mut base = base();
        let old_system = Arc::make_mut(&mut base.system);
        old_system.permissions.push(Permission {
            name: "old.tree.dynamic".into(),
            package: "gone".into(),
            protection_level: 0,
            dynamic: Some((7, Some("label".into()))),
        });
        let owner = state.packages.get_mut("owner").unwrap();
        owner.is.system = true;
        let code = Arc::make_mut(owner.pkg.as_mut().unwrap());
        code.permissions.push(declaration("old.tree", 0, true));
        let mut factory = owner.clone();
        Arc::make_mut(factory.pkg.as_mut().unwrap())
            .permissions
            .push(declaration("owner.FACTORY", 0, false));
        state
            .disabled_system_packages
            .insert("owner".into(), factory);
        old_system.permissions.push(Permission {
            name: "owner.FACTORY".into(),
            package: "owner".into(),
            protection_level: 0,
            dynamic: None,
        });
        let mut squatter = package(
            "squatter",
            10003,
            vec![
                declaration("owner.TEST", 2, false),
                declaration("old.tree.forbidden", 0, false),
            ],
            &[],
        );
        squatter.is.system = true;
        state.packages.insert("squatter".into(), squatter);
        let order = ["owner", "squatter", "client"].map(|name| Admission {
            name: name.into(),
            scan_as_instant: false,
            old_package: None,
        });
        let after = reconcile(
            &base,
            &state,
            &order.iter().collect::<Vec<_>>(),
            false,
            &Policy::default(),
            &no_dynamic,
        )
        .unwrap();
        assert_eq!(
            after
                .system
                .permissions
                .iter()
                .find(|p| p.name == "owner.TEST")
                .unwrap()
                .package,
            "owner"
        );
        assert!(
            after
                .system
                .permissions
                .iter()
                .any(|p| p.name == "owner.FACTORY")
        );
        assert!(
            after
                .system
                .permissions
                .iter()
                .all(|p| !p.name.starts_with("old.tree."))
        );
    }
    #[test]
    fn normal_revocations_and_signature_capabilities_are_reevaluated() {
        let mut state = state();
        let mut base = base();
        Arc::make_mut(&mut base.user0).set_permission_flags(10002, "owner.TEST", INSTALL_REVOKED);
        assert_eq!(
            run(&base, &state, false)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            INSTALL_REVOKED
        );
        assert_eq!(
            run(&base, &state, true)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            INSTALL_GRANTED
        );
        Arc::make_mut(
            state
                .packages
                .get_mut("owner")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        )
        .permissions[0]
            .protection_level = PROTECTION_SIGNATURE;
        assert_eq!(
            run(&base, &state, false)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            0
        );
        state.packages.get_mut("client").unwrap().signatures = Some(Signatures {
            signatures: vec![vec![2]],
            past_signatures: Some(vec![(vec![1], 0), (vec![2], 0)]),
            ..Default::default()
        });
        state.packages.get_mut("owner").unwrap().signatures = Some(Signatures {
            signatures: vec![vec![3]],
            past_signatures: Some(vec![(vec![1], 4), (vec![3], 0)]),
            ..Default::default()
        });
        assert_eq!(
            run(&base, &state, false)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            PROTECTION_GRANTED
        );
        state
            .packages
            .get_mut("owner")
            .unwrap()
            .signatures
            .as_mut()
            .unwrap()
            .past_signatures
            .as_mut()
            .unwrap()[0]
            .1 = 0;
        assert_eq!(
            run(&base, &state, false)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            0
        );
    }
    #[test]
    fn signature_allowlists_and_factory_requests_control_static_grants() {
        let fixture = PersistedAccess::new();
        let root = fixture.host.root().unwrap();
        let path = root.join("vendor/etc/permissions/grants.xml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "<permissions><privapp-permissions package='client'><permission name='owner.TEST'/></privapp-permissions></permissions>").unwrap();
        let config = crate::package::system_config::SystemConfig::read(root, &|_| None);
        let policy = Policy {
            allowlist: config.permission_allowlist,
            control_privapp_permissions: "enforce".into(),
            ..Default::default()
        };
        let mut state = state();
        let owner = state.packages.get_mut("owner").unwrap();
        owner.is.system = true;
        Arc::make_mut(owner.pkg.as_mut().unwrap()).permissions[0].protection_level =
            PROTECTION_SIGNATURE | 0x10;
        let client = state.packages.get_mut("client").unwrap();
        client.is.system = true;
        client.is.privileged = true;
        client.is.vendor = true;
        let run = |state: &State| {
            reconcile(
                &base(),
                state,
                &admissions(state).iter().collect::<Vec<_>>(),
                true,
                &policy,
                &no_dynamic,
            )
            .unwrap()
            .user0
            .permission_flags(10002, "owner.TEST")
        };
        // Vendor privileged apps need vendorPrivileged as well as the allowlist.
        assert_eq!(run(&state), 0);
        Arc::make_mut(
            state
                .packages
                .get_mut("owner")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        )
        .permissions[0]
            .protection_level |= 0x8000;
        assert_eq!(run(&state), PROTECTION_GRANTED);
        let factory = state.packages["client"].clone();
        state
            .disabled_system_packages
            .insert("client".into(), factory);
        state
            .packages
            .get_mut("client")
            .unwrap()
            .is
            .updated_system_app = true;
        assert_eq!(run(&state), PROTECTION_GRANTED);
        Arc::make_mut(
            state
                .disabled_system_packages
                .get_mut("client")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        )
        .requested_permissions
        .clear();
        assert_eq!(run(&state), 0);
        // A platform signature allowlist is separate from the privileged allowlist.
        let mut platform = state.packages.remove("owner").unwrap();
        platform.name = "android".into();
        platform.signatures = state.packages["client"].signatures.clone();
        Arc::make_mut(platform.pkg.as_mut().unwrap()).permissions[0].protection_level =
            PROTECTION_SIGNATURE;
        state.packages.insert("android".into(), platform);
        state.packages.get_mut("client").unwrap().is.system = false;
        state.system.flags.push((
            "android.permission.flags.signature_permission_allowlist_enabled".into(),
            true,
        ));
        let declaration = state.packages["android"].pkg.as_ref().unwrap().permissions[0].clone();
        let permission = Permission {
            name: "owner.TEST".into(),
            package: "android".into(),
            protection_level: PROTECTION_SIGNATURE,
            dynamic: None,
        };
        assert!(
            !policy
                .grant(
                    &state,
                    &[&state.packages["client"]],
                    &permission,
                    Some(&declaration)
                )
                .unwrap()
        );
        let signature_file = root.join("vendor/etc/permissions/signatures.xml");
        std::fs::write(signature_file, "<permissions><signature-permissions package='client'><permission name='owner.TEST'/></signature-permissions></permissions>").unwrap();
        let policy = Policy {
            allowlist: crate::package::system_config::SystemConfig::read(root, &|_| None)
                .permission_allowlist,
            ..Default::default()
        };
        assert!(
            policy
                .grant(
                    &state,
                    &[&state.packages["client"]],
                    &permission,
                    Some(&declaration)
                )
                .unwrap()
        );
    }

    #[test]
    fn first_system_declaration_wins_in_both_scan_orders() {
        for order in [["owner", "other", "client"], ["other", "owner", "client"]] {
            let mut state = state();
            state.packages.get_mut("owner").unwrap().is.system = true;
            let mut other = package(
                "other",
                10003,
                vec![declaration("owner.TEST", 0, false)],
                &[],
            );
            other.is.system = true;
            state.packages.insert("other".into(), other);
            let mut base = base();
            Arc::make_mut(&mut base.system).permissions.clear();
            let receipts = order.map(|name| Admission {
                name: name.into(),
                scan_as_instant: false,
                old_package: None,
            });
            let after = reconcile(
                &base,
                &state,
                &receipts.iter().collect::<Vec<_>>(),
                false,
                &Policy::default(),
                &no_dynamic,
            )
            .unwrap();
            assert_eq!(after.system.permissions[0].package, order[0]);
        }
    }

    #[test]
    fn a_dropped_tree_is_pruned_before_the_next_admission() {
        let mut state = state();
        let mut base = base();
        Arc::make_mut(&mut base.system)
            .permission_trees
            .push(Permission {
                name: "old.tree".into(),
                package: "owner".into(),
                protection_level: 0,
                dynamic: None,
            });
        let other = package(
            "other",
            10003,
            vec![declaration("old.tree.CHILD", 0, false)],
            &[],
        );
        state.packages.insert("other".into(), other);
        let receipts = ["owner", "other", "client"].map(|name| Admission {
            name: name.into(),
            scan_as_instant: false,
            old_package: None,
        });
        let after = reconcile(
            &base,
            &state,
            &receipts.iter().collect::<Vec<_>>(),
            false,
            &Policy::default(),
            &no_dynamic,
        )
        .unwrap();
        assert!(after.system.permission_trees.is_empty());
        assert_eq!(
            after
                .system
                .permissions
                .iter()
                .find(|p| p.name == "old.tree.CHILD")
                .unwrap()
                .package,
            "other"
        );
    }

    #[test]
    fn shared_uid_migration_and_old_access_versions_reject_only_their_branches() {
        let state = state();
        let mut receipts = admissions(&state);
        let mut old = (**state.packages["owner"].pkg.as_ref().unwrap()).clone();
        old.shared_user_id = Some("old.shared".into());
        receipts
            .iter_mut()
            .find(|r| r.name == "owner")
            .unwrap()
            .old_package = Some(old);
        let error = reconcile(
            &base(),
            &state,
            &receipts.iter().collect::<Vec<_>>(),
            false,
            &Policy::default(),
            &no_dynamic,
        )
        .unwrap_err();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("shared UID migration") && error.message.contains("#2169"));
        let mut base = base();
        Arc::make_mut(&mut base.user0).package_versions[0].1 = 16;
        let error = run(&base, &state, false).unwrap_err();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("package-state upgrade") && error.message.contains("#2169"));
    }

    #[test]
    fn runtime_group_changes_revoke_grants_and_legacy_transitions_keep_user_denials() {
        let mut state = state();
        let mut base = base();
        Arc::make_mut(&mut base.system).permissions[0].protection_level = PROTECTION_DANGEROUS;
        Arc::make_mut(&mut base.user0).set_permission_flags(
            10002,
            "owner.TEST",
            RUNTIME_GRANTED | USER_SET,
        );
        let owner = Arc::make_mut(
            state
                .packages
                .get_mut("owner")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        );
        owner.permissions[0].protection_level = PROTECTION_DANGEROUS;
        let old = owner.clone();
        owner.permissions[0].group = Some("owner.NEW".into());
        let mut receipts = admissions(&state);
        receipts
            .iter_mut()
            .find(|r| r.name == "owner")
            .unwrap()
            .old_package = Some(old);
        let after = reconcile(
            &base,
            &state,
            &receipts.iter().collect::<Vec<_>>(),
            false,
            &Policy::default(),
            &no_dynamic,
        )
        .unwrap();
        assert_eq!(after.user0.permission_flags(10002, "owner.TEST"), 0);
        Arc::make_mut(&mut base.user0).set_permission_flags(
            10002,
            "owner.TEST",
            LEGACY_GRANTED | IMPLICIT | APP_OP_REVOKED | USER_SET,
        );
        assert_eq!(
            run(&base, &state, false)
                .unwrap()
                .user0
                .permission_flags(10002, "owner.TEST"),
            0
        );
    }

    #[test]
    fn persistence_failure_does_not_publish_access_state() {
        let fixture = PersistedAccess::new();
        fixture.write(
            SYSTEM_ACCESS,
            br#"<access><permissions/><permission-trees/></access>"#,
        );
        fixture.write(
            USER0_ACCESS,
            br#"<access><package-versions/><app-id-permissions/></access>"#,
        );
        let manager = PermissionManager::new(fixture.host.clone());
        let before = manager.retained_access().unwrap();
        let state = state();
        let path = fixture
            .host
            .data()
            .unwrap()
            .join(SYSTEM_ACCESS)
            .with_extension("abx.new");
        std::fs::create_dir(&path).unwrap();
        assert!(
            manager
                .reconcile_boot(
                    &state,
                    &admissions(&state).iter().collect::<Vec<_>>(),
                    false
                )
                .is_err()
        );
        assert!(before.same_base(&manager.retained_access().unwrap()));
        assert!(manager.access_writes.lock().unwrap().is_empty());
    }

    #[test]
    fn restricted_and_role_rules_match_the_retained_java_cases() {
        let check = |name: &str, level, info, target, old, new, expected| {
            let permission = Permission {
                name: name.into(),
                package: "owner".into(),
                protection_level: level,
                dynamic: None,
            };
            assert_eq!(
                evaluate_permission(&permission, info, target, old, new),
                expected,
                "{name}: level={level}, info={info}, target={target}, old={old}, new={new}"
            );
        };
        let granted = RUNTIME_GRANTED | USER_SET;
        for info in [FLAG_HARD_RESTRICTED, FLAG_SOFT_RESTRICTED] {
            let restriction = if info == FLAG_HARD_RESTRICTED {
                RESTRICTION_REVOKED
            } else {
                SOFT_RESTRICTED
            };
            for grant in [0, granted] {
                let denied = grant | restriction;
                check("restricted", 1, info, 36, denied, denied, denied);
                for source in [
                    INSTALLER_EXEMPT,
                    SYSTEM_EXEMPT,
                    UPGRADE_EXEMPT,
                    INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT,
                ] {
                    let exempt = denied | source;
                    check("restricted", 1, info, 36, exempt, exempt, grant | source);
                }
                check("new", 1, info, 36, grant, grant, grant | UPGRADE_EXEMPT);
            }
        }
        for name in [
            "android.permission.READ_EXTERNAL_STORAGE",
            "android.permission.WRITE_EXTERNAL_STORAGE",
        ] {
            let old = granted | SOFT_RESTRICTED;
            check(name, 1, FLAG_SOFT_RESTRICTED, 28, old, old, old);
            check(name, 1, FLAG_SOFT_RESTRICTED, 29, old, old, granted);
        }
        let old = granted | RESTRICTION_REVOKED | SOFT_RESTRICTED;
        check("no longer restricted", 1, 0, 36, old, old, granted);
        for base in [2, 4] {
            for old in [0, ROLE, RUNTIME_GRANTED, ROLE | RUNTIME_GRANTED] {
                check("role", base | PROTECTION_FLAG_ROLE, 0, 36, old, 0, old);
                check(
                    "role with static grant",
                    base | PROTECTION_FLAG_ROLE,
                    0,
                    36,
                    old,
                    PROTECTION_GRANTED,
                    old | PROTECTION_GRANTED,
                );
            }
        }
    }

    #[test]
    fn package_boot_reconciles_without_a_system_process_or_policy_bridge() {
        // A local host with no remote binder/process attached. Exercise the real
        // access owner, persistence and reload, not an injected evaluator.
        for updated in [false, true] {
            for fresh in [false, true] {
                for (level, info, old, expected) in [
                    (
                        1,
                        FLAG_HARD_RESTRICTED,
                        RUNTIME_GRANTED,
                        RUNTIME_GRANTED | UPGRADE_EXEMPT,
                    ),
                    (
                        1,
                        FLAG_HARD_RESTRICTED,
                        RUNTIME_GRANTED | RESTRICTION_REVOKED,
                        RUNTIME_GRANTED | RESTRICTION_REVOKED,
                    ),
                    (
                        1,
                        FLAG_SOFT_RESTRICTED,
                        RUNTIME_GRANTED | SYSTEM_EXEMPT | SOFT_RESTRICTED,
                        RUNTIME_GRANTED | SYSTEM_EXEMPT,
                    ),
                    (
                        2 | PROTECTION_FLAG_ROLE,
                        0,
                        ROLE | RUNTIME_GRANTED,
                        ROLE | RUNTIME_GRANTED,
                    ),
                    (
                        4 | PROTECTION_FLAG_ROLE,
                        0,
                        ROLE | RUNTIME_GRANTED,
                        ROLE | RUNTIME_GRANTED,
                    ),
                ] {
                    let fixture = PersistedAccess::new();
                    if !fresh {
                        fixture.write(SYSTEM_ACCESS, format!("<access><permissions><permission name=\"owner.TEST\" packageName=\"owner\" protectionLevel=\"{level:x}\" type=\"0\"/></permissions><permission-trees/></access>").as_bytes());
                        // Definitions use hex, user flags use decimal in text XML.
                        // An unchanged mount starts with already evaluated flags.
                        let initial = if updated { old } else { expected };
                        fixture.write(USER0_ACCESS, format!("<access><package-versions><package name=\"owner\" version=\"17\"/><package name=\"client\" version=\"17\"/></package-versions><app-id-permissions><app-id id=\"10002\"><permission name=\"owner.TEST\" flags=\"{initial}\"/></app-id></app-id-permissions></access>").as_bytes());
                    }
                    let manager = PermissionManager::new(fixture.host.clone());
                    let mut state = state();
                    let declaration = &mut Arc::make_mut(
                        state
                            .packages
                            .get_mut("owner")
                            .unwrap()
                            .pkg
                            .as_mut()
                            .unwrap(),
                    )
                    .permissions[0];
                    declaration.protection_level = level;
                    declaration.component.flags = info;
                    let receipts = admissions(&state);
                    let after = manager
                        .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), updated)
                        .unwrap();
                    let expected = if fresh {
                        if level == 1 { UPGRADE_EXEMPT } else { 0 }
                    } else {
                        expected
                    };
                    assert_eq!(
                        after.user0.permission_flags(10002, "owner.TEST"),
                        expected,
                        "updated={updated}, fresh={fresh}, level={level}, info={info}"
                    );
                    let reload = PermissionManager::new(fixture.host.clone())
                        .retained_access()
                        .unwrap();
                    assert_eq!(*after.user0, *reload.user0);
                    if !updated && !fresh {
                        assert!(
                            manager.access_writes.lock().unwrap().is_empty(),
                            "unchanged mount wrote access: level={level}, info={info}"
                        );
                    }
                    let writes = manager.access_writes.lock().unwrap().len();
                    let repeat = manager
                        .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), false)
                        .unwrap();
                    assert!(after.same_base(&repeat));
                    assert_eq!(manager.access_writes.lock().unwrap().len(), writes);
                }
            }
        }
    }

    #[test]
    fn update_projects_restricted_and_role_state_through_cold_policy() {
        // Verify the evaluator receives the native snapshot intact and a
        // failed evaluation cannot publish partial flags.
        for (info, level, old, result) in [
            (
                FLAG_HARD_RESTRICTED,
                1,
                RUNTIME_GRANTED | RESTRICTION_REVOKED,
                RUNTIME_GRANTED | RESTRICTION_REVOKED,
            ),
            (
                FLAG_SOFT_RESTRICTED,
                1,
                RUNTIME_GRANTED | SOFT_RESTRICTED,
                RUNTIME_GRANTED | SOFT_RESTRICTED,
            ),
            (
                FLAG_HARD_RESTRICTED,
                1,
                RUNTIME_GRANTED | SYSTEM_EXEMPT,
                RUNTIME_GRANTED | SYSTEM_EXEMPT,
            ),
            (
                FLAG_SOFT_RESTRICTED,
                1,
                RUNTIME_GRANTED | INSTALLER_EXEMPT,
                RUNTIME_GRANTED | INSTALLER_EXEMPT,
            ),
            (
                FLAG_HARD_RESTRICTED,
                1,
                RUNTIME_GRANTED | UPGRADE_EXEMPT,
                RUNTIME_GRANTED | UPGRADE_EXEMPT,
            ),
            (
                0,
                2 | PROTECTION_FLAG_ROLE,
                ROLE | RUNTIME_GRANTED,
                ROLE | RUNTIME_GRANTED,
            ),
            (
                0,
                4 | PROTECTION_FLAG_ROLE,
                ROLE | RUNTIME_GRANTED,
                ROLE | RUNTIME_GRANTED,
            ),
            (0, 4 | PROTECTION_FLAG_ROLE, 0, 0),
        ] {
            let mut state = state();
            let declaration = &mut Arc::make_mut(
                state
                    .packages
                    .get_mut("owner")
                    .unwrap()
                    .pkg
                    .as_mut()
                    .unwrap(),
            )
            .permissions[0];
            declaration.protection_level = level;
            declaration.component.flags = info;
            // Internal role permissions cannot gain a static signature grant,
            // even when their requesting package shares the declaring signature.
            if level & PROTECTION_MASK_BASE == 4 {
                state.packages.get_mut("owner").unwrap().signatures =
                    state.packages["client"].signatures.clone();
            }
            let mut base = base();
            Arc::make_mut(&mut base.system).permissions[0].protection_level = level;
            Arc::make_mut(&mut base.user0).set_permission_flags(10002, "owner.TEST", old);
            let calls = std::cell::Cell::new(0);
            let evaluate = |p: &Permission, flags, target, before, computed| {
                calls.set(calls.get() + 1);
                assert_eq!(
                    (&*p.name, p.protection_level, flags, target, before),
                    ("owner.TEST", level, info, 36, old)
                );
                assert_eq!(computed, if level == 1 { old } else { 0 });
                Ok(result)
            };
            let after = reconcile(
                &base,
                &state,
                &admissions(&state).iter().collect::<Vec<_>>(),
                true,
                &Policy::default(),
                &evaluate,
            )
            .unwrap();
            assert_eq!(calls.get(), 1);
            assert_eq!(after.user0.permission_flags(10002, "owner.TEST"), result);
            assert_eq!(base.user0.permission_flags(10002, "owner.TEST"), old);
            let error = reconcile(
                &base,
                &state,
                &admissions(&state).iter().collect::<Vec<_>>(),
                true,
                &Policy::default(),
                &|_, _, _, _, _| Err(Exception::security("policy failed")),
            )
            .unwrap_err();
            assert_eq!(error.message, "policy failed");
            assert_eq!(base.user0.permission_flags(10002, "owner.TEST"), old);
        }
    }
}
