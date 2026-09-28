//! Relocation provenance from actual resolver winners, not lookup candidates.
use super::*;

impl LoadedElfGraph {
    /// Exact mapped/global-catalog ELF images selected by successful lookups.
    /// External-provider slots (including ELF-backed providers routed through
    /// that resolver) remain recorded internally but are excluded here.
    /// DT_NEEDED edges without a relocation are separate;
    /// this list alone is therefore not a complete unload dependency closure.
    /// No namespace/SONAME re-search occurs: resident indices refer to the
    /// original retained global snapshot and local indices to this load.
    pub fn relocation_elf_dependencies(&self, soname: &str) -> Option<Vec<GlobalElfImage>> {
        let &index = self.inner.indices.get(soname)?;
        Some(
            self.inner
                .bindings
                .get(index)?
                .iter()
                .filter_map(|source| match *source {
                    symbols::BindingSource::Local(slot) => self
                        .inner
                        .binding_names
                        .get(slot)
                        .and_then(|name| self.global_image(name)),
                    symbols::BindingSource::Resident(slot) => {
                        self.inner.external.globals.get(slot).cloned()
                    }
                })
                .collect(),
        )
    }
}
