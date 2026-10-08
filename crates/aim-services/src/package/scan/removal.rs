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
    /// A retained factory remains verified, but active declarations must be
    /// withdrawn before its normal system-path scan can re-admit the code.
    pub fn withdraw_for_live_factory(&mut self,name:&str)->Result<(),String> {
        let setting=self.settings.packages.iter().find(|package|package.name==name).cloned().ok_or("factory restoration active setting absent")?;
        if !self.settings.disabled_system_packages.iter().any(|package|package.name==name) {return Err("factory restoration disabled setting absent".into());}
        let code=self.loaded.get(name).cloned().ok_or("factory restoration active code absent")?;
        let record=super::Record{settings:setting.clone(),parsed:code.package.clone(),signing:code.collected_signing.clone(),
            identity:super::Identity{manifest_name:code.package.package_name.clone(),internal_name:name.into(),real_name:setting.real_name.clone()},
            origin:crate::package::owner::shared_users::ScanOrigin::Data};
        self.withdraw_scanned_package(&record);
        Ok(())
    }
    /// KEEP_DATA retires executable declarations while retaining the setting,
    /// UID/signers and exact user state for a later reinstall or unarchival.
    pub fn prepare_live_code_retirement(&self, name: &str) -> Result<Self, String> {
        let mut next = self.clone();
        let saved = next.settings.packages.iter().find(|package| package.name == name)
            .cloned().ok_or("code retirement setting unavailable")?;
        if saved.flags & crate::package::settings::FLAG_SYSTEM != 0 {
            return Err("system code retirement requires factory restoration".into());
        }
        let users = next.scanned_users.get(name).cloned().ok_or("code retirement user owner unavailable")?;
        if users.values().any(|state| state.installed) {
            return Err("code retirement has installed users".into());
        }
        if let Some(code) = next.loaded.get(name).cloned() {
            let record = super::Record {
                settings: saved.clone(), parsed: code.package.clone(), signing: code.collected_signing.clone(),
                identity: super::Identity { manifest_name: code.package.package_name.clone(), internal_name: name.into(), real_name: saved.real_name.clone() },
                origin: crate::package::owner::shared_users::ScanOrigin::Data,
            };
            next.withdraw_scanned_package(&record);
        }
        next.scanned_users.insert(name.into(), users.clone());
        next.update_disabled_user_aliases(name, &users);
        next.retain_seinfo_after_code_removal()?;
        next.complete_retained_library_dependencies()?;
        if next.has_shared_processes() { next.rebuild_shared_processes_from_native_members()?; }
        next.rebind_retired_code_runtime(name)?;
        Ok(next)
    }
    /// A live removal withdraws declarations before releasing its setting/UID.
    /// The installation owner has already completed data/permission/filter
    /// cleanup. Updated-system restoration is a separate scan admission.
    pub fn prepare_live_package_removal(&self, name: &str) -> Result<Self, String> {
        let mut next = self.clone();
        let saved = next.settings.packages.iter().find(|package| package.name == name)
            .cloned().ok_or("live removal package setting unavailable")?;
        if saved.flags & crate::package::settings::FLAG_SYSTEM != 0 {
            return Err("system removal requires a factory restoration admission".into());
        }
        if let Some(users) = next.scanned_users.get(name) {
            if users.values().any(|user| user.installed) {
                return Err("live setting removal still has installed users".into());
            }
        }
        if let Some(code) = next.loaded.get(name).cloned() {
            let record = super::Record {
                settings: saved.clone(), parsed: code.package.clone(), signing: code.collected_signing.clone(),
                identity: super::Identity { manifest_name: code.package.package_name.clone(), internal_name: name.into(), real_name: saved.real_name.clone() },
                origin: crate::package::owner::shared_users::ScanOrigin::Data,
            };
            next.withdraw_scanned_package(&record);
        }
        next.remove_package_setting(name).map_err(|error| format!("live setting retirement: {error:?}"))?;
        next.retain_seinfo_after_code_removal()?;
        next.complete_retained_library_dependencies()?;
        if next.has_shared_processes() { next.rebuild_shared_processes_from_native_members()?; }
        Ok(next)
    }
    /// Remove the real-name key only after permission uninstall and any shared
    /// UID conversion, as RemovePackageHelper does. Values naming the deleted
    /// package under other keys are not removed by Settings here.
    pub fn remove_renamed_package(&mut self, real_name: Option<&str>) -> bool {
        let before = self.settings.renamed_packages.len();
        self.settings
            .renamed_packages
            .retain(|(new, _)| Some(new.as_str()) != real_name);
        self.settings.renamed_packages.len() != before
    }

    /// Commit removal from an old group after a replacement setting is accepted.
    /// Disabled references keep the group and its UID slot alive.
    pub(super) fn detach_shared_member(&mut self, package: &Package) -> Result<bool, SigningError> {
        let Some(id) = package.shared_app_id() else {
            return Ok(false);
        };
        let fail = |message: &str| {
            SigningError::Fatal(Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase: "shared-setting",
                message: message.into(),
            })
        };
        let displaced = self
            .is_displaced_shared_setting(package)
            .map_err(|message| fail(&message))?;
        let Some(Owner::SharedUser(name)) = self.identities.ids.get(id) else {
            return Err(fail("shared UID slot disagrees with setting"));
        };
        let name = name.clone();
        let group = self
            .identities
            .shared_users
            .get_mut(&name)
            .ok_or_else(|| fail("shared UID owner is missing"))?;
        if !group.remove_package(&package.name) && !displaced {
            return Err(fail("shared UID membership is missing"));
        }
        let retained = group.member_count() != 0;
        let used = retained
            || self
                .settings
                .disabled_system_packages
                .iter()
                .any(|p| p.shared_app_id() == Some(id));
        if !used {
            self.remove_legacy_shared(&name)
                .map_err(|message| fail(&message))?;
            self.identities.remove_shared_user(&name);
            self.settings.shared_users.retain(|g| g.name != name);
            self.identities.ids.remove(id);
        }
        Ok(!used)
    }

    /// Remove the saved setting and its UID membership. The caller must first
    /// complete data/domain/keyset/filter/preferred cleanup and withdraw loaded
    /// code. Permission uninstall reconciliation follows this step (#822/#798).
    /// No persistence, keystore work or query publication is implicit here.
    pub fn remove_package_setting(
        &mut self,
        name: &str,
    ) -> Result<Option<RemovedSetting>, SigningError> {
        let mut staged = self.clone();
        let result = staged.remove_package_setting_inner(name)?;
        *self = staged;
        Ok(result)
    }

    fn remove_package_setting_inner(
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
        let displaced = self
            .is_displaced_shared_setting(&package)
            .map_err(|message| fail(&message))?;
        let shared = match self.identities.ids.get(package.uid_owner_id()) {
            Some(Owner::SharedUser(group_name)) if package.shared_user => {
                let group = self
                    .identities
                    .shared_users
                    .get(group_name)
                    .ok_or_else(|| fail("shared UID owner is missing"))?;
                if !group.has_package(name) && !displaced {
                    return Err(fail("shared UID membership is missing"));
                }
                Some(group_name.clone())
            }
            Some(Owner::Package(_) | Owner::DetachedPackage(_))
                if self.identities.ids.owns_package_slot(&package) =>
            {
                None
            }
            None if displaced => None,
            _ => return Err(fail("package UID slot disagrees with setting")),
        };
        self.detach_retained_user_aliases(name);
        self.settings.packages.remove(at);
        self.installers.remove(name, &mut self.settings);
        let app_id_removed = if shared.is_some() {
            self.detach_shared_member(&package)?
        } else {
            self.identities.ids.remove(package.uid_owner_id());
            true
        };
        self.forget_displaced_shared_setting(name);
        self.scanned_users.remove(name);
        self.remove_setting_legacy(name);
        self.remove_setting_runtime(name);
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
    fn removing_adopted_setting_releases_retained_slot_without_changing_capture() {
        let mut scan = owner(
            b"<packages><package name='a' codePath='/system/app/a' userId='10100'/></packages>",
        );
        let original = scan.settings.packages[0].clone();
        scan.retain_original_setting(&original, &Default::default())
            .unwrap();
        let capture = scan.clone();
        let removed = scan.remove_package_setting("a").unwrap().unwrap();
        assert!(removed.app_id_removed);
        assert_eq!(removed.package, original);
        assert!(scan.identities.ids.get(10100).is_none());
        assert!(scan.identities.ids.detached_setting(10100).is_none());
        assert_eq!(
            capture
                .identities
                .ids
                .detached_setting(10100)
                .unwrap()
                .package,
            original
        );
    }

    #[test]
    fn old_shared_pruning_uses_members_even_when_request_setting_stays_in_map() {
        for (other, disabled) in [(false, false), (true, false), (false, true)] {
            let xml = format!(
                "<packages><shared-user name='old' userId='10100'/><package name='incoming' codePath='/system/app/incoming' sharedUserId='10100'/>{}{}</packages>",
                if other {
                    "<package name='other' codePath='/system/app/other' sharedUserId='10100'/>"
                } else {
                    ""
                },
                if disabled {
                    "<updated-package name='incoming' codePath='/system/app/incoming' sharedUserId='10100'/>"
                } else {
                    ""
                },
            );
            let mut scan = owner(xml.as_bytes());
            let request = scan.settings.packages[0].clone();
            let capture = scan.clone();
            assert_eq!(
                scan.detach_shared_member(&request).unwrap(),
                !other && !disabled
            );
            assert_eq!(scan.settings.packages[0], request);
            assert_eq!(scan.identities.ids.get(10100).is_some(), other || disabled);
            assert_eq!(
                scan.settings.shared_users.iter().any(|g| g.name == "old"),
                other || disabled
            );
            if other || disabled {
                assert!(!scan.identities.shared_users["old"].has_package("incoming"));
            }
            assert!(capture.identities.shared_users["old"].has_package("incoming"));
            assert!(capture.identities.ids.get(10100).is_some());
        }
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
