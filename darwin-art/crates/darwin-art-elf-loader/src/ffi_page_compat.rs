//! Per-discovered-graph page-layout policy, separate from namespace visibility.
use super::*;

/// # Safety
/// Discovery exclusively borrowed (no concurrent discovery/load/destruction).
/// The bool is a linker-owner snapshot and is applied only by a subsequent
/// discovered-graph link; metadata discovery itself remains permissive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_set_16kb_appcompat(
    discovered: *mut DarwinArtElfDiscoveredGraph,
    enabled: bool,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        let graph = unsafe { discovered.as_mut() }.ok_or(FfiFailure::Invalid("null discovery"))?;
        graph.appcompat_16kb = Some(enabled);
        Ok(())
    })
}
