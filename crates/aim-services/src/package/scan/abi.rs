//! Native library paths, bundled ABIs and shared-user ABI adjustment, ported
//! from PackageAbiHelperImpl, ScanPackageUtils and VMRuntime at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::pkg::AndroidPackage;
mod zip;
pub use zip::{SupportedAbi, ZipNativeLibraries};

/// Image/installation and guest filesystem inputs supplied by their owners.
pub struct NativeLibraryEnvironment<'a> {
    pub preferred_abi: &'a str,
    pub app_lib32_install_dir: &'a str,
    pub code_is_directory: bool,
    /// Required only for an unrecognized bundled partition; a guest path,
    /// resolved by the guest filesystem owner, never a canonical host path.
    pub canonical_source: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeLibraryPaths {
    pub root: String,
    pub requires_isa: bool,
    pub primary: String,
    pub secondary: Option<String>,
}

impl NativeLibraryPaths {
    /// Uses raw selected ABIs, not an existing PackageSetting's saved paths.
    pub fn derive(
        pkg: &AndroidPackage,
        env: &NativeLibraryEnvironment<'_>,
        system: bool,
        updated_system: bool,
    ) -> Result<Self, String> {
        let code = guest_path(pkg.path.as_deref().ok_or("missing package code path")?)?;
        if code.ends_with(".apk") {
            let name = code.rsplit('/').next().unwrap();
            let name = if env.code_is_directory {
                name
            } else {
                &name[..name.len() - 4]
            };
            let (root, secondary) = if system && !updated_system {
                let isa =
                    instruction_set(pkg.primary_cpu_abi.as_deref().unwrap_or(env.preferred_abi))?;
                let source = guest_path(
                    pkg.base_apk_path
                        .as_deref()
                        .ok_or("missing base APK path")?,
                )?;
                let apk_root = bundled_root(&source, env.canonical_source)?;
                let wide = matches!(isa, "arm64" | "x86_64" | "riscv64");
                let lib = if wide { "lib64" } else { "lib" };
                let other = if wide { "lib" } else { "lib64" };
                (
                    join(&join(&apk_root, lib), name),
                    pkg.secondary_cpu_abi
                        .as_ref()
                        .map(|_| join(&join(&apk_root, other), name)),
                )
            } else {
                (join(&guest_path(env.app_lib32_install_dir)?, name), None)
            };
            Ok(Self {
                primary: root.clone(),
                root,
                requires_isa: false,
                secondary,
            })
        } else {
            let isa = instruction_set(pkg.primary_cpu_abi.as_deref().unwrap_or(env.preferred_abi))?;
            let root = join(&code, "lib");
            let secondary = pkg
                .secondary_cpu_abi
                .as_deref()
                .map(instruction_set)
                .transpose()?
                .map(|isa| join(&root, isa));
            Ok(Self {
                primary: join(&root, isa),
                root,
                requires_isa: true,
                secondary,
            })
        }
    }

    pub fn apply(self, pkg: &mut AndroidPackage) {
        pkg.native_library_root_dir = Some(self.root);
        pkg.native_library_root_requires_isa = self.requires_isa;
        pkg.native_library_dir = Some(self.primary);
        pkg.secondary_native_library_dir = self.secondary;
    }
}

/// The selected image's Build.SUPPORTED_{32,64}_BIT_ABIS in preference order.
pub struct SupportedAbis<'a> {
    pub bit32: &'a [String],
    pub bit64: &'a [String],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledAbis {
    pub primary: Option<String>,
    pub secondary: Option<String>,
    /// The original reports this condition but still selects both ABIs.
    pub multi_arch_mismatch: bool,
}

impl BundledAbis {
    /// PackageAbiHelperImpl.getBundledAppAbis: inspect unpacked image libraries,
    /// without opening the APK ZIP or copying a saved PackageSetting ABI.
    pub fn derive(
        pkg: &AndroidPackage,
        env: &NativeLibraryEnvironment<'_>,
        supported: &SupportedAbis<'_>,
        exists: &dyn Fn(&str) -> Result<bool, String>,
    ) -> Result<Self, String> {
        let code = guest_path(pkg.path.as_deref().ok_or("missing package code path")?)?;
        let source = guest_path(
            pkg.base_apk_path
                .as_deref()
                .ok_or("missing base APK path")?,
        )?;
        let apk_root = bundled_root(&source, env.canonical_source)?;
        let first32 = supported.bit32.first().map(String::as_str);
        let first64 = supported.bit64.first().map(String::as_str);
        let (has32, has64) = if code.ends_with(".apk") {
            let name = code.rsplit('/').next().unwrap();
            let name = if env.code_is_directory {
                name
            } else {
                &name[..name.len() - 4]
            };
            let has64 = exists(&join(&join(&apk_root, "lib64"), name))?;
            let has32 = exists(&join(&join(&apk_root, "lib"), name))?;
            (has32, has64)
        } else {
            let root = join(&code, "lib");
            let check = |abi: Option<&str>| -> Result<bool, String> {
                match abi.filter(|a| !a.is_empty()) {
                    Some(abi) => exists(&join(&root, instruction_set(abi)?)),
                    None => Ok(false),
                }
            };
            let has64 = check(first64)?;
            let has32 = check(first32)?;
            (has32, has64)
        };
        let get = |abi: Option<&str>, bits| -> Result<String, String> {
            abi.map(str::to_owned)
                .ok_or_else(|| format!("bundled {bits}-bit libraries without supported ABI"))
        };
        let (primary, secondary) = match (has32, has64) {
            (false, false) => (None, None),
            (true, false) => (Some(get(first32, 32)?), None),
            (false, true) => (Some(get(first64, 64)?), None),
            (true, true) => {
                let narrow = get(first32, 32)?;
                let wide = get(first64, 64)?;
                if matches!(
                    instruction_set(env.preferred_abi)?,
                    "arm64" | "x86_64" | "riscv64"
                ) {
                    (Some(wide), Some(narrow))
                } else {
                    (Some(narrow), Some(wide))
                }
            }
        };
        Ok(Self {
            primary,
            secondary,
            multi_arch_mismatch: has32
                && has64
                && !pkg.is(crate::package::pkg::booleans::MULTI_ARCH),
        })
    }

    pub fn apply(self, pkg: &mut AndroidPackage) {
        pkg.primary_cpu_abi = self.primary;
        pkg.secondary_cpu_abi = self.secondary;
    }
}

impl crate::package::write::Apks {
    pub fn bundled_abis(
        &self,
        pkg: &AndroidPackage,
        env: &NativeLibraryEnvironment<'_>,
        supported: &SupportedAbis<'_>,
    ) -> Result<BundledAbis, String> {
        BundledAbis::derive(pkg, env, supported, &|path| {
            let file =
                (self.files)(path).ok_or_else(|| format!("unmapped library inventory: {path}"))?;
            match std::fs::metadata(file) {
                Ok(_) => Ok(true),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    Ok(false)
                }
                Err(e) => Err(format!("library inventory {path}: {e}")),
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedUserAbiMismatch {
    /// None identifies the scanned package, rather than an existing member.
    pub required_by: Option<String>,
    pub required_isa: String,
    pub package: String,
    pub package_isa: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedUserAbi {
    pub primary: Option<String>,
    pub mismatches: Vec<SharedUserAbiMismatch>,
}

impl SharedUserAbi {
    /// PackageAbiHelperImpl.getAdjustedAbiForSharedUser. Members retain the
    /// shared-user owner's ArraySet order; settings with no ABI are skipped.
    pub fn derive(
        members: &[crate::package::settings::Package],
        scanned: Option<&AndroidPackage>,
    ) -> Result<Self, String> {
        let mut primary = scanned.and_then(|p| p.primary_cpu_abi.clone());
        let mut required = primary.as_deref().map(instruction_set).transpose()?;
        let mut required_by = None;
        let mut mismatches = Vec::new();
        for member in members {
            if scanned.is_some_and(|p| p.package_name == member.name) {
                continue;
            }
            let Some(abi) = member.primary_cpu_abi.as_deref() else {
                continue;
            };
            let isa = instruction_set(abi)?;
            if let Some(required) = required {
                if required != isa {
                    mismatches.push(SharedUserAbiMismatch {
                        required_by: required_by.clone(),
                        required_isa: required.into(),
                        package: member.name.clone(),
                        package_isa: isa.into(),
                    });
                }
            } else {
                primary = Some(abi.into());
                required = Some(isa);
                required_by = Some(member.name.clone());
            }
        }
        Ok(Self {
            primary,
            mismatches,
        })
    }

    /// ScanPackageUtils.applyAdjustedAbiToSharedUser: settings without a raw
    /// ABI inherit it; existing parsed members and all secondary ABIs stay as
    /// they are. Returned code paths feed the later dex/installation owner.
    pub fn apply(
        &self,
        members: &mut [crate::package::settings::Package],
        parsed: &std::collections::BTreeMap<String, AndroidPackage>,
        scanned: Option<&mut AndroidPackage>,
    ) -> Vec<String> {
        let scanned_name = scanned.as_ref().map(|p| p.package_name.clone());
        if let Some(scanned) = scanned {
            scanned.primary_cpu_abi = self.primary.clone();
        }
        let mut changed = Vec::new();
        for member in members {
            if scanned_name.as_deref() == Some(&member.name) || member.primary_cpu_abi.is_some() {
                continue;
            }
            member.primary_cpu_abi = self.primary.clone();
            if parsed
                .get(&member.name)
                .is_some_and(|p| p.primary_cpu_abi != self.primary)
            {
                changed.push(member.code_path.clone());
            }
        }
        changed
    }
}

fn instruction_set(abi: &str) -> Result<&'static str, String> {
    match abi {
        "armeabi" | "armeabi-v7a" => Ok("arm"),
        "arm64-v8a" | "arm64-v8a-hwasan" => Ok("arm64"),
        "x86" => Ok("x86"),
        "x86_64" => Ok("x86_64"),
        "riscv64" => Ok("riscv64"),
        _ => Err(format!("unsupported ABI: {abi}")),
    }
}

fn guest_path(path: &str) -> Result<String, String> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(format!("not an absolute guest path: {path:?}"));
    }
    // java.io.File normalizes repeated/trailing separators, retaining dots.
    let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
    Ok(format!("/{}", parts.join("/")))
}

fn join(parent: &str, child: &str) -> String {
    format!("{}/{child}", parent.trim_end_matches('/'))
}

fn bundled_root(source: &str, canonical: Option<&str>) -> Result<String, String> {
    for root in [
        "/system",
        "/oem",
        "/vendor",
        "/odm",
        "/product",
        "/system_ext",
    ] {
        if source == root
            || source
                .strip_prefix(root)
                .is_some_and(|p| p.starts_with('/'))
        {
            return Ok(root.into());
        }
    }
    if source == "/apex" || source.starts_with("/apex/") {
        return Ok(source.split('/').take(3).collect::<Vec<_>>().join("/"));
    }
    let canonical =
        guest_path(canonical.ok_or("unknown bundled root requires canonical guest source")?)?;
    let first = canonical
        .split('/')
        .nth(1)
        .filter(|s| !s.is_empty())
        .ok_or("canonical source has no root segment")?;
    Ok(format!("/{first}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env() -> NativeLibraryEnvironment<'static> {
        NativeLibraryEnvironment {
            preferred_abi: "arm64-v8a",
            app_lib32_install_dir: "/data/app-lib",
            code_is_directory: false,
            canonical_source: None,
        }
    }
    #[test]
    fn shared_user_abi_preserves_existing_members_and_reports_isa_conflicts() {
        use crate::package::settings::Package;
        let mut members = vec![
            Package {
                name: "a".into(),
                primary_cpu_abi: Some("armeabi".into()),
                ..Default::default()
            },
            Package {
                name: "b".into(),
                primary_cpu_abi: Some("armeabi-v7a".into()),
                ..Default::default()
            },
            Package {
                name: "c".into(),
                primary_cpu_abi: Some("arm64-v8a".into()),
                ..Default::default()
            },
            Package {
                name: "d".into(),
                code_path: "/system/app/d".into(),
                secondary_cpu_abi: Some("x86_64".into()),
                ..Default::default()
            },
        ];
        let mut scanned = AndroidPackage {
            package_name: "a".into(),
            secondary_cpu_abi: Some("x86".into()),
            ..Default::default()
        };
        let choice = SharedUserAbi::derive(&members, Some(&scanned)).unwrap();
        assert_eq!(choice.primary.as_deref(), Some("armeabi-v7a"));
        assert_eq!(
            choice.mismatches,
            vec![SharedUserAbiMismatch {
                required_by: Some("b".into()),
                required_isa: "arm".into(),
                package: "c".into(),
                package_isa: "arm64".into(),
            }]
        );
        let parsed = std::collections::BTreeMap::from([("d".into(), AndroidPackage::default())]);
        assert_eq!(
            choice.apply(&mut members, &parsed, Some(&mut scanned)),
            vec!["/system/app/d"]
        );
        assert_eq!(members[0].primary_cpu_abi.as_deref(), Some("armeabi"));
        assert_eq!(members[2].primary_cpu_abi.as_deref(), Some("arm64-v8a"));
        assert_eq!(members[3].primary_cpu_abi, choice.primary);
        assert_eq!(members[3].secondary_cpu_abi.as_deref(), Some("x86_64"));
        assert_eq!(scanned.primary_cpu_abi, choice.primary);
        assert_eq!(scanned.secondary_cpu_abi.as_deref(), Some("x86"));
        assert_eq!(parsed["d"].primary_cpu_abi, None);
        let choice = SharedUserAbi::derive(
            &members,
            Some(&AndroidPackage {
                primary_cpu_abi: Some("arm64-v8a".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(choice.primary.as_deref(), Some("arm64-v8a"));
        assert_eq!(choice.mismatches.len(), 3);
        assert!(choice.mismatches.iter().all(|m| m.required_by.is_none()));
    }

    #[test]
    fn shared_user_abi_rejects_unknown_isa_and_handles_no_requirement() {
        use crate::package::settings::Package;
        let mut members = vec![Package {
            name: "a".into(),
            primary_cpu_abi: Some("invalid".into()),
            ..Default::default()
        }];
        let before = members.clone();
        assert!(SharedUserAbi::derive(&members, None).is_err());
        assert_eq!(members, before);
        let mut scanned = AndroidPackage {
            package_name: "a".into(),
            ..Default::default()
        };
        let none = SharedUserAbi::derive(&members, Some(&scanned)).unwrap();
        assert_eq!(none.primary, None);
        assert!(
            none.apply(&mut members, &Default::default(), Some(&mut scanned))
                .is_empty()
        );
        assert_eq!(members, before);
        members[0].primary_cpu_abi = None;
        let none = SharedUserAbi::derive(&members, None).unwrap();
        let parsed = std::collections::BTreeMap::from([(
            "a".into(),
            AndroidPackage {
                primary_cpu_abi: Some("arm64-v8a".into()),
                ..Default::default()
            },
        )]);
        assert_eq!(none.apply(&mut members, &parsed, None), vec![String::new()]);
        assert_eq!(members[0].primary_cpu_abi, None);
    }

    #[test]
    fn bundled_inventory_selects_preferred_abis_and_reports_multiarch_mismatch() {
        let bit32 = vec!["armeabi-v7a".into(), "armeabi".into()];
        let bit64 = vec!["arm64-v8a".into(), "x86_64".into()];
        let supported = SupportedAbis {
            bit32: &bit32,
            bit64: &bit64,
        };
        for monolithic in [false, true] {
            for has32 in [false, true] {
                for has64 in [false, true] {
                    for prefer64 in [false, true] {
                        for multi_arch in [false, true] {
                            let code = if monolithic {
                                "/system/app/fixture.apk"
                            } else {
                                "/system/app/fixture"
                            };
                            let mut pkg = AndroidPackage {
                                path: Some(code.into()),
                                base_apk_path: Some(if monolithic {
                                    code.into()
                                } else {
                                    format!("{code}/base.apk")
                                }),
                                ..Default::default()
                            };
                            if multi_arch {
                                pkg.booleans |= crate::package::pkg::booleans::MULTI_ARCH;
                            }
                            let mut config = env();
                            config.preferred_abi =
                                if prefer64 { "arm64-v8a" } else { "armeabi-v7a" };
                            let result = BundledAbis::derive(&pkg, &config, &supported, &|path| {
                                Ok(if path.contains("/lib64/") || path.ends_with("/arm64") {
                                    has64
                                } else {
                                    has32
                                })
                            })
                            .unwrap();
                            let expected = match (has32, has64, prefer64) {
                                (false, false, _) => (None, None),
                                (true, false, _) => (Some("armeabi-v7a"), None),
                                (false, true, _) => (Some("arm64-v8a"), None),
                                (true, true, true) => (Some("arm64-v8a"), Some("armeabi-v7a")),
                                (true, true, false) => (Some("armeabi-v7a"), Some("arm64-v8a")),
                            };
                            assert_eq!(
                                (result.primary.as_deref(), result.secondary.as_deref()),
                                expected
                            );
                            assert_eq!(result.multi_arch_mismatch, has32 && has64 && !multi_arch);
                            result.apply(&mut pkg);
                            assert_eq!(
                                (
                                    pkg.primary_cpu_abi.as_deref(),
                                    pkg.secondary_cpu_abi.as_deref()
                                ),
                                expected
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bundled_missing_abi_lists_and_inventory_errors_reject_without_mutation() {
        let pkg = AndroidPackage {
            path: Some("/system/app/fixture.apk".into()),
            base_apk_path: Some("/system/app/fixture.apk".into()),
            ..Default::default()
        };
        let before = pkg.clone();
        let supported = SupportedAbis {
            bit32: &[],
            bit64: &[],
        };
        assert!(BundledAbis::derive(&pkg, &env(), &supported, &|_| Ok(true)).is_err());
        assert_eq!(
            BundledAbis::derive(
                &pkg,
                &env(),
                &supported,
                &|_| Err("inventory denied".into())
            ),
            Err("inventory denied".into())
        );
        let none = BundledAbis::derive(&pkg, &env(), &supported, &|_| Ok(false)).unwrap();
        assert_eq!((none.primary, none.secondary), (None, None));
        assert_eq!(pkg, before);
    }

    #[test]
    fn cluster_uses_selected_or_preferred_isa_and_secondary_abi() {
        let mut pkg = AndroidPackage {
            path: Some("/data/app/pkg".into()),
            ..Default::default()
        };
        let paths = NativeLibraryPaths::derive(&pkg, &env(), true, false).unwrap();
        assert_eq!(paths.primary, "/data/app/pkg/lib/arm64");
        assert!(paths.requires_isa);
        pkg.primary_cpu_abi = Some("armeabi-v7a".into());
        pkg.secondary_cpu_abi = Some("arm64-v8a-hwasan".into());
        let paths = NativeLibraryPaths::derive(&pkg, &env(), false, false).unwrap();
        assert_eq!(paths.primary, "/data/app/pkg/lib/arm");
        assert_eq!(paths.secondary.as_deref(), Some("/data/app/pkg/lib/arm64"));
        paths.clone().apply(&mut pkg);
        assert_eq!(
            pkg.native_library_root_dir.as_deref(),
            Some(paths.root.as_str())
        );
        assert_eq!(
            pkg.native_library_dir.as_deref(),
            Some(paths.primary.as_str())
        );
    }
    #[test]
    fn bundled_roots_update_paths_and_monolithic_secondary_rules() {
        for root in [
            "/system",
            "/oem",
            "/vendor",
            "/odm",
            "/product",
            "/system_ext",
            "/apex/module",
        ] {
            let path = format!("{root}/app/fixture.apk");
            let mut pkg = AndroidPackage {
                path: Some(path.clone()),
                base_apk_path: Some(path),
                secondary_cpu_abi: Some("armeabi-v7a".into()),
                ..Default::default()
            };
            let paths = NativeLibraryPaths::derive(&pkg, &env(), true, false).unwrap();
            assert_eq!(paths.root, format!("{root}/lib64/fixture"));
            assert_eq!(paths.secondary, Some(format!("{root}/lib/fixture")));
            assert!(!paths.requires_isa);
            pkg.primary_cpu_abi = Some("x86".into());
            assert_eq!(
                NativeLibraryPaths::derive(&pkg, &env(), true, false)
                    .unwrap()
                    .root,
                format!("{root}/lib/fixture")
            );
            for system in [false, true] {
                let paths = NativeLibraryPaths::derive(&pkg, &env(), system, true).unwrap();
                assert_eq!(paths.root, "/data/app-lib/fixture");
                assert_eq!(paths.secondary, None);
            }
        }
    }
    #[test]
    fn unknown_roots_need_guest_canonical_input_and_invalid_inputs_do_not_mutate() {
        let pkg = AndroidPackage {
            path: Some("/custom/app/fixture.apk".into()),
            base_apk_path: Some("/custom/app/fixture.apk".into()),
            ..Default::default()
        };
        let before = pkg.clone();
        assert!(NativeLibraryPaths::derive(&pkg, &env(), true, false).is_err());
        let mut config = env();
        config.canonical_source = Some("/vendor-other/app/fixture.apk");
        assert_eq!(
            NativeLibraryPaths::derive(&pkg, &config, true, false)
                .unwrap()
                .root,
            "/vendor-other/lib64/fixture"
        );
        config.preferred_abi = "bad";
        assert!(NativeLibraryPaths::derive(&pkg, &config, true, false).is_err());
        assert_eq!(pkg, before);
        // Unbundled monolithic paths do not consult either ABI.
        assert_eq!(
            NativeLibraryPaths::derive(&pkg, &config, false, false)
                .unwrap()
                .root,
            "/data/app-lib/fixture"
        );
        assert_eq!(
            guest_path("/data//app/./pkg/../").unwrap(),
            "/data/app/./pkg/.."
        );
        assert!(guest_path("relative").is_err());
    }
}
