//! Two-phase binder_thread reply state around authority transport. No device
//! lock survives between begin and commit/abort.

use super::*;

impl OpenConnection {
    pub fn begin_remote_reply(
        &self,
        thread_id: u64,
    ) -> Result<
        Option<(
            crate::thread::RemoteReplySubmission,
            crate::authority_protocol::CallToken,
        )>,
        Error<std::convert::Infallible>,
    > {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread.begin_remote_reply().map_err(Error::Thread)
    }

    pub fn commit_remote_reply(
        &self,
        thread_id: u64,
        submission: crate::thread::RemoteReplySubmission,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .commit_remote_reply(submission, call)
            .map_err(Error::Thread)
    }

    pub fn abort_remote_reply(
        &self,
        thread_id: u64,
        submission: crate::thread::RemoteReplySubmission,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .abort_remote_reply(submission, call)
            .map_err(Error::Thread)
    }
}
