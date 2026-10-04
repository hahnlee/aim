//! Captured shared process owners require the actual ordered member inventory (#870).
use super::SigningScan;
use crate::package::{owner::shared_processes::Processes, pkg::Process};
use std::collections::{BTreeMap, BTreeSet};

// None is an unparsed member; Some(None) is parsed code with a null process map.
type Members = BTreeMap<String, Option<Option<Vec<Process>>>>;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    orders: BTreeMap<String, Vec<String>>,
    inputs: BTreeMap<String, (i32, Members)>,
    owners: BTreeMap<String, Processes>,
}

fn build(scan: &SigningScan, orders: BTreeMap<String, Vec<String>>) -> Result<Assignments, String> {
    if !scan.capture_ready() {
        return Err("scan metadata is not finalized".into());
    }
    if orders.keys().collect::<BTreeSet<_>>() != scan.identities.shared_users.keys().collect() {
        return Err("shared process group inventory differs".into());
    }
    let mut inputs = BTreeMap::new();
    let mut owners = BTreeMap::new();
    for (name, group) in &scan.identities.shared_users {
        let members: BTreeMap<_, _> = scan
            .settings
            .packages
            .iter()
            .filter(|p| p.shared_user && p.app_id == group.app_id)
            .map(|p| {
                (
                    p.name.clone(),
                    scan.loaded
                        .get(&p.name)
                        .map(|p| p.package.processes.clone()),
                )
            })
            .collect();
        if members.keys().map(String::as_str).collect::<BTreeSet<_>>()
            != group.package_names().collect()
        {
            return Err(format!("shared process membership owner differs: {name}"));
        }
        let order = &orders[name];
        if order.len() != members.len()
            || order.iter().collect::<BTreeSet<_>>() != members.keys().collect()
        {
            return Err(format!("shared process member inventory differs: {name}"));
        }
        let mut owner = Processes::default();
        owner.rebuild(
            order
                .iter()
                .map(|name| members[name].as_ref().and_then(|p| p.as_deref())),
        )?;
        inputs.insert(name.clone(), (group.app_id, members));
        owners.insert(name.clone(), owner);
    }
    Ok(Assignments {
        orders,
        inputs,
        owners,
    })
}

impl SigningScan {
    /// Complete inputs in the operation's real iteration order. An empty list is
    /// valid only for an empty group; unparsed members remain explicit entries.
    pub fn complete_shared_processes(
        &mut self,
        orders: BTreeMap<String, Vec<String>>,
    ) -> Result<(), String> {
        let candidate = build(self, orders)?;
        self.shared_processes = Some(candidate);
        Ok(())
    }

    pub(in crate::package) fn validate_shared_processes(&self) -> Result<(), String> {
        let Some(assignment) = &self.shared_processes else {
            return Ok(());
        };
        let current = build(self, assignment.orders.clone())?;
        if current.inputs != assignment.inputs {
            return Err("shared process inputs changed".into());
        }
        Ok(())
    }

    pub fn shared_processes(&self, name: &str) -> Result<Option<&Processes>, String> {
        if !self.identities.shared_users.contains_key(name) {
            return Ok(None);
        }
        let owners = self
            .shared_processes
            .as_ref()
            .ok_or("missing shared process owner")?;
        self.validate_shared_processes()?;
        Ok(owners.owners.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, Settings, SharedUser};
    fn scan() -> SigningScan {
        SigningScan::new(
            &Default::default(),
            &Settings {
                shared_users: vec![SharedUser {
                    name: "fixture".into(),
                    app_id: 10100,
                    ..Default::default()
                }],
                packages: vec![
                    Package {
                        name: "a".into(),
                        app_id: 10100,
                        shared_user: true,
                        ..Default::default()
                    },
                    Package {
                        name: "b".into(),
                        app_id: 10100,
                        shared_user: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            36,
        )
        .unwrap()
    }
    fn orders(scan: &SigningScan) -> BTreeMap<String, Vec<String>> {
        scan.identities
            .shared_users
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    if name == "fixture" {
                        vec!["b".into(), "a".into()]
                    } else {
                        vec![]
                    },
                )
            })
            .collect()
    }
    fn load(scan: &mut SigningScan, name: &str, mode: i32) {
        scan.loaded.insert(
            name.into(),
            std::sync::Arc::new(super::super::LoadedPackage {
                package: crate::package::pkg::AndroidPackage {
                    package_name: name.into(),
                    uid: 10100,
                    processes: Some(vec![Process {
                        name: Some("worker".into()),
                        map_key: Some("worker".into()),
                        gwp_asan_mode: mode,
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
                collected_signing: crate::package::sign::SigningDetails::from_saved(
                    &Default::default(),
                )
                .unwrap(),
            }),
        );
    }
    #[test]
    fn supplied_iteration_controls_modes_and_code_changes_invalidate_capture() {
        let mut scan = scan();
        load(&mut scan, "a", 1);
        load(&mut scan, "b", 0);
        let order = orders(&scan);
        scan.complete_shared_processes(order.clone()).unwrap();
        let retained = scan.clone();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            1
        );
        std::sync::Arc::make_mut(scan.loaded.get_mut("a").unwrap())
            .package
            .processes
            .as_mut()
            .unwrap()[0]
            .gwp_asan_mode = -1;
        assert!(scan.shared_processes("fixture").is_err());
        assert!(scan.validate_shared_processes().is_err());
        scan.complete_shared_processes(order.clone()).unwrap();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            -1
        );
        assert_eq!(
            retained
                .shared_processes("fixture")
                .unwrap()
                .unwrap()
                .records()[0]
                .gwp_asan_mode,
            1
        );
        scan.loaded.remove("a");
        assert!(scan.shared_processes("fixture").is_err());
        scan.complete_shared_processes(order).unwrap();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            0
        );
    }
    #[test]
    fn inventories_are_complete_atomic_and_stale_members_reject() {
        let mut scan = scan();
        assert!(scan.shared_processes("fixture").is_err());
        assert!(scan.shared_processes("absent").unwrap().is_none());
        let inputs = orders(&scan);
        scan.complete_shared_processes(inputs.clone()).unwrap();
        let retained = scan.clone();
        assert!(
            scan.shared_processes("fixture")
                .unwrap()
                .unwrap()
                .records()
                .is_empty()
        );
        for members in [
            vec![],
            vec!["a".into()],
            vec!["a".into(), "a".into()],
            vec!["a".into(), "unknown".into()],
        ] {
            let mut bad = inputs.clone();
            bad.insert("fixture".into(), members);
            assert!(scan.complete_shared_processes(bad).is_err());
            assert_eq!(scan, retained);
        }
        let mut bad = inputs;
        bad.remove("fixture");
        assert!(scan.complete_shared_processes(bad).is_err());
        assert_eq!(scan, retained);
        let assignments = scan.shared_processes.clone();
        scan.identities
            .shared_users
            .get_mut("fixture")
            .unwrap()
            .add_package("foreign", 0, 0);
        assert!(scan.complete_shared_processes(orders(&retained)).is_err());
        assert_eq!(scan.shared_processes, assignments);
        assert!(scan.shared_processes("fixture").is_err());
        scan.identities
            .shared_users
            .get_mut("fixture")
            .unwrap()
            .remove_package("foreign");
        scan.settings.packages.remove(0);
        assert!(scan.shared_processes("fixture").is_err());
        assert!(
            retained
                .shared_processes("fixture")
                .unwrap()
                .unwrap()
                .records()
                .is_empty()
        );
    }
}
