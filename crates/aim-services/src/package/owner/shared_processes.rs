//! SharedUserSetting.addProcesses/updateProcesses and ParsedProcessImpl.addStateFrom,
//! android-16.0.0_r1. Copyright AOSP, Apache License 2.0.
use crate::package::{
    parse::parcel::{ArrayMap, java_hash},
    pkg::Process,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Processes(Vec<Process>);

fn hash(name: Option<&str>) -> i32 {
    name.map_or(0, java_hash)
}

fn union(target: &mut Vec<String>, incoming: &[String]) {
    for permission in incoming {
        if !target.contains(permission) {
            target.push(permission.clone());
        }
    }
    target.sort_by_key(|name| java_hash(name));
}

fn classes(target: &mut Vec<(String, Option<String>)>, incoming: &[(String, Option<String>)]) {
    let mut map = ArrayMap::default();
    for (name, value) in target.iter().chain(incoming) {
        map.put(name, value.clone());
    }
    *target = map
        .iter()
        .map(|(name, value)| (name.into(), value.clone()))
        .collect();
}

impl Processes {
    pub fn records(&self) -> &[Process] {
        &self.0
    }

    /// Process map keys do not select the aggregate: the original uses each
    /// value's process name. Collected code must satisfy the original non-null name contract.
    pub fn add(&mut self, incoming: Option<&[Process]>) -> Result<(), String> {
        if incoming.into_iter().flatten().any(|p| p.name.is_none()) {
            return Err("collected process name is null".into());
        }
        let mut candidate = self.clone();
        candidate.merge(incoming)?;
        *self = candidate;
        Ok(())
    }

    fn merge(&mut self, incoming: Option<&[Process]>) -> Result<(), String> {
        for new in incoming.unwrap_or_default() {
            if let Some(old) = self.0.iter_mut().find(|old| old.name == new.name) {
                // ParsedProcessImpl's copy uses immutable ArrayMap.EMPTY here;
                // addStateFrom cannot grow it. Reject before publishing a candidate.
                if old.app_class_names_by_package.is_empty()
                    && !new.app_class_names_by_package.is_empty()
                {
                    return Err("original empty process class owner is immutable".into());
                }
                union(&mut old.denied_permissions, &new.denied_permissions);
                classes(
                    &mut old.app_class_names_by_package,
                    &new.app_class_names_by_package,
                );
                old.gwp_asan_mode = new.gwp_asan_mode;
                old.memtag_mode = new.memtag_mode;
                old.native_heap_zero_initialized = new.native_heap_zero_initialized;
                old.use_embedded_dex = new.use_embedded_dex;
            } else {
                let mut copy = new.clone();
                copy.map_key = copy.name.clone();
                let mut denied = Vec::new();
                union(&mut denied, &copy.denied_permissions);
                copy.denied_permissions = denied;
                let mut names = Vec::new();
                classes(&mut names, &copy.app_class_names_by_package);
                copy.app_class_names_by_package = names;
                self.0.push(copy);
                self.0.sort_by_key(|process| hash(process.name.as_deref()));
            }
        }
        Ok(())
    }

    /// The owning operation supplies the original reverse member iteration;
    /// sorted package names cannot stand in for an identity-based ArraySet.
    pub fn rebuild<'a>(
        &mut self,
        members: impl IntoIterator<Item = Option<&'a [Process]>>,
    ) -> Result<(), String> {
        let mut candidate = Self::default();
        for member in members {
            candidate.add(member)?;
        }
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merges_incoming_modes_overwrites_classes_and_retains_collisions() {
        let a = Process {
            name: Some("worker".into()),
            map_key: Some("ignored".into()),
            app_class_names_by_package: vec![
                ("BB".into(), Some("first".into())),
                ("Aa".into(), None),
            ],
            denied_permissions: vec!["BB".into()],
            gwp_asan_mode: 1,
            memtag_mode: 2,
            native_heap_zero_initialized: 1,
            use_embedded_dex: true,
        };
        let b = Process {
            name: Some("worker".into()),
            map_key: Some("different".into()),
            app_class_names_by_package: vec![
                ("BB".into(), None),
                ("new".into(), Some("last".into())),
            ],
            denied_permissions: vec!["Aa".into(), "BB".into()],
            gwp_asan_mode: -1,
            memtag_mode: -1,
            native_heap_zero_initialized: -1,
            use_embedded_dex: false,
        };
        let mut owner = Processes::default();
        owner.add(Some(std::slice::from_ref(&a))).unwrap();
        let old = owner.clone();
        owner.add(Some(std::slice::from_ref(&b))).unwrap();
        let merged = &owner.records()[0];
        assert_eq!(merged.map_key, merged.name);
        assert_eq!(merged.denied_permissions, ["BB", "Aa"]);
        assert_eq!(
            merged
                .app_class_names_by_package
                .iter()
                .find(|(name, _)| name == "BB")
                .unwrap()
                .1,
            None
        );
        assert_eq!(
            (
                merged.gwp_asan_mode,
                merged.memtag_mode,
                merged.native_heap_zero_initialized,
                merged.use_embedded_dex
            ),
            (-1, -1, -1, false)
        );
        assert_eq!(old.records()[0], a.clone().with_aggregate_key());
        owner
            .rebuild([None, Some(std::slice::from_ref(&a))])
            .unwrap();
        assert_eq!(owner, old);
        owner.rebuild([]).unwrap();
        assert!(owner.records().is_empty());
    }
    impl Process {
        fn with_aggregate_key(mut self) -> Self {
            self.map_key = self.name.clone();
            self
        }
    }
    #[test]
    fn null_names_reject_atomically_and_empty_names_retain_collision_order() {
        let inputs = [Some(""), Some("BB"), Some("Aa")].map(|name| Process {
            name: name.map(str::to_owned),
            map_key: Some("same".into()),
            ..Default::default()
        });
        let mut owner = Processes::default();
        owner.add(Some(&inputs)).unwrap();
        assert_eq!(
            owner
                .records()
                .iter()
                .map(|p| p.name.as_deref())
                .collect::<Vec<_>>(),
            [Some(""), Some("BB"), Some("Aa")]
        );
        owner.add(None).unwrap();
        let retained = owner.clone();
        let invalid = [inputs[0].clone(), Process::default()];
        assert!(owner.add(Some(&invalid)).is_err());
        assert_eq!(owner, retained);
        assert!(
            owner
                .rebuild([Some(inputs.as_slice()), Some(invalid.as_slice())])
                .is_err()
        );
        assert_eq!(owner, retained);
    }
}
