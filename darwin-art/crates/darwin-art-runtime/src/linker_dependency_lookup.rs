//! Resolve an original dependency edge before retiring its source group.
//! Group IDs alone are not authority: both source membership and the original
//! target counter allocation must agree with this registry.
use super::*;

/// # Safety
/// Live registry/source and writable output under one complete linker operation.
/// Call before source detachment. Returns a non-counting owning root lease (0),
/// edge end (1), invalid source (-1), unknown legacy graph (-4), or inconsistent
/// target (-3). No open/dependency counts change and no foreign callbacks run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_image_dependency_root(
    registry: *const LinkerRegistry,
    source: *const LinkerImageLease,
    index: usize,
    output: *mut *mut LinkerImageLease,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { *output = std::ptr::null_mut() };
    let (Some(registry), Some(source)) = (unsafe { registry.as_ref() }, unsafe { source.as_ref() })
    else {
        return -1;
    };
    let Ok(state) = registry.0.lock() else {
        return -2;
    };
    if !state
        .resident_images()
        .any(|image| Arc::ptr_eq(image, &source.0))
    {
        return -1;
    }
    if !source.0.lease.group_dependencies_complete {
        return -4;
    }
    let Some(edge) = source
        .0
        .lease
        ._group_edges
        .as_ref()
        .and_then(|edges| edges.get(index))
    else {
        return 1;
    };
    let mut root = None;
    for image in state.resident_images() {
        if image
            .lease
            .group
            .is_some_and(|group| group.id == edge.target_group && group.is_root != 0)
            && image
                .lease
                .group_incoming
                .as_ref()
                .is_some_and(|count| Arc::ptr_eq(count, &edge.incoming))
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
    unsafe { *output = Box::into_raw(Box::new(LinkerImageLease(root, None))) };
    0
}
