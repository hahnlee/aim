//! Android Binder worker-pool accounting owned by one `binder_proc`.
//!
//! The configured maximum and REGISTER admission history are process state,
//! not properties of an individual ioctl or framework service.

use super::*;
use std::collections::HashSet;

pub(crate) const BR_SPAWN_LOOPER: u32 = 0x0000_720d;

#[derive(Debug, Default)]
pub(crate) struct Pool {
    max_threads: Option<u32>,
    requested_thread: bool,
    admitted_threads: u32,
    idle_waiters: HashSet<u64>,
}

impl Pool {
    pub(crate) fn set_max_threads(&mut self, maximum: u32) {
        self.max_threads = Some(maximum);
    }

    pub(crate) fn max_threads(&self) -> Option<u32> {
        self.max_threads
    }

    pub(crate) fn begin_register(&self) -> Result<(), crate::thread::Error> {
        self.requested_thread
            .then_some(())
            .ok_or(crate::thread::Error::UnrequestedRegistration)
    }

    pub(crate) fn finish_register(&mut self) {
        debug_assert!(self.requested_thread);
        self.requested_thread = false;
        self.admitted_threads = self.admitted_threads.saturating_add(1);
    }

    pub(crate) fn enter_wait(&mut self, thread_id: u64) -> bool {
        self.idle_waiters.insert(thread_id)
    }

    pub(crate) fn leave_wait(&mut self, thread_id: u64) {
        self.idle_waiters.remove(&thread_id);
    }

    pub(crate) fn reserve_spawn(&mut self) -> bool {
        let Some(maximum) = self.max_threads else {
            return false;
        };
        if self.requested_thread
            || !self.idle_waiters.is_empty()
            || self.admitted_threads >= maximum
        {
            return false;
        }
        self.requested_thread = true;
        true
    }
}

impl Connection {
    pub(crate) fn set_max_threads(&self, maximum: u32) -> Result<(), Error> {
        self.pool
            .lock()
            .map_err(|_| Error::Poisoned)?
            .set_max_threads(maximum);
        Ok(())
    }

    pub(crate) fn max_threads(&self) -> Result<Option<u32>, Error> {
        self.pool
            .lock()
            .map(|pool| pool.max_threads())
            .map_err(|_| Error::Poisoned)
    }

    pub(crate) fn execute_looper_command(
        &self,
        thread: &mut crate::thread::Thread,
        input: &[u8],
    ) -> Result<usize, Error> {
        let (command, _) = crate::command::decode(input)
            .map_err(|error| Error::Thread(crate::thread::Error::Framing(error)))?;
        let mut pool = self.pool.lock().map_err(|_| Error::Poisoned)?;
        if command.kind == crate::command::Kind::RegisterLooper {
            pool.begin_register().map_err(Error::Thread)?;
        }
        let bytes = thread.execute_looper(input).map_err(Error::Thread)?;
        if command.kind == crate::command::Kind::RegisterLooper {
            pool.finish_register();
        }
        if command.kind == crate::command::Kind::ExitLooper {
            pool.leave_wait(thread.id());
        }
        Ok(bytes)
    }

    pub(crate) fn enter_idle_wait(&self, thread: &crate::thread::Thread) -> Result<bool, Error> {
        if !thread.can_wait_for_process_work() {
            return Ok(false);
        }
        self.pool
            .lock()
            .map_err(|_| Error::Poisoned)
            .map(|mut pool| pool.enter_wait(thread.id()))
    }

    pub(crate) fn leave_idle_wait(&self, thread_id: u64) -> Result<(), Error> {
        self.pool
            .lock()
            .map_err(|_| Error::Poisoned)?
            .leave_wait(thread_id);
        Ok(())
    }

    pub(crate) fn read_spawn_request(
        &self,
        thread: &crate::thread::Thread,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        if output.len() < 4 || !thread.is_looper() || !self.has_untargeted_transaction()? {
            return Ok(0);
        }
        if !self
            .pool
            .lock()
            .map_err(|_| Error::Poisoned)?
            .reserve_spawn()
        {
            return Ok(0);
        }
        output[..4].copy_from_slice(&BR_SPAWN_LOOPER.to_le_bytes());
        Ok(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_register_and_lifetime_admission_follow_kernel_accounting() {
        let mut pool = Pool::default();
        assert!(!pool.reserve_spawn());
        pool.set_max_threads(0);
        assert!(!pool.reserve_spawn());
        pool.set_max_threads(2);
        assert!(pool.reserve_spawn());
        assert!(!pool.reserve_spawn());
        assert!(pool.begin_register().is_ok());
        pool.finish_register();
        assert!(pool.reserve_spawn());
        assert!(pool.begin_register().is_ok());
        pool.finish_register();
        assert!(!pool.reserve_spawn());
        assert_eq!(
            pool.begin_register(),
            Err(crate::thread::Error::UnrequestedRegistration)
        );
    }

    #[test]
    fn idle_process_waiter_suppresses_spawn_demand() {
        let mut pool = Pool::default();
        pool.set_max_threads(4);
        assert!(pool.enter_wait(7));
        assert!(!pool.enter_wait(7));
        assert!(!pool.reserve_spawn());
        pool.leave_wait(7);
        assert!(pool.reserve_spawn());
    }
}
