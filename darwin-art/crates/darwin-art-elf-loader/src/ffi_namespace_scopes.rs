//! Copy linker-owned scope decisions by admitted image identity, not caller
//! supplied SONAME aliases. Updates are atomic and remain owned by discovery.
use super::*;
use std::collections::{HashMap, HashSet};

#[repr(C)]
pub struct ImagePlacement {
    pub image: u64,
    pub namespace_id: u64,
}
#[repr(C)]
pub struct NamespaceScope {
    pub namespace_id: u64,
    pub visible_images: *const u64,
    pub visible_count: usize,
    pub global_indices: *const usize,
    pub global_count: usize,
}
unsafe fn borrowed<'a, T>(pointer: *const T, count: usize) -> Result<&'a [T], FfiFailure> {
    if count > isize::MAX as usize / std::mem::size_of::<T>() || (count != 0 && pointer.is_null()) {
        return Err(FfiFailure::Invalid("invalid namespace scope array"));
    }
    Ok(if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pointer, count) }
    })
}
/// # Safety
/// Discovery exclusively borrowed (no concurrent discovery/load/destruction).
/// Arrays readable through this call. All decisions are trusted linker-owner
/// inputs; this validates identities and shape, not platform namespace policy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_discovered_graph_set_namespace_scopes(
    discovered: *mut DarwinArtElfDiscoveredGraph,
    placements: *const ImagePlacement,
    placement_count: usize,
    scopes: *const NamespaceScope,
    scope_count: usize,
    global_count: usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        let graph = unsafe { discovered.as_mut() }.ok_or(FfiFailure::Invalid("null discovery"))?;
        let mut identities = HashMap::new();
        for (&id, name) in graph.source_images.iter().zip(&graph._names) {
            identities.insert(
                id,
                name.to_str().expect("validated discovery name").to_owned(),
            );
        }
        for resident in &graph._residents {
            identities.insert(
                resident.image,
                resident
                    .name
                    .to_str()
                    .expect("validated resident name")
                    .to_owned(),
            );
        }
        if placement_count != identities.len() || scope_count > placement_count {
            return Err(FfiFailure::Invalid(
                "namespace placements must cover every admitted image",
            ));
        }
        let mut result = crate::NamespaceScopes::default();
        let mut namespaces = HashSet::new();
        for placement in unsafe { borrowed(placements, placement_count)? } {
            let name = identities
                .get(&placement.image)
                .ok_or(FfiFailure::Invalid("unknown placement image"))?;
            if placement.namespace_id == 0
                || result
                    .primary
                    .insert(name.clone(), placement.namespace_id)
                    .is_some()
            {
                return Err(FfiFailure::Invalid("invalid or duplicate placement"));
            }
            namespaces.insert(placement.namespace_id);
        }
        for scope in unsafe { borrowed(scopes, scope_count)? } {
            if !namespaces.contains(&scope.namespace_id)
                || result.accessible.contains_key(&scope.namespace_id)
                || scope.visible_count > identities.len()
                || scope.global_count > global_count
            {
                return Err(FfiFailure::Invalid("invalid namespace scope"));
            }
            let mut visible = HashSet::new();
            for id in unsafe { borrowed(scope.visible_images, scope.visible_count)? } {
                let name = identities
                    .get(id)
                    .ok_or(FfiFailure::Invalid("unknown visible image"))?;
                if !visible.insert(name.clone()) {
                    return Err(FfiFailure::Invalid("duplicate visible image"));
                }
            }
            let globals = unsafe { borrowed(scope.global_indices, scope.global_count)? }.to_vec();
            if globals.iter().copied().collect::<HashSet<_>>().len() != globals.len() {
                return Err(FfiFailure::Invalid("duplicate global image index"));
            }
            result.accessible.insert(scope.namespace_id, visible);
            result.globals.insert(scope.namespace_id, globals);
        }
        result
            .validate(&identities.into_values().collect::<Vec<_>>(), global_count)
            .map_err(FfiFailure::Namespace)?;
        graph.namespace_scopes = Some((result, global_count));
        Ok(())
    })
}
