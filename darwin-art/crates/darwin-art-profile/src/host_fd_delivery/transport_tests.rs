use super::*;
use crate::{fd_passing, protocol};
use darwin_art_binder_device::{
    objects, transaction_snapshot::TransactionSnapshot, transfer_image::TransferImage,
};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;

const OP: u16 = protocol::OP_BINDER_TRANSFER_TAKE;

fn image_with_payload() -> (TransferImage, UnixStream) {
    let (payload, peer) = UnixStream::pair().unwrap();
    let mut data = vec![0_u8; 28];
    data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
    data[12..16].copy_from_slice(&77_u32.to_le_bytes());
    let snapshot = TransactionSnapshot::capture(&data, &4_u64.to_le_bytes(), 0).unwrap();
    let image = TransferImage::capture_with_objects_and_fds(
        &snapshot,
        &[],
        &[],
        vec![(4, OwnedFd::from(payload))],
    )
    .unwrap();
    (image, peer)
}

fn write_offer(sender: &mut UnixStream, offer: &[u8]) {
    protocol::write_response(sender, OP, 0, offer).unwrap();
}

#[test]
fn real_transport_admits_image_and_retains_socket_payload() {
    let owner = Arc::new(new_owner().unwrap());
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let destination = destination(&sender).unwrap();
    let (source_image, mut payload_peer) = image_with_payload();
    let mut descriptors = vec![source_image.try_clone_descriptor().unwrap()];
    descriptors.extend(source_image.try_clone_files().unwrap());
    let prepared = prepare(owner.as_ref(), destination, descriptors).unwrap();
    let offer_bytes = offer(&prepared).unwrap();
    let server_owner = Arc::clone(&owner);
    let server = thread::spawn(move || {
        write_offer(&mut sender, &offer_bytes);
        send_and_admit(server_owner.as_ref(), &mut sender, prepared)
    });
    // Close source-owned originals before checking central alias retirement.
    drop(source_image);

    let offered = protocol::expect_ok(&mut receiver, OP).unwrap();
    let received = receive(&mut receiver, &offered).unwrap();
    server.join().unwrap().unwrap();
    assert_eq!(received.files().len(), 1);
    assert_eq!(
        &received.data()[4..8],
        &objects::Kind::Fd.tag().to_le_bytes()
    );
    assert_eq!(&received.data()[12..16], &77_u32.to_le_bytes());
    assert!(!has_pending(owner.as_ref()).unwrap());

    payload_peer.write_all(b"ok").unwrap();
    let mut bytes = [0_u8; 2];
    let result = unsafe {
        libc::read(
            received.files()[0].descriptor().as_raw_fd(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    assert_eq!(result, 2);
    assert_eq!(&bytes, b"ok");
}

#[test]
fn invalid_offered_count_drops_received_fds_and_allows_guardian_retirement() {
    let owner = new_owner().unwrap();
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let destination = destination(&sender).unwrap();
    let invalid_image: OwnedFd = File::open("/dev/null").unwrap().into();
    let prepared = prepare(&owner, destination, vec![invalid_image]).unwrap();
    let offer_bytes = offer(&prepared).unwrap();
    write_offer(&mut sender, &offer_bytes);
    let offered = protocol::expect_ok(&mut receiver, OP).unwrap();
    let mut malformed_offer = offered.clone();
    malformed_offer[32..36].copy_from_slice(&2_u32.to_le_bytes());
    let descriptors = prepared.descriptors().unwrap();
    fd_passing::send_many(&sender, &descriptors).unwrap();
    drop(descriptors);
    drop(prepared);

    assert!(receive(&mut receiver, &malformed_offer).is_err());
    drop(receiver);
    drop(sender);
    assert!(!has_pending(&owner).unwrap());
}

#[test]
fn malformed_image_with_valid_count_drops_guardian_and_payload() {
    let owner = new_owner().unwrap();
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let destination = destination(&sender).unwrap();
    let invalid_image: OwnedFd = File::open("/dev/null").unwrap().into();
    let prepared = prepare(&owner, destination, vec![invalid_image]).unwrap();
    let offer_bytes = offer(&prepared).unwrap();
    write_offer(&mut sender, &offer_bytes);
    let offered = protocol::expect_ok(&mut receiver, OP).unwrap();
    let descriptors = prepared.descriptors().unwrap();
    fd_passing::send_many(&sender, &descriptors).unwrap();
    drop(descriptors);
    drop(prepared);

    assert!(receive(&mut receiver, &offered).is_err());
    drop(receiver);
    drop(sender);
    assert!(!has_pending(&owner).unwrap());
}

#[test]
fn malformed_offer_is_rejected_before_native_payload_decode() {
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let malformed = [0_u8; WIRE_SIZE];
    write_offer(&mut sender, &malformed);
    let offered = protocol::expect_ok(&mut receiver, OP).unwrap();
    assert!(receive(&mut receiver, &offered).is_err());
}

#[test]
fn control_disconnect_does_not_retire_still_owned_native_group() {
    let owner = Arc::new(new_owner().unwrap());
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let target = destination(&sender).unwrap();
    let (source_image, mut payload_peer) = image_with_payload();
    let mut bundle = vec![source_image.try_clone_descriptor().unwrap()];
    bundle.extend(source_image.try_clone_files().unwrap());
    let prepared = prepare(&owner, target, bundle).unwrap();
    let offered = offer(&prepared).unwrap();
    drop(source_image);
    let server_owner = Arc::clone(&owner);
    let server = thread::spawn(move || {
        write_offer(&mut sender, &offered);
        send_and_admit(&server_owner, &mut sender, prepared)
    });
    protocol::expect_ok(&mut receiver, OP).unwrap();
    let native_group = fd_passing::receive_many(&receiver).unwrap();
    assert_eq!(native_group.len(), 3);
    drop(receiver); // actual EOF before the acquisition receipt
    assert!(server.join().unwrap().is_err());
    assert!(
        has_pending(&owner).unwrap(),
        "handler exit is not retirement"
    );
    drop(native_group); // closes guardian and never-imported FDs
    assert!(!has_pending(&owner).unwrap());
    let mut byte = [0u8; 1];
    assert_eq!(payload_peer.read(&mut byte).unwrap(), 0);
}
