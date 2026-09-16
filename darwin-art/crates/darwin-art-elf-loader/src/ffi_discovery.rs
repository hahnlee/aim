//! Trusted directory and metadata discovery for the ELF C ABI.
//!
//! This phase owns byte-component validation and bounded sibling-graph reads;
//! handle loading and exported C entrypoints remain in `ffi.rs`.

use super::*;

fn cstring_from_dynamic(bytes: Vec<u8>, what: &'static str) -> Result<CString, FfiFailure> {
    CString::new(bytes).map_err(|_| FfiFailure::Format(format!("{what} contains embedded NUL")))
}

pub(super) fn inspection_from_bytes(bytes: &[u8]) -> Result<DarwinArtElfInspection, FfiFailure> {
    let metadata = inspect_elf_metadata(bytes).map_err(FfiFailure::Load)?;
    let soname = metadata
        .soname
        .map(|name| cstring_from_dynamic(name, "DT_SONAME"))
        .transpose()?;
    let needed = metadata
        .needed_libraries
        .into_iter()
        .map(|name| cstring_from_dynamic(name, "DT_NEEDED"))
        .collect::<Result<Vec<_>, _>>()?;
    let runpath = metadata
        .runpath
        .map(|bytes| cstring_from_dynamic(bytes, "DT_RUNPATH"))
        .transpose()?;
    Ok(DarwinArtElfInspection {
        flags_1: metadata.flags_1,
        soname,
        needed,
        runpath,
    })
}

/// # Safety
/// Inspection is live and output is writable. Borrowed string lasts until
/// inspection_destroy; null means no DT_RUNPATH, not an empty present string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_inspection_runpath(
    inspection: *const DarwinArtElfInspection,
    output: *mut *const c_char,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("runpath output is null"));
        }
        unsafe {
            *output = ptr::null();
        }
        let inspection =
            unsafe { inspection.as_ref() }.ok_or(FfiFailure::Invalid("inspection is null"))?;
        unsafe {
            *output = inspection
                .runpath
                .as_ref()
                .map_or(ptr::null(), |path| path.as_ptr());
        }
        Ok(())
    })
}

pub(super) fn validate_discovery_component(bytes: &[u8], what: &str) -> Result<(), FfiFailure> {
    let invalid = bytes.is_empty()
        || bytes.len() > MAX_DISCOVERY_COMPONENT_SIZE
        || bytes.contains(&0)
        || bytes.contains(&b'/')
        || bytes == b"."
        || bytes == b"..";
    if invalid {
        return Err(FfiFailure::InvalidOwned(format!(
            "{what} must be one nonempty byte component without NUL, slash, dot, or dot-dot and at most {MAX_DISCOVERY_COMPONENT_SIZE} bytes"
        )));
    }
    Ok(())
}

pub(super) fn discover_sibling_graph(
    directory_fd: i32,
    root_component: &[u8],
    providers: HashSet<Vec<u8>>,
    root_is_elf: &mut bool,
) -> Result<DarwinArtElfDiscoveredGraph, FfiFailure> {
    validate_discovery_component(root_component, "root ELF filename")?;
    if directory_fd < 0 {
        return Err(FfiFailure::Invalid("library directory fd is negative"));
    }
    // SAFETY: dup creates an independently owned descriptor or returns -1.
    let duplicated = unsafe { dup(directory_fd) };
    if duplicated < 0 {
        return Err(FfiFailure::Io(
            "could not duplicate trusted library directory fd".to_owned(),
        ));
    }
    // SAFETY: duplicated is a fresh descriptor now uniquely owned by File.
    let directory = unsafe { File::from_raw_fd(duplicated) };
    let broker = ReadOnlyBroker::from_directory(directory)
        .map_err(|error| FfiFailure::Io(format!("invalid trusted library directory: {error}")))?;

    super::ffi_graph_discovery::discover_admitted_graph(
        root_component,
        providers,
        root_is_elf,
        |component, _needed_by| {
            broker
                .open(component)
                .map(|opened| (opened.into_file(), 0))
                .map_err(|error| {
                    FfiFailure::Io(format!("secure ELF graph component open failed: {error}"))
                })
        },
    )
}
