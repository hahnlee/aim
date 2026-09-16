//! Exact mapping identity at the private loader boundary.
use super::ffi_selected_image::DarwinArtElfSelectedImage;
use super::*;

unsafe fn selected(
    image: *const DarwinArtElfSelectedImage,
) -> Result<crate::GlobalElfImage, FfiFailure> {
    let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("null selected image"))?;
    select_graph_image(
        &image.graph,
        image.soname.to_str().expect("validated SONAME"),
    )
}

/// # Safety
/// Live selected image and writable result. Address is a numeric query, never
/// dereferenced. Reports only this image's original PT_LOAD coverage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_contains_address(
    image: *const DarwinArtElfSelectedImage,
    address: usize,
    output: *mut i32,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null address membership output"));
        }
        unsafe { *output = 0 };
        let image = unsafe { selected(image) }?;
        unsafe { *output = i32::from(image.contains_address(address)) };
        Ok(())
    })
}

/// # Safety
/// Live selected image and writable range outputs. Address is a numeric query,
/// never dereferenced. The returned half-open range is the exact executable
/// PT_LOAD in this retained image and remains valid while the image is live.
/// A data-segment, hole, adjacent-image, or out-of-image address returns
/// `DARWIN_ART_ELF_SYMBOL_NOT_FOUND` and leaves the cleared outputs at zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_executable_range(
    image: *const DarwinArtElfSelectedImage,
    address: usize,
    begin: *mut usize,
    end: *mut usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if begin.is_null() || end.is_null() {
            return Err(FfiFailure::Invalid("null executable range output"));
        }
        unsafe {
            *begin = 0;
            *end = 0;
        }
        let image = unsafe { selected(image) }?;
        let range = image.executable_range_containing(address).ok_or_else(|| {
            FfiFailure::Load(LoadError::SymbolNotFound(
                "no executable PT_LOAD contains address".to_owned(),
            ))
        })?;
        unsafe {
            *begin = range.start;
            *end = range.end;
        }
        Ok(())
    })
}

/// # Safety
/// Live selected parent, NUL name and writable output. Returned selected image
/// owns the ORIGINAL declared ELF dependency, not a fresh SONAME lookup. Native
/// or ambiguous edges fail with cleared output. No Android open ref is acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_dependency_image(
    parent: *const DarwinArtElfSelectedImage,
    name: *const c_char,
    output: *mut *mut DarwinArtElfSelectedImage,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null dependency output"));
        }
        unsafe {
            *output = ptr::null_mut();
        }
        let name = unsafe { required_utf8(name, "null dependency name") }?;
        let parent = unsafe { selected(parent) }?;
        let dependency = parent.dependency_image(&name).ok_or(FfiFailure::Invalid(
            "original ELF dependency unavailable or ambiguous",
        ))?;
        let dependency = ffi_selected_image::retained(dependency)?;
        unsafe {
            *output = Box::into_raw(Box::new(dependency));
        }
        Ok(())
    })
}

/// # Safety
/// Live selected image, NUL name, writable output. Returned native pointer is
/// borrowed from the originally retained callback result until image release.
/// Only declared DT_NEEDED edges are exposed, not the whole retained group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_native_dependency(
    image: *const DarwinArtElfSelectedImage,
    name: *const c_char,
    output: *mut *mut c_void,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null native dependency output"));
        }
        unsafe { *output = ptr::null_mut() };
        let name = unsafe { required_utf8(name, "null dependency name") }?;
        let image = unsafe { selected(image) }?;
        if !image
            .needed_libraries()
            .iter()
            .any(|needed| needed == &name)
        {
            return Err(FfiFailure::Invalid("not a declared dependency"));
        }
        for owner in image.retained_native_owners() {
            let owner = owner
                .downcast_ref::<crate::namespace::image_resource::ImageResource>()
                .map_or(owner.as_ref(), |resource| resource.value.as_ref());
            if let Some(owner) = owner.downcast_ref::<ffi_native_owners::Retained>()
                && owner
                    .soname
                    .as_ref()
                    .is_some_and(|stored| stored.as_bytes() == name.as_bytes())
            {
                unsafe { *output = owner.pointer.as_ptr() };
                return Ok(());
            }
        }
        Err(FfiFailure::Invalid(
            "original native dependency owner unavailable",
        ))
    })
}

/// # Safety
/// Both selected images remain live; output is writable. No raw mapping
/// address/ownership pointer is exposed. Locks are never held simultaneously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_same(
    left: *const DarwinArtElfSelectedImage,
    right: *const DarwinArtElfSelectedImage,
    output: *mut i32,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null identity output"));
        }
        unsafe { *output = 0 };
        let left = unsafe { selected(left) }?;
        let right = unsafe { selected(right) }?;
        unsafe { *output = i32::from(left.same_image(&right)) };
        Ok(())
    })
}

/// # Safety
/// Live images, readable NUL dependency name, writable output. Unknown/non-ELF
/// original dependency is an error, not a name-based match or permission grant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_dependency_same(
    parent: *const DarwinArtElfSelectedImage,
    name: *const c_char,
    candidate: *const DarwinArtElfSelectedImage,
    output: *mut i32,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null identity output"));
        }
        unsafe { *output = 0 };
        let name = unsafe { required_utf8(name, "null dependency name") }?;
        let parent = unsafe { selected(parent) }?;
        let candidate = unsafe { selected(candidate) }?;
        let same = parent
            .matches_dependency(&name, &candidate)
            .ok_or(FfiFailure::Invalid(
                "original ELF dependency identity unavailable",
            ))?;
        unsafe { *output = i32::from(same) };
        Ok(())
    })
}
