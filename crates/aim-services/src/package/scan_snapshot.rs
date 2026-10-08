//! Captured native scan owners for the C facade's replica builder (#836).
//! This is not the complete PackageState replica or its visibility policy.
use super::{
    owner::{app_ids::Owner, usage::Usage},
    scan::SigningScan,
};
pub(crate) mod computer;
pub mod endpoint;
pub mod library_record;
pub mod query_state;
mod components;
mod preferred_selection;
pub mod install_context;
mod removal;
pub mod boot_context;
mod diagnostics;
mod readonly_query;
mod retained_record;
pub mod runtime_record;
pub mod setting_record;
pub mod shared_record;
mod uid_record;
pub mod user_record;
pub(crate) mod version_page;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Stale,
    VersionExhausted,
}

/// Scan state and usage belong to this version, including shared UID signing,
/// keysets, libraries, user state and active/factory collected code.
#[derive(Debug)]
pub struct Snapshot {
    version: u64,
    owner: SigningScan,
    usage: Usage,
    replica_validated: bool,
    metadata_revision: u64,
}

/// Exact registered UID-slot projection for the facade's retained capture.
pub fn uid_owner_registry(snapshot: &Snapshot) -> Result<Vec<u8>, String> {
    uid_record::captured(snapshot)
}

impl Snapshot {
    pub(in crate::package) fn replica_runtime(&self,name:&str,factory:bool)->Result<Option<&crate::package::scan::ReplicaRuntime>,String>{
        // Store::create/publish validate the complete runtime/seinfo/library
        // graph once before this owner becomes immutable. Check this record's
        // membership without rebuilding every package identity table.
        let settings=if factory{&self.owner.settings.disabled_system_packages}else{&self.owner.settings.packages};
        if !settings.iter().any(|setting|setting.name==name){return Ok(None);}
        self.owner.snapshot_replica_runtime(name,factory)
    }
    /// Snapshot creation/publication validates these complete owner graphs before
    /// any immutable record access. Mutable SigningScan getters keep their gates.
    pub(in crate::package) fn install_permissions_fixed(&self, name: &str, factory: bool) -> Result<Option<bool>, String> {
        self.owner.validated_install_permissions_fixed(name, factory)
    }
    pub(in crate::package) fn legacy_permissions(&self, name: &str, factory: bool) -> Result<Option<crate::package::owner::legacy_permissions::State>, String> {
        self.owner.validated_legacy_permissions(name, factory)
    }
    pub fn metadata_revision(&self) -> u64 { self.metadata_revision }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
    pub fn owner(&self) -> &SigningScan {
        &self.owner
    }
}

#[derive(Debug)]
pub enum CommitError {
    Snapshot(Error),
    Disk { snapshot: Option<Arc<Snapshot>>, error: super::owner::WriteError },
}
pub struct Store {
    current: Mutex<Arc<Snapshot>>,
    replica: bool,
}

impl Store {
    /// The System publication gate supplies a snapshot already validated by its
    /// candidate Store. Keep the coordinator Arc stable across graph mutations.
    pub(crate) fn publish_validated_store(&self, candidate: &Store) {
        let snapshot = candidate.capture();
        *self.current.lock().unwrap() = snapshot;
    }
    /// Publish a validated candidate only while its exact canonical base remains
    /// current. Query projections may lag a disk-committed installation.
    pub(crate) fn publish_validated_store_after(
        &self, base: &Arc<Snapshot>, candidate: &Store,
    ) -> Result<Arc<Snapshot>, Error> {
        let snapshot = candidate.capture();
        let mut current = self.current.lock().unwrap();
        if !Arc::ptr_eq(&current, base) { return Err(Error::Stale); }
        if snapshot.version != base.version.checked_add(1).ok_or(Error::VersionExhausted)? {
            return Err(Error::Invalid("validated publication version differs".into()));
        }
        *current = snapshot.clone();
        Ok(snapshot)
    }

    pub(crate) fn prepare_usage_store(base: &Arc<Snapshot>, usage: Usage) -> Result<Self, Error> {
        if !base.replica_validated { return Err(Error::Invalid("usage base is not a validated replica".into())); }
        let mut owner = base.owner().clone();
        owner.update_validated_replica_usage(base.usage(), &usage).map_err(Error::Invalid)?;
        let version = base.version.checked_add(1).filter(|version| *version <= i64::MAX as u64).ok_or(Error::VersionExhausted)?;
        // Exact immutable validated owner plus a checked usage-only delta. Code,
        // settings, permissions and user inventories cannot have changed here.
        Ok(Self { current: Mutex::new(Arc::new(Snapshot {version,owner,usage,replica_validated:true,metadata_revision:base.metadata_revision})), replica:true })
    }

    /// A typed query-context delta may advance its Computer lease while the
    /// exact immutable package/usage owner remains unchanged.
    pub(crate) fn prepare_unchanged_metadata_store(base: &Arc<Snapshot>) -> Result<Self, Error> {
        Self::prepare_usage_store(base, base.usage().clone())
    }

    pub fn new(owner: SigningScan, usage: Usage) -> Result<Self, Error> {
        Self::create(owner, usage, false, 1)
    }

    pub fn new_replica(owner: SigningScan, usage: Usage) -> Result<Self, Error> {
        Self::new_replica_at_version(owner, usage, 1)
    }

    pub(crate) fn new_replica_at_version(
        owner: SigningScan,
        usage: Usage,
        version: u64,
    ) -> Result<Self, Error> {
        Self::create(owner, usage, true, version)
    }

    fn create(
        owner: SigningScan,
        usage: Usage,
        replica: bool,
        version: u64,
    ) -> Result<Self, Error> {
        if version == 0 || version > i64::MAX as u64 {
            return Err(Error::VersionExhausted);
        }
        validate(&owner, &usage)?;
        let snapshot = Arc::new(Snapshot {
            version,
            owner,
            usage,
            replica_validated: replica,
            metadata_revision: version,
        });
        if replica {
            validate_replica(&snapshot)?;
        }
        Ok(Self {
            current: Mutex::new(snapshot),
            replica,
        })
    }

    pub fn capture(&self) -> Arc<Snapshot> {
        self.current.lock().unwrap().clone()
    }

    /// Persist while the exact base remains locked. The callback performs disk
    /// work only; Binder calls and cache/broadcast effects happen after return.
    pub fn publish_after(
        &self, base:&Arc<Snapshot>, owner:SigningScan, usage:Usage,
        persist:impl FnOnce(&Arc<Snapshot>)->Result<(),super::owner::WriteError>,
    )->Result<Arc<Snapshot>,CommitError> {
        let mut current=self.current.lock().unwrap();
        if !Arc::ptr_eq(&current,base) {return Err(CommitError::Snapshot(Error::Stale));}
        let version=current.version.checked_add(1).filter(|version|*version<=i64::MAX as u64).ok_or(CommitError::Snapshot(Error::VersionExhausted))?;
        validate(&owner,&usage).map_err(CommitError::Snapshot)?;
        let next=Arc::new(Snapshot {version,owner,usage,replica_validated:self.replica,metadata_revision:version});
        if self.replica {validate_replica(&next).map_err(CommitError::Snapshot)?;}
        match persist(&next) {
            Ok(())=>{*current=next.clone();Ok(next)},
            Err(error) if error.committed=>{*current=next.clone();Err(CommitError::Disk {snapshot:Some(next),error})},
            Err(error)=>Err(CommitError::Disk {snapshot:None,error}),
        }
    }

    /// The commit owner supplies a completed candidate. An exact captured base
    /// prevents stale or foreign transactions from replacing a newer graph.
    /// Files, broadcasts and permission callbacks still belong to that owner.
    pub fn publish(
        &self,
        base: &Arc<Snapshot>,
        owner: SigningScan,
        usage: Usage,
    ) -> Result<Arc<Snapshot>, Error> {
        let mut current = self.current.lock().unwrap();
        if !Arc::ptr_eq(&current, base) {
            return Err(Error::Stale);
        }
        let version = current
            .version
            .checked_add(1)
            .filter(|version| *version <= i64::MAX as u64)
            .ok_or(Error::VersionExhausted)?;
        validate(&owner, &usage)?;
        let next = Arc::new(Snapshot {
            version,
            owner,
            usage,
            replica_validated: self.replica,
            metadata_revision: version,
        });
        if self.replica {
            validate_replica(&next)?;
        }
        *current = next.clone();
        Ok(next)
    }
}

fn validate_replica(snapshot: &Snapshot) -> Result<(), Error> {
    let owner = snapshot.owner();
    if !owner.has_legacy_permissions() || !owner.has_shared_processes() {
        return Err(Error::Invalid(
            "replica requires legacy and shared process owners".into(),
        ));
    }
    let missing = || Error::Invalid("replica setting input is missing".into());
    for (settings, factory) in [
        (&owner.settings.packages, false),
        (&owner.settings.disabled_system_packages, true),
    ] {
        for setting in settings {
            let name = &setting.name;
            snapshot
                .install_permissions_fixed(name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            setting_record::captured(snapshot, name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            runtime_record::captured(snapshot, name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            endpoint::PackageSigningState::captured(snapshot, name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            let ids = user_record::ids(snapshot, name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            for id in ids {
                user_record::captured(snapshot, name, factory, id)
                    .map_err(Error::Invalid)?
                    .ok_or_else(missing)?;
            }
            owner
                .hidden_api_enforcement_policy(name, factory)
                .map_err(Error::Invalid)?
                .ok_or_else(missing)?;
            endpoint::PackageCode::captured(snapshot, name, factory).map_err(Error::Invalid)?;
            if !factory && owner.loaded_packages().contains_key(name) {
                owner
                    .validated_seinfo_state(name)
                    .map_err(Error::Invalid)?
                    .ok_or_else(missing)?;
                owner
                    .validated_library_dependencies(name)
                    .map_err(Error::Invalid)?
                    .ok_or_else(missing)?;
            }
        }
    }
    for name in owner.identities.shared_users.keys() {
        shared_record::captured(snapshot, name)
            .map_err(Error::Invalid)?
            .ok_or_else(missing)?;
    }
    Ok(())
}

fn validate(owner: &SigningScan, usage: &Usage) -> Result<(), Error> {
    let fail = |message: &str| Error::Invalid(message.into());
    if !owner.capture_ready() {
        return Err(fail("scan metadata is not finalized"));
    }
    owner
        .identities
        .ids
        .validate_detached()
        .map_err(Error::Invalid)?;
    owner
        .identities
        .ordered_shared_users()
        .map_err(Error::Invalid)?;
    owner
        .validate_displaced_shared_settings()
        .map_err(Error::Invalid)?;
    owner.validate_seinfo().map_err(Error::Invalid)?;
    owner
        .validate_replica_runtime(Some(usage))
        .map_err(Error::Invalid)?;
    owner.validate_shared_processes().map_err(Error::Invalid)?;
    owner
        .validate_library_dependencies()
        .map_err(Error::Invalid)?;
    owner
        .validate_legacy_permissions()
        .map_err(Error::Invalid)?;
    let package_names: BTreeSet<_> = owner
        .settings
        .packages
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    if usage.names().collect::<BTreeSet<_>>() != package_names {
        return Err(fail("usage package membership differs"));
    }
    for (settings, loaded, active) in [
        (&owner.settings.packages, owner.loaded_packages(), true),
        (
            &owner.settings.disabled_system_packages,
            owner.disabled_loaded_packages(),
            false,
        ),
    ] {
        let mut names = BTreeSet::new();
        for setting in settings {
            let normalized = setting
                .install_source
                .clone()
                .normalized()
                .map_err(Error::Invalid)?;
            if normalized != setting.install_source {
                return Err(fail("install source is not normalized"));
            }

            let mut mime_names = BTreeSet::new();
            if setting
                .mime_groups
                .iter()
                .any(|(name, _)| !mime_names.insert(name))
            {
                return Err(fail("duplicate MIME group owner"));
            }
            if !names.insert(setting.name.as_str()) {
                return Err(fail("duplicate package setting"));
            }
            if active
                && !owner
                    .is_displaced_shared_setting(setting)
                    .map_err(Error::Invalid)?
            {
                match owner.identities.ids.get(setting.uid_owner_id()) {
                    Some(Owner::Package(_) | Owner::DetachedPackage(_))
                        if owner.identities.ids.owns_package_slot(setting) => {}
                    Some(Owner::SharedUser(name)) if setting.shared_user => {
                        let group = owner
                            .identities
                            .shared_users
                            .get(name)
                            .ok_or_else(|| fail("missing shared UID owner"))?;
                        if Some(group.app_id) != setting.shared_app_id()
                            || !group.has_package(&setting.name)
                        {
                            return Err(fail("package shared UID membership differs"));
                        }
                    }
                    None if !setting.shared_user && setting.app_id == -1 => {
                        let code = loaded
                            .get(&setting.name)
                            .filter(|code| code.package.is2(super::pkg::booleans2::APEX))
                            .ok_or_else(|| fail("unregistered setting has no APEX code owner"))?;
                        owner
                            .validate_collected_uid(setting, &code.package, true)
                            .map_err(Error::Invalid)?;
                    }
                    _ => return Err(fail("package UID owner differs")),
                }
            }
        }
        for (name, code) in loaded {
            code.package
                .validate_cache_constructor_fields()
                .map_err(Error::Invalid)?;
            let setting = settings
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| fail("loaded code has no setting"))?;
            owner
                .validate_collected_uid(setting, &code.package, active)
                .map_err(Error::Invalid)?;
            code.validate_setting(setting, !active)
                .map_err(Error::Invalid)?;
            if code.package.signing_details
                != code
                    .collected_signing
                    .package_details()
                    .map_err(Error::Invalid)?
            {
                return Err(fail("loaded code differs from its owner"));
            }
            let users = if active {
                owner.scanned_user_states(name)
            } else {
                owner.disabled_user_states(name)
            };
            if users.is_none() {
                return Err(fail("loaded code has no captured user state"));
            }
        }
    }
    let mut groups = BTreeSet::new();
    for saved in &owner.settings.shared_users {
        if !groups.insert(saved.name.as_str()) {
            return Err(fail("duplicate shared UID setting"));
        }
        let group = owner
            .identities
            .shared_users
            .get(&saved.name)
            .ok_or_else(|| fail("missing saved shared UID owner"))?;
        group.validate_retained().map_err(Error::Invalid)?;
        if group.app_id != saved.app_id || group.signatures != saved.signatures {
            return Err(fail("saved shared UID signing differs from its owner"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, Settings};

    fn owner() -> SigningScan {
        SigningScan::new(
            &Default::default(),
            &Settings {
                packages: vec![Package {
                    name: "fixture".into(),
                    app_id: 10100,
                    code_path: "/data/app/fixture".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            36,
        )
        .unwrap()
    }

    #[test]
    fn raw_metadata_lease_preserves_source_and_rejects_live_queries_until_retirement() {
        use aim_binder_host::{local::{Call,Service},parcel::{Parcel,Reader,EX_ILLEGAL_STATE}};
        use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
        let store=Store::new(owner(),Usage::new(["fixture"])).unwrap();
        let initial=store.capture();let raw=endpoint::Endpoint::raw_metadata(initial.clone());
        let call=|endpoint:&endpoint::Endpoint,code:u32,request:&Parcel|{
            endpoint.transact(&mut Call{code,flags:0,sender_pid:10,sender_euid:1000,data:Reader::new(request.data(),request.objects())}).unwrap()
        };
        let mut names=Parcel::new();api::GetPackageNames{disabled:false}.write(&mut names);
        let reply=call(&raw,api::GET_PACKAGE_NAMES,&names);
        assert_eq!(api::read_get_package_names_reply(&mut Reader::new(reply.data(),reply.objects())).unwrap().unwrap(),Some(vec![Some("fixture".into())]));
        let mut version=Parcel::new();api::GetVersion{}.write(&mut version);
        let reply=call(&raw,api::GET_VERSION,&version);
        assert_eq!(api::read_get_version_reply(&mut Reader::new(reply.data(),reply.objects())).unwrap().unwrap(),initial.version() as i64);
        let mut candidate=initial.owner().clone();candidate.settings.packages[0].version_code=37;
        let full=store.publish(&initial,candidate,initial.usage().clone()).unwrap();
        assert!(full.version()>initial.version());assert_eq!(initial.owner().settings.packages[0].version_code,0);
        let reply=call(&raw,api::GET_VERSION,&version);
        assert_eq!(api::read_get_version_reply(&mut Reader::new(reply.data(),reply.objects())).unwrap().unwrap(),initial.version() as i64);
        let mut query=Parcel::new();api::GetComputer{}.write(&mut query);
        let reply=call(&raw,api::GET_COMPUTER,&query);
        let error=Reader::new(reply.data(),reply.objects()).read_exception().unwrap().unwrap_err();
        assert_eq!(error.code,EX_ILLEGAL_STATE);assert_eq!(error.message,"live permission query capture is not initialized");
        // Session abort/death calls this same retirement boundary.
        raw.close_lease();
        let reply=call(&raw,api::GET_PACKAGE_NAMES,&names);
        assert_eq!(Reader::new(reply.data(),reply.objects()).read_exception().unwrap().unwrap_err().message,"package scan snapshot is closed");
        assert_eq!(full.owner().settings.packages[0].version_code,37);
        let second=endpoint::Endpoint::raw_metadata(initial.clone());
        let mut close=Parcel::new();api::Close{}.write(&mut close);
        let reply=call(&second,api::CLOSE,&close);
        Reader::new(reply.data(),reply.objects()).read_exception().unwrap().unwrap();
        let reply=call(&second,api::GET_VERSION,&version);
        assert_eq!(Reader::new(reply.data(),reply.objects()).read_exception().unwrap().unwrap_err().message,"package scan snapshot is closed");
    }

    #[test]
    fn replica_publication_requires_every_setting_owner_on_every_version() {
        use std::collections::BTreeMap;
        let complete = |users: bool, fixed: bool, runtime: bool| {
            let mut owner = owner();
            if users {
                owner
                    .capture_user_states(BTreeMap::from([(
                        ("fixture".into(), false),
                        super::super::scan::CapturedUsers {
                            states: Default::default(),
                            active_aliases: Default::default(),
                        },
                    )]))
                    .unwrap();
            }
            let groups = owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect();
            owner
                .capture_legacy_permissions(
                    &[0],
                    BTreeMap::from([(("fixture".into(), false), Default::default())]),
                    groups,
                )
                .unwrap();
            if fixed {
                owner
                    .capture_install_permissions_fixed(BTreeMap::from([(
                        ("fixture".into(), false),
                        true,
                    )]))
                    .unwrap();
            }
            let orders = owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), vec![]))
                .collect();
            owner.complete_shared_processes(orders).unwrap();
            if runtime {
                owner
                    .capture_replica_runtime(BTreeMap::from([(
                        ("fixture".into(), false),
                        super::super::scan::ReplicaRuntime {
                            usage: [0; 8],
                            seinfo: None,
                            override_seinfo: None,
                            library_files: vec![],
                            libraries: vec![],
                        },
                    )]))
                    .unwrap();
            }
            owner
        };
        assert!(Store::new_replica(owner(), Usage::new(["fixture"])).is_err());
        let store =
            Store::new_replica(complete(true, true, true), Usage::new(["fixture"])).unwrap();
        let base = store.capture();
        for candidate in [
            complete(false, true, true),
            complete(true, false, true),
            complete(true, true, false),
        ] {
            assert!(Store::new(candidate.clone(), Usage::new(["fixture"])).is_ok());
            assert!(Store::new_replica(candidate.clone(), Usage::new(["fixture"])).is_err());
            assert!(
                store
                    .publish(&base, candidate, Usage::new(["fixture"]))
                    .is_err()
            );
            assert!(Arc::ptr_eq(&base, &store.capture()));
        }
        let next = store
            .publish(&base, complete(true, true, true), Usage::new(["fixture"]))
            .unwrap();
        assert_eq!(next.version(), 2);
        assert_eq!(base.version(), 1);
        for version in [0, i64::MAX as u64 + 1] {
            assert!(matches!(
                Store::new_replica_at_version(
                    complete(true, true, true),
                    Usage::new(["fixture"]),
                    version
                ),
                Err(Error::VersionExhausted)
            ));
        }
        let limit = Store::new_replica_at_version(
            complete(true, true, true),
            Usage::new(["fixture"]),
            i64::MAX as u64,
        )
        .unwrap();
        let last = limit.capture();
        assert!(matches!(
            limit.publish(&last, complete(true, true, true), Usage::new(["fixture"])),
            Err(Error::VersionExhausted)
        ));
        assert!(Arc::ptr_eq(&last, &limit.capture()));
    }

    #[test]
    fn transient_owners_are_separate_from_flags_factories_and_later_versions() {
        use aim_service_aidl::WriteParcelable;
        use endpoint::PackageTransientState;
        let mut owner = owner();
        let mut factory = owner.settings.packages[0].clone();
        factory.flags = 128;
        owner.settings.disabled_system_packages.push(factory);
        owner.settings.packages[0].transient = super::super::owner::transient::State {
            hidden_until_installed: true,
            updated_system_app: true,
            apk_in_updated_apex: true,
            apex_module_name: Some("module".into()),
        };
        let prior = owner.clone();
        assert!(owner.capture_transient_states(Default::default()).is_err());
        assert_eq!(owner, prior);
        let active = owner.settings.packages[0].transient.clone();
        let mut inputs = std::collections::BTreeMap::from([
            (("fixture".into(), false), active.clone()),
            (("foreign".into(), true), Default::default()),
        ]);
        assert!(owner.capture_transient_states(inputs.clone()).is_err());
        assert_eq!(owner, prior);
        inputs.remove(&("foreign".into(), true));
        inputs.insert(("fixture".into(), true), Default::default());
        owner.capture_transient_states(inputs).unwrap();
        assert_eq!(owner.settings.packages[0].transient, active);
        let store = Store::new(owner, Usage::new(["fixture"])).unwrap();
        let old = store.capture();
        let mut changed = old.owner().clone();
        changed.settings.packages[0].transient = Default::default();
        let new = store.publish(&old, changed, old.usage().clone()).unwrap();
        for (capture, disabled, expected) in [
            (&old, false, true),
            (&old, true, false),
            (&new, false, false),
        ] {
            let state = PackageTransientState::captured(capture, "fixture", disabled).unwrap();
            let mut p = aim_binder_host::parcel::Parcel::new();
            state.write_to(&mut p);
            let mut r = aim_binder_host::parcel::Reader::new(p.data(), p.objects());
            assert_eq!(r.read_i64().unwrap(), capture.version() as i64);
            assert_eq!(r.read_string16().unwrap().as_deref(), Some("fixture"));
            assert_eq!(r.read_i32().unwrap(), 10100);
            assert_eq!(r.read_bool().unwrap(), disabled);
            for _ in 0..3 {
                assert_eq!(r.read_bool().unwrap(), expected);
            }
            assert_eq!(
                r.read_string16().unwrap().as_deref(),
                expected.then_some("module")
            );
            assert_eq!(r.remaining(), 0);
        }
        assert!(PackageTransientState::captured(&old, "unknown", false).is_none());
        assert!(
            old.owner().settings.packages[0]
                .transient
                .updated_system_app
        );
        assert!(
            !new.owner().settings.packages[0]
                .transient
                .updated_system_app
        );
    }

    #[test]
    fn usage_is_mandatory_matches_membership_and_isolated_between_versions() {
        assert!(matches!(
            Store::new(owner(), Usage::new([])),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Store::new(owner(), Usage::new(["fixture", "foreign"])),
            Err(Error::Invalid(_))
        ));
        let mut usage = Usage::new(["fixture"]);
        usage.apply(b"fixture 17\n").unwrap();
        let store = Store::new(owner(), usage).unwrap();
        let old = store.capture();
        assert!(matches!(
            store.publish(&old, owner(), Usage::new(["foreign"])),
            Err(Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&old, &store.capture()));
        assert_eq!(old.usage().times("fixture"), Some(&[17; 8]));
        let mut next = old.usage().clone();
        next.notify("fixture", 2, 29);
        let current = store.publish(&old, owner(), next.clone()).unwrap();
        next.notify("fixture", 2, 44);
        assert_eq!(old.usage().latest_foreground("fixture"), Some(17));
        assert_eq!(current.usage().latest_foreground("fixture"), Some(29));
        assert_eq!(current.usage().latest("foreign"), None);
        assert_eq!(
            store.publish(&old, owner(), next).unwrap_err(),
            Error::Stale
        );
        assert!(Arc::ptr_eq(&current, &store.capture()));
    }

    #[test]
    fn invalid_install_source_cannot_publish_over_a_capture() {
        let store = Store::new(owner(), Usage::new(["fixture"])).unwrap();
        let base = store.capture();
        for signed in [false, true] {
            let mut candidate = base.owner().clone();
            let source = &mut candidate.settings.packages[0].install_source;
            if signed {
                source.initiating_package_signatures = Some(Default::default());
            } else {
                source.installer_uid = 123;
            }
            assert!(matches!(
                store.publish(&base, candidate, base.usage().clone()),
                Err(Error::Invalid(_))
            ));
            assert!(Arc::ptr_eq(&store.capture(), &base));
        }
    }

    #[test]
    fn capture_survives_updates_and_stale_foreign_or_invalid_commits() {
        let store = Store::new(owner(), Usage::new(["fixture"])).unwrap();
        let original = store.capture();
        let foreign = Store::new(owner(), Usage::new(["fixture"]))
            .unwrap()
            .capture();
        assert_eq!(
            store
                .publish(&foreign, owner(), Usage::new(["fixture"]))
                .unwrap_err(),
            Error::Stale
        );
        let mut invalid = owner();
        invalid.settings.packages[0].app_id += 1;
        assert!(matches!(
            store.publish(&original, invalid, original.usage().clone()),
            Err(Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&store.capture(), &original));
        let mut candidate = original.owner().clone();
        candidate.settings.packages[0].version_code = 2;
        let next = store
            .publish(&original, candidate.clone(), original.usage().clone())
            .unwrap();
        candidate.settings.packages[0].version_code = 3;
        assert_eq!(original.owner().settings.packages[0].version_code, 0);
        assert_eq!(next.owner().settings.packages[0].version_code, 2);
        assert_eq!(next.version(), 2);
        assert_eq!(
            store
                .publish(&original, candidate, original.usage().clone())
                .unwrap_err(),
            Error::Stale
        );
    }

    #[test]
    fn concurrent_commits_from_one_capture_have_one_winner_and_overflow_preserves_state() {
        let store = Arc::new(Store::new(owner(), Usage::new(["fixture"])).unwrap());
        let base = store.capture();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = vec![];
        for version in [11, 12] {
            let (store, base, barrier) = (store.clone(), base.clone(), barrier.clone());
            handles.push(std::thread::spawn(move || {
                let mut candidate = base.owner().clone();
                candidate.settings.packages[0].version_code = version;
                barrier.wait();
                store.publish(&base, candidate, base.usage().clone())
            }));
        }
        barrier.wait();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(Error::Stale)))
                .count(),
            1
        );
        let exhausted = Arc::new(Snapshot {
            version: i64::MAX as u64,
            owner: owner(),
            usage: Usage::new(["fixture"]),
            replica_validated: false,
            metadata_revision: i64::MAX as u64,
        });
        *store.current.lock().unwrap() = exhausted.clone();
        assert_eq!(
            store
                .publish(&exhausted, owner(), exhausted.usage().clone())
                .unwrap_err(),
            Error::VersionExhausted
        );
        assert!(Arc::ptr_eq(&store.capture(), &exhausted));
    }
}
