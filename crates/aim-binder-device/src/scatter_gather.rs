//! Owned pointed-buffer input, not parent fixups or delivery authorization.
use crate::{object_fields::Fields, objects, transaction_snapshot::TransactionSnapshot};
use std::{io, ops::Range};

#[derive(Debug)]
pub enum Error<E = std::io::Error> {
    Objects(objects::Error),
    Size,
    OutOfMemory,
    Copy(E),
}

pub struct Buffer {
    object_index: usize,
    object_offset: usize,
    destination: Range<usize>,
    bytes: Vec<u8>,
}

impl Buffer {
    pub fn object_index(&self) -> usize {
        self.object_index
    }
    pub fn object_offset(&self) -> usize {
        self.object_offset
    }
    /// Range in the complete receiver transaction allocation, excluding padding.
    pub fn destination(&self) -> Range<usize> {
        self.destination.clone()
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub struct ScatterGather<'a> {
    snapshot: &'a TransactionSnapshot,
    buffers: Vec<Buffer>,
}

impl<'a> ScatterGather<'a> {
    /// The authenticated sender memory boundary must fill each destination
    /// completely or return an error. Addresses are opaque; no host dereference.
    /// Parent pointers and FD arrays are deliberately not interpreted here.
    pub fn capture(
        snapshot: &'a TransactionSnapshot,
        copy_from_sender: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
    ) -> Result<Self, Error> {
        Self::capture_with(snapshot, copy_from_sender)
    }

    pub fn capture_with<E>(
        snapshot: &'a TransactionSnapshot,
        mut copy_from_sender: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
    ) -> Result<Self, Error<E>> {
        let objects = snapshot.objects().map_err(Error::Objects)?;
        let extra = snapshot.layout().extra();
        let mut cursor = extra.start;
        let mut buffers = Vec::new();
        // Check the entire allocation plan before copying any sender memory.
        for (index, object) in objects.iter().enumerate() {
            if let Fields::Buffer { length, .. } = object.fields() {
                let length = usize::try_from(length).map_err(|_| Error::Size)?;
                let end = cursor.checked_add(length).ok_or(Error::Size)?;
                let aligned = length.checked_add(7).ok_or(Error::Size)? & !7;
                let next = cursor.checked_add(aligned).ok_or(Error::Size)?;
                if next > extra.end {
                    return Err(Error::Size);
                }
                buffers.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
                buffers.push(Buffer {
                    object_index: index,
                    object_offset: object.offset(),
                    destination: cursor..end,
                    bytes: Vec::new(),
                });
                cursor = next;
            }
        }
        for buffer in &mut buffers {
            let length = buffer.destination.len();
            buffer
                .bytes
                .try_reserve_exact(length)
                .map_err(|_| Error::OutOfMemory)?;
            buffer.bytes.resize(length, 0);
            if length != 0 {
                let Fields::Buffer { pointer, .. } = objects[buffer.object_index].fields() else {
                    unreachable!("plan contains only buffer objects")
                };
                copy_from_sender(pointer, &mut buffer.bytes).map_err(Error::Copy)?;
            }
        }
        Ok(Self { snapshot, buffers })
    }

    pub fn snapshot(&self) -> &'a TransactionSnapshot {
        self.snapshot
    }
    pub fn buffers(&self) -> &[Buffer] {
        &self.buffers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(lengths: &[u64], extra: usize) -> TransactionSnapshot {
        let mut bytes = Vec::new();
        let mut offsets = Vec::new();
        for length in lengths {
            offsets.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&objects::Kind::Buffer.tag().to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&u64::MAX.to_le_bytes());
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&[0; 16]);
        }
        TransactionSnapshot::capture(&bytes, &offsets, extra).unwrap()
    }

    #[test]
    fn captures_owned_buffers_with_independent_alignment() {
        let input = snapshot(&[3, 0, 9], 24);
        let mut source = [7; 9];
        let mut calls = 0;
        let captured = ScatterGather::capture(&input, |address, destination| {
            assert_eq!(address, u64::MAX); // Opaque sender address, not dereferenced.
            calls += 1;
            destination.copy_from_slice(&source[..destination.len()]);
            Ok(())
        })
        .unwrap();
        source.fill(0);
        assert_eq!(calls, 2);
        assert!(std::ptr::eq(captured.snapshot(), &input));
        let start = input.layout().extra().start;
        for (index, (range, len)) in [
            (start..start + 3, 3),
            (start + 8..start + 8, 0),
            (start + 8..start + 17, 9),
        ]
        .into_iter()
        .enumerate()
        {
            let buffer = &captured.buffers()[index];
            assert_eq!(buffer.object_index(), index);
            assert_eq!(buffer.object_offset(), index * 40);
            assert_eq!(buffer.destination(), range);
            assert_eq!(buffer.bytes(), vec![7; len]);
        }
    }

    #[test]
    fn invalid_total_is_rejected_before_any_read_and_copy_error_propagates() {
        for (lengths, extra) in [(vec![1, 9], 16), (vec![u64::MAX], 8), (vec![1], 0)] {
            let input = snapshot(&lengths, extra);
            assert!(matches!(
                ScatterGather::capture(&input, |_, _| panic!("invalid plan")),
                Err(Error::Size)
            ));
        }
        let input = snapshot(&[1, 1], 16);
        let mut calls = 0;
        let result = ScatterGather::capture(&input, |_, destination| {
            calls += 1;
            destination.fill(42);
            if calls == 2 {
                Err(io::Error::from_raw_os_error(libc::EFAULT))
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(Error::Copy(_))));
        assert_eq!(calls, 2);
    }
}
