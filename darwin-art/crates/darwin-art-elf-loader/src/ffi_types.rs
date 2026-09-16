use crate::{LoadedElf, LoadedElfGraph};
use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DarwinArtElfStatus {
    Ok = 0,
    InvalidArgument = 1,
    Io = 2,
    Format = 3,
    Bounds = 4,
    Capability = 5,
    Protection = 6,
    Resolver = 7,
    UnresolvedSymbol = 8,
    SymbolNotFound = 9,
    InvalidSymbol = 10,
    Lifecycle = 11,
    System = 12,
    Poisoned = 13,
    Panic = 14,
}

#[repr(C)]
pub struct DarwinArtElfErrorBuffer {
    pub data: *mut c_char,
    pub capacity: usize,
    pub required: usize,
}

#[repr(C)]
pub struct DarwinArtElfSymbolRequest {
    pub abi_version: u32,
    pub symbol: *const c_char,
    pub version_soname: *const c_char,
    pub version_name: *const c_char,
    pub version_flags: u16,
    pub version_hidden: u8,
    pub symbol_weak: u8,
    pub needed_libraries: *const *const c_char,
    pub needed_library_count: usize,
}

pub type DarwinArtElfResolverCallback = unsafe extern "C" fn(
    context: *mut c_void,
    request: *const DarwinArtElfSymbolRequest,
    out_address: *mut usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DarwinArtElfLoadOptions {
    pub abi_version: u32,
    pub resolver: Option<DarwinArtElfResolverCallback>,
    pub resolver_context: *mut c_void,
}

pub type DarwinArtElfPublishImageCallback =
    unsafe extern "C" fn(context: *mut c_void, start: usize, end: usize) -> i32;
pub type DarwinArtElfFinalizeImageCallback =
    unsafe extern "C" fn(context: *mut c_void, start: usize, end: usize) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DarwinArtElfLifecycleCallbacks {
    pub abi_version: u32,
    pub publish_image: Option<DarwinArtElfPublishImageCallback>,
    pub finalize_image: Option<DarwinArtElfFinalizeImageCallback>,
    pub context: *mut c_void,
}

pub struct DarwinArtElfHandle {
    pub(crate) image: Mutex<LoadedElf>,
}

pub struct DarwinArtElfGraphHandle {
    pub(crate) owner: GraphHandleOwner,
}
pub(crate) enum GraphHandleOwner {
    Graph(Mutex<LoadedElfGraph>),
    // Read-only source view, not an owning/mutable graph handle.
    Selected(crate::GlobalElfImage),
}

#[repr(C)]
pub struct DarwinArtElfGraphSource {
    pub soname: *const c_char,
    pub bytes: *const u8,
    pub length: usize,
}

#[repr(C)]
pub struct DarwinArtElfGlobalSource {
    pub graph: *const DarwinArtElfGraphHandle,
    pub soname: *const c_char,
}

pub struct DarwinArtElfInspection {
    pub(crate) flags_1: u64,
    pub(crate) soname: Option<CString>,
    pub(crate) runpath: Option<CString>,
    pub(crate) needed: Vec<CString>,
}

pub struct DarwinArtElfDiscoveredGraph {
    pub(crate) namespace_scopes: Option<(crate::NamespaceScopes, usize)>,
    /// Per-load linker policy snapshot. None preserves legacy graph staging;
    /// Some(value) is supplied by the linker owner before graph linking.
    pub(crate) appcompat_16kb: Option<bool>,
    pub(crate) root_soname: CString,
    pub(crate) _names: Vec<CString>,
    pub(crate) _bytes: Vec<Vec<u8>>,
    pub(crate) sources: Vec<DarwinArtElfGraphSource>,
    // Admission-context identities, in exactly the same order as sources.
    pub(crate) source_images: Vec<u64>,
    // Pin the admitted inode, including after unlink/replacement, through load.
    pub(crate) source_files: Vec<std::sync::Arc<std::fs::File>>,
    pub(crate) _residents: Vec<DiscoveredResident>,
}

pub(crate) struct DiscoveredResident {
    pub(crate) needed: Vec<CString>,
    pub(crate) name: CString,
    pub(crate) image: u64,
    // Concrete owner is supplied by the admission layer; dropping it releases
    // its real image lease. No integer-only provider declarations here.
    pub(crate) _lease: Box<dyn std::any::Any>,
}
