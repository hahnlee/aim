//! Authoritative AppIdSettingMap selections of one retained scan capture.
use super::Snapshot;
use crate::package::owner::app_ids::Owner;
use aim_binder_host::parcel::Parcel;

pub(super) fn captured(snapshot: &Snapshot) -> Result<Vec<u8>, String> {
    let owner = snapshot.owner();
    if !owner.capture_ready() {
        return Err("UID slot scan is not finalized".into());
    }
    let slots: Vec<_> = owner.identities.ids.owners().collect();
    let mut out = Parcel::new();
    out.write_i64(snapshot.version() as i64);
    out.write_i32(i32::try_from(slots.len()).map_err(|_| "too many UID slots")?);
    for (id, slot) in slots {
        out.write_i32(id);
        match slot {
            Owner::Package(name) => {
                let setting = owner
                    .settings
                    .packages
                    .iter()
                    .find(|p| &p.name == name)
                    .ok_or("registered UID package is missing")?;
                if setting.app_id != id {
                    return Err("registered UID package identity differs".into());
                }
                out.write_i32(1);
                out.write_string16(Some(name));
            }
            Owner::SharedUser(name) => {
                let group = owner
                    .identities
                    .shared_users
                    .get(name)
                    .ok_or("registered UID group is missing")?;
                if group.app_id != id {
                    return Err("registered UID group identity differs".into());
                }
                out.write_i32(2);
                out.write_string16(Some(name));
            }
            Owner::DetachedPackage(name) => {
                let setting = owner
                    .identities
                    .ids
                    .detached_setting(id)
                    .ok_or("registered detached UID owner is missing")?;
                if &setting.package.name != name || setting.package.app_id != id {
                    return Err("registered detached UID identity differs".into());
                }
                let bytes = super::retained_record::detached(snapshot.version(), setting)?;
                out.write_i32(3);
                out.write_string16(Some(name));
                aim_service_aidl::write_byte_array(&mut out, Some(&bytes));
            }
        }
    }
    i32::try_from(out.data().len()).map_err(|_| "UID registry exceeds transport range")?;
    Ok(out.data().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        owner::{app_ids::DetachedSetting, usage::Usage},
        scan::SigningScan,
        settings::{Package, Settings, SharedUser},
    };
    use aim_binder_host::parcel::Reader;

    fn snapshot(owner: SigningScan) -> Snapshot {
        Snapshot {
            version: 7,
            owner,
            usage: Usage::new([]),
            replica_validated: false,
            metadata_revision: 1,
            lineage: std::sync::Arc::new(()),
        }
    }

    #[test]
    fn registry_selects_registered_slots_not_name_inventory() {
        let settings = Settings {
            packages: vec![Package {
                name: "pkg".into(),
                app_id: 10101,
                ..Default::default()
            }],
            shared_users: vec![SharedUser {
                name: "group".into(),
                app_id: 10102,
                ..Default::default()
            }],
            ..Default::default()
        };
        let owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        let bytes = captured(&snapshot(owner.clone())).unwrap();
        let mut reader = Reader::new(&bytes, &[]);
        assert_eq!(reader.read_i64().unwrap(), 7);
        let count = reader.read_i32().unwrap();
        assert_eq!(count as usize, owner.identities.ids.owners().count());
        let mut selected = Vec::new();
        for _ in 0..count {
            let id = reader.read_i32().unwrap();
            let kind = reader.read_i32().unwrap();
            let name = reader.read_string16().unwrap().unwrap();
            if id >= 10101 {
                selected.push((id, kind, name));
            }
        }
        assert_eq!(
            selected,
            vec![(10101, 1, "pkg".into()), (10102, 2, "group".into())]
        );
        assert_eq!(reader.remaining(), 0);
        let mut changed = owner;
        changed.settings.packages[0].app_id = 10103;
        assert!(captured(&snapshot(changed)).is_err());
        let mut retained = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        retained
            .identities
            .ids
            .detach(DetachedSetting {
                package: retained.settings.packages[0].clone(),
                users: Default::default(),
                user_aliases: Default::default(),
                legacy: None,
                install_fixed: None,
                runtime: None,
            })
            .unwrap();
        assert!(captured(&snapshot(retained))
            .unwrap_err()
            .contains("legacy"));
    }
}
