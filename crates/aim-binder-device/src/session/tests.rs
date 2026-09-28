use super::*;
use crate::objects::{self, Kind as ObjectKind};

fn node(session: &mut Session) -> Arc<Node> {
    let mut bytes = [0; 24];
    bytes[..4].copy_from_slice(&ObjectKind::Binder.tag().to_le_bytes());
    bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
    bytes[16..24].copy_from_slice(&456u64.to_le_bytes());
    let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
    session.resolve_local(&objects[0]).unwrap()
}
fn ack(kind: Kind) -> Vec<u8> {
    let mut record = kind.word().to_le_bytes().to_vec();
    record.extend_from_slice(&123u64.to_le_bytes());
    record.extend_from_slice(&456u64.to_le_bytes());
    record
}
fn release() -> Vec<u8> {
    [Kind::Release.word().to_le_bytes(), 1u32.to_le_bytes()].concat()
}

#[test]
fn same_handle_numbers_do_not_cross_connection_ownership() {
    let mut sender = Session::default();
    let node = node(&mut sender);
    let mut first = Session::default();
    let mut second = Session::default();
    assert_eq!(
        first
            .retain_transferred(Arc::clone(&node), Strength::Strong)
            .unwrap()
            .0,
        1
    );
    assert_eq!(
        second.retain_transferred(node, Strength::Strong).unwrap().0,
        1
    );
    let mut output = [0; 40];
    assert_eq!(sender.read_node_work(&mut output).unwrap(), 40);
    for kind in [Kind::IncrefsDone, Kind::AcquireDone] {
        assert_eq!(
            first.execute(&ack(kind)).unwrap(),
            Outcome::Acknowledgement(node_ack::Outcome {
                bytes: 20,
                acknowledgement: Err(node_owner::Error::UnknownNode),
            })
        );
        assert_eq!(
            sender.execute(&ack(kind)).unwrap(),
            Outcome::Acknowledgement(node_ack::Outcome {
                bytes: 20,
                acknowledgement: Ok(()),
            })
        );
    }
    let done = first.execute(&release()).unwrap();
    assert_eq!(done.bytes(), 8);
    assert!(matches!(
        done,
        Outcome::Reference(reference_command::Outcome { update: Ok(_), .. })
    ));
    assert_eq!(sender.read_node_work(&mut output).unwrap(), 0);
    // Disconnect, rather than an explicit BC_RELEASE, removes the other ref.
    drop(second);
    assert!(sender.wait_for_node_work(Duration::ZERO));
    assert_eq!(sender.read_node_work(&mut output).unwrap(), 40);
    assert_eq!(
        u32::from_le_bytes(output[..4].try_into().unwrap()),
        0x80107209
    );
    assert_eq!(
        u32::from_le_bytes(output[20..24].try_into().unwrap()),
        0x8010720a
    );
}

#[test]
fn sender_disconnect_marks_nodes_dead_and_unsupported_records_do_not_execute() {
    let mut sender = Session::default();
    let node = node(&mut sender);
    let mut receiver = Session::default();
    receiver
        .retain_transferred(Arc::clone(&node), Strength::Strong)
        .unwrap();
    let record = release();
    for length in 0..8 {
        assert_eq!(
            receiver.execute(&record[..length]),
            Err(Error::Framing(command::DecodeError::Truncated))
        );
    }
    assert_eq!(
        receiver.references.counts(1).unwrap(),
        Counts { strong: 1, weak: 0 }
    );
    assert_eq!(
        receiver.execute(&Kind::EnterLooper.word().to_le_bytes()),
        Err(Error::Unsupported(Kind::EnterLooper))
    );
    drop(sender);
    assert!(!node.owner_alive());
    drop(receiver);
}
