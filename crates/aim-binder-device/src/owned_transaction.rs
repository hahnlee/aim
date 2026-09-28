//! Receive allocation retained from delivery publication until BC_FREE_BUFFER,
//! transaction failure or process teardown.

use crate::{
    prepared_transaction::PreparedTransaction, receive_arena::ReceiveArena,
    transaction_layout::TransactionLayout, transaction_snapshot::TransactionPayload,
};
use std::sync::{Arc, Mutex};

pub struct OwnedTransaction {
    arena: Arc<Mutex<ReceiveArena>>,
    address: Option<usize>,
    layout: TransactionLayout,
    _files: Vec<crate::installed_fds::InstalledFd>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Poisoned,
    Arena(i32),
}

impl OwnedTransaction {
    pub fn prepare(
        arena: Arc<Mutex<ReceiveArena>>,
        snapshot: &dyn TransactionPayload,
    ) -> Result<Self, Error> {
        Self::prepare_rewritten(arena, snapshot, &[])
    }

    pub fn prepare_rewritten(
        arena: Arc<Mutex<ReceiveArena>>,
        snapshot: &dyn TransactionPayload,
        rewrites: &[crate::transaction_objects::Rewrite],
    ) -> Result<Self, Error> {
        Self::prepare_rewritten_with_fds(arena, snapshot, rewrites, Vec::new())
    }

    pub fn prepare_rewritten_with_fds(
        arena: Arc<Mutex<ReceiveArena>>,
        snapshot: &dyn TransactionPayload,
        rewrites: &[crate::transaction_objects::Rewrite],
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<Self, Error> {
        let published = {
            let mut guard = arena.lock().map_err(|_| Error::Poisoned)?;
            let mut prepared = PreparedTransaction::new(&mut guard, snapshot)
                .map_err(|error| Error::Arena(error.raw_os_error().unwrap_or(libc::EIO)))?;
            for rewrite in rewrites {
                prepared
                    .rewrite_data(rewrite.offset(), rewrite.bytes())
                    .map_err(|error| Error::Arena(error.raw_os_error().unwrap_or(libc::EIO)))?;
            }
            prepared.publish()
        };
        Ok(Self {
            arena,
            address: Some(published.address),
            layout: published.layout,
            _files: files,
        })
    }

    pub fn buffer_address(&self) -> usize {
        self.address
            .expect("live transaction owns receive allocation")
    }

    pub fn offsets_address(&self) -> usize {
        self.buffer_address() + self.layout.offsets().start
    }

    pub fn layout(&self) -> &TransactionLayout {
        &self.layout
    }

    pub fn release(mut self) -> Result<(), Error> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<(), Error> {
        let Some(address) = self.address.take() else {
            return Ok(());
        };
        self.arena
            .lock()
            .map_err(|_| Error::Poisoned)?
            .release(address)
            .map_err(|error| Error::Arena(error.raw_os_error().unwrap_or(libc::EIO)))
    }
}

impl Drop for OwnedTransaction {
    fn drop(&mut self) {
        if self.release_inner().is_err() {
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction_snapshot::TransactionSnapshot;

    #[test]
    fn published_allocation_survives_preparation_and_releases_on_owner_drop() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let arena = Arc::new(Mutex::new(ReceiveArena::new(16).unwrap()));
        let snapshot = TransactionSnapshot::capture(b"payload", &[], 0).unwrap();
        let address;
        {
            let transaction = OwnedTransaction::prepare(Arc::clone(&arena), &snapshot).unwrap();
            address = transaction.buffer_address();
            assert_eq!(
                &arena.lock().unwrap().bytes(address).unwrap()[..7],
                b"payload"
            );
            assert!(arena.lock().unwrap().allocate(16).is_err());
        }
        assert_eq!(arena.lock().unwrap().allocate(16).unwrap(), address);
    }

    #[test]
    fn explicit_release_is_exactly_once() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let arena = Arc::new(Mutex::new(ReceiveArena::new(8).unwrap()));
        let snapshot = TransactionSnapshot::capture(&[], &[], 0).unwrap();
        let transaction = OwnedTransaction::prepare(Arc::clone(&arena), &snapshot).unwrap();
        let address = transaction.buffer_address();
        transaction.release().unwrap();
        assert_eq!(arena.lock().unwrap().allocate(8).unwrap(), address);
    }
}
