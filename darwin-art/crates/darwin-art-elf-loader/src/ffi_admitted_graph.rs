//! Host-only entrypoint for graph discovery from already admitted descriptors.
//! Namespace selection belongs to the dependency opener, not this byte owner.
use super::*;
use std::os::fd::BorrowedFd;

type OpenDependency = unsafe extern "C" fn(
    *mut c_void,
    u64,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut u64,
) -> i32;

/// # Safety
/// Root fd and input buffers remain live for the call. Callback returns a
/// uniquely owned host fd or negative failure and may not unwind. Outputs are
/// writable. Single closed SONAME scope; not a multi-namespace graph ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discover_admitted_graph(
    root_fd: i32,
    root_image: u64,
    root_name: *const u8,
    root_length: usize,
    provider_names: *const *const c_char,
    provider_count: usize,
    open_dependency: Option<OpenDependency>,
    context: *mut c_void,
    out_is_elf: *mut i32,
    out_graph: *mut *mut DarwinArtElfDiscoveredGraph,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if out_is_elf.is_null() || out_graph.is_null() {
            return Err(FfiFailure::Invalid("graph outputs are null"));
        }
        unsafe {
            *out_is_elf = 0;
            *out_graph = ptr::null_mut();
        }
        if root_fd < 0 || root_name.is_null() {
            return Err(FfiFailure::Invalid("root input is invalid"));
        }
        let root = unsafe { std::slice::from_raw_parts(root_name, root_length) };
        validate_discovery_component(root, "root name")?;
        if provider_count > MAX_DISCOVERY_FILES || (provider_count != 0 && provider_names.is_null())
        {
            return Err(FfiFailure::Invalid("provider array shape is invalid"));
        }
        let mut providers = HashSet::new();
        for index in 0..provider_count {
            let name = unsafe { *provider_names.add(index) };
            if name.is_null() {
                return Err(FfiFailure::Invalid("null provider"));
            }
            let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
            validate_discovery_component(bytes, "provider SONAME")?;
            if !providers.insert(bytes.to_vec()) {
                return Err(FfiFailure::Invalid("duplicate provider"));
            }
        }
        let owned = unsafe { BorrowedFd::borrow_raw(root_fd) }
            .try_clone_to_owned()
            .map_err(|e| FfiFailure::Io(e.to_string()))?;
        let mut root_file = Some(File::from(owned));
        let mut is_elf = false;
        let result = super::ffi_graph_discovery::discover_admitted_graph(
            root,
            providers,
            &mut is_elf,
            |name, parent| {
                let Some(parent) = parent else {
                    return root_file
                        .take()
                        .map(|file| (file, root_image))
                        .ok_or(FfiFailure::Invalid("root requested twice"));
                };
                let opener =
                    open_dependency.ok_or(FfiFailure::Invalid("dependency opener is absent"))?;
                let runpath = parent
                    .runpath
                    .as_ref()
                    .map(|path| CString::new(path.clone()))
                    .transpose()
                    .map_err(|_| FfiFailure::Invalid("RUNPATH contains NUL"))?;
                let parent_image = parent.image;
                let parent = CString::new(parent.soname.clone())
                    .map_err(|_| FfiFailure::Invalid("parent contains NUL"))?;
                let name = CString::new(name)
                    .map_err(|_| FfiFailure::Invalid("dependency contains NUL"))?;
                let mut image = 0;
                let fd = unsafe {
                    opener(
                        context,
                        parent_image,
                        parent.as_ptr(),
                        name.as_ptr(),
                        runpath.as_ref().map_or(ptr::null(), |path| path.as_ptr()),
                        &mut image,
                    )
                };
                if fd < 0 {
                    return Err(FfiFailure::Io(format!(
                        "dependency admission failed: {} needed by {}",
                        name.to_string_lossy(),
                        parent.to_string_lossy()
                    )));
                }
                Ok((unsafe { File::from_raw_fd(fd) }, image))
            },
        );
        unsafe {
            *out_is_elf = i32::from(is_elf);
        }
        let graph = result?;
        unsafe {
            *out_graph = Box::into_raw(Box::new(graph));
        }
        Ok(())
    })
}
