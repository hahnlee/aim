use super::*;
use darwin_art_scm_transfer::capabilities::CapabilityRegistry;
use darwin_art_scm_transfer::{AuthorityEpoch, ProcessEpoch, TransferKey};

fn key() -> TransferKey {
    TransferKey {
        authority: AuthorityEpoch { instance: 0x1122 },
        ticket: 7,
    }
}

fn header(operation: Operation, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![2, operation as u8, 0, 0];
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn register_pair_has_empty_request_body_and_strict_header() {
    assert_eq!(
        decode_request(&header(Operation::RegisterPair, &[])).unwrap(),
        Request::RegisterPair
    );
    let mut trailing = header(Operation::RegisterPair, &[]);
    trailing.push(0);
    assert!(decode_request(&trailing).is_err());
}

#[test]
fn prepare_round_trip_preserves_full_count_and_raw_managed_ids() {
    let mut body = Vec::new();
    body.extend_from_slice(&0x44u128.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&3u64.to_le_bytes());
    body.extend_from_slice(&0x101u128.to_le_bytes());
    body.extend_from_slice(&9u64.to_le_bytes());
    body.extend_from_slice(&0x202u128.to_le_bytes());
    assert_eq!(
        decode_request(&header(Operation::Prepare, &body)).unwrap(),
        Request::Prepare {
            carrier_holder: 0x44,
            payload_count: 16,
            managed: vec![(3, 0x101), (9, 0x202)],
        }
    );
}

#[test]
fn admit_rejects_unmanaged_published_ordinal_and_duplicate_delegation() {
    let mut body = Vec::new();
    body.extend_from_slice(&0x44u128.to_le_bytes());
    body.extend_from_slice(&key().authority.instance.to_le_bytes());
    body.extend_from_slice(&key().ticket.to_le_bytes());
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes());
    body.extend_from_slice(&0x101u128.to_le_bytes());
    body.extend_from_slice(&1u64.to_le_bytes());
    body.extend_from_slice(&0x101u128.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&3u64.to_le_bytes());
    assert!(decode_request(&header(Operation::Admit, &body)).is_err());

    let mut valid = body;
    // Two distinct delegations are required before testing the unmanaged
    // published ordinal itself.
    valid[76..92].copy_from_slice(&0x202u128.to_le_bytes());
    assert!(decode_request(&header(Operation::Admit, &valid)).is_err());
}

#[test]
fn settle_and_release_reject_zero_or_unknown_values() {
    let mut settle = Vec::new();
    settle.extend_from_slice(&key().authority.instance.to_le_bytes());
    settle.extend_from_slice(&key().ticket.to_le_bytes());
    settle.push(1);
    assert!(decode_request(&header(Operation::Settle, &settle)).is_ok());

    settle[24] = 9;
    assert!(decode_request(&header(Operation::Settle, &settle)).is_err());

    let zero = [0u8; 16];
    assert!(decode_request(&header(Operation::ReleaseHolder, &zero)).is_err());
}

#[test]
fn response_encoders_are_versioned_little_endian_and_bounded() {
    let mut body = Vec::new();
    body.extend_from_slice(&key().authority.instance.to_le_bytes());
    body.extend_from_slice(&key().ticket.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&8u64.to_le_bytes());
    body.extend_from_slice(&0xabcdu128.to_le_bytes());
    let encoded = encode_prepared(key(), 16, &[(8, 0xabcd)]).unwrap();
    assert_eq!(
        &encoded[..8],
        &[2, Operation::Prepare as u8, 0, 0, 52, 0, 0, 0]
    );
    assert_eq!(&encoded[8..], &body);
}

#[test]
fn admitted_encoder_uses_explicit_published_ordinals() {
    let authority = key().authority;
    let source = ProcessEpoch {
        pid: 1,
        instance: 2,
    };
    let receiver = ProcessEpoch {
        pid: 8,
        instance: 9,
    };
    let mut registry = CapabilityRegistry::new(authority).unwrap();
    let pair = registry.register_pair(source).unwrap();
    let authorizer = registry.trusted_authorizer();
    let ids = registry
        .prepare_scm_batch(source, key(), &[(7, pair.holder_a)])
        .unwrap();
    let reserved = registry
        .prepare_scm_claim(&authorizer, receiver, key(), &[(7, ids[0])], &[7])
        .unwrap();
    let claims = registry.commit_scm_claim(reserved).unwrap();
    let encoded = encode_admitted(&[(7, claims[0])]).unwrap();
    assert_eq!(
        &encoded[..8],
        &[2, Operation::Admit as u8, 0, 0, 51, 0, 0, 0]
    );
    assert_eq!(u16::from_le_bytes(encoded[8..10].try_into().unwrap()), 1);
    assert_eq!(u64::from_le_bytes(encoded[10..18].try_into().unwrap()), 7);
    assert_eq!(
        u128::from_le_bytes(encoded[18..34].try_into().unwrap()),
        claims[0].grant.id()
    );
    assert_eq!(
        u128::from_le_bytes(encoded[34..50].try_into().unwrap()),
        authority.instance
    );
    assert_eq!(u64::from_le_bytes(encoded[50..58].try_into().unwrap()), 1);
    assert_eq!(encoded[58], 0);
}

#[test]
fn response_encoders_reject_zero_ids_and_duplicates() {
    assert!(encode_prepared(key(), 1, &[(0, 0)]).is_err());
    assert!(encode_prepared(key(), 2, &[(0, 1), (0, 2)]).is_err());
}
