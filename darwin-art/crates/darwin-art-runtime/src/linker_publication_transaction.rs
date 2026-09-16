//! Exact publication rollback identity. Token retention is independent of
//! lookup leases; dropping the token commits (does not remove memberships).
use super::*;
pub struct LinkerPublication {
    pub(super) origin: usize,
    pub(super) images: Vec<Arc<NamespaceImage<ImageOwner>>>,
}
/// # Safety
/// Publication ABI preconditions apply. Output writable, token destroyed before
/// registry. Inputs borrowed; returned token owns the exact published identities.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_publish_transaction(
    registry: *mut LinkerRegistry,
    records: *const publication::Publication,
    count: usize,
    output: *mut *mut LinkerPublication,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe { publication::publish(registry, records, count, output) }
}
/// # Safety
/// Registry and token live, no concurrent token destruction. Token belongs to
/// this registry. Removal is idempotent; outstanding image leases remain live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_publication_rollback(
    registry: *mut LinkerRegistry,
    token: *const LinkerPublication,
) -> i32 {
    let Some(token) = (unsafe { token.as_ref() }) else {
        return -1;
    };
    if registry.is_null() || token.origin != registry as usize {
        return -1;
    }
    let Ok(mut state) = (unsafe { &*registry }).0.lock() else {
        return -2;
    };
    state.unpublish_images(&token.images);
    0
}
/// # Safety
/// Destroy token exactly once outside registry locks, before registry destruction.
/// Does not roll back; release callbacks may run here after a previous rollback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_publication_destroy(token: *mut LinkerPublication) {
    if !token.is_null() {
        drop(unsafe { Box::from_raw(token) });
    }
}
