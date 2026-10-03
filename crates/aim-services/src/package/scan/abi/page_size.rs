//! ScanPackageUtils page-size setting enrichment at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{AbiScanContext, AbiScanMode, NativeLibraryError};
use crate::package::{parse::Platform, pkg::AndroidPackage, settings::Package};

pub struct PageSizeCompatPolicy {
    pub enabled: bool,
}

impl PageSizeCompatPolicy {
    pub fn from_platform(platform: &Platform) -> Result<Self, String> {
        Ok(Self {
            enabled: *platform
                .flags
                .get("android.content.pm.app_compat_option_16kb")
                .ok_or("missing app_compat_option_16kb image policy")?,
        })
    }

    /// Preserve an alignment failure as a caller-visible diagnostic and leave
    /// the saved flags unchanged, as the original scan does. Invalid setting
    /// values and identities reject before mutation.
    pub fn apply_setting(
        &self,
        pkg: &AndroidPackage,
        setting: &mut Package,
        context: AbiScanContext<'_>,
        page_size: u64,
        supported_64: &[String],
        alignment: &dyn Fn() -> Result<u32, String>,
    ) -> Result<Option<String>, NativeLibraryError> {
        if page_size < 4096 || !page_size.is_power_of_two() {
            return Err(NativeLibraryError::Input("invalid guest page size".into()));
        }
        if pkg.package_name != setting.name {
            return Err(NativeLibraryError::Input(
                "page-size setting belongs to another package".into(),
            ));
        }
        if !self.enabled || page_size != 16384 {
            return Ok(None);
        }
        let mode = if pkg.page_size_app_compat_flags > 0 {
            pkg.page_size_app_compat_flags
        } else {
            if supported_64.is_empty()
                || context.system
                || matches!(context.mode, AbiScanMode::Apex)
                || context.platform_runtime_64bit.is_some()
            {
                return Ok(None);
            }
            match alignment() {
                Ok(flags) => i32::try_from(flags)
                    .map_err(|_| NativeLibraryError::Input("alignment flags overflow".into()))?,
                Err(error) => return Ok(Some(error)),
            }
        };
        setting
            .set_page_size_compat(mode)
            .map_err(NativeLibraryError::Input)?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> AbiScanContext<'static> {
        AbiScanContext {
            mode: AbiScanMode::Install { moved: None },
            system: false,
            updated: false,
            override_abi: None,
            platform_runtime_64bit: None,
        }
    }
    #[test]
    fn manifest_and_scan_exclusions_avoid_alignment_reads() {
        let policy = PageSizeCompatPolicy { enabled: true };
        let mut pkg = AndroidPackage {
            package_name: "fixture".into(),
            ..Default::default()
        };
        let mut setting = Package {
            name: pkg.package_name.clone(),
            page_size_compat: 16,
            ..Default::default()
        };
        let never = || panic!("unexpected alignment read");
        let abis = vec!["arm64-v8a".into()];
        for mode in 0..4 {
            let mut ctx = context();
            match mode {
                0 => ctx.system = true,
                1 => ctx.mode = AbiScanMode::Apex,
                2 => ctx.platform_runtime_64bit = Some(true),
                _ => {}
            }
            policy
                .apply_setting(
                    &pkg,
                    &mut setting,
                    ctx,
                    16384,
                    if mode == 3 { &[] } else { &abis },
                    &never,
                )
                .unwrap();
            assert_eq!(setting.page_size_compat, 16);
        }
        pkg.page_size_app_compat_flags = 32;
        let mut ctx = context();
        ctx.system = true;
        policy
            .apply_setting(&pkg, &mut setting, ctx, 16384, &abis, &never)
            .unwrap();
        assert_eq!(setting.page_size_compat, 48);
        policy
            .apply_setting(&pkg, &mut setting, ctx, 4096, &abis, &never)
            .unwrap();
        assert_eq!(setting.page_size_compat, 48);
        PageSizeCompatPolicy { enabled: false }
            .apply_setting(&pkg, &mut setting, context(), 16384, &abis, &never)
            .unwrap();
        assert_eq!(setting.page_size_compat, 48);
    }
    #[test]
    fn failures_retain_saved_flags_and_success_uses_original_setter() {
        let policy = PageSizeCompatPolicy { enabled: true };
        let mut pkg = AndroidPackage {
            package_name: "fixture".into(),
            ..Default::default()
        };
        let mut setting = Package {
            name: pkg.package_name.clone(),
            page_size_compat: 32,
            ..Default::default()
        };
        let abis = vec!["arm64-v8a".into()];
        assert_eq!(
            policy
                .apply_setting(&pkg, &mut setting, context(), 16384, &abis, &|| Err(
                    "unreadable library".into()
                ))
                .unwrap(),
            Some("unreadable library".into())
        );
        assert_eq!(setting.page_size_compat, 32);
        policy
            .apply_setting(&pkg, &mut setting, context(), 16384, &abis, &|| Ok(6))
            .unwrap();
        assert_eq!(setting.page_size_compat, 38);
        for mode in [8, 16, 8] {
            setting.set_page_size_compat(mode).unwrap();
        }
        assert_eq!(setting.page_size_compat, 46);
        let before = setting.clone();
        pkg.page_size_app_compat_flags = 128;
        assert!(
            policy
                .apply_setting(&pkg, &mut setting, context(), 16384, &abis, &|| panic!())
                .is_err()
        );
        assert_eq!(setting, before);
        for mode in [-1, 128] {
            assert!(setting.set_page_size_compat(mode).is_err());
            assert_eq!(setting, before);
        }
    }

    #[test]
    fn persisted_page_size_modes_use_the_same_range_validation() {
        for mode in [-1, 0, 127, 128] {
            let xml = format!(
                "<packages><package name='fixture' codePath='/data/app/fixture' userId='10001' pageSizeCompat='{mode}' /></packages>"
            );
            let root = aim_android_xml::read(xml.as_bytes()).unwrap();
            let settings = crate::package::settings::Settings::parse(&root);
            if (0..128).contains(&mode) {
                assert_eq!(settings.unwrap().packages[0].page_size_compat, mode);
            } else {
                assert_eq!(
                    settings.unwrap_err(),
                    "Invalid page size compat mode specified"
                );
            }
        }
    }
}
