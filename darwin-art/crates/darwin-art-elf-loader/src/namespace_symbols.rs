//! Ordered ELF symbol lookup, separate from graph mapping/lifecycle ownership.
use super::*;
#[cfg(test)]
#[path = "namespace_mixed_symbols_tests.rs"]
mod mixed_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn export(address: usize, hidden: bool) -> ExportedSymbol {
        ExportedSymbol {
            name: b"shared_symbol".to_vec(),
            address,
            version: None,
            version_hidden: hidden,
            version_index: None,
        }
    }
    #[test]
    fn global_bindings_preserve_original_slot_and_deduplicate_winners() {
        // Identical symbol/address values do not determine ownership. The
        // catalog's admitted source determines resident versus newly global.
        for source in [BindingSource::Resident(7), BindingSource::Local(4)] {
            let catalogs = vec![vec![export(17, false)]];
            let sources = [source];
            let names = vec!["requester".into()];
            let scopes = vec![vec![]];
            let providers = HashSet::new();
            let mut external = RejectAllResolver;
            let mut resolver = GraphResolver {
                global_sources: &sources,
                bindings: Vec::new(),
                global_catalog: &catalogs,
                global_versions: &[None],
                requester: 0,
                object_sonames: &names,
                scopes: &scopes,
                catalog: &[],
                versions: &[],
                provider_sonames: &providers,
                external: &mut external,
            };
            assert!(
                resolver
                    .resolve(SymbolRequest {
                        symbol: "missing",
                        needed_libraries: &[],
                        version: None,
                        is_weak: true,
                    })
                    .unwrap()
                    .is_none()
            );
            assert!(resolver.bindings.is_empty());
            assert_eq!(
                resolver
                    .resolve_defined(b"shared_symbol", None)
                    .unwrap()
                    .unwrap()
                    .address(),
                17
            );
            assert_eq!(
                resolver
                    .resolve(SymbolRequest {
                        symbol: "shared_symbol",
                        needed_libraries: &[],
                        version: None,
                        is_weak: false,
                    })
                    .unwrap()
                    .unwrap()
                    .address(),
                17
            );
            assert_eq!(resolver.bindings, [source]);
        }
    }
    #[test]
    fn first_eligible_definition_wins_even_when_weak() {
        let names = vec!["first.so".into(), "second.so".into()];
        let scopes = vec![vec![0, 1], vec![1, 0]];
        let catalog = vec![vec![export(17, false)], vec![export(29, false)]];
        let providers = HashSet::new();
        let versions = vec![None; catalog.len()];
        let mut external = RejectAllResolver;
        let mut resolver = GraphResolver {
            global_sources: &[],
            bindings: Vec::new(),
            global_catalog: &[],
            global_versions: &[],
            requester: 0,
            object_sonames: &names,
            scopes: &scopes,
            catalog: &catalog,
            versions: &versions,
            provider_sonames: &providers,
            external: &mut external,
        };
        assert_eq!(
            resolver.graph_lookup("shared_symbol", None, false).unwrap(),
            Some(17)
        );
        resolver.requester = 1;
        assert_eq!(
            resolver.graph_lookup("shared_symbol", None, false).unwrap(),
            Some(29)
        );
    }
    #[test]
    fn hidden_default_definition_does_not_stop_search() {
        let names = vec!["first.so".into(), "second.so".into()];
        let scopes = vec![vec![0, 1]];
        let catalog = vec![vec![export(17, true)], vec![export(29, false)]];
        let providers = HashSet::new();
        let versions = vec![None; catalog.len()];
        let mut external = RejectAllResolver;
        let mut resolver = GraphResolver {
            global_sources: &[],
            bindings: Vec::new(),
            global_catalog: &[],
            global_versions: &[],
            requester: 0,
            object_sonames: &names,
            scopes: &scopes,
            catalog: &catalog,
            versions: &versions,
            provider_sonames: &providers,
            external: &mut external,
        };
        assert_eq!(
            resolver.graph_lookup("shared_symbol", None, false).unwrap(),
            Some(29)
        );
    }

    #[test]
    fn versioned_import_accepts_unversioned_dso_without_versym_like_bionic() {
        let names = vec!["consumer.so".into(), "libnativehelper.so".into()];
        let scopes = vec![vec![0, 1], vec![1]];
        let catalog = vec![vec![], vec![export(29, false)]];
        let versions = vec![None; catalog.len()];
        let providers = HashSet::new();
        let mut external = RejectAllResolver;
        let mut resolver = GraphResolver {
            global_sources: &[],
            bindings: Vec::new(),
            global_catalog: &[],
            global_versions: &[],
            requester: 0,
            object_sonames: &names,
            scopes: &scopes,
            catalog: &catalog,
            versions: &versions,
            provider_sonames: &providers,
            external: &mut external,
        };
        assert_eq!(
            resolver
                .graph_lookup(
                    "shared_symbol",
                    Some(VersionRequirement {
                        soname: "libnativehelper.so",
                        name: "LIBNATIVEHELPER_S",
                        hidden: false,
                        flags: 0,
                    }),
                    false,
                )
                .unwrap(),
            Some(29)
        );
    }

    #[test]
    fn native_provider_keeps_its_position_before_later_elf_definition() {
        // root DT_NEEDED order is native.so, later.so. A native implementation
        // of a library must occupy the same local-group slot as its ELF image.
        struct Native;
        impl SymbolResolver for Native {
            fn resolve(
                &mut self,
                _: SymbolRequest<'_>,
            ) -> Result<Option<ResolvedSymbol>, ResolveError> {
                Ok(Some(unsafe {
                    ResolvedSymbol::new(NonZeroUsize::new(17).unwrap())
                }))
            }
        }
        let providers = HashSet::from(["native.so".into()]);
        let files = vec![
            (
                "root.so".into(),
                vec!["native.so".into(), "later.so".into()],
            ),
            ("later.so".into(), vec![]),
        ];
        let (names, scopes) = super::super::lookup_layout::layout(0, &files, &providers);
        let catalog = vec![vec![], vec![export(29, false)], vec![]];
        let versions = vec![None; catalog.len()];
        let mut external = Native;
        let mut resolver = GraphResolver {
            global_sources: &[],
            bindings: Vec::new(),
            global_catalog: &[],
            global_versions: &[],
            requester: 0,
            object_sonames: &names,
            scopes: &scopes,
            catalog: &catalog,
            versions: &versions,
            provider_sonames: &providers,
            external: &mut external,
        };
        let needed = vec!["native.so".into(), "later.so".into()];
        let result = resolver
            .resolve(SymbolRequest {
                symbol: "shared_symbol",
                needed_libraries: &needed,
                version: None,
                is_weak: false,
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            result.address(),
            17,
            "native image must not be moved after all ELF images"
        );
    }
}

pub(super) struct GraphResolver<'a> {
    pub(super) global_sources: &'a [BindingSource],
    pub(super) bindings: Vec<BindingSource>,
    pub(super) global_catalog: &'a [Vec<ExportedSymbol>],
    pub(super) global_versions: &'a [Option<HashMap<u16, String>>],
    pub(super) requester: usize,
    pub(super) object_sonames: &'a [String],
    pub(super) scopes: &'a [Vec<usize>],
    pub(super) catalog: &'a [Vec<ExportedSymbol>],
    pub(super) versions: &'a [Option<HashMap<u16, String>>],
    pub(super) provider_sonames: &'a HashSet<String>,
    pub(super) external: &'a mut dyn SymbolResolver,
}

/// Original admitted slot identity; global catalogs may contain either a
/// previously retained image or a newly mapped DF_1_GLOBAL image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BindingSource {
    Local(usize),
    Resident(usize),
}

impl GraphResolver<'_> {
    fn global_lookup(&mut self, name: &[u8], version: Option<&str>) -> Option<usize> {
        for (index, catalog) in self.global_catalog.iter().enumerate() {
            if let Some(address) = android_versions::selected_export(
                catalog,
                self.global_versions[index].as_ref(),
                name,
                version,
            ) {
                let source = self.global_sources[index];
                if !self.bindings.contains(&source) {
                    self.bindings.push(source);
                }
                return Some(address);
            }
        }
        None
    }
    fn graph_lookup(
        &mut self,
        symbol: &str,
        version: Option<VersionRequirement<'_>>,
        is_weak: bool,
    ) -> Result<Option<usize>, ResolveError> {
        let scope = &self.scopes[self.requester];
        let candidate_objects: Vec<usize> = match version {
            Some(requirement) => {
                let Some(index) = scope
                    .iter()
                    .copied()
                    .find(|&index| self.object_sonames[index] == requirement.soname)
                else {
                    return Err(ResolveError::UnknownSoname(requirement.soname.to_owned()));
                };
                vec![index]
            }
            None => scope.clone(),
        };
        for index in candidate_objects {
            if self.provider_sonames.contains(&self.object_sonames[index]) {
                let needed = std::slice::from_ref(&self.object_sonames[index]);
                if let Some(result) = self.external.resolve(SymbolRequest {
                    symbol,
                    needed_libraries: needed,
                    version,
                    is_weak,
                })? {
                    if !self.bindings.contains(&BindingSource::Local(index)) {
                        self.bindings.push(BindingSource::Local(index));
                    }
                    return Ok(Some(result.address()));
                }
                continue;
            }
            if let Some(address) = android_versions::selected_export(
                &self.catalog[index],
                self.versions[index].as_ref(),
                symbol.as_bytes(),
                version.map(|requirement| requirement.name),
            ) {
                // AOSP dynamic lookup takes the first matching global/weak
                // definition. A later strong definition must not override it.
                if !self.bindings.contains(&BindingSource::Local(index)) {
                    self.bindings.push(BindingSource::Local(index));
                }
                return Ok(Some(address));
            }
        }
        if let Some(requirement) = version {
            let provider_has_symbol = scope.iter().copied().any(|index| {
                self.object_sonames[index] == requirement.soname
                    && self.catalog[index]
                        .iter()
                        .any(|export| export.name == symbol.as_bytes())
            });
            if provider_has_symbol {
                return Err(ResolveError::VersionMismatch {
                    soname: requirement.soname.to_owned(),
                    symbol: symbol.to_owned(),
                    requested: requirement.name.to_owned(),
                });
            }
        }
        Ok(None)
    }
}

impl SymbolResolver for GraphResolver<'_> {
    fn resolve_defined(
        &mut self,
        name: &[u8],
        version: Option<&str>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if let Some(address) = self.global_lookup(name, version) {
            let address = NonZeroUsize::new(address)
                .ok_or_else(|| ResolveError::Rejected("ELF export has null address".into()))?;
            // New graph retains the selected global images and dependencies.
            return Ok(Some(unsafe { ResolvedSymbol::new(address) }));
        }
        for &index in &self.scopes[self.requester] {
            if self.provider_sonames.contains(&self.object_sonames[index]) {
                let soname = &self.object_sonames[index];
                let symbol = std::str::from_utf8(name).map_err(|_| {
                    ResolveError::Rejected("native symbol name is not UTF-8".into())
                })?;
                if let Some(result) = self.external.resolve(SymbolRequest {
                    symbol,
                    needed_libraries: std::slice::from_ref(soname),
                    version: version.map(|name| VersionRequirement {
                        soname,
                        name,
                        hidden: false,
                        flags: 0,
                    }),
                    is_weak: false,
                })? {
                    if !self.bindings.contains(&BindingSource::Local(index)) {
                        self.bindings.push(BindingSource::Local(index));
                    }
                    return Ok(Some(result));
                }
                continue;
            }
            if let Some(address) = android_versions::selected_export(
                &self.catalog[index],
                self.versions[index].as_ref(),
                name,
                version,
            ) {
                let address = NonZeroUsize::new(address)
                    .ok_or_else(|| ResolveError::Rejected("ELF export has null address".into()))?;
                // Graph retains every mapping through relocation and all users.
                if !self.bindings.contains(&BindingSource::Local(index)) {
                    self.bindings.push(BindingSource::Local(index));
                }
                return Ok(Some(unsafe { ResolvedSymbol::new(address) }));
            }
        }
        Err(ResolveError::Rejected(
            "definition absent from its local group".into(),
        ))
    }
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if let Some(address) = self.global_lookup(
            request.symbol.as_bytes(),
            request.version.map(|version| version.name),
        ) {
            let address = NonZeroUsize::new(address)
                .ok_or_else(|| ResolveError::Rejected("ELF export has null address".into()))?;
            return Ok(Some(unsafe { ResolvedSymbol::new(address) }));
        }
        if let Some(address) =
            self.graph_lookup(request.symbol, request.version, request.is_weak)?
        {
            let address = NonZeroUsize::new(address)
                .ok_or_else(|| ResolveError::Rejected("ELF export has null address".to_owned()))?;
            // SAFETY: the graph owns the provider mapping until every graph handle is dropped.
            return Ok(Some(unsafe { ResolvedSymbol::new(address) }));
        }
        Ok(None)
    }
}
