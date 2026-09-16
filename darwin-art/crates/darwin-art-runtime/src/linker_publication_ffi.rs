//! Native resource preparation outside the registry lock, atomic publication
//! inside it. Failed preparation owns no partially visible namespace entries.
use super::*;
#[cfg(test)]
#[path = "linker_group_detach_tests.rs"]
mod group_detach_tests;
#[cfg(test)]
#[path = "linker_group_root_flags_tests.rs"]
mod group_root_flags_tests;

#[repr(C)]
pub struct Publication {
    id: u64,
    soname: *const c_char,
    path: *const c_char,
    source: *mut c_void,
    retain: Option<RetainImage>,
    release: Option<ReleaseImage>,
    flags_1: u64,
    global: u8,
    kind: u32,
    device: u64,
    inode: u64,
    offset: u64,
}

/// # Safety
/// Registry, records and strings stay live for this call. Callbacks implement
/// the typed image publication contract and may reenter the registry. No
/// concurrent registry destruction. No ownership transfer of input records.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_batch(
    registry: *mut LinkerRegistry,
    records: *const Publication,
    count: usize,
) -> i32 {
    unsafe { publish(registry, records, count, std::ptr::null_mut()) }
}

pub(super) unsafe fn publish(
    registry: *mut LinkerRegistry,
    records: *const Publication,
    count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
) -> i32 {
    unsafe { publish_with_groups(registry, records, count, output, None, &[], false, None) }
}

pub(super) unsafe fn publish_with_groups(
    registry: *mut LinkerRegistry,
    records: *const Publication,
    count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
    groups: Option<&[group_metadata::GroupMetadata]>,
    edges: &[dependency_references::DependencyEdge],
    dependencies_complete: bool,
    target_sdk: Option<i32>,
) -> i32 {
    let origin = registry as usize;
    if !output.is_null() {
        unsafe { *output = std::ptr::null_mut() };
    }
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return -1;
    };
    if count > isize::MAX as usize / std::mem::size_of::<Publication>()
        || (count != 0 && records.is_null())
    {
        return -1;
    }
    let records = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(records, count) }
    };
    let mut prepared = Vec::with_capacity(count);
    if let Some(groups) = groups {
        if groups.len() != count
            || !group_metadata::valid(groups)
            || records.iter().any(|record| record.kind != 1)
        {
            return -1;
        }
    }
    for record in records {
        if record.global > 1
            || !matches!(record.kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8)
            || record.path.is_null()
            || record.source.is_null()
        {
            return -1;
        }
        let (Some(soname), Some(retain), Some(release)) = (
            unsafe { text(record.soname) },
            record.retain,
            record.release,
        ) else {
            return -1;
        };
        if !crate::linker_namespace::library_name(&soname) {
            return -1;
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(
            unsafe { CStr::from_ptr(record.path) }.to_bytes().to_vec(),
        ));
        prepared.push((record, soname, path, retain, release));
    }
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        for (record, _, path, _, _) in &prepared {
            if state.permits(NamespaceId::from_raw(record.id), path) != Ok(true) {
                return -1;
            }
        }
        let Ok(retired) = registry.2.lock() else {
            return -2;
        };
        if groups
            .into_iter()
            .flatten()
            .any(|group| retired.contains(&group.id))
        {
            return -1;
        }
    }
    let mut images = Vec::with_capacity(count);
    let visibility: Vec<_> = records
        .iter()
        .map(|record| {
            crate::linker_namespace::ImageVisibility::new(record.flags_1, record.global != 0)
        })
        .collect();
    // Capture the original root's shared flag owner, not a copy of its bits
    // or a union of member flags. Future promotions must be observed by every
    // member, including lookup leases surviving publication rollback.
    let roots: std::collections::BTreeMap<_, _> = groups
        .into_iter()
        .flatten()
        .zip(&visibility)
        .filter(|(group, _)| group.is_root != 0)
        .map(|(group, visibility)| (group.id, visibility.retained_flags()))
        .collect();
    let open_counts: std::collections::BTreeMap<_, _> = roots
        .keys()
        .map(|&id| (id, Arc::new(std::sync::atomic::AtomicUsize::new(0))))
        .collect();
    let incoming: std::collections::BTreeMap<_, _> = roots
        .keys()
        .map(|&id| (id, Arc::new(std::sync::atomic::AtomicUsize::new(0))))
        .collect();
    let retired: std::collections::BTreeMap<_, _> = roots
        .keys()
        .map(|&id| (id, Arc::new(std::sync::atomic::AtomicBool::new(false))))
        .collect();
    let outgoing =
        match dependency_references::prepare(registry, groups.unwrap_or(&[]), &incoming, edges) {
            Ok(value) => value,
            Err(status) => return status,
        };
    for (index, ((record, soname, path, retain, release), visibility)) in
        prepared.into_iter().zip(visibility).enumerate()
    {
        let value = unsafe { retain(record.source) };
        if value.is_null() {
            return -3;
        }
        images.push((
            NamespaceId::from_raw(record.id),
            Arc::new(NamespaceImage {
                visibility,
                soname,
                path,
                lease: Arc::new(ImageOwner {
                    target_sdk,
                    group: groups.map(|groups| groups[index]),
                    group_root_flags: groups.map(|groups| roots[&groups[index].id].clone()),
                    group_opens: groups.map(|groups| open_counts[&groups[index].id].clone()),
                    group_incoming: groups.map(|groups| incoming[&groups[index].id].clone()),
                    group_dependencies_complete: groups.is_some() && dependencies_complete,
                    group_retired: groups.map(|groups| retired[&groups[index].id].clone()),
                    _group_edges: groups
                        .and_then(|groups| outgoing.get(&groups[index].id).cloned()),
                    file_identity: (record.device != 0 && record.inode != 0).then_some((
                        record.device,
                        record.inode,
                        record.offset,
                    )),
                    value: value as usize,
                    kind: record.kind,
                    primary_namespace: record.id,
                    release,
                }),
            }),
        ));
    }
    let result = {
        let Ok(mut state) = registry.0.lock() else {
            return -2;
        };
        let Ok(retired) = registry.2.lock() else {
            return -2;
        };
        if groups
            .into_iter()
            .flatten()
            .any(|group| retired.contains(&group.id))
        {
            return -1;
        }
        state.publish_batch(&images)
    };
    // Last failed owner references drop only after the registry guard above.
    if result.is_err() {
        return -1;
    }
    if !output.is_null() {
        unsafe {
            *output = Box::into_raw(Box::new(publication_transaction::LinkerPublication {
                origin,
                images: images.into_iter().map(|(_, image)| image).collect(),
            }))
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Owner {
        registry: *mut LinkerRegistry,
        fail: AtomicBool,
        retains: AtomicUsize,
        releases: AtomicUsize,
    }
    #[test]
    fn grouped_publication_retains_metadata_through_rollback_and_rejects_missing_root() {
        use super::group_metadata::*;
        use super::publication_transaction::*;
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let id = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), None)
                .unwrap()
                .raw();
            let owner = Owner {
                registry,
                fail: AtomicBool::new(false),
                retains: AtomicUsize::new(0),
                releases: AtomicUsize::new(0),
            };
            let record = Publication {
                id,
                soname: c"root.so".as_ptr(),
                path: c"/root.so".as_ptr(),
                source: (&owner as *const Owner).cast_mut().cast(),
                retain: Some(retain),
                release: Some(release),
                flags_1: 0,
                global: 0,
                kind: 1,
                device: 1,
                inode: 2,
                offset: 0,
            };
            let mut group = GroupMetadata { id: 99, is_root: 0 };
            let mut token = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_publish_groups(
                    registry, &record, &group, 1, &mut token
                ),
                -1
            );
            assert!(token.is_null());
            assert_eq!(owner.retains.load(Ordering::Relaxed), 0);
            group.is_root = 1;
            assert_eq!(
                darwin_art_linker_namespace_publish_groups(
                    registry, &record, &group, 1, &mut token
                ),
                0
            );
            let mut lease = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_find(registry, id, c"root.so".as_ptr(), &mut lease),
                0
            );
            let mut found = GroupMetadata { id: 0, is_root: 0 };
            assert_eq!(darwin_art_linker_image_group_metadata(lease, &mut found), 0);
            assert_eq!(found, group);
            assert_eq!(darwin_art_linker_publication_rollback(registry, token), 0);
            darwin_art_linker_publication_destroy(token);
            assert_eq!(owner.releases.load(Ordering::Relaxed), 0);
            assert_eq!(darwin_art_linker_image_group_metadata(lease, &mut found), 0);
            assert_eq!(found, group);
            darwin_art_linker_image_release(lease);
            assert_eq!(owner.releases.load(Ordering::Relaxed), 1);
            darwin_art_linker_registry_destroy(registry);
        }
    }
    unsafe extern "C" fn retain(pointer: *mut c_void) -> *mut c_void {
        let owner = unsafe { &*pointer.cast::<Owner>() };
        assert!(unsafe { &*owner.registry }.0.try_lock().is_ok());
        if owner.fail.load(Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        owner.retains.fetch_add(1, Ordering::Relaxed);
        pointer
    }
    unsafe extern "C" fn release(pointer: *mut c_void) {
        let owner = unsafe { &*pointer.cast::<Owner>() };
        assert!(unsafe { &*owner.registry }.0.try_lock().is_ok());
        owner.releases.fetch_add(1, Ordering::Relaxed);
    }
    #[test]
    fn failed_retention_rolls_back_without_lock_or_partial_publication() {
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let id = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), None)
                .unwrap()
                .raw();
            let first = Owner {
                registry,
                fail: AtomicBool::new(false),
                retains: AtomicUsize::new(0),
                releases: AtomicUsize::new(0),
            };
            let second = Owner {
                registry,
                fail: AtomicBool::new(true),
                retains: AtomicUsize::new(0),
                releases: AtomicUsize::new(0),
            };
            let record = |name: &CStr, owner: &Owner| Publication {
                id,
                soname: name.as_ptr(),
                path: c"/system/image.so".as_ptr(),
                source: (owner as *const Owner).cast_mut().cast(),
                retain: Some(retain),
                release: Some(release),
                flags_1: 0,
                global: 0,
                kind: 1,
                device: 7,
                inode: if name == c"first.so" { 1 } else { 2 },
                offset: 0,
            };
            let records = [record(c"first.so", &first), record(c"second.so", &second)];
            assert_eq!(
                darwin_art_linker_namespace_publish_batch(registry, records.as_ptr(), 2),
                -3
            );
            assert_eq!(first.retains.load(Ordering::Relaxed), 1);
            assert_eq!(first.releases.load(Ordering::Relaxed), 1);
            assert!(
                (&*registry)
                    .0
                    .lock()
                    .unwrap()
                    .find(NamespaceId::from_raw(id), "first.so")
                    .unwrap()
                    .is_none()
            );
            second.fail.store(false, Ordering::Relaxed);
            use super::publication_transaction::*;
            let mut token = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_publish_transaction(
                    registry,
                    records.as_ptr(),
                    2,
                    &mut token
                ),
                0
            );
            assert_eq!(first.releases.load(Ordering::Relaxed), 1);
            let foreign = darwin_art_linker_registry_create();
            let mut by_file = std::ptr::null_mut();
            assert_eq!(
                file_lookup::darwin_art_linker_namespace_find_file(
                    registry,
                    id,
                    7,
                    1,
                    0,
                    &mut by_file
                ),
                0
            );
            darwin_art_linker_image_release(by_file);
            assert_eq!(
                file_lookup::darwin_art_linker_namespace_find_file(
                    registry,
                    id,
                    7,
                    1,
                    1,
                    &mut by_file
                ),
                1
            );
            assert!(by_file.is_null());
            let client = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), None)
                .unwrap();
            (&*registry)
                .0
                .lock()
                .unwrap()
                .link(
                    client,
                    NamespaceId::from_raw(id),
                    ["second.so".into()].into_iter().collect(),
                )
                .unwrap();
            assert_eq!(
                file_lookup::darwin_art_linker_namespace_find_file(
                    registry,
                    client.raw(),
                    7,
                    1,
                    0,
                    &mut by_file
                ),
                1
            );
            assert_eq!(
                file_lookup::darwin_art_linker_namespace_find_file(
                    registry,
                    client.raw(),
                    7,
                    2,
                    0,
                    &mut by_file
                ),
                0
            );
            darwin_art_linker_image_release(by_file);
            assert_eq!(darwin_art_linker_publication_rollback(foreign, token), -1);
            darwin_art_linker_registry_destroy(foreign);
            assert_eq!(darwin_art_linker_publication_rollback(registry, token), 0);
            assert_eq!(darwin_art_linker_publication_rollback(registry, token), 0);
            assert!(
                (&*registry)
                    .0
                    .lock()
                    .unwrap()
                    .find(NamespaceId::from_raw(id), "first.so")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(first.releases.load(Ordering::Relaxed), 1);
            darwin_art_linker_publication_destroy(token);
            assert_eq!(first.releases.load(Ordering::Relaxed), 2);
            assert_eq!(second.releases.load(Ordering::Relaxed), 1);
            darwin_art_linker_registry_destroy(registry);
        }
    }
}
