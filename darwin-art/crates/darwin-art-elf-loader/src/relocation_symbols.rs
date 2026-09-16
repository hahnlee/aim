//! Relocation symbol binding against the graph owner, separate from mapping.
use super::*;

impl LoadedElf {
    pub(super) fn resolve_relocation_symbol(
        &self,
        index: u32,
        symbol_count: u32,
        versions: &HashMap<u16, OwnedVersionRequirement>,
        resolved: &mut HashMap<u32, usize>,
        resolver: &mut dyn SymbolResolver,
    ) -> Result<usize, LoadError> {
        if let Some(address) = resolved.get(&index) {
            return Ok(*address);
        }
        if index == 0 {
            return Ok(0);
        }
        if index >= symbol_count {
            return Err(LoadError::Bounds("relocation symbol index"));
        }
        let symbol = self.dynamic_symbol(index)?;
        if symbol.kind == STT_TLS {
            return Err(LoadError::Capability(Capability::Tls));
        }
        if !matches!(symbol.kind, STT_NOTYPE | STT_OBJECT | STT_FUNC) {
            return Err(LoadError::InvalidSymbol(format!("dynsym[{index}] type")));
        }
        if symbol.section_index != SHN_UNDEF {
            let symbol_name = self.dynamic_string_bytes(symbol.name_offset)?;
            if symbol.section_index == SHN_ABS
                && !self.is_image_relative_absolute_marker(&symbol, symbol_name)
            {
                return Err(LoadError::Capability(Capability::AbsoluteSymbolDefinition));
            }
            let is_load_end_marker = symbol.size == 0
                && self.loads.iter().any(|load| {
                    load.virtual_address
                        .checked_add(load.memory_size)
                        .is_some_and(|end| end == symbol.value)
                });
            if !is_load_end_marker {
                self.require_loaded_range(
                    symbol.value,
                    symbol.size.max(1),
                    None,
                    "defined symbol",
                )?;
            }
            let pointer_size = usize::try_from(symbol.size)
                .map_err(|_| LoadError::Bounds("defined symbol size"))?;
            let mut address = self.loaded_pointer(symbol.value, pointer_size)? as usize;
            if matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
                && symbol.visibility == STV_DEFAULT
                && self
                    .dynamic
                    .flags
                    .is_none_or(|flags| flags & DF_SYMBOLIC == 0)
            {
                let definitions = self.parse_version_definitions(symbol_count)?;
                let (definition, _) = self.definition_for_symbol(index, &definitions)?;
                if let Some(selected) = resolver
                    .resolve_defined(
                        symbol_name,
                        definition.map(|definition| definition.name.as_str()),
                    )
                    .map_err(|source| LoadError::Resolver {
                        symbol: String::from_utf8_lossy(symbol_name).into_owned(),
                        source,
                    })?
                {
                    address = selected.address();
                }
            }
            resolved.insert(index, address);
            return Ok(address);
        }
        if !matches!(symbol.binding, STB_GLOBAL | STB_WEAK) || symbol.visibility != 0 {
            return Err(LoadError::InvalidSymbol(format!("dynsym[{index}] binding")));
        }
        let name_bytes = self.dynamic_string_bytes(symbol.name_offset)?;
        if name_bytes.is_empty() {
            return Err(LoadError::InvalidSymbol(format!(
                "dynsym[{index}] empty name"
            )));
        }
        // Undefined symbols are passed through the Rust resolver's historical
        // UTF-8 API.  Android DSOs may contain arbitrary bytes in *defined*
        // names (which are retained below), but an invalid undefined import
        // cannot be represented by this external resolver contract.
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| LoadError::InvalidSymbol(format!("dynsym[{index}] non-UTF-8 name")))?;
        let version = self.version_for_symbol(index, versions)?;
        let request_version = version.map(|(requirement, hidden)| VersionRequirement {
            soname: &requirement.soname,
            name: &requirement.name,
            hidden,
            flags: requirement.flags,
        });
        let result = resolver
            .resolve(SymbolRequest {
                symbol: name,
                needed_libraries: &self.needed_libraries,
                version: request_version,
                is_weak: symbol.binding == STB_WEAK,
            })
            .map_err(|source| LoadError::Resolver {
                symbol: name.to_owned(),
                source,
            })?;
        let address = match result {
            Some(symbol) => symbol.address(),
            None if symbol.binding == STB_WEAK => 0,
            None => {
                return Err(LoadError::UnresolvedSymbol {
                    symbol: name.to_owned(),
                    soname: version.map(|(item, _)| item.soname.clone()),
                    version: version.map(|(item, _)| item.name.clone()),
                });
            }
        };
        resolved.insert(index, address);
        Ok(address)
    }
}
