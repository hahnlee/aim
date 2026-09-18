//! Private fixed-size delivery framing owned by the host transport.

use core::fmt;

pub const WIRE_VERSION: u32 = 1;
pub const WIRE_SIZE: usize = 40;
pub const MAX_WIRE_COUNT: u32 = 253;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum WireKind {
    Offer = 1,
    Acquired = 2,
    Admitted = 3,
}

impl WireKind {
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Offer),
            2 => Some(Self::Acquired),
            3 => Some(Self::Admitted),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryWire {
    pub kind: WireKind,
    pub authority_epoch: u128,
    pub ticket: u64,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    WrongSize,
    UnknownKind,
    WrongVersion,
    ReservedBits,
    ZeroEpoch,
    ZeroTicket,
    InvalidCount,
    UnexpectedKind,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid host-fd delivery wire frame: {self:?}")
    }
}
impl std::error::Error for WireError {}

impl DeliveryWire {
    pub fn new(
        kind: WireKind,
        authority_epoch: u128,
        ticket: u64,
        count: u32,
    ) -> Result<Self, WireError> {
        validate_fields(authority_epoch, ticket, count)?;
        Ok(Self {
            kind,
            authority_epoch,
            ticket,
            count,
        })
    }

    pub fn encode(self) -> [u8; WIRE_SIZE] {
        let mut bytes = [0u8; WIRE_SIZE];
        bytes[0..4].copy_from_slice(&(self.kind as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes[8..24].copy_from_slice(&self.authority_epoch.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.ticket.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.count.to_le_bytes());
        bytes
    }

    pub fn decode_expected(bytes: &[u8], expected: WireKind) -> Result<Self, WireError> {
        if bytes.len() != WIRE_SIZE {
            return Err(WireError::WrongSize);
        }
        let kind = WireKind::from_raw(u32::from_le_bytes(bytes[0..4].try_into().unwrap()))
            .ok_or(WireError::UnknownKind)?;
        if kind != expected {
            return Err(WireError::UnexpectedKind);
        }
        if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != WIRE_VERSION {
            return Err(WireError::WrongVersion);
        }
        if bytes[36..40] != [0; 4] {
            return Err(WireError::ReservedBits);
        }
        let authority_epoch = u128::from_le_bytes(bytes[8..24].try_into().unwrap());
        let ticket = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let count = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        validate_fields(authority_epoch, ticket, count)?;
        Ok(Self {
            kind,
            authority_epoch,
            ticket,
            count,
        })
    }
}

fn validate_fields(authority_epoch: u128, ticket: u64, count: u32) -> Result<(), WireError> {
    if authority_epoch == 0 {
        return Err(WireError::ZeroEpoch);
    }
    if ticket == 0 {
        return Err(WireError::ZeroTicket);
    }
    if !(1..=MAX_WIRE_COUNT).contains(&count) {
        return Err(WireError::InvalidCount);
    }
    Ok(())
}
