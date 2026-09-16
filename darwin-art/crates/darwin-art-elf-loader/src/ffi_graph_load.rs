//! Graph input validation and retained-global transfer at the native boundary.
use super::*;

pub(super) struct GraphLoadRequest<'a> {
    pub(super) root_soname: *const c_char,
    pub(super) sources: *const DarwinArtElfGraphSource,
    pub(super) source_count: usize,
    pub(super) provider_sonames: *const *const c_char,
    pub(super) provider_count: usize,
    pub(super) options: *const DarwinArtElfLoadOptions,
    pub(super) lifecycle: *const DarwinArtElfLifecycleCallbacks,
    pub(super) global_sources: *const DarwinArtElfGlobalSource,
    pub(super) global_count: usize,
    pub(super) native_owners: *const ffi_native_owners::NativeOwner,
    pub(super) native_count: usize,
    pub(super) out_handle: *mut *mut DarwinArtElfGraphHandle,
    pub(super) discovered: Option<&'a DarwinArtElfDiscoveredGraph>,
    pub(super) initialize: bool,
    pub(super) owned_lifecycle: Option<Arc<dyn DsoLifecycle>>,
}

/// # Safety
/// Input arrays/strings/handles stay live during this synchronous call; output
/// is writable and non-aliasing. Borrowed lifecycle contexts must outlive every
/// resulting mapping/selected lease, not only the original graph handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_graph_load_with_globals(
    root_soname: *const c_char,
    sources: *const DarwinArtElfGraphSource,
    source_count: usize,
    provider_sonames: *const *const c_char,
    provider_count: usize,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const DarwinArtElfLifecycleCallbacks,
    global_sources: *const DarwinArtElfGlobalSource,
    global_count: usize,
    out_handle: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    unsafe {
        darwin_art_elf_graph_load_with_owners(
            root_soname,
            sources,
            source_count,
            provider_sonames,
            provider_count,
            options,
            lifecycle,
            global_sources,
            global_count,
            ptr::null(),
            0,
            out_handle,
            error,
        )
    }
}

/// # Safety
/// Same graph-input requirements as load_with_globals. Owner records are
/// borrowed during the call; callbacks retain thread-safe resources and never
/// unwind. Successful retention is released on rollback or final graph drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_graph_load_with_owners(
    root_soname: *const c_char,
    sources: *const DarwinArtElfGraphSource,
    source_count: usize,
    provider_sonames: *const *const c_char,
    provider_count: usize,
    options: *const DarwinArtElfLoadOptions,
    lifecycle: *const DarwinArtElfLifecycleCallbacks,
    global_sources: *const DarwinArtElfGlobalSource,
    global_count: usize,
    native_owners: *const ffi_native_owners::NativeOwner,
    native_count: usize,
    out_handle: *mut *mut DarwinArtElfGraphHandle,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || unsafe {
        load_graph(GraphLoadRequest {
            root_soname,
            sources,
            source_count,
            provider_sonames,
            provider_count,
            options,
            lifecycle,
            global_sources,
            global_count,
            native_owners,
            native_count,
            out_handle,
            discovered: None,
            initialize: true,
            owned_lifecycle: None,
        })
    })
}

pub(super) unsafe fn load_graph(request: GraphLoadRequest<'_>) -> Result<(), FfiFailure> {
    let GraphLoadRequest {
        root_soname,
        sources,
        source_count,
        provider_sonames,
        provider_count,
        options,
        lifecycle,
        global_sources,
        global_count,
        native_owners,
        native_count,
        out_handle,
        discovered,
        initialize,
        owned_lifecycle,
    } = request;
    if out_handle.is_null() {
        return Err(FfiFailure::Invalid("out_handle is null"));
    }
    // SAFETY: validated non-null writable out parameter.
    unsafe { *out_handle = ptr::null_mut() };
    let names = discovered.map(|graph| {
        graph
            ._residents
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    });
    let mut native_owners = if let Some(names) = names.as_deref() {
        unsafe { ffi_native_owners::retain_named_owners(native_owners, native_count, Some(names)) }?
    } else {
        unsafe { ffi_native_owners::retain_owners(native_owners, native_count) }?
    };
    if let Some(graph) = discovered {
        // Retain admitted inodes for the mapping lifetime, not staging bytes.
        // This prevents pathname replacement/unlink from recycling identity.
        native_owners.extend(ffi_source_owners::retain_files(graph)?);
    }
    if global_count > MAX_INPUT_SIZE / std::mem::size_of::<DarwinArtElfGlobalSource>()
        || (global_count != 0 && global_sources.is_null())
    {
        return Err(FfiFailure::Invalid("invalid global image array"));
    }
    let global_sources = if global_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(global_sources, global_count) }
    };
    let mut globals = Vec::with_capacity(global_count);
    for source in global_sources {
        let owner =
            unsafe { source.graph.as_ref() }.ok_or(FfiFailure::Invalid("global graph is null"))?;
        let soname = unsafe { required_utf8(source.soname, "global SONAME is null") }?;
        let image = select_graph_image(owner, &soname)?;
        globals.push(image);
    }
    // Every owner lock has been released before callbacks/constructors.
    if source_count == 0 {
        return Err(FfiFailure::Invalid("ELF graph has no sources"));
    }
    if sources.is_null() {
        return Err(FfiFailure::Invalid("ELF graph sources are null"));
    }
    if source_count > MAX_INPUT_SIZE / std::mem::size_of::<DarwinArtElfGraphSource>() {
        return Err(FfiFailure::Invalid("ELF graph source count is excessive"));
    }
    if provider_count != 0 && provider_sonames.is_null() {
        return Err(FfiFailure::Invalid("ELF graph providers are null"));
    }
    if provider_count > MAX_INPUT_SIZE / std::mem::size_of::<*const c_char>() {
        return Err(FfiFailure::Invalid("ELF graph provider count is excessive"));
    }
    // SAFETY: the C contract supplies source_count readable source records.
    let sources = unsafe { std::slice::from_raw_parts(sources, source_count) };
    // SAFETY: the C contract supplies provider_count readable string pointers.
    let providers = unsafe {
        std::slice::from_raw_parts(
            if provider_sonames.is_null() {
                NonNull::<*const c_char>::dangling().as_ptr()
            } else {
                provider_sonames
            },
            provider_count,
        )
    };
    // SAFETY: validated by required_utf8 for this synchronous call.
    let root = unsafe { required_utf8(root_soname, "root_soname is null")? };
    let mut namespace = ClosedElfNamespace::new();
    let mut total_size = 0_usize;
    for source in sources {
        // SAFETY: source strings and byte ranges are borrowed for this synchronous call.
        let soname = unsafe { required_utf8(source.soname, "source SONAME is null")? };
        if source.length != 0 && source.bytes.is_null() {
            return Err(FfiFailure::Invalid(
                "source bytes are null for nonzero length",
            ));
        }
        total_size = total_size
            .checked_add(source.length)
            .ok_or(FfiFailure::Invalid("ELF graph byte size overflow"))?;
        if total_size > MAX_INPUT_SIZE {
            return Err(FfiFailure::Invalid("ELF graph exceeds 1 GiB limit"));
        }
        // SAFETY: validated null/length shape and guaranteed readable by the C contract.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                if source.bytes.is_null() {
                    NonZeroUsize::MIN.get() as *const u8
                } else {
                    source.bytes
                },
                source.length,
            )
        };
        namespace
            .add_elf(soname, bytes)
            .map_err(FfiFailure::Namespace)?;
    }
    for &provider in providers {
        // SAFETY: provider strings are borrowed for this synchronous call.
        let provider = unsafe { required_utf8(provider, "provider SONAME is null")? };
        let needed = discovered
            .and_then(|graph| {
                graph
                    ._residents
                    .iter()
                    .find(|entry| entry.name.as_bytes() == provider.as_bytes())
            })
            .map(|entry| {
                entry
                    .needed
                    .iter()
                    .map(|name| name.to_str().expect("discovery validated UTF-8").to_owned())
                    .collect()
            })
            .unwrap_or_default();
        namespace
            .add_provider_with_dependencies(provider, needed)
            .map_err(FfiFailure::Namespace)?;
    }

    let lifecycle: Option<Arc<dyn DsoLifecycle>> = if let Some(owner) = owned_lifecycle {
        if !lifecycle.is_null() {
            return Err(FfiFailure::Invalid("two lifecycle owners supplied"));
        }
        Some(owner)
    } else if lifecycle.is_null() {
        None
    } else {
        // SAFETY: the C contract supplies one readable lifecycle callback record.
        let lifecycle = unsafe { &*lifecycle };
        if lifecycle.abi_version != ABI_VERSION {
            return Err(FfiFailure::Invalid(
                "lifecycle callback ABI version mismatch",
            ));
        }
        let publish = lifecycle
            .publish_image
            .ok_or(FfiFailure::Invalid("publish_image callback is null"))?;
        let finalize = lifecycle
            .finalize_image
            .ok_or(FfiFailure::Invalid("finalize_image callback is null"))?;
        Some(Arc::new(CallbackDsoLifecycle {
            publish,
            finalize,
            context: lifecycle.context as usize,
            _owner: None,
        }))
    };
    let options = options_from_pointer(options)?;
    let namespace_scopes = discovered.and_then(|graph| graph.namespace_scopes.as_ref());
    let appcompat_16kb = discovered.and_then(|graph| graph.appcompat_16kb);
    if namespace_scopes.is_some_and(|(_, count)| *count != globals.len()) {
        return Err(FfiFailure::Invalid(
            "namespace global snapshot count changed",
        ));
    }
    let namespace_scopes = namespace_scopes.map(|(scopes, _)| scopes);
    let graph = match options.and_then(|options| {
        options
            .resolver
            .map(|callback| (callback, options.resolver_context))
    }) {
        Some((callback, context)) => {
            let mut resolver = CallbackResolver { callback, context };
            namespace
                .link_with_scopes_and_appcompat(
                    &root,
                    &mut resolver,
                    lifecycle,
                    &globals,
                    native_owners,
                    crate::namespace::ScopeLinkOptions {
                        namespace_scopes,
                        appcompat_16kb,
                    },
                )
                .map_err(FfiFailure::Namespace)?
        }
        None => {
            let mut resolver = crate::RejectAllResolver;
            namespace
                .link_with_scopes_and_appcompat(
                    &root,
                    &mut resolver,
                    lifecycle,
                    &globals,
                    native_owners,
                    crate::namespace::ScopeLinkOptions {
                        namespace_scopes,
                        appcompat_16kb,
                    },
                )
                .map_err(FfiFailure::Namespace)?
        }
    };
    if initialize {
        graph.initialize().map_err(FfiFailure::Namespace)?;
    }
    let handle = Box::new(DarwinArtElfGraphHandle {
        owner: GraphHandleOwner::Graph(Mutex::new(graph)),
    });
    // Linked-only callers own namespace publication before initialization.
    unsafe { *out_handle = Box::into_raw(handle) };
    Ok(())
}
