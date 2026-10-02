//! Shared library identities built by the native scan (#707). Ported
//! from AOSP android-16.0.0_r1 `SharedLibrariesImpl` and
//! `AndroidPackageUtils`, Copyright (C) The Android Open Source Project,
//! Apache License 2.0. Dependency resolution is separate from declaration
//! registration; these records initially have no consumers or edges.

use std::collections::BTreeMap;

use super::apps_filter::NotModelled;
use super::model::{PackageState, SharedLibrary};
use super::system_config::SystemConfig;

pub const VERSION_UNDEFINED: i64 = -1;
pub const TYPE_BUILTIN: i32 = 0;
pub const TYPE_DYNAMIC: i32 = 1;
pub const TYPE_STATIC: i32 = 2;
pub const TYPE_SDK_PACKAGE: i32 = 3;

/// SharedLibrariesImpl's name/version map. Scan order matters: dynamic
/// declarations cannot replace an existing built-in or dynamic library.
#[derive(Debug, Default)]
pub struct Registry {
    entries: BTreeMap<String, BTreeMap<i64, SharedLibrary>>,
}

impl Registry {
    pub fn new(config: &SystemConfig) -> Self {
        let mut registry = Self::default();
        for entry in config.libraries.values() {
            registry.insert(SharedLibrary {
                path: Some(entry.filename.clone()),
                name: Some(entry.name.clone()),
                version: VERSION_UNDEFINED,
                kind: TYPE_BUILTIN,
                native: entry.native,
                declaring: ("android".into(), 0),
                ..Default::default()
            });
        }
        registry
    }

    pub fn get(&self, name: &str, version: i64) -> Option<&SharedLibrary> {
        self.entries.get(name)?.get(&version)
    }

    pub fn entries(&self) -> impl Iterator<Item = &SharedLibrary> {
        self.entries.values().flat_map(|versions| versions.values())
    }

    /// Registers declarations after the scan validated the package and
    /// selected its internal name. An update may expose only dynamic
    /// libraries that its disabled system package originally declared.
    pub fn add_package(
        &mut self,
        ps: &PackageState,
        disabled: Option<&PackageState>,
    ) -> Result<(), NotModelled> {
        let Some(pkg) = &ps.pkg else {
            return if ps.parcel.is_some() {
                Err(NotModelled("a library package that does not parse"))
            } else {
                Ok(())
            };
        };
        let sdk = pkg.sdk_library_name.as_ref().filter(|n| !n.is_empty());
        let static_name = pkg
            .static_shared_library_name
            .as_ref()
            .filter(|n| !n.is_empty());
        let (names, version, kind) = if let Some(name) = sdk {
            (
                vec![name.clone()],
                i64::from(pkg.sdk_lib_version_major),
                TYPE_SDK_PACKAGE,
            )
        } else if let Some(name) = static_name {
            (
                vec![name.clone()],
                pkg.static_shared_lib_version,
                TYPE_STATIC,
            )
        } else if ps.is.system && !pkg.library_names.is_empty() {
            let allowed = if ps.is.updated_system_app {
                match disabled {
                    Some(old) => old.pkg.as_ref(),
                    None => {
                        return Err(NotModelled("an updated library's disabled system package"));
                    }
                }
            } else {
                Some(pkg)
            };
            let names = pkg
                .library_names
                .iter()
                .filter(|n| {
                    allowed.is_some_and(|old| old.library_names.contains(n))
                        && self.get(n, VERSION_UNDEFINED).is_none()
                })
                .cloned()
                .collect();
            (names, VERSION_UNDEFINED, TYPE_DYNAMIC)
        } else {
            return Ok(());
        };
        if names.is_empty() {
            return Ok(());
        }
        let base = pkg
            .base_apk_path
            .as_ref()
            .ok_or(NotModelled("a library's base APK path"))?;
        let mut paths = vec![base.clone()];
        if let Some(splits) = &pkg.split_code_paths {
            for path in splits {
                paths.push(
                    path.clone()
                        .ok_or(NotModelled("a library's null split APK path"))?,
                );
            }
        }
        let declaring = if kind == TYPE_DYNAMIC {
            &ps.name
        } else {
            pkg.manifest_package_name
                .as_ref()
                .ok_or(NotModelled("a library's manifest package name"))?
        };
        let package_version =
            (i64::from(pkg.version_code_major) << 32) | i64::from(pkg.version_code as u32);
        for name in names {
            self.insert(SharedLibrary {
                package_name: Some(ps.name.clone()),
                code_paths: Some(paths.clone()),
                name: Some(name),
                version,
                kind,
                declaring: (declaring.clone(), package_version),
                ..Default::default()
            });
        }
        Ok(())
    }

    fn insert(&mut self, library: SharedLibrary) {
        self.entries
            .entry(library.name.clone().unwrap())
            .or_default()
            .insert(library.version, library);
    }
}

#[cfg(test)]
mod tests;
