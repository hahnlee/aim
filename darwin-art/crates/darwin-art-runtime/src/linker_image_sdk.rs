//! Immutable SDK at link/publication time, owned by the exact image lease.
//! Legacy publications remain unknown rather than inheriting a later SDK.
use super::*;

/// # Safety
/// Linked-group publication preconditions apply. The caller holds its recursive
/// linker operation while capturing the normalized SDK and publishing images.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_linked_groups_for_sdk(
    registry: *mut LinkerRegistry,
    records: *const publication::Publication,
    groups: *const group_metadata::GroupMetadata,
    count: usize,
    edges: *const dependency_references::DependencyEdge,
    edge_count: usize,
    output: *mut *mut publication_transaction::LinkerPublication,
    target_sdk: i32,
) -> i32 {
    unsafe {
        dependency_references::publish_linked_groups(
            registry,
            records,
            groups,
            count,
            edges,
            edge_count,
            output,
            Some(target_sdk),
        )
    }
}

/// # Safety
/// Live image lease and writable output. -4 means unknown legacy metadata,
/// not platform API or zero. No registry or process SDK is consulted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_target_sdk(
    image: *const LinkerImageLease,
    output: *mut i32,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = 0;
    let Some(image) = (unsafe { image.as_ref() }) else {
        return -1;
    };
    let Some(sdk) = image.0.lease.target_sdk else {
        return -4;
    };
    *output = sdk;
    0
}

/// # Safety
/// Live image and writable output. Apply in namespace order in the linear
/// dlsym phase only. -4 means SDK-dependent eligibility cannot be determined;
/// do not silently skip that candidate or apply this to ordinary handle lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_linear_lookup_eligible(
    image: *const LinkerImageLease,
    output: *mut u8,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = 0;
    let Some(image) = (unsafe { image.as_ref() }) else {
        return -1;
    };
    let Some(eligible) = image
        .0
        .visibility
        .linear_lookup_eligible(image.0.lease.target_sdk)
    else {
        return -4;
    };
    *output = u8::from(eligible);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_sdk_query_clears_output() {
        unsafe {
            let mut sdk = 99;
            assert_eq!(
                darwin_art_linker_image_target_sdk(std::ptr::null(), &mut sdk),
                -1
            );
            assert_eq!(sdk, 0);
            assert_eq!(
                darwin_art_linker_image_target_sdk(std::ptr::null(), std::ptr::null_mut()),
                -1
            );
        }
    }
}
