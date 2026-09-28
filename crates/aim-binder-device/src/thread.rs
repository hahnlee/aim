//! State owned by one Binder-calling host thread (`binder_thread` scope).
//! Process objects, handles and receive mappings remain in `Session`.

use crate::command::{self, Kind};
use std::{collections::VecDeque, sync::Arc};
mod transactions;
pub(crate) use transactions::SubmissionReservation;
pub use transactions::{BR_DEAD_REPLY, BR_FAILED_REPLY, BR_TRANSACTION_COMPLETE};
pub use transactions::{CallId, RemoteReplySubmission, RemoteSubmission};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Looper {
    #[default]
    None,
    Registered,
    Entered,
    Exited,
}

#[derive(Debug, Default)]
pub struct Thread {
    looper: Looper,
    id: u64,
    outgoing: Vec<transactions::OutgoingCall>,
    incoming: Vec<transactions::IncomingCall>,
    local_work: VecDeque<transactions::LocalWork>,
    next_remote_submission: u64,
    work_signal: Option<Arc<crate::work_signal::Signal>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Framing(command::DecodeError),
    NotLooperCommand(Kind),
    InvalidTransition { from: &'static str, command: Kind },
    UnrequestedRegistration,
    NotEligibleForRead,
    OutOfMemory,
    InvalidReplyStack,
    InvalidOutgoingReply,
    SubmissionIdsExhausted,
}

impl Thread {
    pub fn new(id: u64) -> Option<Self> {
        (id != 0).then_some(Self {
            id,
            ..Self::default()
        })
    }

    pub(crate) fn new_with_signal(
        id: u64,
        work_signal: Arc<crate::work_signal::Signal>,
    ) -> Option<Self> {
        let mut thread = Self::new(id)?;
        thread.work_signal = Some(work_signal);
        Some(thread)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn execute_looper(&mut self, input: &[u8]) -> Result<usize, Error> {
        let (command, bytes) = command::decode(input).map_err(Error::Framing)?;
        let next = match (self.looper, command.kind) {
            (Looper::None, Kind::RegisterLooper) => Looper::Registered,
            (Looper::None, Kind::EnterLooper) => Looper::Entered,
            (Looper::Registered | Looper::Entered, Kind::ExitLooper) => Looper::Exited,
            (_, Kind::RegisterLooper | Kind::EnterLooper | Kind::ExitLooper) => {
                return Err(Error::InvalidTransition {
                    from: self.state_name(),
                    command: command.kind,
                });
            }
            (_, kind) => return Err(Error::NotLooperCommand(kind)),
        };
        self.looper = next;
        Ok(bytes)
    }

    pub fn ensure_read_eligible(&self) -> Result<(), Error> {
        match self.looper {
            Looper::Registered | Looper::Entered => Ok(()),
            Looper::None | Looper::Exited
                if self.outgoing.is_empty() && self.local_work.is_empty() =>
            {
                Err(Error::NotEligibleForRead)
            }
            Looper::None | Looper::Exited => Ok(()),
        }
    }

    pub(crate) fn is_looper(&self) -> bool {
        matches!(self.looper, Looper::Registered | Looper::Entered)
    }

    pub(crate) fn can_wait_for_process_work(&self) -> bool {
        self.is_looper()
            && self.outgoing.is_empty()
            && self.incoming.is_empty()
            && self.local_work.is_empty()
    }

    fn state_name(&self) -> &'static str {
        match self.looper {
            Looper::None => "none",
            Looper::Registered => "registered",
            Looper::Entered => "entered",
            Looper::Exited => "exited",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: Kind) -> [u8; 4] {
        kind.word().to_le_bytes()
    }

    #[test]
    fn looper_registration_is_thread_local_and_transition_checked() {
        let mut first = Thread::default();
        let second = Thread::default();
        assert_eq!(first.ensure_read_eligible(), Err(Error::NotEligibleForRead));
        assert_eq!(first.execute_looper(&record(Kind::EnterLooper)), Ok(4));
        assert!(first.ensure_read_eligible().is_ok());
        assert_eq!(
            second.ensure_read_eligible(),
            Err(Error::NotEligibleForRead)
        );
        assert!(matches!(
            first.execute_looper(&record(Kind::RegisterLooper)),
            Err(Error::InvalidTransition { .. })
        ));
        assert_eq!(first.execute_looper(&record(Kind::ExitLooper)), Ok(4));
        assert_eq!(first.ensure_read_eligible(), Err(Error::NotEligibleForRead));
    }
}
