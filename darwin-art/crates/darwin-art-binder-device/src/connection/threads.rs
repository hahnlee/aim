//! `binder_proc` ownership of its `binder_thread` table.
//!
//! Device opens authenticate process identity, while the connection owns every
//! thread participating in that process. This lets transaction teardown route
//! driver return work to an exact surviving caller without leaking host-thread
//! handles into the global registry.

use super::*;

impl Connection {
    pub(crate) fn fail_remote_authority(&self) -> Result<(), Error> {
        let threads = self.threads.lock().map_err(|_| Error::Poisoned)?;
        for thread in threads.values() {
            thread
                .lock()
                .map_err(|_| Error::Poisoned)?
                .fail_remote_authority()
                .map_err(Error::Thread)?;
        }
        Ok(())
    }

    pub(crate) fn work_generation(&self) -> u64 {
        self.work_signal.snapshot()
    }

    pub(crate) fn wait_for_work(
        &self,
        generation: u64,
        timeout: std::time::Duration,
    ) -> crate::work_signal::WaitOutcome {
        self.work_signal.wait_after(generation, timeout)
    }

    pub(crate) fn thread(&self, id: u64) -> Result<Arc<Mutex<crate::thread::Thread>>, Error> {
        if id == 0 {
            return Err(Error::Io(libc::EINVAL));
        }
        let mut threads = self.threads.lock().map_err(|_| Error::Poisoned)?;
        Ok(Arc::clone(threads.entry(id).or_insert_with(|| {
            Arc::new(Mutex::new(
                crate::thread::Thread::new_with_signal(id, Arc::clone(&self.work_signal))
                    .expect("nonzero thread id checked"),
            ))
        })))
    }

    pub(crate) fn existing_thread(
        &self,
        id: u64,
    ) -> Result<Option<Arc<Mutex<crate::thread::Thread>>>, Error> {
        let threads = self.threads.lock().map_err(|_| Error::Poisoned)?;
        Ok(threads.get(&id).map(Arc::clone))
    }
}
