//! Private modern access-policy candidates, android-16.0.0_r1.
//! Adapted from AOSP PermissionService/AppIdPermissionPolicy (Apache-2.0).
use super::*;
use crate::package::{
    installer::permission_capture,
    scan_snapshot::{Snapshot, endpoint::Endpoint, query_state::Capture},
};
use aim_binder_host::parcel::Reader;

struct Install {
    name: String,
    app_id: i32,
    path: String,
    version: i64,
    code: crate::package::pkg::AndroidPackage,
    previous_code: Option<crate::package::pkg::AndroidPackage>,
}
pub(super) struct Candidate {
    scope: Weak<Endpoint>,
    package_base: Arc<Snapshot>,
    base: Arc<permissions::AccessView>,
    access: Arc<permissions::AccessView>,
    installs: Vec<Install>,
}
fn illegal(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
fn unsupported(case: &str) -> Exception {
    Exception::new(
        EX_UNSUPPORTED_OPERATION,
        format!("installer permissions: {case} (#2161)"),
    )
}
impl PermissionManager {
    pub(in crate::app_host) fn prepare_install(
        self: &Arc<Self>,
        candidate: Binder,
        previous: Binder,
        installation: &[u8],
    ) -> Result<Vec<u8>, Exception> {
        let scope =
            permission_capture::local_scope(&self.host.process, candidate).map_err(illegal)?;
        let before = permission_capture::local_scope(&self.host.process, previous)
            .map_err(illegal)?
            .install_capture()
            .map_err(illegal)?;
        let capture = scope.install_capture().map_err(illegal)?;
        let Binder::Local(key) = candidate else {
            unreachable!()
        };
        let mut pending = self.installs.lock().unwrap();
        pending.retain(|_, candidate| {
            candidate
                .scope
                .upgrade()
                .is_some_and(|s| s.install_capture().is_ok())
        });
        if pending.contains_key(&key) {
            return Err(illegal("permission candidate already prepared"));
        }
        let base = self.retained_access()?;
        let installs = decode(&capture, &before, installation)?;
        let access = prepare(&base, capture.state(), &installs)?;
        let config =
            crate::package::system_config::SystemConfig::read(self.host.root()?, &|name| {
                self.host.property(name)
            });
        let mut out = Parcel::new();
        aim_service_aidl::write_int_array(&mut out, Some(&[0]));
        out.write_i32(installs.len() as i32);
        for install in &installs {
            out.write_i32(install.app_id);
            aim_service_aidl::write_byte_array(
                &mut out,
                Some(&access.legacy_state(install.app_id, &[0], &config)?),
            );
        }
        scope
            .pin_install_access(access.clone(), &config)
            .map_err(illegal)?;
        pending.insert(
            key,
            Candidate {
                scope: Arc::downgrade(&scope),
                package_base: before.scan().clone(),
                base,
                access,
                installs,
            },
        );
        let owner = Arc::downgrade(self);
        scope.on_install_retired(Box::new(move || {
            if let Some(owner) = owner.upgrade() {
                owner.installs.lock().unwrap().remove(&key);
            }
        }));
        Ok(out.data().to_vec())
    }
    pub(in crate::app_host) fn discard_install(
        &self,
        candidate: Binder,
        names: &[Option<String>],
    ) -> Result<(), Exception> {
        let scope =
            permission_capture::local_scope(&self.host.process, candidate).map_err(illegal)?;
        let Binder::Local(key) = candidate else {
            unreachable!()
        };
        let mut pending = self.installs.lock().unwrap();
        if let Some(candidate) = pending.get(&key) {
            let expected: BTreeSet<_> = candidate
                .installs
                .iter()
                .map(|i| Some(i.name.clone()))
                .collect();
            if names.iter().cloned().collect::<BTreeSet<_>>() != expected
                || names.len() != expected.len()
            {
                return Err(Exception::illegal_argument(
                    "permission rollback identity differs",
                ));
            }
        }
        pending.remove(&key);
        drop(pending);
        scope.revoke_install_scope();
        Ok(())
    }
}
fn strings(input: &mut Reader<'_>) -> Result<Option<Vec<String>>, i32> {
    let count = input.read_i32()?;
    if count == -1 {
        return Ok(None);
    }
    if count < 0 || count as usize > input.remaining() / 4 {
        return Err(BAD_VALUE);
    }
    (0..count)
        .map(|_| input.read_string16()?.ok_or(BAD_VALUE))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}
fn decode(capture: &Capture, before: &Capture, bytes: &[u8]) -> Result<Vec<Install>, Exception> {
    let decode = || -> Result<_, i32> {
        let mut input = Reader::new(bytes, &[]);
        let factory = input.read_bool()?;
        let count = input.read_i32()?;
        if count <= 0 || count as usize > input.remaining() / 28 {
            return Err(BAD_VALUE);
        }
        let mut records = Vec::new();
        for _ in 0..count {
            let name = input.read_string16()?.ok_or(BAD_VALUE)?;
            let flags = input.read_i32()?;
            let previous = input.read_i32()?;
            let user = input.read_i32()?;
            let names = strings(&mut input)?.ok_or(BAD_VALUE)?;
            let states = aim_service_aidl::read_int_array(&mut input)?.ok_or(BAD_VALUE)?;
            if names.len() != states.len() {
                return Err(BAD_VALUE);
            }
            let allowlist = strings(&mut input)?;
            input.read_i32()?; // The modern PermissionService does not use auto-revoke mode here.
            records.push((name, flags, previous, user, names, allowlist));
        }
        if input.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        Ok((factory, records))
    };
    let (factory, records) =
        decode().map_err(|_| Exception::illegal_argument("invalid permission installation"))?;
    if factory {
        return Err(unsupported("factory restoration"));
    }
    if capture.context().users.keys().copied().collect::<Vec<_>>() != [0] {
        return Err(unsupported("permission users other than user 0"));
    }
    let mut seen = BTreeSet::new();
    let mut installs = Vec::new();
    for (name, flags, previous, user, states, allowlist) in records {
        if !seen.insert(name.clone()) || !matches!(user, -1 | 0) {
            return Err(Exception::illegal_argument(
                "invalid permission installation identity",
            ));
        }
        if flags & 0x100 != 0 || !states.is_empty() {
            return Err(unsupported("installer runtime permission grants"));
        }
        if allowlist.as_ref().is_some_and(|a| !a.is_empty()) {
            return Err(unsupported("restricted permission allowlist"));
        }
        if flags & 0x800 != 0 {
            return Err(unsupported("instant installation"));
        }
        let ps = capture
            .state()
            .packages
            .get(&name)
            .ok_or_else(|| illegal("permission candidate package absent"))?;
        let code = ps
            .pkg
            .as_deref()
            .ok_or_else(|| illegal("permission candidate code absent"))?;
        let old = before.state().packages.get(&name);
        if ps.shared_user.is_some()
            || old.is_some_and(|p| p.shared_user.is_some())
            || previous != -1
        {
            return Err(unsupported("shared UID installation"));
        }
        if ps.is.system {
            return Err(unsupported("system package installation"));
        }
        if old.is_some_and(|p| p.app_id != ps.app_id) {
            return Err(illegal("permission application ID changed"));
        }
        if !code.adopt_permissions.is_empty() {
            return Err(unsupported("permission ownership transfer"));
        }
        if code.target_sdk_version < M || !code.implicit_permissions.is_empty() {
            return Err(unsupported("legacy or implicit runtime permission policy"));
        }
        installs.push(Install {
            name,
            app_id: ps.app_id,
            path: ps.path.clone(),
            version: ps.version_code,
            code: code.clone(),
            previous_code: old.and_then(|p| p.pkg.as_deref()).cloned(),
        });
    }
    Ok(installs)
}
fn prepare(
    base: &permissions::AccessView,
    state: &State,
    installs: &[Install],
) -> Result<Arc<permissions::AccessView>, Exception> {
    let mut system = (*base.system).clone();
    let mut user = (*base.user0).clone();
    let mut changed = BTreeSet::new();
    for install in installs {
        match user
            .package_versions
            .iter_mut()
            .find(|(name, _)| name == &install.name)
        {
            Some((_, version)) if *version < 17 => {
                return Err(unsupported("access package-state upgrade"));
            }
            Some(_) => {}
            None => {
                user.package_versions.push((install.name.clone(), 17));
                user.package_versions = info::array_order(user.package_versions, |(name, _)| name);
            }
        }
        for declared in &install.code.permissions {
            let name = &declared.component.name;
            let tree = system
                .permission_trees
                .iter()
                .find(|p| name.starts_with(&format!("{}.", p.name)));
            if tree.is_some_and(|p| p.package != install.name) {
                continue;
            }
            let definitions = if declared.tree {
                &mut system.permission_trees
            } else {
                &mut system.permissions
            };
            let old = definitions.iter().position(|p| p.name == *name);
            if old.is_some_and(|i| definitions[i].package != install.name) {
                return Err(unsupported("permission ownership transfer"));
            }
            let old_declaration = install
                .previous_code
                .as_ref()
                .and_then(|code| code.permissions.iter().find(|p| p.component.name == *name));
            let reset = old_declaration.is_some_and(|old| {
                let runtime =
                    declared.protection_level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS;
                (runtime && old.protection_level & PROTECTION_MASK_BASE != PROTECTION_DANGEROUS)
                    || (runtime && declared.group.is_some() && declared.group != old.group)
                    || (declared.protection_level & PROTECTION_MASK_BASE == 4
                        && old.protection_level & PROTECTION_MASK_BASE != 4)
            });
            if reset {
                for (_, flags) in &mut user.app_id_permissions {
                    flags.retain(|(permission, _)| permission != name);
                }
            }
            let value = Permission {
                name: name.clone(),
                package: install.name.clone(),
                protection_level: declared.protection_level,
                dynamic: None,
            };
            let differs = old.is_none_or(|i| definitions[i] != value);
            if let Some(i) = old {
                definitions[i] = value;
            } else {
                definitions.push(value);
            }
            if !declared.tree && (differs || reset) {
                changed.insert(name.clone());
            }
        }
        for definitions in [&mut system.permissions, &mut system.permission_trees] {
            definitions.retain(|p| {
                let keep = p.package != install.name
                    || p.dynamic.is_some()
                    || install
                        .code
                        .permissions
                        .iter()
                        .any(|d| d.component.name == p.name);
                if !keep {
                    changed.insert(p.name.clone());
                    for (_, flags) in &mut user.app_id_permissions {
                        flags.retain(|(name, _)| name != &p.name);
                    }
                }
                keep
            });
        }
    }
    for ps in state.packages.values() {
        let Some(code) = ps.pkg.as_deref() else {
            continue;
        };
        let installing = installs.iter().any(|i| i.name == ps.name);
        let requested: BTreeSet<_> = code
            .requested_permissions
            .iter()
            .map(String::as_str)
            .collect();
        if installing {
            if let Some((_, flags)) = user
                .app_id_permissions
                .iter_mut()
                .find(|(id, _)| *id == ps.app_id)
            {
                flags.retain(|(name, _)| requested.contains(name.as_str()));
            }
        }
        for name in &code.requested_permissions {
            if !installing && !changed.contains(name) {
                continue;
            }
            if ps.shared_user.is_some() {
                return Err(unsupported("shared UID permission reevaluation"));
            }
            let old = user.permission_flags(ps.app_id, name);
            let Some(permission) = system.permissions.iter().find(|p| p.name == *name) else {
                if old == 0 {
                    user.set_permission_flags(ps.app_id, name, INSTALL_REVOKED);
                }
                continue;
            };
            let declared = state
                .packages
                .get(&permission.package)
                .and_then(|p| p.pkg.as_deref())
                .and_then(|p| p.permissions.iter().find(|p| p.component.name == *name));
            if declared.is_some_and(|p| {
                p.component.flags & (FLAG_HARD_RESTRICTED | FLAG_SOFT_RESTRICTED) != 0
            }) {
                return Err(unsupported("restricted permission policy"));
            }
            if is_role(permission) {
                return Err(unsupported("role permission grants"));
            }
            let flags = match permission.protection_level & PROTECTION_MASK_BASE {
                0 => {
                    if old & INSTALL_GRANTED != 0
                        || old & INSTALL_REVOKED == 0
                        || installing
                        || ps.is.system
                    {
                        INSTALL_GRANTED
                    } else {
                        INSTALL_REVOKED
                    }
                }
                1 => {
                    if old & (LEGACY_GRANTED | IMPLICIT_GRANTED | IMPLICIT) != 0 {
                        return Err(unsupported("legacy or implicit runtime permission policy"));
                    }
                    old & (((1 << 24) - 1)
                        & !(INSTALL_GRANTED | INSTALL_REVOKED | PROTECTION_GRANTED))
                }
                _ => return Err(unsupported("signature/internal protection policy")),
            };
            let flags = if permission.protection_level & 0x40 != 0 && !is_runtime(permission) {
                flags | (old & (ROLE | USER_SET))
            } else {
                flags
            };
            user.set_permission_flags(ps.app_id, name, flags);
        }
    }
    system.permissions = info::array_order(system.permissions, |p| &p.name);
    system.permission_trees = info::array_order(system.permission_trees, |p| &p.name);
    Ok(Arc::new(permissions::AccessView {
        system: Arc::new(system),
        user0: Arc::new(user),
    }))
}
impl permissions::AccessView {
    pub(crate) fn legacy_state(
        &self,
        app_id: i32,
        users: &[i32],
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Vec<u8>, Exception> {
        validate_legacy_identity(app_id, users)?;
        let flags = self
            .user0
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == app_id)
            .map(|(_, flags)| flags);
        Ok(legacy_state_record(app_id, users, flags, |name| {
            self.system
                .permissions
                .iter()
                .find(|p| p.name == name)
                .map(is_runtime)
                .or_else(|| config.permissions.contains_key(name).then_some(false))
        }))
    }
}
impl permissions::AccessView {
    pub(crate) fn project_context(
        &self,
        context: &mut crate::package::scan_snapshot::query_state::Context,
        state: &State,
        config: &crate::package::system_config::SystemConfig,
    ) {
        for ((name, factory), inputs) in &mut context.packages {
            inputs.installed_permissions = tree_order(
                self.system
                    .permissions
                    .iter()
                    .filter(|p| &p.package == name)
                    .map(|p| p.name.clone())
                    .collect(),
            );
            let ps = if *factory {
                state.disabled_system_packages.get(name)
            } else {
                state.packages.get(name)
            };
            let permission_id = ps
                .and_then(|ps| ps.shared_user_app_id)
                .unwrap_or(inputs.app_id);
            for (user, values) in &mut inputs.users {
                let flags = (*user == 0)
                    .then(|| {
                        self.user0
                            .app_id_permissions
                            .iter()
                            .find(|(id, _)| *id == permission_id)
                            .map(|(_, flags)| flags)
                    })
                    .flatten();
                let installed = ps
                    .and_then(|p| p.users.get(user))
                    .is_none_or(|u| u.installed);
                let instant = ps
                    .and_then(|p| p.users.get(user))
                    .is_some_and(|u| u.instant_app);
                values.gids = if permission_id < 0 {
                    Vec::new()
                } else {
                    uid_gids(config, flags, *user)
                };
                values.granted_permissions = if installed {
                    tree_order(
                        flags
                            .into_iter()
                            .flatten()
                            .filter(|(name, flag)| {
                                single_permission_granted(
                                    &self.system.permissions,
                                    *flag,
                                    instant,
                                    name,
                                )
                            })
                            .map(|(name, _)| name.clone())
                            .collect(),
                    )
                } else {
                    Vec::new()
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{BOOT_SYSTEM, BOOT_USER, PersistedAccess};
    use super::*;
    use crate::package::pkg::{Component, Permission as ParsedPermission};
    fn declaration(name: &str, tree: bool) -> ParsedPermission {
        ParsedPermission {
            component: Component {
                name: name.into(),
                package_name: "p".into(),
                ..Default::default()
            },
            tree,
            group: (!tree).then(|| "p.GROUP".into()),
            ..Default::default()
        }
    }
    fn seed(persisted: &PersistedAccess) {
        persisted.write(SYSTEM_ACCESS, BOOT_SYSTEM);
        persisted.write(USER0_ACCESS, BOOT_USER);
        let path = persisted
            .host
            .root()
            .unwrap()
            .join("system/etc/permissions/platform.xml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, br#"<permissions><permission name="android.permission.INTERNET"><group gid="3003"/></permission></permissions>"#).unwrap();
    }
    fn installation() -> Vec<u8> {
        let mut p = Parcel::new();
        p.write_bool(false);
        p.write_i32(1);
        p.write_string16(Some("p"));
        p.write_i32(0x400000);
        p.write_i32(-1);
        p.write_i32(0);
        p.write_i32(0);
        aim_service_aidl::write_int_array(&mut p, Some(&[]));
        p.write_i32(-1);
        p.write_i32(3);
        p.data().to_vec()
    }
    #[test]
    fn prepare_pins_private_definitions_and_install_grants_and_rollback_discards_them() {
        crate::system::with_discovery_session(|host, publish, _| {
            let persisted = PersistedAccess::new();
            seed(&persisted);
            let manager = Arc::new(PermissionManager::new(persisted.host.clone()));
            let base = manager.retained_access().unwrap();
            let previous = host.package_capture().unwrap();
            publish(&mut |code| {
                if code.package_name != "p" {
                    return;
                }
                code.permission_groups = vec![crate::package::pkg::PermissionGroup {
                    component: Component {
                        name: "p.GROUP".into(),
                        package_name: "p".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }];
                code.target_sdk_version = 36;
                code.permissions =
                    vec![declaration("p.NORMAL", false), declaration("p.tree", true)];
                code.requested_permissions = vec![
                    "p.NORMAL".into(),
                    "android.permission.INTERNET".into(),
                    "android.permission.CAMERA".into(),
                ];
                code.implicit_permissions.clear();
            });
            let capture = host.package_capture().unwrap();
            let process = &persisted.host.process;
            let endpoint = Arc::new(Endpoint::with_computer(
                capture.scan().clone(),
                capture.clone(),
                process,
            ));
            let before = Arc::new(Endpoint::with_computer(
                previous.scan().clone(),
                previous.clone(),
                process,
            ));
            let binder = permission_capture::bind(process, &endpoint);
            let old = permission_capture::bind(process, &before);
            use aim_service_aidl::dev_aim_server_iinstallerpermissionbridge as api;
            assert!(
                persisted
                    .host
                    .permission_manager
                    .set(manager.clone())
                    .is_ok()
            );
            let bridge = process.add_service(Arc::new(
                crate::app_host::installer_permissions::InstallerPermissions::new(
                    persisted.host.clone(),
                ),
            ));
            let Binder::Local(ptr) = bridge else {
                unreachable!()
            };
            let bridge = process.local_service(ptr).unwrap();
            let mut request = Parcel::new();
            api::Prepare {
                candidate: Some(binder),
                previous: Some(old),
                installation: Some(installation()),
                cross_user_suspensions: false,
            }
            .write(&mut request);
            let reply = bridge.transact(api::PREPARE, &request, false).unwrap();
            let receipt = api::read_prepare_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
                .unwrap();
            let view = endpoint.install_capture().unwrap();
            let access = view.context().access.as_ref().unwrap();
            let id = capture.state().packages["p"].app_id;
            assert!(
                access
                    .system
                    .permissions
                    .iter()
                    .any(|p| p.name == "p.NORMAL")
            );
            assert!(
                access
                    .system
                    .permission_trees
                    .iter()
                    .any(|p| p.name == "p.tree")
            );
            assert_eq!(
                access.user0.permission_flags(id, "p.NORMAL"),
                INSTALL_GRANTED
            );
            assert_eq!(
                access
                    .user0
                    .permission_flags(id, "android.permission.INTERNET"),
                INSTALL_GRANTED
            );
            assert_eq!(
                access
                    .user0
                    .permission_flags(id, "android.permission.CAMERA"),
                0
            );
            assert!(
                view.state().packages["p"].users[&0]
                    .granted_permissions
                    .contains(&"p.NORMAL".into())
            );
            assert_eq!(view.state().packages["p"].users[&0].gids, [3003]);
            assert!(
                permission_groups(view.state())
                    .iter()
                    .any(|g| g.item.name.as_deref() == Some("p.GROUP"))
            );
            assert!(base.same_base(&manager.retained_access().unwrap()));
            assert!(!receipt.is_empty());
            let mut request = Parcel::new();
            api::Rollback {
                candidate: Some(binder),
                current: Some(old),
                package_names: Some(vec![Some("p".into())]),
                cross_user_suspensions: false,
            }
            .write(&mut request);
            let reply = bridge.transact(api::ROLLBACK, &request, false).unwrap();
            api::read_rollback_reply(&mut reply.reader())
                .unwrap()
                .unwrap();
            assert!(endpoint.install_capture().is_err());
            assert!(manager.installs.lock().unwrap().is_empty());
            assert!(base.same_base(&manager.retained_access().unwrap()));
            assert!(
                access
                    .system
                    .permissions
                    .iter()
                    .any(|p| p.name == "p.NORMAL")
            );
        });
    }
    #[test]
    fn install_publication_commits_the_owner_or_retains_the_last_complete_capture() {
        for case in ["commit", "stale", "access-write", "capture"] {
            crate::system::with_discovery_session(|host, publish, _| {
                let persisted = PersistedAccess::new();
                seed(&persisted);
                let manager = Arc::new(PermissionManager::new(persisted.host.clone()));
                host.package_system()
                    .unwrap()
                    .set_native_permissions(&manager);
                publish(&mut |code| {
                    if code.package_name != "p" {
                        return;
                    }
                    code.permission_groups = vec![crate::package::pkg::PermissionGroup {
                        component: Component {
                            name: "p.GROUP".into(),
                            package_name: "p".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }];
                    code.target_sdk_version = 36;
                    code.permissions =
                        vec![declaration("p.NORMAL", false), declaration("p.tree", true)];
                    code.requested_permissions = vec![
                        "p.NORMAL".into(),
                        "android.permission.INTERNET".into(),
                        "android.permission.CAMERA".into(),
                    ];
                    code.implicit_permissions.clear();
                });
                let prepare = || {
                    let capture = host.package_capture().unwrap();
                    let process = &persisted.host.process;
                    let endpoint = Arc::new(Endpoint::with_computer(
                        capture.scan().clone(),
                        capture.clone(),
                        process,
                    ));
                    let before = Arc::new(Endpoint::with_computer(
                        capture.scan().clone(),
                        capture.clone(),
                        process,
                    ));
                    let binder = permission_capture::bind(process, &endpoint);
                    let old = permission_capture::bind(process, &before);
                    manager
                        .prepare_install(binder, old, &installation())
                        .unwrap();
                    endpoint
                };
                let endpoint = prepare();
                let last = host.package_capture().unwrap();
                let base = manager.retained_access().unwrap();
                let documents = || {
                    [SYSTEM_ACCESS, USER0_ACCESS].map(|path| {
                        std::fs::read(persisted.host.data().unwrap().join(path)).unwrap()
                    })
                };
                let before_files = documents();
                let run = |scope: Arc<Endpoint>, fail_capture: bool| {
                    let source = Arc::new(move |scan: &Arc<Snapshot>| {
                        if fail_capture {
                            return Err("injected query capture failure".into());
                        }
                        let mut context = (**scope.install_capture()?.context()).clone();
                        context.scan_version = scan.version();
                        Ok(context)
                    });
                    (crate::system::install_publication(host, source))(
                        host.package_capture().unwrap().scan(),
                    )
                };
                match case {
                    "stale" => {
                        let mut after = (*base.user0).clone();
                        after.set_permission_flags(
                            10999,
                            "android.permission.CAMERA",
                            RUNTIME_GRANTED | USER_SET,
                        );
                        manager.write_user0(&after).unwrap();
                        manager.owner.lock().unwrap().user0 = Some(Arc::new(after));
                        assert!(
                            run(endpoint.clone(), false)
                                .unwrap_err()
                                .contains("reprepare")
                        );
                        assert_eq!(
                            manager
                                .retained_access()
                                .unwrap()
                                .user0
                                .permission_flags(10999, "android.permission.CAMERA"),
                            RUNTIME_GRANTED | USER_SET
                        );
                        assert_eq!(documents()[0], before_files[0]);
                        assert!(Arc::ptr_eq(&last, &host.package_capture().unwrap()));
                    }
                    "access-write" => {
                        std::fs::create_dir(
                            persisted
                                .host
                                .data()
                                .unwrap()
                                .join(USER0_ACCESS)
                                .with_extension("abx.new"),
                        )
                        .unwrap();
                        assert!(run(endpoint.clone(), false).is_err());
                        assert!(Arc::ptr_eq(&last, &host.package_capture().unwrap()));
                        assert_eq!(documents()[1], before_files[1]);
                        assert!(
                            !manager
                                .retained_access()
                                .unwrap()
                                .system
                                .permissions
                                .iter()
                                .all(|p| p.name != "p.NORMAL")
                        );
                    }
                    "capture" => {
                        assert!(
                            run(endpoint.clone(), true)
                                .unwrap_err()
                                .contains("injected")
                        );
                        assert!(Arc::ptr_eq(&last, &host.package_capture().unwrap()));
                        assert!(base.same_base(&manager.retained_access().unwrap()));
                        assert_eq!(documents(), before_files);
                    }
                    "commit" => {
                        run(endpoint.clone(), false).unwrap();
                        let first = host.package_capture().unwrap();
                        let access = first.context().access.as_ref().unwrap();
                        assert!(access.same_base(&manager.retained_access().unwrap()));
                        assert_eq!(first.state().packages["p"].users[&0].gids, [3003]);
                        let id = first.state().packages["p"].app_id;
                        assert!(
                            first.state().packages["p"]
                                .installed_permissions
                                .contains(&"p.NORMAL".into())
                        );
                        assert_eq!(
                            access.user0.permission_flags(id, "p.NORMAL"),
                            INSTALL_GRANTED
                        );
                        assert_eq!(
                            access
                                .user0
                                .permission_flags(id, "android.permission.CAMERA"),
                            0
                        );
                        endpoint.revoke_install_scope();
                        assert!(manager.installs.lock().unwrap().is_empty());
                        assert!(access.same_base(&manager.retained_access().unwrap()));
                        publish(&mut |code| {
                            if code.package_name != "p" {
                                return;
                            }
                            code.permissions.push(declaration("p.NEW", false));
                            code.requested_permissions.push("p.NEW".into());
                        });
                        let updated = prepare();
                        run(updated.clone(), false).unwrap();
                        let second = host.package_capture().unwrap();
                        assert!(
                            second.state().packages["p"].users[&0]
                                .granted_permissions
                                .contains(&"p.NEW".into())
                        );
                        assert!(!access.system.permissions.iter().any(|p| p.name == "p.NEW"));
                        let reload = PermissionManager::new(persisted.host.clone())
                            .retained_access()
                            .unwrap();
                        assert_eq!(*reload.system, *manager.retained_access().unwrap().system);
                        assert_eq!(*reload.user0, *manager.retained_access().unwrap().user0);
                        assert_eq!(
                            reload
                                .user0
                                .package_versions
                                .iter()
                                .find(|(name, _)| name == "p")
                                .unwrap()
                                .1,
                            17
                        );
                        updated.revoke_install_scope();
                    }
                    _ => unreachable!(),
                }
                endpoint.revoke_install_scope();
                assert!(manager.installs.lock().unwrap().is_empty());
            });
        }
    }
}
impl Candidate {
    fn matches(&self, scan: &Snapshot) -> bool {
        self.package_base.same_lineage(scan)
            && self
                .scope
                .upgrade()
                .is_some_and(|scope| scope.install_capture().is_ok())
            && self.installs.iter().all(|install| {
                scan.owner().settings.packages.iter().any(|p| {
                    p.name == install.name
                        && p.app_id == install.app_id
                        && p.code_path == install.path
                        && p.version_code == install.version
                }) && scan
                    .owner()
                    .loaded_packages()
                    .get(&install.name)
                    .is_some_and(|code| code.runtime_package() == &install.code)
                    && scan
                        .owner()
                        .scanned_user_states(&install.name)
                        .is_some_and(|users| users.keys().copied().collect::<Vec<_>>() == [0])
            })
    }
}
impl PermissionManager {
    pub(crate) fn install_access(
        &self,
        scan: &Snapshot,
    ) -> Result<Arc<permissions::AccessView>, Exception> {
        let pending = self.installs.lock().unwrap();
        let mut candidates = pending.values().filter(|candidate| candidate.matches(scan));
        let first = candidates.next();
        if candidates.next().is_some() {
            return Err(illegal("ambiguous permission install candidate"));
        }
        match first {
            Some(candidate) => Ok(candidate.access.clone()),
            None => self.retained_access(),
        }
    }
    /// Validate the access view while publishing under the package owner lock.
    /// The callback makes no owner RPC.
    pub(crate) fn if_current_access<T>(
        &self,
        view: &permissions::AccessView,
        publish: impl FnOnce() -> T,
    ) -> Option<T> {
        let owner = self.owner.lock().unwrap();
        let current = permissions::AccessView {
            system: owner.system.clone()?,
            user0: owner.user0.clone()?,
        };
        view.same_base(&current).then(publish)
    }
    pub(crate) fn commit_install<T>(
        &self,
        scan: &Snapshot,
        view: &Arc<permissions::AccessView>,
        publish: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let mut pending = self.installs.lock().unwrap();
        let mut candidates = pending
            .iter()
            .filter(|(_, candidate)| candidate.matches(scan));
        let (&key, candidate) = candidates
            .next()
            .ok_or("permission install candidate unavailable")?;
        if candidates.next().is_some() || !Arc::ptr_eq(&candidate.access, view) {
            return Err("permission install candidate differs".into());
        }
        let mut owner = self.owner.lock().unwrap();
        let current = permissions::AccessView {
            system: owner.system.clone().ok_or("access system unavailable")?,
            user0: owner.user0.clone().ok_or("access user unavailable")?,
        };
        if !candidate.base.same_base(&current) {
            return Err("stale permission access base; reprepare required (#2161)".into());
        }
        if *current.system != *view.system {
            self.write_system(&view.system).map_err(|e| e.message)?;
        }
        owner.system = Some(view.system.clone());
        if *current.user0 != *view.user0 {
            self.write_user0(&view.user0).map_err(|e| e.message)?;
        }
        owner.user0 = Some(view.user0.clone());
        pending.remove(&key);
        publish()
    }
}
impl permissions::AccessView {
    pub(crate) fn gids(
        &self,
        app_id: i32,
        user: i32,
        config: &crate::package::system_config::SystemConfig,
    ) -> Vec<i32> {
        let flags = (user == 0)
            .then(|| {
                self.user0
                    .app_id_permissions
                    .iter()
                    .find(|(id, _)| *id == app_id)
                    .map(|(_, flags)| flags)
            })
            .flatten();
        uid_gids(config, flags, user)
    }
    pub(crate) fn installed_permissions(&self, package: &str) -> Vec<String> {
        tree_order(
            self.system
                .permissions
                .iter()
                .filter(|p| p.package == package)
                .map(|p| p.name.clone())
                .collect(),
        )
    }
}
