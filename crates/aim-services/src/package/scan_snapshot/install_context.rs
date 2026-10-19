//! Candidate query owners for native installation permission transactions.
//! Per-package identities are rebuilt from the candidate; globals retain their owners.
use super::{
    Snapshot,
    query_state::{Capture, Context, PackageInputs, UserInputs},
};
use crate::package::{
    bootstrap::Bridge,
    domain_verification::owner as domain,
    owner::{app_ids::Owner as UidOwner, legacy_permissions},
    restrictions, settings,
    system_config::SystemConfig,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub type Current = Arc<dyn Fn() -> Result<Arc<Capture>, String> + Send + Sync>;
/// Permit a retained read across actual usage publication only. All settings,
/// code, UID, user, signing, library and permission ownership remain compared.
pub(crate) fn same_install_identity(before: &Snapshot, after: &Snapshot) -> Result<bool, String> {
    if before.owner() == after.owner() && before.usage() == after.usage() {
        return Ok(true);
    }
    if before.usage().names().ne(after.usage().names()) {
        return Ok(false);
    }
    let mut normalized = before.owner().clone();
    normalized.update_replica_usage(before.usage(), after.usage())?;
    Ok(&normalized == after.owner())
}
/// Installation owns the changed code/settings and every package sharing its
/// old/new appId. Other packages may receive native user-state publications.
pub(crate) fn install_targets(
    base: &Snapshot,
    candidate: &crate::package::scan::SigningScan,
) -> BTreeSet<String> {
    let old = base.owner();
    let mut names = BTreeSet::new();
    for setting in old
        .settings
        .packages
        .iter()
        .chain(candidate.settings.packages.iter())
    {
        let a = old
            .settings
            .packages
            .iter()
            .find(|p| p.name == setting.name);
        let b = candidate
            .settings
            .packages
            .iter()
            .find(|p| p.name == setting.name);
        if a != b
            || old.loaded_packages().get(&setting.name)
                != candidate.loaded_packages().get(&setting.name)
        {
            names.insert(setting.name.clone());
        }
    }
    names
}
/// A three-way merge of unrelated user state. Code, settings, UID ownership,
/// permissions and library inputs remain strict; installed bits affect the
/// dependency graph and require a new admission rather than this rebase.
pub(crate) fn rebase_install_users(
    base: &Snapshot,
    current: &Snapshot,
    candidate: &mut crate::package::scan::SigningScan,
    targets: &BTreeSet<String>,
) -> Result<bool, String> {
    Ok(install_user_rebase_conflict(base, current, candidate, targets)?.is_none())
}
pub(crate) fn install_user_rebase_conflict(
    base: &Snapshot,
    current: &Snapshot,
    candidate: &mut crate::package::scan::SigningScan,
    targets: &BTreeSet<String>,
) -> Result<Option<String>, String> {
    let protected = base
        .owner()
        .settings
        .packages
        .iter()
        .chain(candidate.settings.packages.iter())
        .filter(|p| targets.contains(&p.name))
        .map(|p| p.uid_owner_id())
        .collect::<BTreeSet<_>>();
    let mut normalized = base.owner().clone();
    let mut latest = current.owner().clone();
    let mut changes = Vec::new();
    for setting in &base.owner().settings.packages {
        if targets.contains(&setting.name) {
            // An update keeps the package's overlay paths: OverlayManager's
            // update after the previous PACKAGE_ADDED carries into the
            // candidate. Any other concurrent target change stays a conflict.
            let (Some(before), Some(after)) = (
                base.owner().scanned_user_states(&setting.name),
                current.owner().scanned_user_states(&setting.name),
            ) else {
                continue;
            };
            for (user, state) in before {
                let Some(now) = after.get(user).filter(|now| *now != state) else {
                    continue;
                };
                let mut overlays = state.clone();
                overlays
                    .runtime
                    .set_overlay_paths(now.runtime.overlays().cloned());
                if overlays != *now {
                    continue;
                }
                let Some(mut merged) = candidate
                    .scanned_user_states(&setting.name)
                    .and_then(|users| users.get(user))
                    .cloned()
                else {
                    continue;
                };
                merged
                    .runtime
                    .set_overlay_paths(now.runtime.overlays().cloned());
                candidate.set_user_state(&setting.name, *user, merged)?;
                normalized.set_user_state(&setting.name, *user, now.clone())?;
            }
            continue;
        }
        if protected.contains(&setting.uid_owner_id()) {
            continue;
        }
        let Some(before) = base.owner().scanned_user_states(&setting.name) else {
            continue;
        };
        let Some(after) = current.owner().scanned_user_states(&setting.name) else {
            return Ok(Some(format!(
                "package {} missing current user owner",
                setting.name
            )));
        };
        if before.keys().ne(after.keys()) {
            return Ok(Some(format!(
                "package {} user inventory {:?} -> {:?}",
                setting.name,
                before.keys().collect::<Vec<_>>(),
                after.keys().collect::<Vec<_>>()
            )));
        }
        for (user, state) in before {
            let now = &after[user];
            if state == now {
                continue;
            }
            if state.installed != now.installed {
                return Ok(Some(format!(
                    "package {} user {user} installed {} -> {}",
                    setting.name, state.installed, now.installed
                )));
            }
            let value = candidate
                .scanned_user_states(&setting.name)
                .and_then(|users| users.get(user));
            if value != Some(state) && value != Some(now) {
                return Ok(Some(format!(
                    "package {} user {user} competing candidate state: before={state:?}, current={now:?}, candidate={value:?}; targets={targets:?}, protected appIds={protected:?}",
                    setting.name
                )));
            }
            normalized.set_user_state(&setting.name, *user, now.clone())?;
            changes.push((setting.name.clone(), *user, now.clone()));
        }
    }
    let before_domains = &base.owner().settings.domain_verification.active;
    let current_domains = &current.owner().settings.domain_verification.active;
    let before_names = before_domains
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    let current_names = current_domains
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    let candidate_names = candidate
        .settings
        .domain_verification
        .active
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    if before_names.iter().copied().collect::<BTreeSet<_>>().len() != before_names.len()
        || current_names.iter().copied().collect::<BTreeSet<_>>().len() != current_names.len()
        || candidate_names
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != candidate_names.len()
    {
        return Ok(Some(
            "domain active inventory contains duplicate package names".into(),
        ));
    }
    if before_names != current_names {
        let retained = current_names
            .iter()
            .copied()
            .filter(|name| before_names.contains(name))
            .collect::<Vec<_>>();
        let removed = before_names
            .iter()
            .copied()
            .filter(|name| !current_names.contains(name))
            .collect::<Vec<_>>();
        let added = current_names
            .iter()
            .copied()
            .filter(|name| !before_names.contains(name))
            .collect::<Vec<_>>();
        if !removed.is_empty() || retained != before_names {
            return Ok(Some(format!(
                "domain active inventory/order changed: added={added:?}, removed={removed:?}, retained_order_changed={}",
                retained != before_names
            )));
        }
        for name in &added {
            let Some(after) = current_domains.iter().find(|package| package.name == *name) else {
                unreachable!()
            };
            let values = candidate
                .settings
                .domain_verification
                .active
                .iter()
                .filter(|package| package.name == *name)
                .collect::<Vec<_>>();
            let setting = current
                .owner()
                .settings
                .packages
                .iter()
                .chain(candidate.settings.packages.iter())
                .find(|setting| setting.name == *name);
            let protected_addition = targets.contains(*name)
                || setting.is_some_and(|setting| protected.contains(&setting.uid_owner_id()));
            if !protected_addition || values.as_slice() != [after] {
                return Ok(Some(format!(
                    "domain active inventory added competing package {name}: target_or_shared={protected_addition}, candidate_matches={}",
                    values.as_slice() == [after]
                )));
            }
            let users = current
                .owner()
                .scanned_user_states(name)
                .or_else(|| candidate.scanned_user_states(name))
                .ok_or("added active domain user owner unavailable")?;
            let selected = after
                .users
                .iter()
                .map(|user| user.id)
                .collect::<BTreeSet<_>>();
            if selected.len() != after.users.len()
                || selected.iter().any(|user| !users.contains_key(user))
            {
                return Ok(Some(format!(
                    "domain selection references unknown user: {name}"
                )));
            }
        }
        normalized.settings.domain_verification.active = current_domains.clone();
    }
    let mut domain_changes = Vec::new();
    for before in before_domains {
        let Some(after) = current_domains
            .iter()
            .find(|package| package.name == before.name)
        else {
            continue;
        };
        if before == after {
            continue;
        }
        let setting = base
            .owner()
            .settings
            .packages
            .iter()
            .find(|setting| setting.name == before.name)
            .ok_or("changed active domain package has no settings owner")?;
        if targets.contains(&before.name) || protected.contains(&setting.uid_owner_id()) {
            continue;
        }
        // An unrelated domain owner may publish its identity/manifest capture
        // independently. Merge the whole row only when the candidate retains
        // the base or current row; the final comparison still protects code,
        // settings, signing and UID ownership.
        let users = base
            .owner()
            .scanned_user_states(&before.name)
            .ok_or("changed active domain user owner unavailable")?;
        let selected = after
            .users
            .iter()
            .map(|user| user.id)
            .collect::<BTreeSet<_>>();
        if selected.len() != after.users.len()
            || selected.iter().any(|user| !users.contains_key(user))
        {
            return Ok(Some(format!(
                "domain selection references unknown user: {}",
                before.name
            )));
        }
        let values = candidate
            .settings
            .domain_verification
            .active
            .iter()
            .filter(|package| package.name == before.name)
            .collect::<Vec<_>>();
        if values.len() != 1 || values[0] != before && values[0] != after {
            return Ok(Some(format!(
                "competing candidate domain state: {}",
                before.name
            )));
        }
        let value = normalized
            .settings
            .domain_verification
            .active
            .iter_mut()
            .find(|package| package.name == after.name)
            .ok_or("validated normalized domain owner missing")?;
        *value = after.clone();
        domain_changes.push(after.clone());
    }
    if base.usage().names().ne(current.usage().names()) {
        return Ok(Some("usage package inventory differs".into()));
    }
    if base.usage() != current.usage() && normalized.has_replica_runtime() {
        normalized.update_replica_usage(base.usage(), current.usage())?;
    }
    // Resolved dependency records also capture user state; refresh only after
    // validating identical code/signatures/user inventory/installed bits.
    normalized.refresh_library_user_inputs()?;
    latest.refresh_library_user_inputs()?;
    if normalized != latest {
        let before = Snapshot {
            version: base.version(),
            owner: normalized,
            usage: current.usage().clone(),
            replica_validated: false,
            metadata_revision: base.metadata_revision(),
            lineage: base.lineage.clone(),
            libraries: Default::default(),
            records: Default::default(),
        };
        return Ok(Some(format!(
            "unmerged normalized owner: {}; targets={targets:?}, protected appIds={protected:?}",
            install_identity_delta(&before, current)
        )));
    }
    for (name, user, state) in changes {
        candidate.set_user_state(&name, user, state)?;
    }
    for domain in domain_changes {
        let value = candidate
            .settings
            .domain_verification
            .active
            .iter_mut()
            .find(|package| package.name == domain.name)
            .ok_or("validated candidate domain owner missing")?;
        *value = domain;
    }
    candidate.refresh_library_user_inputs()?;
    Ok(None)
}

/// Rebase a newly-created user's candidate over unrelated live package state.
/// Usage may advance independently while the user owner obtains live permission
/// inputs; mirror that projection into the candidate after the strict conflict
/// checks have accepted the newer snapshot.
pub(crate) fn rebase_user_transition(
    base: &Snapshot,
    current: &Snapshot,
    candidate: &mut crate::package::scan::SigningScan,
) -> Result<Option<String>, String> {
    let conflict = install_user_rebase_conflict(base, current, candidate, &BTreeSet::new())?;
    if conflict.is_some() {
        return Ok(conflict);
    }
    if base.usage() != current.usage() && candidate.has_replica_runtime() {
        candidate.update_replica_usage(base.usage(), current.usage())?;
    }
    Ok(None)
}
fn entries_delta<T: PartialEq, K: Ord + std::fmt::Debug>(
    field: &str,
    before: &[T],
    after: &[T],
    key: impl Fn(&T) -> K,
) -> String {
    let before_keys = before.iter().map(&key).collect::<Vec<_>>();
    let after_keys = after.iter().map(&key).collect::<Vec<_>>();
    let keys = before
        .iter()
        .chain(after)
        .map(&key)
        .collect::<BTreeSet<_>>();
    let changed = keys
        .iter()
        .filter(|name| {
            before.iter().find(|value| key(value) == **name)
                != after.iter().find(|value| key(value) == **name)
        })
        .collect::<Vec<_>>();
    format!(
        "settings.{field}: count {} -> {}, changed_keys={} {:?}, key_order_changed={}",
        before.len(),
        after.len(),
        changed.len(),
        changed.iter().take(16).collect::<Vec<_>>(),
        before_keys != after_keys
    )
}
fn settings_delta(before: &settings::Settings, after: &settings::Settings) -> Vec<String> {
    let mut fields = Vec::new();
    macro_rules! entries {
        ($field:ident,$key:expr) => {
            if before.$field != after.$field {
                fields.push(entries_delta(
                    stringify!($field),
                    &before.$field,
                    &after.$field,
                    $key,
                ));
            }
        };
    }
    entries!(versions, |value: &settings::Version| value
        .volume_uuid
        .clone());
    if before.verifier != after.verifier {
        fields.push(format!(
            "settings.verifier: changed, present {} -> {}",
            before.verifier.is_some(),
            after.verifier.is_some()
        ));
    }
    entries!(permission_trees, |value: &settings::Permission| value
        .name
        .clone());
    entries!(permissions, |value: &settings::Permission| value
        .name
        .clone());
    entries!(packages, |value: &settings::Package| value.name.clone());
    entries!(disabled_system_packages, |value: &settings::Package| value
        .name
        .clone());
    entries!(shared_users, |value: &settings::SharedUser| value
        .name
        .clone());
    entries!(renamed_packages, |value: &(String, String)| value.0.clone());
    let a = &before.key_sets;
    let b = &after.key_sets;
    if a.versioned != b.versioned {
        fields.push("settings.key_sets.versioned: changed".into());
    }
    if a.public_keys != b.public_keys {
        fields.push(entries_delta(
            "key_sets.public_keys",
            &a.public_keys,
            &b.public_keys,
            |value| value.0,
        ));
    }
    if a.key_sets != b.key_sets {
        fields.push(entries_delta(
            "key_sets.key_sets",
            &a.key_sets,
            &b.key_sets,
            |value| value.0,
        ));
    }
    if a.last_issued_key_id != b.last_issued_key_id {
        fields.push("settings.key_sets.last_issued_key_id: changed".into());
    }
    if a.last_issued_key_set_id != b.last_issued_key_set_id {
        fields.push("settings.key_sets.last_issued_key_set_id: changed".into());
    }
    if a.reference_counts != b.reference_counts {
        let old = a
            .reference_counts
            .as_ref()
            .map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (*key, *value))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let new = b
            .reference_counts
            .as_ref()
            .map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (*key, *value))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        fields.push(format!(
            "{}; present {} -> {}",
            entries_delta("key_sets.reference_counts", &old, &new, |value| value.0),
            a.reference_counts.is_some(),
            b.reference_counts.is_some()
        ));
    }
    let a = &before.domain_verification;
    let b = &after.domain_verification;
    if a.active != b.active {
        fields.push(entries_delta(
            "domain_verification.active",
            &a.active,
            &b.active,
            |value| value.name.clone(),
        ));
    }
    if a.restored != b.restored {
        fields.push(entries_delta(
            "domain_verification.restored",
            &a.restored,
            &b.restored,
            |value| value.name.clone(),
        ));
    }
    if a.legacy != b.legacy {
        fields.push(entries_delta(
            "domain_verification.legacy",
            &a.legacy,
            &b.legacy,
            |value| value.0.clone(),
        ));
    }
    if before.legacy_domain_info != after.legacy_domain_info {
        let old = before
            .legacy_domain_info
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect::<Vec<_>>();
        let new = after
            .legacy_domain_info
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect::<Vec<_>>();
        fields.push(entries_delta("legacy_domain_info", &old, &new, |value| {
            value.0.clone()
        }));
    }
    fields
}
pub(crate) fn install_identity_delta(before: &Snapshot, after: &Snapshot) -> String {
    let old = before.owner();
    let new = after.owner();
    let mut fields = Vec::new();
    if old.settings != new.settings {
        fields.extend(settings_delta(&old.settings, &new.settings));
    }
    if old.identities != new.identities {
        fields.push("UID/shared identity ownership".into());
    }
    if old.loaded_packages() != new.loaded_packages() {
        fields.push("active parsed code".into());
    }
    if old.disabled_loaded_packages() != new.disabled_loaded_packages() {
        fields.push("factory parsed code".into());
    }
    for setting in &old.settings.packages {
        let a = old.scanned_user_states(&setting.name);
        let b = new.scanned_user_states(&setting.name);
        match (a, b) {
            (Some(a), Some(b)) => {
                for (user, before) in a {
                    match b.get(user) {
                        Some(after) if before != after => {
                            if before.runtime.overlays() != after.runtime.overlays()
                                || before.runtime.libraries() != after.runtime.libraries()
                            {
                                fields.push(format!("package {} user {user} overlay paths {:?} -> {:?}; library overlays {:?} -> {:?}",setting.name,before.runtime.overlays(),after.runtime.overlays(),before.runtime.libraries(),after.runtime.libraries()));
                            } else {
                                fields.push(format!("package {} user {user} state ownership: before={before:?}, current={after:?}",setting.name));
                            }
                        }
                        None => {
                            fields.push(format!("package {} removed user {user}", setting.name))
                        }
                        _ => {}
                    }
                }
                for user in b.keys().filter(|user| !a.contains_key(user)) {
                    fields.push(format!("package {} added user {user}", setting.name));
                }
            }
            _ if a != b => fields.push(format!("package {} user inventory", setting.name)),
            _ => {}
        }
    }
    if old.libraries != new.libraries {
        fields.push("library registry".into());
    }
    if old.update_ownership != new.update_ownership {
        fields.push("update ownership".into());
    }
    if old.installers != new.installers {
        fields.push("install source ownership".into());
    }
    if fields.is_empty() {
        fields.push("scoped user/seInfo/permission/runtime ownership".into());
    }
    format!(
        "version {} -> {}: {}",
        before.version(),
        after.version(),
        fields.join("; ")
    )
}
impl super::Store {
    /// Rebase usage and persist while the publication gate is held. The native
    /// candidate computation calls no Binder/guest owner, closing the previous
    /// capture-to-CAS window without weakening code/settings/UID ownership.
    #[track_caller]
    pub(crate) fn publish_install_after(
        &self,
        base: &Arc<Snapshot>,
        mut owner: crate::package::scan::SigningScan,
        persist: impl FnOnce(&Arc<Snapshot>) -> Result<(), crate::package::owner::WriteError>,
    ) -> Result<Arc<Snapshot>, super::CommitError> {
        let mut current = self.current.lock().unwrap();
        let targets = install_targets(base, &owner);
        if let Some(reason) = install_user_rebase_conflict(base, &current, &mut owner, &targets)
            .map_err(|error| super::CommitError::Snapshot(super::Error::Invalid(error)))?
        {
            return Err(super::CommitError::Snapshot(super::Error::Invalid(
                format!("install publication owner changed: {reason}"),
            )));
        }
        let usage = current.usage().for_install(
            owner
                .settings
                .packages
                .iter()
                .map(|package| package.name.as_str()),
        );
        crate::package::installer::permission_prepare::complete_candidate_runtime(
            &mut owner, &current, &usage,
        )
        .map_err(|error| {
            super::CommitError::Snapshot(super::Error::Invalid(format!(
                "install publication runtime: {error}"
            )))
        })?;
        let version = current
            .version()
            .checked_add(1)
            .filter(|version| *version <= i64::MAX as u64)
            .ok_or(super::CommitError::Snapshot(super::Error::VersionExhausted))?;
        super::validate(&owner, &usage).map_err(super::CommitError::Snapshot)?;
        let mut next = Snapshot {
            version,
            owner,
            usage,
            replica_validated: self.replica,
            metadata_revision: version,
            lineage: current.lineage.clone(),
            libraries: Default::default(),
            records: Default::default(),
        };
        if self.replica {
            super::validate_replica(&next).map_err(super::CommitError::Snapshot)?;
        }
        next.metadata_revision = super::derive_metadata_revision(&current, &next)
            .map_err(|error| super::CommitError::Snapshot(error))?;
        let next = Arc::new(next);
        super::diagnose_metadata(
            4,
            Some(&current),
            next.owner(),
            next.usage(),
            version,
            next.metadata_revision(),
        );
        match persist(&next) {
            Ok(()) => {
                *current = next.clone();
                Ok(next)
            }
            Err(error) if error.committed => {
                *current = next.clone();
                Err(super::CommitError::Disk {
                    snapshot: Some(next),
                    error,
                })
            }
            Err(error) => Err(super::CommitError::Disk {
                snapshot: None,
                error,
            }),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    BeforePermissionLifecycle,
    AfterPermissionLifecycle,
}
/// Invoke after the original permission lifecycle completed and installed its
/// live states in this candidate. The phase is an explicit pipeline boundary.
pub struct Builder {
    current: Current,
    bridge: Arc<Bridge>,
    config: Arc<SystemConfig>,
}
impl Builder {
    pub fn new(current: Current, bridge: Arc<Bridge>, config: Arc<SystemConfig>) -> Arc<Self> {
        Arc::new(Self {
            current,
            bridge,
            config,
        })
    }
    pub fn permission_source(
        self: &Arc<Self>,
    ) -> crate::package::installer::permission_capture::ContextSource {
        let builder = self.clone();
        Arc::new(move |snapshot| builder.build(snapshot, Phase::BeforePermissionLifecycle))
    }
    pub fn publication_source(
        self: &Arc<Self>,
    ) -> crate::package::installer::permission_capture::ContextSource {
        let builder = self.clone();
        Arc::new(move |snapshot| builder.build(snapshot, Phase::AfterPermissionLifecycle))
    }
    pub fn build(&self, snapshot: &Arc<Snapshot>, phase: Phase) -> Result<Context, String> {
        let users = (self.current)()?
            .context()
            .users
            .keys()
            .copied()
            .collect::<Vec<_>>();
        self.bridge
            .read_ahead(live_calls(snapshot.owner(), &users), || {
                self.build_reading(snapshot, phase)
            })
    }
    fn build_reading(&self, snapshot: &Arc<Snapshot>, phase: Phase) -> Result<Context, String> {
        let current = (self.current)()?;
        let scan = snapshot.owner();
        // Registration is a completed native owner, never reconstructed in map order.
        scan.package_registry()?;
        let mut context = (**current.context()).clone();
        context.scan_version = snapshot.version();
        context.packages.clear();
        context.retained_packages.clear();
        context.native_domains = None;
        let users = context.users.keys().copied().collect::<Vec<_>>();
        if users.is_empty() {
            return Err("install context has no resolved user owner".into());
        }
        let scan_users = context
            .scan_users
            .users
            .as_ref()
            .ok_or("install context scan users unavailable")?;
        if scan_users
            .iter()
            .map(|user| user.id)
            .collect::<BTreeSet<_>>()
            != users.iter().copied().collect()
        {
            return Err("install context UM and scan-user inventories differ".into());
        }
        let mut permissions = BTreeMap::new();
        for (settings, factory) in [
            (&scan.settings.packages, false),
            (&scan.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                let stored = if factory {
                    scan.disabled_user_states(&setting.name)
                } else {
                    scan.scanned_user_states(&setting.name)
                }
                .ok_or("install context native package user owner unavailable")?;
                let code = if factory {
                    scan.disabled_loaded_packages()
                } else {
                    scan.loaded_packages()
                }
                .get(&setting.name);
                let parsed = code.map(|code| code.runtime_package());
                let inputs = self.package(
                    setting,
                    super::boot_context::permission_uid_owner(setting, parsed)?,
                    stored,
                    parsed,
                    &users,
                    &mut permissions,
                )?;
                context
                    .packages
                    .insert((setting.name.clone(), factory), inputs);
            }
        }
        for (id, slot) in scan.identities.ids.owners() {
            if let UidOwner::DetachedPackage(name) = slot {
                let old = scan
                    .identities
                    .ids
                    .detached_setting(id)
                    .ok_or("install context detached UID owner unavailable")?;
                let inputs = self.package(
                    &old.package,
                    Some(id),
                    &old.users,
                    None,
                    &users,
                    &mut permissions,
                )?;
                context.retained_packages.insert((id, name.clone()), inputs);
            }
        }
        for group in scan.identities.shared_users.values() {
            for (name, old) in group.retained_settings() {
                let inputs = self.package(
                    &old.package,
                    Some(group.app_id),
                    &old.users,
                    None,
                    &users,
                    &mut permissions,
                )?;
                if context
                    .retained_packages
                    .insert((group.app_id, name.into()), inputs)
                    .is_some()
                {
                    return Err("install context retained permission owner duplicated".into());
                }
            }
        }
        if phase == Phase::AfterPermissionLifecycle {
            // Migration state is not a substitute for a completed live permission receipt.
            // Receipt equality applies only to its genuine affected UIDs.
            // Every UID still has fresh original live grants/GIDs above.
            let receipt = scan.installed_permission_receipt_uids()?;
            for setting in &scan.settings.packages {
                let parsed = scan
                    .loaded_packages()
                    .get(&setting.name)
                    .map(|code| code.runtime_package());
                let Some(permission_id) =
                    super::boot_context::permission_uid_owner(setting, parsed)?
                else {
                    continue;
                };
                if !receipt.contains(&permission_id) {
                    continue;
                }
                let modern = scan
                    .installed_permission_state(permission_id)?
                    .ok_or("post-install modern permission receipt unavailable")?;
                let live = permissions
                    .get(&permission_id)
                    .ok_or("post-install live permission UID unavailable")?;
                if modern.app_id() != permission_id || modern.bytes() != live.bytes() {
                    return Err(format!(
                        "post-install permission lifecycle/live owner differs: {}",
                        setting.name
                    ));
                }
            }
        }
        let existing = current
            .domains()
            .ok_or("install context native domain owner unavailable")?;
        let mut domains = existing.owner().clone();
        for name in current.state().packages.keys() {
            if !scan
                .settings
                .packages
                .iter()
                .any(|setting| &setting.name == name)
            {
                domains.clear_package(name);
            }
        }
        let mut domain_active = BTreeSet::new();
        for setting in &scan.settings.packages {
            if scan.domain_code_owner(setting)?.is_some() {
                domain_active.insert(setting.name.as_str());
            }
        }
        let persisted = domains.persisted();
        domains.rebase_persisted_projection(&persisted, persisted.clone(), &domain_active)?;
        let mut policies = BTreeMap::new();
        let mut changes = Vec::new();
        for setting in &scan.settings.packages {
            let Some(code) = scan.domain_code_owner(setting)? else {
                continue;
            };
            let code = &code.package;
            let policy = self
                .bridge
                .domain_verification_restricted(&code.package_name, code.target_sdk_version)
                .map_err(|error| format!("install domain compatibility: {error:?}"))?;
            policies.insert(setting.name.clone(), policy);
            let id = setting
                .domain_set_id
                .as_deref()
                .ok_or("install domain UUID owner unavailable")?;
            let old = current.scan().owner().loaded_packages().get(&setting.name);
            let unchanged = old.is_some_and(|old| old.package == *code)
                && domains
                    .package(&setting.name)
                    .is_some_and(|old| old.id.eq_ignore_ascii_case(id));
            if unchanged {
                continue;
            }
            let signatures = setting
                .signatures
                .as_ref()
                .ok_or("install domain signing owner unavailable")?;
            let input = domain::Input {
                id,
                name: &setting.name,
                code: Some(code),
                signatures: &signatures.signatures,
                system: setting.flags & settings::FLAG_SYSTEM != 0,
                restrict_domains: policy,
                pre_verified: None,
            };
            let change = match domains.package(&setting.name).map(|old| old.id.clone()) {
                Some(old_id) => {
                    domains.migrate(&old_id, old.map(|code| &code.package), input, &self.config)?
                }
                None => domains.add(input, &self.config)?,
            };
            changes.push((setting.name.clone(), change));
        }
        context = context.attach_boot_domains(
            scan,
            domain::Boot {
                owner: domains,
                changes,
            },
            &self.config,
            &policies,
        )?;
        // Factory and detached settings have no new domain lifecycle event. Their
        // old external domain projections are usable only with the same identity.
        for (key, inputs) in &mut context.packages {
            if !key.1 {
                continue;
            }
            let code = scan
                .disabled_loaded_packages()
                .get(&key.0)
                .ok_or("factory domain parsed-code owner unavailable")?;
            let restrict = self
                .bridge
                .domain_verification_restricted(
                    &code.package.package_name,
                    code.package.target_sdk_version,
                )
                .map_err(|error| format!("factory domain compatibility: {error:?}"))?;
            let values = existing
                .owner()
                .queries(
                    &code.package,
                    restrict,
                    &self.config,
                    inputs.users.keys().copied(),
                )?
                .ok_or("factory domain owner unavailable")?;
            inputs.domain_verification = values.verification;
            inputs.uri_relative_filter_groups = values.uri_relative_filter_groups;
            for (user, selection) in values.users {
                inputs
                    .users
                    .get_mut(&user)
                    .ok_or("factory domain user disappeared")?
                    .domain_selection = Some(selection);
            }
        }
        for (key, inputs) in &mut context.retained_packages {
            let previous = current
                .context()
                .retained_packages
                .get(key)
                .ok_or("retained domain projection owner unavailable")?;
            if inputs.app_id != previous.app_id
                || inputs.path != previous.path
                || inputs.version != previous.version
            {
                return Err("changed retained domain projection requires its own owner".into());
            }
            inputs.domain_verification = previous.domain_verification.clone();
            inputs.uri_relative_filter_groups = previous.uri_relative_filter_groups.clone();
            for (user, state) in &mut inputs.users {
                state.domain_selection = previous
                    .users
                    .get(user)
                    .ok_or("retained domain user owner unavailable")?
                    .domain_selection
                    .clone();
            }
        }
        let latest = (self.current)()?; // producer validates the actual boot epoch
        let targets = install_targets(current.scan(), scan);
        let mut retained = current.scan().owner().clone();
        if let Some(reason) =
            install_user_rebase_conflict(current.scan(), latest.scan(), &mut retained, &targets)?
        {
            return Err(format!(
                "install context package/UID ownership changed during owner reads: {reason}"
            ));
        }
        if current.state().users != latest.state().users {
            return Err("install context UM user owner changed during owner reads".into());
        }
        // Isolated processes start and end with no relation to the install;
        // the context carries their latest owners (isolated_delta).
        context.system.isolated_owners = latest.context().system.isolated_owners.clone();
        Ok(context)
    }
    fn package(
        &self,
        setting: &settings::Package,
        permission_id: Option<i32>,
        stored: &BTreeMap<i32, restrictions::UserState>,
        code: Option<&crate::package::pkg::AndroidPackage>,
        users: &[i32],
        permissions: &mut BTreeMap<i32, legacy_permissions::State>,
    ) -> Result<PackageInputs, String> {
        if let Some(id) = permission_id {
            if !(0..100_000).contains(&id) {
                return Err(format!(
                    "install context live permission UID unavailable: package={}, permission_id={id}",
                    setting.name
                ));
            }
        } else if super::boot_context::permission_uid_owner(setting, code)?.is_some() {
            return Err("install registered permission owner was omitted".into());
        }
        if stored.keys().any(|id| !users.contains(id)) {
            return Err("install context package references unknown UM user".into());
        }
        if let Some(id) = permission_id {
            if !permissions.contains_key(&id) {
                permissions.insert(
                    id,
                    self.bridge
                        .legacy_permissions(id, users)
                        .map_err(|error| format!("install live permission UID: {error:?}"))?,
                );
            }
        }
        let mut inputs = BTreeMap::new();
        for user in users {
            let Some(permission_id) = permission_id else {
                inputs.insert(
                    *user,
                    UserInputs {
                        gids: Vec::new(),
                        granted_permissions: Vec::new(),
                        domain_selection: None,
                    },
                );
                continue;
            };
            let live = &permissions[&permission_id];
            let installed = stored.get(user).is_none_or(|state| state.installed);
            let gids = self
                .bridge
                .permission_gids(permission_id, &[*user])
                .map_err(|error| format!("install live permission GIDs: {error:?}"))?
                .into_iter()
                .map(|gid| gid as i32)
                .collect();
            inputs.insert(*user, live_user_inputs(live, *user, installed, gids)?);
        }
        let syncable_authorities = code
            .into_iter()
            .flat_map(|code| code.providers.iter())
            .filter(|provider| provider.syncable)
            .filter_map(|provider| {
                provider
                    .authority
                    .as_ref()
                    .map(|authority| (provider.main.component.name.clone(), authority.clone()))
            })
            .collect();
        Ok(PackageInputs {
            app_id: setting.app_id,
            path: setting.code_path.clone(),
            version: setting.version_code,
            installed_permissions: self
                .bridge
                .installed_permissions(&setting.name)
                .map_err(|error| format!("install permission definitions: {error:?}"))?,
            filter_application_query: self
                .bridge
                .application_query_filtering(&setting.name, setting.target_sdk_version)
                .map_err(|error| format!("install query compatibility: {error:?}"))?,
            users: inputs,
            syncable_authorities,
            domain_verification: None,
            uri_relative_filter_groups: Vec::new(),
        })
    }
}

/// The live owner reads `build` makes, sent ahead in one transaction.
fn live_calls(
    scan: &crate::package::scan::SigningScan,
    users: &[i32],
) -> Vec<(u32, aim_binder_host::parcel::Parcel)> {
    use crate::package::bootstrap as bridge;
    let mut calls = Vec::new();
    let mut package = |setting: &settings::Package, permission_id: Option<i32>| {
        if let Some(id) = permission_id {
            calls.push(legacy_permissions::call(id, users));
            calls.extend(bridge::permission_gids_calls(id, users));
        }
        calls.push(bridge::installed_permissions_call(&setting.name));
        calls.push(bridge::application_query_filtering_call(
            &setting.name,
            setting.target_sdk_version,
        ));
    };
    for (settings, factory) in [
        (&scan.settings.packages, false),
        (&scan.settings.disabled_system_packages, true),
    ] {
        for setting in settings {
            let code = if factory {
                scan.disabled_loaded_packages()
            } else {
                scan.loaded_packages()
            }
            .get(&setting.name);
            let id = super::boot_context::permission_uid_owner(
                setting,
                code.map(|code| code.runtime_package()),
            )
            .ok()
            .flatten();
            package(setting, id);
        }
    }
    for (id, _) in scan.identities.ids.owners() {
        if let Some(old) = scan.identities.ids.detached_setting(id) {
            package(&old.package, Some(id));
        }
    }
    for group in scan.identities.shared_users.values() {
        for (_, old) in group.retained_settings() {
            package(&old.package, Some(group.app_id));
        }
    }
    for setting in &scan.settings.packages {
        if let Ok(Some(code)) = scan.domain_code_owner(setting) {
            calls.push(bridge::domain_verification_restricted_call(
                &code.package.package_name,
                code.package.target_sdk_version,
            ));
        }
    }
    for code in scan.disabled_loaded_packages().values() {
        calls.push(bridge::domain_verification_restricted_call(
            &code.package.package_name,
            code.package.target_sdk_version,
        ));
    }
    calls
}

fn live_user_inputs(
    live: &legacy_permissions::State,
    user: i32,
    installed: bool,
    gids: Vec<i32>,
) -> Result<UserInputs, String> {
    let state = live
        .user(user)
        .ok_or("install live permission user unavailable")?;
    let mut grants = Vec::new();
    if installed {
        for permission in &state.permissions {
            if permission.granted {
                grants.push(
                    permission
                        .name
                        .clone()
                        .ok_or("live granted permission has null identity")?,
                );
            }
        }
    }
    Ok(UserInputs {
        gids,
        granted_permissions: grants,
        domain_selection: None,
    })
}
#[cfg(test)]
mod permission_owner_tests {
    use super::*;
    fn domain_snapshot(settings: settings::Settings) -> Arc<Snapshot> {
        use crate::package::{
            owner::usage::Usage,
            scan::{CapturedUsers, SigningScan},
        };
        let names = settings
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect::<Vec<_>>();
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        owner
            .capture_user_states(
                names
                    .iter()
                    .map(|name| {
                        (
                            (name.clone(), false),
                            CapturedUsers {
                                states: BTreeMap::from([(0, Default::default())]),
                                active_aliases: Default::default(),
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap();
        super::super::Store::new(owner, Usage::new(names.iter().map(String::as_str)))
            .unwrap()
            .capture()
    }
    fn domain_settings() -> settings::Settings {
        let mut settings = settings::Settings {
            packages: vec![
                settings::Package {
                    name: "target".into(),
                    app_id: 10100,
                    shared_user: true,
                    ..Default::default()
                },
                settings::Package {
                    name: "sibling".into(),
                    app_id: 10100,
                    shared_user: true,
                    ..Default::default()
                },
                settings::Package {
                    name: "unrelated".into(),
                    app_id: 10101,
                    ..Default::default()
                },
            ],
            shared_users: vec![settings::SharedUser {
                name: "fixture.shared".into(),
                app_id: 10100,
                ..Default::default()
            }],
            ..Default::default()
        };
        settings.domain_verification.active = settings
            .packages
            .iter()
            .enumerate()
            .map(
                |(index, package)| crate::package::domain_verification::Package {
                    name: package.name.clone(),
                    id: format!("00000000-0000-0000-0000-{:012}", index + 1),
                    has_auto_verify_domains: true,
                    signature: None,
                    domains: vec![(Some("fixture.example".into()), 1)],
                    users: vec![crate::package::domain_verification::User {
                        id: 0,
                        allow_link_handling: true,
                        enabled_hosts: vec![],
                    }],
                    uri_relative_filter_groups: vec![],
                },
            )
            .collect();
        settings
    }
    #[test]
    fn install_domain_rebase_preserves_actual_unrelated_status_and_user_selection() {
        let settings = domain_settings();
        let base = domain_snapshot(settings.clone());
        let mut changed = settings.clone();
        let domain = &mut changed.domain_verification.active[2];
        domain.domains[0].1 = 2;
        domain.users[0].allow_link_handling = false;
        domain.users[0].enabled_hosts = vec!["fixture.example".into()];
        let current = domain_snapshot(changed.clone());
        let targets = BTreeSet::from(["target".into()]);
        let mut candidate = base.owner().clone();
        assert!(
            install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            candidate.settings.domain_verification.active[2],
            changed.domain_verification.active[2]
        );
        assert_eq!(
            candidate.settings.domain_verification.active[..2],
            settings.domain_verification.active[..2]
        );
        assert_eq!(
            base.owner().settings.domain_verification,
            settings.domain_verification
        );
        assert!(
            install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .is_none(),
            "already-current domain projection is idempotent"
        );
        let mut competing = base.owner().clone();
        competing.settings.domain_verification.active[2].users[0].enabled_hosts =
            vec!["fixture.example".into()];
        assert!(
            install_user_rebase_conflict(&base, &current, &mut competing, &targets)
                .unwrap()
                .unwrap()
                .contains("competing candidate domain state")
        );
    }
    #[test]
    fn install_domain_rebase_accepts_exact_new_target_owner_publication() {
        let settings = domain_settings();
        let mut before = settings.clone();
        before.domain_verification.active.remove(0);
        let base = domain_snapshot(before);
        let current = domain_snapshot(settings.clone());
        let targets = BTreeSet::from(["target".into()]);
        let mut candidate = current.owner().clone();
        assert!(
            install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            candidate.settings.domain_verification.active,
            settings.domain_verification.active
        );
        let mut competing = base.owner().clone();
        let error = install_user_rebase_conflict(&base, &current, &mut competing, &targets)
            .unwrap()
            .unwrap();
        assert!(
            error.contains("added competing package target")
                && error.contains("candidate_matches=false"),
            "{error}"
        );
    }
    #[test]
    fn install_domain_rebase_rejects_target_shared_and_inventory_conflicts() {
        let settings = domain_settings();
        let base = domain_snapshot(settings.clone());
        let targets = BTreeSet::from(["target".into()]);
        for index in [0, 1] {
            let mut changed = settings.clone();
            changed.domain_verification.active[index].domains[0].1 = 2;
            let current = domain_snapshot(changed);
            let mut candidate = base.owner().clone();
            assert!(
                install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                candidate,
                base.owner().clone(),
                "rejected target/shared-UID change must not mutate candidate"
            );
        }
        for kind in 5..8 {
            let mut changed = settings.clone();
            let domain = &mut changed.domain_verification.active[2];
            match kind {
                0 => domain.id = "00000000-0000-0000-0000-000000000099".into(),
                1 => domain.signature = Some("different-signature-binding".into()),
                2 => domain.domains[0].0 = Some("different.example".into()),
                3 => domain
                    .uri_relative_filter_groups
                    .push((Some("fixture.example".into()), vec![])),
                4 => domain.has_auto_verify_domains = false,
                5 => {
                    changed.domain_verification.active.reverse();
                }
                6 => {
                    changed.domain_verification.active.pop();
                }
                _ => domain.users.push(domain.users[0].clone()),
            }
            let current = domain_snapshot(changed);
            let mut candidate = base.owner().clone();
            assert!(
                install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                    .unwrap()
                    .is_some(),
                "domain identity/manifest/inventory change {kind} was accepted"
            );
            assert_eq!(candidate, base.owner().clone());
        }
    }
    #[test]
    fn install_domain_rebase_preserves_unrelated_identity_and_manifest_capture() {
        let settings = domain_settings();
        let base = domain_snapshot(settings.clone());
        let targets = BTreeSet::from(["target".into()]);
        for kind in 0..5 {
            let mut changed = settings.clone();
            let domain = &mut changed.domain_verification.active[2];
            match kind {
                0 => domain.id = "00000000-0000-0000-0000-000000000099".into(),
                1 => domain.signature = Some("current-domain-signature-binding".into()),
                2 => domain.domains[0].0 = Some("current.example".into()),
                3 => domain
                    .uri_relative_filter_groups
                    .push((Some("fixture.example".into()), vec![])),
                _ => domain.has_auto_verify_domains = false,
            }
            let current = domain_snapshot(changed.clone());
            let mut candidate = base.owner().clone();
            assert!(
                install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                    .unwrap()
                    .is_none(),
                "unrelated capture {kind}"
            );
            assert_eq!(
                candidate.settings.domain_verification.active[2],
                changed.domain_verification.active[2]
            );
            assert_eq!(
                candidate.settings.domain_verification.active[..2],
                settings.domain_verification.active[..2]
            );
            assert_eq!(
                base.owner().settings.domain_verification,
                settings.domain_verification
            );
            assert!(
                install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                    .unwrap()
                    .is_none()
            );
            let mut competing = base.owner().clone();
            competing.settings.domain_verification.active[2].id =
                "00000000-0000-0000-0000-000000000098".into();
            let retained = competing.clone();
            assert!(
                install_user_rebase_conflict(&base, &current, &mut competing, &targets)
                    .unwrap()
                    .unwrap()
                    .contains("competing candidate domain state")
            );
            assert_eq!(competing, retained);
        }
    }
    #[test]
    fn install_domain_rebase_keeps_package_uid_and_user_ownership_strict() {
        let settings = domain_settings();
        let base = domain_snapshot(settings.clone());
        let targets = BTreeSet::from(["target".into()]);
        for kind in 0..3 {
            let mut changed = settings.clone();
            changed.domain_verification.active[2].domains[0].0 = Some("current.example".into());
            match kind {
                0 => changed.packages[2].version_code += 1,
                1 => changed.packages[2].app_id = 10102,
                _ => changed.domain_verification.active[2].users[0].id = 10,
            }
            let current = domain_snapshot(changed);
            let mut candidate = base.owner().clone();
            let retained = candidate.clone();
            assert!(
                install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                    .unwrap()
                    .is_some(),
                "ownership conflict {kind}"
            );
            assert_eq!(candidate, retained);
        }
    }
    #[test]
    fn install_settings_diagnostic_identifies_fields_without_key_material() {
        let before = settings::Settings::default();
        let mut after = before.clone();
        after.permissions.push(settings::Permission {
            name: "fixture.permission".into(),
            package: "target".into(),
            owner: settings::PermissionOwner::Manifest,
            protection_level: 0,
            dynamic: None,
        });
        after.permission_trees.push(settings::Permission {
            name: "fixture.tree".into(),
            package: "target".into(),
            owner: settings::PermissionOwner::Manifest,
            protection_level: 0,
            dynamic: None,
        });
        after.shared_users.push(settings::SharedUser {
            name: "fixture.shared".into(),
            app_id: 10100,
            ..Default::default()
        });
        after.disabled_system_packages.push(settings::Package {
            name: "fixture.factory".into(),
            ..Default::default()
        });
        after
            .key_sets
            .public_keys
            .push((42, b"SECRET_KEY_MATERIAL".to_vec()));
        after.key_sets.key_sets.push((43, vec![42]));
        after.verifier = Some("SECRET_DEVICE_IDENTITY".into());
        after.legacy_domain_info.insert("fixture.domain".into(), 1);
        let message = settings_delta(&before, &after).join("; ");
        for field in [
            "permissions",
            "permission_trees",
            "shared_users",
            "disabled_system_packages",
            "key_sets.public_keys",
            "key_sets.key_sets",
            "verifier",
            "legacy_domain_info",
        ] {
            assert!(message.contains(&format!("settings.{field}:")), "{message}");
        }
        for key in [
            "fixture.permission",
            "fixture.tree",
            "fixture.shared",
            "fixture.factory",
            "fixture.domain",
        ] {
            assert!(message.contains(key));
        }
        assert!(
            !message.contains("SECRET_KEY_MATERIAL") && !message.contains("SECRET_DEVICE_IDENTITY")
        );
        let mut reordered = after.clone();
        reordered.permissions.push(settings::Permission {
            name: "second.permission".into(),
            package: "target".into(),
            owner: settings::PermissionOwner::Manifest,
            protection_level: 0,
            dynamic: None,
        });
        let before = reordered.clone();
        reordered.permissions.reverse();
        let message = settings_delta(&before, &reordered).join("; ");
        assert!(message.contains("changed_keys=0") && message.contains("key_order_changed=true"));
    }

    #[test]
    fn install_normalized_residual_keeps_global_target_and_shared_uid_conflicts() {
        use crate::package::{
            owner::usage::Usage,
            restrictions::UserState,
            scan::{CapturedUsers, SigningScan},
        };
        let settings = settings::Settings {
            packages: vec![
                settings::Package {
                    name: "target".into(),
                    app_id: 10100,
                    shared_user: true,
                    ..Default::default()
                },
                settings::Package {
                    name: "sibling".into(),
                    app_id: 10100,
                    shared_user: true,
                    ..Default::default()
                },
                settings::Package {
                    name: "unrelated".into(),
                    app_id: 10101,
                    ..Default::default()
                },
            ],
            shared_users: vec![settings::SharedUser {
                name: "fixture.shared".into(),
                app_id: 10100,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        owner
            .capture_user_states(
                ["target", "sibling", "unrelated"]
                    .into_iter()
                    .map(|name| {
                        (
                            (name.into(), false),
                            CapturedUsers {
                                states: BTreeMap::from([(0, UserState::default())]),
                                active_aliases: Default::default(),
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap();
        let snapshot = |owner: SigningScan| {
            super::super::Store::new(owner, Usage::new(vec!["target", "sibling", "unrelated"]))
                .unwrap()
                .capture()
        };
        let base = snapshot(owner.clone());
        let targets = BTreeSet::from(["target".into()]);
        let mut changed = owner.clone();
        let user = UserState {
            enabled_components: Some(vec!["fixture.Component".into()]),
            ..Default::default()
        };
        changed
            .set_user_state("unrelated", 0, user.clone())
            .unwrap();
        let current = snapshot(changed.clone());
        let mut candidate = owner.clone();
        assert!(
            install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            candidate.scanned_user_states("unrelated").unwrap()[&0],
            user
        );
        changed.settings.verifier = Some("actual-verifier-change".into());
        let current = snapshot(changed);
        let mut candidate = owner.clone();
        let reason = install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
            .unwrap()
            .unwrap();
        assert!(
            reason.contains("unmerged normalized owner") && reason.contains("settings.verifier:")
        );
        assert!(
            !reason.contains("package unrelated user"),
            "merged unrelated user must not be reported as residual: {reason}"
        );
        for name in ["target", "sibling"] {
            let mut changed = owner.clone();
            changed.set_user_state(name, 0, user.clone()).unwrap();
            let current = snapshot(changed);
            let mut candidate = owner.clone();
            let reason = install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .unwrap();
            assert!(
                reason.contains(&format!("package {name} user 0 state ownership")),
                "{reason}"
            );
        }
        // OverlayManager's paths for the target carry into the candidate.
        let mut changed = owner.clone();
        let mut overlaid = UserState::default();
        let paths = crate::package::model::OverlayPaths {
            resource_dirs: vec!["/product/overlay/fixture.apk".into()],
            overlay_paths: vec![],
        };
        overlaid.runtime.set_overlay_paths(Some(paths.clone()));
        changed.set_user_state("target", 0, overlaid).unwrap();
        let current = snapshot(changed);
        let mut candidate = owner.clone();
        assert!(
            install_user_rebase_conflict(&base, &current, &mut candidate, &targets)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            candidate.scanned_user_states("target").unwrap()[&0]
                .runtime
                .overlays(),
            Some(&paths)
        );
    }
    #[test]
    fn modern_context_preserves_grants_and_actual_gid_inputs_independent_of_legacy() {
        let mut modern = legacy_permissions::Migration::default();
        modern
            .put(
                0,
                legacy_permissions::Permission {
                    name: Some("modern.grant".into()),
                    runtime: true,
                    granted: true,
                    flags: 0x34,
                },
            )
            .unwrap();
        let modern = modern.project(10100, &[0]).unwrap();
        let gids = vec![3003, 10100, 111];
        let installed = live_user_inputs(&modern, 0, true, gids.clone()).unwrap();
        assert_eq!(installed.gids, gids);
        assert_eq!(installed.granted_permissions, vec!["modern.grant"]);
        let absent = live_user_inputs(&modern, 0, false, gids.clone()).unwrap();
        assert_eq!(absent.gids, gids);
        assert!(absent.granted_permissions.is_empty());
        assert!(modern.user(0).unwrap().permissions[0].granted);
        assert_eq!(modern.user(0).unwrap().permissions[0].flags, 0x34);
        assert!(live_user_inputs(&modern, 10, true, gids).is_err());
    }
}
