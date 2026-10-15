//! The per-process receive-buffer allocator (`binder_alloc.c`): best-fit
//! over the mmap'ed range, half of it reserved for one-way transactions, and
//! the one-way spam heuristic.

use std::collections::BTreeMap;

use crate::host::{Errno, errno};
use crate::state::{BufferRecord, NodeId, TxnId};

/// Linux caps a binder mapping at 4 MiB.
pub const MAX_MAPPING: usize = 4 * 1024 * 1024;

pub(crate) fn align8(value: u64) -> Option<u64> {
    value.checked_add(7).map(|v| v & !7)
}

/// One allocated transaction buffer. Offsets are relative to the mapping.
pub(crate) struct Buffer {
    pub size: usize,
    pub data_size: usize,
    pub offsets_size: usize,
    pub txn: Option<TxnId>,
    pub delivery: Option<(crate::state::Tid, TxnId)>,
    pub target_node: Option<NodeId>,
    pub is_async: bool,
    pub allow_user_free: bool,
    pub clear_on_free: bool,
    pub sender_pid: i32,
    /// Objects translated into this buffer, released when it is freed.
    pub objects: Vec<BufferRecord>,
    /// Fds installed from fd arrays, closed by the owner when it is freed.
    pub fda_fds: Vec<u32>,
}

pub(crate) struct Allocator {
    pub vm_start: u64,
    pub size: usize,
    free: BTreeMap<usize, usize>,
    pub buffers: BTreeMap<usize, Buffer>,
    free_async_space: usize,
    oneway_spam_detected: bool,
}

pub(crate) struct Allocation {
    pub offset: usize,
    pub oneway_spam_suspect: bool,
}

impl Allocator {
    pub fn new(vm_start: u64, size: usize) -> Self {
        let size = size.min(MAX_MAPPING);
        Self {
            vm_start,
            size,
            free: BTreeMap::from([(0, size)]),
            buffers: BTreeMap::new(),
            free_async_space: size / 2,
            oneway_spam_detected: false,
        }
    }

    /// `binder_alloc_new_buf`.
    pub fn allocate(
        &mut self,
        data_size: u64,
        offsets_size: u64,
        extra_size: u64,
        is_async: bool,
        sender_pid: i32,
    ) -> Result<Allocation, Errno> {
        let total = align8(data_size)
            .zip(align8(offsets_size))
            .and_then(|(a, b)| a.checked_add(b))
            .zip(align8(extra_size))
            .and_then(|(a, b)| a.checked_add(b))
            .ok_or(errno::EINVAL)?;
        let size = usize::try_from(total).map_err(|_| errno::EINVAL)?.max(8);
        if is_async && self.free_async_space < size {
            return Err(errno::ENOSPC);
        }
        let (&offset, &len) = self
            .free
            .iter()
            .filter(|(_, len)| **len >= size)
            .min_by_key(|(offset, len)| (**len, **offset))
            .ok_or(errno::ENOSPC)?;
        self.free.remove(&offset);
        if len > size {
            self.free.insert(offset + size, len - size);
        }
        let mut oneway_spam_suspect = false;
        if is_async {
            self.free_async_space -= size;
            oneway_spam_suspect = self.low_async_space(sender_pid);
        }
        self.buffers.insert(
            offset,
            Buffer {
                size,
                data_size: data_size as usize,
                offsets_size: offsets_size as usize,
                txn: None,
                delivery: None,
                target_node: None,
                is_async,
                allow_user_free: false,
                clear_on_free: false,
                sender_pid,
                objects: Vec::new(),
                fda_fds: Vec::new(),
            },
        );
        Ok(Allocation {
            offset,
            oneway_spam_suspect,
        })
    }

    /// `debug_low_async_space_locked`: flag a sender that holds too much of
    /// the target's async space, once per episode.
    fn low_async_space(&mut self, sender_pid: i32) -> bool {
        if self.free_async_space >= self.size / 10 {
            self.oneway_spam_detected = false;
            return false;
        }
        let (count, total) = self
            .buffers
            .values()
            .filter(|b| b.is_async && b.sender_pid == sender_pid)
            .fold((0usize, 0usize), |(n, t), b| (n + 1, t + b.size));
        // The buffer being allocated is not inserted yet; count it too.
        if (count + 1 > 50 || total > self.size / 4) && !self.oneway_spam_detected {
            self.oneway_spam_detected = true;
            return true;
        }
        false
    }

    /// Resolve a user pointer passed to `BC_FREE_BUFFER`.
    pub fn offset_of(&self, user_ptr: u64) -> Option<usize> {
        let offset = user_ptr.checked_sub(self.vm_start)?;
        let offset = usize::try_from(offset).ok()?;
        self.buffers.contains_key(&offset).then_some(offset)
    }

    pub fn user_address(&self, offset: usize) -> u64 {
        self.vm_start + offset as u64
    }

    /// Return a buffer to the free list, coalescing neighbours.
    pub fn free(&mut self, offset: usize) -> Option<Buffer> {
        let buffer = self.buffers.remove(&offset)?;
        if buffer.is_async {
            self.free_async_space += buffer.size;
        }
        let mut start = offset;
        let mut len = buffer.size;
        if let Some((&prev, &prev_len)) = self.free.range(..offset).next_back()
            && prev + prev_len == offset
        {
            self.free.remove(&prev);
            start = prev;
            len += prev_len;
        }
        if let Some(next_len) = self.free.remove(&(offset + buffer.size)) {
            len += next_len;
        }
        self.free.insert(start, len);
        Some(buffer)
    }

    #[cfg(test)]
    pub fn free_bytes(&self) -> usize {
        self.free.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_fit_coalesces_and_reserves_async_half() {
        let mut alloc = Allocator::new(0x10_0000, 4096);
        let a = alloc.allocate(100, 8, 0, false, 1).unwrap().offset;
        let b = alloc.allocate(10, 0, 0, false, 1).unwrap().offset;
        assert_eq!(a, 0);
        assert_eq!(b, 112);
        assert_eq!(alloc.offset_of(0x10_0000 + 112), Some(112));
        assert_eq!(alloc.offset_of(0x10_0000 + 113), None);
        alloc.free(a);
        alloc.free(b);
        assert_eq!(alloc.free_bytes(), 4096);
        // Async allocations may use only half of the mapping.
        assert!(alloc.allocate(2048, 0, 0, true, 1).is_ok());
        assert_eq!(alloc.allocate(8, 0, 0, true, 1).err(), Some(errno::ENOSPC));
        assert!(alloc.allocate(8, 0, 0, false, 1).is_ok());
        assert_eq!(
            alloc.allocate(u64::MAX, 0, 0, false, 1).err(),
            Some(errno::EINVAL)
        );
    }
}
