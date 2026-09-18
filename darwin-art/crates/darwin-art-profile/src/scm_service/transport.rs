//! Private request/response I/O. No socket wait runs under the ownership mutex.
use super::{
    failed,
    owner::Owner,
    wire::{self, Request},
};
use crate::{ProfileError, fd_passing, protocol};
use darwin_art_scm_transfer::{ProcessEpoch, capabilities::RegisteredPair};
use std::{
    io::{Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    sync::Mutex,
    time::{Duration, Instant},
};

struct PairRollback<'a> {
    owner: &'a Mutex<Owner>,
    peer: ProcessEpoch,
    pair: RegisteredPair,
    committed: bool,
}
impl Drop for PairRollback<'_> {
    fn drop(&mut self) {
        if !self.committed {
            match self.owner.lock() {
                Ok(mut owner) => {
                    // Death may already have removed these exact grants.
                    let _ = owner.release_holder(self.peer, self.pair.holder_a.id());
                    let _ = owner.release_holder(self.peer, self.pair.holder_b.id());
                }
                Err(_) => eprintln!("SCM service: pair rollback owner poisoned"),
            }
        }
    }
}

pub(super) fn serve(
    owner: &Mutex<Owner>,
    peer: ProcessEpoch,
    stream: &mut UnixStream,
    bytes: &[u8],
) -> Result<(), ProfileError> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut responded = false;
    let result = (|| {
        match wire::decode_request(bytes)? {
            Request::RegisterPair => {
                let pair = owner
                    .lock()
                    .map_err(|_| failed("owner poisoned"))?
                    .register_pair(peer)?;
                let mut rollback = PairRollback {
                    owner,
                    peer,
                    pair,
                    committed: false,
                };
                let response = wire::encode_pair(pair)?;
                reply(stream, &mut responded, &response)?;
                // Native caller must install the returned attributes before
                // echoing this exact fresh response. It waits for confirmation
                // before exposing its pair. Lost response/echo revokes grants.
                let mut installed = Vec::new();
                installed
                    .try_reserve_exact(response.len())
                    .map_err(failed)?;
                installed.resize(response.len(), 0);
                stream.read_exact(&mut installed)?;
                if installed != response {
                    return Err(failed("pair installation receipt mismatch"));
                }
                protocol::write_response(stream, protocol::OP_SCM_SERVICE, 0, b"")?;
                rollback.committed = true;
            }
            Request::Prepare {
                carrier_holder,
                payload_count,
                managed,
            } => {
                let payloads = fd_passing::receive_many_until(
                    stream,
                    Instant::now() + Duration::from_secs(5),
                )?;
                if payloads.len() != payload_count {
                    return Err(failed("full native payload group mismatch"));
                }
                let prepared = owner
                    .lock()
                    .map_err(|_| failed("owner poisoned"))?
                    .prepare(peer, carrier_holder, &payloads, &managed)?;
                reply(stream, &mut responded, &prepared.response)?;
                fd_passing::send_many(
                    stream,
                    &[
                        prepared.metadata.as_fd(),
                        prepared.transfer.guardian_writer(),
                    ],
                )?;
                // Closing local writers on any return does NOT remove aliases;
                // they survive this handler and retire only by actual EOF.
            }
            Request::Admit {
                carrier_holder,
                key,
                payload_count,
                managed,
                publish_ordinals,
            } => {
                let claims = owner.lock().map_err(|_| failed("owner poisoned"))?.admit(
                    peer,
                    carrier_holder,
                    key,
                    payload_count,
                    &managed,
                    &publish_ordinals,
                )?;
                reply(stream, &mut responded, &wire::encode_admitted(&claims)?)?;
            }
            Request::Settle { key, disposition } => {
                owner.lock().map_err(|_| failed("owner poisoned"))?.settle(
                    peer,
                    key,
                    disposition,
                )?;
                reply(stream, &mut responded, b"")?;
            }
            Request::ReleaseHolder { holder } => {
                owner
                    .lock()
                    .map_err(|_| failed("owner poisoned"))?
                    .release_holder(peer, holder)?;
                reply(stream, &mut responded, b"")?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !responded {
            protocol::write_response(
                stream,
                protocol::OP_SCM_SERVICE,
                22,
                error.to_string().as_bytes(),
            )?;
        }
        return Err(error);
    }
    stream.flush()?;
    Ok(())
}

fn reply(stream: &mut UnixStream, responded: &mut bool, bytes: &[u8]) -> Result<(), ProfileError> {
    // Once a response prefix may have escaped, failures close the connection;
    // never append another response or retransmit positive ancillary sends.
    *responded = true;
    protocol::write_response(stream, protocol::OP_SCM_SERVICE, 0, bytes)?;
    Ok(())
}
