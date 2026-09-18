use super::types::{HolderGrant, RegisteredPair};
use super::{CapabilityError, CapabilityRegistry, MAX_CARRIERS, MAX_HOLDERS, valid_process};
use crate::{AuthorityEpoch, CarrierId, EndpointId, ProcessEpoch, Side};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HolderRecord {
    pub grant: HolderGrant,
}
impl HolderRecord {
    pub fn endpoint(self) -> EndpointId {
        self.grant.endpoint()
    }
    pub fn process(self) -> ProcessEpoch {
        self.grant.process()
    }
}

impl CapabilityRegistry {
    /// Both initial endpoint holders are bound to the one authenticated
    /// process epoch. Cross-process import is represented by claim().
    pub fn register_pair(
        &mut self,
        process: ProcessEpoch,
    ) -> Result<RegisteredPair, CapabilityError> {
        self.ensure_live()?;
        valid_process(process)?;
        if self.is_dead(process) {
            return Err(CapabilityError::SourceDead);
        }
        if self.carriers.len() >= MAX_CARRIERS || self.holders.len() > MAX_HOLDERS - 2 {
            return Err(CapabilityError::QuotaExceeded);
        }
        self.carriers
            .try_reserve(1)
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        self.holders
            .try_reserve(2)
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        let serial = self.next_carrier_serial()?;
        let carrier = CarrierId {
            authority: self.authority,
            serial,
        };
        let endpoint_a = EndpointId {
            carrier,
            side: Side::A,
        };
        let endpoint_b = EndpointId {
            carrier,
            side: Side::B,
        };
        let id_a = self.fresh_id(true)?;
        let id_b = self.fresh_id_excluding(true, id_a)?;
        let holder_a = HolderGrant {
            authority: self.authority,
            id: id_a,
            process,
            endpoint: endpoint_a,
        };
        let holder_b = HolderGrant {
            authority: self.authority,
            id: id_b,
            process,
            endpoint: endpoint_b,
        };
        self.commit_carrier_serial(serial);
        self.carriers.insert(carrier, ());
        self.holders.insert(id_a, HolderRecord { grant: holder_a });
        self.holders.insert(id_b, HolderRecord { grant: holder_b });
        Ok(RegisteredPair {
            endpoint_a,
            endpoint_b,
            holder_a,
            holder_b,
        })
    }

    pub fn guest_dup(&self, holder: HolderGrant) -> Result<HolderGrant, CapabilityError> {
        self.authenticate(holder)?;
        Ok(holder)
    }

    pub fn duplicate_holder(&self, holder: HolderGrant) -> Result<HolderGrant, CapabilityError> {
        self.guest_dup(holder)
    }

    /// Resolve a native opaque grant id with authenticated authority and exact
    /// process incarnation. No FD, socket, or marker is consulted.
    pub fn authenticated_holder_id(
        &self,
        process: ProcessEpoch,
        authority: AuthorityEpoch,
        opaque_id: u128,
    ) -> Result<HolderGrant, CapabilityError> {
        valid_process(process)?;
        if authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        let holder = self
            .holders
            .get(&opaque_id)
            .ok_or(CapabilityError::UnknownHolder)?
            .grant;
        if holder.authority != authority || holder.process() != process {
            return Err(CapabilityError::WrongHolder);
        }
        self.authenticate(holder)?;
        Ok(holder)
    }

    pub fn release_holder(&mut self, holder: HolderGrant) -> Result<(), CapabilityError> {
        self.authenticate(holder)?;
        self.holders
            .remove(&holder.id)
            .ok_or(CapabilityError::UnknownHolder)?;
        self.retire_if_empty(holder.endpoint().carrier);
        Ok(())
    }

    pub(crate) fn authenticate(&self, holder: HolderGrant) -> Result<(), CapabilityError> {
        if holder.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        let record = self
            .holders
            .get(&holder.id)
            .ok_or(CapabilityError::UnknownHolder)?
            .grant;
        if record != holder {
            return Err(CapabilityError::WrongHolder);
        }
        if self.is_dead(holder.process()) {
            return Err(CapabilityError::SourceDead);
        }
        if !self.carrier_known(holder.endpoint().carrier) {
            return Err(CapabilityError::UnknownCarrier);
        }
        Ok(())
    }
}
