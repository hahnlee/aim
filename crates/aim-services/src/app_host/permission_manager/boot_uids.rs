//! PermissionService's SystemConfig fallback for UIDs no APK can acquire.
use super::*;
use crate::package::owner::app_ids::{AppIds, FIRST_APPLICATION_UID};
use crate::package::{settings::Settings, system_config::SystemConfig};

impl PermissionManager {
    /// Settings has restored all UID slots, including platform/OEM shared users.
    /// New package and shared-user allocations start at FIRST_APPLICATION_UID.
    /// An unoccupied lower UID therefore cannot gain a package during the scan.
    pub(crate) fn publish_boot_system_permissions(
        &self,
        config: &SystemConfig,
        ids: &AppIds,
        settings: &Settings,
        system_user_exists: bool,
    ) {
        let grants = config
            .system_permissions
            .iter()
            .filter(|(uid, _)| {
                system_user_exists
                    && (0..FIRST_APPLICATION_UID).contains(*uid)
                    && ids.get(**uid).is_none()
                    && !settings
                        .disabled_system_packages
                        .iter()
                        .any(|p| p.uid_owner_id() == **uid)
            })
            .map(|(uid, grants)| (*uid, grants.clone()))
            .collect();
        let _ = self
            .host
            .package_boot
            .system_permissions
            .set(Ok(Arc::new(grants)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::app_ids::Owner;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn component_permission_shortcuts_need_no_package_state() {
        let host = AppHost::new(crate::test_process(1000));
        let owner = PermissionManager::new(host.clone());
        for uid in [0, 1000, 100000, 101000] {
            assert!(owner.holds(uid, "undefined.permission").unwrap());
        }
        for uid in [99000, 199000] {
            assert!(
                !owner
                    .holds(uid, "android.permission.PACKAGE_USAGE_STATS")
                    .unwrap()
            );
        }
        assert!(host.package_boot.scan.get().is_none());
        host.process.shutdown().unwrap();
    }

    #[test]
    fn native_uid_registration_checks_finish_before_the_apk_scan() {
        crate::system::with_late_package_construction(|host, _| {
            let owner = Arc::new(PermissionManager::with_system(
                host.clone(),
                AccessSystem::default(),
            ));
            owner.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser::default()));
            assert!(host.permission_manager.set(owner.clone()).is_ok());
            let mut config = SystemConfig::default();
            config.system_permissions.insert(
                1041,
                BTreeSet::from([
                    "android.permission.PACKAGE_USAGE_STATS".into(),
                    "android.permission.OBSERVE_SENSOR_PRIVACY".into(),
                    "android.permission.ACCESS_FINE_LOCATION".into(),
                ]),
            );
            owner.publish_boot_system_permissions(
                &config,
                &AppIds::default(),
                &Settings::default(),
                true,
            );
            let (sent, received) = mpsc::channel();
            let mut early = None;
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let checks = (|| {
                        // AMS.hasUsageStatsPermission notes the op as system before
                        // checking PACKAGE_USAGE_STATS; sensor privacy checks follow.
                        let mode = super::super::super::appops_service::note(
                            host,
                            43,
                            1041,
                            Some("audioserver"),
                        )?;
                        Ok::<_, Exception>((
                            mode,
                            owner.holds(1041, "android.permission.PACKAGE_USAGE_STATS")?,
                            owner.holds(1041, "android.permission.OBSERVE_SENSOR_PRIVACY")?,
                            owner.holds(1041, "android.permission.ACCESS_COARSE_LOCATION")?,
                            owner.holds(1041, "undefined.permission")?,
                        ))
                    })();
                    sent.send(checks).unwrap();
                });
                early = Some(received.recv_timeout(Duration::from_secs(2)));
                host.package_boot
                    .scan
                    .set(Err(Exception::new(EX_ILLEGAL_STATE, "scan released")))
                    .unwrap();
            });
            assert_eq!(
                early.unwrap().unwrap().unwrap(),
                (3, true, true, true, false)
            );
            assert!(host.package_boot.runtime.get().is_none());
            // Once a scan failure is known, new calls retain its error.
            assert!(
                owner
                    .holds(1041, "android.permission.PACKAGE_USAGE_STATS")
                    .is_err()
            );
        });
    }

    #[test]
    fn package_and_shared_slots_and_factory_settings_keep_the_scan_gate() {
        let host = AppHost::new(crate::test_process(1000));
        let owner = PermissionManager::new(host.clone());
        let mut config = SystemConfig::default();
        for uid in [1001, 1041, 1047, 2900, 10000] {
            config
                .system_permissions
                .insert(uid, BTreeSet::from(["permission".into()]));
        }
        config
            .oem_defined_uids
            .push(("android.uid.vendor".into(), 2900));
        let mut ids = crate::package::owner::shared_users::Bootstrap::new(&config).ids;
        ids.register_existing(1041, Owner::Package("saved".into()))
            .unwrap();
        let xml = aim_android_xml::read(b"<packages><updated-package name='factory' codePath='/system/app/f' userId='1047'/></packages>").unwrap();
        let settings = Settings::parse_with_config(&xml, &config).unwrap();
        owner.publish_boot_system_permissions(&config, &ids, &settings, true);
        assert!(
            host.package_boot
                .system_permissions
                .wait()
                .as_ref()
                .unwrap()
                .is_empty()
        );
        host.process.shutdown().unwrap();
    }

    #[test]
    fn an_absent_system_user_has_no_early_grants() {
        let host = AppHost::new(crate::test_process(1000));
        let owner = PermissionManager::new(host.clone());
        let mut config = SystemConfig::default();
        config
            .system_permissions
            .insert(1041, BTreeSet::from(["permission".into()]));
        owner.publish_boot_system_permissions(
            &config,
            &AppIds::default(),
            &Settings::default(),
            false,
        );
        assert!(
            host.package_boot
                .system_permissions
                .wait()
                .as_ref()
                .unwrap()
                .is_empty()
        );
        host.process.shutdown().unwrap();
    }
}
