//! Recover the original mapped local-group root, never perform a new lookup
//! by SONAME in a possibly different namespace registry.
use super::ffi_selected_image::DarwinArtElfSelectedImage;
use super::*;
/// # Safety
/// Live selected image. Caller establishes original group unload eligibility,
/// complete parent ownership, serialization and native quiescence. This runs
/// destructors synchronously but does not unmap or release dependency groups.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_group_finalize(
    image: *const DarwinArtElfSelectedImage,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("null selected image"))?;
        let selected =
            select_graph_image(&image.graph, image.soname.to_str().expect("validated name"))?;
        unsafe { selected.finalize_local_group() }.map_err(FfiFailure::Invalid)
    })
}
/// # Safety
/// Live selected image and distinct writable outputs. IDs are scoped to the
/// loader instance, not addresses, serialized handles or dlopen references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_group_info(
    image: *const DarwinArtElfSelectedImage,
    id: *mut u64,
    is_root: *mut u8,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if id.is_null() || is_root.is_null() {
            return Err(FfiFailure::Invalid("null group metadata output"));
        }
        unsafe {
            *id = 0;
            *is_root = 0;
        }
        let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("null selected image"))?;
        let selected =
            select_graph_image(&image.graph, image.soname.to_str().expect("validated name"))?;
        unsafe {
            *id = selected.local_group_id();
            *is_root = u8::from(selected.is_local_group_root());
        }
        Ok(())
    })
}
/// # Safety
/// Selected image live; output writable and non-aliasing. Returned root owns a
/// selected-image lease. This grants identity/retention, not dlopen reference
/// count changes or permission to finalize/unmap the group independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_group_root(
    image: *const DarwinArtElfSelectedImage,
    output: *mut *mut DarwinArtElfSelectedImage,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null group root output"));
        }
        unsafe {
            *output = ptr::null_mut();
        }
        let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("null selected image"))?;
        let selected =
            select_graph_image(&image.graph, image.soname.to_str().expect("validated name"))?;
        let root = selected
            .local_group_root_image()
            .ok_or(FfiFailure::Invalid("missing original local group root"))?;
        let root = ffi_selected_image::retained(root)?;
        unsafe {
            *output = Box::into_raw(Box::new(root));
        }
        Ok(())
    })
}
