//! Authenticated transfer ledger composition, separate from native transport.
//! The containing service holds ONE mutex across validation/reservation/commit.
use crate::ProfileError;
use super::credentials::Credentials;
use darwin_art_scm_transfer::{
    AuthorityEpoch, MAX_GLOBAL_LEASES, MAX_PAYLOADS, MAX_PROCESS_LEASES, PreparedTransfer,
    ProcessEpoch, Retirement, ScmTransferLeaseOwner, TransferKey, TrustedReceiveContext,
    TrustedSendContext,
    capabilities::{
        CapabilityRegistry, ClaimedDelivery, DelegationId, DeliveryDisposition, RegisteredPair,
    },
};
use std::{
    collections::HashMap,
    os::fd::{AsFd, OwnedFd},
};

fn failed(error: impl std::fmt::Display) -> ProfileError {
    ProfileError::Daemon(format!("SCM service: {error}"))
}

struct Ledger {
    send: TrustedSendContext,
    count: usize,
    managed: Vec<(u64, DelegationId)>,
    receiver: Option<ProcessEpoch>,
    settled: Option<DeliveryDisposition>,
    credentials: Credentials,
}

pub(super) struct Prepared {
    pub response: Vec<u8>,
    pub metadata: OwnedFd,
    pub transfer: PreparedTransfer,
    pub credentials: Credentials,
}

pub(super) struct Owner {
    leases: ScmTransferLeaseOwner,
    pub(super) registry: CapabilityRegistry,
    pub(super) binder_deliveries: HashMap<(u64, u64), super::binder_capabilities::BinderLedger>,
    ledger: HashMap<TransferKey, Ledger>,
}

impl Owner {
    pub fn new(authority: AuthorityEpoch) -> Result<Self, ProfileError> {
        Ok(Self {
            leases: ScmTransferLeaseOwner::new(authority, MAX_GLOBAL_LEASES, MAX_PROCESS_LEASES),
            registry: CapabilityRegistry::new(authority).map_err(failed)?,
            binder_deliveries: HashMap::new(),
            ledger: HashMap::new(),
        })
    }

    pub fn register_pair(&mut self, peer: ProcessEpoch) -> Result<RegisteredPair, ProfileError> {
        self.registry.register_pair(peer).map_err(failed)
    }

    pub fn prepare(
        &mut self,
        peer: ProcessEpoch,
        carrier: u128,
        payloads: &[OwnedFd],
        managed: &[(u64, u128)],
    ) -> Result<Prepared, ProfileError> {
        self.prepare_authenticated(
            peer,
            Credentials { pid: peer.pid as i32, uid: 0, gid: 0 },
            carrier,
            payloads,
            managed,
        )
    }

    pub fn prepare_authenticated(
        &mut self,
        peer: ProcessEpoch,
        credentials: Credentials,
        carrier: u128,
        payloads: &[OwnedFd],
        managed: &[(u64, u128)],
    ) -> Result<Prepared, ProfileError> {
        if !credentials.valid()
            || payloads.len() > MAX_PAYLOADS
            || managed.len() > payloads.len()
        {
            return Err(failed("invalid full native payload count"));
        }
        let carrier = self
            .registry
            .authenticated_holder_id(peer, self.registry.authority(), carrier)
            .map_err(failed)?;
        let send = TrustedSendContext {
            sender: peer,
            endpoint: carrier.endpoint(),
        };
        let mut holders = Vec::new();
        let mut manifest = Vec::new();
        let mut raw_manifest = Vec::new();
        let mut borrowed = Vec::new();
        holders.try_reserve(managed.len()).map_err(failed)?;
        manifest.try_reserve(managed.len()).map_err(failed)?;
        raw_manifest.try_reserve(managed.len()).map_err(failed)?;
        borrowed.try_reserve(payloads.len()).map_err(failed)?;
        for (index, &(ordinal, holder)) in managed.iter().enumerate() {
            if ordinal >= payloads.len() as u64
                || managed[..index]
                    .iter()
                    .any(|(previous, _)| *previous == ordinal)
            {
                return Err(failed("invalid managed ordinal"));
            }
            holders.push((
                ordinal,
                self.registry
                    .authenticated_holder_id(peer, self.registry.authority(), holder)
                    .map_err(failed)?,
            ));
        }
        if self.ledger.len() >= MAX_GLOBAL_LEASES {
            return Err(failed("ledger quota exceeded"));
        }
        self.ledger.try_reserve(1).map_err(failed)?;
        borrowed.extend(payloads.iter().map(AsFd::as_fd));
        // Mint and prepare are serialized, not two independently reorderable RPCs.
        let key = self.leases.mint_key().map_err(failed)?;
        let transfer = self.leases.prepare(send, key, &borrowed).map_err(failed)?;
        let result = (|| {
            let ids = self
                .registry
                .prepare_scm_batch(peer, key, &holders)
                .map_err(failed)?;
            for ((ordinal, _), id) in managed.iter().zip(ids) {
                manifest.push((*ordinal, id));
                raw_manifest.push((*ordinal, id.get()));
            }
            let response = super::wire::encode_prepared_authenticated(
                key,
                payloads.len(),
                &raw_manifest,
                credentials,
            )?;
            let metadata = super::metadata::create(&response)?;
            self.leases.arm_enqueued(send, key).map_err(failed)?;
            Ok((response, metadata))
        })();
        let (response, metadata) = match result {
            Ok(value) => value,
            Err(error) => {
                // This path precedes arming. A response/send failure after this
                // method returns MUST close writers and retire by EOF instead.
                let _ = self.leases.cancel_unsent(send, key);
                let _ = self.registry.settle_scm_batch(
                    &self.registry.trusted_authorizer(),
                    key,
                    DeliveryDisposition::Aborted,
                );
                return Err(error);
            }
        };
        self.ledger.insert(
            key,
            Ledger {
                send,
                count: payloads.len(),
                managed: manifest,
            receiver: None,
            settled: None,
            credentials,
            },
        );
        Ok(Prepared {
            response,
            metadata,
            transfer,
            credentials,
        })
    }

    pub fn admit(
        &mut self,
        peer: ProcessEpoch,
        carrier: u128,
        key: TransferKey,
        count: usize,
        received: &[(u64, u128)],
        publish: &[u64],
    ) -> Result<Vec<(u64, ClaimedDelivery)>, ProfileError> {
        self.admit_authoritative(peer, carrier, key, count, received, publish)
            .map(|(_, claims)| claims)
    }

    pub fn admit_authoritative(
        &mut self,
        peer: ProcessEpoch,
        carrier: u128,
        key: TransferKey,
        count: usize,
        received: &[(u64, u128)],
        publish: &[u64],
    ) -> Result<(Credentials, Vec<(u64, ClaimedDelivery)>), ProfileError> {
        let carrier = self
            .registry
            .authenticated_holder_id(peer, self.registry.authority(), carrier)
            .map_err(failed)?;
        let receive = TrustedReceiveContext {
            receiver: peer,
            endpoint: carrier.endpoint(),
        };
        let ledger = self
            .ledger
            .get(&key)
            .ok_or_else(|| failed("unknown transfer ledger"))?;
        if ledger.receiver.is_some()
            || ledger.send.endpoint.carrier != carrier.endpoint().carrier
            || count != ledger.count
            || received.len() != ledger.managed.len()
            || !received
                .iter()
                .zip(&ledger.managed)
                .all(|((ordinal, id), (expected, grant))| ordinal == expected && *id == grant.get())
        {
            return Err(failed("exact native manifest mismatch or replay"));
        }
        self.leases
            .validate_import(receive, key, count)
            .map_err(failed)?;
        let mut result = Vec::new();
        result.try_reserve(publish.len()).map_err(failed)?;
        let reserved = self
            .registry
            .prepare_scm_claim(
                &self.registry.trusted_authorizer(),
                peer,
                key,
                &ledger.managed,
                publish,
            )
            .map_err(failed)?;
        // No allocation, randomness, native wait or intervening registry mutation
        // between reservation, core admission and registry commit.
        self.leases
            .admit_import(receive, key, count)
            .map_err(failed)?;
        let claimed = self.registry.commit_scm_claim(reserved).map_err(failed)?;
        for claim in claimed {
            let ordinal = ledger
                .managed
                .iter()
                .find(|(_, id)| *id == claim.delegation)
                .expect("reserved exact ledger delegation")
                .0;
            result.push((ordinal, claim));
        }
        self.ledger
            .get_mut(&key)
            .expect("validated ledger")
            .receiver = Some(peer);
        let credentials = self.ledger.get(&key).expect("validated ledger").credentials;
        Ok((credentials, result))
    }

    pub fn settle(
        &mut self,
        peer: ProcessEpoch,
        key: TransferKey,
        outcome: DeliveryDisposition,
    ) -> Result<(), ProfileError> {
        let ledger = self
            .ledger
            .get(&key)
            .ok_or_else(|| failed("unknown transfer ledger"))?;
        if ledger.receiver != Some(peer) || ledger.settled.is_some() {
            return Err(failed("settlement peer mismatch or replay"));
        }
        self.registry
            .settle_scm_batch(&self.registry.trusted_authorizer(), key, outcome)
            .map_err(failed)?;
        self.ledger.get_mut(&key).expect("validated ledger").settled = Some(outcome);
        Ok(())
    }

    pub fn release_holder(&mut self, peer: ProcessEpoch, id: u128) -> Result<(), ProfileError> {
        let holder = self
            .registry
            .authenticated_holder_id(peer, self.registry.authority(), id)
            .map_err(failed)?;
        self.registry.release_holder(holder).map_err(failed)
    }

    pub fn has_authority_refs(&self) -> bool {
        self.registry.holder_count() != 0 || self.registry.delegation_count() != 0
    }

    pub fn process_died(&mut self, peer: ProcessEpoch) -> Result<(), ProfileError> {
        // Always perform both role cleanups, even if tombstone saturation seals
        // future operations. Neither death notification releases native aliases.
        let source = self.registry.source_died(peer);
        let receiver = self.registry.receiver_died(peer);
        let leases = self.leases.sender_died(peer);
        self.binder_deliveries
            .retain(|_, ledger| ledger.receiver != Some(peer));
        for ledger in self.ledger.values_mut() {
            if ledger.receiver == Some(peer) {
                ledger.settled = Some(DeliveryDisposition::Aborted);
            }
        }
        source.map_err(failed)?;
        receiver.map_err(failed)?;
        leases.map_err(failed)
    }

    pub fn has_pending(&mut self) -> Result<bool, ProfileError> {
        // Bounded allocation-free scan. All queued entries fit the fixed bound;
        // no temporary FD duplication or timeout-derived retirement.
        let mut keys = [None; MAX_GLOBAL_LEASES];
        for (slot, key) in keys.iter_mut().zip(self.ledger.keys()) {
            *slot = Some(*key);
        }
        for key in keys.into_iter().flatten() {
            if self.leases.retire_if_eof(key).map_err(failed)? == Retirement::Retired {
                let ledger = self.ledger.remove(&key).expect("scanned ledger");
                let outcome = ledger.settled.unwrap_or(DeliveryDisposition::Aborted);
                self.registry
                    .settle_scm_batch(&self.registry.trusted_authorizer(), key, outcome)
                    .map_err(failed)?;
            }
        }
        Ok(self.leases.active_count() != 0)
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> [usize; 3] {
        [
            self.registry.holder_count(),
            self.registry.carrier_count(),
            self.registry.delegation_count(),
        ]
    }
}

#[cfg(test)]
#[path = "owner_tests.rs"]
mod tests;
