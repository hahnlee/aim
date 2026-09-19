//! Narrow unsafe native Binder capability port. FD aliases belong to the caller;
//! this port authenticates binding/grants through the genuine daemon protocol.
use super::{release_id, status, Context};
use crate::{
    binder_capability_wire::{self as wire, Request},
    protocol,
    scm_service::client_transport,
    ProfileError,
};
use darwin_art_engine_sys::{ScmBinderBindingV2, ScmGrantV2};
use darwin_art_scm_transfer::capabilities::{decode_attributes, AttributeKind};
use std::{ffi::c_void, io::Write, os::unix::net::UnixStream, time::Duration};

fn connect(context: &Context, request: &Request) -> Result<UnixStream, ProfileError> {
    let mut stream = client_transport::connect(&context.socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    protocol::write_request(
        &mut stream,
        protocol::OP_BINDER_CAPABILITY,
        &wire::encode(request)?,
    )?;
    Ok(stream)
}
fn expect_empty(stream: &mut UnixStream) -> Result<(), ProfileError> {
    if !protocol::expect_ok(stream, protocol::OP_BINDER_CAPABILITY)?.is_empty() {
        return Err(ProfileError::Daemon(
            "Binder capability ACK contained trailing bytes".into(),
        ));
    }
    Ok(())
}
fn cancel(
    context: &Context,
    binding: darwin_art_scm_transfer::capabilities::Binding,
) -> Result<(), ProfileError> {
    expect_empty(&mut connect(context, &Request::Cancel { binding })?)
}
struct Pending<'a> {
    context: &'a Context,
    binding: darwin_art_scm_transfer::capabilities::Binding,
    armed: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = cancel(self.context, self.binding) {
                eprintln!("Binder pending Bind rollback failed: {error}");
            }
        }
    }
}
struct Claim<'a> {
    context: &'a Context,
    holder: u128,
    armed: bool,
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = release_id(self.context, self.holder) {
                eprintln!("Binder native claim rollback failed: {error}");
            }
        }
    }
}

pub(super) unsafe extern "C" fn bind_binder(
    context: *mut c_void,
    holder: *const u8,
    binding: *const ScmBinderBindingV2,
    attributes: *mut u8,
) -> i32 {
    if !attributes.is_null() {
        unsafe { std::ptr::write_bytes(attributes, 0, 40) };
    }
    if context.is_null() || holder.is_null() || binding.is_null() || attributes.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let raw_binding = unsafe { *binding };
    let mut holder_bytes = [0; 16];
    unsafe { std::ptr::copy_nonoverlapping(holder, holder_bytes.as_mut_ptr(), 16) };
    status(|| {
        let binding = wire::binding(raw_binding)?;
        let mut pending = Pending {
            context,
            binding,
            armed: true,
        };
        let mut stream = connect(
            context,
            &Request::Bind {
                binding,
                holder: u128::from_le_bytes(holder_bytes),
            },
        )?;
        let bytes = protocol::expect_ok(&mut stream, protocol::OP_BINDER_CAPABILITY)?;
        let decoded = decode_attributes(&bytes).map_err(|error| {
            ProfileError::Daemon(format!("invalid Binder Bind attributes: {error:?}"))
        })?;
        if decoded.kind != AttributeKind::Binder {
            return Err(ProfileError::Daemon(
                "Binder Bind returned wrong attribute kind".into(),
            ));
        }
        stream.write_all(&bytes)?;
        stream.flush()?;
        expect_empty(&mut stream)?;
        // The native retained-export lease owns exact pending cancellation from
        // here through deposit; successful deposit makes it a harmless no-op.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), attributes, 40) };
        pending.armed = false;
        Ok(())
    })
}
pub(super) unsafe extern "C" fn cancel_binder(
    context: *mut c_void,
    binding: *const ScmBinderBindingV2,
) -> i32 {
    if context.is_null() || binding.is_null() {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let raw = unsafe { *binding };
    status(|| cancel(context, wire::binding(raw)?))
}
pub(super) unsafe extern "C" fn claim_binder(
    context: *mut c_void,
    binding: *const ScmBinderBindingV2,
    attributes: *const u8,
    length: u32,
    output: *mut ScmGrantV2,
) -> i32 {
    if !output.is_null() {
        unsafe { *output = ScmGrantV2::default() };
    }
    if context.is_null()
        || binding.is_null()
        || attributes.is_null()
        || output.is_null()
        || length != 40
    {
        return libc::EINVAL;
    }
    let context = unsafe { &*context.cast::<Context>() };
    let raw = unsafe { *binding };
    let mut attrs = [0; 40];
    unsafe { std::ptr::copy_nonoverlapping(attributes, attrs.as_mut_ptr(), 40) };
    status(|| {
        let decoded = decode_attributes(&attrs).map_err(|error| {
            ProfileError::Daemon(format!("invalid Binder claim attributes: {error:?}"))
        })?;
        if decoded.kind != AttributeKind::Binder {
            return Err(ProfileError::Daemon(
                "wrong Binder claim attribute kind".into(),
            ));
        }
        let mut stream = connect(
            context,
            &Request::Claim {
                binding: wire::binding(raw)?,
                attributes: attrs,
            },
        )?;
        let bytes = protocol::expect_ok(&mut stream, protocol::OP_BINDER_CAPABILITY)?;
        let grant = wire::decode_claim(&bytes)?;
        let mut claim = Claim {
            context,
            holder: u128::from_le_bytes(grant.holder),
            armed: true,
        };
        if u128::from_le_bytes(grant.authority) != decoded.authority.instance {
            return Err(ProfileError::Daemon(
                "Binder claim authority changed".into(),
            ));
        }
        // This Rust receipt owns the staged grant before echo, while native
        // keeps its raw FD/unpublished object alive. Nothing is guest-visible.
        stream.write_all(&bytes)?;
        stream.flush()?;
        expect_empty(&mut stream)?;
        unsafe { *output = grant };
        claim.armed = false;
        Ok(())
    })
}
