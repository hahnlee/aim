//! Public process-endpoint ingress; routing/authentication is owned by the
//! profile authority and final binder_proc publication by `Device`.

use super::*;

impl OpenConnection {
    pub fn fail_remote_authority(&self) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .fail_remote_authority(key)
            .map_err(Error::Registry)
    }

    pub fn deliver_remote_transaction(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteTransaction,
    ) -> Result<usize, Error<std::convert::Infallible>> {
        self.deliver_remote_transaction_with_fds(payload, receiver, remote, Vec::new())
    }

    pub fn deliver_remote_transaction_with_fds(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteTransaction,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .enqueue_remote_transaction(key, payload, receiver, remote, files)
            .map_err(Error::Registry)
    }

    pub fn deliver_remote_reply(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteReply,
    ) -> Result<usize, Error<std::convert::Infallible>> {
        self.deliver_remote_reply_with_fds(payload, receiver, remote, Vec::new())
    }

    pub fn deliver_remote_reply_with_fds(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteReply,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .enqueue_remote_reply(key, payload, receiver, remote, files)
            .map_err(Error::Registry)
    }

    pub fn deliver_remote_target_dead(
        &self,
        thread_id: u64,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .enqueue_dead_reply(crate::thread::CallId::remote(call))
            .map_err(Error::Thread)
    }

    pub fn deliver_remote_caller_dead(
        &self,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .cancel_remote_incoming(key, call)
            .map_err(Error::Registry)
    }
}
