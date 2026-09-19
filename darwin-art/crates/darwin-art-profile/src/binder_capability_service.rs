//! Binder capability transport. The authenticated Binder owner performs policy;
//! this module owns bounded request/receipt I/O outside its locks.
use crate::{
    ProfileError,
    binder_capability_wire::{self as wire, Request},
    binder_service::BinderService,
    protocol,
};
use darwin_art_binder_device::routing_authority::PeerIdentity;
use darwin_art_scm_transfer::capabilities::{Binding, ClaimedDelivery};
use std::{io::Read, os::unix::net::UnixStream, time::Duration};

struct ClaimRollback<'a> {
    binder: &'a BinderService,
    claim: ClaimedDelivery,
    committed: bool,
}
impl Drop for ClaimRollback<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = self.binder.abort_endpoint_claim(self.claim) {
                eprintln!("Binder claim receipt rollback failed: {error}");
            }
        }
    }
}
struct BindRollback<'a> {
    binder: &'a BinderService,
    peer: PeerIdentity,
    binding: Binding,
    committed: bool,
}
impl Drop for BindRollback<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = self
                .binder
                .rollback_pending_endpoint(self.peer, self.binding)
            {
                eprintln!("Binder Bind receipt rollback failed: {error}");
            }
        }
    }
}
pub(crate) fn serve(
    binder: &BinderService,
    peer: PeerIdentity,
    stream: &mut UnixStream,
    bytes: &[u8],
) -> Result<(), ProfileError> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = match wire::decode(bytes) {
        Ok(request) => request,
        Err(error) => {
            return protocol::write_response(
                stream,
                protocol::OP_BINDER_CAPABILITY,
                22,
                error.to_string().as_bytes(),
            )
            .map_err(Into::into);
        }
    };
    let mut responded = false;
    let result = (|| {
        match request {
            Request::Bind { binding, holder } => {
                let attrs = binder.bind_endpoint(peer, binding, holder)?;
                let mut receipt = BindRollback {
                    binder,
                    peer,
                    binding,
                    committed: false,
                };
                responded = true;
                protocol::write_response(stream, protocol::OP_BINDER_CAPABILITY, 0, &attrs)?;
                let mut installed = [0; 40];
                stream.read_exact(&mut installed)?;
                if installed != attrs {
                    return Err(ProfileError::Daemon("Binder Bind receipt mismatch".into()));
                }
                protocol::write_response(stream, protocol::OP_BINDER_CAPABILITY, 0, b"")?;
                receipt.committed = true;
            }
            Request::Cancel { binding } => {
                binder.cancel_pending_endpoint(peer, binding)?;
                responded = true;
                protocol::write_response(stream, protocol::OP_BINDER_CAPABILITY, 0, b"")?;
            }
            Request::Claim {
                binding,
                attributes,
            } => {
                let claim = binder.claim_endpoint(peer, binding, &attributes)?;
                let mut receipt = ClaimRollback {
                    binder,
                    claim,
                    committed: false,
                };
                let bytes = wire::encode_claim(claim);
                responded = true;
                protocol::write_response(stream, protocol::OP_BINDER_CAPABILITY, 0, &bytes)?;
                let mut installed = [0; 41];
                stream.read_exact(&mut installed)?;
                if installed != bytes {
                    return Err(ProfileError::Daemon("Binder claim receipt mismatch".into()));
                }
                let Binding::Binder {
                    source_connection,
                    transfer,
                    ..
                } = binding
                else {
                    unreachable!()
                };
                binder.finish_endpoint_claim(claim, source_connection, transfer)?;
                protocol::write_response(stream, protocol::OP_BINDER_CAPABILITY, 0, b"")?;
                receipt.committed = true;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !responded {
            protocol::write_response(
                stream,
                protocol::OP_BINDER_CAPABILITY,
                22,
                error.to_string().as_bytes(),
            )?;
        }
        return Err(error);
    }
    Ok(())
}
