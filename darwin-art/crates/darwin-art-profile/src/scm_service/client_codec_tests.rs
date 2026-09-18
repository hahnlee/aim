use super::*;
use darwin_art_scm_transfer::capabilities::CapabilityRegistry;
use darwin_art_scm_transfer::{AuthorityEpoch, ProcessEpoch};

fn key() -> TransferKey {
    TransferKey {
        authority: AuthorityEpoch { instance: 0x1122 },
        ticket: 7,
    }
}

#[test]
fn all_client_requests_decode_by_existing_server() {
    let requests = [
        Request::RegisterPair,
        Request::Prepare {
            carrier_holder: 9,
            payload_count: 16,
            managed: vec![(3, 10), (9, 10)],
        },
        Request::Admit {
            carrier_holder: 9,
            key: key(),
            payload_count: 16,
            managed: vec![(3, 10), (9, 11)],
            publish_ordinals: vec![9, 3],
        },
        Request::Settle {
            key: key(),
            disposition: DeliveryDisposition::Finished,
        },
        Request::Settle {
            key: key(),
            disposition: DeliveryDisposition::Aborted,
        },
        Request::ReleaseHolder { holder: 9 },
    ];
    for request in requests {
        assert_eq!(
            decode_request(&encode_request(&request).unwrap()).unwrap(),
            request
        );
    }
}

#[test]
fn server_pair_and_prepare_responses_decode_without_minting_client_grants() {
    let mut registry = CapabilityRegistry::new(key().authority).unwrap();
    let pair = registry
        .register_pair(ProcessEpoch {
            pid: 10,
            instance: 11,
        })
        .unwrap();
    let bytes = encode_pair(pair).unwrap();
    assert_eq!(
        decode_pair(&bytes).unwrap(),
        PairOffer {
            authority: key().authority.instance,
            carrier: pair.endpoint_a.carrier.serial,
            holder_a: pair.holder_a.id(),
            holder_b: pair.holder_b.id(),
        }
    );
    let prepared = encode_prepared(key(), 16, &[(3, 10), (9, 11)]).unwrap();
    assert_eq!(
        decode_prepared(&prepared).unwrap(),
        PreparedOffer {
            key: key(),
            payload_count: 16,
            managed: vec![(3, 10), (9, 11)],
        }
    );
    assert!(decode_pair(&prepared).is_err());
    assert!(decode_prepared(&bytes).is_err());
    for response in [bytes, prepared] {
        for end in 0..response.len() {
            assert!(decode_pair(&response[..end]).is_err());
            assert!(decode_prepared(&response[..end]).is_err());
        }
    }
}

#[test]
fn invalid_last_item_never_encodes_a_valid_prefix() {
    let request = Request::Admit {
        carrier_holder: 9,
        key: key(),
        payload_count: 16,
        managed: vec![(3, 10), (9, 10)],
        publish_ordinals: vec![3, 9],
    };
    assert!(encode_request(&request).is_err());
    assert!(
        encode_request(&Request::Prepare {
            carrier_holder: 9,
            payload_count: usize::MAX,
            managed: vec![]
        })
        .is_err()
    );
    assert!(encode_request(&Request::ReleaseHolder { holder: 0 }).is_err());
}

#[test]
fn admitted_response_requires_exact_order_and_no_duplicate_holders() {
    let mut body = body_with_capacity(50).unwrap();
    push_u16(&mut body, 2);
    push_items(&mut body, &[(9, 10), (3, 11)]);
    let bytes = frame(Operation::Admit, body).unwrap();
    assert_eq!(
        decode_admitted(&bytes, &[9, 3]).unwrap(),
        vec![(9, 10), (3, 11)]
    );
    assert!(decode_admitted(&bytes, &[3, 9]).is_err());
    assert!(decode_admitted(&bytes, &[9]).is_err());
    let mut duplicate = bytes.clone();
    duplicate[HEADER_SIZE + 2 + 24 + 8..].copy_from_slice(&10u128.to_le_bytes());
    assert!(decode_admitted(&duplicate, &[9, 3]).is_err());
    let mut reserved = bytes.clone();
    reserved[2] = 1;
    assert!(decode_admitted(&reserved, &[9, 3]).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(decode_admitted(&trailing, &[9, 3]).is_err());
    let empty = frame(Operation::Admit, vec![0, 0]).unwrap();
    assert!(decode_admitted(&empty, &[]).unwrap().is_empty());
}
