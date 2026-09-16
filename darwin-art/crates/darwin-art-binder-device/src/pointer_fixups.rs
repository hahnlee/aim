//! Validated receive-relative pointer writes. No receiver address is published.
use crate::{object_fields::Fields, objects, scatter_gather::ScatterGather};
mod order;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Objects(objects::Error),
    Parent,
    Order,
    Bounds,
    Alignment,
    OutOfMemory,
}

pub struct PointerWrite {
    destination: usize,
    pointee: usize,
}
impl PointerWrite {
    pub fn destination(&self) -> usize {
        self.destination
    }
    pub fn pointee(&self) -> usize {
        self.pointee
    }
}

pub struct FdSlot {
    object_index: usize,
    destination: usize,
    sender_fd: u32,
}
impl FdSlot {
    pub fn object_index(&self) -> usize {
        self.object_index
    }
    pub fn destination(&self) -> usize {
        self.destination
    }
    pub fn sender_fd(&self) -> u32 {
        self.sender_fd
    }
}

pub struct PointerFixups<'s, 'a> {
    capture: &'s ScatterGather<'a>,
    writes: Vec<PointerWrite>,
    fd_slots: Vec<FdSlot>,
}
impl<'s, 'a> PointerFixups<'s, 'a> {
    pub fn plan(capture: &'s ScatterGather<'a>) -> Result<Self, Error> {
        let objects = capture.snapshot().objects().map_err(Error::Objects)?;
        let mut writes = Vec::new();
        let mut fd_slots = Vec::new();
        let mut last: Option<usize> = None;
        let mut last_minimum = 0;
        for (index, object) in objects.iter().enumerate() {
            if let Fields::FdArray {
                count,
                parent,
                parent_offset,
            } = object.fields()
            {
                let parent = usize::try_from(parent).map_err(|_| Error::Parent)?;
                if parent >= index {
                    return Err(Error::Parent);
                }
                let parent_buffer = capture
                    .buffers()
                    .iter()
                    .find(|b| b.object_index() == parent)
                    .ok_or(Error::Parent)?;
                order::validate(&objects, last, last_minimum, parent, parent_offset)?;
                if count != 0 {
                    let count = usize::try_from(count).map_err(|_| Error::Bounds)?;
                    if count >= usize::MAX / 4 {
                        return Err(Error::Bounds);
                    }
                    let offset = usize::try_from(parent_offset).map_err(|_| Error::Bounds)?;
                    let end = offset.checked_add(count * 4).ok_or(Error::Bounds)?;
                    let bytes = parent_buffer
                        .bytes()
                        .get(offset..end)
                        .ok_or(Error::Bounds)?;
                    let destination = parent_buffer
                        .destination()
                        .start
                        .checked_add(offset)
                        .ok_or(Error::Bounds)?;
                    let Fields::Buffer { pointer, .. } = objects[parent].fields() else {
                        return Err(Error::Parent);
                    };
                    let sender_address = pointer.checked_add(parent_offset).ok_or(Error::Bounds)?;
                    if destination % 4 != 0 || sender_address % 4 != 0 {
                        return Err(Error::Alignment);
                    }
                    fd_slots
                        .try_reserve(count)
                        .map_err(|_| Error::OutOfMemory)?;
                    for (n, bytes) in bytes.chunks_exact(4).enumerate() {
                        fd_slots.push(FdSlot {
                            object_index: index,
                            destination: destination + n * 4,
                            sender_fd: u32::from_le_bytes(bytes.try_into().unwrap()),
                        });
                    }
                }
                // Even empty arrays update ordering, while original translation
                // skips their range/alignment validation and FD acquisition.
                last = Some(parent);
                last_minimum = parent_offset
                    .checked_add(count.checked_mul(4).ok_or(Error::Bounds)?)
                    .ok_or(Error::Bounds)?;
                continue;
            }
            if object.kind() != objects::Kind::Buffer {
                continue;
            }
            let buffer = capture
                .buffers()
                .iter()
                .find(|b| b.object_index() == index)
                .ok_or(Error::Parent)?;
            let Fields::Buffer {
                flags,
                parent,
                parent_offset,
                ..
            } = objects[index].fields()
            else {
                unreachable!("capture contains validated buffer objects")
            };
            if flags & 1 != 0 {
                let parent = usize::try_from(parent).map_err(|_| Error::Parent)?;
                if parent >= index {
                    return Err(Error::Parent);
                }
                let parent_buffer = capture
                    .buffers()
                    .iter()
                    .find(|b| b.object_index() == parent)
                    .ok_or(Error::Parent)?;
                let offset = usize::try_from(parent_offset).map_err(|_| Error::Bounds)?;
                if offset
                    .checked_add(8)
                    .is_none_or(|end| end > parent_buffer.bytes().len())
                {
                    return Err(Error::Bounds);
                }
                order::validate(&objects, last, last_minimum, parent, parent_offset)?;
                writes.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
                writes.push(PointerWrite {
                    destination: parent_buffer
                        .destination()
                        .start
                        .checked_add(offset)
                        .ok_or(Error::Bounds)?,
                    pointee: buffer.destination().start,
                });
            }
            writes.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
            writes.push(PointerWrite {
                destination: buffer.object_offset().checked_add(8).ok_or(Error::Bounds)?,
                pointee: buffer.destination().start,
            });
            last = Some(index);
            last_minimum = 0;
        }
        Ok(Self {
            capture,
            writes,
            fd_slots,
        })
    }
    pub fn capture(&self) -> &'s ScatterGather<'a> {
        self.capture
    }
    pub fn writes(&self) -> &[PointerWrite] {
        &self.writes
    }
    /// Numeric sender claims only; receiver FD installation is still required.
    pub fn fd_slots(&self) -> &[FdSlot] {
        &self.fd_slots
    }
}

#[cfg(test)]
mod fd_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction_snapshot::TransactionSnapshot;
    // Entries are (length, optional parent index/offset).
    fn snapshot(entries: &[(u64, Option<(u64, u64)>)]) -> TransactionSnapshot {
        let mut data = vec![0; 4];
        let mut offsets = Vec::new();
        for &(length, parent) in entries {
            offsets.extend_from_slice(&(data.len() as u64).to_le_bytes());
            data.extend_from_slice(&objects::Kind::Buffer.tag().to_le_bytes());
            data.extend_from_slice(&u32::from(parent.is_some()).to_le_bytes());
            data.extend_from_slice(&99u64.to_le_bytes());
            data.extend_from_slice(&length.to_le_bytes());
            let (index, offset) = parent.unwrap_or((0, 0));
            data.extend_from_slice(&index.to_le_bytes());
            data.extend_from_slice(&offset.to_le_bytes());
        }
        TransactionSnapshot::capture(&data, &offsets, 128).unwrap()
    }
    #[test]
    fn nested_and_sibling_writes_follow_depth_first_order() {
        let snapshot = snapshot(&[
            (24, None),
            (8, Some((0, 0))),
            (0, Some((1, 0))),
            (0, Some((0, 8))),
        ]);
        let sg = ScatterGather::capture(&snapshot, |_, b| {
            b.fill(0);
            Ok(())
        })
        .unwrap();
        let plan = PointerFixups::plan(&sg).unwrap();
        let base = snapshot.layout().extra().start;
        let writes: Vec<_> = plan
            .writes()
            .iter()
            .map(|w| (w.destination(), w.pointee()))
            .collect();
        assert_eq!(
            writes,
            [
                (12, base),
                (base, base + 24),
                (52, base + 24),
                (base + 24, base + 32),
                (92, base + 32),
                (base + 8, base + 32),
                (132, base + 32)
            ]
        );
    }
    #[test]
    fn invalid_parent_bounds_and_revisited_branch_rejected() {
        for entries in [
            vec![(8, Some((0, 0)))],
            vec![(8, None), (0, Some((0, 1)))],
            vec![(8, None), (0, Some((0, u64::MAX)))],
            vec![
                (24, None),
                (8, Some((0, 0))),
                (0, Some((0, 8))),
                (0, Some((1, 0))),
            ],
            vec![(24, None), (0, Some((0, 8))), (0, Some((0, 8)))],
        ] {
            let snapshot = snapshot(&entries);
            let sg = ScatterGather::capture(&snapshot, |_, _| Ok(())).unwrap();
            assert!(PointerFixups::plan(&sg).is_err());
        }
    }
}
