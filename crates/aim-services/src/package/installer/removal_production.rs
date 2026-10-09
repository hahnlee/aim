//! Production binding for the existing native deletion/archive coordinator.
use super::{Controller, External, NativeStore, Plan, Policy, PolicySource, RestoreFactory};
use crate::package::apps_filter;
use aim_binder_host::{
    local::LocalProcess,
    parcel::{EX_ILLEGAL_STATE, Exception},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

/// Values selected by the native bootstrap's real package/resource/flag owners.
pub struct Roles {
    pub emergency_installer: Option<String>,
    pub system_protection: Option<String>,
    pub verifier_packages: Vec<String>,
    pub uninstaller: Option<String>,
    pub storage_manager: Option<String>,
    pub keep_uninstalled: BTreeSet<String>,
    pub sdk_library_independence: bool,
}
pub struct Inputs {
    pub store: Arc<NativeStore>,
    pub source: super::super::native::QuerySource,
    pub process: Arc<LocalProcess>,
    pub effects: Arc<crate::package::effects::Owner>,
    pub external: Arc<External>,
    pub roles: Arc<Roles>,
    pub user_policy: super::super::native::PolicySource,
    pub gate: Arc<Mutex<()>>,
    pub preferred_removal: super::PreferredRemoval,
    pub factory: Arc<Factory>,
    pub keystore: Arc<crate::package::owner::keystore::KeystoreCleanup>,
}
pub fn build(inputs: Inputs) -> Result<Arc<Controller>, Exception> {
    let base = inputs.store.snapshots.capture();
    inputs
        .store
        .disk
        .lock()
        .unwrap()
        .validate_committed_scan(base.owner())
        .map_err(|error| fail(error.to_string()))?;
    if (inputs.source)()?.generation != base.version() {
        return Err(fail("Removal constructor query/scan generation differs"));
    }
    let external = inputs.external.clone();
    let effects = inputs.effects.clone();
    let roles = inputs.roles.clone();
    let user_policy = inputs.user_policy;
    let policy: PolicySource = Arc::new(move |query, request| {
        let device = user_policy(request.uid, request.pid, request.user)?;
        let users = external.users()?;
        if users.iter().any(|user| *user < 0)
            || users.iter().collect::<BTreeSet<_>>().len() != users.len()
        {
            return Err(fail("Removal UserManager inventory malformed"));
        }
        let mut child_users = BTreeMap::new();
        let mut admins = BTreeSet::new();
        let mut protected = BTreeSet::new();
        let mut restricted = BTreeSet::new();
        let mut targets = users.clone();
        if !targets.contains(&request.user) {
            targets.push(request.user);
        }
        for user in targets {
            child_users.insert(user, external.children(user)?);
            if !request.package.is_empty() {
                if external.call(
                    super::external_api::HAS_ACTIVE_ADMIN,
                    |p| {
                        p.write_string16(Some(&request.package));
                        p.write_i32(user);
                    },
                    |reader| reader.read_bool(),
                )? {
                    admins.insert(user);
                }
                if effects.state_protected(&request.package, user)? {
                    protected.insert(user);
                }
            }
            if external.restricted(user)? {
                restricted.insert(user);
            }
        }
        let same = |name: Option<&str>| {
            apps_filter::is_caller_same_app(query.state, name, query.calling_uid)
                .map_err(|error| fail(error.0))
        };
        let caller = request.caller_package.as_deref();
        Ok(Policy {
            device,
            can_silently_install: external.silent(request.uid, caller)?,
            emergency_installer: same(roles.emergency_installer.as_deref())?,
            system_protection_role: same(roles.system_protection.as_deref())?,
            pinned: !request.package.is_empty() && external.pinned(&request.package)?,
            admins,
            protected,
            uninstall_restricted: restricted,
            users,
            child_users,
            verifier_packages: roles.verifier_packages.clone(),
            uninstaller_package: roles.uninstaller.clone(),
            storage_manager_package: roles.storage_manager.clone(),
            keep_uninstalled: roles.keep_uninstalled.contains(&request.package),
            sdk_library_independence: roles.sdk_library_independence,
        })
    });
    let factory = inputs.factory;
    let restore: RestoreFactory = Arc::new(move |plan| factory.restore(plan));
    let process = inputs.process.clone();
    Ok(Arc::new(Controller {
        store: inputs.store,
        source: inputs.source,
        resolver: Default::default(),
        effects: inputs.effects,
        external: inputs.external,
        policy,
        publisher: Arc::new(move |service| Ok(process.add_service(service))),
        factory_restore: restore,
        preferred_removal: inputs.preferred_removal,
        gate: inputs.gate,
        keystore: inputs.keystore,
    }))
}

pub struct Factory {
    pub store: Arc<NativeStore>,
    pub image: Arc<super::super::environment::Config>,
    pub resources: Arc<crate::package::owner::resources::CodeResources>,
    pub bridge: Arc<crate::package::bootstrap::Bridge>,
    pub external: Arc<External>,
    pub effects: Arc<crate::package::effects::Owner>,
    pub page_size: u64,
    pub compat_16kb_disabled: bool,
    pub preferred_removal: super::PreferredRemoval,
}
impl Factory {
    pub fn restore(&self, plan: &Plan) -> Result<super::FactoryEffects, Exception> {
        use crate::package::{pkg::booleans, scan};
        let mut base = self.store.snapshots.capture();
        let active = base
            .owner()
            .settings
            .packages
            .iter()
            .find(|package| package.name == plan.internal_package)
            .cloned()
            .ok_or_else(|| fail("System update setting absent"))?;
        let factory = base
            .owner()
            .settings
            .disabled_system_packages
            .iter()
            .find(|package| package.name == plan.internal_package)
            .cloned()
            .ok_or_else(|| fail("Disabled factory setting absent"))?;
        let old_users = base
            .owner()
            .scanned_user_states(&active.name)
            .cloned()
            .ok_or_else(|| fail("System update user owner absent"))?;
        let apexes = if factory.code_path.starts_with("/apex/") {
            self.bridge.apex_inventory()
                .map_err(|error| fail(format!("Factory APEX inventory: {error:?}")))?
                .scan_apexes()
        } else {
            Vec::new()
        };
        let location = system_location(&factory.code_path, &apexes)?;
        let parsed = self.image.apks
            .parsed_path(&factory.code_path, location.parse_flags())
            .map_err(fail)?;
        let signing = self.image.apks.signing_details(&parsed).map_err(fail)?;
        let code = scan::Code {
            location,
            parsed,
            signing,
        };
        let wipe = factory.version_code < active.version_code || factory.app_id != active.app_id;
        let preferred = (self.preferred_removal)(&active.name, -1, !wipe)?;
        base = self.store.snapshots.capture();
        let permission = self
            .external
            .prepare_permissions(&active.name, active.app_id, -1)?;
        for (user, state) in &old_users {
            if !state.installed {
                continue;
            }
            self.effects.kill(
                &active.name,
                active.app_id,
                *user,
                "uninstall system update",
                10,
            )?;
            if wipe {
                self.external.destroy_data(
                    active.volume_uuid.as_deref(),
                    &active.name,
                    *user,
                    state.ce_data_inode,
                )?;
            }
        }
        self.external.destroy_profiles(&active.name)?;
        self.resources
            .clean(&active.code_path, false)
            .map_err(fail)?;
        let domain = self
            .bridge
            .new_domain_id()
            .map_err(|error| fail(format!("Factory domain identity: {error:?}")))?;
        let mut next = base.owner().clone();
        next.withdraw_for_live_factory(&active.name).map_err(fail)?;
        let restored = next
            .enable_system_setting(&active.name, domain)
            .ok_or_else(|| fail("Factory UID reservation failed"))?;
        let host = (self.image.apks.files)(&factory.code_path)
            .ok_or_else(|| fail("Factory VFS mapping unavailable"))?;
        let native = scan::NativeLibraryEnvironment {
            preferred_abi: &self.image.preferred_abi,
            app_lib32_install_dir: &self.image.app_lib32_dir,
            code_is_directory: host.is_dir(),
            canonical_source: None,
        };
        let saved = [(active.name.clone(), old_users.clone())].into();
        let update = scan::SettingUpdate {
            code_path: factory.code_path.clone(),
            legacy_native_library_path: factory.legacy_native_library_path.clone(),
            primary_cpu_abi: factory.primary_cpu_abi.clone(),
            secondary_cpu_abi: factory.secondary_cpu_abi.clone(),
            flags: factory.flags,
            private_flags: factory.private_flags,
            uses_sdk_libraries: factory.uses_sdk_libraries.clone(),
            uses_static_libraries: factory.uses_static_libraries.clone(),
            mime_groups: factory
                .mime_groups
                .iter()
                .filter_map(|(name, _)| name.clone())
                .collect(),
            domain_set_id: domain,
            target_sdk_version: factory.target_sdk_version,
            restrict_update_hash: factory.restrict_update_hash.clone(),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_millis() as i64;
        let metadata = next
            .scan_existing(
                &code,
                update,
                &saved,
                Some(&self.image.users),
                None,
                &self.image.apks,
                scan::ScanMetadataCompletion {
                    seinfo: scan::SeInfoScan {
                        policy: &self.image.seinfo,
                        compatibility: self.image.compatibility.as_ref(),
                    },
                    abi_policy: &self.image.abi,
                    native_environment: &native,
                    context: scan::AbiScanContext {
                        mode: scan::AbiScanMode::Existing {
                            first_boot_or_upgrade: false,
                            old_was_stub: false,
                            saved: Some(&restored),
                        },
                        system: true,
                        updated: false,
                        override_abi: restored.cpu_abi_override.as_deref(),
                        platform_runtime_64bit: None,
                    },
                    install: scan::NativeLibraryInstallPolicy {
                        page_size: self.page_size,
                        extract: code.parsed.is(booleans::EXTRACT_NATIVE_LIBS),
                        debuggable: code.parsed.is(booleans::DEBUGGABLE),
                        compat_16kb_disabled: self.compat_16kb_disabled,
                        manifest_compat_disabled: code.parsed.page_size_app_compat_flags == 64,
                    },
                    destination: None,
                    clock: scan::ScanClock {
                        current_time: now,
                        user_id: plan.request.user,
                        update_time: true,
                    },
                    factory_test: self.image.factory_test,
                    scan_as_instant_app: old_users
                        .get(&plan.request.user)
                        .is_some_and(|state| state.instant_app),
                },
            )
            .map_err(|error| fail(format!("Factory native re-admission: {error:?}")))?;
        let mut admission = scan::live_install::CompletedAdmission {
            owner: next,
            completed: vec![metadata],
        };
        (self.image.permissions)(&mut admission, &base)?;
        let mut data = PreparedData {
            image: self.image.clone(),
            created: Vec::new(),
            committed: false,
        };
        for completed in &mut admission.completed {
            let setting = &completed.candidate.record.settings;
            let label = admission
                .owner
                .seinfo(&setting.name)
                .map_err(fail)?
                .map(str::to_owned);
            for (user, state) in &mut completed.candidate.users {
                if !state.installed {
                    continue;
                }
                let flags = (self.image.app_data_flags)(*user)?
                    | if setting.uses_sdk_libraries.is_empty() {
                        0
                    } else {
                        8
                    };
                if flags & 3 == 0 {
                    continue;
                }
                let app_data = crate::package::users::AppData {
                    volume_uuid: setting.volume_uuid.clone(),
                    package: setting.name.clone(),
                    user: *user,
                    flags,
                    app_id: setting.app_id,
                    seinfo: label.as_ref().map(|base| {
                        format!(
                            "{}{}:complete",
                            base,
                            if state.instant_app {
                                ":ephemeralapp"
                            } else {
                                ""
                            }
                        )
                    }),
                    target_sdk_version: setting.target_sdk_version,
                };
                let result = (self.image.app_data)(app_data)?;
                if result.newly_created {
                    data.created
                        .push((setting.name.clone(), *user, result.ce_inode));
                }
                if flags & 2 != 0 && result.ce_inode != -1 {
                    state.ce_data_inode = result.ce_inode;
                }
                if flags & 1 != 0 && result.de_inode != -1 {
                    state.de_data_inode = result.de_inode;
                }
                admission
                    .owner
                    .set_user_state(&setting.name, *user, state.clone())
                    .map_err(fail)?;
            }
        }
        let users = self
            .image
            .users
            .iter()
            .map(|user| user.id as u32)
            .collect::<Vec<_>>();
        let mut disk = self.store.disk.lock().unwrap();
        let result = admission.persist_publish(
            &self.store.snapshots,
            &base,
            &mut disk,
            base.usage().clone(),
            &users,
            self.image.cross_user_suspensions,
        );
        drop(disk);
        let snapshot = match result {
            Ok(publication) => publication.snapshot,
            Err(crate::package::scan::live_install::PublicationFailure::Persist {
                error:
                    crate::package::scan_snapshot::CommitError::Disk {
                        snapshot: Some(next),
                        error,
                    },
                ..
            }) => {
                data.committed = true;
                (self.store.publish)(&base, &next)?;
                data.commit()?;
                return Err(fail(format!(
                    "Factory restoration committed: {}",
                    error.message
                )));
            }
            Err(failure) => {
                return Err(fail(format!(
                    "Factory persistence: {}",
                    publication_error(failure)
                )));
            }
        };
        data.committed = true;
        (self.store.publish)(&base, &snapshot)?;
        data.commit()?;
        self.external.apply_permissions(permission, true, false)?;
        let mut effects = vec![preferred];
        for user in &self.image.users {
            if snapshot
                .owner()
                .scanned_user_states(&active.name)
                .and_then(|states| states.get(&user.id))
                .is_none_or(|state| !state.installed)
            {
                continue;
            }
            effects.push(super::PostCommitEffect::Added {
                owner: self.effects.clone(),
                package: active.name.clone(),
                user: user.id,
            });
        }
        self.external.remove_code(&active.name, &active.code_path)?;
        Ok(super::FactoryEffects { effects })
    }
}
fn publication_error(error: crate::package::scan::live_install::PublicationFailure) -> String {
    match error {
        crate::package::scan::live_install::PublicationFailure::Persist { error, .. } => {
            format!("{error:?}")
        }
    }
}
fn system_location(path: &str, apexes: &[crate::package::scan::Apex]) -> Result<crate::package::scan::Location, Exception> {
    use crate::package::scan::{Kind, Location, Partition};
    if path.starts_with("/apex/") {
        let apex = apexes.iter().filter(|apex| {
            path.strip_prefix(&apex.mount_path).is_some_and(|rest| rest.starts_with('/'))
        }).max_by_key(|apex| apex.mount_path.len())
            .ok_or_else(|| fail("Factory APEX code has no original active origin"))?;
        if apex.partition == Partition::Data {
            return Err(fail("Factory APEX has no preinstalled partition"));
        }
        let relative = &path[apex.mount_path.len() + 1..];
        let kind = if relative.starts_with("priv-app/") { Kind::PrivApp }
            else if relative.starts_with("app/") { Kind::App }
            else { return Err(fail("Factory APEX code is outside its application directories")); };
        if relative.split('/').any(|component| component.is_empty() || matches!(component, "." | "..")) {
            return Err(fail("Factory APEX code path is not canonical"));
        }
        return Ok(Location { path: path.into(), partition: apex.partition, kind, apex: Some(apex.clone()) });
    }
    let (partition, prefix) = if path.starts_with("/system_ext/") {
        (Partition::SystemExt, "/system_ext/")
    } else if path.starts_with("/product/") {
        (Partition::Product, "/product/")
    } else if path.starts_with("/vendor/") {
        (Partition::Vendor, "/vendor/")
    } else if path.starts_with("/odm/") {
        (Partition::Odm, "/odm/")
    } else if path.starts_with("/oem/") {
        (Partition::Oem, "/oem/")
    } else if path.starts_with("/system/") {
        (Partition::System, "/system/")
    } else {
        return Err(fail("Factory code is outside the pinned system partitions"));
    };
    let relative = &path[prefix.len()..];
    let kind = if relative.starts_with("framework/") {
        Kind::Framework
    } else if relative.starts_with("priv-app/") {
        Kind::PrivApp
    } else if relative.starts_with("overlay/") {
        Kind::Overlay
    } else {
        Kind::App
    };
    Ok(Location {
        path: path.into(),
        partition,
        kind,
        apex: None,
    })
}
fn fail(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

struct PreparedData {
    image: Arc<super::super::environment::Config>,
    created: Vec<(String, i32, i64)>,
    committed: bool,
}
impl PreparedData {
    fn commit(&self) -> Result<(), Exception> {
        for (name, user, inode) in &self.created {
            (self.image.commit_app_data)(name, *user, *inode)?;
        }
        Ok(())
    }
}
impl Drop for PreparedData {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (name, user, inode) in self.created.iter().rev() {
            if let Err(error) = (self.image.rollback_app_data)(name, *user, *inode) {
                eprintln!("Factory app-data rollback for {name}/{user}: {error:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{parse, scan::{Apex, Kind, Partition}};
    #[test]
    fn factory_restore_uses_original_apex_origin_and_parse_policy() {
        let apex = Apex { module_name: Some("vendor.module".into()), mount_path: "/apex/vendor.module".into(), partition: Partition::Product, factory: true, active_changed: false };
        let path = "/apex/vendor.module/priv-app/Factory/base.apk";
        let location = system_location(path, &[apex.clone()]).unwrap();
        assert_eq!(location.partition, Partition::Product);
        assert_eq!(location.kind, Kind::PrivApp);
        assert_eq!(location.apex.as_ref(), Some(&apex));
        assert_eq!(location.parse_flags(), parse::PARSE_IS_SYSTEM_DIR | parse::PARSE_APK_IN_APEX);
        assert!(location.privileged());
        let app = system_location("/apex/vendor.module/app/Factory/base.apk", &[apex.clone()]).unwrap();
        assert_eq!(app.kind, Kind::App); assert!(!app.privileged());
        for path in ["/apex/vendor.module-other/priv-app/F", "/apex/vendor.module/priv-app/../F", "/apex/vendor.module/bin/F", "/apex/unknown/priv-app/F"] {
            assert!(system_location(path, &[apex.clone()]).is_err(), "{path}");
        }
        let mut data = apex; data.partition = Partition::Data;
        assert!(system_location("/apex/vendor.module/app/F", &[data]).is_err());
        let ordinary = system_location("/system_ext/priv-app/F/base.apk", &[]).unwrap();
        assert_eq!(ordinary.partition, Partition::SystemExt); assert_eq!(ordinary.apex, None);
        assert_eq!(ordinary.parse_flags(), parse::PARSE_IS_SYSTEM_DIR);
    }
}
