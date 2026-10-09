//! Active scan seInfo assignments, from pinned SELinuxMMAC (#838).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{LoadedPackage, SigningScan};
use crate::package::{
    owner::{
        app_ids::Owner,
        seinfo::{Partition, Policy, Signing},
    },
    pkg::AndroidPackage,
    settings::{FLAG_SYSTEM, PRIVATE_FLAG_PRIVILEGED},
};
use std::{collections::BTreeMap, sync::Arc};
crate::install_mutator_seinfo_apply!();

#[derive(Clone, Debug, PartialEq)]
struct Input {
    code: Arc<LoadedPackage>,
    flags: i32,
    private_flags: i32,
    app_id: i32,
    shared: Option<(String, i32, bool)>,
}

/// Transient PackageStateUnserialized fields. A boot-only shared override may
/// precede base restoration; that missing base is explicit, never invented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeInfoState {
    pub base: Option<String>,
    pub override_label: Option<String>,
}

impl SeInfoState {
    pub fn effective(&self) -> Option<&str> {
        self.override_label
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.base.as_deref())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeInfoSetting {
    /// Copy existing in-memory PackageStateUnserialized fields.
    Retained,
    /// Fresh transient state from a new setting or an initial disk restore;
    /// neither seInfo field is persisted by Settings.
    New,
}

/// Original PlatformCompat decisions when no loaded shared code supplies a target.
pub trait SeInfoCompatibility {
    fn target_sdk(&self, package: &AndroidPackage) -> Result<i32, String>;
}

impl<F: Fn(&AndroidPackage) -> Result<i32, String>> SeInfoCompatibility for F {
    fn target_sdk(&self, package: &AndroidPackage) -> Result<i32, String> {
        self(package)
    }
}

impl SeInfoCompatibility for crate::package::bootstrap::Bridge {
    fn target_sdk(&self, package: &AndroidPackage) -> Result<i32, String> {
        self.seinfo_target_sdk(package)
            .map_err(|error| format!("original seInfo compatibility: {error:?}"))
    }
}

/// Required policy and compatibility owners for accepted scan metadata.
#[derive(Clone, Copy)]
pub struct SeInfoScan<'a> {
    pub policy: &'a Policy,
    pub compatibility: &'a dyn SeInfoCompatibility,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    inputs: BTreeMap<String, Input>,
    labels: BTreeMap<String, SeInfoState>,
}

fn inputs(owner: &SigningScan) -> Result<BTreeMap<String, Input>, String> {
    if !owner.capture_ready() {
        return Err("scan metadata is not finalized".into());
    }
    active_inputs(owner)
}

fn active_inputs(owner: &SigningScan) -> Result<BTreeMap<String, Input>, String> {
    owner
        .loaded
        .iter()
        .map(|(name, code)| {
            let setting = owner
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("seInfo code has no active setting: {name}"))?;
            let shared = if setting.shared_user {
                let Some(Owner::SharedUser(group_name)) =
                    owner.identities.ids.get(setting.uid_owner_id())
                else {
                    return Err(format!("seInfo shared UID owner differs: {name}"));
                };
                let group = owner
                    .identities
                    .shared_users
                    .get(group_name)
                    .filter(|group| {
                        Some(group.app_id) == setting.shared_app_id() && group.has_package(name)
                    })
                    .ok_or_else(|| format!("seInfo shared UID membership differs: {name}"))?;
                Some((
                    group_name.clone(),
                    group.seinfo_target_sdk(),
                    group.private_flags & PRIVATE_FLAG_PRIVILEGED != 0,
                ))
            } else {
                None
            };
            Ok((
                name.clone(),
                Input {
                    code: code.clone(),
                    flags: setting.flags,
                    private_flags: setting.private_flags,
                    app_id: setting.uid_owner_id(),
                    shared,
                },
            ))
        })
        .collect()
}

fn partition(input: &Input) -> Partition {
    // PackageSetting's original ApplicationInfo private-flag masks.
    for (mask, partition) in [
        (1 << 21, Partition::SystemExt),
        (1 << 19, Partition::Product),
        (1 << 18, Partition::Vendor),
        (1 << 17, Partition::Oem),
        (1 << 30, Partition::Odm),
    ] {
        if input.private_flags & mask != 0 {
            return partition;
        }
    }
    if input.flags & FLAG_SYSTEM != 0 {
        Partition::System
    } else {
        Partition::Data
    }
}

impl SigningScan {
    /// Assign at the completed boot scan, including shared SDK finalization. The
    /// bootstrap owner supplies non-shared compatibility; no fallback is guessed.
    /// All owner queries finish before replacing any prior assignments.
    pub fn assign_seinfo_at_boot(
        &mut self,
        policy: &Policy,
        compatibility: &mut dyn FnMut(&AndroidPackage) -> Result<i32, String>,
    ) -> Result<(), String> {
        let mut candidate = self.clone();
        let abi = candidate.reconcile_shared_user_abis()?;
        for mismatch in &abi.mismatches { eprintln!("shared UID ABI mismatch at boot: {mismatch:?}"); }
        candidate.fix_shared_seinfo_target_sdks_at_boot()?;
        let inputs = inputs(&candidate)?;
        let mut labels = BTreeMap::new();
        for (name, input) in &inputs {
            let (target, shared_privileged) = match &input.shared {
                Some((_, target, privileged)) => (*target, *privileged),
                None => (compatibility(&input.code.package)?, false),
            };
            let label = policy.label(
                &input.code.package.package_name,
                Signing::Known(&input.code.collected_signing),
                shared_privileged || input.private_flags & PRIVATE_FLAG_PRIVILEGED != 0,
                target,
                partition(input),
            );
            let state = if input.shared.is_some() {
                SeInfoState {
                    base: self.seinfo.as_ref().and_then(|old| {
                        old.inputs
                            .get(name)
                            .filter(|previous| same_setting(previous, input))
                            .and_then(|_| old.labels.get(name))
                            .and_then(|state| state.base.clone())
                    }),
                    override_label: Some(label),
                }
            } else {
                SeInfoState {
                    base: Some(label),
                    override_label: None,
                }
            };
            labels.insert(name.clone(), state);
        }
        self.identities.shared_users = candidate.identities.shared_users;
        self.seinfo = Some(Assignments { inputs, labels });
        Ok(())
    }

    /// Settings conversion keeps PackageStateUnserialized's assigned label;
    /// only its validated UID/group dependency changes.
    pub(super) fn rebind_apex_seinfo_after_conversion(
        &mut self,
        name: &str,
        id: i32,
    ) -> Result<(), String> {
        let current = active_inputs(self)?
            .remove(name)
            .ok_or("converted APEX seInfo input is missing")?;
        let previous = self
            .seinfo
            .as_mut()
            .and_then(|a| a.inputs.get_mut(name))
            .ok_or("converted APEX seInfo assignment is missing")?;
        if previous.app_id != id
            || previous.shared.is_none()
            || current.app_id != -1
            || current.shared.is_some()
            || previous.code != current.code
            || previous.flags != current.flags
            || previous.private_flags != current.private_flags
        {
            return Err("converted APEX seInfo dependencies differ".into());
        }
        *previous = current;
        self.validate_seinfo()
    }

    pub(in crate::package) fn retain_seinfo_after_code_removal(&mut self) -> Result<(), String> {
        let current = inputs(self)?;
        if let Some(assigned) = &mut self.seinfo {
            for (name, input) in &current {
                if assigned.inputs.get(name).is_none_or(|previous| !same_setting(previous, input)) {
                    return Err(format!("remaining removal seInfo identity differs: {name}"));
                }
            }
            assigned.inputs.retain(|name, _| current.contains_key(name));
            assigned.labels.retain(|name, _| current.contains_key(name));
        }
        Ok(())
    }

    pub(in crate::package) fn validate_seinfo(&self) -> Result<(), String> {
        if let Some(assignments) = &self.seinfo {
            let current = inputs(self)?;
            if assignments.inputs.len() != current.len()
                || !assignments.inputs.iter().all(|(name, previous)| {
                    current
                        .get(name)
                        .is_some_and(|input| same_setting(previous, input))
                })
            {
                return Err("seInfo inputs changed since assignment".into());
            }
        }
        Ok(())
    }

    /// Missing assignment is an unfinished scan phase, not the original
    /// explicit policy-unread state. Unknown/unloaded names return None.
    pub fn seinfo(&self, name: &str) -> Result<Option<&str>, String> {
        Ok(self.seinfo_state(name)?.and_then(SeInfoState::effective))
    }

    pub fn seinfo_state(&self, name: &str) -> Result<Option<&SeInfoState>, String> {
        self.validate_seinfo()?;
        self.validated_seinfo_state(name)
    }

    /// Internal batch accessor: caller has validated the complete seInfo owner
    /// and holds an immutable borrow for the entire batch.
    pub(in crate::package) fn validated_seinfo_state(&self, name: &str) -> Result<Option<&SeInfoState>, String> {
        let assignments = self
            .seinfo
            .as_ref()
            .ok_or_else(|| "seInfo is not assigned".to_string())?;
        Ok(assignments.labels.get(name))
    }

    /// ScanPackageUtils copies a retained setting, updates its base label, and
    /// leaves the copied override intact. A replacement setting starts fresh.
    /// This does not run boot's shared-SDK minimum or relabel other members.
    pub fn assign_seinfo_for_scan(
        &mut self,
        name: &str,
        setting: SeInfoSetting,
        policy: &Policy,
        compatibility: &mut dyn FnMut(&AndroidPackage) -> Result<i32, String>,
    ) -> Result<(), String> {
        self.assign_seinfo_for_scan_with_shared_target(name, setting, policy, compatibility, None)
    }

    pub(super) fn assign_seinfo_for_scan_with_shared_target(
        &mut self,
        name: &str,
        setting: SeInfoSetting,
        policy: &Policy,
        compatibility: &mut dyn FnMut(&AndroidPackage) -> Result<i32, String>,
        shared_target: Option<i32>,
    ) -> Result<(), String> {
        if self.pending_metadata.contains(name) {
            return Err("scan metadata is not finalized".into());
        }
        let current = active_inputs(self)?;
        let input = current
            .get(name)
            .ok_or_else(|| "seInfo scan has no active code".to_string())?;
        let old = self.seinfo.as_ref();
        let override_label = match setting {
            SeInfoSetting::New => None,
            SeInfoSetting::Retained => {
                let previous = old
                    .and_then(|old| old.inputs.get(name))
                    .ok_or_else(|| "retained seInfo setting is missing".to_string())?;
                if previous.app_id != input.app_id || shared_name(previous) != shared_name(input) {
                    return Err("retained seInfo UID ownership changed".into());
                }
                old.unwrap()
                    .labels
                    .get(name)
                    .unwrap()
                    .override_label
                    .clone()
            }
        };
        let (target, shared_privileged) = match &input.shared {
            Some((_, target, privileged)) => (shared_target.unwrap_or(*target), *privileged),
            None => (compatibility(&input.code.package)?, false),
        };
        let base = policy.label(
            &input.code.package.package_name,
            Signing::Known(&input.code.collected_signing),
            shared_privileged || input.private_flags & PRIVATE_FLAG_PRIVILEGED != 0,
            target,
            partition(input),
        );
        let mut next = old.cloned().unwrap_or(Assignments {
            inputs: BTreeMap::new(),
            labels: BTreeMap::new(),
        });
        next.inputs.insert(name.into(), input.clone());
        next.labels.insert(
            name.into(),
            SeInfoState {
                base: Some(base),
                override_label,
            },
        );
        self.seinfo = Some(next);
        Ok(())
    }
}

impl SigningScan {
    pub(super) fn has_seinfo_assignment(&self, name: &str) -> bool {
        self.seinfo
            .as_ref()
            .is_some_and(|state| state.inputs.contains_key(name) && state.labels.contains_key(name))
    }

    pub(super) fn seinfo_setting_for_scan(&self, name: &str) -> Result<SeInfoSetting, String> {
        let current = active_inputs(self)?;
        let input = current
            .get(name)
            .ok_or_else(|| "seInfo scan has no active code".to_string())?;
        let previous = self
            .seinfo
            .as_ref()
            .and_then(|state| state.inputs.get(name));
        Ok(match previous {
            Some(previous)
                if previous.app_id == input.app_id
                    && shared_name(previous) == shared_name(input) =>
            {
                SeInfoSetting::Retained
            }
            _ => SeInfoSetting::New,
        })
    }
}

fn shared_name(input: &Input) -> Option<&str> {
    input.shared.as_ref().map(|(name, _, _)| name.as_str())
}

fn same_setting(previous: &Input, current: &Input) -> bool {
    previous.code == current.code
        && previous.flags == current.flags
        && previous.private_flags == current.private_flags
        && previous.app_id == current.app_id
        && shared_name(previous) == shared_name(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        settings::{Package, Settings},
        sign::SigningDetails,
    };

    #[test]
    fn boot_assignment_queries_nonshared_owner_and_retains_state_on_failure() {
        let settings = Settings {
            packages: vec![
                Package {
                    name: "a".into(),
                    app_id: 19000,
                    flags: 1,
                    private_flags: (1 << 21) | (1 << 18),
                    ..Default::default()
                },
                Package {
                    name: "b".into(),
                    app_id: 19001,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        for setting in &settings.packages {
            let signing = SigningDetails {
                unknown: false,
                current_flags: Vec::new(),
                signatures: vec![vec![3]],
                scheme_version: 3,
                public_keys: Some(vec![]),
                past_signing_certificates: None,
            };
            owner.loaded.insert(
                setting.name.clone(),
                Arc::new(
                    LoadedPackage::new(
                        AndroidPackage {
                            package_name: setting.name.clone(),
                            target_sdk_version: 29,
                            signing_details: Some(signing.parcel_details().unwrap()),
                            ..Default::default()
                        },
                        signing,
                    )
                    .unwrap(),
                ),
            );
        }
        let policy = Policy::unread();
        let before = owner.clone();
        let mut calls = vec![];
        assert_eq!(
            owner.assign_seinfo_at_boot(&policy, &mut |pkg| {
                calls.push(pkg.package_name.clone());
                if pkg.package_name == "b" {
                    Err("compat owner denied".into())
                } else {
                    Ok(30)
                }
            }),
            Err("compat owner denied".into())
        );
        assert_eq!(calls, ["a", "b"]);
        assert_eq!(owner, before);
        owner
            .assign_seinfo_at_boot(&policy, &mut |pkg| {
                assert_eq!(pkg.target_sdk_version, 29);
                Ok(if pkg.package_name == "a" { 10000 } else { 30 })
            })
            .unwrap();
        assert_eq!(
            owner.seinfo("a").unwrap(),
            Some("default:targetSdkVersion=10000:partition=system_ext")
        );
        assert_eq!(
            owner.seinfo("b").unwrap(),
            Some("default:targetSdkVersion=30")
        );
        let before = owner.clone();
        assert_eq!(
            owner.assign_seinfo_for_scan("b", SeInfoSetting::Retained, &policy, &mut |_| Err(
                "runtime compat denied".into()
            )),
            Err("runtime compat denied".into())
        );
        assert_eq!(owner, before);
        owner
            .assign_seinfo_for_scan("b", SeInfoSetting::Retained, &policy, &mut |_| Ok(10000))
            .unwrap();
        assert_eq!(
            owner.seinfo("b").unwrap(),
            Some("default:targetSdkVersion=10000")
        );
        assert_eq!(
            before.seinfo("b").unwrap(),
            Some("default:targetSdkVersion=30")
        );
        let capture = owner.clone();
        owner.pending_metadata.insert("b".into());
        let before = owner.clone();
        assert_eq!(
            owner.assign_seinfo_at_boot(&policy, &mut |_| panic!("pending metadata")),
            Err("scan metadata is not finalized".into())
        );
        assert_eq!(owner, before);
        assert_eq!(
            capture.seinfo("b").unwrap(),
            Some("default:targetSdkVersion=10000")
        );
        owner.pending_metadata.clear();
        owner.loaded.remove("b");
        assert_eq!(
            owner.validate_seinfo(),
            Err("seInfo inputs changed since assignment".into())
        );
    }
}
