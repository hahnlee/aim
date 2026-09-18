//! Rust ownership behind the private native endpoint installer ABI.
//! This does NOT activate managed sockets or private ancillary framing.
use super::{client_transport, failed, wire::Request};
use crate::ProfileError;
use darwin_art_engine_sys::*;
use std::{ffi::c_void, io::Write, path::PathBuf, sync::Arc};

struct Context {
    socket: PathBuf,
}

pub struct NativeScmEndpointProvider {
    context: Arc<Context>,
}

impl NativeScmEndpointProvider {
    pub fn new(socket: PathBuf) -> Result<Self, ProfileError> {
        if !socket.is_absolute() {
            return Err(failed("provider requires trusted absolute profile socket"));
        }
        Ok(Self {
            context: Arc::new(Context { socket }),
        })
    }

    /// Raw table borrows this owner until native installation retains it.
    /// It alone cannot extend lifetime; all callbacks are unsafe private ABI.
    pub fn hooks(&self) -> ScmEndpointProviderV1 {
        ScmEndpointProviderV1 {
            abi_version: SCM_ENDPOINT_ABI_VERSION,
            struct_size: size_of::<ScmEndpointProviderV1>() as u32,
            context: Arc::as_ptr(&self.context).cast_mut().cast(),
            retain: Some(retain),
            release: Some(release),
            register_pair: Some(register_pair),
            release_holder: Some(release_holder),
        }
    }
}

// All pointers originate at the trusted runtime installer. No guest pointer,
// PID, native socket identity or path is accepted by operation callbacks.
unsafe extern "C" fn retain(context: *mut c_void) -> *mut c_void {
    if context.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { Arc::increment_strong_count(context.cast::<Context>()) };
    context
}
unsafe extern "C" fn release(context: *mut c_void) {
    if !context.is_null() {
        unsafe { Arc::decrement_strong_count(context.cast::<Context>()) };
    }
}

fn status(operation: impl FnOnce() -> Result<(), ProfileError>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(())) => 0,
        Ok(Err(ProfileError::Io(error))) => error.raw_os_error().unwrap_or(libc::EIO),
        Ok(Err(_)) => libc::EPROTO,
        Err(_) => libc::EIO,
    }
}

struct Installation<'a> {
    context: &'a Context,
    installer: ScmPairInstallerV1,
    holders: [u128; 2],
    armed: bool,
}
impl Drop for Installation<'_> {
    fn drop(&mut self) {
        if self.armed {
            // Clear partial native state first. No RPC or grant release in it.
            unsafe { (self.installer.clear.unwrap())(self.installer.target) };
            // Explicit release also covers a lost final confirmation AFTER the
            // daemon committed. Its connection-EOF rollback is insufficient.
            for holder in self.holders {
                if let Err(error) = release_id(self.context, holder) {
                    // Preserve the initiating error, but never report cleanup
                    // failure as success or hide a possibly committed grant.
                    let _ = writeln!(std::io::stderr().lock(), "SCM endpoint rollback: {error}");
                }
            }
        }
    }
}

unsafe extern "C" fn register_pair(context: *mut c_void, target: *const ScmPairInstallerV1) -> i32 {
    if context.is_null() || target.is_null() {
        return libc::EINVAL;
    }
    // Private caller guarantees aligned/readable target for this synchronous
    // operation and retained context/code for every callback until return.
    let installer = unsafe { *target };
    if installer.abi_version != SCM_ENDPOINT_ABI_VERSION
        || installer.struct_size != size_of::<ScmPairInstallerV1>() as u32
        || installer.target.is_null()
        || installer.install.is_none()
        || installer.clear.is_none()
    {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    status(|| {
        let stream = client_transport::connect(&context.socket)?;
        let mut installed = client_transport::register_pair(stream, |offer| {
            let receipt = Installation {
                context,
                installer,
                holders: [offer.holder_a, offer.holder_b],
                armed: true,
            };
            let raw = ScmPairOfferV1 {
                authority: offer.authority.to_le_bytes(),
                carrier: offer.carrier,
                holder_a: offer.holder_a.to_le_bytes(),
                holder_b: offer.holder_b.to_le_bytes(),
            };
            let result = unsafe { installer.install.unwrap()(installer.target, &raw) };
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(if result > 0 {
                    result
                } else {
                    libc::EIO
                })
                .into());
            }
            Ok(receipt)
        })?;
        // Caller-owned native receipt STILL owns rollback through publication.
        // Only daemon-grant responsibility transfers to the installed objects.
        installed.armed = false;
        Ok(())
    })
}

fn release_id(context: &Context, holder: u128) -> Result<(), ProfileError> {
    client_transport::settle_or_release(
        client_transport::connect(&context.socket)?,
        &Request::ReleaseHolder { holder },
    )
}

unsafe extern "C" fn release_holder(context: *mut c_void, holder: *const u8) -> i32 {
    if context.is_null() || holder.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let mut bytes = [0; 16];
    unsafe { std::ptr::copy_nonoverlapping(holder, bytes.as_mut_ptr(), bytes.len()) };
    status(|| release_id(context, u128::from_le_bytes(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_context_outlives_local_provider_and_release_balances() {
        let owner =
            NativeScmEndpointProvider::new(PathBuf::from("/tmp/darwin-scm-unused.sock")).unwrap();
        let weak = Arc::downgrade(&owner.context);
        let hooks = owner.hooks();
        let pinned = unsafe { hooks.retain.unwrap()(hooks.context) };
        drop(owner);
        assert!(weak.upgrade().is_some());
        assert_eq!(
            unsafe { hooks.register_pair.unwrap()(pinned, std::ptr::null()) },
            libc::EINVAL
        );
        unsafe { hooks.release.unwrap()(pinned) };
        assert!(weak.upgrade().is_none());
    }
}
