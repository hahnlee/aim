//! Native scan candidates for shared libraries (#707, cycles #799). File ordering,
//! APK-only dependency edges and static-library user installation follow
//! android-16.0.0_r1 SharedLibrariesImpl.executeSharedLibrariesUpdateLPw.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::*;
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
pub struct GraphError {
    pub package: String,
    pub cause: ResolveError,
}

/// A fully computed candidate. The owner publishes it only after the
/// scan validates all packages; no input state or persistence is changed.
#[derive(Debug)]
pub struct Resolved {
    pub registry: Registry,
    pub packages: BTreeMap<String, PackageState>,
}

impl Registry {
    pub fn resolve(
        &self,
        available: &BTreeMap<String, PackageState>,
        policy: &dyn Fn(&PackageState) -> Result<Policy, ResolveError>,
    ) -> Result<Resolved, GraphError> {
        let mut selections = BTreeMap::new();
        for (name, ps) in available {
            let Some(pkg) = &ps.pkg else {
                if ps.parcel.is_some() {
                    return Err(error(
                        name,
                        ResolveError::Incomplete("unparsed package APK"),
                    ));
                }
                continue;
            };
            let selection = policy(ps)
                .and_then(|p| self.collect(pkg, available, p))
                .map_err(|e| error(name, e))?;
            selections.insert(name.clone(), selection);
        }
        let mut packages = available.clone();
        let mut done = BTreeSet::new();
        let mut active = BTreeSet::new();
        for name in selections.keys() {
            resolve_files(name, &selections, &mut packages, &mut active, &mut done)?;
        }
        let mut registry = self.clone();
        for versions in registry.entries.values_mut() {
            for library in versions.values_mut() {
                library.dependencies.clear();
                if let Some(provider) = &library.package_name {
                    let selection = selections.get(provider).ok_or_else(|| {
                        error(
                            provider,
                            ResolveError::Incomplete("library provider selection"),
                        )
                    })?;
                    for dependency in &selection.libraries {
                        if dependency.path.is_none() {
                            let ps = packages
                                .get(dependency.package_name.as_ref().ok_or_else(|| {
                                    error(
                                        provider,
                                        ResolveError::Incomplete("dependency provider name"),
                                    )
                                })?)
                                .ok_or_else(|| {
                                    error(provider, ResolveError::Incomplete("dependency provider"))
                                })?;
                            if ps.pkg.is_some() {
                                library.dependencies.push(dependency.clone());
                            }
                        }
                    }
                }
            }
        }
        // Copy nested dependency records from the completed graph.
        let source = registry.clone();
        for versions in registry.entries.values_mut() {
            for library in versions.values_mut() {
                *library = expand(library, &source);
            }
        }
        for (name, selection) in selections {
            let infos = selection
                .libraries
                .iter()
                .map(|l| {
                    registry
                        .get(l.name.as_ref().unwrap(), l.version)
                        .unwrap()
                        .clone()
                })
                .collect();
            packages.get_mut(&name).unwrap().uses_library_infos = infos;
            let users: Vec<_> = packages[&name]
                .users
                .iter()
                .filter_map(|(id, s)| s.installed.then_some(*id))
                .collect();
            for library in selection.libraries.iter().filter(|l| l.kind == TYPE_STATIC) {
                let provider_name = library.package_name.as_ref().ok_or_else(|| {
                    error(
                        &name,
                        ResolveError::Incomplete("static library provider name"),
                    )
                })?;
                let provider = packages.get_mut(provider_name).ok_or_else(|| {
                    error(&name, ResolveError::Incomplete("static library provider"))
                })?;
                for user in &users {
                    provider
                        .users
                        .get_mut(user)
                        .ok_or_else(|| {
                            error(&name, ResolveError::Incomplete("static library user state"))
                        })?
                        .installed = true;
                }
            }
        }
        Ok(Resolved { registry, packages })
    }
}

fn resolve_files(
    name: &str,
    selections: &BTreeMap<String, Selection>,
    packages: &mut BTreeMap<String, PackageState>,
    active: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<(), GraphError> {
    if done.contains(name) {
        return Ok(());
    }
    if !active.insert(name.to_owned()) {
        return Err(error(
            name,
            ResolveError::Incomplete("cyclic library provider scan order"),
        ));
    }
    let selection = selections
        .get(name)
        .ok_or_else(|| error(name, ResolveError::Incomplete("library provider selection")))?;
    for lib in &selection.libraries {
        if lib.path.is_none() {
            let provider = lib
                .package_name
                .as_deref()
                .ok_or_else(|| error(name, ResolveError::Incomplete("library provider name")))?;
            resolve_files(provider, selections, packages, active, done)?;
        }
    }
    let files = selection.files(packages).map_err(|e| error(name, e))?;
    packages.get_mut(name).unwrap().uses_library_files = files;
    active.remove(name);
    done.insert(name.to_owned());
    Ok(())
}

fn expand(library: &SharedLibrary, registry: &Registry) -> SharedLibrary {
    let mut out = library.clone();
    out.dependencies = library
        .dependencies
        .iter()
        .map(|dependency| {
            expand(
                registry
                    .get(dependency.name.as_ref().unwrap(), dependency.version)
                    .unwrap(),
                registry,
            )
        })
        .collect();
    out
}

fn error(package: &str, cause: ResolveError) -> GraphError {
    GraphError {
        package: package.into(),
        cause,
    }
}
