//! Private host acquisition handshake. No bytes enter the guest Binder stream.
use super::*;
use crate::ProfileError;
use std::{
    io::{Read, Write},
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    sync::Mutex,
};

fn failed(error: impl std::fmt::Display) -> ProfileError {
    ProfileError::Daemon(format!("host FD delivery: {error}"))
}

pub(crate) fn new_owner() -> Result<Mutex<HostFdDeliveryOwner>, ProfileError> {
    let mut epoch = [0u8; 16];
    // Darwin getentropy fills all requested bytes or fails; never use a PID epoch.
    if unsafe { libc::getentropy(epoch.as_mut_ptr().cast(), epoch.len()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    HostFdDeliveryOwner::new(u128::from_ne_bytes(epoch), DeliveryLimits::default())
        .map(Mutex::new)
        .map_err(failed)
}

pub(crate) fn destination(stream: &UnixStream) -> Result<DestinationEpoch, ProfileError> {
    let (pid, incarnation) = crate::peer_process::identity(stream)?;
    Ok(DestinationEpoch {
        pid,
        birth: incarnation.parts(),
    })
}

pub(crate) fn prepare(
    owner: &Mutex<HostFdDeliveryOwner>,
    destination: DestinationEpoch,
    descriptors: Vec<OwnedFd>,
) -> Result<PreparedDelivery, ProfileError> {
    let mut owner = owner.lock().map_err(|_| failed("owner poisoned"))?;
    let prepared = owner.reserve(destination, descriptors).map_err(failed)?;
    owner.arm(destination, prepared.offer()).map_err(failed)?;
    Ok(prepared)
}

pub(crate) fn offer(prepared: &PreparedDelivery) -> Result<[u8; WIRE_SIZE], ProfileError> {
    frame(WireKind::Offer, prepared.offer()).map(DeliveryWire::encode)
}

fn frame(kind: WireKind, offer: DeliveryOffer) -> Result<DeliveryWire, ProfileError> {
    DeliveryWire::new(kind, offer.daemon_epoch, offer.ticket, offer.count).map_err(failed)
}

pub(crate) fn send_and_admit(
    owner: &Mutex<HostFdDeliveryOwner>,
    stream: &mut UnixStream,
    prepared: PreparedDelivery,
) -> Result<(), ProfileError> {
    let expected = prepared.offer();
    let target = prepared.destination();
    crate::fd_passing::send_many(stream, &prepared.descriptors().map_err(failed)?)?;
    // Never retry native send or release retained aliases on handler exit.
    drop(prepared);
    let mut receipt = [0u8; WIRE_SIZE];
    stream.read_exact(&mut receipt)?;
    let receipt = DeliveryWire::decode_expected(&receipt, WireKind::Acquired).map_err(failed)?;
    if receipt != frame(WireKind::Acquired, expected)? || destination(stream)? != target {
        return Err(failed("acquisition identity mismatch"));
    }
    owner
        .lock()
        .map_err(|_| failed("owner poisoned"))?
        .admit_acquired(target, expected)
        .map_err(failed)?;
    stream.write_all(&frame(WireKind::Admitted, expected)?.encode())?;
    Ok(())
}

pub(crate) fn receive(
    stream: &mut UnixStream,
    offered: &[u8],
) -> Result<darwin_art_binder_device::transfer_image::TransferImage, ProfileError> {
    let offer = DeliveryWire::decode_expected(offered, WireKind::Offer).map_err(failed)?;
    let mut descriptors = crate::fd_passing::receive_many(stream)?;
    if descriptors.len() != offer.count as usize + 1 {
        return Err(failed("native delivery count mismatch"));
    }
    let guardian = descriptors.remove(0);
    let image_descriptor = descriptors.remove(0);
    let image = darwin_art_binder_device::transfer_image::TransferImage::import_with_fds(
        image_descriptor,
        descriptors,
    )
    .map_err(|error| failed(format!("invalid transfer image: {error:?}")))?;
    let acquired = DeliveryWire {
        kind: WireKind::Acquired,
        ..offer
    };
    stream.write_all(&acquired.encode())?;
    let mut admitted = [0u8; WIRE_SIZE];
    stream.read_exact(&mut admitted)?;
    let admitted = DeliveryWire::decode_expected(&admitted, WireKind::Admitted).map_err(failed)?;
    if admitted
        != (DeliveryWire {
            kind: WireKind::Admitted,
            ..offer
        })
    {
        return Err(failed("admission mismatch"));
    }
    drop(guardian);
    Ok(image)
}

pub(crate) fn has_pending(owner: &Mutex<HostFdDeliveryOwner>) -> Result<bool, ProfileError> {
    let mut owner = owner.lock().map_err(|_| failed("owner poisoned"))?;
    owner.retire_ready(MAX_RETIRE_SCAN).map_err(failed)?;
    Ok(owner.active_count() != 0)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;
