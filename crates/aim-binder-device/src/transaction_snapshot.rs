//! Owned client data snapshot. Structural checks apply to the exact bytes kept
//! for later translation, never to memory the caller can subsequently change.
use crate::{objects, transaction_layout::TransactionLayout};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Layout,
    ExtraAlignment,
    Objects(objects::Error),
    OutOfMemory,
}

pub struct TransactionSnapshot {
    data: Vec<u8>,
    offsets: Vec<u8>,
    layout: TransactionLayout,
}

/// Immutable transaction sections consumed by receiver-side preparation.
/// Local captures and cross-process shared-memory images implement the same
/// contract so the receive queue never materializes a second Parcel.
pub trait TransactionPayload {
    fn data(&self) -> &[u8];
    fn offsets(&self) -> &[u8];
    fn layout(&self) -> &TransactionLayout;

    fn objects(&self) -> Result<Vec<objects::Object<'_>>, objects::Error> {
        objects::validate(self.data(), self.offsets())
    }
}

impl TransactionSnapshot {
    /// `data` and `offsets` must already be safe slices obtained by the platform
    /// copy-from-client boundary. This API does not dereference native pointers,
    /// authenticate sender identity or snapshot SG buffers referenced by objects.
    pub fn capture(data: &[u8], offsets: &[u8], extra_size: usize) -> Result<Self, Error> {
        if extra_size % 8 != 0 {
            return Err(Error::ExtraAlignment);
        }
        let layout =
            TransactionLayout::new(data.len(), offsets.len(), extra_size).ok_or(Error::Layout)?;
        fn copy(source: &[u8]) -> Result<Vec<u8>, Error> {
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(source.len())
                .map_err(|_| Error::OutOfMemory)?;
            owned.extend_from_slice(source);
            Ok(owned)
        }
        let data = copy(data)?;
        let offsets = copy(offsets)?;
        objects::validate(&data, &offsets).map_err(Error::Objects)?;
        Ok(Self {
            data,
            offsets,
            layout,
        })
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn offsets(&self) -> &[u8] {
        &self.offsets
    }
    pub fn layout(&self) -> &TransactionLayout {
        &self.layout
    }
    pub fn objects(&self) -> Result<Vec<objects::Object<'_>>, objects::Error> {
        objects::validate(&self.data, &self.offsets)
    }
}

impl TransactionPayload for TransactionSnapshot {
    fn data(&self) -> &[u8] {
        self.data()
    }

    fn offsets(&self) -> &[u8] {
        self.offsets()
    }

    fn layout(&self) -> &TransactionLayout {
        self.layout()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_buffers_cannot_change_validated_snapshot() {
        let mut data = vec![0; 28];
        data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
        let mut offsets = 4u64.to_le_bytes();
        let snapshot = TransactionSnapshot::capture(&data, &offsets, 8).unwrap();
        data.fill(0xff);
        offsets.fill(0xff);
        let objects = snapshot.objects().unwrap();
        assert_eq!(objects[0].kind(), objects::Kind::Fd);
        assert_eq!(objects[0].offset(), 4);
        assert_eq!(snapshot.layout().offsets(), 32..40);
        assert_eq!(snapshot.layout().extra(), 40..48);
    }
    #[test]
    fn invalid_snapshot_is_not_published() {
        assert!(matches!(
            TransactionSnapshot::capture(&[], &[0], 0),
            Err(Error::Objects(_))
        ));
        assert!(matches!(
            TransactionSnapshot::capture(&[], &[], 1),
            Err(Error::ExtraAlignment)
        ));
        assert!(matches!(
            TransactionSnapshot::capture(&[0; 8], &[], usize::MAX - 7),
            Err(Error::Layout)
        ));
        let data = [0xff; 24];
        assert!(matches!(
            TransactionSnapshot::capture(&data, &0u64.to_le_bytes(), 0),
            Err(Error::Objects(_))
        ));
    }
}
