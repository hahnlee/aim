//! Original ELF local-group identity at the publication boundary.
//! This is not an Android open count or an unload eligibility decision.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupMetadata {
    pub(super) id: u64,
    pub(super) is_root: u8,
}

pub(super) fn valid(groups: &[GroupMetadata]) -> bool {
    let mut roots = std::collections::BTreeMap::<u64, usize>::new();
    for group in groups {
        if group.id == 0 || group.is_root > 1 {
            return false;
        }
        *roots.entry(group.id).or_default() += usize::from(group.is_root);
    }
    roots.values().all(|&count| count == 1)
}

/// # Safety
/// Publication contract applies. Groups and records contain count entries.
/// IDs come from the same live ELF loader instance; each complete original
/// group has exactly one root. Output is optional, with transaction semantics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_groups(
    registry: *mut LinkerRegistry,
    records: *const publication::Publication,
    groups: *const GroupMetadata,
    count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
) -> i32 {
    if !output.is_null() {
        unsafe { *output = std::ptr::null_mut() };
    }
    if count > isize::MAX as usize / std::mem::size_of::<GroupMetadata>()
        || (count != 0 && groups.is_null())
    {
        return -1;
    }
    let groups = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(groups, count) }
    };
    unsafe {
        publication::publish_with_groups(
            registry,
            records,
            count,
            output,
            Some(groups),
            &[],
            false,
            None,
        )
    }
}

/// # Safety
/// Live lease and writable output. Legacy publications return -4 (unknown),
/// never a fabricated group identity. This does not retain an open reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_group_metadata(
    lease: *const LinkerImageLease,
    output: *mut GroupMetadata,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = GroupMetadata { id: 0, is_root: 0 };
    let Some(lease) = (unsafe { lease.as_ref() }) else {
        return -1;
    };
    let Some(group) = lease.0.lease.group else {
        return -4;
    };
    *output = group;
    0
}

/// # Safety
/// Live registered member and registry under a complete linker operation.
/// Returns a retained non-counting lease to the original local-group root.
/// No SONAME/path search; group reference-owner identity must also match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_local_group_root(
    registry: *const LinkerRegistry,
    member: *const LinkerImageLease,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        *output = std::ptr::null_mut();
    }
    let (Some(registry), Some(member)) = (unsafe { registry.as_ref() }, unsafe { member.as_ref() })
    else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    if !state
        .resident_images()
        .any(|image| Arc::ptr_eq(image, &member.0))
    {
        return -1;
    }
    let (Some(group), Some(opens)) = (member.0.lease.group, member.0.lease.group_opens.as_ref())
    else {
        return -4;
    };
    let mut root: Option<Arc<NamespaceImage<ImageOwner>>> = None;
    for image in state.resident_images() {
        if image
            .lease
            .group
            .is_some_and(|candidate| candidate.id == group.id && candidate.is_root == 1)
            && image
                .lease
                .group_opens
                .as_ref()
                .is_some_and(|count| Arc::ptr_eq(count, opens))
        {
            if root
                .as_ref()
                .is_some_and(|previous| !Arc::ptr_eq(previous, image))
            {
                return -3;
            }
            root = Some(image.clone());
        }
    }
    let Some(root) = root else {
        return -3;
    };
    drop(state);
    unsafe {
        *output = Box::into_raw(Box::new(LinkerImageLease(root, None)));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_original_groups_require_exactly_one_root() {
        let root = GroupMetadata { id: 42, is_root: 1 };
        let member = GroupMetadata { id: 42, is_root: 0 };
        assert!(valid(&[]));
        assert!(valid(&[member, root, GroupMetadata { id: 43, is_root: 1 }]));
        assert!(!valid(&[member]));
        assert!(!valid(&[root, root]));
        assert!(!valid(&[GroupMetadata { id: 0, is_root: 1 }]));
        assert!(!valid(&[GroupMetadata { id: 42, is_root: 2 }]));
    }
}
