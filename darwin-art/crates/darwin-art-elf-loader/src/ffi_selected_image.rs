//! Typed retained ELF image payload for namespace ownership.
use super::*;

pub struct DarwinArtElfSelectedImage {
    pub(super) graph: DarwinArtElfGraphHandle,
    pub(super) soname: CString,
    needed: Vec<CString>,
}

pub(super) fn retained(
    image: crate::GlobalElfImage,
) -> Result<DarwinArtElfSelectedImage, FfiFailure> {
    let soname = CString::new(image.soname()).map_err(|_| FfiFailure::Invalid("SONAME NUL"))?;
    let needed = image
        .needed_libraries()
        .iter()
        .map(|name| CString::new(name.as_str()).map_err(|_| FfiFailure::Invalid("dependency NUL")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DarwinArtElfSelectedImage {
        graph: DarwinArtElfGraphHandle {
            owner: GraphHandleOwner::Selected(image),
        },
        soname,
        needed,
    })
}

/// # Safety
/// Live graph, NUL name, writable output and valid optional error buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_select_image(
    graph: *const DarwinArtElfGraphHandle,
    soname: *const c_char,
    output: *mut *mut DarwinArtElfSelectedImage,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("image output is null"));
        }
        unsafe { *output = ptr::null_mut() };
        let graph = unsafe { graph.as_ref() }.ok_or(FfiFailure::Invalid("graph is null"))?;
        let soname = unsafe { required_utf8(soname, "SONAME is null") }?;
        let image = retained(select_graph_image(graph, &soname)?)?;
        unsafe { *output = Box::into_raw(Box::new(image)) };
        Ok(())
    })
}

/// # Safety
/// Live image and writable output. The returned name is borrowed until image
/// release. A null name at/after the end terminates ordered enumeration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_needed(
    image: *const DarwinArtElfSelectedImage,
    index: usize,
    name: *mut *const c_char,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if name.is_null() {
            return Err(FfiFailure::Invalid("dependency output is null"));
        }
        unsafe { *name = ptr::null() };
        let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("image is null"))?;
        unsafe {
            *name = image
                .needed
                .get(index)
                .map_or(ptr::null(), |name| name.as_ptr())
        };
        Ok(())
    })
}

/// # Safety
/// Input remains live through call; output is independently owned on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_clone(
    image: *const DarwinArtElfSelectedImage,
    output: *mut *mut DarwinArtElfSelectedImage,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    let Some(image) = (unsafe { image.as_ref() }) else {
        return unsafe { darwin_art_elf_select_image(ptr::null(), ptr::null(), output, error) };
    };
    unsafe { darwin_art_elf_select_image(&image.graph, image.soname.as_ptr(), output, error) }
}

/// # Safety
/// Image remains live through use of returned borrowed source. Writable outputs
/// must not alias. Provides actual mapped flags, never guessed load mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_source(
    image: *const DarwinArtElfSelectedImage,
    source: *mut DarwinArtElfGlobalSource,
    flags_1: *mut u64,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if source.is_null() || flags_1.is_null() {
            return Err(FfiFailure::Invalid("null image outputs"));
        }
        let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("image is null"))?;
        let flags = select_graph_image(
            &image.graph,
            image.soname.to_str().expect("validated SONAME"),
        )?
        .dynamic_flags_1();
        unsafe {
            *source = DarwinArtElfGlobalSource {
                graph: &image.graph,
                soname: image.soname.as_ptr(),
            };
            *flags_1 = flags;
        }
        Ok(())
    })
}

fn with_lookup_inputs(
    image: *const DarwinArtElfSelectedImage,
    symbol: *const c_char,
    version: *const c_char,
    address: *mut usize,
    operation: impl FnOnce(
        &DarwinArtElfSelectedImage,
        &[u8],
        Option<&str>,
        &mut usize,
    ) -> Result<(), FfiFailure>,
) -> Result<(), FfiFailure> {
    if address.is_null() {
        return Err(FfiFailure::Invalid("symbol address output is null"));
    }
    // Reset the output before validating the borrowed inputs, preserving the
    // strict API's failure behavior for every lookup frontend.
    let address = unsafe { &mut *address };
    *address = 0;
    let image = unsafe { image.as_ref() }.ok_or(FfiFailure::Invalid("image is null"))?;
    if symbol.is_null() {
        return Err(FfiFailure::Invalid("symbol is null"));
    }
    let symbol = unsafe { CStr::from_ptr(symbol) }.to_bytes();
    let version = if version.is_null() {
        None
    } else {
        Some(unsafe { required_utf8(version, "version is null") }?)
    };
    operation(image, symbol, version.as_deref(), address)
}

fn selected_image_lookup(
    image: *const DarwinArtElfSelectedImage,
    symbol: *const c_char,
    version: *const c_char,
    address: *mut usize,
    android: bool,
) -> Result<(), FfiFailure> {
    with_lookup_inputs(
        image,
        symbol,
        version,
        address,
        |image, symbol, version, address| {
            let selected = select_graph_image(
                &image.graph,
                image.soname.to_str().expect("validated SONAME"),
            )?;
            let result = if android {
                selected.lookup_android_exported(symbol, version)
            } else {
                selected.lookup_exported(symbol, version)
            }
            .map_err(FfiFailure::Namespace)?;
            *address = result.unwrap_or(0);
            Ok(())
        },
    )
}

/// # Safety
/// Image and NUL-terminated strings remain live through this call. Address is
/// writable. This applies Android's dynamic-linker export matching policy;
/// success with address zero means absent, not permission to search unrelated
/// images.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_lookup_android(
    image: *const DarwinArtElfSelectedImage,
    symbol: *const c_char,
    version: *const c_char,
    address: *mut usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        selected_image_lookup(image, symbol, version, address, true)
    })
}

/// # Safety
/// Image and NUL-terminated strings remain live through this call. Address is
/// writable. A null version requests the default export. Success with address
/// zero means absent, not permission to search unrelated images.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_lookup(
    image: *const DarwinArtElfSelectedImage,
    symbol: *const c_char,
    version: *const c_char,
    address: *mut usize,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        selected_image_lookup(image, symbol, version, address, false)
    })
}

/// # Safety
/// Transfer owned image once, after borrowed sources/users have drained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_selected_image_release(
    image: *mut DarwinArtElfSelectedImage,
) {
    if !image.is_null() {
        drop(unsafe { Box::from_raw(image) });
    }
}
