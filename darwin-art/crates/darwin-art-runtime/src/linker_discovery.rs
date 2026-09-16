//! Synchronous per-image context for admitted ELF discovery. Policy remains in
//! namespace search; this owner retains referring path/namespace, never fds.
use super::*;
use std::ffi::CString;
#[path = "linker_discovery_placement.rs"]
mod placement;
#[path = "linker_discovery_resident.rs"]
mod resident;
#[path = "linker_discovery_scope.rs"]
mod scope;

type OpenNode = unsafe extern "C" fn(*const c_char, *mut c_char, usize, *mut i32) -> i32;
type Errno = unsafe extern "C" fn() -> i32;
struct ImageContext {
    namespace: u64,
    path: CString,
    resident: Option<Arc<NamespaceImage<ImageOwner>>>,
}
pub struct LinkerDiscovery {
    registry: *mut LinkerRegistry,
    images: Vec<ImageContext>,
    edges: Vec<(u64, u64)>,
    directory: OpenNode,
    image: OpenNode,
    errno: Errno,
}

/// # Safety
/// Registry/callbacks remain live until destruction. Root path is the canonical
/// guest path of the retained root fd, already admitted by its owner. Context
/// is single-threaded/non-reentrant. It must be destroyed before registry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_create(
    registry: *mut LinkerRegistry,
    namespace: u64,
    root: *const c_char,
    directory: Option<OpenNode>,
    image: Option<OpenNode>,
    errno: Option<Errno>,
) -> *mut LinkerDiscovery {
    let (Some(directory), Some(image), Some(errno)) = (directory, image, errno) else {
        return std::ptr::null_mut();
    };
    if registry.is_null() || root.is_null() {
        return std::ptr::null_mut();
    }
    let path = unsafe { CStr::from_ptr(root) }.to_owned();
    let guest = PathBuf::from(std::ffi::OsString::from_vec(path.as_bytes().to_vec()));
    let Ok(state) = (unsafe { &*registry }).0.lock() else {
        return std::ptr::null_mut();
    };
    if state.permits(NamespaceId::from_raw(namespace), &guest) != Ok(true) {
        return std::ptr::null_mut();
    }
    drop(state);
    Box::into_raw(Box::new(LinkerDiscovery {
        registry,
        edges: Vec::new(),
        images: vec![ImageContext {
            namespace,
            path,
            resident: None,
        }],
        directory,
        image,
        errno,
    }))
}

/// # Safety
/// Matches the ELF admitted-graph dependency callback. Context is live, unused
/// concurrently and non-reentrant; strings readable; output writable. Returns
/// a uniquely owned host fd or -1. Root image ID is 1, zero is always invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_open(
    context: *mut c_void,
    parent: u64,
    _parent_soname: *const c_char,
    name: *const c_char,
    runpath: *const c_char,
    out_image: *mut u64,
) -> i32 {
    if context.is_null() || out_image.is_null() {
        return -1;
    }
    unsafe {
        *out_image = 0;
    }
    let state = unsafe { &mut *context.cast::<LinkerDiscovery>() };
    let Some(index) = parent
        .checked_sub(1)
        .and_then(|id| usize::try_from(id).ok())
    else {
        return -1;
    };
    let Some(origin) = state.images.get(index) else {
        return -1;
    };
    let mut canonical = [0i8; 4096];
    let mut namespace = 0;
    let mut fd = -1;
    let status = unsafe {
        super::open_file::darwin_art_linker_namespace_open(
            state.registry,
            origin.namespace,
            name,
            runpath,
            origin.path.as_ptr(),
            Some(state.directory),
            Some(state.image),
            Some(state.errno),
            canonical.as_mut_ptr(),
            canonical.len(),
            &mut namespace,
            &mut fd,
        )
    };
    if status != 0 {
        return -1;
    }
    let path = unsafe { CStr::from_ptr(canonical.as_ptr()) }.to_owned();
    let id = if let Some(index) = state.images.iter().position(|image| {
        image.resident.is_none() && image.namespace == namespace && image.path == path
    }) {
        index + 1
    } else {
        state.images.push(ImageContext {
            namespace,
            path,
            resident: None,
        });
        state.images.len()
    };
    unsafe {
        *out_image = id as u64;
    }
    if !state.edges.contains(&(parent, id as u64)) {
        state.edges.push((parent, id as u64));
    }
    fd
}

/// # Safety
/// No discovery call is active; destroy exactly once. Does not own registry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_destroy(context: *mut LinkerDiscovery) {
    if !context.is_null() {
        drop(unsafe { Box::from_raw(context) });
    }
}
