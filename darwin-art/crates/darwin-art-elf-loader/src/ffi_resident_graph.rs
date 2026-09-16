//! Owned per-edge admission ABI, separate from legacy provider-name discovery.
use super::ffi_graph_discovery::AdmittedImage;
use super::ffi_resident_metadata::{ResidentMetadata, copy_needed};
use super::*;
use std::os::fd::BorrowedFd;

#[repr(C)]
pub struct Admission {
    kind: u32,
    fd: i32,
    image: u64,
    resident: *mut c_void,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
}
type Admit = unsafe extern "C" fn(
    *mut c_void,
    u64,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut Admission,
) -> i32;
pub(super) struct ResidentOwner {
    pub(super) pointer: NonNull<c_void>,
    release: unsafe extern "C" fn(*mut c_void),
}

#[cfg(test)]
#[path = "ffi_resident_graph_tests.rs"]
mod tests;
impl Drop for ResidentOwner {
    fn drop(&mut self) {
        unsafe { (self.release)(self.pointer.as_ptr()) };
    }
}

/// Borrow an admitted native image without transferring its release obligation.
/// # Safety
/// Graph remains live and is not destroyed concurrently. Outputs are writable,
/// non-aliasing; returned name and owner remain borrowed from that graph.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_resident(
    graph: *const DarwinArtElfDiscoveredGraph,
    index: usize,
    out_name: *mut *const c_char,
    out_image: *mut u64,
    out_resident: *mut *mut c_void,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if out_name.is_null() || out_image.is_null() || out_resident.is_null() {
            return Err(FfiFailure::Invalid("null resident outputs"));
        }
        unsafe {
            *out_name = ptr::null();
            *out_image = 0;
            *out_resident = ptr::null_mut();
        }
        let graph =
            unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("null discovered graph"))?;
        let entry = graph
            ._residents
            .get(index)
            .ok_or(FfiFailure::Invalid("resident index out of range"))?;
        let owner = entry
            ._lease
            .downcast_ref::<ResidentOwner>()
            .ok_or(FfiFailure::Invalid(
                "resident has no native admission owner",
            ))?;
        unsafe {
            *out_name = entry.name.as_ptr();
            *out_image = entry.image;
            *out_resident = owner.pointer.as_ptr();
        }
        Ok(())
    })
}

/// # Safety
/// Graph is live during the call and output is writable and non-aliasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_resident_count(
    graph: *const DarwinArtElfDiscoveredGraph,
    output: *mut usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null resident count output"));
        }
        unsafe {
            *output = 0;
        }
        let graph =
            unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("null discovered graph"))?;
        unsafe {
            *output = graph._residents.len();
        }
        Ok(())
    })
}

/// # Safety
/// Same root/input contracts as admitted_graph. Callback success transfers
/// exactly one owned fd or live resident+infallible release. Failure transfers
/// nothing. Callback must not unwind; image IDs are stable, nonzero identities.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discover_resident_graph(
    root_fd: i32,
    root_image: u64,
    root_name: *const u8,
    root_length: usize,
    admit: Option<Admit>,
    context: *mut c_void,
    out_is_elf: *mut i32,
    out_graph: *mut *mut DarwinArtElfDiscoveredGraph,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    unsafe {
        darwin_art_elf_discover_resident_graph_with_edges(
            root_fd,
            root_image,
            root_name,
            root_length,
            admit,
            None,
            context,
            out_is_elf,
            out_graph,
            error,
        )
    }
}

/// # Safety
/// Same admission contract as resident_graph. Metadata arrays/strings remain
/// readable while the transferred resident lease is held, including after the
/// callback returns. They are copied before any lease release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discover_resident_graph_with_edges(
    root_fd: i32,
    root_image: u64,
    root_name: *const u8,
    root_length: usize,
    admit: Option<Admit>,
    metadata: Option<ResidentMetadata>,
    context: *mut c_void,
    out_is_elf: *mut i32,
    out_graph: *mut *mut DarwinArtElfDiscoveredGraph,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if out_is_elf.is_null() || out_graph.is_null() {
            return Err(FfiFailure::Invalid("null outputs"));
        }
        unsafe {
            *out_is_elf = 0;
            *out_graph = ptr::null_mut();
        }
        if root_fd < 0
            || root_image == 0
            || root_name.is_null()
            || root_length > MAX_DISCOVERY_COMPONENT_SIZE
        {
            return Err(FfiFailure::Invalid("invalid root input"));
        }
        let name = unsafe { std::slice::from_raw_parts(root_name, root_length) };
        validate_discovery_component(name, "root name")?;
        let fd = unsafe { BorrowedFd::borrow_raw(root_fd) }
            .try_clone_to_owned()
            .map_err(|e| FfiFailure::Io(e.to_string()))?;
        let mut root = Some(File::from(fd));
        let mut is_elf = false;
        let graph =
            ffi_graph_discovery::discover_resident_graph(name, &mut is_elf, |name, origin| {
                let Some(origin) = origin else {
                    return Ok(AdmittedImage::File(
                        root.take().ok_or(FfiFailure::Invalid("repeated root"))?,
                        root_image,
                    ));
                };
                let callback = admit.ok_or(FfiFailure::Invalid("missing admission callback"))?;
                let name = CString::new(name).map_err(|_| FfiFailure::Invalid("invalid name"))?;
                let parent = CString::new(origin.soname.clone())
                    .map_err(|_| FfiFailure::Invalid("invalid parent"))?;
                let runpath = origin
                    .runpath
                    .as_ref()
                    .map(|p| CString::new(p.clone()))
                    .transpose()
                    .map_err(|_| FfiFailure::Invalid("invalid RUNPATH"))?;
                let mut output = Admission {
                    kind: 0,
                    fd: -1,
                    image: 0,
                    resident: ptr::null_mut(),
                    release: None,
                };
                let status = unsafe {
                    callback(
                        context,
                        origin.image,
                        parent.as_ptr(),
                        name.as_ptr(),
                        runpath.as_ref().map_or(ptr::null(), |p| p.as_ptr()),
                        &mut output,
                    )
                };
                if status != 0 {
                    return Err(FfiFailure::Io(format!(
                        "admission failed: {} needed by {}",
                        name.to_string_lossy(),
                        parent.to_string_lossy()
                    )));
                }
                // Adopt transferred resources before validating the discriminant,
                // so malformed successful results do not leak valid resources.
                let file = (output.fd >= 0).then(|| unsafe { File::from_raw_fd(output.fd) });
                let resident = NonNull::new(output.resident)
                    .zip(output.release)
                    .map(|(pointer, release)| ResidentOwner { pointer, release });
                if output.image == 0 {
                    return Err(FfiFailure::Invalid("zero admitted image ID"));
                }
                match (output.kind, file, resident) {
                    (1, Some(file), None)
                        if output.resident.is_null() && output.release.is_none() =>
                    {
                        Ok(AdmittedImage::File(file, output.image))
                    }
                    (2, None, Some(owner)) => Ok(AdmittedImage::Resident {
                        image: output.image,
                        needed: unsafe { copy_needed(metadata, context, owner.pointer.as_ptr()) }?,
                        lease: Box::new(owner),
                    }),
                    _ => Err(FfiFailure::Invalid("invalid admission ownership/tag")),
                }
            });
        unsafe {
            *out_is_elf = i32::from(is_elf);
        }
        unsafe {
            *out_graph = Box::into_raw(Box::new(graph?));
        }
        Ok(())
    })
}
