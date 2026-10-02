//! Scan identity selection, ported from android-16.0.0_r1
//! PackageManagerService.renameStaticSharedLibraryPackage,
//! InstallPackageHelper, ScanPackageUtils and AndroidPackageUtils (#804).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::{pkg::AndroidPackage, settings::Settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub manifest_name: String,
    pub internal_name: String,
    pub real_name: Option<String>,
}

impl Identity {
    /// InstallPackageHelper.getOriginalPackageLocked: declarations are tried
    /// backwards, and an already scanned original cannot transfer its data.
    pub(super) fn original_setting<'a>(
        pkg: &AndroidPackage,
        settings: &'a Settings,
        scanned: &dyn Fn(&str) -> bool,
    ) -> Option<&'a crate::package::settings::Package> {
        if Self::select(pkg, settings, true).real_name.is_some() {
            return None;
        }
        pkg.original_packages
            .as_ref()?
            .iter()
            .rev()
            .flatten()
            .find_map(|name| {
                let old = settings.packages.iter().find(|p| p.name == *name)?;
                if old.flags & crate::package::settings::FLAG_SYSTEM == 0 || scanned(name) {
                    return None;
                }
                if old.shared_user {
                    let group = settings
                        .shared_users
                        .iter()
                        .find(|g| g.app_id == old.app_id)?;
                    if pkg.shared_user_id.as_deref() != Some(&group.name) {
                        return None;
                    }
                }
                Some(old)
            })
    }

    /// Select from a raw native-parsed APK, before any owner rename.
    /// Apply the result after code verification, before reconciliation.
    pub fn select(pkg: &AndroidPackage, settings: &Settings, system: bool) -> Self {
        let manifest_name = pkg
            .manifest_package_name
            .clone()
            .unwrap_or_else(|| pkg.package_name.clone());
        let mut internal_name = if pkg.static_shared_library_name.is_some() {
            format!("{}_{}", pkg.package_name, pkg.static_shared_lib_version)
        } else {
            pkg.package_name.clone()
        };
        let mut real_name = None;
        if system
            && let Some(originals) = &pkg.original_packages
            && !originals.is_empty()
        {
            // Settings' ArrayMap keeps the last declaration for a name.
            if let Some((_, old)) = settings
                .renamed_packages
                .iter()
                .rev()
                .find(|(new, _)| *new == manifest_name)
                && originals.iter().any(|name| name.as_ref() == Some(old))
            {
                internal_name = old.clone();
                real_name = Some(manifest_name.clone());
            }
        }
        Self {
            manifest_name,
            internal_name,
            real_name,
        }
    }

    /// PackageImpl.setPackageName changes the package and each top-level
    /// component's owning package. Manifest/class/process names and
    /// component attributes remain those parsed from the original APK.
    pub fn apply(&self, pkg: &mut AndroidPackage) {
        pkg.package_name.clone_from(&self.internal_name);
        for p in &mut pkg.permissions {
            p.component.package_name.clone_from(&self.internal_name);
        }
        for g in &mut pkg.permission_groups {
            g.component.package_name.clone_from(&self.internal_name);
        }
        for a in pkg.activities.iter_mut().chain(&mut pkg.receivers) {
            a.main
                .component
                .package_name
                .clone_from(&self.internal_name);
        }
        for p in &mut pkg.providers {
            p.main
                .component
                .package_name
                .clone_from(&self.internal_name);
        }
        for s in &mut pkg.services {
            s.main
                .component
                .package_name
                .clone_from(&self.internal_name);
        }
        for i in &mut pkg.instrumentations {
            i.component.package_name.clone_from(&self.internal_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::*;
    #[test]
    fn originals_follow_reverse_order_system_presence_and_shared_uid_rules() {
        use crate::package::settings::{FLAG_SYSTEM, Package, SharedUser};
        let mut settings = Settings {
            packages: ["first", "last"]
                .map(|name| Package {
                    name: name.into(),
                    flags: FLAG_SYSTEM,
                    ..Default::default()
                })
                .into(),
            ..Default::default()
        };
        let mut pkg = AndroidPackage {
            package_name: "new".into(),
            original_packages: Some(vec![
                Some("first".into()),
                None,
                Some("missing".into()),
                Some("last".into()),
            ]),
            ..Default::default()
        };
        let selected = |settings: &Settings, scanned: &dyn Fn(&str) -> bool| {
            Identity::original_setting(&pkg, settings, scanned).map(|p| p.name.clone())
        };
        assert_eq!(selected(&settings, &|_| false).as_deref(), Some("last"));
        assert_eq!(
            selected(&settings, &|n| n == "last").as_deref(),
            Some("first")
        );
        assert_eq!(selected(&settings, &|_| true), None);
        settings.packages[1].flags = 0;
        assert_eq!(selected(&settings, &|_| false).as_deref(), Some("first"));
        settings.packages[1].flags = FLAG_SYSTEM;
        settings.packages[1].shared_user = true;
        settings.packages[1].app_id = 1000;
        settings.shared_users.push(SharedUser {
            name: "group".into(),
            app_id: 1000,
            flags: 0,
            signatures: None,
        });
        assert_eq!(selected(&settings, &|_| false).as_deref(), Some("first"));
        pkg.shared_user_id = Some("group".into());
        assert_eq!(
            Identity::original_setting(&pkg, &settings, &|_| false)
                .unwrap()
                .name,
            "last"
        );
        settings
            .renamed_packages
            .push(("new".into(), "first".into()));
        assert!(Identity::original_setting(&pkg, &settings, &|_| false).is_none());
    }
    #[test]
    fn applying_identity_changes_only_the_original_owners_fields() {
        let component = Component {
            name: "new.Class".into(),
            package_name: "new".into(),
            flags: 7,
            ..Default::default()
        };
        let main = MainComponent {
            component: component.clone(),
            process_name: Some("new:process".into()),
            ..Default::default()
        };
        let mut pkg = AndroidPackage {
            package_name: "new".into(),
            manifest_package_name: Some("new".into()),
            activities: vec![Activity {
                main: main.clone(),
                ..Default::default()
            }],
            receivers: vec![Activity {
                main: main.clone(),
                ..Default::default()
            }],
            services: vec![Service {
                main: main.clone(),
                ..Default::default()
            }],
            providers: vec![Provider {
                main: main.clone(),
                ..Default::default()
            }],
            permissions: vec![Permission {
                component: component.clone(),
                ..Default::default()
            }],
            permission_groups: vec![PermissionGroup {
                component: component.clone(),
                ..Default::default()
            }],
            instrumentations: vec![Instrumentation {
                component,
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = pkg.clone();
        let identity = Identity {
            manifest_name: "new".into(),
            internal_name: "old".into(),
            real_name: Some("new".into()),
        };
        identity.apply(&mut pkg);
        assert_eq!(pkg.package_name, "old");
        let fields = [
            &pkg.activities[0].main.component,
            &pkg.receivers[0].main.component,
            &pkg.services[0].main.component,
            &pkg.providers[0].main.component,
            &pkg.permissions[0].component,
            &pkg.permission_groups[0].component,
            &pkg.instrumentations[0].component,
        ];
        assert!(
            fields
                .iter()
                .all(|c| c.package_name == "old" && c.name == "new.Class" && c.flags == 7)
        );
        let mut restored = identity;
        restored.internal_name = "new".into();
        restored.apply(&mut pkg);
        assert_eq!(pkg, before);
    }
    #[test]
    fn ordinary_static_and_declared_system_renames_select_distinct_names() {
        let mut pkg = AndroidPackage {
            package_name: "new".into(),
            manifest_package_name: Some("new".into()),
            ..Default::default()
        };
        let mut settings = Settings::default();
        let ordinary = Identity {
            manifest_name: "new".into(),
            internal_name: "new".into(),
            real_name: None,
        };
        assert_eq!(Identity::select(&pkg, &settings, true), ordinary);
        pkg.static_shared_library_name = Some("library".into());
        pkg.static_shared_lib_version = 42;
        assert_eq!(
            Identity::select(&pkg, &settings, false).internal_name,
            "new_42"
        );
        assert_eq!(Identity::select(&pkg, &settings, true).manifest_name, "new");
        pkg.static_shared_library_name = None;
        settings.renamed_packages.push(("new".into(), "old".into()));
        assert_eq!(Identity::select(&pkg, &settings, true), ordinary);
        pkg.original_packages = Some(vec![Some("other".into())]);
        assert_eq!(Identity::select(&pkg, &settings, true), ordinary);
        pkg.original_packages = Some(vec![Some("old".into())]);
        assert_eq!(Identity::select(&pkg, &settings, false), ordinary);
        assert_eq!(
            Identity::select(&pkg, &settings, true),
            Identity {
                manifest_name: "new".into(),
                internal_name: "old".into(),
                real_name: Some("new".into()),
            }
        );
        settings
            .renamed_packages
            .push(("new".into(), "other".into()));
        assert_eq!(Identity::select(&pkg, &settings, true), ordinary);
    }
}
