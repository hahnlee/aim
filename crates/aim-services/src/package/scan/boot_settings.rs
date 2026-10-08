//! PackageManagerService constructor VersionInfo completion and pre-Q migration.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::SigningScan;
use crate::package::settings::Version;

const APP_DETAILS_ACTIVITY: &str = "android.app.AppDetailsActivity";
impl SigningScan {
    pub(crate) fn finish_boot_settings(&mut self, current: &Version, lifecycle: &crate::package::lifecycle::Owner) -> Result<(), String> {
        if current.volume_uuid.is_some() { return Err("boot current version is not internal storage".into()); }
        self.settings.versions.iter().find(|version|version.volume_uuid.is_none())
            .ok_or("boot internal VersionInfo owner absent")?;
        let upgrading = lifecycle.partition_upgrading();
        if lifecycle.is_upgrading_from_lower_than(29) {
            let names = self.settings.packages.iter().filter(|setting|setting.flags & 1 == 0)
                .map(|setting|setting.name.clone()).collect::<Vec<_>>();
            for name in names {
                let users = self.scanned_users.get(&name).ok_or("pre-Q package user owner absent")?;
                let mut state = users.get(&0).cloned().unwrap_or_default();
                if let Some(enabled) = &mut state.enabled_components {
                    enabled.retain(|component|component != APP_DETAILS_ACTIVITY);
                }
                let disabled = state.disabled_components.get_or_insert_with(Vec::new);
                if !disabled.iter().any(|component|component == APP_DETAILS_ACTIVITY) {
                    disabled.push(APP_DETAILS_ACTIVITY.into());
                }
                self.set_user_state(&name,0,state)?;
            }
        }
        let version = self.settings.find_or_create_version(None);
        version.sdk_version = current.sdk_version;
        version.database_version = current.database_version;
        if upgrading {
            version.build_fingerprint = current.build_fingerprint.clone();
            version.fingerprint = current.fingerprint.clone();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{owner::{Store,usage::Usage},restrictions::UserState,
        scan::CapturedUsers,scan_snapshot};
    use std::collections::BTreeMap;

    #[test]
    fn completed_boot_versions_and_pre_q_component_migration_persist_then_do_not_repeat() {
        let data=std::env::temp_dir().join(format!("aim-boot-version-{}",std::process::id()));
        std::fs::create_dir_all(data.join("system/users/0")).unwrap();
        std::fs::write(data.join("system/packages.xml"),b"<packages><version sdkVersion='0' databaseVersion='0'/><version volumeUuid='primary_physical' sdkVersion='35' databaseVersion='2' fingerprint='external'/><package name='fixture.data' codePath='/data/app/fixture' userId='10100' version='1' publicFlags='0' domainSetId='00000000-0000-0000-0000-000000000001'/><package name='fixture.system' codePath='/system/app/fixture' userId='10101' version='1' publicFlags='1' domainSetId='00000000-0000-0000-0000-000000000002'/></packages>").unwrap();
        std::fs::write(data.join("system/users/0/package-restrictions.xml"),b"<package-restrictions/>").unwrap();
        let mut disk=Store::open(&data,&[0]).unwrap().unwrap();
        let mut scan=SigningScan::new(&Default::default(),&disk.state().settings,36).unwrap();
        let state=UserState{enabled_components:Some(vec![APP_DETAILS_ACTIVITY.into(),"fixture.UserEnabled".into()]),disabled_components:Some(vec!["fixture.UserDisabled".into()]),..Default::default()};
        scan.capture_user_states(["fixture.data","fixture.system"].into_iter().map(|name|((name.into(),false),CapturedUsers{states:BTreeMap::from([(0,state.clone()),(10,state.clone())]),active_aliases:Default::default()})).collect()).unwrap();
        let current=Version{sdk_version:36,database_version:3,build_fingerprint:Some("actual-build".into()),fingerprint:Some("actual-partitions".into()),..Default::default()};
        let lifecycle=crate::package::lifecycle::Owner::from_settings(&disk.state().settings,true,"actual-partitions",Box::new(|_|None),Box::new(|_|Err("CE owner not used in this fixture".into()))).unwrap();
        scan.finish_boot_settings(&current,&lifecycle).unwrap();
        assert!(lifecycle.is_upgrading_from_lower_than(29));assert_eq!(lifecycle.prior_sdk_version(),0);

        let migrated=&scan.scanned_users["fixture.data"][&0];
        assert_eq!(migrated.enabled_components.as_ref().unwrap(),&["fixture.UserEnabled"]);
        assert_eq!(migrated.disabled_components.as_ref().unwrap(),&["fixture.UserDisabled",APP_DETAILS_ACTIVITY]);
        assert_eq!(scan.scanned_users["fixture.data"][&10],state);
        assert_eq!(scan.scanned_users["fixture.system"][&0],state);
        let snapshot=scan_snapshot::Store::new(scan,Usage::new(["fixture.data","fixture.system"])).unwrap().capture();
        disk.commit_completed_boot_scan(&snapshot,false).unwrap();drop(disk);
        let disk=Store::open(&data,&[0]).unwrap().unwrap();
        let internal=disk.state().settings.versions.iter().find(|version|version.volume_uuid.is_none()).unwrap();
        assert_eq!(internal,&current);
        let external=disk.state().settings.versions.iter().find(|version|version.volume_uuid.is_some()).unwrap();
        assert_eq!((external.sdk_version,external.database_version,external.fingerprint.as_deref()),(35,2,Some("external")));
        let users=&disk.state().users[0].1.restrictions.packages;
        assert_eq!(users.iter().find(|(name,_)|name=="fixture.data").unwrap().1.disabled_components.as_ref().unwrap(),&["fixture.UserDisabled",APP_DETAILS_ACTIVITY]);
        let mut current_scan=SigningScan::new(&Default::default(),&disk.state().settings,36).unwrap();
        current_scan.capture_user_states(["fixture.data","fixture.system"].into_iter().map(|name|((name.into(),false),CapturedUsers{states:BTreeMap::from([(0,state.clone())]),active_aliases:Default::default()})).collect()).unwrap();
        let current_lifecycle=crate::package::lifecycle::Owner::from_settings(&disk.state().settings,true,"actual-partitions",Box::new(|_|None),Box::new(|_|Err("CE owner not used in this fixture".into()))).unwrap();
        assert!(!current_lifecycle.device_upgrading());
        let build_only=Version{build_fingerprint:Some("different-build-same-partitions".into()),..current.clone()};
        current_scan.finish_boot_settings(&build_only,&current_lifecycle).unwrap();
        assert_eq!(current_scan.settings.versions.iter().find(|version|version.volume_uuid.is_none()).unwrap(),&current,
            "a build-only difference is not the constructor partition upgrade predicate");
        assert_eq!(current_scan.scanned_users["fixture.data"][&0],state,"current-version rollback must not fabricate a legacy disable override");
        drop(disk);std::fs::remove_dir_all(data).unwrap();
    }
}
