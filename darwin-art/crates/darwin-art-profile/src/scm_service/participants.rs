//! Bounded exact-birth participants, independent of connection lifetimes.
use super::failed;
use crate::{ProfileError, process_incarnation::ProcessIncarnation};
use darwin_art_scm_transfer::{MAX_GLOBAL_LEASES, ProcessEpoch};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) struct Participant {
    pub epoch: ProcessEpoch,
    pub birth: ProcessIncarnation,
}

#[derive(Default)]
pub(super) struct Participants(HashMap<ProcessEpoch, ProcessIncarnation>);

impl Participants {
    pub fn track(
        &mut self,
        epoch: ProcessEpoch,
        birth: ProcessIncarnation,
    ) -> Result<(), ProfileError> {
        if let Some(existing) = self.0.get(&epoch) {
            if *existing != birth {
                return Err(failed("participant birth mismatch"));
            }
            return Ok(());
        }
        if self.0.len() >= MAX_GLOBAL_LEASES {
            return Err(failed("participant quota exceeded"));
        }
        self.0.try_reserve(1).map_err(failed)?;
        self.0.insert(epoch, birth);
        Ok(())
    }

    pub fn remove(&mut self, epoch: ProcessEpoch) {
        self.0.remove(&epoch);
    }

    pub fn snapshot(&self) -> [Option<Participant>; MAX_GLOBAL_LEASES] {
        let mut result = [None; MAX_GLOBAL_LEASES];
        for (slot, (epoch, birth)) in result.iter_mut().zip(&self.0) {
            *slot = Some(Participant {
                epoch: *epoch,
                birth: *birth,
            });
        }
        result
    }
}
