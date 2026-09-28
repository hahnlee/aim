//! Borrowed dependency metadata is copied while the admission lease is live.
use super::*;

pub(super) type ResidentMetadata =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *const *const c_char, *mut usize) -> i32;

pub(super) unsafe fn copy_needed(
    callback: Option<ResidentMetadata>,
    context: *mut c_void,
    resident: *mut c_void,
) -> Result<Vec<Vec<u8>>, FfiFailure> {
    let Some(callback) = callback else {
        return Ok(Vec::new());
    };
    let mut names = ptr::null();
    let mut count = 0;
    if unsafe { callback(context, resident, &mut names, &mut count) } != 0 {
        return Err(FfiFailure::Invalid("resident dependency metadata failed"));
    }
    if count > MAX_INPUT_SIZE / std::mem::size_of::<*const c_char>()
        || (count != 0 && names.is_null())
    {
        return Err(FfiFailure::Invalid("invalid resident dependency array"));
    }
    let mut needed = Vec::new();
    let mut total = 0usize;
    for index in 0..count {
        let name = unsafe { *names.add(index) };
        if name.is_null() {
            return Err(FfiFailure::Invalid("null resident dependency"));
        }
        let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
        validate_discovery_component(bytes, "resident dependency")?;
        total = total
            .checked_add(bytes.len() + std::mem::size_of::<Vec<u8>>())
            .ok_or_else(|| FfiFailure::Bounds("resident metadata size overflow".into()))?;
        if total > MAX_DISCOVERY_TOTAL_SIZE {
            return Err(FfiFailure::Bounds(
                "resident metadata exceeds size cap".into(),
            ));
        }
        needed.push(bytes.to_vec());
    }
    Ok(needed)
}
