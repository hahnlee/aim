//! Descriptor authority shared with the authenticated Binder transfer owner.
//! Native FD aliases stay with HostFdDeliveryOwner; these methods own only
//! capability transitions. BinderService proves sessions, deposits and TAKE.
use super::{ScmService, failed, process_epoch};
use crate::{ProfileError, process_incarnation::ProcessIncarnation};
use darwin_art_binder_device::routing_authority::PeerIdentity;
use darwin_art_scm_transfer::{
    MAX_GLOBAL_LEASES, ProcessEpoch,
    capabilities::{
        ATTRIBUTES_BYTES, AttributeKind, BinderManifestItem, Binding, CapabilityAttributes,
        ClaimedDelivery, DeliveryDisposition, encode_attributes,
    },
};

pub(super) struct BinderLedger {
    pub receiver: Option<ProcessEpoch>,
    destination_connection: Option<u64>,
    remaining: usize,
}

pub(super) fn epoch(peer: PeerIdentity) -> ProcessEpoch {
    let [seconds, micros] = peer.incarnation();
    ProcessEpoch {
        pid: peer.pid(),
        instance: ((seconds as u128) << 64) | micros as u128,
    }
}

impl ScmService {
    fn track_binder_peer(&self, peer: PeerIdentity) -> Result<ProcessEpoch, ProfileError> {
        let birth = ProcessIncarnation::read_live(peer.pid())?;
        if birth.parts() != peer.incarnation() {
            return Err(failed("Binder participant birth changed"));
        }
        let process = process_epoch(peer.pid(), birth);
        self.participants
            .lock()
            .map_err(|_| failed("participants poisoned"))?
            .track(process, birth)?;
        Ok(process)
    }

    pub(crate) fn binder_bind(
        &self,
        peer: PeerIdentity,
        binding: Binding,
        holder: u128,
    ) -> Result<[u8; ATTRIBUTES_BYTES], ProfileError> {
        if !matches!(binding, Binding::Binder { .. }) {
            return Err(failed("wrong Binder binding kind"));
        }
        let process = self.track_binder_peer(peer)?;
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let Binding::Binder {
            source_connection,
            transfer,
            ..
        } = binding
        else {
            unreachable!()
        };
        if owner
            .binder_deliveries
            .contains_key(&(source_connection, transfer))
        {
            return Err(failed("Binder manifest already committed or taken"));
        }
        let authority = owner.registry.authority();
        let grant = owner
            .registry
            .authenticated_holder_id(process, authority, holder)
            .map_err(failed)?;
        let delegation = owner
            .registry
            .prepare_delegation(grant, binding)
            .map_err(failed)?;
        Ok(encode_attributes(CapabilityAttributes {
            kind: AttributeKind::Binder,
            authority,
            delegation,
        }))
    }

    pub(crate) fn binder_cancel_pending(
        &self,
        peer: PeerIdentity,
        binding: Binding,
    ) -> Result<(), ProfileError> {
        self.owner
            .lock()
            .map_err(|_| failed("owner poisoned"))?
            .registry
            .cancel_pending_binder_binding(epoch(peer), binding)
            .map_err(failed)
    }

    pub(crate) fn binder_commit_deposit(
        &self,
        peer: PeerIdentity,
        connection: u64,
        transfer: u64,
        manifest: &[BinderManifestItem],
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let key = (connection, transfer);
        if owner.binder_deliveries.contains_key(&key) {
            return Err(failed("duplicate Binder ledger"));
        }
        if !manifest.is_empty() {
            if owner.binder_deliveries.len() >= MAX_GLOBAL_LEASES {
                return Err(failed("Binder ledger quota"));
            }
            owner.binder_deliveries.try_reserve(1).map_err(failed)?;
        }
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .commit_binder_deposit(&authorizer, epoch(peer), connection, transfer, manifest)
            .map_err(failed)?;
        if !manifest.is_empty() {
            owner.binder_deliveries.insert(
                key,
                BinderLedger {
                    receiver: None,
                    destination_connection: None,
                    remaining: manifest.len(),
                },
            );
        }
        Ok(())
    }

    pub(crate) fn binder_authorize_take(
        &self,
        peer: PeerIdentity,
        destination: u64,
        source: u64,
        transfer: u64,
    ) -> Result<(), ProfileError> {
        let process = self.track_binder_peer(peer)?;
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let key = (source, transfer);
        if let Some(ledger) = owner.binder_deliveries.get(&key) {
            if ledger.receiver.is_some() {
                return Err(failed("Binder delivery replay"));
            }
        }
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .authorize_binder_take(&authorizer, source, transfer, process)
            .map_err(failed)?;
        if let Some(ledger) = owner.binder_deliveries.get_mut(&key) {
            ledger.receiver = Some(process);
            ledger.destination_connection = Some(destination);
        }
        Ok(())
    }

    pub(crate) fn binder_claim(
        &self,
        peer: PeerIdentity,
        destination: u64,
        binding: Binding,
        attributes: &[u8],
    ) -> Result<ClaimedDelivery, ProfileError> {
        let Binding::Binder {
            source_connection,
            transfer,
            ..
        } = binding
        else {
            return Err(failed("wrong claim binding kind"));
        };
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let ledger = owner
            .binder_deliveries
            .get(&(source_connection, transfer))
            .ok_or_else(|| failed("unknown Binder delivery ledger"))?;
        let process = epoch(peer);
        if ledger.receiver != Some(process) || ledger.destination_connection != Some(destination) {
            return Err(failed("Binder claim destination mismatch"));
        }
        owner
            .registry
            .claim(process, attributes, binding)
            .map_err(failed)
    }

    pub(crate) fn binder_finish_claim(
        &self,
        claim: ClaimedDelivery,
        source: u64,
        transfer: u64,
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let ledger = owner
            .binder_deliveries
            .get(&(source, transfer))
            .ok_or_else(|| failed("claim lost Binder delivery ledger"))?;
        if ledger.receiver != Some(claim.receiver) || ledger.remaining == 0 {
            return Err(failed("Binder claim settlement mismatch"));
        }
        owner
            .registry
            .finish(claim.delegation, claim.grant)
            .map_err(failed)?;
        owner
            .binder_deliveries
            .get_mut(&(source, transfer))
            .expect("validated ledger")
            .remaining -= 1;
        Ok(())
    }

    pub(crate) fn binder_abort_claim(&self, claim: ClaimedDelivery) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .abort_binder_claim_receipt(&authorizer, claim)
            .map_err(failed)
    }

    pub(crate) fn binder_receiver_settle(
        &self,
        peer: PeerIdentity,
        destination: u64,
        source: u64,
        transfer: u64,
        outcome: DeliveryDisposition,
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let key = (source, transfer);
        let Some(ledger) = owner.binder_deliveries.get(&key) else {
            return Ok(());
        };
        let process = epoch(peer);
        if ledger.receiver != Some(process) || ledger.destination_connection != Some(destination) {
            return Err(failed("Binder settlement destination mismatch"));
        }
        if outcome == DeliveryDisposition::Finished && ledger.remaining != 0 {
            return Err(failed("unfinished Binder descriptor claims"));
        }
        if outcome == DeliveryDisposition::Aborted {
            let authorizer = owner.registry.trusted_authorizer();
            owner
                .registry
                .abort_binder_transfer(&authorizer, source, transfer, Some(process))
                .map_err(failed)?;
        }
        owner.binder_deliveries.remove(&key);
        Ok(())
    }

    pub(crate) fn binder_cancel_pending_transfer(
        &self,
        peer: PeerIdentity,
        source: u64,
        transfer: u64,
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .cancel_pending_binder_transfer(&authorizer, epoch(peer), source, transfer)
            .map_err(failed)
    }

    /// The Binder owner already proved these transfers were discarded. Pending
    /// Bind cleanup is separate from committed/routed delivery lifetime.
    pub(crate) fn binder_discard_transfer(
        &self,
        source: u64,
        transfer: u64,
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .abort_binder_transfer(&authorizer, source, transfer, None)
            .map_err(failed)?;
        owner.binder_deliveries.remove(&(source, transfer));
        Ok(())
    }

    pub(crate) fn binder_session_closed(
        &self,
        peer: PeerIdentity,
        connection: u64,
    ) -> Result<(), ProfileError> {
        let mut owner = self.owner.lock().map_err(|_| failed("owner poisoned"))?;
        let authorizer = owner.registry.trusted_authorizer();
        owner
            .registry
            .cancel_pending_binder_session(&authorizer, epoch(peer), connection)
            .map_err(failed)?;
        // TAKE-removed deliveries are no longer in TransferTable. Exact
        // destination-session loss makes their unclaimed capability unusable.
        let mut keys = [None; MAX_GLOBAL_LEASES];
        for (slot, key) in keys
            .iter_mut()
            .zip(owner.binder_deliveries.iter().filter_map(|(key, ledger)| {
                (ledger.destination_connection == Some(connection)
                    && ledger.receiver == Some(epoch(peer)))
                .then_some(*key)
            }))
        {
            *slot = Some(key);
        }
        for (source, transfer) in keys.into_iter().flatten() {
            owner
                .registry
                .abort_binder_transfer(&authorizer, source, transfer, Some(epoch(peer)))
                .map_err(failed)?;
            owner.binder_deliveries.remove(&(source, transfer));
        }
        Ok(())
    }
}
