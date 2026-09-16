use super::group_metadata::*;
use super::publication_transaction::*;
use super::retention_flags::*;
use super::*;

unsafe extern "C" fn retain(value: *mut c_void) -> *mut c_void {
    value
}
unsafe extern "C" fn release(_: *mut c_void) {}
unsafe extern "C" fn all_match(_: *mut c_void, _: usize, _: *mut c_void) -> i32 {
    1
}

#[test]
fn member_flags_do_not_replace_original_root_policy() {
    unsafe {
        let registry = darwin_art_linker_registry_create();
        let id = (&*registry)
            .0
            .lock()
            .unwrap()
            .create(NamespaceConfig::default(), None)
            .unwrap()
            .raw();
        // These owners are metadata-only test tokens; no ELF execution claim.
        let source = 1u8;
        let record = |name: &CStr, flags_1| Publication {
            id,
            soname: name.as_ptr(),
            path: c"/image.so".as_ptr(),
            source: (&source as *const u8).cast_mut().cast(),
            retain: Some(retain),
            release: Some(release),
            flags_1,
            global: 0,
            kind: 1,
            device: 0,
            inode: 0,
            offset: 0,
        };
        // Root comes after child: root policy is not insertion-order selection.
        let records = [
            record(c"child.so", 2 | 8),
            record(c"root.so", 0),
            record(c"other.so", 0),
        ];
        let groups = [
            GroupMetadata { id: 70, is_root: 0 },
            GroupMetadata { id: 70, is_root: 1 },
            GroupMetadata { id: 71, is_root: 1 },
        ];
        let mut token = std::ptr::null_mut();
        let edges = [
            super::dependency_references::DependencyEdge {
                source_index: 0,
                target_group: 71,
            },
            super::dependency_references::DependencyEdge {
                source_index: 1,
                target_group: 71,
            },
            super::dependency_references::DependencyEdge {
                source_index: 0,
                target_group: 70,
            },
        ];
        assert_eq!(
            super::dependency_references::darwin_art_linker_namespace_publish_linked_groups(
                registry,
                records.as_ptr(),
                groups.as_ptr(),
                3,
                edges.as_ptr(),
                edges.len(),
                &mut token
            ),
            0
        );
        let find = |name: &CStr| {
            let mut lease = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(registry, id, name.as_ptr(), &mut lease),
                0
            );
            lease
        };
        let child = find(c"child.so");
        let mut unknown_sdk = 99;
        assert_eq!(
            super::image_sdk::darwin_art_linker_image_target_sdk(child, &mut unknown_sdk),
            -4
        );
        assert_eq!(unknown_sdk, 0); // Legacy publications do not invent SDK36.
        // This member is GLOBAL even though its SDK is unknown. Linear lookup
        // eligibility is per image, not inherited from its non-GLOBAL root.
        let mut linear = 99;
        assert_eq!(
            super::image_sdk::darwin_art_linker_image_linear_lookup_eligible(child, &mut linear),
            0
        );
        assert_eq!(linear, 1);
        let mut ambiguous = std::ptr::null_mut();
        assert_eq!(
            super::caller_lookup::darwin_art_linker_find_image_address(
                registry,
                1,
                7,
                Some(all_match),
                std::ptr::null_mut(),
                &mut ambiguous
            ),
            -5
        );
        assert!(ambiguous.is_null());
        let root = find(c"root.so");
        let mut original_root = std::ptr::null_mut();
        assert_eq!(
            darwin_art_linker_image_local_group_root(registry, child, &mut original_root),
            0
        );
        let mut same = 0;
        assert_eq!(
            super::image_kind::darwin_art_linker_image_same(root, original_root, &mut same),
            0
        );
        assert_eq!(same, 1);
        darwin_art_linker_image_release(original_root);
        use super::dependency_references::darwin_art_linker_image_dependency_target;
        let mut target = 99;
        for image in [root, child] {
            for index in 0..2 {
                assert_eq!(
                    darwin_art_linker_image_dependency_target(image, index, &mut target),
                    0
                );
                assert_eq!(target, 71);
            }
            assert_eq!(
                darwin_art_linker_image_dependency_target(image, 2, &mut target),
                1
            );
            assert_eq!(target, 0);
        }
        assert_eq!(
            darwin_art_linker_image_dependency_target(std::ptr::null(), 0, &mut target),
            -1
        );
        assert_eq!(target, 0);
        let mut primary = 99;
        assert_eq!(
            super::image_kind::darwin_art_linker_image_registered_primary_namespace(
                registry,
                root,
                &mut primary
            ),
            0
        );
        assert_eq!(primary, id);
        let foreign = darwin_art_linker_registry_create();
        use super::dependency_lookup::darwin_art_linker_image_dependency_root;
        let mut dependency = std::ptr::null_mut();
        assert_eq!(
            darwin_art_linker_image_dependency_root(foreign, root, 0, &mut dependency),
            -1
        );
        assert!(dependency.is_null());
        assert_eq!(
            super::image_kind::darwin_art_linker_image_registered_primary_namespace(
                foreign,
                root,
                &mut primary
            ),
            -1
        );
        assert_eq!(primary, 0);
        darwin_art_linker_registry_destroy(foreign);
        let other = find(c"other.so");
        for image in [root, child] {
            for index in 0..2 {
                assert_eq!(
                    darwin_art_linker_image_dependency_root(
                        registry,
                        image,
                        index,
                        &mut dependency
                    ),
                    0
                );
                assert!(Arc::ptr_eq(&(*dependency).0, &(*other).0));
                assert!((*dependency).1.is_none());
                darwin_art_linker_image_release(dependency);
            }
        }
        assert_eq!(
            darwin_art_linker_image_dependency_root(registry, child, 2, &mut dependency),
            1
        );
        assert!(dependency.is_null());
        let observed_other = super::image_kind::darwin_art_linker_image_clone(other);
        let mut dependencies = 99;
        assert_eq!(
            super::dependency_references::darwin_art_linker_image_dependency_count(
                other,
                &mut dependencies
            ),
            0
        );
        assert_eq!(dependencies, 2); // Two original image edges, not one group-pair.
        assert_eq!(
            super::dependency_references::darwin_art_linker_image_dependency_count(
                child,
                &mut dependencies
            ),
            0
        );
        assert_eq!(dependencies, 0); // Local edges never increment group refs.
        use super::open_reference::*;
        let mut count = 99;
        assert_eq!(darwin_art_linker_image_open_count(child, &mut count), 0);
        assert_eq!(count, 0); // Lookups and snapshots do not create opens.
        let mut first_open = std::ptr::null_mut();
        let mut second_open = std::ptr::null_mut();
        assert_eq!(
            darwin_art_linker_image_acquire_open(root, &mut first_open),
            0
        );
        assert_eq!(
            darwin_art_linker_image_acquire_open(child, &mut second_open),
            0
        );
        let shared_open = super::image_kind::darwin_art_linker_image_clone(first_open);
        assert_eq!(darwin_art_linker_image_open_count(child, &mut count), 0);
        assert_eq!(count, 2);
        let original = first_open;
        let mut view = std::ptr::null_mut();
        assert_eq!(
            darwin_art_linker_image_consume_open(&mut first_open, &mut view),
            -7
        );
        assert_eq!(first_open, original);
        assert!(view.is_null());
        assert_eq!(
            darwin_art_linker_image_consume_open(&mut first_open, &mut first_open),
            -1
        );
        assert_eq!(first_open, original);
        darwin_art_linker_image_release(first_open);
        assert_eq!(darwin_art_linker_image_open_count(root, &mut count), 0);
        assert_eq!(count, 2); // Clone retains the same open, not a third one.
        darwin_art_linker_image_release(shared_open);
        assert_eq!(darwin_art_linker_image_open_count(child, &mut count), 0);
        assert_eq!(count, 1);
        assert_eq!(
            darwin_art_linker_image_consume_open(&mut second_open, &mut view),
            0
        );
        assert!(second_open.is_null());
        assert!(Arc::ptr_eq(&(*view).0, &(*child).0));
        assert_eq!(darwin_art_linker_image_open_count(child, &mut count), 0);
        assert_eq!(count, 0);
        let saved_view = view;
        let mut invalid_output = std::ptr::null_mut();
        assert_eq!(
            darwin_art_linker_image_consume_open(&mut view, &mut invalid_output),
            -4
        );
        assert_eq!(view, saved_view);
        assert!(invalid_output.is_null());
        // A pre-finalizer rejection can restore an open to this exact image.
        assert_eq!(
            darwin_art_linker_image_acquire_open(view, &mut second_open),
            0
        );
        assert_eq!(darwin_art_linker_image_open_count(child, &mut count), 0);
        assert_eq!(count, 1);
        darwin_art_linker_image_release(second_open);
        darwin_art_linker_image_release(view);
        let mut global = 9;
        let mut nodelete = 9;
        assert_eq!(
            darwin_art_linker_image_retention_flags(child, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (1, 1));
        assert_eq!(
            darwin_art_linker_image_group_retention_flags(child, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (0, 0));
        assert_eq!(darwin_art_linker_image_promote_nodelete(root), 0);
        assert_eq!(
            darwin_art_linker_image_group_retention_flags(child, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (0, 1));
        (&*root).0.visibility.promote_global();
        assert_eq!(
            darwin_art_linker_image_group_retention_flags(child, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (1, 1));
        assert_eq!(
            darwin_art_linker_image_group_retention_flags(other, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (0, 0));
        assert_eq!(darwin_art_linker_publication_rollback(registry, token), 0);
        assert_eq!(
            darwin_art_linker_image_dependency_root(registry, child, 0, &mut dependency),
            -1
        );
        assert!(dependency.is_null());
        darwin_art_linker_publication_destroy(token);
        assert_eq!(
            super::image_kind::darwin_art_linker_image_registered_primary_namespace(
                registry,
                root,
                &mut primary
            ),
            -1
        );
        assert_eq!(primary, 0);
        darwin_art_linker_image_release(root);
        darwin_art_linker_image_release(other);
        darwin_art_linker_registry_destroy(registry);
        assert_eq!(
            darwin_art_linker_image_dependency_target(child, 1, &mut target),
            0
        );
        assert_eq!(target, 71); // Original edge identity survives registry teardown.
        assert_eq!(
            darwin_art_linker_image_group_retention_flags(child, &mut global, &mut nodelete),
            0
        );
        assert_eq!((global, nodelete), (1, 1));
        darwin_art_linker_image_release(child);
        assert_eq!(
            super::dependency_references::darwin_art_linker_image_dependency_count(
                observed_other,
                &mut dependencies
            ),
            0
        );
        assert_eq!(dependencies, 0);
        darwin_art_linker_image_release(observed_other);
    }
}
