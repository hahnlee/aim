//! Clone the graph owner under the handle lock, release lock before guest code.
use super::*;

/// # Safety
/// Handle is live and not destroyed concurrently. Linker owner serializes
/// initialization and handles reentrant opens; this API rejects duplicate init.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_graph_initialize(
    handle: *const DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        let handle = unsafe { handle.as_ref() }.ok_or(FfiFailure::Invalid("null graph"))?;
        let graph = { lock_graph(handle)?.clone() };
        graph.initialize().map_err(FfiFailure::Namespace)
    })
}
