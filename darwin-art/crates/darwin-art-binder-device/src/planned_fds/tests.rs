use super::*;
use crate::{
    objects::Kind, scatter_gather::ScatterGather, transaction_snapshot::TransactionSnapshot,
};
use std::{io::Read, os::unix::net::UnixStream};

fn snapshot() -> TransactionSnapshot {
    let mut data = vec![0; 100];
    data[4..8].copy_from_slice(&Kind::Buffer.tag().to_le_bytes());
    data[12..20].copy_from_slice(&100u64.to_le_bytes());
    data[20..28].copy_from_slice(&8u64.to_le_bytes());
    data[44..48].copy_from_slice(&Kind::FdArray.tag().to_le_bytes());
    data[52..60].copy_from_slice(&2u64.to_le_bytes());
    data[76..80].copy_from_slice(&Kind::Fd.tag().to_le_bytes());
    data[84..88].copy_from_slice(&900_003u32.to_le_bytes());
    let offsets: Vec<_> = [4u64, 44, 76]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect();
    TransactionSnapshot::capture(&data, &offsets, 8).unwrap()
}

#[test]
fn mixed_references_keep_object_order_origin_and_file_lifetime() {
    let snapshot = snapshot();
    let capture = ScatterGather::capture(&snapshot, |_, bytes| {
        bytes[..4].copy_from_slice(&900_001u32.to_le_bytes());
        bytes[4..].copy_from_slice(&900_002u32.to_le_bytes());
        Ok(())
    })
    .unwrap();
    let plan = PointerFixups::plan(&capture).unwrap();
    let (sender, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let mut seen = Vec::new();
    let refs = PlannedFds::acquire(&plan, true, |fd| {
        seen.push(fd);
        sender.as_fd().try_clone_to_owned()
    })
    .unwrap();
    assert_eq!(seen, [900_001, 900_002, 900_003]);
    let start = snapshot.layout().extra().start;
    assert_eq!(
        refs.references()
            .iter()
            .map(|r| (r.destination(), r.origin()))
            .collect::<Vec<_>>(),
        [
            (start, Origin::Array),
            (start + 4, Origin::Array),
            (84, Origin::DirectObject)
        ]
    );
    assert!(std::ptr::eq(refs.plan(), &plan));
    drop(sender);
    assert_eq!(
        peer.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(refs);
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn denied_and_partial_mixed_acquisition_do_not_leak_files() {
    let snapshot = snapshot();
    let capture = ScatterGather::capture(&snapshot, |_, bytes| {
        bytes.fill(0);
        Ok(())
    })
    .unwrap();
    let plan = PointerFixups::plan(&capture).unwrap();
    assert!(matches!(
        PlannedFds::acquire(&plan, false, |_| panic!("denied")),
        Err(Error::NotAccepted)
    ));
    let (sender, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let mut sender: Option<OwnedFd> = Some(sender.into());
    let result = PlannedFds::acquire(&plan, true, |_| {
        sender
            .take()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))
    });
    assert!(matches!(result, Err(Error::Acquire(_))));
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
