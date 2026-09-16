//! Native ABI for explicit linker configuration, not ClassLoader policy.
use super::*;

/// # Safety
/// Live registry, NUL-terminated strings and writable output. Paths are already
/// guest-resolved. Allowed libraries are colon-delimited basenames; null/empty
/// means no name restriction, as in bionic NamespaceConfig. No file I/O here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_create_configured(
    registry: *mut LinkerRegistry,
    isolated: u8,
    search: *const c_char,
    default_search: *const c_char,
    permitted: *const c_char,
    allowed: *const c_char,
    shared_parent: u64,
    output: *mut u64,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    if registry.is_null() || isolated > 1 {
        return -1;
    }
    let allowed = if allowed.is_null() {
        String::new()
    } else {
        let Some(value) = (unsafe { text(allowed) }) else {
            return -1;
        };
        value
    };
    let config = NamespaceConfig {
        isolated: isolated != 0,
        search_paths: unsafe { paths(search) },
        default_search_paths: unsafe { paths(default_search) },
        permitted_paths: unsafe { paths(permitted) },
        allowed_libraries: if allowed.is_empty() {
            Default::default()
        } else {
            allowed.split(':').map(str::to_owned).collect()
        },
    };
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match state.create(
        config,
        (shared_parent != 0).then(|| NamespaceId::from_raw(shared_parent)),
    ) {
        Ok(id) => {
            unsafe { *output = id.raw() };
            0
        }
        Err(_) => -1,
    }
}

/// # Safety
/// Registry must be live. Both IDs identify actual namespaces. Unlike named
/// Android links, the all-libraries operation has no null/default target alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_link_all(
    registry: *mut LinkerRegistry,
    from: u64,
    to: u64,
) -> i32 {
    if registry.is_null() {
        return -1;
    }
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    match state.link_all(NamespaceId::from_raw(from), NamespaceId::from_raw(to)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_names_restrict_paths_and_reject_malformed_input() {
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let mut id = 99;
            assert_eq!(
                darwin_art_linker_namespace_create_configured(
                    registry,
                    1,
                    c"/lib".as_ptr(),
                    std::ptr::null(),
                    c"/permitted".as_ptr(),
                    c"liba.so:libb.so".as_ptr(),
                    0,
                    &mut id
                ),
                0
            );
            {
                let state = (&*registry).0.lock().unwrap();
                for (path, expected) in [
                    ("/lib/liba.so", true),
                    ("/lib/libb.so", true),
                    ("/lib/libprivate.so", false),
                    ("/permitted/sub/libprivate.so", false),
                    ("/permitted/sub/liba.so", true),
                    ("/outside/liba.so", false),
                ] {
                    assert_eq!(
                        state.permits(NamespaceId::from_raw(id), std::path::Path::new(path)),
                        Ok(expected)
                    );
                }
            }
            for malformed in [c"../liba.so", c"liba.so::libb.so"] {
                assert_eq!(
                    darwin_art_linker_namespace_create_configured(
                        registry,
                        1,
                        c"/lib".as_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        malformed.as_ptr(),
                        0,
                        &mut id
                    ),
                    -1
                );
                assert_eq!(id, 0);
            }
            assert_eq!(
                darwin_art_linker_namespace_create_configured(
                    registry,
                    1,
                    c"/lib".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    c"".as_ptr(),
                    0,
                    &mut id
                ),
                0
            );
            assert_eq!(
                (&*registry).0.lock().unwrap().permits(
                    NamespaceId::from_raw(id),
                    std::path::Path::new("/lib/libother.so")
                ),
                Ok(true)
            );
            darwin_art_linker_registry_destroy(registry);
        }
    }
    #[test]
    fn configured_link_requires_two_real_owners() {
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let mut from = 0;
            let mut to = 0;
            for output in [&mut from, &mut to] {
                assert_eq!(
                    darwin_art_linker_namespace_create(
                        registry,
                        1,
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                        0,
                        output
                    ),
                    0
                );
            }
            assert_eq!(darwin_art_linker_namespace_link_all(registry, from, 0), -1);
            assert_eq!(darwin_art_linker_namespace_link_all(registry, 0, to), -1);
            assert_eq!(darwin_art_linker_namespace_link_all(registry, from, to), 0);
            darwin_art_linker_registry_destroy(registry);
        }
    }
}
