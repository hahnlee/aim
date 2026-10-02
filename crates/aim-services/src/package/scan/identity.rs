//! Scan identity selection, ported from android-16.0.0_r1
//! PackageManagerService.renameStaticSharedLibraryPackage,
//! ScanPackageUtils and AndroidPackageUtils (#804).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::{pkg::AndroidPackage, settings::Settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub manifest_name: String,
    pub internal_name: String,
    pub real_name: Option<String>,
}

impl Identity {
    /// Select from a raw native-parsed APK, before any owner rename.
    /// Parsed component names are transformed separately at publication.
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
