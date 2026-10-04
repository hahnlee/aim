//! Captured shared process owners require the actual ordered member inventory (#870).
use super::SigningScan;
use crate::package::owner::shared_processes::Processes;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct OriginalSharedProcesses {
    pub name: String,
    pub app_id: i32,
    pub members: Vec<String>,
    pub records: Vec<crate::package::pkg::Process>,
}
impl OriginalSharedProcesses {
    pub fn read_original_record(bytes: &[u8]) -> aim_binder_host::parcel::Result<Self> {
        use aim_binder_host::parcel::{BAD_VALUE, Reader};
        let mut r = Reader::new(bytes, &[]);
        fn required(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<String> {
            r.read_string16()?.ok_or(BAD_VALUE)
        }
        fn count(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<usize> {
            let n = r.read_i32()?;
            if n < 0 || n as usize > r.remaining() / 4 {
                Err(BAD_VALUE)
            } else {
                Ok(n as usize)
            }
        }
        let name = required(&mut r)?;
        let app_id = r.read_i32()?;
        let mut members = Vec::new();
        for _ in 0..count(&mut r)? {
            members.push(required(&mut r)?);
        }
        let mut records = Vec::new();
        for _ in 0..count(&mut r)? {
            let map_key = Some(required(&mut r)?);
            let name = Some(required(&mut r)?);
            let mut classes = Vec::new();
            for _ in 0..count(&mut r)? {
                classes.push((required(&mut r)?, r.read_string16()?));
            }
            let mut denied = Vec::new();
            for _ in 0..count(&mut r)? {
                denied.push(required(&mut r)?);
            }
            let gwp_asan_mode = r.read_i32()?;
            let memtag_mode = r.read_i32()?;
            let native_heap_zero_initialized = r.read_i32()?;
            let use_embedded_dex = match r.read_i32()? {
                0 => false,
                1 => true,
                _ => return Err(BAD_VALUE),
            };
            records.push(crate::package::pkg::Process {
                map_key,
                name,
                app_class_names_by_package: classes,
                denied_permissions: denied,
                gwp_asan_mode,
                memtag_mode,
                native_heap_zero_initialized,
                use_embedded_dex,
            });
        }
        if r.remaining() != 0
            || bytes.len() % 4 != 0
            || members.iter().collect::<BTreeSet<_>>().len() != members.len()
            || Processes::capture_original(records.clone()).is_err()
        {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            name,
            app_id,
            members,
            records,
        })
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Member {
    path: String,
    version: i64,
    // None is unparsed; parsed code separately retains a nullable process map.
    code: Option<std::sync::Arc<super::LoadedPackage>>,
}
type Members = BTreeMap<String, Member>;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    orders: BTreeMap<String, Vec<String>>,
    inputs: BTreeMap<String, (i32, Members)>,
    owners: BTreeMap<String, Processes>,
}

fn build(
    scan: &SigningScan,
    orders: BTreeMap<String, Vec<String>>,
    mut imported: Option<BTreeMap<String, Processes>>,
) -> Result<Assignments, String> {
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
                    Member {
                        path: p.code_path.clone(),
                        version: p.version_code,
                        code: scan.loaded.get(&p.name).cloned(),
                    },
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
        let owner = match &mut imported {
            Some(values) => values
                .remove(name)
                .ok_or("missing original shared process aggregate")?,
            None => {
                let mut value = Processes::default();
                value.rebuild(order.iter().map(|name| {
                    members[name]
                        .code
                        .as_ref()
                        .and_then(|p| p.package.processes.as_deref())
                }))?;
                value
            }
        };
        inputs.insert(name.clone(), (group.app_id, members));
        owners.insert(name.clone(), owner);
    }
    if imported.as_ref().is_some_and(|values| !values.is_empty()) {
        return Err("foreign original shared process aggregate".into());
    }
    Ok(Assignments {
        orders,
        inputs,
        owners,
    })
}

impl SigningScan {
    /// PackageFeed keeps SharedUserApi's forward ArraySet order. The original
    /// updateProcesses rebuild visits that same set in reverse (android-16 r1).
    pub fn rebuild_shared_processes_from_original_members(
        &mut self,
        source: &crate::package::model::State,
    ) -> Result<(), String> {
        let orders = self.original_member_orders(source)?;
        self.complete_shared_processes(orders)
    }

    pub fn capture_original_shared_processes(
        &mut self,
        source: &crate::package::model::State,
    ) -> Result<(), String> {
        let orders = self.original_member_orders(source)?;
        if source
            .shared_process_inputs
            .keys()
            .ne(source.shared_users.keys())
        {
            return Err("original shared process aggregate inventory differs".into());
        }
        let mut owners = BTreeMap::new();
        for (name, input) in &source.shared_process_inputs {
            let group = &source.shared_users[name];
            if input.name != *name
                || input.app_id != group.app_id
                || input.members != group.packages
            {
                return Err(format!(
                    "original shared process aggregate identity differs: {name}"
                ));
            }
            owners.insert(
                name.clone(),
                Processes::capture_original(input.records.clone())?,
            );
        }
        self.shared_processes = Some(build(self, orders, Some(owners))?);
        Ok(())
    }

    fn original_member_orders(
        &self,
        source: &crate::package::model::State,
    ) -> Result<BTreeMap<String, Vec<String>>, String> {
        if source
            .shared_users
            .keys()
            .ne(self.identities.shared_users.keys())
        {
            return Err("original shared process group inventory differs".into());
        }
        let mut orders = BTreeMap::new();
        for (name, group) in &self.identities.shared_users {
            let original = &source.shared_users[name];
            if original.name != *name || original.app_id != group.app_id {
                return Err(format!(
                    "original shared process group identity differs: {name}"
                ));
            }
            let source_members: BTreeSet<_> = source
                .packages
                .values()
                .filter(|p| p.shared_user.as_deref() == Some(name))
                .map(|p| &p.name)
                .collect();
            if original.packages.iter().collect::<BTreeSet<_>>() != source_members {
                return Err(format!(
                    "original shared process membership differs: {name}"
                ));
            }
            for member in &original.packages {
                let setting = self
                    .settings
                    .packages
                    .iter()
                    .find(|p| &p.name == member)
                    .ok_or_else(|| {
                        format!("original shared process member is unknown: {member}")
                    })?;
                let input = source.packages.get(member).ok_or_else(|| {
                    format!("original shared process member is missing: {member}")
                })?;
                if !setting.shared_user
                    || setting.app_id != group.app_id
                    || input.name != *member
                    || input.app_id != setting.app_id
                    || input.path != setting.code_path
                    || input.version_code != setting.version_code
                {
                    return Err(format!(
                        "original shared process setting identity differs: {member}"
                    ));
                }
                match (self.loaded.get(member), input.pkg.as_deref()) {
                    (Some(code), Some(original))
                        if code.package.processes == original.processes
                            && code.package.package_name == original.package_name
                            && code.package.uid == original.uid
                            && code.package.path == original.path
                            && code.package.version_code == original.version_code
                            && code.package.version_code_major == original.version_code_major => {}
                    (None, None) if input.parcel.is_none() => {}
                    _ => return Err(format!("original shared process code differs: {member}")),
                }
            }
            orders.insert(
                name.clone(),
                original.packages.iter().rev().cloned().collect(),
            );
        }
        Ok(orders)
    }

    /// Complete inputs in the operation's real iteration order. An empty list is
    /// valid only for an empty group; unparsed members remain explicit entries.
    pub fn complete_shared_processes(
        &mut self,
        orders: BTreeMap<String, Vec<String>>,
    ) -> Result<(), String> {
        let candidate = build(self, orders, None)?;
        self.shared_processes = Some(candidate);
        Ok(())
    }

    pub(in crate::package) fn validate_shared_processes(&self) -> Result<(), String> {
        let Some(assignment) = &self.shared_processes else {
            return Ok(());
        };
        let current = build(
            self,
            assignment.orders.clone(),
            Some(assignment.owners.clone()),
        )?;
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
    use crate::package::pkg::Process;
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
    fn original_source(scan: &SigningScan) -> crate::package::model::State {
        let mut source = crate::package::model::State::default();
        for (name, group) in &scan.identities.shared_users {
            source.shared_users.insert(
                name.clone(),
                crate::package::model::SharedUser {
                    name: name.clone(),
                    app_id: group.app_id,
                    packages: if name == "fixture" {
                        vec!["a".into(), "b".into()]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
            );
        }
        for setting in &scan.settings.packages {
            source.packages.insert(
                setting.name.clone(),
                crate::package::model::PackageState {
                    name: setting.name.clone(),
                    app_id: setting.app_id,
                    path: setting.code_path.clone(),
                    version_code: setting.version_code,
                    shared_user: Some("fixture".into()),
                    pkg: scan
                        .loaded
                        .get(&setting.name)
                        .map(|code| std::sync::Arc::new(code.package.clone())),
                    ..Default::default()
                },
            );
        }
        source
    }
    #[test]
    fn original_incremental_aggregate_is_imported_without_rebuild() {
        let mut scan = scan();
        load(&mut scan, "a", 1);
        load(&mut scan, "b", 0);
        let mut source = original_source(&scan);
        let mut aggregate = Processes::default();
        aggregate
            .add(scan.loaded["a"].package.processes.as_deref())
            .unwrap();
        aggregate
            .add(scan.loaded["b"].package.processes.as_deref())
            .unwrap();
        for (name, group) in &source.shared_users {
            source.shared_process_inputs.insert(
                name.clone(),
                OriginalSharedProcesses {
                    name: name.clone(),
                    app_id: group.app_id,
                    members: group.packages.clone(),
                    records: if name == "fixture" {
                        aggregate.records().to_vec()
                    } else {
                        vec![]
                    },
                },
            );
        }
        scan.capture_original_shared_processes(&source).unwrap();
        assert_eq!(scan.shared_processes("fixture").unwrap(), Some(&aggregate));
        assert_eq!(aggregate.records()[0].gwp_asan_mode, 0);
        let retained = scan.clone();
        let mut rebuilt = scan.clone();
        rebuilt
            .rebuild_shared_processes_from_original_members(&source)
            .unwrap();
        assert_eq!(
            rebuilt
                .shared_processes("fixture")
                .unwrap()
                .unwrap()
                .records()[0]
                .gwp_asan_mode,
            1
        );
        for mode in 0..5 {
            let mut bad = source.clone();
            match mode {
                0 => {
                    bad.shared_process_inputs.remove("fixture");
                }
                1 => bad.shared_process_inputs.get_mut("fixture").unwrap().app_id += 1,
                2 => bad
                    .shared_process_inputs
                    .get_mut("fixture")
                    .unwrap()
                    .members
                    .reverse(),
                3 => {
                    bad.shared_process_inputs
                        .get_mut("fixture")
                        .unwrap()
                        .records[0]
                        .map_key = Some("foreign".into())
                }
                _ => {
                    bad.shared_process_inputs
                        .get_mut("fixture")
                        .unwrap()
                        .records[0]
                        .denied_permissions = vec!["duplicate".into(), "duplicate".into()]
                }
            }
            assert!(scan.capture_original_shared_processes(&bad).is_err());
            assert_eq!(scan, retained);
        }
    }
    #[test]
    fn original_aggregate_decoder_rejects_malformed_frames() {
        use aim_binder_host::parcel::Parcel;
        let mut p = Parcel::new();
        p.write_string16(Some("group"));
        p.write_i32(10100);
        p.write_i32(1);
        p.write_string16(Some("a"));
        p.write_i32(1);
        p.write_string16(Some("worker"));
        p.write_string16(Some("worker"));
        p.write_i32(1);
        p.write_string16(Some("a"));
        p.write_string16(None);
        p.write_i32(1);
        p.write_string16(Some("denied"));
        p.write_i32(1);
        p.write_i32(2);
        p.write_i32(3);
        p.write_bool(true);
        let decoded = OriginalSharedProcesses::read_original_record(p.data()).unwrap();
        assert_eq!(
            decoded.records[0].app_class_names_by_package,
            [("a".into(), None)]
        );
        assert_eq!(decoded.records[0].denied_permissions, ["denied"]);
        let mut tail = p.data().to_vec();
        tail.extend_from_slice(&0i32.to_le_bytes());
        assert!(OriginalSharedProcesses::read_original_record(&tail).is_err());
        assert!(
            OriginalSharedProcesses::read_original_record(&p.data()[..p.data().len() - 1]).is_err()
        );
        let mut invalid = p.data().to_vec();
        let end = invalid.len();
        invalid[end - 4..].copy_from_slice(&2i32.to_le_bytes());
        assert!(OriginalSharedProcesses::read_original_record(&invalid).is_err());
        let mut missing = Parcel::new();
        missing.write_string16(None);
        missing.write_i32(0);
        assert!(OriginalSharedProcesses::read_original_record(missing.data()).is_err());
    }
    #[test]
    fn original_forward_member_order_drives_rebuild_and_rejects_foreign_sources() {
        let mut scan = scan();
        load(&mut scan, "a", 1);
        load(&mut scan, "b", 0);
        let source = original_source(&scan);
        scan.rebuild_shared_processes_from_original_members(&source)
            .unwrap();
        let retained = scan.clone();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            1
        );
        let mut moved = scan.clone();
        moved.settings.packages[0].code_path.push_str("/changed");
        assert!(moved.shared_processes("fixture").is_err());
        let mut upgraded = scan.clone();
        upgraded.settings.packages[0].version_code += 1;
        assert!(upgraded.shared_processes("fixture").is_err());
        let mut reversed = source.clone();
        reversed
            .shared_users
            .get_mut("fixture")
            .unwrap()
            .packages
            .reverse();
        scan.rebuild_shared_processes_from_original_members(&reversed)
            .unwrap();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            0
        );
        scan = retained.clone();
        for mode in 0..10 {
            let mut invalid = source.clone();
            match mode {
                0 => {
                    invalid.shared_users.remove("fixture");
                }
                1 => invalid.shared_users.get_mut("fixture").unwrap().app_id += 1,
                2 => invalid.shared_users.get_mut("fixture").unwrap().name = "foreign".into(),
                3 => invalid
                    .shared_users
                    .get_mut("fixture")
                    .unwrap()
                    .packages
                    .push("a".into()),
                4 => {
                    invalid.packages.remove("a");
                }
                5 => invalid.packages.get_mut("a").unwrap().path = "other".into(),
                6 => invalid.packages.get_mut("a").unwrap().version_code += 1,
                7 => invalid.packages.get_mut("a").unwrap().shared_user = None,
                8 => invalid.packages.get_mut("a").unwrap().pkg = None,
                _ => {
                    std::sync::Arc::make_mut(
                        invalid.packages.get_mut("a").unwrap().pkg.as_mut().unwrap(),
                    )
                    .processes
                    .as_mut()
                    .unwrap()[0]
                        .gwp_asan_mode = 2
                }
            }
            assert!(
                scan.rebuild_shared_processes_from_original_members(&invalid)
                    .is_err()
            );
            assert_eq!(scan, retained);
        }
        scan.loaded.remove("a");
        let unloaded = original_source(&scan);
        scan.rebuild_shared_processes_from_original_members(&unloaded)
            .unwrap();
        assert_eq!(
            scan.shared_processes("fixture").unwrap().unwrap().records()[0].gwp_asan_mode,
            0
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
