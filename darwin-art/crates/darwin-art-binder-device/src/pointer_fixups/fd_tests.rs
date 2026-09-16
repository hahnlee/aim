use super::*;
use crate::transaction_snapshot::TransactionSnapshot;

fn snapshot(count: u64, offset: u64, child: Option<u64>, pointer: u64) -> TransactionSnapshot {
    let mut data = vec![0; 4];
    let mut offsets = Vec::new();
    offsets.extend_from_slice(&4u64.to_le_bytes());
    data.extend_from_slice(&objects::Kind::Buffer.tag().to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&pointer.to_le_bytes());
    data.extend_from_slice(&24u64.to_le_bytes());
    data.extend_from_slice(&[0; 16]);
    offsets.extend_from_slice(&44u64.to_le_bytes());
    data.extend_from_slice(&objects::Kind::FdArray.tag().to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&count.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&offset.to_le_bytes());
    if let Some(child_offset) = child {
        offsets.extend_from_slice(&76u64.to_le_bytes());
        data.extend_from_slice(&objects::Kind::Buffer.tag().to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&child_offset.to_le_bytes());
    }
    TransactionSnapshot::capture(&data, &offsets, 24).unwrap()
}

fn capture(snapshot: &TransactionSnapshot) -> ScatterGather<'_> {
    ScatterGather::capture(snapshot, |_, bytes| {
        for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&(11 + index as u32).to_le_bytes());
        }
        Ok(())
    })
    .unwrap()
}

#[test]
fn fd_array_extraction_and_following_pointer_share_one_order_cursor() {
    let snapshot = snapshot(2, 4, Some(12), 100);
    let capture = capture(&snapshot);
    let plan = PointerFixups::plan(&capture).unwrap();
    let start = snapshot.layout().extra().start;
    assert_eq!(
        plan.fd_slots()
            .iter()
            .map(|fd| (fd.destination(), fd.sender_fd()))
            .collect::<Vec<_>>(),
        [(start + 4, 12), (start + 8, 13)]
    );
    assert_eq!(
        plan.writes()
            .iter()
            .map(|w| (w.destination(), w.pointee()))
            .collect::<Vec<_>>(),
        [(12, start), (start + 12, start + 24), (84, start + 24)]
    );
}

#[test]
fn array_overflow_bounds_alignment_and_following_overlap_fail() {
    for (count, offset, child, pointer) in [
        (u64::MAX, 0, None, 100),
        (7, 0, None, 100),
        (1, 24, None, 100),
        (1, 1, None, 100),
        (1, 0, None, 101),
        (2, 4, Some(8), 100),
    ] {
        let snapshot = snapshot(count, offset, child, pointer);
        let capture = capture(&snapshot);
        assert!(PointerFixups::plan(&capture).is_err());
    }
}

#[test]
fn empty_array_skips_range_checks_but_updates_order() {
    let input = snapshot(0, 1001, None, 101);
    let captured = capture(&input);
    assert!(
        PointerFixups::plan(&captured)
            .unwrap()
            .fd_slots()
            .is_empty()
    );
    let input = snapshot(0, 16, Some(8), 100);
    let captured = capture(&input);
    assert!(matches!(PointerFixups::plan(&captured), Err(Error::Order)));
}
