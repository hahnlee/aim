use super::*;

struct Native {
    found: bool,
    calls: usize,
}
impl SymbolResolver for Native {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        assert_eq!(request.needed_libraries, ["native.so"]);
        self.calls += 1;
        Ok(self
            .found
            .then(|| unsafe { ResolvedSymbol::new(NonZeroUsize::new(17).unwrap()) }))
    }
}

#[test]
fn mixed_order_handles_elf_first_missing_native_and_definition_preemption() {
    for (native_first, found, expected) in [(true, true, 17), (true, false, 29), (false, true, 29)]
    {
        let needed = if native_first {
            vec!["native.so".into(), "elf.so".into()]
        } else {
            vec!["elf.so".into(), "native.so".into()]
        };
        let providers = HashSet::from(["native.so".into(), "unreachable.so".into()]);
        let files = vec![("root.so".into(), needed), ("elf.so".into(), vec![])];
        let (names, scopes) = super::super::lookup_layout::layout(0, &files, &providers);
        let mut catalog = vec![
            vec![],
            vec![ExportedSymbol {
                name: b"symbol".to_vec(),
                address: 29,
                version: None,
                version_hidden: false,
                version_index: None,
            }],
        ];
        catalog.resize_with(names.len(), Vec::new);
        let versions = vec![None; catalog.len()];
        let mut native = Native { found, calls: 0 };
        {
            let mut resolver = GraphResolver {
                global_sources: &[],
                bindings: Vec::new(),
                global_catalog: &[],
                global_versions: &[],
                requester: 1,
                object_sonames: &names,
                scopes: &scopes,
                catalog: &catalog,
                versions: &versions,
                provider_sonames: &providers,
                external: &mut native,
            };
            // Requester has no direct dependency on native; the load group does.
            assert_eq!(
                resolver
                    .resolve(SymbolRequest {
                        symbol: "symbol",
                        needed_libraries: &[],
                        version: None,
                        is_weak: false
                    })
                    .unwrap()
                    .unwrap()
                    .address(),
                expected
            );
            assert_eq!(
                resolver
                    .resolve_defined(b"symbol", None)
                    .unwrap()
                    .unwrap()
                    .address(),
                expected
            );
            assert!(matches!(
                resolver.resolve(SymbolRequest {
                    symbol: "symbol",
                    needed_libraries: &[],
                    is_weak: false,
                    version: Some(VersionRequirement {
                        soname: "unreachable.so",
                        name: "V1",
                        hidden: false,
                        flags: 0
                    })
                }),
                Err(ResolveError::UnknownSoname(_))
            ));
            let chosen = if native_first && found {
                "native.so"
            } else {
                "elf.so"
            };
            assert_eq!(
                resolver.bindings,
                [BindingSource::Local(
                    names.iter().position(|name| name == chosen).unwrap()
                )]
            );
        }
        assert_eq!(native.calls, if native_first { 2 } else { 0 });
    }
}
