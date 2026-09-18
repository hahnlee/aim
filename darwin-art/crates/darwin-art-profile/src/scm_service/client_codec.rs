//! Client-side structural codec for the private, authenticated SCM transport.
//! Decoded IDs are untrusted wire values, NOT capability grants. Native callers
//! still need same-authority admission and complete FD ownership before publish.

use super::*;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PairOffer {
    pub authority: u128,
    pub carrier: u64,
    pub holder_a: u128,
    pub holder_b: u128,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PreparedOffer {
    pub key: TransferKey,
    pub payload_count: usize,
    pub managed: Vec<(u64, u128)>,
}

pub(crate) fn encode_request(request: &Request) -> Result<Vec<u8>, ProfileError> {
    // All variable-length lists are validated BEFORE encoding/casts; the fixed
    // maximum reservation means no encoding push allocates afterward.
    let mut body = body_with_capacity(MAX_BODY_SIZE)?;
    let operation = match request {
        Request::RegisterPair => Operation::RegisterPair,
        Request::Prepare {
            carrier_holder,
            payload_count,
            managed,
        } => {
            ensure_id(*carrier_holder).map_err(invalid)?;
            ensure_managed(*payload_count, managed, false).map_err(invalid)?;
            push_u128(&mut body, *carrier_holder);
            push_u16(&mut body, *payload_count as u16);
            push_u16(&mut body, managed.len() as u16);
            push_items(&mut body, managed);
            Operation::Prepare
        }
        Request::Admit {
            carrier_holder,
            key,
            payload_count,
            managed,
            publish_ordinals,
        } => {
            ensure_id(*carrier_holder).map_err(invalid)?;
            ensure_key(*key).map_err(invalid)?;
            ensure_managed(*payload_count, managed, true).map_err(invalid)?;
            ensure_publish(publish_ordinals, managed).map_err(invalid)?;
            push_u128(&mut body, *carrier_holder);
            push_key(&mut body, *key);
            push_u16(&mut body, *payload_count as u16);
            push_u16(&mut body, managed.len() as u16);
            push_items(&mut body, managed);
            push_u16(&mut body, publish_ordinals.len() as u16);
            for ordinal in publish_ordinals {
                push_u64(&mut body, *ordinal);
            }
            Operation::Admit
        }
        Request::Settle { key, disposition } => {
            ensure_key(*key).map_err(invalid)?;
            push_key(&mut body, *key);
            body.push(match disposition {
                DeliveryDisposition::Finished => 1,
                DeliveryDisposition::Aborted => 2,
            });
            Operation::Settle
        }
        Request::ReleaseHolder { holder } => {
            ensure_id(*holder).map_err(invalid)?;
            push_u128(&mut body, *holder);
            Operation::ReleaseHolder
        }
    };
    frame(operation, body)
}

fn push_items(body: &mut Vec<u8>, items: &[(u64, u128)]) {
    for &(ordinal, id) in items {
        push_u64(body, ordinal);
        push_u128(body, id);
    }
}

fn push_key(body: &mut Vec<u8>, key: TransferKey) {
    push_u128(body, key.authority.instance);
    push_u64(body, key.ticket);
}

fn response_body(bytes: &[u8], expected: Operation) -> Result<&[u8], WireError> {
    let (operation, body) = read_header(bytes)?;
    if operation != expected {
        return Err(WireError::UnknownOperation);
    }
    Ok(body)
}

pub(crate) fn decode_pair(bytes: &[u8]) -> Result<PairOffer, ProfileError> {
    (|| {
        let mut reader = Reader::new(response_body(bytes, Operation::RegisterPair)?);
        let offer = PairOffer {
            authority: reader.u128()?,
            carrier: reader.u64()?,
            holder_a: reader.u128()?,
            holder_b: reader.u128()?,
        };
        reader.finish()?;
        ensure_authority(offer.authority)?;
        ensure_id(offer.holder_a)?;
        ensure_id(offer.holder_b)?;
        if offer.carrier == 0 || offer.holder_a == offer.holder_b {
            return Err(WireError::InconsistentPair);
        }
        Ok(offer)
    })()
    .map_err(invalid)
}

pub(crate) fn decode_prepared(bytes: &[u8]) -> Result<PreparedOffer, ProfileError> {
    (|| {
        let mut reader = Reader::new(response_body(bytes, Operation::Prepare)?);
        let key = TransferKey {
            authority: darwin_art_scm_transfer::AuthorityEpoch {
                instance: reader.u128()?,
            },
            ticket: reader.u64()?,
        };
        ensure_key(key)?;
        let payload_count = reader.u16()? as usize;
        ensure_count(payload_count)?;
        let count = reader.u16()? as usize;
        let managed = read_items(&mut reader, count, payload_count, true)?;
        reader.finish()?;
        Ok(PreparedOffer {
            key,
            payload_count,
            managed,
        })
    })()
    .map_err(invalid)
}

/// Require precisely the requested publish ordinal/order, including empty
/// discard. This structural check never turns a raw holder ID into authority.
pub(crate) fn decode_admitted(
    bytes: &[u8],
    expected: &[u64],
) -> Result<Vec<(u64, u128)>, ProfileError> {
    (|| {
        let mut reader = Reader::new(response_body(bytes, Operation::Admit)?);
        let count = reader.u16()? as usize;
        if count != expected.len() || count > MAX_ITEMS {
            return Err(WireError::InvalidCount);
        }
        let mut claims = Vec::new();
        claims
            .try_reserve_exact(count)
            .map_err(|_| WireError::Allocation)?;
        for (index, &ordinal) in expected.iter().enumerate() {
            let received = reader.u64()?;
            let holder = reader.u128()?;
            ensure_id(holder)?;
            if received != ordinal || ordinal >= MAX_ITEMS as u64 {
                return Err(WireError::InvalidPublishedOrdinal);
            }
            if expected[..index].contains(&ordinal) {
                return Err(WireError::DuplicateOrdinal);
            }
            if claims.iter().any(|&(_, previous)| previous == holder) {
                return Err(WireError::DuplicateId);
            }
            claims.push((ordinal, holder));
        }
        reader.finish()?;
        Ok(claims)
    })()
    .map_err(invalid)
}

#[cfg(test)]
#[path = "client_codec_tests.rs"]
mod tests;
