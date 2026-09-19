//! Capability-bounded authority and endpoint lifetime for managed transfers.
//!
//! This module does not inspect FDs, sockets, Binder wire data, or magic
//! markers. A trusted daemon adapter supplies authenticated process epochs and
//! invokes the separate authorizer API after proving a transfer.

mod attributes;
mod binder_batch;
pub use binder_batch::BinderManifestItem;
mod delegations;
mod holders;
mod scm_batch;
mod scm_claim;
mod scm_cleanup;
pub use scm_claim::ClaimReservation;
mod types;

pub use attributes::{ATTRIBUTES_BYTES, AttributeError, CapabilityAttributes};
pub use attributes::{decode as decode_attributes, encode as encode_attributes};
pub use types::{
    AttributeKind, Binding, ClaimedDelivery, DelegationId, DelegationRecord, DelegationState,
    DeliveryDisposition, HolderGrant, RegisteredPair, TrustedAuthorizer,
};

#[cfg(test)]
mod claim_tests;
#[cfg(test)]
mod tests;

use crate::{AuthorityEpoch, CarrierId, ProcessEpoch};
use std::collections::{HashMap, HashSet};
use std::fmt;

pub const MAX_HOLDERS: usize = 1_024;
pub const MAX_CARRIERS: usize = 256;
pub const MAX_DELEGATIONS: usize = 256;
const MAX_DEAD_PROCESSES: usize = 256;
const RANDOM_RETRIES: usize = 8;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CapabilityError {
    InvalidAuthority,
    InvalidProcess,
    UnknownCarrier,
    UnknownHolder,
    UnknownDelegation,
    WrongHolder,
    WrongBinding,
    WrongAuthority,
    WrongKind,
    WrongState,
    ReceiverDead,
    SourceDead,
    QuotaExceeded,
    IdentifierExhausted,
    RandomUnavailable,
    Attribute(AttributeError),
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidAuthority => "invalid authority epoch",
            Self::InvalidProcess => "invalid process epoch",
            Self::UnknownCarrier => "unknown carrier",
            Self::UnknownHolder => "unknown holder grant",
            Self::UnknownDelegation => "unknown delegation",
            Self::WrongHolder => "holder is not bound to this process or endpoint",
            Self::WrongBinding => "delegation binding does not match",
            Self::WrongAuthority => "attribute authority does not match",
            Self::WrongKind => "attribute kind does not match",
            Self::WrongState => "delegation is not in the required state",
            Self::ReceiverDead => "receiver process epoch is dead",
            Self::SourceDead => "source process epoch is dead",
            Self::QuotaExceeded => "capability registry quota exceeded",
            Self::IdentifierExhausted => "fresh identifier could not be allocated",
            Self::RandomUnavailable => "Darwin getentropy failed",
            Self::Attribute(error) => return write!(f, "invalid capability attributes: {error:?}"),
        };
        f.write_str(message)
    }
}

impl std::error::Error for CapabilityError {}
impl From<AttributeError> for CapabilityError {
    fn from(value: AttributeError) -> Self {
        Self::Attribute(value)
    }
}

/// Pure authority state, independent of ScmTransferLeaseOwner's payload
/// aliases and guardian lifetime.
#[derive(Debug)]
pub struct CapabilityRegistry {
    authority: AuthorityEpoch,
    carriers: HashMap<CarrierId, ()>,
    next_carrier_serial: u64,
    holders: HashMap<u128, holders::HolderRecord>,
    delegations: HashMap<u128, delegations::Delegation>,
    dead_processes: HashSet<ProcessEpoch>,
    sealed: bool,
}

impl CapabilityRegistry {
    pub fn new(authority: AuthorityEpoch) -> Result<Self, CapabilityError> {
        if authority.instance == 0 {
            return Err(CapabilityError::InvalidAuthority);
        }
        Ok(Self {
            authority,
            carriers: HashMap::new(),
            next_carrier_serial: 0,
            holders: HashMap::new(),
            delegations: HashMap::new(),
            dead_processes: HashSet::new(),
            sealed: false,
        })
    }

    pub fn authority(&self) -> AuthorityEpoch {
        self.authority
    }
    pub fn trusted_authorizer(&self) -> TrustedAuthorizer {
        TrustedAuthorizer {
            authority: self.authority,
        }
    }
    pub fn holder_count(&self) -> usize {
        self.holders.len()
    }
    pub fn carrier_count(&self) -> usize {
        self.carriers.len()
    }
    pub fn delegation_count(&self) -> usize {
        self.delegations.len()
    }

    pub fn delegation(&self, id: DelegationId) -> Result<DelegationRecord, CapabilityError> {
        self.delegations
            .get(&id.0)
            .map(|v| v.record)
            .ok_or(CapabilityError::UnknownDelegation)
    }

    pub(crate) fn next_carrier_serial(&self) -> Result<u64, CapabilityError> {
        self.next_carrier_serial
            .checked_add(1)
            .ok_or(CapabilityError::IdentifierExhausted)
    }

    pub(crate) fn commit_carrier_serial(&mut self, serial: u64) {
        self.next_carrier_serial = serial;
    }

    pub(crate) fn fresh_id(&self, holders: bool) -> Result<u128, CapabilityError> {
        for _ in 0..RANDOM_RETRIES {
            let id = random_u128()?;
            let exists = if holders {
                self.holders.contains_key(&id)
            } else {
                self.delegations.contains_key(&id)
            };
            if id != 0 && !exists {
                return Ok(id);
            }
        }
        Err(CapabilityError::IdentifierExhausted)
    }

    pub(crate) fn fresh_id_excluding(
        &self,
        holders: bool,
        excluded: u128,
    ) -> Result<u128, CapabilityError> {
        for _ in 0..RANDOM_RETRIES {
            let id = random_u128()?;
            let exists = if holders {
                self.holders.contains_key(&id)
            } else {
                self.delegations.contains_key(&id)
            };
            if id != 0 && id != excluded && !exists {
                return Ok(id);
            }
        }
        Err(CapabilityError::IdentifierExhausted)
    }

    pub(crate) fn reserve_dead(&mut self, process: ProcessEpoch) -> Result<(), CapabilityError> {
        if self.sealed {
            return Err(CapabilityError::QuotaExceeded);
        }
        if self.dead_processes.contains(&process) {
            return Ok(());
        }
        if self.dead_processes.len() >= MAX_DEAD_PROCESSES {
            self.sealed = true;
            return Err(CapabilityError::QuotaExceeded);
        }
        if self.dead_processes.try_reserve(1).is_err() {
            self.sealed = true;
            return Err(CapabilityError::QuotaExceeded);
        }
        self.dead_processes.insert(process);
        Ok(())
    }

    pub(crate) fn is_dead(&self, process: ProcessEpoch) -> bool {
        self.dead_processes.contains(&process)
    }
    pub(crate) fn ensure_live(&self) -> Result<(), CapabilityError> {
        if self.sealed {
            Err(CapabilityError::QuotaExceeded)
        } else {
            Ok(())
        }
    }
    pub(crate) fn retire_if_empty(&mut self, carrier: CarrierId) {
        let holder = self
            .holders
            .values()
            .any(|h| h.endpoint().carrier == carrier);
        let work = self
            .delegations
            .values()
            .any(|d| d.record.endpoint.carrier == carrier);
        if !holder && !work {
            self.carriers.remove(&carrier);
        }
    }

    pub(crate) fn carrier_known(&self, carrier: CarrierId) -> bool {
        carrier.authority == self.authority
            && carrier.serial != 0
            && self.carriers.contains_key(&carrier)
    }
}

pub(crate) fn valid_process(process: ProcessEpoch) -> Result<(), CapabilityError> {
    (process.pid != 0 && process.instance != 0)
        .then_some(())
        .ok_or(CapabilityError::InvalidProcess)
}

pub(crate) fn random_u128() -> Result<u128, CapabilityError> {
    let mut bytes = [0_u8; 16];
    // SAFETY: bytes is valid writable storage and receives exactly 16 bytes.
    let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    if result != 0 {
        return Err(CapabilityError::RandomUnavailable);
    }
    Ok(u128::from_ne_bytes(bytes))
}
