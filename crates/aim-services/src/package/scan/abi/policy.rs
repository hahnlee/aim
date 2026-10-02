//! PackageAbiHelperImpl ABI selection at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    NativeLibraryEnvironment, NativeLibraryPaths, SupportedAbi, SupportedAbis, ZipNativeLibraries,
    instruction_set,
};
use crate::package::{
    parse::Platform,
    pkg::{AndroidPackage, booleans},
};

pub struct AbiPolicy {
    pub all: Vec<String>,
    pub bit32: Vec<String>,
    pub bit64: Vec<String>,
    pub native32: Vec<String>,
    pub native64: Vec<String>,
    pub force_multi_arch_match: bool,
}

impl AbiPolicy {
    pub fn from_platform(
        platform: &Platform,
        all: &[String],
        supported: &SupportedAbis<'_>,
        property: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            all: all.to_vec(),
            bit32: supported.bit32.to_vec(),
            bit64: supported.bit64.to_vec(),
            native32: native_supported(supported.bit32, property)?,
            native64: native_supported(supported.bit64, property)?,
            force_multi_arch_match: *platform
                .flags
                .get("android.content.pm.force_multi_arch_native_libs_match")
                .ok_or("missing force_multi_arch_native_libs_match image policy")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageAbis {
    pub primary: Option<String>,
    pub secondary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiSelectionError {
    pub code: i32,
    pub message: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeLibraryError {
    Input(String),
    Selection(AbiSelectionError),
}

#[derive(Debug)]
pub struct NativeLibraryScan {
    pub abis: PackageAbis,
    pub paths: NativeLibraryPaths,
    pub multi_arch_mismatch: bool,
    pub requires_extraction: bool,
}

impl NativeLibraryScan {
    /// Commit scan metadata only after the side-effect owner has completed any
    /// required extraction/alignment work. Factory image inputs need no extraction.
    pub fn apply_metadata(self, pkg: &mut AndroidPackage) {
        self.abis.apply(pkg);
        self.paths.apply(pkg);
    }
}

impl crate::package::write::Apks {
    /// Initial derivation phase of ScanPackageUtils: ZIP ABI policy, factory
    /// inventory fallback only when primary is absent, then final native paths.
    /// Read/selection/path errors leave the supplied package unchanged.
    pub fn native_library_scan(
        &self,
        pkg: &AndroidPackage,
        policy: &AbiPolicy,
        env: &NativeLibraryEnvironment<'_>,
        system: bool,
        updated: bool,
        override_abi: Option<&str>,
    ) -> Result<NativeLibraryScan, NativeLibraryError> {
        NativeLibraryPaths::derive(pkg, env, system, updated).map_err(NativeLibraryError::Input)?;
        let inventory = self
            .zip_native_libraries(pkg)
            .map_err(NativeLibraryError::Input)?;
        let mut abis = PackageAbis::select(pkg, &inventory, policy, override_abi)
            .map_err(NativeLibraryError::Selection)?;
        let mut mismatch = false;
        if system && !updated && abis.primary.is_none() {
            let bundled = self
                .bundled_abis(
                    pkg,
                    env,
                    &SupportedAbis {
                        bit32: &policy.bit32,
                        bit64: &policy.bit64,
                    },
                )
                .map_err(NativeLibraryError::Input)?;
            mismatch = bundled.multi_arch_mismatch;
            abis = PackageAbis {
                primary: bundled.primary,
                secondary: bundled.secondary,
            };
        }
        let library = pkg.sdk_library_name.is_some()
            || pkg.static_shared_library_name.is_some()
            || !pkg.library_names.is_empty();
        let requires_extraction = !library
            && pkg.is(booleans::EXTRACT_NATIVE_LIBS)
            && !(system && !updated)
            && abis
                .primary
                .as_ref()
                .is_some_and(|a| inventory.abis.contains(a.as_bytes()));
        let mut selected = pkg.clone();
        abis.clone().apply(&mut selected);
        let paths = NativeLibraryPaths::derive(&selected, env, system, updated)
            .map_err(NativeLibraryError::Input)?;
        Ok(NativeLibraryScan {
            abis,
            paths,
            multi_arch_mismatch: mismatch,
            requires_extraction,
        })
    }
}

impl PackageAbis {
    /// Select from checked ZIP inventory. Extraction/alignment is a separate
    /// owner phase; this does not claim an installation has completed.
    pub fn select(
        pkg: &AndroidPackage,
        inventory: &ZipNativeLibraries,
        policy: &AbiPolicy,
        override_abi: Option<&str>,
    ) -> Result<Self, AbiSelectionError> {
        let error = |code, message| AbiSelectionError { code, message };
        if pkg.is(booleans::MULTI_ARCH) {
            let force = policy.force_multi_arch_match
                && pkg.target_sdk_version >= 35
                && override_abi.is_none();
            let (narrow, wide) = if force {
                (&policy.native32, &policy.native64)
            } else {
                (&policy.bit32, &policy.bit64)
            };
            let selected = |abis: &[String]| -> Result<Option<String>, AbiSelectionError> {
                if abis.is_empty() {
                    return Ok(None);
                }
                match inventory.find_supported_abi(abis) {
                    SupportedAbi::Index(at) => Ok(Some(abis[at].clone())),
                    SupportedAbi::NoMatch if force => Err(error(
                        -131,
                        "The multiArch app's native libs don't support all the natively supported ABIs of the device.",
                    )),
                    _ => Ok(None),
                }
            };
            let narrow = selected(narrow)?;
            let wide = selected(wide)?;
            let (primary, secondary) = match (narrow, wide) {
                (Some(n), Some(w)) if pkg.is(booleans::USE_32_BIT_ABI) => (Some(n), Some(w)),
                (Some(n), Some(w)) => (Some(w), Some(n)),
                (n, w) => (n.or(w), None),
            };
            Ok(Self { primary, secondary })
        } else {
            let override_list = override_abi.map(|a| vec![a.into()]);
            let renderscript = !policy.bit64.is_empty()
                && override_abi.is_none()
                && inventory.renderscript_bitcode;
            let supported = if renderscript {
                if policy.bit32.is_empty() {
                    return Err(error(
                        -16,
                        "Apps that contain RenderScript with target API level < 21 are not supported on 64-bit only platforms",
                    ));
                }
                &policy.bit32
            } else {
                override_list.as_ref().unwrap_or(&policy.all)
            };
            let primary = match inventory.find_supported_abi(supported) {
                SupportedAbi::Index(at) => {
                    if pkg.sdk_library_name.is_some()
                        || pkg.static_shared_library_name.is_some()
                        || !pkg.library_names.is_empty()
                    {
                        return Err(error(
                            -110,
                            "Shared library with native libs must be multiarch",
                        ));
                    }
                    Some(supported[at].clone())
                }
                SupportedAbi::NoMatch => {
                    return Err(error(
                        -110,
                        "Error unpackaging native libs for app, errorCode=-113",
                    ));
                }
                SupportedAbi::None => override_abi
                    .map(str::to_owned)
                    .or_else(|| renderscript.then(|| supported[0].clone())),
            };
            Ok(Self {
                primary,
                secondary: None,
            })
        }
    }

    pub fn apply(self, pkg: &mut AndroidPackage) {
        pkg.primary_cpu_abi = self.primary;
        pkg.secondary_cpu_abi = self.secondary;
    }
}

fn native_supported(
    abis: &[String],
    property: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<String>, String> {
    let mut selected = Vec::new();
    for abi in abis {
        let isa = instruction_set(abi)?;
        if property(&format!("ro.dalvik.vm.isa.{isa}")).is_none_or(|v| v.is_empty()) {
            selected.push(abi.clone());
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_bridge_properties_filter_abis_in_original_preference_order() {
        let abis = vec![
            "armeabi-v7a".into(),
            "armeabi".into(),
            "arm64-v8a".into(),
            "x86".into(),
        ];
        assert_eq!(
            native_supported(&abis, &|key| {
                match key {
                    "ro.dalvik.vm.isa.arm" => Some("arm64".into()),
                    "ro.dalvik.vm.isa.x86" => Some("".into()),
                    _ => None,
                }
            })
            .unwrap(),
            vec!["arm64-v8a", "x86"]
        );
        assert!(native_supported(&["invalid".into()], &|_| None).is_err());
    }
    #[test]
    fn multiarch_selection_honors_force_matching_preference_and_override() {
        let policy = AbiPolicy {
            all: vec!["arm64-v8a".into(), "armeabi-v7a".into()],
            bit32: vec!["armeabi-v7a".into()],
            bit64: vec!["arm64-v8a".into()],
            native32: vec!["armeabi-v7a".into()],
            native64: vec!["arm64-v8a".into()],
            force_multi_arch_match: true,
        };
        let mut pkg = AndroidPackage {
            booleans: booleans::MULTI_ARCH,
            target_sdk_version: 35,
            ..Default::default()
        };
        let mut inventory = ZipNativeLibraries::default();
        inventory.abis.insert(b"arm64-v8a".to_vec());
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, None)
                .unwrap_err()
                .code,
            -131
        );
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, Some("x86"))
                .unwrap()
                .primary
                .as_deref(),
            Some("arm64-v8a")
        );
        pkg.target_sdk_version = 34;
        assert!(PackageAbis::select(&pkg, &inventory, &policy, None).is_ok());
        inventory.abis.insert(b"armeabi-v7a".to_vec());
        pkg.booleans |= booleans::USE_32_BIT_ABI;
        let selected = PackageAbis::select(&pkg, &inventory, &policy, None).unwrap();
        assert_eq!(selected.primary.as_deref(), Some("armeabi-v7a"));
        assert_eq!(selected.secondary.as_deref(), Some("arm64-v8a"));
        selected.apply(&mut pkg);
        assert_eq!(pkg.primary_cpu_abi.as_deref(), Some("armeabi-v7a"));
    }
    #[test]
    fn single_arch_renderscript_override_and_shared_library_failures_are_explicit() {
        let mut policy = AbiPolicy {
            all: vec!["arm64-v8a".into()],
            bit32: vec![],
            bit64: vec!["arm64-v8a".into()],
            native32: vec![],
            native64: vec![],
            force_multi_arch_match: false,
        };
        let mut pkg = AndroidPackage::default();
        let mut inventory = ZipNativeLibraries {
            renderscript_bitcode: true,
            ..Default::default()
        };
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, None)
                .unwrap_err()
                .code,
            -16
        );
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, Some("x86"))
                .unwrap()
                .primary
                .as_deref(),
            Some("x86")
        );
        policy.bit32.push("armeabi-v7a".into());
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, None)
                .unwrap()
                .primary
                .as_deref(),
            Some("armeabi-v7a")
        );
        inventory.renderscript_bitcode = false;
        inventory.abis.insert(b"unknown".to_vec());
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, None)
                .unwrap_err()
                .code,
            -110
        );
        inventory.abis.insert(b"arm64-v8a".to_vec());
        pkg.library_names.push("library".into());
        assert_eq!(
            PackageAbis::select(&pkg, &inventory, &policy, None)
                .unwrap_err()
                .code,
            -110
        );
        assert_eq!(pkg.primary_cpu_abi, None);
    }
}
