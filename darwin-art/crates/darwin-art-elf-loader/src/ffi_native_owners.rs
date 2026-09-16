//! Native resource retention at the graph-load ABI; no symbol admission policy.
use super::*;

#[repr(C)]
pub struct NativeOwner {
    pub(super) context: *mut c_void,
    retain: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
}
pub(super) struct Retained {
    pub(super) pointer: NonNull<c_void>,
    pub(super) soname: Option<CString>,
    release: unsafe extern "C" fn(*mut c_void),
}
// ABI requires a thread-safe retained resource and infallible release callable
// on whichever thread drops the last graph/global image, with no unwind.
unsafe impl Send for Retained {}
unsafe impl Sync for Retained {}
impl Drop for Retained {
    fn drop(&mut self) {
        unsafe { (self.release)(self.pointer.as_ptr()) };
    }
}

pub(super) unsafe fn retain_owners(
    input: *const NativeOwner,
    count: usize,
) -> Result<Vec<Arc<dyn std::any::Any + Send + Sync>>, FfiFailure> {
    unsafe { retain_named_owners(input, count, None) }
}

pub(super) unsafe fn retain_named_owners(
    input: *const NativeOwner,
    count: usize,
    names: Option<&[CString]>,
) -> Result<Vec<Arc<dyn std::any::Any + Send + Sync>>, FfiFailure> {
    if names.is_some_and(|names| names.len() != count) {
        return Err(FfiFailure::Invalid("native owner/name count mismatch"));
    }
    if count > MAX_INPUT_SIZE / std::mem::size_of::<NativeOwner>()
        || (count != 0 && input.is_null())
    {
        return Err(FfiFailure::Invalid("invalid native owner array"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let input = unsafe { std::slice::from_raw_parts(input, count) };
    for owner in input {
        if owner.context.is_null() || owner.retain.is_none() || owner.release.is_none() {
            return Err(FfiFailure::Invalid("incomplete native owner callbacks"));
        }
    }
    let mut retained: Vec<Arc<dyn std::any::Any + Send + Sync>> = Vec::with_capacity(count);
    for (index, owner) in input.iter().enumerate() {
        let pointer = unsafe { owner.retain.unwrap()(owner.context) };
        let pointer =
            NonNull::new(pointer).ok_or(FfiFailure::Invalid("native owner retain failed"))?;
        let value: Arc<dyn std::any::Any + Send + Sync> = Arc::new(Retained {
            pointer,
            soname: names.map(|names| names[index].clone()),
            release: owner.release.unwrap(),
        });
        retained.push(if let Some(names) = names {
            Arc::new(crate::namespace::image_resource::ImageResource {
                name: names[index]
                    .to_str()
                    .map_err(|_| FfiFailure::Invalid("native owner name is not UTF-8"))?
                    .to_owned(),
                kind: crate::namespace::image_resource::ResourceKind::Provider,
                value,
            })
        } else {
            value
        });
    }
    Ok(retained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    unsafe extern "C" fn retain(p: *mut c_void) -> *mut c_void {
        p
    }
    unsafe extern "C" fn fail(_: *mut c_void) -> *mut c_void {
        ptr::null_mut()
    }
    unsafe extern "C" fn release(p: *mut c_void) {
        unsafe { &*p.cast::<AtomicUsize>() }.fetch_add(1, Ordering::SeqCst);
    }
    #[test]
    fn retention_rollback_and_last_reference_release() {
        let drops = AtomicUsize::new(0);
        let p = (&drops as *const AtomicUsize).cast_mut().cast();
        let mut owners = [
            NativeOwner {
                context: p,
                retain: Some(retain),
                release: Some(release),
            },
            NativeOwner {
                context: p,
                retain: Some(fail),
                release: Some(release),
            },
        ];
        assert!(unsafe { retain_owners(owners.as_ptr(), 2) }.is_err());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        owners[1].release = None;
        assert!(unsafe { retain_owners(owners.as_ptr(), 2) }.is_err());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        let retained = unsafe { retain_owners(owners.as_ptr(), 1) }.ok().unwrap();
        let clone = retained[0].clone();
        drop(retained);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        drop(clone);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        let mut graph = ptr::null_mut();
        // A later load-input failure must release the retained ABI resource.
        let status = unsafe {
            ffi_graph_load::darwin_art_elf_graph_load_with_owners(
                ptr::null(),
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                owners.as_ptr(),
                1,
                &mut graph,
                ptr::null_mut(),
            )
        };
        assert_eq!(status, DarwinArtElfStatus::InvalidArgument);
        assert!(graph.is_null());
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }
}
