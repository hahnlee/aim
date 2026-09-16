//! Preserve admission identity through staging. These IDs belong to the
//! caller's discovery context; they do not grant namespace visibility.
use super::*;
use std::os::unix::fs::MetadataExt;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct DarwinArtElfFileIdentity {
    pub device: u64,
    pub inode: u64,
    pub offset: u64,
}
/// # Safety
/// Live graph, writable output. Reads the retained admitted fd, never a path.
/// Current discovery reads whole ELF files at offset zero (not APK zip slices).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_file_identity(
    graph: *const DarwinArtElfDiscoveredGraph,
    index: usize,
    output: *mut DarwinArtElfFileIdentity,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("missing file identity output"));
        }
        unsafe { *output = DarwinArtElfFileIdentity::default() };
        let graph =
            unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("missing discovered graph"))?;
        let file = graph
            .source_files
            .get(index)
            .ok_or(FfiFailure::Invalid("source index out of range"))?;
        let metadata = file.metadata().map_err(|e| FfiFailure::Io(e.to_string()))?;
        unsafe {
            *output = DarwinArtElfFileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
                offset: 0,
            }
        };
        Ok(())
    })
}

/// # Safety
/// Graph is live, output writable and non-aliasing. Index addresses the same
/// source ordering returned by discovered_graph_sources.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_source_image(
    graph: *const DarwinArtElfDiscoveredGraph,
    index: usize,
    output: *mut u64,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("missing source identity output"));
        }
        unsafe { *output = 0 };
        let graph =
            unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("missing discovered graph"))?;
        let image = graph
            .source_images
            .get(index)
            .ok_or(FfiFailure::Invalid("source index out of range"))?;
        unsafe { *output = *image };
        Ok(())
    })
}
