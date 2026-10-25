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
}
pub(super) struct Candidate {
    scope: Weak<Endpoint>,
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
        &self,
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
                base,
                access,
                installs,
            },
        );
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
            input.read_i32()?; // Auto-revoke mode is consumed by the separate AppOps install owner.
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
            let value = Permission {
                name: name.clone(),
                package: install.name.clone(),
                protection_level: declared.protection_level,
                dynamic: None,
            };
            if let Some(i) = old {
                definitions[i] = value;
            } else {
                definitions.push(value);
            }
            if !declared.tree {
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
                1 => old & !(INSTALL_GRANTED | INSTALL_REVOKED | PROTECTION_GRANTED),
                _ => return Err(unsupported("signature/internal protection policy")),
            };
            user.set_permission_flags(ps.app_id, name, flags);
        }
    }
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
        for ((name, _), inputs) in &mut context.packages {
            inputs.installed_permissions = tree_order(
                self.system
                    .permissions
                    .iter()
                    .filter(|p| &p.package == name)
                    .map(|p| p.name.clone())
                    .collect(),
            );
            for (user, values) in &mut inputs.users {
                let flags = (*user == 0)
                    .then(|| {
                        self.user0
                            .app_id_permissions
                            .iter()
                            .find(|(id, _)| *id == inputs.app_id)
                            .map(|(_, flags)| flags)
                    })
                    .flatten();
                let ps = state.packages.get(name);
                let installed = ps
                    .and_then(|p| p.users.get(user))
                    .is_none_or(|u| u.installed);
                let instant = ps
                    .and_then(|p| p.users.get(user))
                    .is_some_and(|u| u.instant_app);
                values.gids = uid_gids(config, flags, *user);
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
            ..Default::default()
        }
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
            persisted.write(SYSTEM_ACCESS, BOOT_SYSTEM);
            persisted.write(USER0_ACCESS, BOOT_USER);
            let manager = PermissionManager::new(persisted.host.clone());
            let base = manager.retained_access().unwrap();
            let previous = host.package_capture().unwrap();
            publish(&mut |code| {
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
            let receipt = manager
                .prepare_install(binder, old, &installation())
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
            assert!(base.same_base(&manager.retained_access().unwrap()));
            assert!(!receipt.is_empty());
            manager
                .discard_install(binder, &[Some("p".into())])
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
}
