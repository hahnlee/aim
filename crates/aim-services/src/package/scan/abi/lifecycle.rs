//! ScanPackageUtils scan ABI lifecycle at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiPolicy, NativeLibraryEnvironment, NativeLibraryError, NativeLibraryPaths, NativeLibraryScan,
    PackageAbis,
};
use crate::package::{pkg::AndroidPackage, settings::Package, write::Apks};

#[derive(Clone, Copy)]
pub enum AbiScanMode<'a> {
    Existing {
        first_boot_or_upgrade: bool,
        old_was_stub: bool,
        saved: Option<&'a Package>,
    },
    /// Compilation/installation owns ABI derivation before SCAN_NEW_INSTALL.
    Install { moved: Option<&'a Package> },
    /// apexd owns APEX native libraries; APK derivation must not inspect them.
    Apex,
}

#[derive(Clone, Copy)]
pub struct AbiScanContext<'a> {
    pub mode: AbiScanMode<'a>,
    pub system: bool,
    pub updated: bool,
    pub override_abi: Option<&'a str>,
    /// Supplied only for the validated platform package, from the VM owner.
    pub platform_runtime_64bit: Option<bool>,
}

enum Source {
    Derive,
    Known(PackageAbis),
    Apex,
}

impl AbiScanMode<'_> {
    fn source(self, pkg: &AndroidPackage) -> Result<Source, NativeLibraryError> {
        let known = |saved: &Package| {
            if saved.name != pkg.package_name {
                return Err(NativeLibraryError::Input(
                    "ABI setting belongs to another package".into(),
                ));
            }
            Ok(Source::Known(PackageAbis {
                primary: saved.primary_cpu_abi.clone(),
                secondary: saved.secondary_cpu_abi.clone(),
            }))
        };
        match self {
            Self::Apex => Ok(Source::Apex),
            Self::Existing {
                first_boot_or_upgrade: true,
                ..
            }
            | Self::Existing {
                old_was_stub: true, ..
            }
            | Self::Existing { saved: None, .. } => Ok(Source::Derive),
            Self::Existing {
                saved: Some(saved), ..
            }
            | Self::Install { moved: Some(saved) } => known(saved),
            Self::Install { moved: None } => Ok(Source::Known(PackageAbis {
                primary: pkg.primary_cpu_abi.clone(),
                secondary: pkg.secondary_cpu_abi.clone(),
            })),
        }
    }
}

impl AbiScanContext<'_> {
    pub fn effective_override(&self) -> Option<&str> {
        self.override_abi.filter(|a| *a != "-")
    }

    /// Final ScanPackageUtils setting enrichment, including APEX settings.
    pub fn apply_setting(
        &self,
        pkg: &AndroidPackage,
        setting: &mut Package,
    ) -> Result<(), NativeLibraryError> {
        if pkg.package_name != setting.name {
            return Err(NativeLibraryError::Input(
                "ABI setting belongs to another package".into(),
            ));
        }
        setting.primary_cpu_abi = pkg.primary_cpu_abi.clone();
        setting.secondary_cpu_abi = pkg.secondary_cpu_abi.clone();
        setting.cpu_abi_override = self.effective_override().map(str::to_owned);
        setting.legacy_native_library_path = pkg.native_library_root_dir.clone();
        Ok(())
    }
}

impl Apks {
    pub fn scan_native_libraries(
        &self,
        pkg: &AndroidPackage,
        policy: &AbiPolicy,
        env: &NativeLibraryEnvironment<'_>,
        context: AbiScanContext<'_>,
    ) -> Result<Option<NativeLibraryScan>, NativeLibraryError> {
        let mut scan = match context.mode.source(pkg)? {
            Source::Apex => return Ok(None),
            Source::Derive => self.native_library_scan(
                pkg,
                policy,
                env,
                context.system,
                context.updated,
                context.effective_override(),
            )?,
            Source::Known(abis) => {
                let mut selected = pkg.clone();
                abis.clone().apply(&mut selected);
                let paths =
                    NativeLibraryPaths::derive(&selected, env, context.system, context.updated)
                        .map_err(NativeLibraryError::Input)?;
                NativeLibraryScan {
                    abis,
                    paths,
                    multi_arch_mismatch: false,
                    requires_extraction: false,
                    extraction_abis: Vec::new(),
                }
            }
        };
        // Original forces the platform primary ABI after native paths have
        // been derived, retaining the secondary ABI and those paths.
        if let Some(wide) = context.platform_runtime_64bit {
            scan.abis.primary = Some(
                (if wide { &policy.bit64 } else { &policy.bit32 })
                    .first()
                    .ok_or_else(|| {
                        NativeLibraryError::Input("platform VM has no supported ABI".into())
                    })?
                    .clone(),
            );
        }
        Ok(Some(scan))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_selects_saved_compiled_rederived_and_apex_sources() {
        let pkg = AndroidPackage {
            package_name: "fixture".into(),
            primary_cpu_abi: Some("x86".into()),
            ..Default::default()
        };
        let saved = Package {
            name: "fixture".into(),
            primary_cpu_abi: Some("arm64-v8a".into()),
            secondary_cpu_abi: Some("armeabi-v7a".into()),
            ..Default::default()
        };
        for first in [false, true] {
            for stub in [false, true] {
                let source = AbiScanMode::Existing {
                    first_boot_or_upgrade: first,
                    old_was_stub: stub,
                    saved: Some(&saved),
                }
                .source(&pkg)
                .unwrap();
                if first || stub {
                    assert!(matches!(source, Source::Derive));
                } else {
                    let Source::Known(abis) = source else {
                        panic!()
                    };
                    assert_eq!(abis.primary, saved.primary_cpu_abi);
                    assert_eq!(abis.secondary, saved.secondary_cpu_abi);
                }
            }
        }
        assert!(matches!(
            AbiScanMode::Existing {
                first_boot_or_upgrade: false,
                old_was_stub: false,
                saved: None
            }
            .source(&pkg)
            .unwrap(),
            Source::Derive
        ));
        let Source::Known(compiled) = AbiScanMode::Install { moved: None }.source(&pkg).unwrap()
        else {
            panic!()
        };
        assert_eq!(compiled.primary, pkg.primary_cpu_abi);
        let Source::Known(moved) = AbiScanMode::Install {
            moved: Some(&saved),
        }
        .source(&pkg)
        .unwrap() else {
            panic!()
        };
        assert_eq!(moved.primary, saved.primary_cpu_abi);
        assert!(matches!(
            AbiScanMode::Apex.source(&pkg).unwrap(),
            Source::Apex
        ));
    }
    #[test]
    fn setting_enrichment_normalizes_clear_override_and_rejects_foreign_identity() {
        let context = AbiScanContext {
            mode: AbiScanMode::Apex,
            system: true,
            updated: false,
            override_abi: Some("-"),
            platform_runtime_64bit: None,
        };
        let pkg = AndroidPackage {
            package_name: "fixture".into(),
            primary_cpu_abi: Some("arm64-v8a".into()),
            ..Default::default()
        };
        let mut setting = Package {
            name: "other".into(),
            cpu_abi_override: Some("old".into()),
            ..Default::default()
        };
        let before = setting.clone();
        assert!(context.apply_setting(&pkg, &mut setting).is_err());
        assert_eq!(setting, before);
        setting.name = pkg.package_name.clone();
        context.apply_setting(&pkg, &mut setting).unwrap();
        assert_eq!(setting.cpu_abi_override, None);
        assert_eq!(setting.primary_cpu_abi, pkg.primary_cpu_abi);
        let Source::Known(_) = (AbiScanMode::Install {
            moved: Some(&setting),
        })
        .source(&pkg)
        .unwrap() else {
            panic!()
        };
        setting.name = "other".into();
        assert!(
            (AbiScanMode::Install {
                moved: Some(&setting)
            })
            .source(&pkg)
            .is_err()
        );
    }
}
