//! Process-local binder_thread access, work waiting and receive-buffer I/O.
//!
//! Global capability and transaction routing remain in the parent authority;
//! this module is the seam that an out-of-process router must not own.

use super::*;

impl Registry {
    pub(crate) fn set_max_threads(&self, key: &Key, maximum: u32) -> Result<(), Error> {
        self.lookup(key)?
            .set_max_threads(maximum)
            .map_err(Error::Connection)
    }

    pub(crate) fn max_threads(&self, key: &Key) -> Result<Option<u32>, Error> {
        self.lookup(key)?.max_threads().map_err(Error::Connection)
    }

    pub(crate) fn prepare_thread_work_wait(
        &self,
        key: &Key,
        thread_id: u64,
    ) -> Result<connection::work_wait::ThreadWorkWait, Error> {
        connection::work_wait::ThreadWorkWait::new(self.lookup(key)?, thread_id)
            .map_err(Error::Connection)
    }

    pub(crate) fn execute_looper_command(
        &self,
        key: &Key,
        thread: &mut crate::thread::Thread,
        input: &[u8],
    ) -> Result<usize, Error> {
        self.lookup(key)?
            .execute_looper_command(thread, input)
            .map_err(Error::Connection)
    }

    pub(crate) fn read_spawn_request(
        &self,
        key: &Key,
        thread: &crate::thread::Thread,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        self.lookup(key)?
            .read_spawn_request(thread, output)
            .map_err(Error::Connection)
    }

    pub(crate) fn thread(
        &self,
        key: &Key,
        thread_id: u64,
    ) -> Result<Arc<Mutex<crate::thread::Thread>>, Error> {
        self.lookup(key)?
            .thread(thread_id)
            .map_err(Error::Connection)
    }

    pub(crate) fn work_generation(&self, key: &Key) -> Result<u64, Error> {
        Ok(self.lookup(key)?.work_generation())
    }

    pub(crate) fn wait_for_work(
        &self,
        key: &Key,
        generation: u64,
        timeout: std::time::Duration,
    ) -> Result<crate::work_signal::WaitOutcome, Error> {
        Ok(self.lookup(key)?.wait_for_work(generation, timeout))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn deliver_transaction(
        &self,
        key: &Key,
        thread: &mut crate::thread::Thread,
        output: &mut [u8],
    ) -> Result<usize, DeliveryError> {
        let _gate = self
            .transaction_gate
            .lock()
            .map_err(|_| DeliveryError::Registry(Error::Poisoned))?;
        let connection = self.lookup(key).map_err(DeliveryError::Registry)?;
        let next = connection
            .next_transaction_route(thread.id())
            .map_err(|error| DeliveryError::Registry(Error::Connection(error)))?;
        if let Some(next) = next
            && let Some(call) = next.call
        {
            if next.reply {
                thread
                    .validate_outgoing(call)
                    .map_err(DeliveryError::Thread)?;
            } else {
                thread.reserve_incoming().map_err(DeliveryError::Thread)?;
            }
        }
        let delivery = connection
            .read_transaction_for_thread(thread.id(), output)
            .map_err(|error| DeliveryError::Registry(Error::Connection(error)))?;
        if let Some(delivery) = delivery
            && let Some(call) = delivery.call
        {
            if delivery.reply {
                if !delivery.completion {
                    thread
                        .finish_outgoing(call)
                        .map_err(DeliveryError::Thread)?;
                }
            } else {
                thread.accept_incoming(call);
            }
        }
        Ok(delivery.map_or(0, |delivery| delivery.bytes))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn free_transaction_buffer(&self, key: &Key, address: usize) -> Result<(), Error> {
        self.lookup(key)?
            .free_transaction_buffer(address)
            .map_err(Error::Connection)
    }

    #[cfg(target_os = "macos")]
    pub fn install_receive_mapping(&self, key: &Key, capacity: usize) -> Result<usize, Error> {
        self.lookup(key)?
            .install_receive_mapping(capacity)
            .map_err(Error::Connection)
    }

    pub fn duplicate_receive_mapping(&self, key: &Key) -> Result<std::os::fd::OwnedFd, Error> {
        self.lookup(key)?
            .duplicate_receive_mapping()
            .map_err(Error::Connection)
    }
}
