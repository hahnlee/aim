//! Consume discovered identities and edges together, without reconstructing
//! an independent SONAME allowlist at the native caller.
use super::*;

struct DiscoveredLoadRequest {
    discovered: *const DarwinArtElfDiscoveredGraph,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const DarwinArtElfLifecycleCallbacks,
    globals: *const DarwinArtElfGlobalSource,
    global_count: usize,
    owners: *const ffi_native_owners::NativeOwner,
    owner_count: usize,
    output: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
}

/// # Safety
/// Discovery, arrays and callbacks remain live through this call. Native owner
/// callbacks must retain actual residents through the resulting graph lifetime.
/// Output is writable/non-aliasing. Discovery remains caller-owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_load_with_owners(
    discovered: *const DarwinArtElfDiscoveredGraph,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const DarwinArtElfLifecycleCallbacks,
    globals: *const DarwinArtElfGlobalSource,
    global_count: usize,
    owners: *const ffi_native_owners::NativeOwner,
    owner_count: usize,
    output: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    unsafe {
        load(
            DiscoveredLoadRequest {
                discovered,
                options,
                lifecycle,
                globals,
                global_count,
                owners,
                owner_count,
                output,
                error,
            },
            true,
            None,
        )
    }
}

/// # Safety
/// Same lifetime contract as discovered_graph_load_with_owners. Does not run
/// constructors; caller must publish, initialize and roll back failed loads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_link_with_owners(
    discovered: *const DarwinArtElfDiscoveredGraph,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const DarwinArtElfLifecycleCallbacks,
    globals: *const DarwinArtElfGlobalSource,
    global_count: usize,
    owners: *const ffi_native_owners::NativeOwner,
    owner_count: usize,
    output: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    unsafe {
        load(
            DiscoveredLoadRequest {
                discovered,
                options,
                lifecycle,
                globals,
                global_count,
                owners,
                owner_count,
                output,
                error,
            },
            false,
            None,
        )
    }
}

/// # Safety
/// Same discovered ownership contract, but lifecycle context is retained by
/// each mapping. The source owner handle need only survive this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_link_with_owned_lifecycle(
    discovered: *const DarwinArtElfDiscoveredGraph,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const ffi_owned_lifecycle::DarwinArtElfLifecycleOwner,
    globals: *const DarwinArtElfGlobalSource,
    global_count: usize,
    owners: *const ffi_native_owners::NativeOwner,
    owner_count: usize,
    output: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    unsafe {
        load(
            DiscoveredLoadRequest {
                discovered,
                options,
                lifecycle: ptr::null(),
                globals,
                global_count,
                owners,
                owner_count,
                output,
                error,
            },
            false,
            Some(lifecycle),
        )
    }
}

unsafe fn load(
    request: DiscoveredLoadRequest,
    initialize: bool,
    owned_lifecycle: Option<*const ffi_owned_lifecycle::DarwinArtElfLifecycleOwner>,
) -> DarwinArtElfStatus {
    let DiscoveredLoadRequest {
        discovered,
        options,
        lifecycle,
        globals,
        global_count,
        owners,
        owner_count,
        output,
        error,
    } = request;
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null graph output"));
        }
        unsafe { *output = ptr::null_mut() };
        let owned_lifecycle = owned_lifecycle
            .map(|owner| {
                unsafe { owner.as_ref() }
                    .map(|owner| owner.0.clone())
                    .ok_or(FfiFailure::Invalid("null owned lifecycle"))
            })
            .transpose()?;
        let graph =
            unsafe { discovered.as_ref() }.ok_or(FfiFailure::Invalid("null discovered graph"))?;
        if owner_count != graph._residents.len() || (owner_count != 0 && owners.is_null()) {
            return Err(FfiFailure::Invalid("owners must match admitted residents"));
        }
        for (index, resident) in graph._residents.iter().enumerate() {
            let original = resident
                ._lease
                .downcast_ref::<ffi_resident_graph::ResidentOwner>()
                .ok_or(FfiFailure::Invalid("missing admitted native owner"))?;
            if unsafe { (*owners.add(index)).context } != original.pointer.as_ptr() {
                return Err(FfiFailure::Invalid(
                    "native owner does not match admitted identity",
                ));
            }
        }
        let providers: Vec<_> = graph
            ._residents
            .iter()
            .map(|entry| entry.name.as_ptr())
            .collect();
        unsafe {
            ffi_graph_load::load_graph(ffi_graph_load::GraphLoadRequest {
                root_soname: graph.root_soname.as_ptr(),
                sources: graph.sources.as_ptr(),
                source_count: graph.sources.len(),
                provider_sonames: providers.as_ptr(),
                provider_count: providers.len(),
                options,
                lifecycle,
                global_sources: globals,
                global_count,
                native_owners: owners,
                native_count: owner_count,
                out_handle: output,
                discovered: Some(graph),
                initialize,
                owned_lifecycle,
            })
        }
    })
}
