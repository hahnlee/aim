//! Strict private wire framing for the authenticated SCM service.
//!
//! This module deliberately carries only opaque holder/delegation identifiers.
//! Process credentials and birth/incarnation values are authenticated by the
//! service owner before it calls this codec; they must never be supplied by a
//! wire peer.  Native descriptor groups are transported beside these bytes.

#[path = "client_codec.rs"]
pub(super) mod client;

use crate::ProfileError;
use darwin_art_scm_transfer::{
    TransferKey,
    capabilities::{ClaimedDelivery, DeliveryDisposition, RegisteredPair},
};

const HEADER_SIZE: usize = 8;
const MAX_BODY_SIZE: usize = 1024;
const MAX_FRAME_SIZE: usize = HEADER_SIZE + MAX_BODY_SIZE;
const MAX_ITEMS: usize = 16;

/// Version one private SCM operation tags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Operation {
    RegisterPair = 1,
    Prepare = 2,
    Admit = 3,
    Settle = 4,
    ReleaseHolder = 5,
}

impl Operation {
    fn from_raw(raw: u8) -> Result<Self, WireError> {
        match raw {
            1 => Ok(Self::RegisterPair),
            2 => Ok(Self::Prepare),
            3 => Ok(Self::Admit),
            4 => Ok(Self::Settle),
            5 => Ok(Self::ReleaseHolder),
            _ => Err(WireError::UnknownOperation),
        }
    }
}

/// A request after framing and structural validation.  Holder and delegation
/// values remain raw here: the authenticated owner resolves them against its
/// current ledger and constructs the non-forgeable provider values itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Request {
    RegisterPair,
    Prepare {
        carrier_holder: u128,
        payload_count: usize,
        managed: Vec<(u64, u128)>,
    },
    Admit {
        carrier_holder: u128,
        key: TransferKey,
        payload_count: usize,
        managed: Vec<(u64, u128)>,
        publish_ordinals: Vec<u64>,
    },
    Settle {
        key: TransferKey,
        disposition: DeliveryDisposition,
    },
    ReleaseHolder {
        holder: u128,
    },
}

fn decode_disposition(raw: u8) -> Result<DeliveryDisposition, WireError> {
    match raw {
        1 => Ok(DeliveryDisposition::Finished),
        2 => Ok(DeliveryDisposition::Aborted),
        _ => Err(WireError::UnknownSettlement),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WireError {
    Truncated,
    WrongVersion,
    ReservedBits,
    UnknownOperation,
    WrongLength,
    TrailingBytes,
    ZeroId,
    ZeroAuthority,
    ZeroTicket,
    InvalidCount,
    TooManyItems,
    DuplicateOrdinal,
    DuplicateId,
    UnknownSettlement,
    InvalidPublishedOrdinal,
    InconsistentPair,
    Allocation,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Truncated => "truncated frame",
            Self::WrongVersion => "unsupported wire version",
            Self::ReservedBits => "reserved header bits are non-zero",
            Self::UnknownOperation => "unknown operation",
            Self::WrongLength => "frame length does not match its header",
            Self::TrailingBytes => "trailing operation bytes",
            Self::ZeroId => "identifier must be non-zero",
            Self::ZeroAuthority => "authority instance must be non-zero",
            Self::ZeroTicket => "transfer ticket must be non-zero",
            Self::InvalidCount => "payload count must be in 1..=16",
            Self::TooManyItems => "item count exceeds the private wire bound",
            Self::DuplicateOrdinal => "duplicate managed or published ordinal",
            Self::DuplicateId => "duplicate delegation, holder, or published ID",
            Self::UnknownSettlement => "unknown settlement outcome",
            Self::InvalidPublishedOrdinal => "published ordinal is not managed",
            Self::InconsistentPair => "registered pair has inconsistent or duplicate fields",
            Self::Allocation => "wire allocation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WireError {}

fn invalid(error: WireError) -> ProfileError {
    ProfileError::Daemon(format!("invalid SCM service wire: {error}"))
}

fn allocation() -> ProfileError {
    invalid(WireError::Allocation)
}

fn ensure_authority(authority: u128) -> Result<(), WireError> {
    if authority == 0 {
        Err(WireError::ZeroAuthority)
    } else {
        Ok(())
    }
}

fn ensure_key(key: TransferKey) -> Result<(), WireError> {
    ensure_authority(key.authority.instance)?;
    if key.ticket == 0 {
        return Err(WireError::ZeroTicket);
    }
    Ok(())
}

fn ensure_count(count: usize) -> Result<(), WireError> {
    if (1..=MAX_ITEMS).contains(&count) {
        Ok(())
    } else {
        Err(WireError::InvalidCount)
    }
}

fn ensure_id(id: u128) -> Result<(), WireError> {
    if id == 0 {
        Err(WireError::ZeroId)
    } else {
        Ok(())
    }
}

fn ensure_managed(
    payload_count: usize,
    managed: &[(u64, u128)],
    require_distinct_ids: bool,
) -> Result<(), WireError> {
    ensure_count(payload_count)?;
    if managed.len() > MAX_ITEMS || managed.len() > payload_count {
        return Err(WireError::TooManyItems);
    }
    for (index, &(ordinal, id)) in managed.iter().enumerate() {
        if ordinal >= payload_count as u64 {
            return Err(WireError::InvalidPublishedOrdinal);
        }
        ensure_id(id)?;
        if managed[..index]
            .iter()
            .any(|(previous, _)| *previous == ordinal)
        {
            return Err(WireError::DuplicateOrdinal);
        }
        if require_distinct_ids && managed[..index].iter().any(|(_, previous)| *previous == id) {
            return Err(WireError::DuplicateId);
        }
    }
    Ok(())
}

fn ensure_publish(publish: &[u64], managed: &[(u64, u128)]) -> Result<(), WireError> {
    if publish.len() > MAX_ITEMS || publish.len() > managed.len() {
        return Err(WireError::TooManyItems);
    }
    for (index, ordinal) in publish.iter().copied().enumerate() {
        if publish[..index].contains(&ordinal) {
            return Err(WireError::DuplicateOrdinal);
        }
        if !managed.iter().any(|(candidate, _)| *candidate == ordinal) {
            return Err(WireError::InvalidPublishedOrdinal);
        }
    }
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(WireError::Truncated)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(WireError::Truncated)?;
        self.position = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn u128(&mut self) -> Result<u128, WireError> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().unwrap()))
    }

    fn finish(self) -> Result<(), WireError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(WireError::TrailingBytes)
        }
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u128(bytes: &mut Vec<u8>, value: u128) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn frame(operation: Operation, body: Vec<u8>) -> Result<Vec<u8>, ProfileError> {
    if body.len() > MAX_BODY_SIZE {
        return Err(invalid(WireError::TooManyItems));
    }
    let total = HEADER_SIZE.checked_add(body.len()).ok_or_else(allocation)?;
    let mut bytes = Vec::new();
    bytes.try_reserve(total).map_err(|_| allocation())?;
    bytes.extend_from_slice(&[1, operation as u8, 0, 0]);
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    Ok(bytes)
}

fn body_with_capacity(capacity: usize) -> Result<Vec<u8>, ProfileError> {
    if capacity > MAX_BODY_SIZE {
        return Err(invalid(WireError::TooManyItems));
    }
    let mut body = Vec::new();
    body.try_reserve(capacity).map_err(|_| allocation())?;
    Ok(body)
}

fn read_header(bytes: &[u8]) -> Result<(Operation, &[u8]), WireError> {
    if bytes.len() < HEADER_SIZE || bytes.len() > MAX_FRAME_SIZE {
        return Err(WireError::Truncated);
    }
    if bytes[0] != 1 {
        return Err(WireError::WrongVersion);
    }
    if bytes[2..4] != [0, 0] {
        return Err(WireError::ReservedBits);
    }
    let operation = Operation::from_raw(bytes[1])?;
    let body_length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if body_length > MAX_BODY_SIZE {
        return Err(WireError::WrongLength);
    }
    let expected = HEADER_SIZE
        .checked_add(body_length)
        .ok_or(WireError::WrongLength)?;
    if expected != bytes.len() {
        return Err(WireError::WrongLength);
    }
    Ok((operation, &bytes[HEADER_SIZE..]))
}

fn read_items(
    reader: &mut Reader<'_>,
    count: usize,
    payload_count: usize,
    require_distinct_ids: bool,
) -> Result<Vec<(u64, u128)>, WireError> {
    if count > MAX_ITEMS || count > payload_count {
        return Err(WireError::TooManyItems);
    }
    let mut items = Vec::new();
    items
        .try_reserve(count)
        .map_err(|_| WireError::Allocation)?;
    for _ in 0..count {
        items.push((reader.u64()?, reader.u128()?));
    }
    ensure_managed(payload_count, &items, require_distinct_ids)?;
    Ok(items)
}

/// Decode one complete request frame.  The length in the fixed header must
/// consume the entire input; this function never accepts concatenated frames.
pub(crate) fn decode_request(bytes: &[u8]) -> Result<Request, ProfileError> {
    decode_request_inner(bytes).map_err(invalid)
}

fn decode_request_inner(bytes: &[u8]) -> Result<Request, WireError> {
    let (operation, body) = read_header(bytes)?;
    let mut reader = Reader::new(body);
    let request = match operation {
        Operation::RegisterPair => {
            if !body.is_empty() {
                return Err(WireError::WrongLength);
            }
            Request::RegisterPair
        }
        Operation::Prepare => {
            let carrier_holder = reader.u128()?;
            let payload_count = reader.u16()? as usize;
            let managed_count = reader.u16()? as usize;
            ensure_id(carrier_holder)?;
            let managed = read_items(&mut reader, managed_count, payload_count, false)?;
            Request::Prepare {
                carrier_holder,
                payload_count,
                managed,
            }
        }
        Operation::Admit => {
            let carrier_holder = reader.u128()?;
            let authority = reader.u128()?;
            let ticket = reader.u64()?;
            let payload_count = reader.u16()? as usize;
            let managed_count = reader.u16()? as usize;
            ensure_id(carrier_holder)?;
            let key = TransferKey {
                authority: darwin_art_scm_transfer::AuthorityEpoch {
                    instance: authority,
                },
                ticket,
            };
            ensure_key(key)?;
            let managed = read_items(&mut reader, managed_count, payload_count, true)?;
            let publish_count = reader.u16()? as usize;
            if publish_count > MAX_ITEMS {
                return Err(WireError::TooManyItems);
            }
            let mut publish_ordinals = Vec::new();
            publish_ordinals
                .try_reserve(publish_count)
                .map_err(|_| WireError::Allocation)?;
            for _ in 0..publish_count {
                publish_ordinals.push(reader.u64()?);
            }
            ensure_publish(&publish_ordinals, &managed)?;
            Request::Admit {
                carrier_holder,
                key,
                payload_count,
                managed,
                publish_ordinals,
            }
        }
        Operation::Settle => {
            let authority = reader.u128()?;
            let ticket = reader.u64()?;
            let key = TransferKey {
                authority: darwin_art_scm_transfer::AuthorityEpoch {
                    instance: authority,
                },
                ticket,
            };
            ensure_key(key)?;
            let disposition = decode_disposition(reader.u8()?)?;
            Request::Settle { key, disposition }
        }
        Operation::ReleaseHolder => {
            let holder = reader.u128()?;
            ensure_id(holder)?;
            Request::ReleaseHolder { holder }
        }
    };
    reader.finish()?;
    Ok(request)
}

/// Encode the authoritative pair response.  Endpoint process identities are
/// intentionally absent; callers obtain those only from authenticated state.
pub(crate) fn encode_pair(pair: RegisteredPair) -> Result<Vec<u8>, ProfileError> {
    let authority_a = pair.endpoint_a.carrier.authority.instance;
    let authority_b = pair.endpoint_b.carrier.authority.instance;
    if authority_a == 0
        || authority_a != authority_b
        || pair.endpoint_a.carrier.serial == 0
        || pair.endpoint_a.carrier.serial != pair.endpoint_b.carrier.serial
        || pair.endpoint_a.side == pair.endpoint_b.side
        || pair.holder_a.endpoint() != pair.endpoint_a
        || pair.holder_b.endpoint() != pair.endpoint_b
    {
        return Err(invalid(WireError::InconsistentPair));
    }
    let holder_a = pair.holder_a.id();
    let holder_b = pair.holder_b.id();
    ensure_id(holder_a).map_err(invalid)?;
    ensure_id(holder_b).map_err(invalid)?;
    if holder_a == holder_b {
        return Err(invalid(WireError::InconsistentPair));
    }
    let mut body = body_with_capacity(16 + 8 + 16 + 16)?;
    push_u128(&mut body, authority_a);
    push_u64(&mut body, pair.endpoint_a.carrier.serial);
    push_u128(&mut body, holder_a);
    push_u128(&mut body, holder_b);
    frame(Operation::RegisterPair, body)
}

/// Encode the metadata/response frame for a prepared native FD group.
pub(crate) fn encode_prepared(
    key: TransferKey,
    payload_count: usize,
    managed: &[(u64, u128)],
) -> Result<Vec<u8>, ProfileError> {
    ensure_key(key).map_err(invalid)?;
    let payload_count =
        u16::try_from(payload_count).map_err(|_| invalid(WireError::InvalidCount))?;
    ensure_managed(payload_count as usize, managed, true).map_err(invalid)?;
    let mut body = body_with_capacity(16 + 8 + 2 + 2 + managed.len() * 24)?;
    push_u128(&mut body, key.authority.instance);
    push_u64(&mut body, key.ticket);
    push_u16(&mut body, payload_count);
    push_u16(&mut body, managed.len() as u16);
    for &(ordinal, delegation) in managed {
        push_u64(&mut body, ordinal);
        push_u128(&mut body, delegation);
    }
    frame(Operation::Prepare, body)
}

/// Encode published claims, preserving the exact ordinal mapping returned by
/// the daemon ledger.  The native guardian/FD group is carried separately.
pub(crate) fn encode_admitted(claims: &[(u64, ClaimedDelivery)]) -> Result<Vec<u8>, ProfileError> {
    if claims.len() > MAX_ITEMS {
        return Err(invalid(WireError::TooManyItems));
    }
    let mut body = body_with_capacity(2 + claims.len() * 24)?;
    push_u16(&mut body, claims.len() as u16);
    for (index, &(ordinal, claim)) in claims.iter().enumerate() {
        if ordinal >= MAX_ITEMS as u64 {
            return Err(invalid(WireError::InvalidPublishedOrdinal));
        }
        if claims[..index]
            .iter()
            .any(|(previous, _)| *previous == ordinal)
        {
            return Err(invalid(WireError::DuplicateOrdinal));
        }
        let holder = claim.grant.id();
        ensure_id(holder).map_err(invalid)?;
        if claims[..index]
            .iter()
            .any(|(_, previous)| previous.grant.id() == holder)
        {
            return Err(invalid(WireError::DuplicateId));
        }
        push_u64(&mut body, ordinal);
        push_u128(&mut body, holder);
    }
    frame(Operation::Admit, body)
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
