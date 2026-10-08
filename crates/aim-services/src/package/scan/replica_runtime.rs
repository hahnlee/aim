//! Explicit scoped PackageStateUnserialized inputs for replica assembly (#873).
//! AOSP android-16.0.0_r1; Copyright AOSP, Apache License 2.0.
use super::SigningScan;
use crate::package::{model::SharedLibrary, owner::usage::Usage};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ReplicaRuntime {
    pub usage: [i64; crate::package::owner::usage::REASONS],
    pub seinfo: Option<String>,
    pub override_seinfo: Option<String>,
    pub library_files: Vec<Option<String>>,
    pub libraries: Vec<SharedLibrary>,
}

/// Identity and runtime values exported together from one original snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalRuntime {
    pub name: String,
    pub app_id: i32,
    pub path: String,
    pub version: i64,
    pub has_code: bool,
    pub state: ReplicaRuntime,
    pub transient: crate::package::owner::transient::State,
}

impl OriginalRuntime {
    pub fn read_original_record(bytes: &[u8]) -> aim_binder_host::parcel::Result<Self> {
        use aim_binder_host::parcel::{BAD_VALUE, Reader};
        let mut r = Reader::new(bytes, &[]);
        let name = r.read_string16()?.ok_or(BAD_VALUE)?;
        let app_id = r.read_i32()?;
        let path = r.read_string16()?.ok_or(BAD_VALUE)?;
        let version = r.read_i64()?;
        let boolean = |r: &mut Reader<'_>| match r.read_i32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(BAD_VALUE),
        };
        let has_code = boolean(&mut r)?;
        let seinfo = r.read_string16()?;
        let override_seinfo = r.read_string16()?;
        let usage: [i64; 8] = aim_service_aidl::read_long_array(&mut r)?
            .ok_or(BAD_VALUE)?
            .try_into()
            .map_err(|_| BAD_VALUE)?;
        let count = |r: &mut Reader<'_>| {
            let n = r.read_i32()?;
            if n < 0 || n as usize > r.remaining() / 4 {
                Err(BAD_VALUE)
            } else {
                Ok(n as usize)
            }
        };
        let mut library_files = Vec::new();
        for _ in 0..count(&mut r)? {
            library_files.push(r.read_string16()?);
        }
        let mut libraries = Vec::new();
        for _ in 0..count(&mut r)? {
            libraries.push(super::super::library_parcel::read_feed(&mut r)?);
        }
        let transient = crate::package::owner::transient::State {
            hidden_until_installed: boolean(&mut r)?,
            updated_system_app: boolean(&mut r)?,
            apk_in_updated_apex: boolean(&mut r)?,
            apex_module_name: r.read_string16()?,
        };
        if r.remaining() != 0 || bytes.len() % 4 != 0 {
            return Err(BAD_VALUE);
        }
        Ok(crate::package::scan::OriginalRuntime {
            name,
            app_id,
            path,
            version,
            has_code,
            state: crate::package::scan::ReplicaRuntime {
                usage,
                seinfo,
                override_seinfo,
                library_files,
                libraries,
            },
            transient,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Identity {
    app_id: i32,
    path: String,
    version: i64,
    shared: bool,
    code: Option<std::sync::Arc<super::LoadedPackage>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    identities: BTreeMap<(String, bool), Identity>,
    values: BTreeMap<(String, bool), ReplicaRuntime>,
}

fn identities(owner: &SigningScan) -> BTreeMap<(String, bool), Identity> {
    let mut result = BTreeMap::new();
    for (packages, code, factory) in [
        (&owner.settings.packages, &owner.loaded, false),
        (
            &owner.settings.disabled_system_packages,
            &owner.disabled_loaded,
            true,
        ),
    ] {
        for setting in packages {
            result.insert(
                (setting.name.clone(), factory),
                Identity {
                    app_id: setting.app_id,
                    path: setting.code_path.clone(),
                    version: setting.version_code,
                    shared: setting.shared_user,
                    code: code.get(&setting.name).cloned(),
                },
            );
        }
    }
    result
}

impl SigningScan {
    pub(super) fn rebind_retired_code_runtime(&mut self, name: &str) -> Result<(), String> {
        let current = identities(self);
        if let Some(assigned) = &mut self.replica_runtime {
            let key = (name.to_owned(), false);
            let before = assigned.identities.get(&key).ok_or("retired code runtime identity unavailable")?;
            let after = current.get(&key).ok_or("retired setting runtime identity unavailable")?;
            if before.app_id != after.app_id || before.path != after.path || before.version != after.version
                || before.shared != after.shared || after.code.is_some() {
                return Err("retired setting runtime identity differs".into());
            }
            assigned.identities.insert(key, after.clone());
        }
        Ok(())
    }

    pub(super) fn remove_setting_runtime(&mut self, name: &str) {
        if let Some(assigned) = &mut self.replica_runtime {
            let key = (name.into(), false);
            assigned.identities.remove(&key);
            assigned.values.remove(&key);
        }
    }

    pub(super) fn original_setting_runtime(
        &self,
        setting: &crate::package::settings::Package,
    ) -> Result<Option<ReplicaRuntime>, String> {
        let Some(assigned) = &self.replica_runtime else {
            return Ok(None);
        };
        let key = (setting.name.clone(), false);
        let current = identities(self);
        let identity = current
            .get(&key)
            .ok_or("original runtime setting is missing")?;
        if self
            .settings
            .packages
            .iter()
            .find(|p| p.name == setting.name)
            != Some(setting)
            || identity.code.is_some()
            || assigned.identities.get(&key) != Some(identity)
        {
            return Err("original runtime setting identity differs".into());
        }
        assigned
            .values
            .get(&key)
            .cloned()
            .map(Some)
            .ok_or_else(|| "original runtime input is missing".into())
    }

    /// Import an entire original snapshot's runtime inventory after matching
    /// the native scan's settings and code scope. A rejected source changes nothing.
    pub fn capture_original_runtime(
        &mut self,
        source: &crate::package::model::State,
    ) -> Result<(), String> {
        self.capture_runtime_records(&source.runtime_inputs)
    }

    /// Finish boot runtime after seInfo and dependency resolution. Retained
    /// factory/unloaded settings require their own identity-bearing inputs.
    pub fn complete_runtime_at_boot(
        &mut self,
        usage: &Usage,
        mut retained: BTreeMap<(String, bool), OriginalRuntime>,
    ) -> Result<(), String> {
        if usage.names().ne(self
            .settings
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter())
        {
            return Err("boot runtime usage inventory differs".into());
        }
        let expected = identities(self);
        let mut inputs = BTreeMap::new();
        for (key, identity) in &expected {
            let input = if !key.1 && identity.code.is_some() {
                let labels = self
                    .seinfo_state(&key.0)?
                    .ok_or("missing boot runtime seInfo owner")?;
                let (files, libraries) = self
                    .library_dependencies(&key.0)?
                    .ok_or("missing boot runtime library owner")?;
                let setting = self
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == key.0)
                    .ok_or("missing boot runtime setting")?;
                OriginalRuntime {
                    name: key.0.clone(),
                    app_id: identity.app_id,
                    path: identity.path.clone(),
                    version: identity.version,
                    has_code: true,
                    state: ReplicaRuntime {
                        usage: *usage
                            .times(&key.0)
                            .ok_or("missing boot runtime usage owner")?,
                        seinfo: labels.base.clone(),
                        override_seinfo: labels.override_label.clone(),
                        library_files: files.to_vec(),
                        libraries: libraries.to_vec(),
                    },
                    transient: setting.transient.clone(),
                }
            } else {
                retained
                    .remove(key)
                    .ok_or_else(|| format!("missing retained boot runtime: {key:?}"))?
            };
            if !key.1 && usage.times(&key.0) != Some(&input.state.usage) {
                return Err(format!("retained boot runtime usage differs: {}", key.0));
            }
            inputs.insert(key.clone(), input);
        }
        if !retained.is_empty() {
            return Err("unexpected retained boot runtime inventory".into());
        }
        self.capture_runtime_records(&inputs)
    }

    fn capture_runtime_records(
        &mut self,
        inputs: &BTreeMap<(String, bool), OriginalRuntime>,
    ) -> Result<(), String> {
        let expected = identities(self);
        if expected.keys().ne(inputs.keys()) {
            return Err("original runtime inventory differs".into());
        }
        let mut values = BTreeMap::new();
        let mut transient = BTreeMap::new();
        for (key, input) in inputs {
            let identity = &expected[key];
            if input.name != key.0
                || input.app_id != identity.app_id
                || input.path != identity.path
                || input.version != identity.version
                || input.has_code != identity.code.is_some()
            {
                return Err(format!(
                    "original runtime setting/code scope differs: {}",
                    key.0
                ));
            }
            values.insert(key.clone(), input.state.clone());
            transient.insert(key.clone(), input.transient.clone());
        }
        let mut candidate = self.clone();
        candidate.capture_replica_runtime(values)?;
        candidate.capture_transient_states(transient)?;
        self.replica_runtime = candidate.replica_runtime;
        self.settings = candidate.settings;
        Ok(())
    }

    /// Capture the owning operation's complete current runtime inventory. No
    /// active fields are substituted for a factory setting with the same name.
    /// notifyPackageUse updates active PackageStateUnserialized usage and its
    /// package usage owner together. Disabled factory records retain their own
    /// historical state and must not inherit an active package's new timestamp.
    /// Only the immutable, fully validated Snapshot owner may use this path.
    /// General mutable scan updates continue to validate their entire graph.
    pub(in crate::package) fn update_validated_replica_usage(&mut self, before: &Usage, after: &Usage) -> Result<(), String> {
        if before.names().ne(after.names()) || before.historical_available() != after.historical_available() {
            return Err("validated usage immutable inventory differs".into());
        }
        let assigned = self.replica_runtime.as_mut().ok_or("validated runtime owner unavailable")?;
        for name in before.names() {
            let previous = before.times(name).unwrap();
            let times = after.times(name).unwrap();
            if previous == times { continue; }
            let runtime = assigned.values.get_mut(&(name.to_owned(), false)).ok_or("validated usage member unavailable")?;
            if runtime.usage != *previous { return Err("validated usage runtime base differs".into()); }
            runtime.usage = *times;
        }
        Ok(())
    }

    pub(in crate::package) fn update_replica_usage(&mut self, before: &Usage, after: &Usage) -> Result<(), String> {
        self.validate_replica_runtime(Some(before))?;
        let assigned = self.replica_runtime.as_ref().ok_or("replica runtime is not captured")?;
        let active = self.settings.packages.iter().map(|setting|setting.name.as_str()).collect::<std::collections::BTreeSet<_>>();
        if active != after.names().collect() || active != before.names().collect() {
            return Err("runtime usage immutable package inventory differs".into());
        }
        let mut updates = Vec::with_capacity(active.len());
        for name in active {
            let key = (name.to_owned(), false);
            if !assigned.values.contains_key(&key) { return Err(format!("active runtime usage owner absent: {name}")); }
            let times = *after.times(name).ok_or_else(||format!("active usage owner absent: {name}"))?;
            updates.push((key, times));
        }
        let assigned = self.replica_runtime.as_mut().unwrap();
        for (key, times) in updates { assigned.values.get_mut(&key).unwrap().usage = times; }
        Ok(())
    }

    pub(in crate::package) fn has_replica_runtime(&self) -> bool {
        self.replica_runtime.is_some()
    }

    pub(in crate::package) fn rebase_removal_usage(&mut self, usage: &Usage) -> Result<(), String> {
        self.validate_replica_runtime(None)?;
        let Some(assigned) = &self.replica_runtime else { return Ok(()); };
        let mut values = assigned.values.clone();
        for setting in &self.settings.packages {
            let state = values.get_mut(&(setting.name.clone(), false))
                .ok_or_else(|| format!("removal active runtime unavailable: {}",setting.name))?;
            state.usage = *usage.times(&setting.name)
                .ok_or_else(|| format!("removal active usage unavailable: {}",setting.name))?;
        }
        // Factory values retain their original historical usage. Only active
        // code/settings survivors receive actual latest canonical times.
        self.capture_replica_runtime(values)
    }

    pub fn capture_replica_runtime(
        &mut self,
        values: BTreeMap<(String, bool), ReplicaRuntime>,
    ) -> Result<(), String> {
        if !self.capture_ready() {
            return Err("scan metadata is not finalized".into());
        }
        let inputs = identities(self);
        if inputs.keys().ne(values.keys()) {
            return Err("replica runtime inventory differs".into());
        }
        let mut candidate = self.clone();
        candidate.replica_runtime = Some(Assignments {
            identities: inputs,
            values,
        });
        candidate.validate_replica_runtime(None)?;
        self.replica_runtime = candidate.replica_runtime;
        Ok(())
    }

    pub(in crate::package) fn validate_replica_runtime(
        &self,
        usage: Option<&Usage>,
    ) -> Result<(), String> {
        let Some(assigned) = &self.replica_runtime else {
            return Ok(());
        };
        if !self.capture_ready() || identities(self) != assigned.identities {
            return Err("replica runtime setting/code identity differs".into());
        }
        // Validate each complete dependency graph once. Scoped lookups below
        // remain under this immutable borrow and cannot invalidate those checks.
        if self.seinfo.is_some() { self.validate_seinfo()?; }
        if self.library_dependencies.is_some() { self.validate_library_dependencies()?; }
        for ((name, factory), value) in &assigned.values {
            if *factory {
                continue;
            }
            if let Some(usage) = usage {
                if usage.times(name).map(|v| v.as_slice()) != Some(value.usage.as_slice()) {
                    return Err(format!("replica runtime usage differs: {name}"));
                }
            }
            if self.loaded.contains_key(name) {
                if self.seinfo.is_some() {
                    let labels = self
                        .validated_seinfo_state(name)?
                        .ok_or("missing replica seInfo owner")?;
                    if labels.base != value.seinfo || labels.override_label != value.override_seinfo
                    {
                        return Err(format!("replica runtime seInfo differs: {name}"));
                    }
                }
                if self.library_dependencies.is_some() {
                    let (files, infos) = self
                        .validated_library_dependencies(name)?
                        .ok_or("missing replica library owner")?;
                    if files != value.library_files || infos != value.libraries {
                        return Err(format!("replica runtime libraries differ: {name}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Only an immutable scan Snapshot, validated at creation/publication,
    /// may call this accessor. Mutable SigningScan callers use replica_runtime.
    pub(in crate::package) fn snapshot_replica_runtime(&self,name:&str,factory:bool)->Result<Option<&ReplicaRuntime>,String>{
        let assigned=self.replica_runtime.as_ref().ok_or("replica runtime is not captured")?;
        Ok(assigned.values.get(&(name.into(),factory)))
    }
    pub fn replica_runtime(
        &self,
        name: &str,
        factory: bool,
    ) -> Result<Option<&ReplicaRuntime>, String> {
        self.validate_replica_runtime(None)?;
        let assigned = self
            .replica_runtime
            .as_ref()
            .ok_or("replica runtime is not captured")?;
        Ok(assigned.values.get(&(name.into(), factory)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        restrictions::UserState,
        scan::CapturedUsers,
        scan_snapshot::{Error, Store, runtime_record},
        settings::{Package, Settings},
        system_config::SystemConfig,
    };

    fn owner() -> SigningScan {
        let active = Package {
            name: "p".into(),
            app_id: 10123,
            code_path: "/data/p".into(),
            domain_set_id: Some("00000000-0000-0000-0000-000000000001".into()),
            ..Default::default()
        };
        let factory = Package {
            code_path: "/system/p".into(),
            ..active.clone()
        };
        let mut settings = Settings::default();
        settings.packages.push(active);
        settings.disabled_system_packages.push(factory);
        SigningScan::new(&SystemConfig::default(), &settings, 29).unwrap()
    }
    fn values() -> BTreeMap<(String, bool), ReplicaRuntime> {
        [false, true]
            .into_iter()
            .map(|factory| {
                (
                    ("p".into(), factory),
                    ReplicaRuntime {
                        usage: if factory { [17; 8] } else { [0; 8] },
                        seinfo: if factory { None } else { Some("active".into()) },
                        override_seinfo: if factory { Some("".into()) } else { None },
                        library_files: if factory {
                            vec![None, Some("/system/library".into())]
                        } else {
                            vec![]
                        },
                        libraries: vec![],
                    },
                )
            })
            .collect()
    }
    #[test]
    fn usage_publication_updates_active_runtime_and_retains_factory_and_old_capture() {
        let mut scan = owner(); scan.capture_replica_runtime(values()).unwrap();
        let store = Store::new(scan, Usage::new(["p"])).unwrap();
        let base = store.capture();
        let factory = runtime_record::captured(&base, "p", true).unwrap().unwrap();
        let factory_state = base.replica_runtime("p", true).unwrap().unwrap().clone();
        let active = runtime_record::captured(&base, "p", false).unwrap().unwrap();
        let mut usage = base.usage().clone(); usage.notify("p", 2, 1234);
        let mut next = base.owner().clone(); next.update_replica_usage(base.usage(), &usage).unwrap();
        let published = store.publish(&base, next, usage).unwrap();
        assert_eq!(published.replica_runtime("p", false).unwrap().unwrap().usage[2], 1234);
        assert_eq!(published.usage().times("p").unwrap()[2], 1234);
        assert_eq!(runtime_record::captured(&base, "p", false).unwrap().unwrap(), active);
        assert_eq!(published.replica_runtime("p", true).unwrap().unwrap(), &factory_state);
        assert_eq!(runtime_record::captured(&base, "p", true).unwrap().unwrap(), factory);
        let mut candidate = published.owner().clone(); let original = candidate.clone();
        assert!(candidate.update_replica_usage(published.usage(), &Usage::new(["foreign"])).is_err());
        assert_eq!(candidate, original);
    }
    #[test]
    fn immutable_runtime_accessor_keeps_validation_at_publication_and_retains_old_records(){
        let mut source=owner();source.capture_replica_runtime(values()).unwrap();
        let store=Store::new(source,Usage::new(["p"])).unwrap();let base=store.capture();
        let expected=runtime_record::captured(&base,"p",true).unwrap().unwrap();
        let mut invalid=base.owner().clone();invalid.settings.packages[0].version_code+=1;
        assert!(invalid.replica_runtime("p",false).is_err());
        assert!(matches!(store.publish(&base,invalid,base.usage().clone()),Err(Error::Invalid(_))));
        assert!(std::sync::Arc::ptr_eq(&base,&store.capture()));
        let mut next=base.owner().clone();let mut runtime=values();runtime.get_mut(&("p".into(),true)).unwrap().usage[0]=71;
        next.capture_replica_runtime(runtime).unwrap();let published=store.publish(&base,next,base.usage().clone()).unwrap();
        for _ in 0..8{assert_eq!(runtime_record::captured(&base,"p",true).unwrap().unwrap(),expected);}
        assert_ne!(runtime_record::captured(&published,"p",true).unwrap().unwrap(),expected);
        assert!(runtime_record::captured(&base,"foreign",true).unwrap().is_none());
    }
    #[test]
    fn original_runtime_retention_checks_its_source_before_full_recompletion() {
        let mut owner = owner();
        let original = owner.settings.packages[0].clone();
        let mut other = original.clone();
        other.name = "other".into();
        other.app_id += 1;
        owner.settings.packages.push(other);
        let mut runtime = values();
        runtime.insert(
            ("other".into(), false),
            runtime[&("p".into(), false)].clone(),
        );
        owner.capture_replica_runtime(runtime.clone()).unwrap();
        owner.settings.packages[1].version_code += 1;
        assert!(owner.validate_replica_runtime(None).is_err());
        assert_eq!(
            owner.original_setting_runtime(&original).unwrap(),
            Some(runtime[&("p".into(), false)].clone())
        );
        owner.settings.packages[0].version_code += 1;
        assert!(owner.original_setting_runtime(&original).is_err());
        let changed = owner.settings.packages[0].clone();
        assert!(owner.original_setting_runtime(&changed).is_err());
    }

    #[test]
    fn scoped_inventory_rejects_partial_foreign_and_stale_owners_atomically() {
        let mut owner = owner();
        assert!(owner.replica_runtime("p", false).is_err());
        owner.capture_replica_runtime(values()).unwrap();
        let prior = owner.clone();
        let mut partial = values();
        partial.remove(&("p".into(), true));
        assert!(owner.capture_replica_runtime(partial).is_err());
        assert!(owner == prior);
        let mut foreign = values();
        foreign.insert(("other".into(), true), foreign[&("p".into(), true)].clone());
        assert!(owner.capture_replica_runtime(foreign).is_err());
        assert!(owner == prior);
        assert_eq!(owner.replica_runtime("unknown", true).unwrap(), None);
        let store = Store::new(owner.clone(), Usage::new(["p"])).unwrap();
        let base = store.capture();
        let old = runtime_record::captured(&base, "p", true).unwrap().unwrap();
        let mut stale = owner.clone();
        stale.settings.disabled_system_packages[0].version_code += 1;
        assert!(stale.replica_runtime("p", true).is_err());
        assert!(matches!(
            store.publish(&base, stale, base.usage().clone()),
            Err(Error::Invalid(_))
        ));
        let mut changed_usage = base.usage().clone();
        changed_usage.notify("p", 0, 99);
        assert!(matches!(
            store.publish(&base, owner.clone(), changed_usage.clone()),
            Err(Error::Invalid(_))
        ));
        let mut refreshed = values();
        refreshed.get_mut(&("p".into(), false)).unwrap().usage[0] = 99;
        owner.capture_replica_runtime(refreshed).unwrap();
        let next = store.publish(&base, owner, changed_usage).unwrap();
        assert_eq!(
            next.owner()
                .replica_runtime("p", false)
                .unwrap()
                .unwrap()
                .usage[0],
            99
        );
        assert_eq!(
            base.owner()
                .replica_runtime("p", false)
                .unwrap()
                .unwrap()
                .usage[0],
            0
        );
        assert_eq!(
            runtime_record::captured(&base, "p", true).unwrap().unwrap(),
            old
        );
        assert!(runtime_record::captured(&next, "p", true).unwrap().unwrap() != old);
    }
    #[test]
    fn complete_user_import_preserves_explicit_aliases_and_rejects_invalid_sources() {
        let mut owner = owner();
        let shared = UserState::default();
        let mut values = BTreeMap::from([
            (
                ("p".into(), false),
                CapturedUsers {
                    states: BTreeMap::from([(10, shared.clone())]),
                    active_aliases: Default::default(),
                },
            ),
            (
                ("p".into(), true),
                CapturedUsers {
                    states: BTreeMap::from([(10, shared)]),
                    active_aliases: [10].into(),
                },
            ),
        ]);
        owner.capture_user_states(values.clone()).unwrap();
        let prior = owner.clone();
        values
            .get_mut(&("p".into(), true))
            .unwrap()
            .states
            .get_mut(&10)
            .unwrap()
            .enabled = 2;
        assert!(owner.capture_user_states(values.clone()).is_err());
        assert!(owner == prior);
        values
            .get_mut(&("p".into(), true))
            .unwrap()
            .active_aliases
            .clear();
        owner.capture_user_states(values.clone()).unwrap();
        let mut changed = UserState::default();
        changed.enabled = 3;
        owner.set_user_state("p", 10, changed.clone()).unwrap();
        assert_eq!(owner.disabled_user_states("p").unwrap()[&10].enabled, 2);
        values
            .get_mut(&("p".into(), true))
            .unwrap()
            .states
            .insert(10, changed.clone());
        values
            .get_mut(&("p".into(), false))
            .unwrap()
            .states
            .insert(10, changed);
        values
            .get_mut(&("p".into(), true))
            .unwrap()
            .active_aliases
            .insert(10);
        owner.capture_user_states(values.clone()).unwrap();
        let mut changed = UserState::default();
        changed.enabled = 4;
        owner.set_user_state("p", 10, changed).unwrap();
        assert_eq!(owner.disabled_user_states("p").unwrap()[&10].enabled, 4);
        let prior = owner.clone();
        values.remove(&("p".into(), true));
        assert!(owner.capture_user_states(values.clone()).is_err());
        assert!(owner == prior);
    }
    #[test]
    fn binder_runtime_pages_pin_scope_and_reject_closed_or_invalid_requests() {
        use crate::package::scan_snapshot::endpoint::{Endpoint, MAX_CHUNK};
        use aim_binder_driver::{
            Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*,
        };
        use aim_binder_host::{
            local::LocalProcess,
            parcel::{Binder, Parcel},
        };
        use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
        use std::sync::Arc;
        struct NoMemory;
        impl GuestProcess for NoMemory {
            fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
                Err(errno::EFAULT)
            }
            fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
                Err(errno::EFAULT)
            }
            fn get_file(&mut self, _: u32) -> Result<File, Errno> {
                Err(errno::EBADF)
            }
            fn install_file(&mut self, _: File) -> Result<u32, Errno> {
                Err(errno::EBADF)
            }
            fn close_fd(&mut self, _: u32) {
                panic!("unexpected fd")
            }
        }
        struct Processes(Arc<Driver>, Vec<Arc<LocalProcess>>);
        impl Drop for Processes {
            fn drop(&mut self) {
                for p in &self.1 {
                    self.0.release(p.proc_handle());
                }
            }
        }
        let mut owner = owner();
        let mut values = values();
        values
            .get_mut(&("p".into(), true))
            .unwrap()
            .library_files
            .push(Some("x".repeat(MAX_CHUNK * 2)));
        owner.capture_replica_runtime(values).unwrap();
        let store = Store::new(owner, Usage::new(["p"])).unwrap();
        let base = store.capture();
        let expected = runtime_record::captured(&base, "p", true).unwrap().unwrap();
        let endpoint = Arc::new(Endpoint::new(base.clone()));
        let mut next = base.owner().clone();
        let mut values = next.replica_runtime.as_ref().unwrap().values.clone();
        values.get_mut(&("p".into(), true)).unwrap().usage[0] = 29;
        next.capture_replica_runtime(values).unwrap();
        store.publish(&base, next, base.usage().clone()).unwrap();
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 1000,
                    security_context: None,
                },
            )
        };
        let server = open(97611);
        let client = open(97612);
        let _processes = Processes(driver.clone(), vec![server.clone(), client.clone()]);
        let Binder::Local(ptr) = server.add_service(endpoint) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                server.proc_handle(),
                97611,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        server.start();
        let remote = client.strong(0);
        let request = |name: Option<&str>, factory| {
            let mut p = Parcel::new();
            p.write_interface_token(api::DESCRIPTOR);
            p.write_string16(name);
            p.write_bool(factory);
            p
        };
        let reply = remote
            .transact(
                api::GET_RUNTIME_STATE_LENGTH,
                &request(Some("p"), true),
                false,
            )
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), expected.len() as i32);
        let mut bytes = Vec::new();
        while bytes.len() < expected.len() {
            let mut p = request(Some("p"), true);
            p.write_i32(bytes.len() as i32);
            p.write_i32(MAX_CHUNK as i32);
            let reply = remote
                .transact(api::GET_RUNTIME_STATE_CHUNK, &p, false)
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let chunk = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= MAX_CHUNK);
            bytes.extend(chunk);
            assert_eq!(r.remaining(), 0);
        }
        assert_eq!(bytes, expected);
        assert_ne!(
            bytes,
            runtime_record::captured(&store.capture(), "p", true)
                .unwrap()
                .unwrap()
        );
        let reply = remote
            .transact(
                api::GET_RUNTIME_STATE_LENGTH,
                &request(Some("unknown"), false),
                false,
            )
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), -1);
        let reply = remote
            .transact(api::GET_RUNTIME_STATE_LENGTH, &request(None, false), false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        let mut p = request(Some("p"), false);
        p.write_i32(1);
        assert!(
            remote
                .transact(api::GET_RUNTIME_STATE_LENGTH, &p, false)
                .is_err()
        );
        for (offset, length) in [(-1, 1), (0, 0), (0, MAX_CHUNK as i32 + 1), (i32::MAX, 1)] {
            let mut p = request(Some("p"), true);
            p.write_i32(offset);
            p.write_i32(length);
            let reply = remote
                .transact(api::GET_RUNTIME_STATE_CHUNK, &p, false)
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        let mut p = Parcel::new();
        p.write_interface_token(api::DESCRIPTOR);
        remote
            .transact(api::CLOSE, &p, false)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let reply = remote
            .transact(
                api::GET_RUNTIME_STATE_LENGTH,
                &request(Some("p"), true),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );
    }
    #[test]
    fn original_runtime_import_rejects_identity_scope_and_partial_sources_atomically() {
        let mut owner = owner();
        let mut source = crate::package::model::State::default();
        for ((name, factory), state) in values() {
            source.runtime_inputs.insert(
                (name.clone(), factory),
                OriginalRuntime {
                    name,
                    app_id: 10123,
                    path: if factory {
                        "/system/p".into()
                    } else {
                        "/data/p".into()
                    },
                    version: 0,
                    has_code: false,
                    state,
                    transient: crate::package::owner::transient::State {
                        hidden_until_installed: factory,
                        updated_system_app: factory,
                        apk_in_updated_apex: factory,
                        apex_module_name: factory.then(|| "factory-apex".into()),
                    },
                },
            );
        }
        let usage = Usage::new(["p"]);
        owner
            .complete_runtime_at_boot(&usage, source.runtime_inputs.clone())
            .unwrap();
        let initialized = owner.clone();
        assert!(
            owner
                .complete_runtime_at_boot(&Usage::new([]), source.runtime_inputs.clone())
                .is_err()
        );
        assert!(owner == initialized);
        assert!(
            owner
                .complete_runtime_at_boot(&usage, BTreeMap::new())
                .is_err()
        );
        assert!(owner == initialized);
        let mut mismatched = source.runtime_inputs.clone();
        mismatched
            .get_mut(&("p".into(), false))
            .unwrap()
            .state
            .usage[2] = 1;
        assert!(owner.complete_runtime_at_boot(&usage, mismatched).is_err());
        assert!(owner == initialized);
        owner.capture_original_runtime(&source).unwrap();
        let prior = owner.clone();
        assert!(
            owner.settings.disabled_system_packages[0]
                .transient
                .updated_system_app
        );
        assert!(!owner.settings.packages[0].transient.updated_system_app);
        for mode in 0..6 {
            let mut changed = source.clone();
            let input = changed.runtime_inputs.get_mut(&("p".into(), true)).unwrap();
            match mode {
                0 => input.name = "foreign".into(),
                1 => input.app_id += 1,
                2 => input.path = "/data/p".into(),
                3 => input.version += 1,
                4 => input.has_code = true,
                _ => {
                    changed.runtime_inputs.remove(&("p".into(), false));
                }
            }
            assert!(owner.capture_original_runtime(&changed).is_err());
            assert!(owner == prior);
            assert!(
                owner
                    .complete_runtime_at_boot(&usage, changed.runtime_inputs)
                    .is_err()
            );
            assert!(owner == prior);
        }
    }
}
