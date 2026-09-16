//! Receive-buffer layout corresponding to binder_alloc.c sanitized_size:
//! align each component independently to pointer size, check overflow, minimum8.
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionLayout {
    data: Range<usize>,
    offsets: Range<usize>,
    extra: Range<usize>,
    total: usize,
}

impl TransactionLayout {
    pub fn new(data_size: usize, offsets_size: usize, extra_size: usize) -> Option<Self> {
        fn align(value: usize) -> Option<usize> {
            value.checked_add(7).map(|v| v & !7)
        }
        let offsets_start = align(data_size)?;
        let extra_start = offsets_start.checked_add(align(offsets_size)?)?;
        let total = extra_start.checked_add(align(extra_size)?)?.max(8);
        Some(Self {
            data: 0..data_size,
            offsets: offsets_start..offsets_start.checked_add(offsets_size)?,
            extra: extra_start..extra_start.checked_add(extra_size)?,
            total,
        })
    }
    pub fn data(&self) -> Range<usize> {
        self.data.clone()
    }
    pub fn offsets(&self) -> Range<usize> {
        self.offsets.clone()
    }
    pub fn extra(&self) -> Range<usize> {
        self.extra.clone()
    }
    pub fn total(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligns_each_component_independently() {
        let layout = TransactionLayout::new(9, 8, 3).unwrap();
        assert_eq!(layout.data(), 0..9);
        assert_eq!(layout.offsets(), 16..24);
        assert_eq!(layout.extra(), 24..27);
        assert_eq!(layout.total(), 32);
        assert_eq!(TransactionLayout::new(0, 0, 0).unwrap().total(), 8);
    }
    #[test]
    fn each_alignment_and_combined_overflow_rejected() {
        assert!(TransactionLayout::new(usize::MAX, 0, 0).is_none());
        assert!(TransactionLayout::new(0, usize::MAX, 0).is_none());
        assert!(TransactionLayout::new(0, 0, usize::MAX).is_none());
        assert!(TransactionLayout::new(usize::MAX - 7, 8, 0).is_none());
        assert!(TransactionLayout::new(0, usize::MAX - 7, 8).is_none());
    }
}
