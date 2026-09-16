use super::*;

fn connection(raw: u64) -> ConnectionToken {
    ConnectionToken::from_nonzero(raw).unwrap()
}

fn local(raw: u64) -> LocalNodeToken {
    LocalNodeToken::from_nonzero(raw).unwrap()
}

fn call(raw: u64) -> CallToken {
    CallToken::from_nonzero(raw).unwrap()
}

fn transfer(raw: u64) -> TransferToken {
    TransferToken::from_nonzero(raw).unwrap()
}

#[test]
fn every_message_round_trips_with_a_fixed_bound() {
    let node = NodeToken::new(connection(2), local(3));
    let messages = [
        Message::OpenConnection,
        Message::PublishNode { local: local(3) },
        Message::SetContextManager { local: local(3) },
        Message::GetContextManager,
        Message::RouteTransaction {
            target: node,
            caller_thread: 4,
            transfer: transfer(5),
            code: 6,
            flags: 7,
        },
        Message::CompleteReply {
            call: call(8),
            transfer: transfer(9),
            code: 10,
            flags: 11,
        },
        Message::RequestDeath {
            target: node,
            cookie: 12,
        },
        Message::ClearDeath {
            target: node,
            cookie: 12,
        },
        Message::CloseConnection,
        Message::ConnectionOpened {
            connection: connection(1),
            android_uid: 1000,
        },
        Message::NodePublished { node },
        Message::ContextManagerSet { node },
        Message::ContextManagerFound { node: None },
        Message::ContextManagerFound { node: Some(node) },
        Message::RouteAccepted { call: None },
        Message::RouteAccepted {
            call: Some(call(8)),
        },
        Message::RouteRejected {
            reason: TransactionFailure::DeadReply,
        },
        Message::RouteRejected {
            reason: TransactionFailure::FailedReply,
        },
        Message::ReplyAccepted { call: call(8) },
        Message::DeathRequested,
        Message::DeathCleared,
        Message::DeliverTransaction {
            call: Some(call(8)),
            sender: connection(1),
            sender_pid: 9,
            sender_euid: 10_001,
            target: local(3),
            caller_thread: 4,
            transfer: transfer(5),
            code: 6,
            flags: 7,
        },
        Message::DeliverReply {
            call: call(8),
            source: connection(2),
            sender_pid: 12,
            sender_euid: 10_002,
            target_thread: 4,
            transfer: transfer(5),
            code: 10,
            flags: 11,
        },
        Message::TargetDead {
            call: call(8),
            target_thread: 4,
        },
        Message::CallerDead { call: call(8) },
        Message::NodeDead { cookie: 12 },
    ];
    for message in messages {
        let mut bytes = Vec::new();
        encode(&mut bytes, message).unwrap();
        assert!(bytes.len() <= 16 + MAX_PAYLOAD_BYTES);
        assert_eq!(decode(&mut bytes.as_slice()).unwrap(), message);
    }
}

#[test]
fn malformed_envelopes_lengths_operations_and_tokens_fail_closed() {
    let mut valid = Vec::new();
    encode(
        &mut valid,
        Message::ConnectionOpened {
            connection: connection(1),
            android_uid: 1000,
        },
    )
    .unwrap();

    let mut bad = valid.clone();
    bad[0] ^= 1;
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::BadMagic)
    ));
    let mut bad = valid.clone();
    bad[8] = 4;
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::UnsupportedVersion)
    ));
    let mut bad = valid.clone();
    bad[10..12].copy_from_slice(&99_u16.to_le_bytes());
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::UnknownOperation)
    ));
    let mut bad = valid.clone();
    bad[12..16].copy_from_slice(&((MAX_PAYLOAD_BYTES + 1) as u32).to_le_bytes());
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::Oversized)
    ));
    let mut bad = valid.clone();
    bad[12..16].copy_from_slice(&7_u32.to_le_bytes());
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::WrongPayloadSize)
    ));
    let mut bad = valid;
    bad[16..24].fill(0);
    assert!(matches!(
        decode(&mut bad.as_slice()),
        Err(DecodeError::ZeroToken)
    ));

    let mut rejected = Vec::new();
    encode(
        &mut rejected,
        Message::RouteRejected {
            reason: TransactionFailure::DeadReply,
        },
    )
    .unwrap();
    rejected[16..20].copy_from_slice(&3_u32.to_le_bytes());
    assert!(matches!(
        decode(&mut rejected.as_slice()),
        Err(DecodeError::InvalidTransactionFailure)
    ));
    rejected[12..16].copy_from_slice(&3_u32.to_le_bytes());
    assert!(matches!(
        decode(&mut rejected.as_slice()),
        Err(DecodeError::WrongPayloadSize)
    ));
}

#[test]
fn requests_have_no_claimed_sender_identity_or_parcel_payload() {
    let mut bytes = Vec::new();
    encode(
        &mut bytes,
        Message::RouteTransaction {
            target: NodeToken::new(connection(2), local(3)),
            caller_thread: 4,
            transfer: transfer(5),
            code: 6,
            flags: 7,
        },
    )
    .unwrap();
    assert_eq!(bytes.len(), 16 + 40);
}
