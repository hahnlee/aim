//! Authenticated Binder descriptor deposit/TAKE policy, not native FD transport.
//! Lock order is endpoints -> transfers -> shared descriptor authority. All
//! socket I/O occurs before/after these bounded state transitions.
use super::{BinderService, TransferDelivery, poisoned, transfer_failed};
use crate::{ProfileError, scm_service::ScmService};
use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, TransferToken},
    routing_authority::PeerIdentity,
};
use darwin_art_scm_transfer::capabilities::{Binding, ClaimedDelivery, DeliveryDisposition};
use std::{os::fd::OwnedFd, sync::Arc};

pub(super) struct DeliveryReceipt {
    authority: Arc<ScmService>,
    peer: PeerIdentity,
    destination: u64,
    source: u64,
    transfer: u64,
    admitted: bool,
}
impl Drop for DeliveryReceipt {
    fn drop(&mut self) {
        if !self.admitted {
            if let Err(error) = self.authority.binder_receiver_settle(
                self.peer,
                self.destination,
                self.source,
                self.transfer,
                DeliveryDisposition::Aborted,
            ) {
                eprintln!("Binder native delivery discard failed: {error}");
            }
        }
    }
}
impl DeliveryReceipt {
    pub(super) fn admit(&mut self) {
        self.admitted = true;
    }
}

impl BinderService {
    pub(crate) fn with_descriptor_authority(authority: Arc<ScmService>) -> Self {
        Self {
            descriptor_authority: Some(authority),
            ..Self::default()
        }
    }

    fn with_peer_connection<T>(
        &self,
        peer: PeerIdentity,
        operation: impl FnOnce(ConnectionToken) -> Result<T, ProfileError>,
    ) -> Result<T, ProfileError> {
        let endpoints = self.endpoints.lock().map_err(|_| poisoned())?;
        let mut matches = endpoints
            .iter()
            .filter_map(|(token, endpoint)| (endpoint.peer == peer).then_some(*token));
        let connection = matches
            .next()
            .ok_or_else(|| ProfileError::Daemon("Binder peer has no active session".into()))?;
        if matches.next().is_some() {
            return Err(ProfileError::Daemon(
                "Binder peer has multiple active sessions".into(),
            ));
        }
        // Keep session authority through mutation; disconnect cannot race a
        // late Bind/deposit/TAKE into a removed session.
        operation(connection)
    }

    fn authority(&self) -> Result<&Arc<ScmService>, ProfileError> {
        self.descriptor_authority
            .as_ref()
            .ok_or_else(|| ProfileError::Daemon("Binder descriptor authority unavailable".into()))
    }

    fn validate_source_binding(
        connection: ConnectionToken,
        binding: Binding,
    ) -> Result<(), ProfileError> {
        if !matches!(binding, Binding::Binder { source_connection, .. } if source_connection == connection.get())
        {
            return Err(ProfileError::Daemon(
                "Binder binding does not name authenticated source session".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn bind_endpoint(
        &self,
        peer: PeerIdentity,
        binding: Binding,
        holder: u128,
    ) -> Result<[u8; 40], ProfileError> {
        self.with_peer_connection(peer, |connection| {
            Self::validate_source_binding(connection, binding)?;
            let Binding::Binder { transfer, .. } = binding else {
                unreachable!()
            };
            let token = TransferToken::from_nonzero(transfer).expect("validated nonzero binding");
            let table = self.transfers.lock().map_err(|_| poisoned())?;
            if table.contains_transfer(connection, token) {
                return Err(ProfileError::Daemon(
                    "Binder image already committed".into(),
                ));
            }
            self.authority()?.binder_bind(peer, binding, holder)
        })
    }
    pub(crate) fn cancel_pending_endpoint(
        &self,
        peer: PeerIdentity,
        binding: Binding,
    ) -> Result<(), ProfileError> {
        self.with_peer_connection(peer, |connection| {
            Self::validate_source_binding(connection, binding)?;
            self.authority()?.binder_cancel_pending(peer, binding)
        })
    }
    pub(crate) fn rollback_pending_endpoint(
        &self,
        peer: PeerIdentity,
        binding: Binding,
    ) -> Result<(), ProfileError> {
        // This receipt was created by a genuinely authenticated Bind handler.
        // Session teardown can happen during I/O; it must not block cleanup.
        self.authority()?.binder_cancel_pending(peer, binding)
    }
    pub(crate) fn claim_endpoint(
        &self,
        peer: PeerIdentity,
        binding: Binding,
        attributes: &[u8],
    ) -> Result<ClaimedDelivery, ProfileError> {
        self.with_peer_connection(peer, |connection| {
            self.authority()?
                .binder_claim(peer, connection.get(), binding, attributes)
        })
    }
    pub(crate) fn finish_endpoint_claim(
        &self,
        claim: ClaimedDelivery,
        source: u64,
        transfer: u64,
    ) -> Result<(), ProfileError> {
        self.authority()?
            .binder_finish_claim(claim, source, transfer)
    }
    pub(crate) fn abort_endpoint_claim(&self, claim: ClaimedDelivery) -> Result<(), ProfileError> {
        self.authority()?.binder_abort_claim(claim)
    }

    pub(super) fn deposit_descriptors(
        &self,
        peer: PeerIdentity,
        token: TransferToken,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), ProfileError> {
        self.with_peer_connection(peer, |connection| {
            let mut table = self.transfers.lock().map_err(|_| poisoned())?;
            let prepared = table
                .prepare_deposit(connection, token, descriptors)
                .map_err(transfer_failed)?;
            if let Some(authority) = &self.descriptor_authority {
                authority.binder_commit_deposit(
                    peer,
                    connection.get(),
                    token.get(),
                    &prepared.managed,
                )?;
            } else if !prepared.managed.is_empty() {
                return Err(ProfileError::Daemon(
                    "managed Binder deposit requires descriptor authority".into(),
                ));
            }
            table.install_deposit(prepared);
            Ok(())
        })
    }

    pub(super) fn prepare_descriptor_take(
        &self,
        peer: PeerIdentity,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferDelivery, ProfileError> {
        self.with_peer_connection(peer, |destination| {
            let mut table = self.transfers.lock().map_err(|_| poisoned())?;
            table
                .verify_take(destination, source, token)
                .map_err(transfer_failed)?;
            let receipt = if let Some(authority) = &self.descriptor_authority {
                authority.binder_authorize_take(
                    peer,
                    destination.get(),
                    source.get(),
                    token.get(),
                )?;
                Some(DeliveryReceipt {
                    authority: authority.clone(),
                    peer,
                    destination: destination.get(),
                    source: source.get(),
                    transfer: token.get(),
                    admitted: false,
                })
            } else {
                None
            };
            let descriptors = table
                .take(destination, source, token)
                .map_err(transfer_failed)?;
            Ok(TransferDelivery {
                descriptors,
                receipt,
            })
        })
    }

    #[cfg(test)]
    pub(super) fn take_descriptors(
        &self,
        peer: PeerIdentity,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<Vec<OwnedFd>, ProfileError> {
        self.prepare_descriptor_take(peer, source, token)
            .map(TransferDelivery::into_descriptors)
    }

    pub(super) fn discard_descriptor_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) {
        if let Some(authority) = &self.descriptor_authority {
            if let Err(error) = authority.binder_discard_transfer(source.get(), token.get()) {
                eprintln!("Binder discarded transfer capability cleanup failed: {error}");
            }
        }
    }

    pub(crate) fn cancel_unrouted_transfer(
        &self,
        peer: PeerIdentity,
        token: TransferToken,
    ) -> Result<(), ProfileError> {
        self.with_peer_connection(peer, |source| {
            let removed = self
                .transfers
                .lock()
                .map_err(|_| poisoned())?
                .cancel_unrouted(source, token)
                .map_err(transfer_failed)?;
            match removed {
                crate::binder_transfer::CancelUnrouted::Discarded => self
                    .authority()?
                    .binder_discard_transfer(source.get(), token.get())?,
                crate::binder_transfer::CancelUnrouted::Absent => self
                    .authority()?
                    .binder_cancel_pending_transfer(peer, source.get(), token.get())?,
                crate::binder_transfer::CancelUnrouted::Routed => {}
            }
            Ok(())
        })
    }
    pub(crate) fn settle_received_transfer(
        &self,
        peer: PeerIdentity,
        source: ConnectionToken,
        token: TransferToken,
        outcome: DeliveryDisposition,
    ) -> Result<(), ProfileError> {
        self.with_peer_connection(peer, |destination| {
            self.authority()?.binder_receiver_settle(
                peer,
                destination.get(),
                source.get(),
                token.get(),
                outcome,
            )
        })
    }
}
