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

#[derive(Clone, Debug, PartialEq)]
struct Input {
    code: Arc<LoadedPackage>,
    flags: i32,
    private_flags: i32,
    app_id: i32,
    shared: Option<(i32, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    inputs: BTreeMap<String, Input>,
    labels: BTreeMap<String, String>,
}

fn inputs(owner: &SigningScan) -> Result<BTreeMap<String, Input>, String> {
    if !owner.capture_ready() {
        return Err("scan metadata is not finalized".into());
    }
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
                let Some(Owner::SharedUser(group_name)) = owner.identities.ids.get(setting.app_id)
                else {
                    return Err(format!("seInfo shared UID owner differs: {name}"));
                };
                let group = owner
                    .identities
                    .shared_users
                    .get(group_name)
                    .filter(|group| group.app_id == setting.app_id && group.has_package(name))
                    .ok_or_else(|| format!("seInfo shared UID membership differs: {name}"))?;
                Some((
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
                    app_id: setting.app_id,
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
        candidate.fix_shared_seinfo_target_sdks_at_boot()?;
        let inputs = inputs(&candidate)?;
        let mut labels = BTreeMap::new();
        for (name, input) in &inputs {
            let (target, shared_privileged) = match input.shared {
                Some(shared) => shared,
                None => (compatibility(&input.code.package)?, false),
            };
            labels.insert(
                name.clone(),
                policy.label(
                    &input.code.package.package_name,
                    Signing::Known(&input.code.collected_signing),
                    shared_privileged || input.private_flags & PRIVATE_FLAG_PRIVILEGED != 0,
                    target,
                    partition(input),
                ),
            );
        }
        self.identities.shared_users = candidate.identities.shared_users;
        self.seinfo = Some(Assignments { inputs, labels });
        Ok(())
    }

    pub(in crate::package) fn validate_seinfo(&self) -> Result<(), String> {
        if let Some(assignments) = &self.seinfo {
            if assignments.inputs != inputs(self)? {
                return Err("seInfo inputs changed since assignment".into());
            }
        }
        Ok(())
    }

    /// Missing assignment is an unfinished scan phase, not the original
    /// explicit policy-unread state. Unknown/unloaded names return None.
    pub fn seinfo(&self, name: &str) -> Result<Option<&str>, String> {
        self.validate_seinfo()?;
        let assignments = self
            .seinfo
            .as_ref()
            .ok_or_else(|| "seInfo is not assigned".to_string())?;
        Ok(assignments.labels.get(name).map(String::as_str))
    }
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
                signatures: vec![vec![3]],
                scheme_version: 3,
                public_keys: vec![],
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
            Some("default:targetSdkVersion=30")
        );
        owner.pending_metadata.clear();
        owner.loaded.remove("b");
        assert_eq!(
            owner.validate_seinfo(),
            Err("seInfo inputs changed since assignment".into())
        );
    }
}
