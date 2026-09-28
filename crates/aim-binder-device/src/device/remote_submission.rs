//! Two-phase binder_thread state around profile-authority transport. No
//! Registry or Thread mutex survives between begin and commit/abort.

use super::*;

impl OpenConnection {
    pub fn begin_remote_submission(
        &self,
        thread_id: u64,
        synchronous: bool,
    ) -> Result<crate::thread::RemoteSubmission, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .begin_remote_submission(synchronous)
            .map_err(Error::Thread)
    }

    pub fn commit_remote_submission(
        &self,
        thread_id: u64,
        submission: crate::thread::RemoteSubmission,
        call: Option<crate::authority_protocol::CallToken>,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .commit_remote_submission(submission, call)
            .map_err(Error::Thread)
    }

    pub fn abort_remote_submission(
        &self,
        thread_id: u64,
        submission: crate::thread::RemoteSubmission,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .abort_remote_submission(submission)
            .map_err(Error::Thread)
    }

    pub fn reject_remote_submission(
        &self,
        thread_id: u64,
        submission: crate::thread::RemoteSubmission,
        reason: crate::authority_protocol::TransactionFailure,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        thread
            .reject_remote_submission(submission, reason)
            .map_err(Error::Thread)
    }
}
