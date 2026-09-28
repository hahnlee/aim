//! Physical mapping lease. Android dlopen references and unload eligibility
//! remain the linker policy owner's responsibility, not Rust Arc counts.
use super::*;

pub struct RetainedElfMapping {
    lease: mappings::MappingLease,
}
impl RetainedElfMapping {
    pub fn lookup_exported(&self, symbol: &str) -> Result<usize, LoadError> {
        self.lease.image().lookup_exported(symbol)
    }
    pub fn call_exported_i32(&self, symbol: &str) -> Result<i32, LoadError> {
        self.lease.image().call_exported_i32(symbol)
    }
}
impl LoadedElfGraph {
    /// Retain one original mapping group and its physical dependencies. This
    /// lease does not keep unrelated local groups alive. Dependency cycles
    /// share one lifetime component without merging Android group identities.
    /// External/native resources are still conservatively retained together.
    /// Explicit unsafe graph finalization must not occur while this is used.
    pub fn retain_mapping(&self, soname: &str) -> Option<RetainedElfMapping> {
        let &index = self.inner.indices.get(soname)?;
        Some(RetainedElfMapping {
            lease: self.inner.objects.retain(index)?,
        })
    }
}
