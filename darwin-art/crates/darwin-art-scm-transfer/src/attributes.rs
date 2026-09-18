use super::types::{AttributeKind, DelegationId};
use crate::AuthorityEpoch;

pub const ATTRIBUTES_BYTES: usize = 40;
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityAttributes {
    pub kind: AttributeKind,
    pub authority: AuthorityEpoch,
    pub delegation: DelegationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeError {
    WrongLength,
    UnsupportedVersion,
    UnknownKind,
    ZeroAuthority,
    ZeroDelegation,
}

/// Encode only transport metadata. Successful decoding is never proof that a
/// sender owns a grant; the registry still checks its live delegation table.
pub fn encode(attributes: CapabilityAttributes) -> [u8; ATTRIBUTES_BYTES] {
    let mut output = [0; ATTRIBUTES_BYTES];
    output[0..4].copy_from_slice(&VERSION.to_le_bytes());
    let kind = match attributes.kind {
        AttributeKind::Binder => 1_u32,
        AttributeKind::Scm => 2_u32,
    };
    output[4..8].copy_from_slice(&kind.to_le_bytes());
    output[8..24].copy_from_slice(&attributes.authority.instance.to_le_bytes());
    output[24..40].copy_from_slice(&attributes.delegation.get().to_le_bytes());
    output
}

pub fn decode(bytes: &[u8]) -> Result<CapabilityAttributes, AttributeError> {
    if bytes.len() != ATTRIBUTES_BYTES {
        return Err(AttributeError::WrongLength);
    }
    let version = u32::from_le_bytes(bytes[0..4].try_into().expect("length checked"));
    if version != VERSION {
        return Err(AttributeError::UnsupportedVersion);
    }
    let kind = match u32::from_le_bytes(bytes[4..8].try_into().expect("length checked")) {
        1 => AttributeKind::Binder,
        2 => AttributeKind::Scm,
        _ => return Err(AttributeError::UnknownKind),
    };
    let authority = u128::from_le_bytes(bytes[8..24].try_into().expect("length checked"));
    if authority == 0 {
        return Err(AttributeError::ZeroAuthority);
    }
    let delegation = u128::from_le_bytes(bytes[24..40].try_into().expect("length checked"));
    if delegation == 0 {
        return Err(AttributeError::ZeroDelegation);
    }
    Ok(CapabilityAttributes {
        kind,
        authority: AuthorityEpoch { instance: authority },
        delegation: DelegationId(delegation),
    })
}
