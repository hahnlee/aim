use super::*;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

const DESTINATION: DestinationEpoch = DestinationEpoch {
    pid: 42,
    birth: [7, 9],
};

#[test]
fn first_delivery_cannot_exceed_destination_fd_budget() {
    let (payload, mut peer) = payload_pair();
    let mut owner = HostFdDeliveryOwner::new(0xdef, limits(8, 16, 8, 1)).unwrap();
    // One payload plus its guardian read requires two retained descriptors.
    assert!(matches!(
        owner.reserve(DESTINATION, vec![payload]),
        Err(DeliveryError::LimitExceeded)
    ));
    assert_eq!(owner.active_count(), 0);
    let mut byte = [0u8; 1];
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}

fn payload_pair() -> (OwnedFd, UnixStream) {
    let (left, right) = UnixStream::pair().unwrap();
    (left.into(), right)
}

fn duplicate(fd: &OwnedFd) -> OwnedFd {
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    assert!(raw >= 0);
    unsafe { OwnedFd::from_raw_fd(raw) }
}

fn limits(global: usize, retained: usize, deliveries: usize, fds: usize) -> DeliveryLimits {
    DeliveryLimits {
        max_deliveries: global,
        max_retained_fds: retained,
        max_destination_deliveries: deliveries,
        max_destination_fds: fds,
    }
}

#[test]
fn eof_retires_aliases_and_imported_payload_survives() {
    let (payload, mut peer) = payload_pair();
    let imported_fd = duplicate(&payload);
    let mut owner = HostFdDeliveryOwner::new(0xabc, DeliveryLimits::default()).unwrap();
    let prepared = owner.reserve(DESTINATION, vec![payload]).unwrap();
    let offer = prepared.offer();
    owner.arm(DESTINATION, offer).unwrap();
    owner.admit_acquired(DESTINATION, offer).unwrap();
    drop(prepared);
    assert_eq!(owner.retire_ready(1).unwrap(), vec![offer]);
    assert_eq!(owner.active_count(), 0);

    let mut imported = unsafe { UnixStream::from_raw_fd(imported_fd.into_raw_fd()) };
    peer.write_all(b"ok").unwrap();
    let mut response = [0u8; 2];
    imported.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"ok");
}

#[test]
fn never_imported_payload_reaches_peer_eof_only_after_retirement() {
    let (payload, mut peer) = payload_pair();
    let mut owner = HostFdDeliveryOwner::new(0xabc, DeliveryLimits::default()).unwrap();
    let prepared = owner.reserve(DESTINATION, vec![payload]).unwrap();
    let offer = prepared.offer();
    drop(prepared);
    assert_eq!(owner.retire_ready(1).unwrap(), vec![offer]);
    let mut byte = [0u8; 1];
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}

#[test]
fn quotas_recover_and_wrong_destination_count_state_are_rejected() {
    let (payload, _peer) = payload_pair();
    let mut owner = HostFdDeliveryOwner::new(0xabc, limits(1, 2, 1, 2)).unwrap();
    let prepared = owner.reserve(DESTINATION, vec![payload]).unwrap();
    let offer = prepared.offer();
    assert!(matches!(
        owner.reserve(DESTINATION, vec![payload_pair().0]),
        Err(DeliveryError::LimitExceeded)
    ));
    assert!(matches!(
        owner.arm(
            DestinationEpoch {
                pid: 43,
                ..DESTINATION
            },
            offer
        ),
        Err(DeliveryError::DestinationMismatch)
    ));
    assert!(matches!(
        owner.admit_acquired(DESTINATION, offer),
        Err(DeliveryError::WrongState(DeliveryState::Prepared))
    ));
    owner.arm(DESTINATION, offer).unwrap();
    assert!(matches!(
        owner.admit_acquired(DESTINATION, DeliveryOffer { count: 2, ..offer }),
        Err(DeliveryError::UnknownOffer)
    ));
    owner.admit_acquired(DESTINATION, offer).unwrap();
    assert!(matches!(
        owner.admit_acquired(DESTINATION, offer),
        Err(DeliveryError::WrongState(DeliveryState::Admitted))
    ));
    drop(prepared);
    assert_eq!(owner.retire_ready(1).unwrap(), vec![offer]);
    let replacement = owner.reserve(DESTINATION, vec![payload_pair().0]).unwrap();
    drop(replacement);
}

#[test]
fn stale_epoch_and_wire_frames_are_rejected() {
    let mut owner = HostFdDeliveryOwner::new(0xabc, DeliveryLimits::default()).unwrap();
    let prepared = owner.reserve(DESTINATION, vec![payload_pair().0]).unwrap();
    let offer = prepared.offer();
    assert!(matches!(
        owner.admit_acquired(
            DESTINATION,
            DeliveryOffer {
                daemon_epoch: 0xdef,
                ..offer
            }
        ),
        Err(DeliveryError::UnknownOffer)
    ));
    let wire = DeliveryWire::new(
        WireKind::Offer,
        offer.daemon_epoch,
        offer.ticket,
        offer.count,
    )
    .unwrap();
    assert_eq!(
        DeliveryWire::decode_expected(&wire.encode(), WireKind::Offer).unwrap(),
        wire
    );
    assert!(matches!(
        DeliveryWire::decode_expected(&wire.encode()[..39], WireKind::Offer),
        Err(WireError::WrongSize)
    ));
    let mut bad = wire.encode();
    bad[36] = 1;
    assert!(matches!(
        DeliveryWire::decode_expected(&bad, WireKind::Offer),
        Err(WireError::ReservedBits)
    ));
    drop(prepared);
    assert_eq!(owner.retire_ready(1).unwrap(), vec![offer]);
}
