//! Private native-owner ABI, not guest Android entry points.
//! Registry pointers require external quiescence before destroy. Image leases
//! may outlive the registry. Retain/release callbacks run outside its mutex;
//! they must be thread-safe and release must arrange any owner-thread teardown.

use crate::linker_namespace::{NamespaceConfig, NamespaceId, NamespaceImage, NamespaceRegistry};
use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[path = "linker_caller_lookup.rs"]
mod caller_lookup;
#[path = "linker_namespace_config.rs"]
mod config;
#[path = "linker_default_paths.rs"]
mod default_paths;
#[path = "linker_dependency_lookup.rs"]
mod dependency_lookup;
#[path = "linker_dependency_references.rs"]
mod dependency_references;
#[path = "linker_discovery.rs"]
mod discovery;
#[path = "linker_error.rs"]
mod error;
#[path = "linker_file_lookup.rs"]
mod file_lookup;
#[path = "linker_group_detach.rs"]
mod group_detach;
#[path = "linker_group_metadata.rs"]
mod group_metadata;
#[path = "linker_group_snapshot.rs"]
mod group_snapshot;
#[path = "linker_handles.rs"]
mod handles;
#[path = "linker_image_kind.rs"]
mod image_kind;
#[path = "linker_image_sdk.rs"]
mod image_sdk;
#[path = "linker_namespace_inherit.rs"]
mod inherit;
#[path = "linker_namespace_open.rs"]
mod open_file;
#[path = "linker_open_reference.rs"]
mod open_reference;
#[path = "linker_operation.rs"]
mod operation;
#[path = "linker_path_open.rs"]
mod path_open;
#[path = "linker_path_targets.rs"]
mod path_targets;
#[path = "linker_publication_ffi.rs"]
mod publication;
#[path = "linker_publication_transaction.rs"]
mod publication_transaction;
#[path = "linker_retention_flags.rs"]
mod retention_flags;
#[path = "linker_search_paths.rs"]
mod search_paths;

pub type RetainImage = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
pub type ReleaseImage = unsafe extern "C" fn(*mut c_void);

struct ImageOwner {
    target_sdk: Option<i32>,
    group: Option<group_metadata::GroupMetadata>,
    group_root_flags: Option<Arc<crate::linker_load_flags::LoadFlags>>,
    group_opens: Option<Arc<std::sync::atomic::AtomicUsize>>,
    group_incoming: Option<Arc<std::sync::atomic::AtomicUsize>>,
    group_dependencies_complete: bool,
    group_retired: Option<Arc<std::sync::atomic::AtomicBool>>,
    _group_edges: Option<Arc<Vec<dependency_references::DependencyReference>>>,
    file_identity: Option<(u64, u64, u64)>,
    value: usize,
    kind: u32,
    primary_namespace: u64,
    release: ReleaseImage,
}
impl Drop for ImageOwner {
    fn drop(&mut self) {
        // SAFETY: publication requires a thread-safe infallible release callback
        // which owns this retained token until its one invocation here.
        unsafe { (self.release)(self.value as *mut c_void) };
    }
}
pub struct LinkerRegistry(
    Mutex<NamespaceRegistry<ImageOwner>>,
    Arc<operation::OperationGate>,
    Mutex<std::collections::BTreeSet<u64>>,
    Mutex<handles::Handles>,
);
pub struct LinkerImageLease(
    Arc<NamespaceImage<ImageOwner>>,
    Option<Arc<open_reference::OpenReference>>,
);
impl Drop for LinkerImageLease {
    fn drop(&mut self) {
        // Release the logical reference while the native image is still live.
        self.1.take();
    }
}

unsafe fn text(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: strings at this private ABI are readable and NUL terminated.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(str::to_owned)
}
unsafe fn paths(value: *const c_char) -> Vec<PathBuf> {
    if value.is_null() {
        return Vec::new();
    }
    // SAFETY: same private ABI string precondition, preserving non-UTF8 paths.
    unsafe { CStr::from_ptr(value) }
        .to_bytes()
        .split(|b| *b == b':')
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(std::ffi::OsString::from_vec(part.to_vec())))
        .collect()
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_linker_registry_create() -> *mut LinkerRegistry {
    Box::into_raw(Box::new(LinkerRegistry(
        Mutex::new(NamespaceRegistry::default()),
        Arc::new(operation::OperationGate::default()),
        Mutex::new(std::collections::BTreeSet::new()),
        Mutex::new(handles::Handles::default()),
    )))
}

/// # Safety
/// `registry` must be a live registry with no concurrent or future calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_registry_destroy(registry: *mut LinkerRegistry) {
    if !registry.is_null() {
        // SAFETY: ownership is transferred exactly once with all callers drained.
        drop(unsafe { Box::from_raw(registry) });
    }
}

/// # Safety
/// Registry must be live, strings NUL terminated, and output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_create(
    registry: *mut LinkerRegistry,
    isolated: u8,
    search: *const c_char,
    default_search: *const c_char,
    permitted: *const c_char,
    shared_parent: u64,
    output: *mut u64,
) -> i32 {
    unsafe {
        config::darwin_art_linker_namespace_create_configured(
            registry,
            isolated,
            search,
            default_search,
            permitted,
            std::ptr::null(),
            shared_parent,
            output,
        )
    }
}

/// # Safety
/// Registry must be live and name NUL terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_export(
    registry: *mut LinkerRegistry,
    id: u64,
    name: *const c_char,
) -> i32 {
    if registry.is_null() {
        return -1;
    }
    let Some(name) = (unsafe { text(name) }) else {
        return -1;
    };
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    if state.export(name, NamespaceId::from_raw(id)).is_ok() {
        0
    } else {
        -1
    }
}

/// # Safety
/// Registry/name must be valid and output writable. Returns 1 for not exported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_exported(
    registry: *mut LinkerRegistry,
    name: *const c_char,
    output: *mut u64,
) -> i32 {
    if registry.is_null() || output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let Some(name) = (unsafe { text(name) }) else {
        return -1;
    };
    let Ok(state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match state.exported(&name) {
        Some(id) => {
            unsafe { *output = id.raw() };
            0
        }
        None => 1,
    }
}

/// # Safety
/// Registry must be live and names must be NUL terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_link(
    registry: *mut LinkerRegistry,
    from: u64,
    to: u64,
    names: *const c_char,
) -> i32 {
    if registry.is_null() {
        return -1;
    }
    let Some(names) = (unsafe { text(names) }) else {
        return -1;
    };
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match state.link(
        NamespaceId::from_raw(from),
        NamespaceId::from_raw(to),
        names.split(':').map(str::to_owned).collect(),
    ) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// # Safety
/// Valid registry/strings/source and callbacks are required. Retain returns an
/// independently owned token or null; release must be infallible/thread-safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    path: *const c_char,
    source: *mut c_void,
    retain: Option<RetainImage>,
    release: Option<ReleaseImage>,
) -> i32 {
    unsafe {
        darwin_art_linker_namespace_publish_flags(
            registry, id, soname, path, source, retain, release, 0, 0,
        )
    }
}

/// # Safety
/// Same lifetime contract as publish. flags_1 comes from the mapped ELF image;
/// global is the Android load mode as a boolean, never Darwin RTLD bit values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_flags(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    path: *const c_char,
    source: *mut c_void,
    retain: Option<RetainImage>,
    release: Option<ReleaseImage>,
    flags_1: u64,
    global: u8,
) -> i32 {
    unsafe {
        publish_image(
            registry, id, soname, path, source, retain, release, flags_1, global, 0,
        )
    }
}

unsafe fn publish_image(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    path: *const c_char,
    source: *mut c_void,
    retain: Option<RetainImage>,
    release: Option<ReleaseImage>,
    flags_1: u64,
    global: u8,
    kind: u32,
) -> i32 {
    if global > 1 {
        return -1;
    }
    if registry.is_null() || source.is_null() || path.is_null() {
        return -1;
    }
    let (Some(retain), Some(release), Some(soname)) = (retain, release, unsafe { text(soname) })
    else {
        return -1;
    };
    if !crate::linker_namespace::library_name(&soname) {
        return -1;
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(
        unsafe { CStr::from_ptr(path) }.to_bytes().to_vec(),
    ));
    let registry = unsafe { &*registry };
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        if state.permits(NamespaceId::from_raw(id), &path) != Ok(true) {
            return -1;
        }
    }
    // Never enter arbitrary resource callbacks while holding namespace state.
    let value = unsafe { retain(source) };
    if value.is_null() {
        return -3;
    }
    let image = Arc::new(NamespaceImage {
        visibility: crate::linker_namespace::ImageVisibility::new(flags_1, global != 0),
        soname,
        path,
        lease: Arc::new(ImageOwner {
            target_sdk: None,
            group: None,
            group_root_flags: None,
            group_opens: None,
            group_incoming: None,
            group_dependencies_complete: false,
            group_retired: None,
            _group_edges: None,
            file_identity: None,
            value: value as usize,
            kind,
            primary_namespace: id,
            release,
        }),
    });
    let result = {
        let Ok(mut state) = registry.0.lock() else {
            return -2;
        };
        state.publish(NamespaceId::from_raw(id), Arc::clone(&image))
    };
    // On failure the local Arc performs rollback after the guard is dropped.
    if result.is_ok() { 0 } else { -1 }
}

/// # Safety
/// Live registry, NUL-terminated SONAME, writable output. Release a result once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_find(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if registry.is_null() || output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let Some(soname) = (unsafe { text(soname) }) else {
        return -1;
    };
    let Ok(state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match state.find(NamespaceId::from_raw(id), &soname) {
        Ok(Some(image)) => {
            unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(image, None))) };
            0
        }
        Ok(None) => 1,
        Err(_) => -1,
    }
}

/// # Safety
/// Lease must remain live while the borrowed payload is used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_payload(
    lease: *const LinkerImageLease,
) -> *mut c_void {
    if lease.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { &*lease }.0.lease.value as *mut c_void
}

/// # Safety
/// Transfer a live lease exactly once, after all its payload users have drained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_release(lease: *mut LinkerImageLease) {
    if !lease.is_null() {
        drop(unsafe { Box::from_raw(lease) });
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static RETAINS: AtomicUsize = AtomicUsize::new(0);
    static RELEASES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn retain_dylib(path: *mut c_void) -> *mut c_void {
        RETAINS.fetch_add(1, Ordering::Relaxed);
        unsafe { libc::dlopen(path.cast(), libc::RTLD_NOW | libc::RTLD_LOCAL) }
    }
    unsafe extern "C" fn release_dylib(image: *mut c_void) {
        assert_eq!(unsafe { libc::dlclose(image) }, 0);
        RELEASES.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn failed_retain(_: *mut c_void) -> *mut c_void {
        std::ptr::null_mut()
    }

    #[test]
    fn real_macho_lease_outlives_registry() {
        // Platform operations are supplied here; registry/admission/shared
        // references use the same exported private ABI as the native owner.
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let mut parent = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    c"/usr/lib".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut parent
                ),
                0
            );
            let source = c"/usr/lib/libSystem.B.dylib".as_ptr().cast_mut().cast();
            assert_eq!(
                darwin_art_linker_namespace_export(registry, parent, c"system".as_ptr()),
                0
            );
            let mut exported = 0;
            assert_eq!(
                darwin_art_linker_namespace_exported(registry, c"system".as_ptr(), &mut exported),
                0
            );
            assert_eq!(exported, parent);
            let mut absent = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    parent,
                    c"missing.dylib".as_ptr(),
                    &mut absent
                ),
                1
            );
            assert!(absent.is_null());
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    0,
                    c"missing.dylib".as_ptr(),
                    &mut absent
                ),
                -1
            );
            assert_eq!(
                darwin_art_linker_namespace_publish(
                    registry,
                    parent,
                    c"libSystem.B.dylib".as_ptr(),
                    c"/usr/lib/libSystem.B.dylib".as_ptr(),
                    source,
                    Some(failed_retain),
                    Some(release_dylib)
                ),
                -3
            );
            assert_eq!(
                darwin_art_linker_namespace_publish(
                    registry,
                    parent,
                    c"libSystem.B.dylib".as_ptr(),
                    c"/outside/libSystem.B.dylib".as_ptr(),
                    source,
                    Some(retain_dylib),
                    Some(release_dylib)
                ),
                -1
            );
            assert_eq!(RETAINS.load(Ordering::Relaxed), 0);
            assert_eq!(
                image_kind::darwin_art_linker_namespace_publish_image(
                    registry,
                    parent,
                    c"libSystem.B.dylib".as_ptr(),
                    c"/usr/lib/libSystem.B.dylib".as_ptr(),
                    source,
                    Some(retain_dylib),
                    Some(release_dylib),
                    0,
                    0,
                    2
                ),
                0
            );
            let mut child = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    c"/app/lib".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    &mut child
                ),
                0
            );
            let mut lease = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    child,
                    c"libSystem.B.dylib".as_ptr(),
                    &mut lease
                ),
                0
            );
            assert!(!lease.is_null());
            let mut primary = 0;
            assert_eq!(
                image_kind::darwin_art_linker_image_primary_namespace(lease, &mut primary),
                0
            );
            assert_eq!(primary, parent);
            assert_ne!(primary, child);
            let mut member = 9;
            for namespace in [parent, child] {
                assert_eq!(
                    image_kind::darwin_art_linker_image_namespace_member(
                        registry,
                        namespace,
                        lease,
                        &mut member
                    ),
                    0
                );
                assert_eq!(member, 1);
            }
            assert_eq!(
                image_kind::darwin_art_linker_image_namespace_member(
                    registry,
                    u64::MAX,
                    lease,
                    &mut member
                ),
                -1
            );
            assert_eq!(member, 0);
            let foreign = darwin_art_linker_registry_create();
            member = 9;
            assert_eq!(
                image_kind::darwin_art_linker_image_namespace_member(
                    foreign,
                    parent,
                    lease,
                    &mut member
                ),
                -1
            );
            assert_eq!(member, 0);
            darwin_art_linker_registry_destroy(foreign);
            let mut linked_only = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    c"/app/other".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut linked_only
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_namespace_link(
                    registry,
                    linked_only,
                    parent,
                    c"libSystem.B.dylib".as_ptr()
                ),
                0
            );
            assert_eq!(
                image_kind::darwin_art_linker_image_namespace_member(
                    registry,
                    linked_only,
                    lease,
                    &mut member
                ),
                0
            );
            assert_eq!(member, 0); // Open permission is not symbol membership.
            assert_eq!(
                image_kind::darwin_art_linker_image_primary_namespace(
                    std::ptr::null(),
                    &mut primary
                ),
                -1
            );
            assert_eq!(primary, 0);
            let mut typed = std::ptr::null_mut();
            assert_eq!(
                image_kind::darwin_art_linker_image_typed_payload(lease, 2, &mut typed),
                0
            );
            assert_eq!(typed, darwin_art_linker_image_payload(lease));
            assert_eq!(
                image_kind::darwin_art_linker_image_typed_payload(lease, 1, &mut typed),
                -4
            );
            assert!(typed.is_null());
            let mut before_promotion = 0;
            assert_eq!(
                inherit::darwin_art_linker_namespace_inherit_group(
                    registry,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    0,
                    &mut before_promotion
                ),
                0
            );
            assert_eq!(inherit::darwin_art_linker_image_promote_global(lease), 0);
            let mut after_promotion = 0;
            assert_eq!(
                inherit::darwin_art_linker_namespace_inherit_group(
                    registry,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    0,
                    &mut after_promotion
                ),
                0
            );
            let mut selected = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    before_promotion,
                    c"libSystem.B.dylib".as_ptr(),
                    &mut selected
                ),
                1
            );
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    after_promotion,
                    c"libSystem.B.dylib".as_ptr(),
                    &mut selected
                ),
                0
            );
            darwin_art_linker_image_release(selected);
            let mut default_child = 0;
            assert_eq!(
                inherit::darwin_art_linker_namespace_inherit_group(
                    registry,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    1,
                    &mut default_child
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    default_child,
                    c"libSystem.B.dylib".as_ptr(),
                    &mut selected
                ),
                1
            );
            let mut inherited = 0;
            let group = [lease.cast_const()];
            assert_eq!(
                inherit::darwin_art_linker_namespace_inherit(
                    registry,
                    1,
                    c"/child/lib".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    group.as_ptr(),
                    group.len(),
                    &mut inherited
                ),
                0
            );
            let mut inherited_lease = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    inherited,
                    c"libSystem.B.dylib".as_ptr(),
                    &mut inherited_lease
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_image_payload(inherited_lease),
                darwin_art_linker_image_payload(lease)
            );
            assert!(
                !(&*registry)
                    .0
                    .lock()
                    .unwrap()
                    .permits(
                        NamespaceId::from_raw(inherited),
                        std::path::Path::new("/usr/lib/new.dylib")
                    )
                    .unwrap()
            );
            darwin_art_linker_image_release(inherited_lease);
            let null_group = [std::ptr::null()];
            assert_eq!(
                inherit::darwin_art_linker_namespace_inherit(
                    registry,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    parent,
                    null_group.as_ptr(),
                    1,
                    &mut inherited
                ),
                -1
            );
            assert_eq!(inherited, 0);
            let payload = darwin_art_linker_image_payload(lease);
            let symbol = libc::dlsym(payload, c"getpid".as_ptr());
            assert!(!symbol.is_null());
            let getpid: unsafe extern "C" fn() -> libc::pid_t = std::mem::transmute(symbol);
            assert_eq!(getpid(), libc::getpid());
            let mut group = std::ptr::null_mut();
            assert_eq!(
                group_snapshot::darwin_art_linker_group_snapshot(registry, parent, 0, &mut group),
                0
            );
            let mut globals = std::ptr::null_mut();
            assert_eq!(
                group_snapshot::darwin_art_linker_group_snapshot(registry, parent, 1, &mut globals),
                0
            );
            let mut item_payload = std::ptr::null_mut();
            let mut item_name = std::ptr::null();
            assert_eq!(
                group_snapshot::darwin_art_linker_group_item(
                    globals,
                    0,
                    &mut item_payload,
                    &mut item_name
                ),
                1
            );
            group_snapshot::darwin_art_linker_group_destroy(globals);
            darwin_art_linker_registry_destroy(registry);
            assert_eq!(RELEASES.load(Ordering::Relaxed), 0);
            assert_eq!(getpid(), libc::getpid());
            darwin_art_linker_image_release(lease);
            assert_eq!(RETAINS.load(Ordering::Relaxed), 1);
            assert_eq!(RELEASES.load(Ordering::Relaxed), 0);
            assert_eq!(
                group_snapshot::darwin_art_linker_group_typed_item(group, 0, 2, &mut typed),
                0
            );
            assert_eq!(typed, payload);
            assert_eq!(
                group_snapshot::darwin_art_linker_group_typed_item(group, 0, 1, &mut typed),
                -4
            );
            assert!(typed.is_null());
            assert_eq!(
                group_snapshot::darwin_art_linker_group_typed_item(group, 1, 2, &mut typed),
                1
            );
            assert!(typed.is_null());
            assert_eq!(
                group_snapshot::darwin_art_linker_group_item(
                    group,
                    0,
                    &mut item_payload,
                    &mut item_name
                ),
                0
            );
            assert_eq!(item_payload, payload);
            assert_eq!(CStr::from_ptr(item_name).to_bytes(), b"libSystem.B.dylib");
            assert_eq!(getpid(), libc::getpid());
            assert_eq!(
                group_snapshot::darwin_art_linker_group_item(
                    group,
                    1,
                    &mut item_payload,
                    &mut item_name
                ),
                1
            );
            assert!(item_payload.is_null() && item_name.is_null());
            group_snapshot::darwin_art_linker_group_destroy(group);
            assert_eq!(RELEASES.load(Ordering::Relaxed), 1);
        }
    }
}
