//! Original ELF flags for native linker policy; does not alter load support.
use super::*;

/// # Safety
/// Graph must remain live; soname is NUL terminated, output writable and error
/// buffer valid if supplied. Failure leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_graph_flags_1(
    graph: *const DarwinArtElfGraphHandle,
    soname: *const c_char,
    output: *mut u64,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("flags output is null"));
        }
        let graph = unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("graph is null"))?;
        let name = unsafe { required_utf8(soname, "soname") }?;
        let flags = lock_graph(graph)?
            .dynamic_flags_1(&name)
            .ok_or(FfiFailure::Invalid("image is not a graph member"))?;
        unsafe { *output = flags };
        Ok(())
    })
}

/// # Safety
/// Inspection must be live; output writable and error buffer valid if supplied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_inspection_flags_1(
    inspection: *const DarwinArtElfInspection,
    output: *mut u64,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("flags output is null"));
        }
        let inspection =
            unsafe { inspection.as_ref() }.ok_or(FfiFailure::Invalid("inspection is null"))?;
        unsafe { *output = inspection.flags_1 };
        Ok(())
    })
}
