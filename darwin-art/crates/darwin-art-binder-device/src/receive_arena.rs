//! Receive-memory region ownership. No Binder node/FD translation or dispatch.
use crate::mapping::ReceiveMapping;
use crate::transaction_layout::TransactionLayout;
use std::os::fd::OwnedFd;
use std::{collections::BTreeMap, io};

pub struct ReceiveArena {
    mapping: ReceiveMapping,
    free: BTreeMap<usize, usize>,
    allocated: BTreeMap<usize, usize>,
}

impl ReceiveArena {
    pub fn client_address(&self) -> usize {
        self.mapping.client_address()
    }

    pub fn duplicate_backing(&self) -> io::Result<OwnedFd> {
        self.mapping.duplicate_backing()
    }

    pub(crate) fn clear(&mut self, address: usize) -> io::Result<()> {
        let start = address
            .checked_sub(self.mapping.client_address())
            .ok_or_else(invalid)?;
        let &length = self.allocated.get(&start).ok_or_else(invalid)?;
        self.mapping.clear(start, length)
    }
    /// Reserves storage only. Caller must populate/translate objects and obtain
    /// delivery authorization before publishing the resulting client pointers.
    pub fn reserve_transaction(&mut self, layout: &TransactionLayout) -> io::Result<usize> {
        self.allocate(layout.total())
    }
    pub fn new(capacity: usize) -> io::Result<Self> {
        if capacity == 0 || capacity % 8 != 0 {
            return Err(invalid());
        }
        Ok(Self {
            mapping: ReceiveMapping::new(capacity)?,
            free: BTreeMap::from([(0, capacity)]),
            allocated: BTreeMap::new(),
        })
    }

    /// Reserve a non-overlapping pointer-aligned region in the client mapping.
    /// Empty transactions still need a unique buffer address until release.
    pub fn allocate(&mut self, length: usize) -> io::Result<usize> {
        let length = length.max(1).checked_add(7).ok_or_else(invalid)? & !7;
        let (&offset, &available) = self
            .free
            .iter()
            .filter(|(_, size)| **size >= length)
            .min_by_key(|(offset, size)| (**size, **offset))
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOSPC))?;
        self.free.remove(&offset);
        if available > length {
            self.free.insert(offset + length, available - length);
        }
        self.allocated.insert(offset, length);
        Ok(self.mapping.client_address() + offset)
    }

    pub fn write(&mut self, address: usize, offset: usize, data: &[u8]) -> io::Result<()> {
        let start = address
            .checked_sub(self.mapping.client_address())
            .ok_or_else(invalid)?;
        let &length = self.allocated.get(&start).ok_or_else(invalid)?;
        if offset
            .checked_add(data.len())
            .is_none_or(|end| end > length)
        {
            return Err(invalid());
        }
        self.mapping.write(start + offset, data)
    }

    pub fn bytes(&self, address: usize) -> io::Result<&[u8]> {
        let start = address
            .checked_sub(self.mapping.client_address())
            .ok_or_else(invalid)?;
        let &length = self.allocated.get(&start).ok_or_else(invalid)?;
        Ok(&self.mapping.bytes()[start..start + length])
    }

    /// Only exact live allocation starts can be released. Transaction execution
    /// must separately authorize BC_FREE_BUFFER and release object/FD references.
    pub fn release(&mut self, address: usize) -> io::Result<()> {
        let mut offset = address
            .checked_sub(self.mapping.client_address())
            .ok_or_else(invalid)?;
        let mut length = self.allocated.remove(&offset).ok_or_else(invalid)?;
        if let Some((&previous, &size)) = self.free.range(..offset).next_back()
            && previous + size == offset
        {
            self.free.remove(&previous);
            offset = previous;
            length += size;
        }
        if let Some(size) = self.free.remove(&(offset + length)) {
            length += size;
        }
        self.free.insert(offset, length);
        Ok(())
    }
}

fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transaction_sections_share_one_reserved_region() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut arena = ReceiveArena::new(32).unwrap();
        let layout = TransactionLayout::new(9, 8, 3).unwrap();
        let address = arena.reserve_transaction(&layout).unwrap();
        arena
            .write(address, layout.data().start, b"123456789")
            .unwrap();
        arena
            .write(address, layout.offsets().start, &0u64.to_le_bytes())
            .unwrap();
        arena.write(address, layout.extra().start, b"xyz").unwrap();
        let bytes = arena.bytes(address).unwrap();
        assert_eq!(&bytes[layout.data()], b"123456789");
        assert_eq!(&bytes[layout.offsets()], &0u64.to_le_bytes());
        assert_eq!(&bytes[layout.extra()], b"xyz");
        assert!(arena.allocate(8).is_err());
        arena.release(address).unwrap();
        assert_eq!(arena.allocate(32).unwrap(), address);
    }
    #[test]
    fn live_regions_are_disjoint_and_writes_are_bounded() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut arena = ReceiveArena::new(64).unwrap();
        let a = arena.allocate(9).unwrap();
        let b = arena.allocate(16).unwrap();
        assert_eq!(b - a, 16);
        arena.write(a, 0, b"first").unwrap();
        arena.write(b, 0, b"second").unwrap();
        assert!(arena.write(a, 15, b"XX").is_err());
        assert!(arena.write(a, usize::MAX, b"x").is_err());
        assert_eq!(&arena.bytes(a).unwrap()[..5], b"first");
        assert_eq!(&arena.bytes(b).unwrap()[..6], b"second");
        assert!(arena.release(a + 1).is_err());
        assert!(arena.release(0).is_err());
        assert_eq!(&arena.bytes(b).unwrap()[..6], b"second");
    }
    #[test]
    fn exhaustion_release_and_both_neighbor_coalescing() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut arena = ReceiveArena::new(32).unwrap();
        let a = arena.allocate(8).unwrap();
        let b = arena.allocate(8).unwrap();
        let c = arena.allocate(16).unwrap();
        assert_eq!(
            arena.allocate(1).unwrap_err().raw_os_error(),
            Some(libc::ENOSPC)
        );
        arena.release(a).unwrap();
        arena.release(c).unwrap();
        arena.release(b).unwrap();
        assert!(arena.release(b).is_err());
        assert_eq!(arena.allocate(32).unwrap(), a);
    }
    #[test]
    fn empty_buffers_unique_and_overflow_does_not_change_state() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut arena = ReceiveArena::new(16).unwrap();
        assert!(arena.allocate(usize::MAX).is_err());
        let a = arena.allocate(0).unwrap();
        let b = arena.allocate(0).unwrap();
        assert_ne!(a, b);
        assert!(arena.allocate(0).is_err());
    }
}
