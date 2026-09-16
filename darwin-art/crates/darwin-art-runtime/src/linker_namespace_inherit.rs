//! Native boundary for a non-shared child and its real inherited image leases.
//! Flag selection remains in the linker; this boundary never guesses globals.
use super::*;

/// # Safety
/// Live registry, NUL strings, writable non-aliasing output. The linker must
/// serialize visibility promotion with creation, and identify its real default
/// namespace via default_parent, not by guessing from a string name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_inherit_group(
    registry: *mut LinkerRegistry,
    isolated: u8,
    search: *const c_char,
    default_search: *const c_char,
    permitted: *const c_char,
    parent: u64,
    default_parent: u8,
    output: *mut u64,
) -> i32 {
    if registry.is_null() || output.is_null() || isolated > 1 || default_parent > 1 {
        return -1;
    }
    unsafe { *output = 0 };
    let config = NamespaceConfig {
        isolated: isolated != 0,
        search_paths: unsafe { paths(search) },
        default_search_paths: unsafe { paths(default_search) },
        permitted_paths: unsafe { paths(permitted) },
        ..Default::default()
    };
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    let parent = NamespaceId::from_raw(parent);
    let Ok(group) = state.shared_group(parent, default_parent != 0) else {
        return -1;
    };
    match state.create_with_shared_group(config, parent, &group) {
        Ok(id) => {
            unsafe { *output = id.raw() };
            0
        }
        Err(_) => -1,
    }
}

/// # Safety
/// Lease remains live. Caller serializes Android linker mutations/creation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_promote_global(
    lease: *const LinkerImageLease,
) -> i32 {
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    lease.0.visibility.promote_global();
    0
}

/// # Safety
/// Registry and all leases must remain live for this call. Strings are NUL
/// terminated. `group` points to `count` readable lease pointers (null allowed
/// only for count zero). Output is writable and must not alias inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_inherit(
    registry: *mut LinkerRegistry,
    isolated: u8,
    search: *const c_char,
    default_search: *const c_char,
    permitted: *const c_char,
    parent: u64,
    group: *const *const LinkerImageLease,
    count: usize,
    output: *mut u64,
) -> i32 {
    if registry.is_null()
        || output.is_null()
        || isolated > 1
        || (count != 0 && group.is_null())
        || count > isize::MAX as usize / std::mem::size_of::<*const LinkerImageLease>()
    {
        return -1;
    }
    unsafe { *output = 0 };
    let pointers = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(group, count) }
    };
    if pointers.iter().any(|lease| lease.is_null()) {
        return -1;
    }
    // Clone Arc leases without calling native retain. Drop this temporary
    // vector after the registry lock; the caller also retains its leases.
    let images: Vec<_> = pointers
        .iter()
        .map(|lease| Arc::clone(&unsafe { &**lease }.0))
        .collect();
    let config = NamespaceConfig {
        isolated: isolated != 0,
        search_paths: unsafe { paths(search) },
        default_search_paths: unsafe { paths(default_search) },
        permitted_paths: unsafe { paths(permitted) },
        ..Default::default()
    };
    let result = {
        let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
            return -2;
        };
        state.create_with_shared_group(config, NamespaceId::from_raw(parent), &images)
    };
    match result {
        Ok(id) => {
            unsafe { *output = id.raw() };
            0
        }
        Err(_) => -1,
    }
}
