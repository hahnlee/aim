//! Private SDK transport. No carrier is marked managed here: the native owner
//! must install attributes and cover every consuming alias before adoption.
//! Each operation consumes one dedicated connection, closing it on EVERY exit;
//! a failed receipt cannot leave the daemon waiting on a caller-held stream.
use super::{
    failed,
    wire::{Request, client},
};
use crate::{ProfileError, fd_passing, protocol};
use std::{
    io::Write,
    os::{
        fd::{BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

pub(crate) struct PreparedDelivery {
    pub offer: client::PreparedOffer,
    pub metadata: OwnedFd,
    pub guardian: OwnedFd,
}

pub(crate) fn connect(socket: &std::path::Path) -> Result<UnixStream, ProfileError> {
    Ok(crate::unix_connect::connect_with_timeout(
        socket,
        Duration::from_secs(5),
    )?)
}

/// Use an already-owned trusted profile connection. FD creation/CLOEXEC and
/// endpoint selection belong to its process transport owner, not this codec.
fn request(stream: &mut UnixStream, request: &Request) -> Result<(), ProfileError> {
    let bytes = client::encode_request(request)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    protocol::write_request(stream, protocol::OP_SCM_SERVICE, &bytes)?;
    Ok(())
}

/// Install BOTH unpublished native descriptions before the genuine receipt.
/// T must own rollback of those installations until this function returns it;
/// an echo/confirmation failure drops T rather than exposing a partial pair.
pub(crate) fn register_pair<T>(
    mut stream: UnixStream,
    install: impl FnOnce(&client::PairOffer) -> Result<T, ProfileError>,
) -> Result<T, ProfileError> {
    request(&mut stream, &Request::RegisterPair)?;
    let bytes = protocol::expect_ok(&mut stream, protocol::OP_SCM_SERVICE)?;
    let offer = client::decode_pair(&bytes)?;
    let installed = install(&offer)?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    expect_empty(&mut stream)?;
    Ok(installed)
}

/// On success metadata/guardian remain owned, not published as guest rights.
/// Closing the returned guardian is not a guessed receipt: the native caller
/// must carry it through actual enqueue, intake and authoritative admission.
pub(crate) fn prepare(
    mut stream: UnixStream,
    carrier_holder: u128,
    payloads: &[BorrowedFd<'_>],
    managed: &[(u64, u128)],
) -> Result<PreparedDelivery, ProfileError> {
    let mut items = Vec::new();
    // Bound before allocation; the server remains the authorization owner.
    if payloads.is_empty() || payloads.len() > 16 || managed.len() > payloads.len() {
        return Err(failed("invalid client payload manifest"));
    }
    items.try_reserve_exact(managed.len()).map_err(failed)?;
    items.extend_from_slice(managed);
    request(
        &mut stream,
        &Request::Prepare {
            carrier_holder,
            payload_count: payloads.len(),
            managed: items,
        },
    )?;
    fd_passing::send_many(&stream, payloads)?;
    let bytes = protocol::expect_ok(&mut stream, protocol::OP_SCM_SERVICE)?;
    // Acquire the complete native group under the existing shared inheritance
    // boundary, then keep RAII ownership through all response validation.
    let mut descriptors =
        fd_passing::receive_many_until(&stream, Instant::now() + Duration::from_secs(5))?;
    let offer = client::decode_prepared(&bytes)?;
    if descriptors.len() != 2
        || offer.payload_count != payloads.len()
        || offer.managed.len() != managed.len()
        || !offer
            .managed
            .iter()
            .zip(managed)
            .all(|(received, sent)| received.0 == sent.0)
    {
        return Err(failed("prepared response/native group manifest mismatch"));
    }
    let guardian = descriptors.pop().unwrap();
    let metadata = descriptors.pop().unwrap();
    Ok(PreparedDelivery {
        offer,
        metadata,
        guardian,
    })
}

/// Caller supplies an Admit request bound to its received envelope and carrier.
/// Returned raw holder IDs are NOT native Description installation/publication.
pub(crate) fn admit(
    mut stream: UnixStream,
    admission: &Request,
) -> Result<Vec<(u64, u128)>, ProfileError> {
    let Request::Admit {
        publish_ordinals, ..
    } = admission
    else {
        return Err(failed("client admission requires Admit operation"));
    };
    request(&mut stream, admission)?;
    let bytes = protocol::expect_ok(&mut stream, protocol::OP_SCM_SERVICE)?;
    client::decode_admitted(&bytes, publish_ordinals)
}

pub(crate) fn settle_or_release(
    mut stream: UnixStream,
    operation: &Request,
) -> Result<(), ProfileError> {
    if !matches!(
        operation,
        Request::Settle { .. } | Request::ReleaseHolder { .. }
    ) {
        return Err(failed(
            "client cleanup requires settlement or holder release",
        ));
    }
    request(&mut stream, operation)?;
    expect_empty(&mut stream)
}

fn expect_empty(stream: &mut UnixStream) -> Result<(), ProfileError> {
    if !protocol::expect_ok(stream, protocol::OP_SCM_SERVICE)?.is_empty() {
        return Err(failed("unexpected client confirmation bytes"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "client_transport_tests.rs"]
mod tests;
