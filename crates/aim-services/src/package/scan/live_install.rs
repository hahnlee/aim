//! Live install settings admission, android-16.0.0_r1 InstallPackageHelper,
//! KeySetManagerService and PackageManagerServiceUtils (AOSP, Apache-2.0).
//! Changes stay in a cloned native scan owner until metadata/effects complete.
use super::{
    Identity, NewPackageOutcome, NewSetting, Record, SettingMetadata, SigningScan, Uid, UserPolicy,
};
use crate::package::{
    owner::{app_ids::Owner, shared_users::ScanOrigin},
    pkg::{AndroidPackage, booleans},
    restrictions::UserState,
    settings::{self, Settings},
    sign::{History, JoinType, SigningDetails},
};
use std::collections::BTreeSet;

pub struct Request {
    pub code: AndroidPackage,
    pub signing: SigningDetails,
    pub metadata: SettingMetadata,
    pub install_flags: i32,
    pub required_installed_version: i64,
    pub rollback: bool,
    pub source: settings::InstallSource,
    pub install_user: i32,
    pub now: i64,
}
#[derive(Debug)]
pub struct Rejection {
    pub status: i32,
    pub message: String,
}
fn reject(status: i32, message: impl Into<String>) -> Rejection {
    Rejection {
        status,
        message: message.into(),
    }
}
pub struct Admission {
    pub owner: SigningScan,
    pub candidates: Vec<NewPackageOutcome>,
}
pub struct CompletedAdmission {
    pub owner: SigningScan,
    pub completed: Vec<super::CompletedScanMetadata>,
}
pub struct Publication {
    pub snapshot: std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    pub completed: Vec<super::CompletedScanMetadata>,
}
pub enum PublicationFailure {
    Persist {
        completed: Vec<super::CompletedScanMetadata>,
        error: crate::package::scan_snapshot::CommitError,
    },
}
impl CompletedAdmission {
    /// The caller holds its data-directory commit coordinator. This writes the
    /// actual settings and users before making the new native generation visible.
    pub fn persist_publish(
        self,
        snapshots: &crate::package::scan_snapshot::Store,
        base: &std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
        disk: &mut crate::package::owner::Store,
        usage: crate::package::owner::usage::Usage,
        users: &[u32],
        cross_user: bool,
    ) -> Result<Publication, PublicationFailure> {
        self.persist_with(snapshots,base,disk,Some(usage),users,cross_user)
    }
    pub fn persist_publish_install(
        self,snapshots:&crate::package::scan_snapshot::Store,
        base:&std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
        disk:&mut crate::package::owner::Store,users:&[u32],cross_user:bool,
    )->Result<Publication,PublicationFailure>{
        self.persist_with(snapshots,base,disk,None,users,cross_user)
    }
    fn persist_with(
        self,snapshots:&crate::package::scan_snapshot::Store,
        base:&std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
        disk:&mut crate::package::owner::Store,usage:Option<crate::package::owner::usage::Usage>,
        users:&[u32],cross_user:bool,
    )->Result<Publication,PublicationFailure>{
        let packages: std::collections::BTreeSet<_> = self
            .completed
            .iter()
            .map(|metadata| metadata.candidate.record.settings.name.clone())
            .collect();
        let persist = |snapshot:&std::sync::Arc<crate::package::scan_snapshot::Snapshot>| {
            disk.commit_scan_settings(snapshot)?;
            for user in users {
                if let Err(mut error) = disk.commit_live_install_restrictions(
                    snapshot.owner(),
                    *user,
                    &packages,
                    cross_user,
                ) {
                    // Global settings committed before every per-user write.
                    error.committed = true;
                    return Err(error);
                }
            }
            Ok(())
        };
        let result=match usage{
            Some(usage)=>snapshots.publish_after(base,self.owner,usage,persist),
            None=>snapshots.publish_install_after(base,self.owner,persist),
        };
        match result {
            Ok(snapshot) => Ok(Publication {
                snapshot,
                completed: self.completed,
            }),
            Err(error) => Err(PublicationFailure::Persist {
                completed: self.completed,
                error,
            }),
        }
    }
}
pub struct CompletionFailure {
    pub admission: Box<Admission>,
    pub completed: Vec<super::CompletedScanMetadata>,
    pub error: super::SigningError,
}
impl Admission {
    /// Run live installation ABI/label/library/metadata owners for every admitted
    /// member. Earlier copies are returned on failure for the transaction's cleanup.
    pub fn complete_metadata<'a>(
        mut self,
        apks: &crate::package::write::Apks,
        inputs: Vec<super::ScanMetadataCompletion<'a>>,
    ) -> Result<CompletedAdmission, CompletionFailure> {
        if inputs.len() != self.candidates.len()
            || inputs
                .iter()
                .any(|input| !matches!(input.context.mode, super::AbiScanMode::Install { .. }))
        {
            return Err(CompletionFailure {
                admission: Box::new(self),
                completed: vec![],
                error: super::SigningError::Fatal(super::Error {
                    package: String::new(),
                    path: String::new(),
                    phase: "live-install-metadata",
                    message: "Live install completion requires one INSTALL-mode input per member"
                        .into(),
                }),
            });
        }
        let mut completed = Vec::new();
        let candidates = std::mem::take(&mut self.candidates);
        for (candidate, input) in candidates.into_iter().zip(inputs) {
            match self.owner.finish_scan_metadata(candidate, apks, input) {
                Ok(metadata) => completed.push(metadata),
                Err(error) => {
                    return Err(CompletionFailure {
                        admission: Box::new(self),
                        completed,
                        error,
                    });
                }
            }
        }
        Ok(CompletedAdmission {
            owner: self.owner,
            completed,
        })
    }
}
impl SigningScan {
    /// Atomic batch admission. Failure consumes no live UID or shared signer;
    /// the returned owner remains unpublishable until complete scan metadata.
    pub fn prepare_live_installs(
        &self,
        requests: Vec<Request>,
        users: &[super::User],
        build_debuggable: bool,
        files: &crate::package::write::Files,
    ) -> Result<Admission, Rejection> {
        if !self.capture_ready() {
            return Err(reject(
                -110,
                "Live install base has unfinished scan metadata",
            ));
        }
        let mut owner = self.clone();
        let mut candidates = Vec::new();
        let mut names = BTreeSet::new();
        for request in requests {
            let identity = Identity::select(&request.code, &owner.settings, false);
            if !names.insert(identity.internal_name.clone()) {
                return Err(reject(-2, "Duplicate package in atomic install"));
            }
            let candidate = owner.admit_live(request, identity, users, build_debuggable, files)?;
            candidates.push(candidate);
        }
        Ok(Admission { owner, candidates })
    }
    fn admit_live(
        &mut self,
        request: Request,
        identity: Identity,
        users: &[super::User],
        build_debuggable: bool,
        files: &crate::package::write::Files,
    ) -> Result<NewPackageOutcome, Rejection> {
        let name = identity.internal_name.clone();
        if !request.metadata.code_path.starts_with("/data/app/")
            || request
                .metadata
                .code_path
                .split('/')
                .any(|part| matches!(part, "." | ".."))
        {
            return Err(reject(
                -2,
                "Live install destination is not an owned data-app location",
            ));
        }
        if request.signing.unknown || request.signing.signatures.is_empty() {
            return Err(reject(
                -103,
                "Install code has no verified signing identity",
            ));
        }
        let at = self
            .settings
            .packages
            .iter()
            .position(|package| package.name == name);
        let old = at.map(|at| self.settings.packages[at].clone());
        let version = ((request.code.version_code_major as i64) << 32)
            | (request.code.version_code as u32 as i64);
        let system = old
            .as_ref()
            .is_some_and(|old| old.flags & settings::FLAG_SYSTEM != 0);
        if (request.metadata.flags & settings::FLAG_SYSTEM != 0) != system {
            return Err(reject(-2, "Install metadata changes system ownership"));
        }
        if request.code.path.as_deref() != Some(request.metadata.code_path.as_str()) {
            return Err(reject(
                -2,
                "Verified code is not at its reserved installation path",
            ));
        }
        if request.metadata.version_code != version
            || request.metadata.target_sdk_version != request.code.target_sdk_version
        {
            return Err(reject(-2, "Install metadata differs from verified code"));
        }
        if request.required_installed_version != -1
            && old.as_ref().is_none_or(|old| {
                old.version_code != request.required_installed_version
                    || !self.loaded.contains_key(&name)
            })
        {
            return Err(reject(-121, "Wrong required installed version"));
        }
        if let Some(old) = &old {
            if request.install_flags & 2 == 0 {
                return Err(reject(-1, "Package already exists"));
            }
            check_update_identity(&self.settings, old, &request.code)?;
            let debug = self
                .loaded
                .get(&name)
                .map_or(old.debuggable, |code| code.package.is(booleans::DEBUGGABLE));
            if !old.is_sdk_library
                && !(request.install_flags & 0x80 != 0
                    && (build_debuggable || debug || request.install_flags & 0x100000 != 0))
            {
                check_downgrade(old, &request.code)?;
            }
            if system
                && request.install_flags & 0x80 != 0
                && (build_debuggable || debug || request.install_flags & 0x100000 != 0)
            {
                let factory = self
                    .settings
                    .disabled_system_packages
                    .iter()
                    .find(|factory| factory.name == name)
                    .unwrap_or(old);
                let factory_code = self
                    .disabled_loaded
                    .get(&name)
                    .or_else(|| self.loaded.get(&name))
                    .ok_or_else(|| reject(-110, "System factory code owner unavailable"))?;
                if !build_debuggable && !factory_code.package.is(booleans::DEBUGGABLE) {
                    check_downgrade(factory, &request.code)?;
                }
            }
            authorize_update(&self.settings, old, &request.signing, request.rollback)?;
            if let Some(expected) = &old.restrict_update_hash {
                if package_digest(files, &request.code)? != *expected {
                    return Err(reject(-2, format!("New package fails restrict-update check: {name}")));
                }
            }
        }
        let declared = super::signing::selected_shared_user(
            old.as_ref().is_some_and(|old| old.shared_user),
            request.code.shared_user_id.as_deref(),
            request.code.is(booleans::LEAVING_SHARED_UID),
        )
        .map(str::to_owned);
        authorize_shared(
            &self.settings,
            &name,
            declared.as_deref(),
            &request.signing,
            old.is_some(),
        )?;
        if system {
            self.disable_system_package(&name)
                .map_err(|error| reject(-110, format!("System factory retention: {error:?}")))?;
        }
        let old = at.map(|at| self.settings.packages[at].clone());
        let saved_users = if old.is_some() {
            self.scanned_users
                .get(&name)
                .cloned()
                .ok_or_else(|| reject(-110, "Installed package user owner unavailable"))?
        } else {
            Default::default()
        };
        let mut setting = if let Some(old) = &old {
            NewSetting::update(
                old,
                &saved_users,
                super::SettingUpdate {
                    code_path: request.metadata.code_path.clone(),
                    legacy_native_library_path: request.metadata.legacy_native_library_path.clone(),
                    primary_cpu_abi: request.metadata.primary_cpu_abi.clone(),
                    secondary_cpu_abi: request.metadata.secondary_cpu_abi.clone(),
                    flags: request.metadata.flags,
                    private_flags: request.metadata.private_flags,
                    uses_sdk_libraries: request.metadata.uses_sdk_libraries.clone(),
                    uses_static_libraries: request.metadata.uses_static_libraries.clone(),
                    mime_groups: request.metadata.mime_groups.clone(),
                    domain_set_id: request.metadata.domain_set_id,
                    target_sdk_version: request.metadata.target_sdk_version,
                    restrict_update_hash: request.metadata.restrict_update_hash.clone(),
                },
                Some(users),
                self.settings
                    .disabled_system_packages
                    .iter()
                    .any(|factory| factory.name == name),
            )
        } else {
            let app_id = if let Some(group) = &declared {
                let (app_id, flags, signatures) = {
                    let shared = self.identities.get_shared_user(group,0,0,true)
                        .map_err(|error|reject(-4,format!("Cannot reserve shared UID: {error:?}")))?.unwrap();
                    (shared.app_id,shared.flags,shared.signatures.clone())
                };
                if !self
                    .settings
                    .shared_users
                    .iter()
                    .any(|saved| saved.name == *group)
                {
                    self.settings.shared_users.push(settings::SharedUser {
                        name: group.clone(),
                        app_id,
                        flags,
                        signatures,
                    });
                    self.legacy_shared_constructor(group,app_id)
                        .map_err(|error|reject(-110,format!("Live shared setting constructor: {error}")))?;
                }
                app_id
            } else {
                self.identities
                    .ids
                    .acquire(Owner::Package(name.clone()))
                    .map_err(|error| reject(-4, format!("Cannot reserve app UID: {error:?}")))?
            };
            NewSetting::new(
                &identity,
                &Uid {
                    app_id,
                    shared_user: declared.clone(),
                },
                request.metadata.clone(),
                UserPolicy {
                    install_user: Some(request.install_user),
                    users: Some(users),
                    allow_install: true,
                    instant_app: request.install_flags & 0x800 != 0,
                    virtual_preload: request.install_flags & 0x10000 != 0,
                    stopped_system_app: false,
                },
            )
        };
        for user in users {
            if request.install_user == user.id
                || request.install_user == -1 && !user.pre_created && !user.adb_install_disallowed
            {
                let state = setting
                    .users
                    .entry(user.id)
                    .or_insert_with(UserState::initialized);
                state.installed = true;
                state.uninstall_reason = 0;
                state.instant_app = request.install_flags & 0x800 != 0;
                if state.first_install_time == 0 {
                    state.first_install_time = request.now;
                }
            }
        }
        if let Some(old) = &old {
            setting.package.restrict_update_hash = old.restrict_update_hash.clone();
        }
        setting.package.version_code = version;
        setting.package.base_revision_code = request.code.base_revision_code;
        setting.package.last_update_time = request.now;
        setting.package.last_modified_time = request.metadata.last_modified_time;
        setting.package.install_source = request.source;
        setting.package.debuggable = request.code.is(booleans::DEBUGGABLE);
        setting.package.is_sdk_library = request.code.sdk_library_name.is_some();
        setting.package.leaving_shared_user = Some(request.code.is(booleans::LEAVING_SHARED_UID));
        setting.package.split_versions = split_versions(&request.code)?;
        let mut parsed = request.code.clone();
        identity.apply(&mut parsed);
        parsed.signing_details = request
            .signing
            .package_details()
            .map_err(|error| reject(-103, error))?;
        let mut record = Record {
            settings: setting.package,
            parsed,
            signing: request.signing,
            identity,
            origin: ScanOrigin::Data,
        };
        if let Some(at) = at {
            self.settings.packages[at] = record.settings.clone();
        } else {
            let inherited = if self.legacy_permissions.is_some()
                && self.settings.disabled_system_packages.iter().any(|factory|
                    factory.name==name&&factory.app_id==record.settings.app_id) {
                Some(self.disabled_legacy_for_replacement(&name)
                    .map_err(|error|reject(-110,format!("Live disabled setting inheritance: {error}")))?)
            } else {None};
            self.settings.packages.push(record.settings.clone());
            self.legacy_setting_constructor(&name,false,inherited)
                .map_err(|error|reject(-110,format!("Live package setting constructor: {error}")))?;
        }
        let disabled = self
            .settings
            .disabled_system_packages
            .iter()
            .find(|factory| factory.name == name)
            .and_then(|factory| {
                self.disabled_loaded.get(&name).map(|loaded| Record {
                    settings: factory.clone(),
                    parsed: loaded.package.clone(),
                    signing: loaded.collected_signing.clone(),
                    identity: Identity {
                        manifest_name: loaded
                            .package
                            .manifest_package_name
                            .clone()
                            .unwrap_or_else(|| name.clone()),
                        internal_name: name.clone(),
                        real_name: factory.real_name.clone(),
                    },
                    origin: ScanOrigin::SystemDirectory,
                })
            });
        self.libraries
            .check_live_provider(&record)
            .map_err(|error| reject(-5, error.0))?;
        let signing = self
            .reconcile_live_authorized(&record, disabled.as_ref())
            .map_err(|error| reject(-7, format!("Native live reconciliation: {error:?}")))?;
        self.rebind_legacy_setting(&name)
            .map_err(|error|reject(-110,format!("Live package legacy identity: {error}")))?;
        self.libraries
            .refresh_live_provider(&record)
            .map_err(|error| reject(-110, error.0))?;
        record.settings = self
            .settings
            .packages
            .iter()
            .find(|setting| setting.name == name)
            .unwrap()
            .clone();
        self.update_disabled_user_aliases(&name, &setting.users);
        self.scanned_users.insert(name, setting.users.clone());
        Ok(NewPackageOutcome {
            record,
            users: setting.users,
            signing,
        })
    }
}
fn check_update_identity(
    settings: &Settings,
    old: &settings::Package,
    code: &AndroidPackage,
) -> Result<(), Rejection> {
    let old_group = if old.shared_user {
        settings
            .shared_users
            .iter()
            .find(|group| Some(group.app_id) == old.shared_app_id())
            .map(|group| group.name.as_str())
    } else {
        None
    };
    let new_group = super::signing::selected_shared_user(
        old.shared_user,
        code.shared_user_id.as_deref(),
        code.is(booleans::LEAVING_SHARED_UID),
    );
    if old_group != new_group
        || old.leaving_shared_user == Some(true) && !code.is(booleans::LEAVING_SHARED_UID)
    {
        return Err(reject(-24, "Update changes or rejoins shared UID"));
    }
    Ok(())
}
fn authorize_update(
    settings: &Settings,
    old: &settings::Package,
    signing: &SigningDetails,
    rollback: bool,
) -> Result<(), Rejection> {
    let upgrades = &old.key_set_data.upgrade_key_sets;
    let valid = !old.shared_user
        && !upgrades.is_empty()
        && upgrades
            .iter()
            .all(|id| settings.key_sets.key_sets.iter().any(|(set, _)| set == id));
    if valid {
        let keys = signing
            .public_keys
            .as_ref()
            .ok_or_else(|| reject(-7, "Verified signing public keys unavailable"))?;
        if upgrades.iter().any(|id| {
            settings
                .key_sets
                .key_sets
                .iter()
                .find(|(set, _)| set == id)
                .unwrap()
                .1
                .iter()
                .all(|key| {
                    settings
                        .key_sets
                        .public_keys
                        .iter()
                        .find(|(id, _)| id == key)
                        .is_some_and(|(_, value)| {
                            keys.iter().any(|key| key.as_ref() == Some(value))
                        })
                })
        }) {
            return Ok(());
        }
        return Err(reject(-7, "New package not signed by upgrade-keyset keys"));
    }
    if let Some(saved) = old
        .signatures
        .as_ref()
        .filter(|saved| !saved.signatures.is_empty())
    {
        if !History::verified(signing).allows_update_from(&History::saved(saved), rollback) {
            return Err(reject(-7, "Update signing identity is incompatible"));
        }
    }
    Ok(())
}
fn authorize_shared(
    settings: &Settings,
    name: &str,
    group: Option<&str>,
    signing: &SigningDetails,
    update: bool,
) -> Result<(), Rejection> {
    let Some(group) = group.and_then(|name| {
        settings
            .shared_users
            .iter()
            .find(|group| group.name == name)
    }) else {
        return Ok(());
    };
    let Some(saved) = group
        .signatures
        .as_ref()
        .filter(|saved| !saved.signatures.is_empty())
    else {
        return Ok(());
    };
    let members: Vec<_> = settings
        .packages
        .iter()
        .filter(|package| package.name != name && package.shared_app_id() == Some(group.app_id))
        .filter_map(|package| package.signatures.as_ref())
        .map(History::saved)
        .collect();
    if !History::verified(signing).can_join_shared_user(
        &History::saved(saved),
        if update {
            JoinType::Update
        } else {
            JoinType::Install
        },
        &members,
    ) {
        return Err(reject(-8, "Shared UID signing identity is incompatible"));
    }
    Ok(())
}
fn split_versions(code: &AndroidPackage) -> Result<Vec<(String, i32)>, Rejection> {
    let names = code.split_names.as_deref().unwrap_or_default();
    let revisions = code.split_revision_codes.as_deref().unwrap_or_default();
    if names.len() != revisions.len() {
        return Err(reject(-2, "Split names and revision codes differ"));
    }
    names
        .iter()
        .zip(revisions)
        .map(|(name, revision)| {
            Ok((
                name.clone().ok_or_else(|| reject(-2, "Null split name"))?,
                *revision,
            ))
        })
        .collect()
}
fn check_downgrade(old: &settings::Package, code: &AndroidPackage) -> Result<(), Rejection> {
    let version = ((code.version_code_major as i64) << 32) | (code.version_code as u32 as i64);
    if version < old.version_code
        || version == old.version_code && code.base_revision_code < old.base_revision_code
    {
        return Err(reject(-25, "Version or base revision downgrade"));
    }
    if version == old.version_code {
        for (name, revision) in split_versions(code)? {
            if old
                .split_versions
                .iter()
                .find(|(old, _)| old == &name)
                .is_some_and(|(_, before)| revision < *before)
            {
                return Err(reject(-25, format!("Split {name} revision downgrade")));
            }
        }
    }
    Ok(())
}

fn package_digest(
    files: &crate::package::write::Files,
    code: &AndroidPackage,
) -> Result<Vec<u8>, Rejection> {
    use sha2::{Digest, Sha512};
    use std::io::Read;
    let mut digest = Sha512::new();
    let mut paths = Vec::new();
    paths.push(
        code.base_apk_path
            .as_deref()
            .ok_or_else(|| reject(-2, "Missing install base APK path"))?,
    );
    for path in code.split_code_paths.iter().flatten() {
        paths.push(
            path.as_deref()
                .ok_or_else(|| reject(-2, "Null install split APK path"))?,
        );
    }
    for path in paths {
        let host = files(path).ok_or_else(|| reject(-2, "Restrict-update VFS file unavailable"))?;
        let mut file = std::fs::File::open(host).map_err(|error| reject(-2, error.to_string()))?;
        let mut buffer = [0u8; 16384];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| reject(-2, error.to_string()))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(digest.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{pkg::AndroidPackage, sign::SigningDetails, system_config::SystemConfig};
    fn request(name: &str, version: i64) -> Request {
        Request {
            code: AndroidPackage {
                package_name: name.into(),
                path: Some(format!("/data/app/{name}")),
                base_apk_path: Some(format!("/data/app/{name}/base.apk")),
                version_code: version as i32,
                version_code_major: (version >> 32) as i32,
                target_sdk_version: 35,
                base_revision_code: 4,
                ..Default::default()
            },
            signing: SigningDetails {
                unknown: false,
                signatures: vec![vec![1]],
                scheme_version: 2,
                ..SigningDetails::unknown()
            },
            metadata: SettingMetadata {
                code_path: format!("/data/app/{name}"),
                legacy_native_library_path: None,
                primary_cpu_abi: None,
                secondary_cpu_abi: None,
                version_code: version,
                flags: 0,
                private_flags: 0,
                last_modified_time: 10,
                uses_sdk_libraries: vec![],
                uses_static_libraries: vec![],
                mime_groups: vec![],
                domain_set_id: [1; 16],
                target_sdk_version: 35,
                restrict_update_hash: None,
            },
            install_flags: 2,
            required_installed_version: -1,
            rollback: false,
            source: Default::default(),
            install_user: 0,
            now: 20,
        }
    }
    #[test]
    fn new_live_settings_seed_constructor_permission_owners_before_original_prepare() {
        use crate::package::owner::legacy_permissions::Migration;
        let mut owner=SigningScan::new(&SystemConfig::default(),&Settings::default(),36).unwrap();
        let groups=owner.identities.shared_users.keys().map(|name|(name.clone(),Migration::default())).collect();
        owner.capture_legacy_permissions(&[0],Default::default(),groups).unwrap();
        owner.capture_install_permissions_fixed(Default::default()).unwrap();
        let users=[super::super::User{id:0,pre_created:false,adb_install_disallowed:false}];
        let files:crate::package::write::Files=Box::new(|_|panic!("constructor seeds do not read APK bytes"));
        let admission=owner.prepare_live_installs(vec![request("org.example.new",1)],&users,false,&files).unwrap();
        let state=admission.owner.legacy_permissions("org.example.new",false).unwrap().unwrap();
        assert_eq!(state.app_id(),admission.owner.settings.packages[0].app_id);
        assert_eq!(state.users().len(),1);assert_eq!(state.users()[0].id,0);assert!(!state.users()[0].missing);assert!(state.users()[0].permissions.is_empty());
        assert_eq!(admission.owner.install_permissions_fixed("org.example.new",false).unwrap(),Some(false));
        assert!(owner.settings.packages.is_empty());
        let mut shared=request("org.example.shared",1);shared.code.shared_user_id=Some("org.example.group".into());
        let admission=owner.prepare_live_installs(vec![shared],&users,false,&files).unwrap();
        let group=admission.owner.shared_legacy_permissions("org.example.group").unwrap().unwrap();assert!(group.users()[0].permissions.is_empty());

    }
    #[test]
    fn live_batch_reserves_uids_and_preserves_old_owner_on_late_rejection() {
        let owner = SigningScan::new(&SystemConfig::default(), &Settings::default(), 36).unwrap();
        let users = [
            super::super::User {
                id: 0,
                pre_created: false,
                adb_install_disallowed: false,
            },
            super::super::User {
                id: 10,
                pre_created: false,
                adb_install_disallowed: false,
            },
        ];
        let files: crate::package::write::Files =
            Box::new(|_| panic!("No restricted update reads needed"));
        let admission = owner
            .prepare_live_installs(
                vec![request("org.example.a", 7), request("org.example.b", 8)],
                &users,
                false,
                &files,
            )
            .unwrap();
        assert!(owner.settings.packages.is_empty());
        assert_eq!(admission.owner.settings.packages.len(), 2);
        assert_ne!(
            admission.owner.settings.packages[0].app_id,
            admission.owner.settings.packages[1].app_id
        );
        assert!(admission.candidates[0].users[&0].installed);
        assert!(!admission.candidates[0].users[&10].installed);
        assert!(admission.candidates[0].users[&0].stopped);
        assert_eq!(admission.candidates[0].users[&0].first_install_time, 20);
        assert!(!admission.owner.capture_ready());
        let mut invalid = request("org.example.bad", 2);
        invalid.required_installed_version = 1;
        assert!(
            owner
                .prepare_live_installs(
                    vec![request("org.example.a", 7), invalid],
                    &users,
                    false,
                    &files
                )
                .is_err()
        );
        assert!(owner.settings.packages.is_empty());
    }
    #[test]
    fn update_uid_rejoin_revision_and_upgrade_keysets_use_live_gates() {
        let mut settings = Settings::default();
        let old = settings::Package {
            name: "org.example.a".into(),
            version_code: 7,
            base_revision_code: 3,
            split_versions: vec![("feature".into(), 5)],
            leaving_shared_user: Some(true),
            ..Default::default()
        };
        let mut code = request("org.example.a", 7).code;
        code.base_revision_code = 2;
        assert_eq!(check_downgrade(&old, &code).unwrap_err().status, -25);
        code.base_revision_code = 3;
        code.split_names = Some(vec![Some("feature".into())]);
        code.split_revision_codes = Some(vec![4]);
        assert_eq!(check_downgrade(&old, &code).unwrap_err().status, -25);
        code.split_revision_codes = Some(vec![5]);
        assert!(check_downgrade(&old, &code).is_ok());
        assert_eq!(
            check_update_identity(&settings, &old, &code)
                .unwrap_err()
                .status,
            -24
        );
        let mut old = old;
        old.key_set_data.upgrade_key_sets = vec![9];
        settings.key_sets.key_sets = vec![(9, vec![11])];
        settings.key_sets.public_keys = vec![(11, vec![42])];
        let mut signing = request("org.example.a", 7).signing;
        signing.public_keys = Some(vec![Some(vec![42]), Some(vec![43])]);
        assert!(authorize_update(&settings, &old, &signing, false).is_ok());
        signing.public_keys = Some(vec![Some(vec![43])]);
        assert_eq!(
            authorize_update(&settings, &old, &signing, false)
                .unwrap_err()
                .status,
            -7
        );
    }
}
