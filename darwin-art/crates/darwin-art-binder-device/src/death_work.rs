//! Process-scoped Binder death-notification work.
//!
//! Registrations are owned by the profile routing authority because the target
//! process may be remote. This queue owns only the receiving binder_proc's BR
//! publication and BC_DEAD_BINDER_DONE acknowledgement state.

use std::collections::{HashSet, VecDeque};

// These are Linux Binder UAPI ioctl encodings. libbinder compiles its protocol
// enum with Linux _IOR even on Darwin; using the host _IOR direction bit makes
// IPCThreadState reject the otherwise well-formed return command.
pub const BR_DEAD_BINDER: u32 = 0x8008_720f;
pub const BR_CLEAR_DEATH_NOTIFICATION_DONE: u32 = 0x8008_7210;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    Dead(u64),
    ClearDone(u64),
}

#[derive(Debug, Default)]
pub struct Queue {
    pending: VecDeque<Work>,
    awaiting_dead_done: HashSet<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    DuplicateCookie,
    UnknownCookie,
    OutOfMemory,
}

impl Queue {
    pub fn enqueue_dead(&mut self, cookie: u64) -> Result<(), Error> {
        if self.awaiting_dead_done.contains(&cookie)
            || self.pending.iter().any(|work| *work == Work::Dead(cookie))
        {
            return Err(Error::DuplicateCookie);
        }
        self.pending
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.awaiting_dead_done
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.pending.push_back(Work::Dead(cookie));
        Ok(())
    }

    pub fn enqueue_clear_done(&mut self, cookie: u64) -> Result<(), Error> {
        self.pending
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.pending.push_back(Work::ClearDone(cookie));
        Ok(())
    }

    pub fn acknowledge_dead(&mut self, cookie: u64) -> Result<(), Error> {
        self.awaiting_dead_done
            .remove(&cookie)
            .then_some(())
            .ok_or(Error::UnknownCookie)
    }

    pub fn read(&mut self, output: &mut [u8]) -> usize {
        if output.len() < 12 {
            return 0;
        }
        let Some(work) = self.pending.pop_front() else {
            return 0;
        };
        let (command, cookie) = match work {
            Work::Dead(cookie) => {
                let inserted = self.awaiting_dead_done.insert(cookie);
                debug_assert!(inserted, "death cookie was reserved before publication");
                (BR_DEAD_BINDER, cookie)
            }
            Work::ClearDone(cookie) => (BR_CLEAR_DEATH_NOTIFICATION_DONE, cookie),
        };
        output[..4].copy_from_slice(&command.to_le_bytes());
        output[4..12].copy_from_slice(&cookie.to_le_bytes());
        12
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_requires_done_and_clear_done_is_independent() {
        let mut queue = Queue::default();
        queue.enqueue_dead(7).unwrap();
        assert_eq!(queue.read(&mut [0; 11]), 0);
        let mut output = [0; 12];
        assert_eq!(queue.read(&mut output), 12);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            BR_DEAD_BINDER
        );
        assert_eq!(u64::from_le_bytes(output[4..].try_into().unwrap()), 7);
        assert_eq!(queue.enqueue_dead(7), Err(Error::DuplicateCookie));
        queue.acknowledge_dead(7).unwrap();
        queue.enqueue_clear_done(9).unwrap();
        assert_eq!(queue.read(&mut output), 12);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            BR_CLEAR_DEATH_NOTIFICATION_DONE
        );
    }
}
