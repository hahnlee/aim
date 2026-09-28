//! binder_thread synchronous transaction stack and thread-local BR work.

use super::*;

pub const BR_TRANSACTION_COMPLETE: u32 = 0x0000_7206;
pub const BR_DEAD_REPLY: u32 = 0x0000_7205;
pub const BR_FAILED_REPLY: u32 = 0x0000_7211;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallId {
    /// Call routed entirely inside one process-local Device registry.
    Local(u64),
    /// Call allocated by the authenticated profile-wide routing authority.
    Remote(crate::authority_protocol::CallToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RemoteSubmission(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RemoteReplySubmission(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutgoingCall {
    Pending(RemoteSubmission),
    Rejected(RemoteSubmission),
    Active(CallId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IncomingCall {
    Active(CallId),
    CallerDead(CallId),
    PendingReply(RemoteReplySubmission, CallId),
}

impl CallId {
    pub(crate) fn local(serial: u64) -> Self {
        Self::Local(serial)
    }

    pub fn remote(token: crate::authority_protocol::CallToken) -> Self {
        Self::Remote(token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalWork {
    TransactionComplete(Option<CallId>),
    DeadReply(CallId),
    ReservedTerminal(CallId),
    PendingRemote(RemoteSubmission),
    RejectedRemote(
        RemoteSubmission,
        bool,
        crate::authority_protocol::TransactionFailure,
    ),
    PendingRemoteReply(RemoteReplySubmission),
}

pub struct SubmissionReservation<'a> {
    thread: &'a mut Thread,
    synchronous: bool,
}

impl Thread {
    fn last_outgoing(&self) -> Option<CallId> {
        match self.outgoing.last() {
            Some(OutgoingCall::Active(call)) => Some(*call),
            Some(OutgoingCall::Pending(_) | OutgoingCall::Rejected(_)) | None => None,
        }
    }

    /// Reserve every allocation before transaction publication. Commit is then
    /// infallible, so target work can never be visible without caller state.
    pub fn reserve_submission(
        &mut self,
        synchronous: bool,
    ) -> Result<SubmissionReservation<'_>, Error> {
        // A synchronous call may later need one terminal BR_DEAD_REPLY in
        // addition to its immediate BR_TRANSACTION_COMPLETE. Reserve both
        // before publication so connection teardown never allocates.
        self.local_work
            .try_reserve(if synchronous { 2 } else { 1 })
            .map_err(|_| Error::OutOfMemory)?;
        if synchronous {
            self.outgoing
                .try_reserve(1)
                .map_err(|_| Error::OutOfMemory)?;
        }
        Ok(SubmissionReservation {
            thread: self,
            synchronous,
        })
    }

    pub fn reserve_incoming(&mut self) -> Result<(), Error> {
        self.incoming.try_reserve(1).map_err(|_| Error::OutOfMemory)
    }

    pub fn accept_incoming(&mut self, call: CallId) {
        debug_assert!(self.incoming.len() < self.incoming.capacity());
        self.incoming.push(IncomingCall::Active(call));
    }

    pub fn current_incoming(&self) -> Result<CallId, Error> {
        self.incoming
            .last()
            .and_then(|entry| match entry {
                IncomingCall::Active(call) | IncomingCall::CallerDead(call) => Some(*call),
                IncomingCall::PendingReply(_, _) => None,
            })
            .ok_or(Error::InvalidReplyStack)
    }

    pub fn finish_incoming(&mut self, call: CallId) -> Result<(), Error> {
        if self.incoming.last().copied() != Some(IncomingCall::Active(call)) {
            return Err(Error::InvalidReplyStack);
        }
        self.incoming.pop();
        Ok(())
    }

    pub(crate) fn cancel_incoming(&mut self, call: CallId) -> bool {
        let Some(position) = self.incoming.iter().rposition(|entry| match entry {
            IncomingCall::Active(entry)
            | IncomingCall::CallerDead(entry)
            | IncomingCall::PendingReply(_, entry) => *entry == call,
        }) else {
            return false;
        };
        match self.incoming[position] {
            IncomingCall::Active(CallId::Remote(_)) => {
                self.incoming[position] = IncomingCall::CallerDead(call);
            }
            IncomingCall::CallerDead(_) => return false,
            IncomingCall::PendingReply(_, _) => {
                // The authority retains an executing call as a tombstone so
                // the target may complete exactly one late reply. Preserve
                // an already submitted reply until ReplyAccepted commits it.
            }
            IncomingCall::Active(CallId::Local(_)) => return false,
        }
        true
    }

    pub fn finish_outgoing(&mut self, call: CallId) -> Result<(), Error> {
        if self.last_outgoing() != Some(call) {
            return Err(Error::InvalidOutgoingReply);
        }
        self.outgoing.pop();
        self.local_work
            .retain(|work| *work != LocalWork::ReservedTerminal(call));
        Ok(())
    }

    pub fn validate_outgoing(&self, call: CallId) -> Result<(), Error> {
        (self.last_outgoing() == Some(call))
            .then_some(())
            .ok_or(Error::InvalidOutgoingReply)
    }

    pub fn read_local_work(&mut self, output: &mut [u8]) -> usize {
        if self.local_work.is_empty() || output.len() < 4 {
            return 0;
        }
        let Some(position) = self.local_work.iter().position(|work| match work {
            LocalWork::TransactionComplete(None) => true,
            LocalWork::TransactionComplete(Some(call)) => self.last_outgoing() == Some(*call),
            LocalWork::DeadReply(call) => self.last_outgoing() == Some(*call),
            LocalWork::RejectedRemote(submission, synchronous, _) => {
                !synchronous || self.outgoing.last() == Some(&OutgoingCall::Rejected(*submission))
            }
            LocalWork::ReservedTerminal(_)
            | LocalWork::PendingRemote(_)
            | LocalWork::PendingRemoteReply(_) => false,
        }) else {
            return 0;
        };
        let work = self
            .local_work
            .remove(position)
            .expect("selected live local work");
        let command = match work {
            LocalWork::TransactionComplete(_) => BR_TRANSACTION_COMPLETE,
            LocalWork::DeadReply(call) => {
                self.outgoing.pop();
                debug_assert_ne!(Some(call), self.last_outgoing());
                BR_DEAD_REPLY
            }
            LocalWork::RejectedRemote(submission, synchronous, reason) => {
                if synchronous {
                    debug_assert_eq!(
                        self.outgoing.pop(),
                        Some(OutgoingCall::Rejected(submission))
                    );
                }
                match reason {
                    crate::authority_protocol::TransactionFailure::DeadReply => BR_DEAD_REPLY,
                    crate::authority_protocol::TransactionFailure::FailedReply => BR_FAILED_REPLY,
                }
            }
            LocalWork::ReservedTerminal(_)
            | LocalWork::PendingRemote(_)
            | LocalWork::PendingRemoteReply(_) => {
                unreachable!("only visible local work is selected")
            }
        };
        output[..4].copy_from_slice(&command.to_le_bytes());
        4
    }

    pub fn has_outgoing(&self) -> bool {
        !self.outgoing.is_empty()
    }

    pub(crate) fn enqueue_dead_reply(&mut self, call: CallId) -> Result<(), Error> {
        if !self.outgoing.contains(&OutgoingCall::Active(call)) {
            return Err(Error::InvalidOutgoingReply);
        }
        if self
            .local_work
            .iter()
            .filter(|work| **work == LocalWork::ReservedTerminal(call))
            .count()
            != 2
        {
            return Err(Error::InvalidOutgoingReply);
        }
        let mut found = 0;
        for work in &mut self.local_work {
            if *work == LocalWork::ReservedTerminal(call) {
                *work = if found == 0 {
                    LocalWork::TransactionComplete(Some(call))
                } else {
                    LocalWork::DeadReply(call)
                };
                found += 1;
            }
        }
        debug_assert_eq!(found, 2);
        if let Some(signal) = &self.work_signal {
            signal.notify();
        }
        Ok(())
    }

    /// Complete every authority-owned synchronous call after the profile-wide
    /// routing transport has irrecoverably closed. Reserved terminal work makes
    /// this allocation-free and preserves normal BR_TRANSACTION_COMPLETE then
    /// BR_DEAD_REPLY ordering for each call.
    pub(crate) fn fail_remote_authority(&mut self) -> Result<(), Error> {
        let calls: Vec<_> = self
            .outgoing
            .iter()
            .filter_map(|entry| match entry {
                OutgoingCall::Active(CallId::Remote(call)) => Some(CallId::Remote(*call)),
                OutgoingCall::Active(CallId::Local(_))
                | OutgoingCall::Pending(_)
                | OutgoingCall::Rejected(_) => None,
            })
            .collect();
        for call in calls {
            self.enqueue_dead_reply(call)?;
        }
        Ok(())
    }

    pub(crate) fn begin_remote_submission(
        &mut self,
        synchronous: bool,
    ) -> Result<RemoteSubmission, Error> {
        let serial = self
            .next_remote_submission
            .checked_add(1)
            .ok_or(Error::SubmissionIdsExhausted)?;
        self.local_work
            .try_reserve(if synchronous { 2 } else { 1 })
            .map_err(|_| Error::OutOfMemory)?;
        if synchronous {
            self.outgoing
                .try_reserve(1)
                .map_err(|_| Error::OutOfMemory)?;
        }
        let token = RemoteSubmission(serial);
        self.next_remote_submission = serial;
        if synchronous {
            self.outgoing.push(OutgoingCall::Pending(token));
            self.local_work.push_back(LocalWork::PendingRemote(token));
        }
        self.local_work.push_back(LocalWork::PendingRemote(token));
        Ok(token)
    }

    pub(crate) fn commit_remote_submission(
        &mut self,
        token: RemoteSubmission,
        call: Option<crate::authority_protocol::CallToken>,
    ) -> Result<(), Error> {
        let mut positions = [0_usize; 2];
        let mut count = 0;
        for (index, work) in self.local_work.iter().enumerate() {
            if *work == LocalWork::PendingRemote(token) {
                if count == positions.len() {
                    return Err(Error::InvalidOutgoingReply);
                }
                positions[count] = index;
                count += 1;
            }
        }
        match call {
            Some(call) if count == 2 => {
                let outgoing = self
                    .outgoing
                    .iter_mut()
                    .find(|entry| **entry == OutgoingCall::Pending(token))
                    .ok_or(Error::InvalidOutgoingReply)?;
                let call = CallId::remote(call);
                *outgoing = OutgoingCall::Active(call);
                self.local_work[positions[0]] = LocalWork::ReservedTerminal(call);
                self.local_work[positions[1]] = LocalWork::ReservedTerminal(call);
            }
            None if count == 1 => {
                if self.outgoing.contains(&OutgoingCall::Pending(token)) {
                    return Err(Error::InvalidOutgoingReply);
                }
                self.local_work[positions[0]] = LocalWork::TransactionComplete(None);
                if let Some(signal) = &self.work_signal {
                    signal.notify();
                }
            }
            _ => return Err(Error::InvalidOutgoingReply),
        }
        Ok(())
    }

    pub(crate) fn abort_remote_submission(&mut self, token: RemoteSubmission) -> Result<(), Error> {
        let before = self.local_work.len();
        self.local_work
            .retain(|work| *work != LocalWork::PendingRemote(token));
        let removed_work = before - self.local_work.len();
        let outgoing = self
            .outgoing
            .iter()
            .position(|entry| *entry == OutgoingCall::Pending(token));
        if let Some(position) = outgoing {
            self.outgoing.remove(position);
        }
        if !matches!((removed_work, outgoing), (1, None) | (2, Some(_))) {
            return Err(Error::InvalidOutgoingReply);
        }
        Ok(())
    }

    /// Finish a transaction which the profile authority could not publish.
    /// Linux Binder reports only the terminal error in this pre-acceptance
    /// case; it does not also publish BR_TRANSACTION_COMPLETE.
    pub(crate) fn reject_remote_submission(
        &mut self,
        token: RemoteSubmission,
        reason: crate::authority_protocol::TransactionFailure,
    ) -> Result<(), Error> {
        let positions: Vec<_> = self
            .local_work
            .iter()
            .enumerate()
            .filter_map(|(index, work)| (*work == LocalWork::PendingRemote(token)).then_some(index))
            .collect();
        let outgoing = self
            .outgoing
            .iter()
            .position(|entry| *entry == OutgoingCall::Pending(token));
        if !matches!((positions.len(), outgoing), (1, None) | (2, Some(_))) {
            return Err(Error::InvalidOutgoingReply);
        }
        let synchronous = outgoing.is_some();
        self.local_work[positions[0]] = LocalWork::RejectedRemote(token, synchronous, reason);
        if positions.len() == 2 {
            self.local_work.remove(positions[1]);
        }
        if let Some(position) = outgoing {
            self.outgoing[position] = OutgoingCall::Rejected(token);
        }
        if let Some(signal) = &self.work_signal {
            signal.notify();
        }
        Ok(())
    }

    pub(crate) fn begin_remote_reply(
        &mut self,
    ) -> Result<Option<(RemoteReplySubmission, crate::authority_protocol::CallToken)>, Error> {
        let call = match self.incoming.last().copied() {
            Some(IncomingCall::Active(CallId::Remote(call)))
            | Some(IncomingCall::CallerDead(CallId::Remote(call))) => call,
            Some(IncomingCall::Active(CallId::Local(_))) => return Ok(None),
            Some(IncomingCall::CallerDead(CallId::Local(_)))
            | Some(IncomingCall::PendingReply(_, _))
            | None => {
                return Err(Error::InvalidReplyStack);
            }
        };
        let serial = self
            .next_remote_submission
            .checked_add(1)
            .ok_or(Error::SubmissionIdsExhausted)?;
        self.local_work
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        let submission = RemoteReplySubmission(serial);
        self.next_remote_submission = serial;
        *self.incoming.last_mut().expect("active call checked") =
            IncomingCall::PendingReply(submission, CallId::Remote(call));
        self.local_work
            .push_back(LocalWork::PendingRemoteReply(submission));
        Ok(Some((submission, call)))
    }

    pub(crate) fn commit_remote_reply(
        &mut self,
        submission: RemoteReplySubmission,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error> {
        if self.incoming.last().copied()
            != Some(IncomingCall::PendingReply(submission, CallId::Remote(call)))
        {
            return Err(Error::InvalidReplyStack);
        }
        let Some(position) = self
            .local_work
            .iter()
            .position(|work| *work == LocalWork::PendingRemoteReply(submission))
        else {
            return Err(Error::InvalidReplyStack);
        };
        self.incoming.pop();
        self.local_work[position] = LocalWork::TransactionComplete(None);
        if let Some(signal) = &self.work_signal {
            signal.notify();
        }
        Ok(())
    }

    pub(crate) fn abort_remote_reply(
        &mut self,
        submission: RemoteReplySubmission,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error> {
        if self.incoming.last().copied()
            != Some(IncomingCall::PendingReply(submission, CallId::Remote(call)))
        {
            return Err(Error::InvalidReplyStack);
        }
        let before = self.local_work.len();
        self.local_work
            .retain(|work| *work != LocalWork::PendingRemoteReply(submission));
        if before - self.local_work.len() != 1 {
            return Err(Error::InvalidReplyStack);
        }
        *self.incoming.last_mut().expect("pending reply checked") =
            IncomingCall::Active(CallId::Remote(call));
        Ok(())
    }
}

impl SubmissionReservation<'_> {
    pub fn thread_id(&self) -> u64 {
        self.thread.id
    }

    pub fn commit(self, call: Option<CallId>) {
        debug_assert_eq!(self.synchronous, call.is_some());
        if let Some(call) = call {
            debug_assert!(self.thread.outgoing.len() < self.thread.outgoing.capacity());
            self.thread.outgoing.push(OutgoingCall::Active(call));
            self.thread
                .local_work
                .push_back(LocalWork::ReservedTerminal(call));
            self.thread
                .local_work
                .push_back(LocalWork::ReservedTerminal(call));
        } else {
            debug_assert!(self.thread.local_work.len() < self.thread.local_work.capacity());
            self.thread
                .local_work
                .push_back(LocalWork::TransactionComplete(None));
            if let Some(signal) = &self.thread.work_signal {
                signal.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_and_nested_stack_are_thread_owned() {
        let mut thread = Thread::new(7).unwrap();
        assert_eq!(
            thread.ensure_read_eligible(),
            Err(Error::NotEligibleForRead)
        );
        thread
            .reserve_submission(true)
            .unwrap()
            .commit(Some(CallId::local(1)));
        assert!(thread.ensure_read_eligible().is_ok());
        assert_eq!(thread.read_local_work(&mut [0; 3]), 0);
        let mut output = [0; 4];
        assert_eq!(thread.read_local_work(&mut output), 0);
        thread.reserve_incoming().unwrap();
        thread.accept_incoming(CallId::local(2));
        thread.reserve_incoming().unwrap();
        thread.accept_incoming(CallId::local(3));
        assert_eq!(thread.current_incoming(), Ok(CallId::local(3)));
        assert_eq!(
            thread.finish_incoming(CallId::local(2)),
            Err(Error::InvalidReplyStack)
        );
        thread.finish_incoming(CallId::local(3)).unwrap();
        thread.finish_incoming(CallId::local(2)).unwrap();
        assert_eq!(
            thread.finish_outgoing(CallId::local(2)),
            Err(Error::InvalidOutgoingReply)
        );
        thread.finish_outgoing(CallId::local(1)).unwrap();
        assert_eq!(
            thread.ensure_read_eligible(),
            Err(Error::NotEligibleForRead)
        );
    }

    #[test]
    fn oneway_gets_completion_without_outgoing_frame() {
        let mut thread = Thread::new(9).unwrap();
        thread.reserve_submission(false).unwrap().commit(None);
        assert!(!thread.has_outgoing());
        assert!(thread.ensure_read_eligible().is_ok());
        assert_eq!(thread.read_local_work(&mut [0; 4]), 4);
        assert_eq!(
            thread.ensure_read_eligible(),
            Err(Error::NotEligibleForRead)
        );
    }

    #[test]
    fn dead_reply_waits_until_the_matching_nested_call_is_topmost() {
        let mut thread = Thread::new(11).unwrap();
        thread
            .reserve_submission(true)
            .unwrap()
            .commit(Some(CallId::local(1)));
        thread
            .reserve_submission(true)
            .unwrap()
            .commit(Some(CallId::local(2)));
        let mut output = [0; 4];
        thread.enqueue_dead_reply(CallId::local(1)).unwrap();
        assert_eq!(thread.read_local_work(&mut output), 0);
        thread.finish_outgoing(CallId::local(2)).unwrap();
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_TRANSACTION_COMPLETE);
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_DEAD_REPLY);
        assert!(!thread.has_outgoing());
    }

    #[test]
    fn remote_submission_commit_abort_and_terminal_slots_are_allocation_free() {
        let mut thread = Thread::new(13).unwrap();
        let aborted = thread.begin_remote_submission(true).unwrap();
        assert!(thread.has_outgoing());
        assert_eq!(thread.read_local_work(&mut [0; 4]), 0);
        thread.abort_remote_submission(aborted).unwrap();
        assert!(!thread.has_outgoing());
        assert_eq!(
            thread.abort_remote_submission(aborted),
            Err(Error::InvalidOutgoingReply)
        );

        let token = crate::authority_protocol::CallToken::from_nonzero(1).unwrap();
        let committed = thread.begin_remote_submission(true).unwrap();
        thread
            .commit_remote_submission(committed, Some(token))
            .unwrap();
        let remote = CallId::remote(token);
        assert_ne!(remote, CallId::local(1));
        thread.enqueue_dead_reply(remote).unwrap();
        let mut output = [0; 4];
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_TRANSACTION_COMPLETE);
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_DEAD_REPLY);
        assert!(!thread.has_outgoing());

        let oneway = thread.begin_remote_submission(false).unwrap();
        thread.commit_remote_submission(oneway, None).unwrap();
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_TRANSACTION_COMPLETE);
    }

    #[test]
    fn rejected_remote_submission_returns_one_error_and_preserves_nested_order() {
        use crate::authority_protocol::TransactionFailure;

        let mut thread = Thread::new(15).unwrap();
        let outer = thread.begin_remote_submission(true).unwrap();
        let inner = thread.begin_remote_submission(true).unwrap();
        thread
            .reject_remote_submission(outer, TransactionFailure::DeadReply)
            .unwrap();
        let mut output = [0; 4];
        assert_eq!(thread.read_local_work(&mut output), 0);
        thread
            .reject_remote_submission(inner, TransactionFailure::FailedReply)
            .unwrap();
        assert_eq!(thread.read_local_work(&mut [0; 3]), 0);
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_FAILED_REPLY);
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_DEAD_REPLY);
        assert!(!thread.has_outgoing());
        assert_eq!(thread.read_local_work(&mut output), 0);
        assert_eq!(
            thread.reject_remote_submission(inner, TransactionFailure::DeadReply),
            Err(Error::InvalidOutgoingReply)
        );

        let oneway = thread.begin_remote_submission(false).unwrap();
        thread
            .reject_remote_submission(oneway, TransactionFailure::DeadReply)
            .unwrap();
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_DEAD_REPLY);
    }

    #[test]
    fn caller_death_preserves_delivered_remote_call_for_one_late_reply() {
        let mut thread = Thread::new(10).unwrap();
        let call = CallId::remote(crate::authority_protocol::CallToken::from_nonzero(4).unwrap());
        thread.reserve_incoming().unwrap();
        thread.accept_incoming(call);

        assert!(thread.cancel_incoming(call));
        let (submission, remote) = thread.begin_remote_reply().unwrap().unwrap();
        assert_eq!(CallId::remote(remote), call);
        thread.commit_remote_reply(submission, remote).unwrap();

        let mut output = [0; 4];
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_TRANSACTION_COMPLETE);
        assert_eq!(thread.current_incoming(), Err(Error::InvalidReplyStack));
    }

    #[test]
    fn caller_death_during_submitted_reply_does_not_invalidate_ack() {
        let mut thread = Thread::new(11).unwrap();
        let call = CallId::remote(crate::authority_protocol::CallToken::from_nonzero(5).unwrap());
        thread.reserve_incoming().unwrap();
        thread.accept_incoming(call);
        let (submission, remote) = thread.begin_remote_reply().unwrap().unwrap();

        assert!(thread.cancel_incoming(call));
        thread.commit_remote_reply(submission, remote).unwrap();

        let mut output = [0; 4];
        assert_eq!(thread.read_local_work(&mut output), 4);
        assert_eq!(u32::from_le_bytes(output), BR_TRANSACTION_COMPLETE);
        assert_eq!(thread.current_incoming(), Err(Error::InvalidReplyStack));
    }
}
