use super::types::{
    Binding, ClaimedDelivery, DelegationId, DelegationRecord, DelegationState, DeliveryDisposition,
    HolderGrant, TrustedAuthorizer,
};
use super::{CapabilityError, CapabilityRegistry, MAX_DELEGATIONS, MAX_HOLDERS, valid_process};
use crate::ProcessEpoch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Delegation {
    pub record: DelegationRecord,
}
impl Delegation {
    fn new(
        id: DelegationId,
        endpoint: crate::EndpointId,
        source: ProcessEpoch,
        binding: Binding,
    ) -> Self {
        Self {
            record: DelegationRecord {
                id,
                endpoint,
                source,
                binding,
                state: DelegationState::Pending,
            },
        }
    }
}

impl CapabilityRegistry {
    pub fn prepare_delegation(
        &mut self,
        holder: HolderGrant,
        binding: Binding,
    ) -> Result<DelegationId, CapabilityError> {
        self.ensure_live()?;
        self.authenticate(holder)?;
        if !binding.valid_for(self.authority) {
            return Err(CapabilityError::WrongBinding);
        }
        if self.delegations.len() >= MAX_DELEGATIONS {
            return Err(CapabilityError::QuotaExceeded);
        }
        self.delegations
            .try_reserve(1)
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        let id = self.fresh_id(false)?;
        self.delegations.insert(
            id,
            Delegation::new(
                DelegationId(id),
                holder.endpoint(),
                holder.process(),
                binding,
            ),
        );
        Ok(DelegationId(id))
    }

    pub fn commit_source_export(
        &mut self,
        holder: HolderGrant,
        delegation: DelegationId,
        binding: Binding,
    ) -> Result<(), CapabilityError> {
        self.ensure_live()?;
        self.authenticate(holder)?;
        let entry = self
            .delegations
            .get_mut(&delegation.0)
            .ok_or(CapabilityError::UnknownDelegation)?;
        if entry.record.source != holder.process() || entry.record.endpoint != holder.endpoint() {
            return Err(CapabilityError::WrongHolder);
        }
        if entry.record.binding != binding || !binding.valid_for(self.authority) {
            return Err(CapabilityError::WrongBinding);
        }
        if entry.record.state != DelegationState::Pending {
            return Err(CapabilityError::WrongState);
        }
        entry.record.state = DelegationState::Committed { receiver: None };
        Ok(())
    }

    pub fn authorize_delivery(
        &mut self,
        authorizer: &TrustedAuthorizer,
        delegation: DelegationId,
        binding: Binding,
        receiver: ProcessEpoch,
    ) -> Result<(), CapabilityError> {
        self.ensure_live()?;
        if authorizer.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        valid_process(receiver)?;
        if self.is_dead(receiver) {
            return Err(CapabilityError::ReceiverDead);
        }
        let entry = self
            .delegations
            .get_mut(&delegation.0)
            .ok_or(CapabilityError::UnknownDelegation)?;
        if entry.record.binding != binding {
            return Err(CapabilityError::WrongBinding);
        }
        if !matches!(
            entry.record.state,
            DelegationState::Committed { receiver: None }
        ) {
            return Err(CapabilityError::WrongState);
        }
        entry.record.state = DelegationState::Committed {
            receiver: Some(receiver),
        };
        Ok(())
    }

    pub fn claim(
        &mut self,
        receiver: ProcessEpoch,
        attributes: &[u8],
        binding: Binding,
    ) -> Result<ClaimedDelivery, CapabilityError> {
        self.ensure_live()?;
        valid_process(receiver)?;
        if self.is_dead(receiver) {
            return Err(CapabilityError::ReceiverDead);
        }
        let decoded = super::attributes::decode(attributes)?;
        if decoded.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if decoded.kind != binding.kind() || !binding.valid_for(self.authority) {
            return Err(CapabilityError::WrongBinding);
        }
        let record = self
            .delegations
            .get(&decoded.delegation.get())
            .ok_or(CapabilityError::UnknownDelegation)?
            .record;
        if record.binding != binding {
            return Err(CapabilityError::WrongBinding);
        }
        if !matches!(record.state, DelegationState::Committed { receiver: Some(expected) } if expected == receiver)
        {
            return Err(CapabilityError::WrongState);
        }
        if self.holders.len() >= MAX_HOLDERS {
            return Err(CapabilityError::QuotaExceeded);
        }
        self.holders
            .try_reserve(1)
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        let id = self.fresh_id(true)?;
        let grant = HolderGrant {
            authority: self.authority,
            id,
            process: receiver,
            endpoint: record.endpoint,
        };
        self.holders
            .insert(id, super::holders::HolderRecord { grant });
        let entry = self
            .delegations
            .get_mut(&decoded.delegation.get())
            .expect("record was checked");
        entry.record.state = DelegationState::Claimed {
            receiver,
            holder_id: id,
        };
        Ok(ClaimedDelivery {
            delegation: entry.record.id,
            grant,
            endpoint: record.endpoint,
            receiver,
        })
    }

    pub fn finish_or_abort(
        &mut self,
        delegation: DelegationId,
        grant: HolderGrant,
        disposition: DeliveryDisposition,
    ) -> Result<(), CapabilityError> {
        let record = self
            .delegations
            .get(&delegation.0)
            .ok_or(CapabilityError::UnknownDelegation)?
            .record;
        let holder_id = match record.state {
            DelegationState::Claimed { holder_id, .. } => holder_id,
            _ => return Err(CapabilityError::WrongState),
        };
        if holder_id != grant.id {
            return Err(CapabilityError::WrongHolder);
        }
        let holder = self
            .holders
            .get(&grant.id)
            .ok_or(CapabilityError::UnknownHolder)?
            .grant;
        if holder != grant || holder.endpoint() != record.endpoint {
            return Err(CapabilityError::WrongHolder);
        }
        self.delegations.remove(&delegation.0);
        if disposition == DeliveryDisposition::Aborted {
            self.holders.remove(&grant.id);
        }
        self.retire_if_empty(record.endpoint.carrier);
        Ok(())
    }
    pub fn finish(
        &mut self,
        delegation: DelegationId,
        grant: HolderGrant,
    ) -> Result<(), CapabilityError> {
        self.finish_or_abort(delegation, grant, DeliveryDisposition::Finished)
    }
    pub fn abort(
        &mut self,
        delegation: DelegationId,
        grant: HolderGrant,
    ) -> Result<(), CapabilityError> {
        self.finish_or_abort(delegation, grant, DeliveryDisposition::Aborted)
    }
    pub fn source_died(&mut self, process: ProcessEpoch) -> Result<(), CapabilityError> {
        valid_process(process)?;
        let death = self.reserve_dead(process);
        self.holders.retain(|_, holder| holder.process() != process);
        self.delegations.retain(|_, delegation| {
            !(delegation.record.source == process
                && delegation.record.state == DelegationState::Pending)
        });
        self.carriers.retain(|carrier, _| {
            self.holders
                .values()
                .any(|h| h.endpoint().carrier == *carrier)
                || self
                    .delegations
                    .values()
                    .any(|d| d.record.endpoint.carrier == *carrier)
        });
        death
    }
    pub fn receiver_died(&mut self, process: ProcessEpoch) -> Result<(), CapabilityError> {
        valid_process(process)?;
        let death = self.reserve_dead(process);
        self.holders.retain(|_, holder| holder.process() != process);
        self.delegations.retain(|_, delegation| {
            !matches!(delegation.record.state,
                DelegationState::Committed { receiver: Some(receiver) }
                    | DelegationState::Claimed { receiver, .. }
                if receiver == process)
        });
        self.carriers.retain(|carrier, _| {
            self.holders
                .values()
                .any(|h| h.endpoint().carrier == *carrier)
                || self
                    .delegations
                    .values()
                    .any(|d| d.record.endpoint.carrier == *carrier)
        });
        death
    }
}
