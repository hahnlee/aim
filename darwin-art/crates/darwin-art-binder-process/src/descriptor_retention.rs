//! Rust-local source retention through genuine Binder deposit acknowledgement.
use crate::descriptor_transport::DescriptorApi;
use darwin_art_binder_device::descriptor_manifest::{
    DescriptorAttributes, DescriptorBundle, MAX_ATTRIBUTES_BYTES,
};
use darwin_art_engine_sys::{DescriptorTransferBinding, RetainedExportedDescriptor};
use std::{
    ffi::c_void,
    os::fd::{FromRawFd, OwnedFd},
    ptr::NonNull,
};

/// Retained provider output. This is a separate versioned port: the V2
/// `ExportedDescriptor` layout above intentionally remains unchanged.
///
/// On success the provider transfers both `host_fd` and the opaque `lease` to
/// the caller. A failing provider call must return no owned output.
/// Separate retained-export provider ABI. Do not fold these fields into V2:
/// old providers are intentionally unmanaged until this port is installed.
#[derive(Clone, Copy)]
pub struct DescriptorRetainedApi {
    pub export: unsafe extern "C" fn(
        i32,
        *const DescriptorTransferBinding,
        *mut RetainedExportedDescriptor,
    ) -> i32,
    pub release: unsafe extern "C" fn(*mut c_void),
}

/// Rust-local owner for one provider export lease. It is deliberately not
/// serialized in `TransferImage`; it remains live around the actual transport
/// acknowledgement and is released exactly once by `Drop`.
pub struct RetainedDescriptorLease {
    lease: NonNull<c_void>,
    release: unsafe extern "C" fn(*mut c_void),
}

impl RetainedDescriptorLease {
    fn from_output(
        output: &RetainedExportedDescriptor,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Option<Self> {
        NonNull::new(output.lease).map(|lease| Self { lease, release })
    }
}

impl Drop for RetainedDescriptorLease {
    fn drop(&mut self) {
        // SAFETY: the provider supplied this callback and opaque lease as one
        // pair; this owner invokes it once when its local lifetime ends.
        unsafe { (self.release)(self.lease.as_ptr()) }
    }
}

impl DescriptorApi {
    pub(crate) fn export_bound_retained(
        &self,
        guest: i32,
        binding: DescriptorTransferBinding,
    ) -> Result<(DescriptorBundle, RetainedDescriptorLease), String> {
        let api = self
            .retained
            .ok_or_else(|| "retained descriptor provider port is not installed".to_owned())?;
        let mut result = RetainedExportedDescriptor::default();
        // SAFETY: the synchronous provider port borrows initialized stack
        // storage and must obey its owned output contract.
        if unsafe { (api.export)(guest, &binding, &mut result) } != 0 {
            return Err("retained descriptor provider export failed".into());
        }
        // Acquire both owned resources before validating output completeness or
        // metadata. Malformed success output therefore retires every resource
        // it did provide instead of leaking a partial export.
        let descriptor =
            (result.host_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(result.host_fd) });
        let lease = RetainedDescriptorLease::from_output(&result, api.release);
        if descriptor.is_none() || lease.is_none() {
            return Err("retained descriptor provider returned incomplete output".into());
        }
        let descriptor = descriptor.expect("retained descriptor output checked above");
        let lease = lease.expect("retained descriptor lease checked above");
        let length = result.attributes_length as usize;
        if length > MAX_ATTRIBUTES_BYTES {
            return Err("descriptor provider attributes exceed the ABI bound".into());
        }
        let mut attributes = Vec::new();
        attributes
            .try_reserve_exact(length)
            .map_err(|_| "descriptor attribute allocation failed".to_owned())?;
        attributes.extend_from_slice(&result.attributes[..length]);
        let attributes = DescriptorAttributes::new(attributes)
            .map_err(|_| "invalid descriptor attributes".to_owned())?;
        Ok((
            DescriptorBundle::new(
                binding.object_offset as usize,
                binding.ordinal,
                descriptor,
                attributes,
            ),
            lease,
        ))
    }
}
