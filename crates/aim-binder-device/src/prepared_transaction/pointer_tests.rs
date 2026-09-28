use super::*;
use crate::transaction_snapshot::TransactionSnapshot;

#[test]
fn relocated_pointers_use_client_view_and_leave_input_immutable() {
    let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
    // Parent object at offset4; child object at44; its pointer fits at an
    // unaligned offset1 in the 16-byte parent buffer (per original byte-copy ABI).
    let mut data = [0; 84];
    for (offset, length) in [(4, 16u64), (44, 3u64)] {
        data[offset..offset + 4].copy_from_slice(&crate::objects::Kind::Buffer.tag().to_le_bytes());
        data[offset + 8..offset + 16].copy_from_slice(&99u64.to_le_bytes());
        data[offset + 16..offset + 24].copy_from_slice(&length.to_le_bytes());
    }
    data[48..52].copy_from_slice(&1u32.to_le_bytes());
    data[76..84].copy_from_slice(&1u64.to_le_bytes());
    let offsets: Vec<_> = [4u64, 44].into_iter().flat_map(u64::to_le_bytes).collect();
    let snapshot = TransactionSnapshot::capture(&data, &offsets, 24).unwrap();
    let capture = ScatterGather::capture(&snapshot, |_, bytes| {
        bytes.fill(7);
        Ok(())
    })
    .unwrap();
    let plan = PointerFixups::plan(&capture).unwrap();
    let capacity = snapshot.layout().total();
    let mut arena = ReceiveArena::new(capacity).unwrap();
    let address;
    {
        let prepared = PreparedTransaction::with_pointer_fixups(&mut arena, &plan).unwrap();
        address = prepared.address;
        let start = snapshot.layout().extra().start;
        let read_pointer = |offset: usize| {
            u64::from_le_bytes(prepared.storage()[offset..offset + 8].try_into().unwrap())
        };
        assert_eq!(read_pointer(12), (address + start) as u64);
        assert_eq!(read_pointer(52), (address + start + 16) as u64);
        assert_eq!(read_pointer(start + 1), (address + start + 16) as u64);
        assert_eq!(prepared.storage().as_ptr() as usize, address);
        let extra = &prepared.storage()[snapshot.layout().extra()];
        assert_eq!(extra[0], 7);
        assert_eq!(&extra[9..19], &[7; 10]);
        assert_eq!(&extra[19..], &[0; 5]);
        assert_eq!(prepared.offsets(), snapshot.offsets());
        assert_eq!(snapshot.data(), data);
        assert_eq!(capture.buffers()[0].bytes(), &[7; 16]);
    }
    assert_eq!(arena.allocate(capacity).unwrap(), address);
}
