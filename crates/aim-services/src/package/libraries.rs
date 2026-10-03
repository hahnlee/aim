//! Shared library identities built by the native scan (#707). Ported
//! from AOSP android-16.0.0_r1 `SharedLibrariesImpl`, `AndroidPackageUtils`
//! and `RemovePackageHelper`, Copyright (C) The Android Open Source Project,
//! Apache License 2.0. Dependency resolution is separate from declaration
//! registration; these records initially have no consumers or edges.

use std::collections::BTreeMap;

use super::apps_filter::NotModelled;
use super::model::{PackageState, SharedLibrary};
use super::system_config::SystemConfig;

mod resolve;
pub use resolve::{Policy, ResolveError, Selection};
mod graph;
pub use graph::{GraphError, Resolved, ScanResolved};
mod policy;
pub use policy::{NativePolicyError, native_dependencies_enforced};

pub const VERSION_UNDEFINED: i64 = -1;
pub const TYPE_BUILTIN: i32 = 0;
pub const TYPE_DYNAMIC: i32 = 1;
pub const TYPE_STATIC: i32 = 2;
pub const TYPE_SDK_PACKAGE: i32 = 3;

/// Library resolution inputs owned by a native scan, without a query facade.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanPackage {
    pub code: std::sync::Arc<super::pkg::AndroidPackage>,
    pub signatures: Option<super::settings::Signatures>,
    pub users: BTreeMap<i32, super::restrictions::UserState>,
    pub uses_library_files: Vec<String>,
    pub uses_library_infos: Vec<SharedLibrary>,
}

trait LibraryPackage: Clone {
    fn code(&self) -> Option<&super::pkg::AndroidPackage>;
    fn unparsed_code(&self) -> bool;
    fn signatures(&self) -> Option<&super::settings::Signatures>;
    fn files(&self) -> &[String];
    fn set_files(&mut self, files: Vec<String>);
    fn set_infos(&mut self, infos: Vec<SharedLibrary>);
    fn installed_users(&self) -> Vec<i32>;
    fn install_for_user(&mut self, id: i32) -> Result<(), ResolveError>;
}

impl LibraryPackage for PackageState {
    fn code(&self) -> Option<&super::pkg::AndroidPackage> {
        self.pkg.as_deref()
    }
    fn unparsed_code(&self) -> bool {
        self.parcel.is_some() && self.pkg.is_none()
    }
    fn signatures(&self) -> Option<&super::settings::Signatures> {
        self.signatures.as_ref()
    }
    fn files(&self) -> &[String] {
        &self.uses_library_files
    }
    fn set_files(&mut self, files: Vec<String>) {
        self.uses_library_files = files;
    }
    fn set_infos(&mut self, infos: Vec<SharedLibrary>) {
        self.uses_library_infos = infos;
    }
    fn installed_users(&self) -> Vec<i32> {
        self.users
            .iter()
            .filter_map(|(id, state)| state.installed.then_some(*id))
            .collect()
    }
    fn install_for_user(&mut self, id: i32) -> Result<(), ResolveError> {
        self.users
            .get_mut(&id)
            .ok_or(ResolveError::Incomplete("static library user state"))?
            .installed = true;
        Ok(())
    }
}

impl LibraryPackage for ScanPackage {
    fn code(&self) -> Option<&super::pkg::AndroidPackage> {
        Some(&self.code)
    }
    fn unparsed_code(&self) -> bool {
        false
    }
    fn signatures(&self) -> Option<&super::settings::Signatures> {
        self.signatures.as_ref()
    }
    fn files(&self) -> &[String] {
        &self.uses_library_files
    }
    fn set_files(&mut self, files: Vec<String>) {
        self.uses_library_files = files;
    }
    fn set_infos(&mut self, infos: Vec<SharedLibrary>) {
        self.uses_library_infos = infos;
    }
    fn installed_users(&self) -> Vec<i32> {
        self.users
            .iter()
            .filter_map(|(id, state)| state.installed.then_some(*id))
            .collect()
    }
    fn install_for_user(&mut self, id: i32) -> Result<(), ResolveError> {
        self.users
            .get_mut(&id)
            .ok_or(ResolveError::Incomplete("static library user state"))?
            .installed = true;
        Ok(())
    }
}

/// SharedLibrariesImpl's name/version map. Scan order matters: dynamic
/// declarations cannot replace an existing built-in or dynamic library.
#[derive(Clone, Debug, Default, PartialEq)]
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

    /// SharedLibrariesImpl.getLatestStaticSharedLibraVersionLPr: the greatest
    /// nonnegative version strictly below the incoming version, not the map's
    /// absolute maximum. An absent selected setting does not fall back again.
    pub fn latest_static_setting<'a>(
        &self,
        pkg: &super::pkg::AndroidPackage,
        settings: &'a super::settings::Settings,
    ) -> Option<&'a super::settings::Package> {
        let versions = self.entries.get(pkg.static_shared_library_name.as_ref()?)?;
        let (version, library) = versions
            .range(..pkg.static_shared_lib_version)
            .next_back()?;
        if *version < 0 {
            return None;
        }
        let name = library.package_name.as_ref()?;
        settings.packages.iter().find(|p| &p.name == name)
    }

    pub fn entries(&self) -> impl Iterator<Item = &SharedLibrary> {
        self.entries.values().flat_map(|versions| versions.values())
    }

    /// RemovePackageHelper.cleanPackageDataStructuresLILPw uses exact library
    /// versions (including zero for system dynamic declarations). This scan
    /// registry does not own published dependency overlays (#798).
    pub(in crate::package) fn remove_scan_record(&mut self, record: &super::scan::Record) {
        let pkg = &record.parsed;
        if record.settings.flags & super::settings::FLAG_SYSTEM != 0 {
            for name in &pkg.library_names {
                self.remove(name, 0);
            }
        }
        if let Some(name) = &pkg.sdk_library_name {
            self.remove(name, i64::from(pkg.sdk_lib_version_major));
        }
        if let Some(name) = &pkg.static_shared_library_name {
            self.remove(name, pkg.static_shared_lib_version);
        }
    }

    fn remove(&mut self, name: &str, version: i64) {
        if let Some(versions) = self.entries.get_mut(name) {
            versions.remove(&version);
            if versions.is_empty() {
                self.entries.remove(name);
            }
        }
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
        self.add_parsed(
            &ps.name,
            pkg,
            ps.is.system,
            ps.is.updated_system_app,
            disabled.and_then(|p| p.pkg.as_deref()),
        )
    }

    /// Scan-owned declaration view; no incomplete query PackageState is built.
    pub(in crate::package) fn add_scan_record(
        &mut self,
        record: &super::scan::Record,
        disabled: Option<&super::scan::Record>,
        updated_system_app: bool,
    ) -> Result<(), NotModelled> {
        self.add_parsed(
            &record.settings.name,
            &record.parsed,
            record.settings.flags & super::settings::FLAG_SYSTEM != 0,
            updated_system_app,
            disabled.map(|r| &r.parsed),
        )
    }

    fn add_parsed(
        &mut self,
        package_name: &str,
        pkg: &super::pkg::AndroidPackage,
        system: bool,
        updated_system_app: bool,
        disabled: Option<&super::pkg::AndroidPackage>,
    ) -> Result<(), NotModelled> {
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
        } else if system && !pkg.library_names.is_empty() {
            let allowed = if updated_system_app {
                match disabled {
                    Some(old) => Some(old),
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
            package_name
        } else {
            pkg.manifest_package_name
                .as_ref()
                .ok_or(NotModelled("a library's manifest package name"))?
        };
        let package_version =
            (i64::from(pkg.version_code_major) << 32) | i64::from(pkg.version_code as u32);
        for name in names {
            self.insert(SharedLibrary {
                package_name: Some(package_name.to_owned()),
                code_paths: Some(paths.clone()),
                name: Some(name),
                version,
                kind,
                declaring: (declaring.to_owned(), package_version),
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
