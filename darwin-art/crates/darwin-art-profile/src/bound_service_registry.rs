//! Daemon ownership and process-incarnation handoff for Android bound services.
//!
//! Binder death is observable before macOS has completed `waitpid`.  Android's
//! system process may therefore request the replacement incarnation while the
//! daemon still owns the exiting child.  This registry makes that ordering an
//! explicit host-boundary contract instead of leaking an `already active`
//! error back through `bindServiceInstance`.

use crate::ProfileError;
use crate::bound_service_child_control::BoundServiceChildControl;
use crate::bound_service_process::{BoundServiceIdentity, BoundServiceProcessResponse};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) const RESTART_REAP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct BoundServiceRecord {
    pub(crate) start_sequence: u64,
    pub(crate) pid: u32,
    pub(crate) incarnation: [u64; 2],
    pub(crate) activation_token: [u8; 16],
    pub(crate) control: Arc<BoundServiceChildControl>,
}

impl BoundServiceRecord {
    pub(crate) fn new(
        response: BoundServiceProcessResponse,
        control: Arc<BoundServiceChildControl>,
    ) -> Self {
        Self {
            start_sequence: response.start_sequence,
            pid: response.pid,
            incarnation: response.incarnation,
            activation_token: response.activation_token,
            control,
        }
    }

    fn response(&self) -> BoundServiceProcessResponse {
        BoundServiceProcessResponse {
            pid: self.pid,
            start_sequence: self.start_sequence,
            incarnation: self.incarnation,
            activation_token: self.activation_token,
        }
    }
}

pub(crate) enum BoundServiceSlot {
    Existing(BoundServiceProcessResponse),
    Vacant,
}

#[derive(Default)]
pub(crate) struct BoundServiceRegistry {
    records: Mutex<BTreeMap<BoundServiceIdentity, BoundServiceRecord>>,
    changed: Condvar,
}

impl BoundServiceRegistry {
    pub(crate) fn await_slot(
        &self,
        identity: &BoundServiceIdentity,
        start_sequence: u64,
        timeout: Duration,
    ) -> Result<BoundServiceSlot, ProfileError> {
        let deadline = Instant::now() + timeout;
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        loop {
            match records.get(identity) {
                None => return Ok(BoundServiceSlot::Vacant),
                Some(record) if record.start_sequence == start_sequence => {
                    return Ok(BoundServiceSlot::Existing(record.response()));
                }
                Some(_) => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ProfileError::Daemon(
                    "timed out waiting for the previous bound-service process incarnation to exit"
                        .into(),
                ));
            }
            let (next, result) = self
                .changed
                .wait_timeout(records, remaining)
                .map_err(|_| poisoned())?;
            records = next;
            if result.timed_out()
                && records
                    .get(identity)
                    .is_some_and(|record| record.start_sequence != start_sequence)
            {
                return Err(ProfileError::Daemon(
                    "timed out waiting for the previous bound-service process incarnation to exit"
                        .into(),
                ));
            }
        }
    }

    pub(crate) fn insert(
        &self,
        identity: BoundServiceIdentity,
        record: BoundServiceRecord,
    ) -> Result<(), ProfileError> {
        self.records
            .lock()
            .map_err(|_| poisoned())?
            .insert(identity, record);
        Ok(())
    }

    pub(crate) fn remove_incarnation(
        &self,
        identity: &BoundServiceIdentity,
        pid: u32,
        start_sequence: u64,
        incarnation: [u64; 2],
    ) -> Result<bool, ProfileError> {
        let removed = {
            let mut records = self.records.lock().map_err(|_| poisoned())?;
            if records.get(identity).is_some_and(|record| {
                record.pid == pid
                    && record.start_sequence == start_sequence
                    && record.incarnation == incarnation
            }) {
                records.remove(identity);
                true
            } else {
                false
            }
        };
        if removed {
            self.changed.notify_all();
        }
        Ok(removed)
    }

    pub(crate) fn control(
        &self,
        handle: BoundServiceProcessResponse,
    ) -> Result<Arc<BoundServiceChildControl>, ProfileError> {
        self.records
            .lock()
            .map_err(|_| poisoned())?
            .values()
            .find(|record| {
                record.pid == handle.pid
                    && record.start_sequence == handle.start_sequence
                    && record.incarnation == handle.incarnation
                    && record.activation_token == handle.activation_token
            })
            .map(|record| Arc::clone(&record.control))
            .ok_or_else(|| ProfileError::Daemon("unknown or stale bound-service handle".into()))
    }
}

fn poisoned() -> ProfileError {
    ProfileError::Daemon("bound-service registry is poisoned".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound_service_process::BoundServiceProcessRequest;
    use std::thread;

    fn identity() -> BoundServiceIdentity {
        BoundServiceProcessRequest {
            package: "org.example.app".into(),
            process_name: "org.example.app:renderer".into(),
            uid: 99_001,
            isolated: true,
            start_sequence: 7,
        }
        .identity()
    }

    fn response(sequence: u64) -> BoundServiceProcessResponse {
        BoundServiceProcessResponse {
            pid: 42,
            start_sequence: sequence,
            incarnation: [3, 4],
            activation_token: [5; 16],
        }
    }

    #[test]
    fn replacement_waits_for_exact_previous_incarnation_reap() {
        let registry = Arc::new(BoundServiceRegistry::default());
        let key = identity();
        let old = response(7);
        registry
            .insert(
                key.clone(),
                BoundServiceRecord::new(old, Arc::new(BoundServiceChildControl::new(None))),
            )
            .unwrap();
        let waiting = Arc::clone(&registry);
        let waiting_key = key.clone();
        let waiter =
            thread::spawn(move || waiting.await_slot(&waiting_key, 8, Duration::from_secs(1)));
        thread::sleep(Duration::from_millis(10));
        assert!(
            registry
                .remove_incarnation(&key, old.pid, old.start_sequence, old.incarnation)
                .unwrap()
        );
        assert!(matches!(
            waiter.join().unwrap().unwrap(),
            BoundServiceSlot::Vacant
        ));
    }

    #[test]
    fn retry_of_same_start_sequence_is_idempotent() {
        let registry = BoundServiceRegistry::default();
        let key = identity();
        let existing = response(7);
        registry
            .insert(
                key.clone(),
                BoundServiceRecord::new(existing, Arc::new(BoundServiceChildControl::new(None))),
            )
            .unwrap();
        match registry
            .await_slot(&key, existing.start_sequence, Duration::ZERO)
            .unwrap()
        {
            BoundServiceSlot::Existing(actual) => assert_eq!(actual, existing),
            BoundServiceSlot::Vacant => panic!("existing incarnation was lost"),
        }
    }

    #[test]
    fn control_lookup_requires_the_exact_incarnation_handle() {
        let registry = BoundServiceRegistry::default();
        let key = identity();
        let existing = response(7);
        registry
            .insert(
                key,
                BoundServiceRecord::new(existing, Arc::new(BoundServiceChildControl::new(None))),
            )
            .unwrap();
        assert!(registry.control(existing).is_ok());
        let mut stale = existing;
        stale.activation_token[0] ^= 1;
        assert!(registry.control(stale).is_err());
    }
}
