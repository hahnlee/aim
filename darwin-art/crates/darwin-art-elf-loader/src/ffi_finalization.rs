//! The Android group owner decides eligibility; this is its destructor phase.
use super::*;
/// # Safety
/// Handle live, no concurrent destruction. Caller satisfies LoadedElfGraph's
/// finalize contract (eligibility, operation serialization, no later library
/// use). Retain registry/mappings through callbacks; no data mutex spans them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_graph_finalize(
    handle: *const DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        let handle = unsafe { handle.as_ref() }.ok_or(FfiFailure::Invalid("null graph"))?;
        let graph = { lock_graph(handle)?.clone() };
        unsafe {
            graph.finalize();
        }
        Ok(())
    })
}
