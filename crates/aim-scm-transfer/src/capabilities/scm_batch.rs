//! Atomic capability preparation for one authenticated native SCM envelope.
//! Native aliases and the EOF guardian remain owned by ScmTransferLeaseOwner.

use super::delegations::Delegation;
use super::{
    Binding, CapabilityError, CapabilityRegistry, DelegationId, DelegationRecord, DelegationState,
    HolderGrant, MAX_DELEGATIONS, RANDOM_RETRIES,
};
use crate::{MAX_PAYLOADS, ProcessEpoch, TransferKey};

impl CapabilityRegistry {
    /// Called only by the daemon after authenticating the caller and copying
    /// its native payload aliases. All validation/allocation precedes mutation;
    /// a failed batch never leaves a partial delegation behind.
    pub fn prepare_scm_batch(
        &mut self,
        source: ProcessEpoch,
        key: TransferKey,
        payloads: &[(u64, HolderGrant)],
    ) -> Result<Vec<DelegationId>, CapabilityError> {
        self.ensure_live()?;
        super::valid_process(source)?;
        if self.is_dead(source) {
            return Err(CapabilityError::SourceDead);
        }
        if key.authority != self.authority || key.ticket == 0 {
            return Err(CapabilityError::WrongBinding);
        }
        if payloads.len() > MAX_PAYLOADS {
            return Err(CapabilityError::QuotaExceeded);
        }
        if self
            .delegations
            .len()
            .checked_add(payloads.len())
            .filter(|count| *count <= MAX_DELEGATIONS)
            .is_none()
        {
            return Err(CapabilityError::QuotaExceeded);
        }
        for (index, (ordinal, holder)) in payloads.iter().enumerate() {
            self.authenticate(*holder)?;
            if holder.process() != source {
                return Err(CapabilityError::WrongHolder);
            }
            if *ordinal >= MAX_PAYLOADS as u64
                || payloads[..index]
                    .iter()
                    .any(|(earlier, _)| earlier == ordinal)
            {
                return Err(CapabilityError::WrongBinding);
            }
            if self.delegations.values().any(|entry| {
                entry.record.binding
                    == (Binding::Scm {
                        transfer_key: key,
                        ordinal: *ordinal,
                    })
            }) {
                return Err(CapabilityError::WrongBinding);
            }
        }
        self.delegations
            .try_reserve(payloads.len())
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        let mut identifiers = Vec::new();
        identifiers
            .try_reserve(payloads.len())
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        for _ in payloads {
            let mut fresh = None;
            for _ in 0..RANDOM_RETRIES {
                let candidate = DelegationId(self.fresh_id(false)?);
                if !identifiers.contains(&candidate) {
                    fresh = Some(candidate);
                    break;
                }
            }
            identifiers.push(fresh.ok_or(CapabilityError::IdentifierExhausted)?);
        }
        for ((ordinal, holder), id) in payloads.iter().zip(&identifiers) {
            self.delegations.insert(
                id.get(),
                Delegation {
                    record: DelegationRecord {
                        id: *id,
                        endpoint: holder.endpoint(),
                        source,
                        binding: Binding::Scm {
                            transfer_key: key,
                            ordinal: *ordinal,
                        },
                        state: DelegationState::Committed { receiver: None },
                    },
                },
            );
        }
        Ok(identifiers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthorityEpoch;

    fn fixture() -> (CapabilityRegistry, ProcessEpoch, TransferKey) {
        let authority = AuthorityEpoch { instance: 301 };
        (
            CapabilityRegistry::new(authority).unwrap(),
            ProcessEpoch {
                pid: 7,
                instance: 91,
            },
            TransferKey {
                authority,
                ticket: 1,
            },
        )
    }

    #[test]
    fn invalid_later_payload_does_not_commit_earlier_payload() {
        let (mut registry, source, key) = fixture();
        let local = registry.register_pair(source).unwrap();
        let foreign = registry
            .register_pair(ProcessEpoch {
                pid: 8,
                instance: 92,
            })
            .unwrap();
        assert_eq!(
            registry.prepare_scm_batch(source, key, &[(0, local.holder_a), (1, foreign.holder_a)]),
            Err(CapabilityError::WrongHolder)
        );
        assert_eq!(registry.delegation_count(), 0);
        assert_eq!(
            registry.prepare_scm_batch(source, key, &[(0, local.holder_a), (0, local.holder_b)]),
            Err(CapabilityError::WrongBinding)
        );
        assert_eq!(registry.delegation_count(), 0);
    }

    #[test]
    fn repeated_description_keeps_each_ordinal_and_committed_source_death() {
        let (mut registry, source, key) = fixture();
        let local = registry.register_pair(source).unwrap();
        let ids = registry
            .prepare_scm_batch(source, key, &[(0, local.holder_a), (1, local.holder_a)])
            .unwrap();
        assert_ne!(ids[0], ids[1]);
        assert_eq!(
            registry.prepare_scm_batch(source, key, &[(0, local.holder_a)]),
            Err(CapabilityError::WrongBinding)
        );
        registry.source_died(source).unwrap();
        for (ordinal, id) in ids.into_iter().enumerate() {
            let record = registry.delegation(id).unwrap();
            assert_eq!(record.endpoint, local.endpoint_a);
            assert_eq!(
                record.binding,
                Binding::Scm {
                    transfer_key: key,
                    ordinal: ordinal as u64
                }
            );
            assert_eq!(record.state, DelegationState::Committed { receiver: None });
        }
    }

    #[test]
    fn full_delegation_budget_rejects_the_whole_next_batch() {
        let (mut registry, source, key) = fixture();
        let local = registry.register_pair(source).unwrap();
        let payloads: Vec<_> = (0..MAX_PAYLOADS as u64)
            .map(|ordinal| (ordinal, local.holder_a))
            .collect();
        for ticket in 1..=(MAX_DELEGATIONS / MAX_PAYLOADS) as u64 {
            registry
                .prepare_scm_batch(source, TransferKey { ticket, ..key }, &payloads)
                .unwrap();
        }
        assert_eq!(registry.delegation_count(), MAX_DELEGATIONS);
        assert_eq!(
            registry.prepare_scm_batch(
                source,
                TransferKey { ticket: 99, ..key },
                &[(0, local.holder_a), (1, local.holder_b)]
            ),
            Err(CapabilityError::QuotaExceeded)
        );
        assert_eq!(registry.delegation_count(), MAX_DELEGATIONS);
        assert_eq!(registry.holder_count(), 2);
    }
}
