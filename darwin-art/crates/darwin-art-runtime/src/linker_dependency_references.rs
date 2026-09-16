//! Original DT_NEEDED edges between local groups. Counters own no images, so
//! cycles cannot form Arc image cycles. Native lifetime stays with mapping leases.
use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
pub struct DependencyEdge {
    pub(super) source_index: usize,
    pub(super) target_group: u64,
}
pub(super) struct DependencyReference {
    pub(super) target_group: u64,
    pub(super) incoming: Arc<AtomicUsize>,
}
impl Drop for DependencyReference {
    fn drop(&mut self) {
        assert!(self.incoming.fetch_sub(1, Ordering::AcqRel) != 0);
    }
}
pub(super) fn prepare(
    registry: &LinkerRegistry,
    groups: &[group_metadata::GroupMetadata],
    incoming: &BTreeMap<u64, Arc<AtomicUsize>>,
    edges: &[DependencyEdge],
) -> Result<BTreeMap<u64, Arc<Vec<DependencyReference>>>, i32> {
    let mut targets = incoming.clone();
    let mut retiring = BTreeMap::new();
    {
        let state = registry.0.lock().map_err(|_| -2)?;
        for image in state.resident_images() {
            if let (Some(group), Some(count)) = (image.lease.group, &image.lease.group_incoming) {
                if let Some(retired) = &image.lease.group_retired {
                    retiring.insert(group.id, retired.clone());
                }
                if let Some(existing) = targets.get(&group.id) {
                    if !Arc::ptr_eq(existing, count) {
                        return Err(-1);
                    }
                } else {
                    targets.insert(group.id, count.clone());
                }
            }
        }
    }
    // Validate every edge before changing any existing target count.
    for edge in edges {
        if edge.source_index >= groups.len() || !targets.contains_key(&edge.target_group) {
            return Err(-1);
        }
        if retiring
            .get(&edge.target_group)
            .is_some_and(|state| state.load(Ordering::Acquire))
        {
            return Err(-1);
        }
    }
    let mut outgoing = BTreeMap::<u64, Vec<DependencyReference>>::new();
    for edge in edges {
        let source = groups[edge.source_index].id;
        if source == edge.target_group {
            continue;
        }
        let target = &targets[&edge.target_group];
        target
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_| -5)?;
        outgoing
            .entry(source)
            .or_default()
            .push(DependencyReference {
                target_group: edge.target_group,
                incoming: target.clone(),
            });
    }
    Ok(outgoing
        .into_iter()
        .map(|(id, refs)| (id, Arc::new(refs)))
        .collect())
}

/// # Safety
/// Group publication contract applies. Edges are original unique image edges
/// (two different target images in one group remain two edges). Targets belong
/// to this registry or this batch. Caller serializes the complete linker operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_linked_groups(
    registry: *mut LinkerRegistry,
    records: *const publication::Publication,
    groups: *const group_metadata::GroupMetadata,
    count: usize,
    edges: *const DependencyEdge,
    edge_count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
) -> i32 {
    unsafe {
        publish_linked_groups(
            registry, records, groups, count, edges, edge_count, output, None,
        )
    }
}

pub(super) unsafe fn publish_linked_groups(
    registry: *mut LinkerRegistry,
    records: *const publication::Publication,
    groups: *const group_metadata::GroupMetadata,
    count: usize,
    edges: *const DependencyEdge,
    edge_count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
    target_sdk: Option<i32>,
) -> i32 {
    if !output.is_null() {
        unsafe {
            *output = std::ptr::null_mut();
        }
    }
    if count > isize::MAX as usize / std::mem::size_of::<group_metadata::GroupMetadata>()
        || edge_count > isize::MAX as usize / std::mem::size_of::<DependencyEdge>()
        || (count != 0 && groups.is_null())
        || (edge_count != 0 && edges.is_null())
    {
        return -1;
    }
    let groups = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(groups, count) }
    };
    let edges = if edge_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(edges, edge_count) }
    };
    unsafe {
        publication::publish_with_groups(
            registry,
            records,
            count,
            output,
            Some(groups),
            edges,
            true,
            target_sdk,
        )
    }
}

/// # Safety
/// Live lease and writable output. Incoming dependency count only; zero does
/// not imply permission to unload. Legacy unregistered groups return -4.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_dependency_count(
    lease: *const LinkerImageLease,
    output: *mut usize,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = 0;
    }
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    if !lease.0.lease.group_dependencies_complete {
        return -4;
    }
    let Some(count) = &lease.0.lease.group_incoming else {
        return -4;
    };
    unsafe {
        *output = count.load(Ordering::Acquire);
    }
    0
}

/// # Safety
/// Live image lease and writable output under the complete linker operation.
/// Returns the original outgoing edge at index (0), end (1), or unknown legacy
/// graph (-4). IDs do not grant registry authority. No image/counter is acquired
/// or released; repeated targets retain original edge multiplicity and order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_dependency_target(
    lease: *const LinkerImageLease,
    index: usize,
    output: *mut u64,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = 0 };
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    if !lease.0.lease.group_dependencies_complete {
        return -4;
    }
    let Some(edge) = lease
        .0
        .lease
        ._group_edges
        .as_ref()
        .and_then(|edges| edges.get(index))
    else {
        return 1;
    };
    unsafe { *output = edge.target_group };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycles_balance_and_failed_preparation_restores_counts() {
        let raw = darwin_art_linker_registry_create();
        let registry = unsafe { &*raw };
        let groups = [
            group_metadata::GroupMetadata { id: 1, is_root: 1 },
            group_metadata::GroupMetadata { id: 2, is_root: 1 },
        ];
        let counts = BTreeMap::from([
            (1, Arc::new(AtomicUsize::new(0))),
            (2, Arc::new(AtomicUsize::new(0))),
        ]);
        let edges = [
            DependencyEdge {
                source_index: 0,
                target_group: 2,
            },
            DependencyEdge {
                source_index: 1,
                target_group: 1,
            },
        ];
        let refs = prepare(registry, &groups, &counts, &edges).unwrap();
        assert_eq!(refs[&1][0].target_group, 2);
        assert_eq!(refs[&2][0].target_group, 1);
        assert_eq!(counts[&1].load(Ordering::Acquire), 1);
        assert_eq!(counts[&2].load(Ordering::Acquire), 1);
        drop(refs);
        assert_eq!(counts[&1].load(Ordering::Acquire), 0);
        assert_eq!(counts[&2].load(Ordering::Acquire), 0);
        counts[&1].store(usize::MAX, Ordering::Release);
        assert!(matches!(
            prepare(registry, &groups, &counts, &edges),
            Err(-5)
        ));
        assert_eq!(counts[&2].load(Ordering::Acquire), 0);
        assert_eq!(counts[&1].load(Ordering::Acquire), usize::MAX);
        let invalid = [DependencyEdge {
            source_index: 0,
            target_group: 999,
        }];
        assert!(matches!(
            prepare(registry, &groups, &counts, &invalid),
            Err(-1)
        ));
        unsafe {
            darwin_art_linker_registry_destroy(raw);
        }
    }
}
