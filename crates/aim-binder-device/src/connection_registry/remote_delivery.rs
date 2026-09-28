//! Profile-authority ingress into one authenticated process-local connection.

use super::*;

impl Registry {
    pub(crate) fn resolve_remote_objects(
        &self,
        key: &Key,
        payload: &crate::transaction_snapshot::TransactionSnapshot,
    ) -> Result<Vec<crate::remote_objects::OutboundObject>, Error> {
        self.lookup(key)?
            .resolve_remote_objects(payload)
            .map_err(Error::Connection)
    }

    pub(crate) fn install_remote_reference(
        &self,
        key: &Key,
        node: crate::authority_protocol::NodeToken,
        strength: crate::reference_table::Strength,
        context_manager: bool,
    ) -> Result<u32, Error> {
        self.lookup(key)?
            .install_remote_reference(node, strength, context_manager)
            .map_err(Error::Connection)
    }

    pub(crate) fn resolve_remote_transaction_target(
        &self,
        key: &Key,
        request: &crate::transaction_request::Request,
    ) -> Result<Option<crate::authority_protocol::NodeToken>, Error> {
        self.lookup(key)?
            .resolve_remote_transaction_target(request)
            .map_err(Error::Connection)
    }

    pub(crate) fn enqueue_remote_transaction(
        &self,
        key: &Key,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteTransaction,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error> {
        let _gate = self.transaction_gate.lock().map_err(|_| Error::Poisoned)?;
        self.lookup(key)?
            .enqueue_remote_transaction(payload, receiver, remote, files)
            .map_err(Error::Connection)
    }

    pub(crate) fn enqueue_remote_reply(
        &self,
        key: &Key,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteReply,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error> {
        let _gate = self.transaction_gate.lock().map_err(|_| Error::Poisoned)?;
        self.lookup(key)?
            .enqueue_remote_reply(payload, receiver, remote, files)
            .map_err(Error::Connection)
    }

    pub(crate) fn cancel_remote_incoming(
        &self,
        key: &Key,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error> {
        let _gate = self.transaction_gate.lock().map_err(|_| Error::Poisoned)?;
        self.lookup(key)?
            .cancel_remote_incoming(call)
            .map_err(Error::Connection)
    }
}
