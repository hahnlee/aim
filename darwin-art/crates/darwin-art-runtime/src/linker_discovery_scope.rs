//! AOSP symbol accessibility: primary/secondary membership, or a direct
//! parent with primary membership. Namespace link lists are NOT symbol scopes.
use super::*;
/// # Safety
/// Context live, discovery finished and not concurrently mutated. Query ID
/// must be an admitted image; outputs writable. No resource callback is run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_discovery_image_scope(
    context: *const LinkerDiscovery,
    image: u64,
    requester: u64,
    primary: *mut u64,
    accessible: *mut u8,
) -> i32 {
    if primary.is_null() || accessible.is_null() {
        return -1;
    }
    unsafe {
        *primary = 0;
        *accessible = 0;
    }
    let Some(state) = (unsafe { context.as_ref() }) else {
        return -1;
    };
    let Some(entry) = image
        .checked_sub(1)
        .and_then(|id| usize::try_from(id).ok())
        .and_then(|index| state.images.get(index))
    else {
        return -1;
    };
    let Ok(registry) = (unsafe { &*state.registry }).0.lock() else {
        return -2;
    };
    if registry
        .permits(NamespaceId::from_raw(requester), &PathBuf::from("/"))
        .is_err()
    {
        return -1;
    }
    let secondary = entry.resident.as_ref().is_some_and(|resident| {
        registry.contains_image(NamespaceId::from_raw(requester), resident) == Ok(true)
    });
    let parent = state.edges.iter().any(|&(parent, child)| {
        child == image && state.images[(parent - 1) as usize].namespace == requester
    });
    unsafe {
        *primary = entry.namespace;
        *accessible = u8::from(entry.namespace == requester || secondary || parent);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" fn release(_: *mut c_void) {}
    unsafe extern "C" fn no_open(_: *const c_char, _: *mut c_char, _: usize, _: *mut i32) -> i32 {
        -1
    }
    unsafe extern "C" fn no_errno() -> i32 {
        2
    }
    #[test]
    fn direct_parent_not_namespace_links_or_secondary_parent_grants_symbols() {
        let registry = darwin_art_linker_registry_create();
        let (app, system, secondary, parent) = {
            let mut state = unsafe { &*registry }.0.lock().unwrap();
            let app = state.create(NamespaceConfig::default(), None).unwrap();
            let system = state.create(NamespaceConfig::default(), None).unwrap();
            let parent = Arc::new(NamespaceImage {
                soname: "parent.so".into(),
                path: "/parent.so".into(),
                visibility: Default::default(),
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
                    value: 0,
                    kind: 0,
                    primary_namespace: app.raw(),
                    release,
                }),
            });
            state.publish(app, parent.clone()).unwrap();
            let secondary = state.create(NamespaceConfig::default(), Some(app)).unwrap();
            state.link_all(app, system).unwrap(); // Not a symbol visibility grant.
            (app.raw(), system.raw(), secondary.raw(), parent)
        };
        let context = LinkerDiscovery {
            registry,
            images: vec![
                ImageContext {
                    namespace: app,
                    path: CString::new("/parent.so").unwrap(),
                    resident: Some(parent),
                },
                ImageContext {
                    namespace: system,
                    path: CString::new("/child.so").unwrap(),
                    resident: None,
                },
                ImageContext {
                    namespace: system,
                    path: CString::new("/private.so").unwrap(),
                    resident: None,
                },
            ],
            edges: vec![(1, 2), (2, 3)],
            directory: no_open,
            image: no_open,
            errno: no_errno,
        };
        for (requester, image, expected) in [
            (app, 1, 1),
            (app, 2, 1),
            (app, 3, 0),
            (secondary, 1, 1),
            (secondary, 2, 0),
            (secondary, 3, 0),
            (system, 3, 1),
        ] {
            let mut primary = 0;
            let mut visible = 0;
            assert_eq!(
                unsafe {
                    darwin_art_linker_discovery_image_scope(
                        &context,
                        image,
                        requester,
                        &mut primary,
                        &mut visible,
                    )
                },
                0
            );
            assert_eq!(visible, expected);
            assert_eq!(primary, if image == 1 { app } else { system });
        }
        let mut primary = 42;
        let mut visible = 1;
        assert_eq!(
            unsafe {
                darwin_art_linker_discovery_image_scope(
                    &context,
                    0,
                    app,
                    &mut primary,
                    &mut visible,
                )
            },
            -1
        );
        assert_eq!((primary, visible), (0, 0));
        drop(context);
        unsafe {
            darwin_art_linker_registry_destroy(registry);
        }
    }
}
