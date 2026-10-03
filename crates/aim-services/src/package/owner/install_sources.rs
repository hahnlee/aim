//! Settings installer-name tracking and InstallSource.removeInstallerPackage.
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.
use crate::package::settings::{InstallSource, Settings};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installers {
    names: BTreeSet<String>,
}

impl Installers {
    pub fn restore(settings: &Settings) -> Self {
        let mut owner = Self::default();
        for package in &settings.packages {
            owner.add(&package.install_source);
        }
        owner
    }

    /// Update owners alone do not enter Settings.mInstallerPackages.
    pub fn add(&mut self, source: &InstallSource) {
        for name in [
            &source.installer,
            &source.initiating_package,
            &source.originating_package,
        ] {
            if let Some(name) = name {
                self.names.insert(name.clone());
            }
        }
    }

    /// Invoke after removing the package setting. Disabled factories are not
    /// visited; the registry retains names until that installer is removed.
    pub fn remove(&mut self, name: &str, settings: &mut Settings) {
        if !self.names.remove(name) {
            return;
        }
        for package in &mut settings.packages {
            remove_source(name, &mut package.install_source);
        }
    }
}

fn remove_source(name: &str, source: &mut InstallSource) {
    let mut changed = false;
    if source.initiating_package.as_deref() == Some(name) && !source.initiating_package_uninstalled
    {
        source.initiating_package_uninstalled = true;
        changed = true;
    }
    if source.originating_package.as_deref() == Some(name) {
        source.originating_package = None;
        changed = true;
    }
    if source.installer.as_deref() == Some(name) {
        source.installer = None;
        source.installer_uid = -1;
        source.is_orphaned = true;
        changed = true;
    }
    if source.update_owner.as_deref() == Some(name) {
        source.update_owner = None;
        changed = true;
    }
    if changed {
        source.installer_attribution_tag = None;
        // createInternal memoizes empty and empty-orphaned sources.
        if source.initiating_package.is_none()
            && source.originating_package.is_none()
            && source.installer.is_none()
            && source.update_owner.is_none()
            && source.initiating_package_signatures.is_none()
            && !source.initiating_package_uninstalled
            && source.package_source == 0
        {
            *source = InstallSource {
                is_orphaned: source.is_orphaned,
                ..Default::default()
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_owner_alone_is_not_registered_and_unmodified_sources_keep_attribution() {
        let root = aim_android_xml::read(b"<packages><package name='app' codePath='/data/app/app' userId='10100' updateOwner='store' installerAttributionTag='keep'/></packages>").unwrap();
        let mut settings = Settings::parse(&root).unwrap();
        let original = settings.clone();
        let mut owner = Installers::restore(&settings);
        owner.remove("store", &mut settings);
        assert_eq!(settings, original);
        // Registration through any of the three source roles activates the
        // global cleanup, including another package's update-owner reference.
        owner.add(&InstallSource {
            originating_package: Some("store".into()),
            ..Default::default()
        });
        owner.remove("store", &mut settings);
        assert_eq!(
            settings.packages[0].install_source,
            InstallSource::default()
        );
    }
}
