use super::*;
use crate::{
    objects::Kind, pointer_fixups::PointerFixups, transaction_snapshot::TransactionSnapshot,
};
use std::{io::Read, os::unix::net::UnixStream};

fn snapshot() -> TransactionSnapshot {
    let mut data = [0; 100];
    data[4..8].copy_from_slice(&Kind::Buffer.tag().to_le_bytes());
    data[12..20].copy_from_slice(&100u64.to_le_bytes());
    data[20..28].copy_from_slice(&4u64.to_le_bytes());
    data[44..48].copy_from_slice(&Kind::FdArray.tag().to_le_bytes());
    data[52..60].copy_from_slice(&1u64.to_le_bytes());
    data[76..80].copy_from_slice(&Kind::Fd.tag().to_le_bytes());
    data[84..88].copy_from_slice(&900_001u32.to_le_bytes());
    let offsets: Vec<_> = [4u64, 44, 76]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect();
    TransactionSnapshot::capture(&data, &offsets, 8).unwrap()
}

#[test]
fn installs_mixed_fds_rewrites_mapping_and_drop_closes_them() {
    let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
    let input = snapshot();
    let capture = ScatterGather::capture(&input, |_, bytes| {
        bytes.copy_from_slice(&900_002u32.to_le_bytes());
        Ok(())
    })
    .unwrap();
    let plan = PointerFixups::plan(&capture).unwrap();
    let (sender, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let refs = PlannedFds::acquire(&plan, true, |_| sender.as_fd().try_clone_to_owned()).unwrap();
    let capacity = input.layout().total();
    let mut arena = ReceiveArena::new(capacity).unwrap();
    let pending = LocalInstallation::prepare_in_current_process(&mut arena, &refs).unwrap();
    for (index, destination) in [input.layout().extra().start, 84].into_iter().enumerate() {
        let fd = pending.files()[index].file().as_raw_fd();
        assert_ne!(fd, refs.references()[index].file().as_raw_fd());
        let stored = u32::from_le_bytes(
            pending.prepared.storage()[destination..destination + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(stored, fd as u32);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    assert_eq!(pending.files()[0].origin(), Origin::Array);
    assert_eq!(pending.files()[1].origin(), Origin::DirectObject);
    drop(refs);
    drop(sender);
    assert_eq!(
        peer.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(pending);
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    assert!(arena.allocate(capacity).is_ok());
}

#[test]
fn second_install_failure_rolls_back_first_fd_and_receive_region() {
    let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
    let input = snapshot();
    let capture = ScatterGather::capture(&input, |_, bytes| {
        bytes.fill(0);
        Ok(())
    })
    .unwrap();
    let plan = PointerFixups::plan(&capture).unwrap();
    let (sender, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let refs = PlannedFds::acquire(&plan, true, |_| sender.as_fd().try_clone_to_owned()).unwrap();
    let capacity = input.layout().total();
    let mut arena = ReceiveArena::new(capacity).unwrap();
    let mut calls = 0;
    let result = LocalInstallation::prepare_with(&mut arena, &refs, |file| {
        calls += 1;
        if calls == 2 {
            Err(io::Error::from_raw_os_error(libc::EMFILE))
        } else {
            file.try_clone_to_owned()
        }
    });
    assert!(matches!(result, Err(ref error) if error.raw_os_error() == Some(libc::EMFILE)));
    drop(result);
    drop(refs);
    drop(sender);
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    assert!(arena.allocate(capacity).is_ok());
}
