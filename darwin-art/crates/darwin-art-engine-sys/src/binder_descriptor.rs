//! Host-local retained Binder descriptor ABI, never serialized in TransferImage.
use core::ffi::c_void;

pub const BINDER_DESCRIPTOR_ATTRIBUTES_BYTES: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorTransferBinding {
    pub source_connection: u64,
    pub transfer: u64,
    pub ordinal: u64,
    pub object_offset: u64,
}

impl DescriptorTransferBinding {
    pub fn new(source_connection: u64, transfer: u64, ordinal: u64, object_offset: usize) -> Self {
        Self {
            source_connection,
            transfer,
            ordinal,
            object_offset: object_offset as u64,
        }
    }
}

#[repr(C)]
pub struct RetainedExportedDescriptor {
    pub host_fd: i32,
    pub attributes_length: u32,
    pub attributes: [u8; BINDER_DESCRIPTOR_ATTRIBUTES_BYTES],
    pub lease: *mut c_void,
}

impl Default for RetainedExportedDescriptor {
    fn default() -> Self {
        Self {
            host_fd: -1,
            attributes_length: 0,
            attributes: [0; BINDER_DESCRIPTOR_ATTRIBUTES_BYTES],
            lease: core::ptr::null_mut(),
        }
    }
}

pub type BinderRetainedExportFn = unsafe extern "C" fn(
    i32,
    *const DescriptorTransferBinding,
    *mut RetainedExportedDescriptor,
) -> i32;
pub type BinderExportLeaseReleaseFn = unsafe extern "C" fn(*mut c_void);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_arm64_layout_matches_native_port() {
        assert_eq!(size_of::<DescriptorTransferBinding>(), 32);
        assert_eq!(
            core::mem::offset_of!(RetainedExportedDescriptor, lease),
            264
        );
        assert_eq!(size_of::<RetainedExportedDescriptor>(), 272);
    }
}
