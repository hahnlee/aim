//! Settings.removePackageAndAppIdLPw after package-state side owners complete.
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.
use super::{Error, SigningError, SigningScan};
use crate::package::{owner::app_ids::Owner, settings::Package};

#[derive(Debug, PartialEq)]
pub struct RemovedSetting {
    pub package: Package,
    pub app_id_removed: bool,
}

impl SigningScan {
    /// Remove the saved setting and its UID membership. The caller must first
    /// complete data/domain/keyset/filter/preferred cleanup and withdraw loaded
    /// code. Permission uninstall reconciliation follows this step (#822/#798).
    /// No persistence, keystore work or query publication is implicit here.
    pub fn remove_package_setting(
        &mut self,
        name: &str,
    ) -> Result<Option<RemovedSetting>, SigningError> {
        let Some(at) = self.settings.packages.iter().position(|p| p.name == name) else {
            return Ok(None);
        };
        let package = self.settings.packages[at].clone();
        let fail = |message: &str| {
            SigningError::Fatal(Error {
                package: name.into(),
                path: package.code_path.clone(),
                phase: "package-setting",
                message: message.into(),
            })
        };
        if self.has_scanned_package(name) {
            return Err(fail(
                "loaded package must be withdrawn before setting removal",
            ));
        }
        let shared = match self.identities.ids.get(package.app_id) {
            Some(Owner::SharedUser(group_name)) if package.shared_user => {
                let group = self
                    .identities
                    .shared_users
                    .get(group_name)
                    .ok_or_else(|| fail("shared UID owner is missing"))?;
                if !group.has_package(name) {
                    return Err(fail("shared UID membership is missing"));
                }
                Some(group_name.clone())
            }
            Some(Owner::Package(owner)) if !package.shared_user && owner == name => None,
            _ => return Err(fail("package UID slot disagrees with setting")),
        };
        self.settings.packages.remove(at);
        self.installers.remove(name, &mut self.settings);
        let app_id_removed = if let Some(group_name) = shared {
            self.identities
                .shared_users
                .get_mut(&group_name)
                .unwrap()
                .remove_package(name);
            let used = self
                .settings
                .packages
                .iter()
                .chain(&self.settings.disabled_system_packages)
                .any(|p| p.shared_user && p.app_id == package.app_id);
            if !used {
                self.identities.shared_users.remove(&group_name);
                self.settings.shared_users.retain(|g| g.name != group_name);
                self.identities.ids.remove(package.app_id);
            }
            !used
        } else {
            self.identities.ids.remove(package.app_id);
            true
        };
        self.scanned_users.remove(name);
        Ok(Some(RemovedSetting {
            package,
            app_id_removed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(xml: &[u8]) -> SigningScan {
        let root = aim_android_xml::read(xml).unwrap();
        let settings = crate::package::settings::Settings::parse(&root).unwrap();
        SigningScan::new(&Default::default(), &settings, 36).unwrap()
    }

    #[test]
    fn setting_removal_keeps_shared_members_and_disabled_factory_uid_reservations() {
        for disabled in [false, true] {
            let xml = format!(
                "<packages><shared-user name='group' userId='10100'/><package name='a' codePath='/data/app/a' sharedUserId='10100'/><package name='b' codePath='/data/app/b' sharedUserId='10100'/>{}</packages>",
                if disabled {
                    "<updated-package name='a' codePath='/system/app/a' sharedUserId='10100'/>"
                } else {
                    ""
                }
            );
            let mut scan = owner(xml.as_bytes());
            assert!(
                !scan
                    .remove_package_setting("a")
                    .unwrap()
                    .unwrap()
                    .app_id_removed
            );
            assert!(scan.identities.shared_users["group"].has_package("b"));
            assert!(!scan.identities.shared_users["group"].has_package("a"));
            assert_eq!(
                scan.remove_package_setting("b")
                    .unwrap()
                    .unwrap()
                    .app_id_removed,
                !disabled
            );
            assert_eq!(scan.identities.ids.get(10100).is_some(), disabled);
            assert_eq!(
                scan.settings.shared_users.iter().any(|g| g.name == "group"),
                disabled
            );
            assert!(scan.settings.packages.is_empty());
            assert_eq!(
                scan.settings.disabled_system_packages.len(),
                usize::from(disabled)
            );
            let saved = scan.clone();
            assert_eq!(scan.remove_package_setting("missing").unwrap(), None);
            assert_eq!(scan, saved);
        }
    }

    #[test]
    fn setting_removal_releases_only_its_uid_and_rejects_conflicting_ownership() {
        let mut scan = owner(b"<packages><package name='a' codePath='/data/app/a' userId='10100'/><package name='b' codePath='/data/app/b' userId='10101'/></packages>");
        scan.identities
            .ids
            .replace(10100, Owner::Package("wrong".into()))
            .unwrap();
        let old = scan.clone();
        assert!(scan.remove_package_setting("a").is_err());
        assert_eq!(scan, old);
        scan.identities
            .ids
            .replace(10100, Owner::Package("a".into()))
            .unwrap();
        assert!(
            scan.remove_package_setting("a")
                .unwrap()
                .unwrap()
                .app_id_removed
        );
        assert_eq!(scan.identities.ids.get(10100), None);
        assert_eq!(
            scan.identities.ids.get(10101),
            Some(&Owner::Package("b".into()))
        );
        assert_eq!(scan.settings.packages[0].name, "b");
    }

    #[test]
    fn installer_removal_retains_initiator_identity_and_skips_disabled_factories() {
        let mut scan = owner(b"<packages><package name='store' codePath='/data/app/store' userId='10100'/><package name='app' codePath='/data/app/app' userId='10101' installer='store' installerUid='10100' updateOwner='store' installInitiator='store' installOriginator='store' installerAttributionTag='tag' packageSource='2'/><updated-package name='app' codePath='/system/app/app' userId='10101'/></packages>");
        let original = scan.settings.packages[1].install_source.clone();
        scan.settings.disabled_system_packages[0].install_source = original.clone();
        scan.remove_package_setting("store").unwrap();
        let source = &scan.settings.packages[0].install_source;
        assert_eq!(source.initiating_package.as_deref(), Some("store"));
        assert!(source.initiating_package_uninstalled);
        assert_eq!(
            source.initiating_package_signatures,
            original.initiating_package_signatures
        );
        assert!(
            source.installer.is_none()
                && source.originating_package.is_none()
                && source.update_owner.is_none()
        );
        assert_eq!(source.installer_uid, -1);
        assert!(source.is_orphaned);
        assert!(source.installer_attribution_tag.is_none());
        assert_eq!(source.package_source, original.package_source);
        assert_eq!(
            scan.settings.disabled_system_packages[0].install_source,
            original
        );
    }
}
