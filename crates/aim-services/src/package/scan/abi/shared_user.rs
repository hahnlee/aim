//! Post-boot shared UID ABI adjustment (PackageManagerService, #810).
use super::{SharedUserAbi, SharedUserAbiMismatch};
use crate::package::scan::SigningScan;
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SharedUserAbiReconciliation {
    pub changed_code_paths: Vec<String>,
    pub mismatches: Vec<SharedUserAbiMismatch>,
}

impl SigningScan {
    /// Boot scans defer adjustment until all system and data requirements are
    /// known. Parsed code keeps its raw ABI; only null setting ABIs inherit.
    pub fn reconcile_shared_user_abis(&mut self) -> Result<SharedUserAbiReconciliation, String> {
        let mut settings = self.settings.clone();
        let mut identities = self.identities.clone();
        let mut report = SharedUserAbiReconciliation::default();
        for group in identities.shared_users.values_mut() {
            let mut retained: BTreeMap<&str, VecDeque<_>> = BTreeMap::new();
            for (name, value) in group.retained_settings() {
                retained.entry(name).or_default().push_back(value.package.clone());
            }
            let mut members = Vec::new();
            let mut active = Vec::new();
            for (name, detached) in group.ordered_process_members()? {
                let package = if *detached {
                    retained.get_mut(name.as_str()).and_then(VecDeque::pop_front)
                        .ok_or("shared ABI retained insertion slot missing")?
                } else {
                    let index = settings.packages.iter().position(|package| package.name == *name)
                        .ok_or("shared ABI active setting missing")?;
                    active.push((index, members.len()));
                    settings.packages[index].clone()
                };
                if package.shared_app_id() != Some(group.app_id) {
                    return Err("shared ABI member belongs to another UID".into());
                }
                members.push(package);
            }
            drop(retained);
            let choice = SharedUserAbi::derive(&members, None)?;
            for (index, _) in &active {
                let member = &settings.packages[*index];
                if member.primary_cpu_abi.is_none() && self.loaded.get(&member.name)
                    .is_some_and(|code| code.package.primary_cpu_abi != choice.primary) {
                    report.changed_code_paths.push(member.code_path.clone());
                }
            }
            choice.apply(&mut members, &BTreeMap::new(), None);
            report.mismatches.extend(choice.mismatches);
            for (index, member) in active {
                settings.packages[index].primary_cpu_abi = members[member].primary_cpu_abi.clone();
            }
            group.apply_retained_primary_abi(&choice.primary);
        }
        self.settings = settings;
        self.identities = identities;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{settings::{Package, Settings, SharedUser}, system_config::SystemConfig};

    fn member(name: &str, abi: Option<&str>) -> Package {
        Package { name: name.into(), app_id: 1000, shared_user: true, code_path: format!("/system/app/{name}"), primary_cpu_abi: abi.map(str::to_owned), secondary_cpu_abi: Some("armeabi-v7a".into()), ..Default::default() }
    }
    fn scan(packages: Vec<Package>) -> SigningScan {
        SigningScan::new(&SystemConfig::default(), &Settings { packages, shared_users: vec![SharedUser { name: "android.uid.system".into(), app_id: 1000, flags: 0, signatures: None }], ..Default::default() }, 35).unwrap()
    }

    #[test]
    fn boot_owner_uses_later_requirement_and_preserves_existing_abis() {
        let mut owner = scan(vec![member("java", None), member("native", Some("arm64-v8a")), member("other", Some("x86_64"))]);
        owner.loaded.insert("java".into(), std::sync::Arc::new(crate::package::scan::LoadedPackage::new(
            crate::package::pkg::AndroidPackage { package_name: "java".into(), ..Default::default() },
            crate::package::sign::SigningDetails::unknown(),
        ).unwrap()));
        let before = owner.clone();
        let report = owner.reconcile_shared_user_abis().unwrap();
        assert_eq!(report.changed_code_paths, ["/system/app/java"]);
        assert_eq!(owner.loaded["java"].package.primary_cpu_abi, None);
        assert_eq!(owner.loaded, before.loaded);
        assert_eq!(owner.settings.packages[0].primary_cpu_abi.as_deref(), Some("arm64-v8a"));
        assert_eq!(owner.settings.packages[2].primary_cpu_abi.as_deref(), Some("x86_64"));
        assert_eq!(report.mismatches.len(), 1);
        assert_eq!(report.mismatches[0].required_by.as_deref(), Some("native"));
        assert_eq!(owner.settings.packages[0].secondary_cpu_abi, before.settings.packages[0].secondary_cpu_abi);
        assert_eq!(before.settings.packages[0].primary_cpu_abi, None);
        assert!(owner.reconcile_shared_user_abis().unwrap().changed_code_paths.is_empty());
    }

    #[test]
    fn retained_same_name_instances_keep_order_and_copy_on_write() {
        let mut owner = scan(vec![member("same", Some("x86_64")), member("java", None)]);
        let detached = |package| crate::package::owner::app_ids::DetachedSetting {
            package, users: Default::default(), user_aliases: Default::default(), legacy: None, install_fixed: None, runtime: None,
        };
        let group = owner.identities.shared_users.get_mut("android.uid.system").unwrap();
        group.retain_read_instance(detached(member("same", Some("arm64-v8a")))).unwrap();
        group.retain_read_instance(detached(member("same", None))).unwrap();
        let old = group.clone();
        let report = owner.reconcile_shared_user_abis().unwrap();
        assert_eq!(report.mismatches.len(), 1);
        assert_eq!(owner.settings.packages[1].primary_cpu_abi.as_deref(), Some("x86_64"));
        let retained: Vec<_> = owner.identities.shared_users["android.uid.system"].retained_settings().map(|(_, value)| value.package.primary_cpu_abi.as_deref()).collect();
        assert_eq!(retained, [Some("arm64-v8a"), Some("x86_64")]);
        assert_eq!(old.retained_settings().last().unwrap().1.package.primary_cpu_abi, None);
        assert_eq!(owner.identities.shared_users["android.uid.system"].retained_settings().last().unwrap().1.package.secondary_cpu_abi.as_deref(), Some("armeabi-v7a"));
    }

    #[test]
    fn absent_requirement_stays_null_and_invalid_abi_is_atomic() {
        let mut owner = scan(vec![member("java", None)]);
        owner.reconcile_shared_user_abis().unwrap();
        assert_eq!(owner.settings.packages[0].primary_cpu_abi, None);
        owner.settings.packages.push(member("bad", Some("unknown")));
        owner.identities.shared_users.get_mut("android.uid.system").unwrap().add_package("bad", 0, 0);
        let before = owner.clone();
        assert!(owner.reconcile_shared_user_abis().is_err());
        assert_eq!(owner, before);
    }
}
