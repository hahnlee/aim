use super::dependency_references::*;
use super::group_detach::*;
use super::group_metadata::GroupMetadata;
use super::open_reference::*;
use super::*;

static TOKEN: u8 = 0;
unsafe extern "C" fn contains(_: *mut c_void, address: usize, context: *mut c_void) -> i32 {
    let registry = unsafe { &*context.cast::<LinkerRegistry>() };
    assert!(registry.0.try_lock().is_ok());
    if address == 9 {
        -1
    } else {
        i32::from(address == 7)
    }
}
unsafe extern "C" fn retain(value: *mut c_void) -> *mut c_void {
    value
}
unsafe extern "C" fn release(_: *mut c_void) {}
struct FinalizeContext {
    registry: *mut LinkerRegistry,
    id: u64,
    fail: bool,
    calls: usize,
}
unsafe extern "C" fn finalize(payload: *mut c_void, context: *mut c_void) -> i32 {
    let context = unsafe { &mut *context.cast::<FinalizeContext>() };
    assert_eq!(payload, (&TOKEN as *const u8).cast_mut().cast());
    assert!(unsafe { &*context.registry }.0.try_lock().is_ok());
    let mut lookup = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            darwin_art_linker_namespace_find(
                context.registry,
                context.id,
                c"same.so".as_ptr(),
                &mut lookup,
            )
        },
        0
    );
    let mut opened = std::ptr::null_mut();
    assert_eq!(
        unsafe { darwin_art_linker_image_acquire_open(lookup, &mut opened) },
        -1
    );
    assert!(opened.is_null());
    let record = record(context.id, 0);
    let group = GroupMetadata {
        id: 402,
        is_root: 1,
    };
    let edge = DependencyEdge {
        source_index: 0,
        target_group: 401,
    };
    assert_eq!(
        unsafe {
            darwin_art_linker_namespace_publish_linked_groups(
                context.registry,
                &record,
                &group,
                1,
                &edge,
                1,
                std::ptr::null_mut(),
            )
        },
        -1
    );
    unsafe {
        darwin_art_linker_image_release(lookup);
    }
    context.calls += 1;
    if context.fail { -1 } else { 0 }
}
fn record(id: u64, flags: u64) -> Publication {
    // Metadata-only valid static token; never interpreted as executable ELF.
    Publication {
        id,
        soname: c"same.so".as_ptr(),
        path: c"/same.so".as_ptr(),
        source: (&TOKEN as *const u8).cast_mut().cast(),
        retain: Some(retain),
        release: Some(release),
        flags_1: flags,
        global: 0,
        kind: 1,
        device: 0,
        inode: 0,
        offset: 0,
    }
}

#[test]
fn detachment_checks_refs_flags_origin_and_prevents_resurrection() {
    unsafe {
        for flags in [0, 2, 8] {
            let registry = darwin_art_linker_registry_create();
            let id = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), None)
                .unwrap();
            let record = record(id.raw(), flags);
            let group = GroupMetadata {
                id: 401,
                is_root: 1,
            };
            assert_eq!(
                darwin_art_linker_namespace_publish_linked_groups(
                    registry,
                    &record,
                    &group,
                    1,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut()
                ),
                0
            );
            let shared = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), Some(id))
                .unwrap();
            let mut lookup = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(
                    registry,
                    id.raw(),
                    c"same.so".as_ptr(),
                    &mut lookup
                ),
                0
            );
            let mut opened = std::ptr::null_mut();
            let mut resolved = std::ptr::null_mut();
            use super::caller_lookup::darwin_art_linker_find_image_address;
            assert_eq!(
                darwin_art_linker_find_image_address(
                    registry,
                    1,
                    7,
                    Some(contains),
                    registry.cast(),
                    &mut resolved
                ),
                0
            );
            assert!(Arc::ptr_eq(&(*resolved).0, &(*lookup).0));
            darwin_art_linker_image_release(resolved);
            assert_eq!(
                darwin_art_linker_find_image_address(
                    registry,
                    1,
                    8,
                    Some(contains),
                    registry.cast(),
                    &mut resolved
                ),
                1
            );
            assert!(resolved.is_null());
            assert_eq!(
                darwin_art_linker_find_image_address(
                    registry,
                    1,
                    9,
                    Some(contains),
                    registry.cast(),
                    &mut resolved
                ),
                -6
            );
            assert!(resolved.is_null());
            assert_eq!(darwin_art_linker_image_acquire_open(lookup, &mut opened), 0);
            use super::handles::*;
            let saved_open = opened;
            assert_eq!(
                darwin_art_linker_handle_adopt(
                    registry,
                    &mut opened,
                    (&mut opened as *mut *mut LinkerImageLease).cast()
                ),
                -1
            );
            assert_eq!(opened, saved_open);
            let foreign_handles = darwin_art_linker_registry_create();
            let mut foreign_token = 99;
            assert_eq!(
                darwin_art_linker_handle_adopt(foreign_handles, &mut opened, &mut foreign_token),
                -1
            );
            assert_eq!(opened, saved_open);
            assert_eq!(foreign_token, 0);
            darwin_art_linker_registry_destroy(foreign_handles);
            let mut handle = 0;
            assert_eq!(
                darwin_art_linker_handle_adopt(registry, &mut opened, &mut handle),
                0
            );
            assert!(opened.is_null() && handle != 0);
            let mut borrowed = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_handle_image(registry, handle, &mut borrowed),
                0
            );
            assert!(Arc::ptr_eq(&(*borrowed).0, &(*lookup).0));
            assert!((*borrowed).1.is_none());
            darwin_art_linker_image_release(borrowed);
            assert_eq!(darwin_art_linker_image_acquire_open(lookup, &mut opened), 0);
            let mut same_handle = 0;
            assert_eq!(
                darwin_art_linker_handle_adopt(registry, &mut opened, &mut same_handle),
                0
            );
            assert_eq!(same_handle, handle);
            assert_eq!(
                darwin_art_linker_handle_take_open(registry, handle, &mut opened),
                0
            );
            darwin_art_linker_image_release(opened);
            assert_eq!(
                darwin_art_linker_handle_take_open(registry, handle, &mut opened),
                0
            );
            // Failure before close side effects can restore the identical token.
            assert_eq!(
                darwin_art_linker_handle_adopt(registry, &mut opened, &mut same_handle),
                0
            );
            assert_eq!(same_handle, handle);
            assert_eq!(
                darwin_art_linker_handle_take_open(registry, handle, &mut opened),
                0
            );
            assert_eq!(darwin_art_linker_handles_retire_group(registry, 401), -1);
            let mut detached = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_group_detach(registry, lookup, &mut detached),
                1
            );
            assert!(detached.is_null());
            darwin_art_linker_image_release(opened);
            let foreign = darwin_art_linker_registry_create();
            assert_eq!(
                darwin_art_linker_group_detach(foreign, lookup, &mut detached),
                -1
            );
            darwin_art_linker_registry_destroy(foreign);
            let mut context = FinalizeContext {
                registry,
                id: id.raw(),
                fail: true,
                calls: 0,
            };
            if flags == 0 {
                assert_eq!(
                    darwin_art_linker_group_finalize_and_detach(
                        registry,
                        lookup,
                        Some(finalize),
                        (&mut context as *mut FinalizeContext).cast(),
                        &mut detached
                    ),
                    -6
                );
                assert!(detached.is_null());
                assert_eq!(context.calls, 1);
                assert_eq!(darwin_art_linker_image_acquire_open(lookup, &mut opened), 0);
                darwin_art_linker_image_release(opened);
            }
            context.fail = false;
            let result = darwin_art_linker_group_finalize_and_detach(
                registry,
                lookup,
                Some(finalize),
                (&mut context as *mut FinalizeContext).cast(),
                &mut detached,
            );
            if flags != 0 {
                assert_eq!(result, 1);
                assert_eq!(context.calls, 0);
                assert!(detached.is_null());
            } else {
                assert_eq!(result, 0);
                assert_eq!(context.calls, 2);
                assert!(!detached.is_null());
                assert_eq!(darwin_art_linker_handles_retire_group(registry, 401), 0);
                assert_eq!(
                    darwin_art_linker_handle_image(registry, handle, &mut borrowed),
                    -1
                );
                assert!(borrowed.is_null());
                assert_eq!(
                    darwin_art_linker_find_image_address(
                        registry,
                        1,
                        7,
                        Some(contains),
                        registry.cast(),
                        &mut resolved
                    ),
                    1
                );
                assert!(resolved.is_null());
                for namespace in [id, shared] {
                    assert!(
                        (&*registry)
                            .0
                            .lock()
                            .unwrap()
                            .find(namespace, "same.so")
                            .unwrap()
                            .is_none()
                    );
                }
                assert_eq!(
                    darwin_art_linker_image_acquire_open(lookup, &mut opened),
                    -1
                );
                assert!(opened.is_null());
                let mut second = std::ptr::null_mut();
                assert_eq!(
                    darwin_art_linker_group_detach(registry, lookup, &mut second),
                    -1
                );
                assert!(second.is_null());
                assert_eq!(
                    darwin_art_linker_namespace_publish_linked_groups(
                        registry,
                        &record,
                        &group,
                        1,
                        std::ptr::null(),
                        0,
                        std::ptr::null_mut()
                    ),
                    -1
                );
                darwin_art_linker_detached_group_destroy(detached);
            }
            darwin_art_linker_image_release(lookup);
            darwin_art_linker_registry_destroy(registry);
        }
    }
}
