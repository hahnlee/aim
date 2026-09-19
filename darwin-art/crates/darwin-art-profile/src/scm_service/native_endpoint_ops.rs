//! Native SCM endpoint operations over the authenticated profile service.
//!
//! This module owns bounded request validation and client transport only. It
//! never installs guest descriptors or treats a returned holder as a guest FD.

use super::super::{client_transport, wire::Request};
use super::native_endpoint_metadata;
use super::super::credentials::Credentials;
use crate::ProfileError;
use darwin_art_engine_sys::SCM_MAX_PAYLOADS;
use darwin_art_scm_transfer::{capabilities::DeliveryDisposition, TransferKey};
use std::{
    collections::HashSet,
    os::fd::{BorrowedFd, OwnedFd, RawFd},
    path::Path,
};

pub(crate) struct PreparedNative {
    pub authority: [u8; 16],
    pub ticket: u64,
    pub metadata: OwnedFd,
    pub guardian: OwnedFd,
    pub payload_count: u32,
    pub credentials: Credentials,
}

pub(crate) struct NativeClaim {
    pub ordinal: u64,
    pub authority: [u8; 16],
    pub carrier: u64,
    pub holder: [u8; 16],
    pub side: u32,
}

pub(crate) struct AdmissionNative {
    pub authority: [u8; 16],
    pub ticket: u64,
    pub claims: Vec<NativeClaim>,
    pub credentials: Credentials,
}

pub(crate) fn prepare(
    socket: &Path,
    carrier_holder: u128,
    payload_fds: &[RawFd],
    managed: &[(u64, u128)],
) -> Result<PreparedNative, ProfileError> {
    if carrier_holder == 0
        || payload_fds.len() > SCM_MAX_PAYLOADS
        || managed.len() > payload_fds.len()
        || managed.len() > SCM_MAX_PAYLOADS
    {
        return Err(invalid("invalid prepare count or carrier holder"));
    }

    let mut seen_fds = HashSet::new();
    seen_fds
        .try_reserve(payload_fds.len())
        .map_err(|_| invalid("native fd validation allocation failed"))?;
    let mut borrowed = Vec::new();
    borrowed
        .try_reserve_exact(payload_fds.len())
        .map_err(|_| invalid("native fd borrow allocation failed"))?;
    for &fd in payload_fds {
        if fd < 0 || !seen_fds.insert(fd) || !valid_fd(fd) {
            return Err(invalid("invalid or duplicate native payload fd"));
        }
        // The caller retains every descriptor for the synchronous deposit.
        borrowed.push(unsafe { BorrowedFd::borrow_raw(fd) });
    }

    let managed = validate_manifest(managed, payload_fds.len())?;
    let stream = client_transport::connect(socket)?;
    let prepared = client_transport::prepare(stream, carrier_holder, &borrowed, &managed)?;
    if !prepared.offer.authenticated {
        return Err(invalid("prepared response has no authenticated credentials"));
    }
    Ok(PreparedNative {
        authority: prepared.offer.key.authority.instance.to_le_bytes(),
        ticket: prepared.offer.key.ticket,
        metadata: prepared.metadata,
        guardian: prepared.guardian,
        payload_count: prepared.offer.payload_count as u32,
        credentials: prepared.offer.credentials,
    })
}

pub(crate) fn admit(
    socket: &Path,
    carrier_holder: u128,
    metadata_fd: RawFd,
    payload_count: usize,
    publish_ordinals: &[u64],
) -> Result<AdmissionNative, ProfileError> {
    if carrier_holder == 0
        || payload_count > SCM_MAX_PAYLOADS
        || publish_ordinals.len() > SCM_MAX_PAYLOADS
    {
        return Err(invalid("invalid admission count or carrier holder"));
    }
    let bytes = native_endpoint_metadata::read(metadata_fd)?;
    let prepared = native_endpoint_metadata::decode_prepared(&bytes)?;
    if prepared.payload_count != payload_count {
        return Err(invalid(
            "received native payload count differs from metadata",
        ));
    }

    let mut seen = HashSet::new();
    seen.try_reserve(publish_ordinals.len())
        .map_err(|_| invalid("publication ordinal allocation failed"))?;
    let mut filtered = Vec::new();
    filtered
        .try_reserve(publish_ordinals.len())
        .map_err(|_| invalid("publication filter allocation failed"))?;
    for &ordinal in publish_ordinals {
        if ordinal >= payload_count as u64 || !seen.insert(ordinal) {
            return Err(invalid("invalid or duplicate publication ordinal"));
        }
        if prepared
            .managed
            .iter()
            .any(|&(managed_ordinal, _)| managed_ordinal == ordinal)
        {
            filtered.push(ordinal);
        }
    }

    let key = prepared.key;
    let request = Request::Admit {
        carrier_holder,
        key,
        payload_count,
        managed: prepared.managed,
        publish_ordinals: filtered.clone(),
    };
    // Reserve the complete native claim storage before the daemon RPC. Once
    // Admit commits its receiver/claims, every local validation failure must
    // send the exact metadata key an Aborted receipt; no unvalidated holder
    // may be released individually.
    let mut claims = Vec::new();
    claims
        .try_reserve_exact(filtered.len())
        .map_err(|_| invalid("admission claim allocation failed"))?;
    let admitted = match client_transport::admit_authenticated(client_transport::connect(socket)?, &request) {
        Ok(admitted) => admitted,
        Err(error) => return abort_admission(socket, key, error),
    };
    if admitted.credentials != prepared.credentials {
        return abort_admission(socket, key, invalid("admission credentials changed"));
    }
    let grants = admitted.claims;
    if grants.len() != filtered.len() {
        return abort_admission(
            socket,
            key,
            invalid("admission grant count differs from requested claims"),
        );
    }
    for (grant, &ordinal) in grants.iter().zip(&filtered) {
        if grant.ordinal != ordinal
            || grant.authority != key.authority.instance
            || grant.carrier == 0
            || grant.side > 1
            || grant.holder == 0
        {
            return abort_admission(socket, key, invalid("admission grant is not authoritative"));
        }
        claims.push(NativeClaim {
            ordinal,
            authority: grant.authority.to_le_bytes(),
            carrier: grant.carrier,
            holder: grant.holder.to_le_bytes(),
            side: grant.side,
        });
    }
    Ok(AdmissionNative {
        authority: key.authority.instance.to_le_bytes(),
        ticket: key.ticket,
        claims,
        credentials: prepared.credentials,
    })
}

fn abort_admission(
    socket: &Path,
    key: TransferKey,
    error: ProfileError,
) -> Result<AdmissionNative, ProfileError> {
    let _ = settle(socket, key.authority.instance.to_le_bytes(), key.ticket, 2);
    Err(error)
}

pub(crate) fn settle(
    socket: &Path,
    authority: [u8; 16],
    ticket: u64,
    disposition: u32,
) -> Result<(), ProfileError> {
    if u128::from_le_bytes(authority) == 0 || ticket == 0 {
        return Err(invalid("invalid settlement key"));
    }
    let disposition = match disposition {
        1 => DeliveryDisposition::Finished,
        2 => DeliveryDisposition::Aborted,
        _ => return Err(invalid("invalid settlement disposition")),
    };
    let request = Request::Settle {
        key: TransferKey {
            authority: darwin_art_scm_transfer::AuthorityEpoch {
                instance: u128::from_le_bytes(authority),
            },
            ticket,
        },
        disposition,
    };
    client_transport::settle_or_release(client_transport::connect(socket)?, &request)
}

fn validate_manifest(
    managed: &[(u64, u128)],
    payload_count: usize,
) -> Result<Vec<(u64, u128)>, ProfileError> {
    let mut ordinals = HashSet::new();
    ordinals
        .try_reserve(managed.len())
        .map_err(|_| invalid("manifest ordinal allocation failed"))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(managed.len())
        .map_err(|_| invalid("manifest allocation failed"))?;
    for &(ordinal, holder) in managed {
        if ordinal >= payload_count as u64 || holder == 0 || !ordinals.insert(ordinal) {
            return Err(invalid("invalid or duplicate managed manifest entry"));
        }
        result.push((ordinal, holder));
    }
    Ok(result)
}

fn valid_fd(fd: RawFd) -> bool {
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

fn invalid(message: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("SCM native endpoint: {message}"))
}
