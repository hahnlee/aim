use super::*;
fn record(kind: Kind) -> Vec<u8> {
    let mut data = kind.word().to_le_bytes().to_vec();
    data.resize(4 + kind.payload_size(), 0xff);
    data[4..8].copy_from_slice(&7u32.to_le_bytes());
    data[20..24].copy_from_slice(&42u32.to_le_bytes());
    data[24..28].copy_from_slice(&0x10u32.to_le_bytes());
    data[36..44].copy_from_slice(&3u64.to_le_bytes());
    data[44..52].copy_from_slice(&0u64.to_le_bytes());
    data[52..60].copy_from_slice(&0x1234u64.to_le_bytes());
    data[60..68].copy_from_slice(&0x5678u64.to_le_bytes());
    if kind.payload_size() == 72 {
        data[68..76].copy_from_slice(&8u64.to_le_bytes());
    }
    data
}
#[test]
fn all_four_headers_unaligned_truncated_and_untrusted_identity_ignored() {
    for kind in [
        Kind::Transaction,
        Kind::Reply,
        Kind::TransactionSg,
        Kind::ReplySg,
    ] {
        let record = record(kind);
        for length in 0..record.len() {
            assert_eq!(
                decode(&record[..length]),
                Err(Error::Framing(command::DecodeError::Truncated))
            );
        }
        let mut stream = vec![0xff];
        stream.extend_from_slice(&record);
        stream.extend_from_slice(&record);
        let (request, bytes) = decode(&stream[1..]).unwrap();
        assert_eq!(bytes, record.len());
        assert_eq!(
            request.target(),
            if matches!(kind, Kind::Reply | Kind::ReplySg) {
                Target::Reply
            } else {
                Target::Handle(7)
            }
        );
        assert_eq!((request.code(), request.flags()), (42, 0x10));
        assert_eq!(
            request.data(),
            SenderRange {
                address: 0x1234,
                size: 3
            }
        );
        assert_eq!(
            request.offsets(),
            SenderRange {
                address: 0x5678,
                size: 0
            }
        );
        assert_eq!(
            request.extra_size(),
            if kind.payload_size() == 72 { 8 } else { 0 }
        );
        // Dirty high target union, cookie, PID and UID have no routing authority.
        let mut changed = record.clone();
        changed[8..20].fill(0);
        changed[28..36].fill(0);
        assert_eq!(decode(&changed).unwrap().0, request);
    }
}
#[test]
fn copied_payload_is_exact_and_structurally_validated() {
    let (request, _) = decode(&record(Kind::TransactionSg)).unwrap();
    assert!(matches!(
        request.capture_copied(&[1, 2], &[]),
        Err(Error::CopiedLengthMismatch)
    ));
    assert!(matches!(
        request.capture_copied(&[1, 2, 3], &[0]),
        Err(Error::CopiedLengthMismatch)
    ));
    let snapshot = request.capture_copied(&[1, 2, 3], &[]).unwrap();
    assert_eq!(snapshot.data(), &[1, 2, 3]);
    assert_eq!(snapshot.layout().extra(), 8..16);
    let mut invalid = record(Kind::TransactionSg);
    invalid[68..76].copy_from_slice(&1u64.to_le_bytes());
    let (request, _) = decode(&invalid).unwrap();
    assert!(matches!(
        request.capture_copied(&[1, 2, 3], &[]),
        Err(Error::Snapshot(transaction_snapshot::Error::ExtraAlignment))
    ));
    assert_eq!(
        decode(&Kind::EnterLooper.word().to_le_bytes()),
        Err(Error::NotTransaction(Kind::EnterLooper))
    );
}
