//! Original permission owner leaves for the native live install candidate.
use super::{codec::SessionParams, environment::RuntimePrepare, pipeline::VerifiedCode};
use crate::package::{
    owner::legacy_permissions::State,
    scan::live_install::CompletedAdmission,
    scan_snapshot::{Store, endpoint::Endpoint},
};
use aim_binder_host::{
    local::{LocalProcess, Strong},
    parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Binder},
};
use aim_service_aidl::dev_aim_server_iinstallerpermissionbridge as api;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Install {
    params: SessionParams,
    user: i32,
    name: String,
}
pub struct Owner {
    bridge: Strong,
    process: Arc<LocalProcess>,
    snapshots: Arc<Store>,
    cross_user_suspensions: bool,
    context: super::permission_capture::ContextSource,
    pending: Mutex<BTreeMap<String, Install>>,
    library_policy: Mutex<Option<LibraryDependencyPolicy>>,
    prepared: Mutex<BTreeMap<String,Arc<Prepared>>>,
}
struct Prepared {
    _candidate: Scope,
    _previous: Scope,
    binder: Binder,
    names: Vec<String>,
    retired: Mutex<bool>,
}
pub type LibraryDependencyPolicy = Arc<
    dyn Fn(
            &str,
            &crate::package::pkg::AndroidPackage,
        ) -> Result<crate::package::libraries::Policy, String>
        + Send
        + Sync,
>;
fn illegal(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
struct Scope(Arc<Endpoint>);
impl Drop for Scope {
    fn drop(&mut self) {
        self.0.revoke_install_scope();
    }
}
/// A live scan has replaced staging paths, assigned appIds and resolved current
/// ABI/seInfo/library metadata. Rebuild loaded runtime from those native owners;
/// preserve factory/unloaded runtime only when its retained identity matches.
pub(in crate::package) fn complete_candidate_runtime(
    owner: &mut crate::package::scan::SigningScan,
    base: &crate::package::scan_snapshot::Snapshot,
    usage: &crate::package::owner::usage::Usage,
) -> Result<(), String> {
    let mut retained = BTreeMap::new();
    for (settings, loaded, factory) in [
        (&owner.settings.packages, owner.loaded_packages(), false),
        (
            &owner.settings.disabled_system_packages,
            owner.disabled_loaded_packages(),
            true,
        ),
    ] {
        for setting in settings {
            if !factory && loaded.contains_key(&setting.name) {
                continue;
            }
            let has_code = loaded.contains_key(&setting.name);
            let mut source = None;
            for old_factory in if factory {
                vec![true, false]
            } else {
                vec![false]
            } {
                let old_settings = if old_factory {
                    &base.owner().settings.disabled_system_packages
                } else {
                    &base.owner().settings.packages
                };
                let old_loaded = if old_factory {
                    base.owner().disabled_loaded_packages()
                } else {
                    base.owner().loaded_packages()
                };
                if old_settings.iter().any(|old| {
                    old.name == setting.name
                        && old.app_id == setting.app_id
                        && old.code_path == setting.code_path
                        && old.version_code == setting.version_code
                        && old.shared_user == setting.shared_user
                        && old_loaded.contains_key(&old.name) == has_code
                }) {
                    source = base.replica_runtime(&setting.name, old_factory)?.cloned();
                    break;
                }
            }
            let state = source.ok_or_else(|| {
                format!(
                    "retained install runtime identity unavailable: {} factory={factory}",
                    setting.name
                )
            })?;
            retained.insert(
                (setting.name.clone(), factory),
                crate::package::scan::OriginalRuntime {
                    name: setting.name.clone(),
                    app_id: setting.app_id,
                    path: setting.code_path.clone(),
                    version: setting.version_code,
                    has_code,
                    state,
                    transient: setting.transient.clone(),
                },
            );
        }
    }
    owner.complete_runtime_at_boot(usage, retained)
}
impl Owner {
    pub fn new(
        bridge: Strong,
        process: Arc<LocalProcess>,
        snapshots: Arc<Store>,
        cross_user_suspensions: bool,
        context: super::permission_capture::ContextSource,
    ) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            process,
            snapshots,
            cross_user_suspensions,
            context,
            pending: Mutex::new(BTreeMap::new()),
            library_policy: Mutex::new(None),
            prepared: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn configure_library_policy(
        &self,
        source: LibraryDependencyPolicy,
    ) -> Result<(), Exception> {
        let mut policy = self.library_policy.lock().unwrap();
        if policy.is_some() {
            return Err(illegal(
                "installer library dependency policy already configured",
            ));
        }
        *policy = Some(source);
        Ok(())
    }
    /// Config.metadata records the verified session against its exclusive code
    /// reservation; no name-only lookup can substitute another session's policy.
    pub fn remember(&self, code: &VerifiedCode, destination: &str) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(destination) {
            return Err("permission reservation already recorded".into());
        }
        pending.insert(
            destination.into(),
            Install {
                params: code.record.params.clone(),
                user: code.record.user as i32,
                name: code.package.package_name.clone(),
            },
        );
        Ok(())
    }
    /// The reservation owner calls this when an admission fails before runtime
    /// preparation consumes its verified policy.
    pub fn forget(&self, destination: &str) {
        self.pending.lock().unwrap().remove(destination);
    }
    pub fn metadata(
        self: &Arc<Self>,
        metadata: super::environment::Metadata,
    ) -> super::environment::Metadata {
        let owner = self.clone();
        Arc::new(move |code, destination, base| {
            let value = metadata(code, destination, base)?;
            owner.remember(code, destination)?;
            Ok(value)
        })
    }
    pub fn release(self: &Arc<Self>) -> super::environment::ReservationRelease {
        let owner = self.clone();
        Arc::new(move |destination,committed| owner.finish(destination,committed))
    }
    fn finish(&self,destination:&str,committed:bool)->Result<(),Exception>{
        self.forget(destination);
        let prepared=self.prepared.lock().unwrap().get(destination).cloned();
        let Some(prepared)=prepared else{return Ok(());};
        let mut retired=prepared.retired.lock().unwrap();
        if *retired{return Ok(());}
        if !committed{
            let current=self.snapshots.capture();let context=(self.context)(&current).map_err(illegal)?;
            let scope=Scope(super::permission_capture::endpoint(current,context,&self.process).map_err(illegal)?);
            let binder=self.process.add_service(scope.0.clone());let mut request=Parcel::new();
            api::Rollback{candidate:Some(prepared.binder),current:Some(binder),package_names:Some(prepared.names.iter().cloned().map(Some).collect()),cross_user_suspensions:self.cross_user_suspensions}.write(&mut request);
            let reply=self.bridge.transact(api::ROLLBACK,&request,false).map_err(|error|illegal(format!("original permission rollback transport: {error}")))?;
            let mut reader=reply.reader();api::read_rollback_reply(&mut reader).map_err(|error|illegal(format!("original permission rollback reply: {error}")))??;
            if reader.remaining()!=0{return Err(illegal("original permission rollback trailing data"));}
        }
        *retired=true;
        self.prepared.lock().unwrap().retain(|_,value|!Arc::ptr_eq(value,&prepared));
        Ok(())
    }
    pub fn attach(
        self: &Arc<Self>,
        mut services: super::environment_image::Services,
    ) -> super::environment_image::Services {
        services.metadata = self.metadata(services.metadata);
        services.permissions = self.runtime_prepare();
        services.release_permissions = self.release();
        services
    }
    pub fn runtime_prepare(self: &Arc<Self>) -> RuntimePrepare {
        let owner = self.clone();
        Arc::new(move |admission, base| owner.prepare(admission, base))
    }
    fn prepare(&self, admission: &mut CompletedAdmission, base: &Arc<crate::package::scan_snapshot::Snapshot>) -> Result<(), Exception> {
        let source =
            self.library_policy.lock().unwrap().clone().ok_or_else(|| {
                illegal("installer original library dependency policy unavailable")
            })?;
        let policies = admission
            .owner
            .loaded_packages()
            .iter()
            .map(|(name, code)| {
                source(name, &code.package)
                    .map(|policy| (name.clone(), policy))
                    .map_err(|error| {
                        illegal(format!("installer library policy for {name}: {error}"))
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        admission
            .owner
            .complete_library_dependencies(&|name, _| {
                let policy = policies.get(name).ok_or(
                    crate::package::libraries::ResolveError::Incomplete(
                        "installer original library policy",
                    ),
                )?;
                Ok(crate::package::libraries::Policy {
                    enforce_native_dependencies: policy.enforce_native_dependencies,
                    sdk_library_independence: policy.sdk_library_independence,
                })
            })
            .map_err(|error| {
                illegal(format!(
                    "permission candidate library completion: {error:?}"
                ))
            })?;
        for metadata in &mut admission.completed {
            metadata.candidate.users = admission
                .owner
                .scanned_user_states(&metadata.candidate.record.settings.name)
                .cloned()
                .ok_or_else(|| illegal("completed install library user scope unavailable"))?;
        }
        let mut records = Vec::new();
        {
            let mut pending = self.pending.lock().unwrap();
            for metadata in &admission.completed {
                let setting = &metadata.candidate.record.settings;
                let install = pending
                    .remove(&setting.code_path)
                    .ok_or_else(|| illegal("permission install reservation unavailable"))?;
                if install.name != metadata.candidate.record.parsed.package_name {
                    return Err(illegal("permission install code identity differs"));
                }
                records.push((setting.name.clone(), install));
            }
        }
        let mut installation = Parcel::new();
        installation.write_i32(records.len() as i32);
        let mut expected = BTreeSet::new();
        for (name, install) in &records {
            let setting = admission
                .owner
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| illegal("permission candidate setting unavailable"))?;
            let old = base
                .owner()
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name);
            let previous = old
                .filter(|p| p.shared_user && p.app_id != setting.app_id)
                .map_or(-1, |p| p.app_id);
            expected.insert(setting.app_id);
            if previous >= 0
                && admission
                    .owner
                    .settings
                    .packages
                    .iter()
                    .any(|p| p.app_id == previous)
            {
                expected.insert(previous);
            }
            installation.write_string16(Some(name));
            installation.write_i32(install.params.install_flags);
            installation.write_i32(previous);
            installation.write_i32(if install.params.install_flags & 0x40 != 0 {
                -1
            } else {
                install.user
            });
            let mut names = Vec::new();
            let mut states = Vec::new();
            for (name, state) in &install.params.permission_states {
                names.push(
                    name.clone()
                        .ok_or_else(|| illegal("null install permission name"))?,
                );
                states.push(state.ok_or_else(|| illegal("null install permission state"))?);
            }
            installation.write_i32(names.len() as i32);
            for name in names {
                installation.write_string16(Some(&name));
            }
            aim_service_aidl::write_int_array(&mut installation, Some(&states));
            let allowlist = install.params.whitelisted_restricted_permissions.as_ref();
            if allowlist.is_some_and(|list| list.iter().any(Option::is_none)) {
                return Err(illegal("null restricted install permission"));
            }
            installation.write_i32(allowlist.map_or(-1, |list| list.len() as i32));
            if let Some(list) = allowlist {
                for name in list {
                    installation.write_string16(name.as_deref());
                }
            }
            installation.write_i32(install.params.auto_revoke_permissions_mode);
        }
        let usage = base.usage().for_install(
            admission
                .owner
                .settings
                .packages
                .iter()
                .map(|p| p.name.as_str()),
        );
        complete_candidate_runtime(&mut admission.owner, &base, &usage)
            .map_err(|error| illegal(format!("permission candidate runtime: {error}")))?;
        let version = base
            .version()
            .checked_add(1)
            .ok_or_else(|| illegal("permission candidate version exhausted"))?;
        let candidate = Store::new_replica_at_version(admission.owner.clone(), usage, version)
            .map_err(|error| illegal(format!("permission candidate scan: {error:?}")))?
            .capture();
        let candidate_context = (self.context)(&candidate).map_err(illegal)?;
        let previous_context = (self.context)(&base).map_err(illegal)?;
        let candidate_scope = Scope(
            super::permission_capture::endpoint(candidate, candidate_context, &self.process)
                .map_err(illegal)?,
        );
        let previous_scope = Scope(
            super::permission_capture::endpoint(base.clone(), previous_context, &self.process)
                .map_err(illegal)?,
        );
        let candidate_endpoint = self.process.add_service(candidate_scope.0.clone());
        let previous_endpoint = self.process.add_service(previous_scope.0.clone());
        let prepared=Arc::new(Prepared{_candidate:candidate_scope,_previous:previous_scope,binder:candidate_endpoint,
            names:records.iter().map(|(name,_)|name.clone()).collect(),retired:Mutex::new(false)});
        {
            let mut active=self.prepared.lock().unwrap();
            for metadata in &admission.completed{
                let destination=&metadata.candidate.record.settings.code_path;
                if active.contains_key(destination){return Err(illegal("prepared permission reservation already exists"));}
            }
            for metadata in &admission.completed{active.insert(metadata.candidate.record.settings.code_path.clone(),prepared.clone());}
        }
        let mut request = Parcel::new();
        request.write_interface_token(api::DESCRIPTOR);
        request.write_binder(Some(candidate_endpoint));
        request.write_binder(Some(previous_endpoint));
        aim_service_aidl::write_byte_array(&mut request, Some(installation.data()));
        request.write_bool(self.cross_user_suspensions);
        let reply = self
            .bridge
            .transact(api::PREPARE, &request, false)
            .map_err(|error| illegal(format!("permission prepare transport: {error}")))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|error| illegal(format!("permission prepare reply: {error}")))?
            .map_err(|mut error| {
                error.message = format!(
                    "original permission PREPARE (exception {}): {}",
                    error.code, error.message
                );
                error
            })?;
        let bytes = aim_service_aidl::read_byte_array(&mut reader)
            .map_err(|error| illegal(format!("permission prepare payload: {error}")))?
            .ok_or_else(|| illegal("permission prepare missing payload"))?;
        if reader.remaining()!=0{return Err(illegal(format!("permission PREPARE reply has {} trailing bytes",reader.remaining())));}
        let current=self.snapshots.capture();
        let targets=records.iter().map(|(name,_)|name.clone()).collect();
        // Check the retained admission without overwriting it with an
        // intermediate publication. The atomic commit performs the single
        // three-way merge against its latest canonical owner.
        let mut retained=admission.owner.clone();
        if let Some(reason)=crate::package::scan_snapshot::install_context::install_user_rebase_conflict(base,&current,&mut retained,&targets).map_err(illegal)?{
            return Err(illegal(format!("permission install base changed: {reason}; {}",crate::package::scan_snapshot::install_context::install_identity_delta(base,&current))));
        }
        let mut input = Reader::new(&bytes, &[]);
        let decode = |input: &mut Reader<'_>| -> Result<_, i32> {
            let users = aim_service_aidl::read_int_array(input)?.ok_or(BAD_VALUE)?;
            let count = input.read_i32()?;
            if count < 0 || count as usize != expected.len() {
                return Err(BAD_VALUE);
            }
            let mut states = BTreeMap::new();
            for _ in 0..count {
                let app_id = input.read_i32()?;
                let bytes = aim_service_aidl::read_byte_array(input)?.ok_or(BAD_VALUE)?;
                let state = State::read(&bytes, app_id, &users).map_err(|_| BAD_VALUE)?;
                if !expected.contains(&app_id) || states.insert(app_id, state).is_some() {
                    return Err(BAD_VALUE);
                }
            }
            if input.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            Ok((users, states))
        };
        let (users, states) = decode(&mut input)
            .map_err(|error| illegal(format!("permission install projection: {error}")))?;
        admission
            .owner
            .apply_installed_permission_states(&users, states)
            .map_err(illegal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        owner::usage::Usage,
        scan::{ReplicaRuntime, SigningScan},
        settings::{Package, Settings},
        system_config::SystemConfig,
    };
    fn base() -> Arc<crate::package::scan_snapshot::Snapshot> {
        let active = Package {
            name: "fixture".into(),
            app_id: 10100,
            code_path: "/data/app/fixture-old".into(),
            version_code: 4,
            ..Default::default()
        };
        let factory = Package {
            code_path: "/system/app/fixture".into(),
            version_code: 2,
            ..active.clone()
        };
        let mut settings = Settings::default();
        settings.packages.push(active);
        settings.disabled_system_packages.push(factory);
        let mut owner = SigningScan::new(&SystemConfig::default(), &settings, 29).unwrap();
        owner
            .capture_replica_runtime(
                [false, true]
                    .into_iter()
                    .map(|factory| {
                        (
                            ("fixture".into(), factory),
                            ReplicaRuntime {
                                usage: if factory { [17; 8] } else { [0; 8] },
                                seinfo: None,
                                override_seinfo: None,
                                library_files: if factory {
                                    vec![Some("/system/library.jar".into())]
                                } else {
                                    vec![]
                                },
                                libraries: vec![],
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap();
        crate::package::scan_snapshot::Store::new(owner, Usage::new(["fixture"]))
            .unwrap()
            .capture()
    }
    #[test]
    fn retained_permission_handoff_allows_real_usage_publication_but_rejects_code_changes() {
        use crate::package::scan_snapshot::install_context::same_install_identity;
        let original = base();
        let store = Store::new(original.owner().clone(), original.usage().clone()).unwrap();
        let before = store.capture();
        let mut usage = before.usage().clone();
        usage.notify("fixture", 0, 991);
        let mut owner = before.owner().clone();
        owner.update_replica_usage(before.usage(), &usage).unwrap();
        let latest = store.publish(&before, owner, usage).unwrap();
        assert!(same_install_identity(&before, &latest).unwrap());
        assert_eq!(
            latest
                .replica_runtime("fixture", false)
                .unwrap()
                .unwrap()
                .usage[0],
            991
        );
        assert_eq!(
            latest
                .replica_runtime("fixture", true)
                .unwrap()
                .unwrap()
                .usage,
            [17; 8]
        );
        assert_eq!(
            store
                .publish(&before, latest.owner().clone(), latest.usage().clone())
                .unwrap_err(),
            crate::package::scan_snapshot::Error::Stale
        );
        let mut changed = latest.owner().clone();
        changed.settings.packages[0].code_path = "/data/app/replaced-code".into();
        let values = [false, true]
            .into_iter()
            .map(|factory| {
                (
                    ("fixture".into(), factory),
                    latest
                        .replica_runtime("fixture", factory)
                        .unwrap()
                        .unwrap()
                        .clone(),
                )
            })
            .collect();
        changed.capture_replica_runtime(values).unwrap();
        let replaced = store
            .publish(&latest, changed, latest.usage().clone())
            .unwrap();
        assert!(!same_install_identity(&before, &replaced).unwrap());
    }
    #[test]
    fn candidate_runtime_preserves_exact_factory_and_unloaded_scopes() {
        let base = base();
        let mut owner = base.owner().clone();
        let usage = base.usage().for_install(["fixture"]);
        complete_candidate_runtime(&mut owner, &base, &usage).unwrap();
        assert_eq!(
            owner.replica_runtime("fixture", false).unwrap(),
            base.replica_runtime("fixture", false).unwrap()
        );
        assert_eq!(
            owner.replica_runtime("fixture", true).unwrap(),
            base.replica_runtime("fixture", true).unwrap()
        );
        let snapshot = Store::new(owner, usage).unwrap().capture();
        assert_eq!(
            snapshot.owner().settings.packages[0].code_path,
            "/data/app/fixture-old"
        );
    }
    #[test]
    fn candidate_runtime_rejects_rebinding_old_values_to_new_install_identity_without_native_code()
    {
        let base = base();
        let mut owner = base.owner().clone();
        let setting = &mut owner.settings.packages[0];
        setting.code_path = "/data/app/fixture-new".into();
        setting.app_id = 10101;
        setting.version_code = 5;
        let before = owner.clone();
        let usage = base.usage().for_install(["fixture"]);
        let error = complete_candidate_runtime(&mut owner, &base, &usage).unwrap_err();
        assert!(error.contains("retained install runtime identity unavailable"));
        assert_eq!(owner, before);
        assert!(Store::new(owner, usage).is_err());
    }
}
