use super::*;
use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, TransferToken},
    transaction_snapshot::TransactionSnapshot,
    transfer_image::TransferImage,
};

fn connection(raw: u64) -> ConnectionToken {
    ConnectionToken::from_nonzero(raw).unwrap()
}

fn token(raw: u64) -> TransferToken {
    TransferToken::from_nonzero(raw).unwrap()
}

fn descriptor() -> OwnedFd {
    let snapshot = TransactionSnapshot::capture(b"parcel", &[], 0).unwrap();
    TransferImage::capture(&snapshot, &[])
        .unwrap()
        .try_clone_descriptor()
        .unwrap()
}

#[test]
fn transfer_is_source_scoped_routed_and_consumed_exactly_once() {
    let mut table = TransferTable::default();
    table
        .deposit(connection(1), token(1), vec![descriptor()])
        .unwrap();
    table
        .deposit(connection(2), token(1), vec![descriptor()])
        .unwrap();
    assert!(matches!(
        table.take(connection(3), connection(1), token(1)),
        Err(Error::NotRouted)
    ));
    table.route(connection(1), token(1), connection(3)).unwrap();
    assert!(matches!(
        table.take(connection(4), connection(1), token(1)),
        Err(Error::WrongDestination)
    ));
    let mut received = table.take(connection(3), connection(1), token(1)).unwrap();
    assert_eq!(
        TransferImage::import(received.remove(0)).unwrap().data(),
        b"parcel"
    );
    assert!(matches!(
        table.take(connection(3), connection(1), token(1)),
        Err(Error::Unknown)
    ));
    assert!(table.ensure_pending(connection(2), token(1)).is_ok());
}

#[test]
fn duplicate_and_disconnect_cleanup_fail_closed() {
    let mut table = TransferTable::default();
    table
        .deposit(connection(1), token(1), vec![descriptor()])
        .unwrap();
    assert_eq!(
        table.deposit(connection(1), token(1), vec![descriptor()]),
        Err(Error::Duplicate)
    );
    table.route(connection(1), token(1), connection(2)).unwrap();
    table
        .deposit(connection(2), token(2), vec![descriptor()])
        .unwrap();
    table.remove_connection(connection(2));
    assert!(matches!(
        table.take(connection(2), connection(1), token(1)),
        Err(Error::Unknown)
    ));
    assert_eq!(
        table.ensure_pending(connection(2), token(2)),
        Err(Error::Unknown)
    );
}

#[test]
fn pending_transfer_can_be_discarded_exactly_once() {
    let mut table = TransferTable::default();
    table
        .deposit(connection(1), token(1), vec![descriptor()])
        .unwrap();
    table.discard_pending(connection(1), token(1)).unwrap();
    assert_eq!(
        table.discard_pending(connection(1), token(1)),
        Err(Error::Unknown)
    );
}

#[test]
fn routed_transfer_survives_source_disconnect_until_destination_takes_it() {
    let mut table = TransferTable::default();
    table
        .deposit(connection(1), token(1), vec![descriptor()])
        .unwrap();
    table.route(connection(1), token(1), connection(2)).unwrap();
    table.remove_connection(connection(1));
    let mut received = table.take(connection(2), connection(1), token(1)).unwrap();
    assert_eq!(
        TransferImage::import(received.remove(0)).unwrap().data(),
        b"parcel"
    );

    table
        .deposit(connection(1), token(2), vec![descriptor()])
        .unwrap();
    table.remove_connection(connection(1));
    assert_eq!(
        table.ensure_pending(connection(1), token(2)),
        Err(Error::Unknown)
    );
}
