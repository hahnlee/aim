//! Library candidates from accepted native code/settings/users (#808, #836).
use super::SigningScan;
use crate::package::libraries::{GraphError, Policy, ResolveError, ScanPackage, ScanResolved};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    registry: crate::package::libraries::Registry,
    packages: BTreeMap<String, ScanPackage>,
}

impl SigningScan {
    /// Compute the complete active code graph before committing dependency
    /// state, static-library user changes and overlays at the owning stage.
    pub fn resolve_library_dependencies(
        &self,
        policy: &dyn Fn(&str, &crate::package::pkg::AndroidPackage) -> Result<Policy, ResolveError>,
    ) -> Result<ScanResolved, GraphError> {
        let available = self.library_inputs()?;
        self.libraries.resolve_scan(&available, &|package| {
            policy(&package.code.package_name, &package.code)
        })
    }

    fn library_inputs(&self) -> Result<BTreeMap<String, ScanPackage>, GraphError> {
        let error = |name: &str, input| GraphError {
            package: name.into(),
            cause: ResolveError::Incomplete(input),
        };
        if !self.capture_ready() {
            return Err(error("", "scan metadata is not finalized"));
        }
        let mut available = BTreeMap::new();
        for (name, loaded) in &self.loaded {
            let setting = self
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| error(name, "library setting owner"))?;
            if loaded.package.package_name != *name
                || loaded.package.uid != setting.app_id
                || loaded.package.path.as_deref() != Some(setting.code_path.as_str())
            {
                return Err(error(name, "library code setting identity"));
            }
            let users = self
                .scanned_user_states(name)
                .ok_or_else(|| error(name, "library user owner"))?;
            available.insert(
                name.clone(),
                ScanPackage {
                    code: Arc::new(loaded.package.clone()),
                    signatures: setting.signatures.clone(),
                    users: users.clone(),
                    // Fresh native dependency candidates, computed by the resolver.
                    uses_library_files: Vec::new(),
                    uses_library_infos: Vec::new(),
                },
            );
        }
        Ok(available)
    }

    /// Commit dependency metadata and the original static-provider installed
    /// bits together only after every package resolves successfully.
    pub fn complete_library_dependencies(
        &mut self,
        policy: &dyn Fn(&str, &crate::package::pkg::AndroidPackage) -> Result<Policy, ResolveError>,
    ) -> Result<(), GraphError> {
        let resolved = self.resolve_library_dependencies(policy)?;
        for (name, package) in &resolved.packages {
            self.scanned_users
                .insert(name.clone(), package.users.clone());
            self.update_disabled_user_aliases(name, &package.users);
        }
        self.libraries = resolved.registry.clone();
        self.library_dependencies = Some(Assignments {
            registry: resolved.registry,
            packages: resolved.packages,
        });
        Ok(())
    }

    pub(in crate::package) fn validate_library_dependencies(&self) -> Result<(), String> {
        let Some(assigned) = &self.library_dependencies else {
            return Ok(());
        };
        if !self.capture_ready()
            || self.libraries != assigned.registry
            || self.loaded.keys().ne(assigned.packages.keys())
        {
            return Err("library dependency inventory differs".into());
        }
        for (name, prior) in &assigned.packages {
            let loaded = &self.loaded[name];
            let setting = self
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("library setting owner missing: {name}"))?;
            let users = self
                .scanned_user_states(name)
                .ok_or_else(|| format!("library user owner missing: {name}"))?;
            if &loaded.package != prior.code.as_ref()
                || setting.signatures != prior.signatures
                || loaded.package.package_name != *name
                || loaded.package.uid != setting.app_id
                || loaded.package.path.as_deref() != Some(setting.code_path.as_str())
                || users.len() != prior.users.len()
                || users.iter().any(|(id, state)| {
                    prior.users.get(id).map(|p| p.installed) != Some(state.installed)
                })
            {
                return Err(format!("library dependency inputs differ: {name}"));
            }
        }
        Ok(())
    }

    pub fn library_dependencies(
        &self,
        name: &str,
    ) -> Result<Option<(&[String], &[crate::package::model::SharedLibrary])>, String> {
        self.validate_library_dependencies()?;
        let assigned = self
            .library_dependencies
            .as_ref()
            .ok_or("library dependencies are not resolved")?;
        Ok(assigned.packages.get(name).map(|package| {
            (
                package.uses_library_files.as_slice(),
                package.uses_library_infos.as_slice(),
            )
        }))
    }
}
