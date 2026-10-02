//! Native library paths after ABI selection, ported from PackageAbiHelperImpl
//! and VMRuntime at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::pkg::AndroidPackage;

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
