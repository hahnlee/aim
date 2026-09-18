//! One binder_thread idle registration pinned to its original binder_proc.
//!
//! Registry removal ends admission, not this lease's cleanup identity. Neither
//! waiting nor cleanup re-resolves a registry key that may already be removed.

use super::{Connection, Error};
use crate::work_signal::WaitOutcome;
use std::{sync::Arc, time::Duration};

pub(crate) struct ThreadWorkWait {
    connection: Arc<Connection>,
    registered_thread: Option<u64>,
}

impl ThreadWorkWait {
    pub(crate) fn new(connection: Arc<Connection>, thread_id: u64) -> Result<Self, Error> {
        let thread = connection.thread(thread_id)?;
        let entered = {
            let thread = thread.lock().map_err(|_| Error::Poisoned)?;
            connection.enter_idle_wait(&thread)?
        };
        Ok(Self {
            connection,
            registered_thread: entered.then_some(thread_id),
        })
    }

    pub(crate) fn wait(&self, generation: u64, timeout: Option<Duration>) -> WaitOutcome {
        match timeout {
            Some(timeout) => self.connection.wait_for_work(generation, timeout),
            None => self.connection.wait_for_work_indefinite(generation),
        }
    }

    pub(crate) fn finish(mut self) -> Result<(), Error> {
        self.release()
    }

    fn release(&mut self) -> Result<(), Error> {
        if let Some(thread_id) = self.registered_thread.take() {
            self.connection.leave_idle_wait(thread_id)?;
        }
        Ok(())
    }
}

impl Drop for ThreadWorkWait {
    fn drop(&mut self) {
        // Explicit finish reports poisoning. Unwind still removes registration
        // where possible without looking up a potentially retired registry key.
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::Kind, connection::ConnectionOwner, connection_registry::Registry, session::Session,
    };

    fn registered() -> (Registry, crate::connection_registry::Key, Arc<Connection>) {
        let registry = Registry::default();
        let owner = ConnectionOwner::new(Session::default());
        let connection = owner.handle();
        let thread = connection.thread(42).unwrap();
        thread
            .lock()
            .unwrap()
            .execute_looper(&Kind::EnterLooper.word().to_le_bytes())
            .unwrap();
        connection.set_max_threads(1).unwrap();
        let key = registry.register(owner).unwrap();
        (registry, key, connection)
    }

    #[test]
    fn registry_removal_wakes_and_cleans_pinned_registration() {
        use std::sync::mpsc;
        let (registry, key, connection) = registered();
        let observed = connection.work_generation();
        // Exactly the same admission method used by the production Device wait.
        let registration = registry.prepare_thread_work_wait(&key, 42).unwrap();
        assert!(!connection.pool.lock().unwrap().reserve_spawn());
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let outcome = registration.wait(observed, None);
            tx.send((outcome, registration.finish())).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        registry.close(&key).unwrap(); // Real owner removal + disconnect.
        assert!(matches!(
            registry.prepare_thread_work_wait(&key, 42),
            Err(crate::connection_registry::Error::UnknownConnection)
        ));
        let (outcome, cleanup) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(outcome, WaitOutcome::Closed);
        assert_eq!(cleanup, Ok(())); // Never re-resolve the removed key.
        worker.join().unwrap();
        assert!(connection.pool.lock().unwrap().reserve_spawn());
    }

    #[test]
    fn dropped_registration_cleans_pool_on_unwind() {
        let (registry, key, connection) = registered();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _registration = registry.prepare_thread_work_wait(&key, 42).unwrap();
            assert!(!connection.pool.lock().unwrap().reserve_spawn());
            panic!("synthetic waiter unwind");
        }));
        assert!(result.is_err());
        assert!(connection.pool.lock().unwrap().reserve_spawn());
        registry.close(&key).unwrap();
    }
}
