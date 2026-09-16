//! Private receive-region preparation. No BR_TRANSACTION publication or
//! permission to deliver untranslated Binder objects is implied by this type.
use crate::{
    pointer_fixups::PointerFixups, receive_arena::ReceiveArena, scatter_gather::ScatterGather,
    transaction_snapshot::TransactionPayload,
};
use std::io;
pub mod fd_installation;

pub struct PreparedTransaction<'a> {
    arena: &'a mut ReceiveArena,
    address: usize,
    snapshot: &'a dyn TransactionPayload,
    release_on_drop: bool,
}

impl<'a> PreparedTransaction<'a> {
    pub fn new(
        arena: &'a mut ReceiveArena,
        snapshot: &'a dyn TransactionPayload,
    ) -> io::Result<Self> {
        let address = arena.reserve_transaction(snapshot.layout())?;
        // Install cleanup ownership before any subsequent fallible operation.
        let prepared = Self {
            arena,
            address,
            snapshot,
            release_on_drop: true,
        };
        prepared.arena.clear(address)?;
        prepared
            .arena
            .write(address, snapshot.layout().data().start, snapshot.data())?;
        prepared.arena.write(
            address,
            snapshot.layout().offsets().start,
            snapshot.offsets(),
        )?;
        Ok(prepared)
    }

    /// Populate captured SG bytes in the same private receive allocation. The
    /// snapshot comes from the capture itself, preventing cross-transaction plans.
    /// Sender pointers remain untranslated; this does not authorize publication.
    pub fn with_scatter_gather(
        arena: &'a mut ReceiveArena,
        captured: &ScatterGather<'a>,
    ) -> io::Result<Self> {
        let prepared = Self::new(arena, captured.snapshot())?;
        for buffer in captured.buffers() {
            prepared
                .arena
                .write(prepared.address, buffer.destination().start, buffer.bytes())?;
        }
        Ok(prepared)
    }

    /// Relocate validated buffer pointers into this allocation's read-only client
    /// view. This remains private preparation: FD/node translation is separate.
    pub fn with_pointer_fixups(
        arena: &'a mut ReceiveArena,
        plan: &PointerFixups<'_, 'a>,
    ) -> io::Result<Self> {
        let prepared = Self::with_scatter_gather(arena, plan.capture())?;
        for write in plan.writes() {
            let pointer = prepared
                .address
                .checked_add(write.pointee())
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
            prepared.arena.write(
                prepared.address,
                write.destination(),
                &pointer.to_le_bytes(),
            )?;
        }
        Ok(prepared)
    }

    pub fn data(&self) -> &[u8] {
        &self.storage()[self.snapshot.layout().data()]
    }
    pub fn offsets(&self) -> &[u8] {
        &self.storage()[self.snapshot.layout().offsets()]
    }
    pub(crate) fn rewrite_data(&mut self, offset: usize, bytes: &[u8]) -> io::Result<()> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        if end > self.snapshot.layout().data().len() {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        self.arena.write(
            self.address,
            self.snapshot.layout().data().start + offset,
            bytes,
        )
    }
    pub(crate) fn publish(mut self) -> PublishedTransaction {
        self.release_on_drop = false;
        PublishedTransaction {
            address: self.address,
            layout: self.snapshot.layout().clone(),
        }
    }
    fn storage(&self) -> &[u8] {
        self.arena
            .bytes(self.address)
            .expect("exclusive preparation owns live region")
    }
}

impl Drop for PreparedTransaction<'_> {
    fn drop(&mut self) {
        // No transfer-to-delivery API until object/FD fixups and reference
        // ownership exist. All current preparation paths therefore roll back.
        if self.release_on_drop {
            self.arena
                .release(self.address)
                .expect("exclusive preparation owns live region");
        }
    }
}

pub(crate) struct PublishedTransaction {
    pub(crate) address: usize,
    pub(crate) layout: crate::transaction_layout::TransactionLayout,
}

#[cfg(test)]
mod pointer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction_snapshot::TransactionSnapshot;
    #[test]
    fn sg_capture_populates_mapping_without_retaining_capture_or_sender() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut data = [0; 80];
        for (offset, length) in [(0, 3u64), (40, 9u64)] {
            data[offset..offset + 4]
                .copy_from_slice(&crate::objects::Kind::Buffer.tag().to_le_bytes());
            data[offset + 8..offset + 16].copy_from_slice(&99u64.to_le_bytes());
            data[offset + 16..offset + 24].copy_from_slice(&length.to_le_bytes());
        }
        let offsets: Vec<_> = [0u64, 40].into_iter().flat_map(u64::to_le_bytes).collect();
        let snapshot = TransactionSnapshot::capture(&data, &offsets, 32).unwrap();
        let capacity = snapshot.layout().total();
        let mut arena = ReceiveArena::new(capacity).unwrap();
        let dirty = arena.allocate(capacity).unwrap();
        arena.write(dirty, 0, &vec![0xff; capacity]).unwrap();
        arena.release(dirty).unwrap();
        {
            let capture = ScatterGather::capture(&snapshot, |_, target| {
                target.fill(7);
                Ok(())
            })
            .unwrap();
            let prepared = PreparedTransaction::with_scatter_gather(&mut arena, &capture).unwrap();
            drop(capture);
            assert_eq!(prepared.data(), snapshot.data());
            assert_eq!(prepared.offsets(), snapshot.offsets());
            let extra = &prepared.storage()[snapshot.layout().extra()];
            assert_eq!(&extra[..3], &[7; 3]);
            assert_eq!(&extra[3..8], &[0; 5]);
            assert_eq!(&extra[8..17], &[7; 9]);
            assert!(extra[17..].iter().all(|byte| *byte == 0));
        }
        assert_eq!(arena.allocate(capacity).unwrap(), dirty);
    }
    #[test]
    fn stages_owned_snapshot_and_drop_recovers_capacity() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut arena = ReceiveArena::new(48).unwrap();
        let dirty = arena.allocate(48).unwrap();
        arena.write(dirty, 0, &[0xff; 48]).unwrap();
        arena.release(dirty).unwrap();
        let mut data = vec![0; 28];
        data[4..8].copy_from_slice(&crate::objects::Kind::Fd.tag().to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&data, &4u64.to_le_bytes(), 8).unwrap();
        {
            let staged = PreparedTransaction::new(&mut arena, &snapshot).unwrap();
            assert_eq!(staged.data(), snapshot.data());
            assert_eq!(staged.offsets(), snapshot.offsets());
            assert!(staged.storage()[28..32].iter().all(|b| *b == 0));
            assert!(staged.storage()[40..48].iter().all(|b| *b == 0));
        }
        assert_eq!(arena.allocate(48).unwrap(), dirty);
    }
    #[test]
    fn error_unwind_releases_preparation_and_exhaustion_preserves_existing() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let snapshot = TransactionSnapshot::capture(b"abcdefgh", &[], 0).unwrap();
        let mut arena = ReceiveArena::new(8).unwrap();
        let aborted: io::Result<()> = (|| {
            let _staged = PreparedTransaction::new(&mut arena, &snapshot)?;
            Err(io::Error::other("later preparation failed"))
        })();
        assert!(aborted.is_err());
        let address = arena.allocate(8).unwrap();
        arena.write(address, 0, b"original").unwrap();
        assert!(PreparedTransaction::new(&mut arena, &snapshot).is_err());
        assert_eq!(arena.bytes(address).unwrap(), b"original");
    }
}
