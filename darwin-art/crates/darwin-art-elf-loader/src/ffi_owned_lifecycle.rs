//! Mapping-retained lifecycle context, independent of a launcher/graph handle.
use super::*;
pub struct DarwinArtElfLifecycleOwner(pub(super) Arc<dyn DsoLifecycle>);
pub(super) struct ContextOwner {
    pointer: NonNull<c_void>,
    release: unsafe extern "C" fn(*mut c_void),
}
impl Drop for ContextOwner {
    fn drop(&mut self) {
        unsafe { (self.release)(self.pointer.as_ptr()) };
    }
}
pub(super) struct CallbackDsoLifecycle {
    pub(super) publish: DarwinArtElfPublishImageCallback,
    pub(super) finalize: DarwinArtElfFinalizeImageCallback,
    pub(super) context: usize,
    pub(super) _owner: Option<ContextOwner>,
}
// The native ABI requires callbacks/context/release to be thread-safe, with no
// unwind, through the final mapping release (not merely original graph release).
unsafe impl Send for CallbackDsoLifecycle {}
unsafe impl Sync for CallbackDsoLifecycle {}
impl DsoLifecycle for CallbackDsoLifecycle {
    fn publish_image(&self, range: std::ops::Range<usize>) -> Result<(), String> {
        let status = unsafe { (self.publish)(self.context as *mut c_void, range.start, range.end) };
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "image lifecycle publish callback failed with status {status}"
            ))
        }
    }
    fn finalize_image(&self, range: std::ops::Range<usize>) -> Result<(), String> {
        let status =
            unsafe { (self.finalize)(self.context as *mut c_void, range.start, range.end) };
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "image lifecycle finalize callback failed with status {status}"
            ))
        }
    }
}

/// # Safety
/// Borrow readable callbacks/context during creation. retain returns an owned
/// thread-safe context used for BOTH callbacks; release consumes it once after
/// all mapped images drop. Callbacks must not unwind. Output is non-aliasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_lifecycle_owner_create(
    callbacks: *const DarwinArtElfLifecycleCallbacks,
    retain: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
    output: *mut *mut DarwinArtElfLifecycleOwner,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("null lifecycle owner output"));
        }
        unsafe { *output = ptr::null_mut() };
        let callbacks =
            unsafe { callbacks.as_ref() }.ok_or(FfiFailure::Invalid("null lifecycle callbacks"))?;
        if callbacks.abi_version != ABI_VERSION || callbacks.context.is_null() {
            return Err(FfiFailure::Invalid("invalid lifecycle owner ABI/context"));
        }
        let publish = callbacks
            .publish_image
            .ok_or(FfiFailure::Invalid("null lifecycle publish"))?;
        let finalize = callbacks
            .finalize_image
            .ok_or(FfiFailure::Invalid("null lifecycle finalize"))?;
        let retain = retain.ok_or(FfiFailure::Invalid("null lifecycle retain"))?;
        let release = release.ok_or(FfiFailure::Invalid("null lifecycle release"))?;
        let pointer = NonNull::new(unsafe { retain(callbacks.context) })
            .ok_or(FfiFailure::Invalid("lifecycle retain failed"))?;
        let owner = CallbackDsoLifecycle {
            publish,
            finalize,
            context: pointer.as_ptr() as usize,
            _owner: Some(ContextOwner { pointer, release }),
        };
        unsafe { *output = Box::into_raw(Box::new(DarwinArtElfLifecycleOwner(Arc::new(owner)))) };
        Ok(())
    })
}

/// # Safety
/// Destroy one live owner handle once. Mapping clones keep their context alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_lifecycle_owner_destroy(
    owner: *mut DarwinArtElfLifecycleOwner,
) {
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner) });
    }
}
