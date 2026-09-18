//! Binder descriptor transport, not provider capability policy.
//!
//! Bindings identify the authenticated transport bundle. Opaque attributes
//! must still be admitted by the descriptor provider before guest publication.

use darwin_art_binder_device::descriptor_manifest::{
    DescriptorAttributes, DescriptorBundle, MAX_ATTRIBUTES_BYTES,
};
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

pub use darwin_art_engine_sys::{DescriptorTransferBinding, RetainedExportedDescriptor};
const _: () =
    assert!(MAX_ATTRIBUTES_BYTES == darwin_art_engine_sys::BINDER_DESCRIPTOR_ATTRIBUTES_BYTES);

#[repr(C)]
pub struct ExportedDescriptor {
    pub host_fd: i32,
    pub attributes_length: u32,
    pub attributes: [u8; MAX_ATTRIBUTES_BYTES],
}

impl Default for ExportedDescriptor {
    fn default() -> Self {
        Self {
            host_fd: -1,
            attributes_length: 0,
            attributes: [0; MAX_ATTRIBUTES_BYTES],
        }
    }
}

pub use crate::descriptor_retention::{DescriptorRetainedApi, RetainedDescriptorLease};

/// V2 provider port. Success exports a newly owned host FD and bounded
/// attributes atomically. On failure the provider closes all partial exports
/// and leaves no owned descriptor in the output. Import consumes its host FD
/// on both success and failure, just like the existing bare-FD import port.
#[derive(Clone, Copy)]
pub struct DescriptorBundleApi {
    pub export:
        unsafe extern "C" fn(i32, *const DescriptorTransferBinding, *mut ExportedDescriptor) -> i32,
    pub import:
        unsafe extern "C" fn(i32, *const DescriptorTransferBinding, *const u8, usize) -> i32,
}

#[derive(Clone, Copy)]
pub struct DescriptorApi {
    pub export: unsafe extern "C" fn(i32) -> i32,
    pub import: unsafe extern "C" fn(i32) -> i32,
    pub close: unsafe extern "C" fn(i32) -> i32,
    pub bundle: Option<DescriptorBundleApi>,
    pub retained: Option<DescriptorRetainedApi>,
}

impl DescriptorApi {
    pub(crate) fn export_bound(
        &self,
        guest: i32,
        binding: DescriptorTransferBinding,
    ) -> Result<DescriptorBundle, String> {
        let mut result = ExportedDescriptor::default();
        if let Some(api) = self.bundle {
            // SAFETY: the synchronous provider port borrows initialized stack
            // storage and must obey its owned-descriptor result contract.
            if unsafe { (api.export)(guest, &binding, &mut result) } != 0 {
                return Err("descriptor provider export failed".into());
            }
        } else {
            // Legacy exports have no capability attributes. A managed provider
            // must refuse this port, rather than exporting an untagged alias.
            result.host_fd = unsafe { (self.export)(guest) };
        }
        if result.host_fd < 0 {
            return Err("descriptor provider returned no host FD".into());
        }
        // SAFETY: either successful provider port transfers one fresh owned FD.
        // Acquire it BEFORE validating the remaining output so errors close it.
        let descriptor = unsafe { OwnedFd::from_raw_fd(result.host_fd) };
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
        Ok(DescriptorBundle::new(
            binding.object_offset as usize,
            binding.ordinal,
            descriptor,
            attributes,
        ))
    }

    pub(crate) fn import_bound(
        &self,
        bundle: DescriptorBundle,
        binding: DescriptorTransferBinding,
    ) -> Result<i32, String> {
        let (offset, ordinal, descriptor, attributes) = bundle.into_parts();
        if ordinal != binding.ordinal || usize::try_from(binding.object_offset).ok() != Some(offset)
        {
            return Err("descriptor offset or ordinal does not match its binding".into());
        }
        if let Some(api) = self.bundle {
            let descriptor = descriptor.into_raw_fd();
            let bytes = attributes.as_bytes();
            let pointer = if bytes.is_empty() {
                std::ptr::null()
            } else {
                bytes.as_ptr()
            };
            // SAFETY: the provider consumes descriptor on both success and
            // failure; binding and bytes remain live through the call.
            let result = unsafe { (api.import)(descriptor, &binding, pointer, bytes.len()) };
            if result < 0 {
                return Err("descriptor provider import failed".into());
            }
            return Ok(result);
        }
        if !attributes.as_bytes().is_empty() {
            return Err("descriptor attributes require the V2 provider port".into());
        }
        let descriptor = descriptor.into_raw_fd();
        // SAFETY: the legacy provider consumes the raw host descriptor.
        let result = unsafe { (self.import)(descriptor) };
        if result < 0 {
            return Err("descriptor provider import failed".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "descriptor_transport_tests.rs"]
mod tests;
