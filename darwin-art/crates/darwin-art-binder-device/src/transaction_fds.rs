//! Retained direct Binder FD objects, before receiver-side FD installation.
//! Sender lookup must be supplied by an authenticated session/transport owner;
//! never interpret a sender's numeric FD as a descriptor in this process.
use crate::{object_fields::Fields, objects, transaction_snapshot::TransactionSnapshot};
use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};

#[derive(Debug)]
pub enum Error {
    Objects(objects::Error),
    NotAccepted,
    ScatterGatherRequired,
    OutOfMemory,
    Acquire(io::Error),
}

pub struct DirectFdReference {
    object_offset: usize,
    file: OwnedFd,
}

impl DirectFdReference {
    pub fn object_offset(&self) -> usize {
        self.object_offset
    }
    pub fn file(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

pub struct DirectFdReferences<'a> {
    snapshot: &'a TransactionSnapshot,
    references: Vec<DirectFdReference>,
}

impl<'a> DirectFdReferences<'a> {
    /// `accepts_fds` is derived by the transaction owner from the target node or
    /// original call's reply policy. Acquisition returns a retained file reference
    /// owned by this preparation; failure drops every already-acquired reference.
    /// Other object kinds still require their own translation. FDA requires the
    /// separate pointed-buffer snapshot path and is explicitly not accepted here.
    pub fn acquire(
        snapshot: &'a TransactionSnapshot,
        accepts_fds: bool,
        mut acquire_from_sender: impl FnMut(u32) -> io::Result<OwnedFd>,
    ) -> Result<Self, Error> {
        let objects = snapshot.objects().map_err(Error::Objects)?;
        let mut count = 0;
        for object in &objects {
            match object.fields() {
                Fields::Fd { .. } => count += 1,
                Fields::FdArray { .. } => return Err(Error::ScatterGatherRequired),
                _ => {}
            }
        }
        if count != 0 && !accepts_fds {
            return Err(Error::NotAccepted);
        }
        let mut references = Vec::new();
        references
            .try_reserve_exact(count)
            .map_err(|_| Error::OutOfMemory)?;
        for object in objects {
            if let Fields::Fd { fd, .. } = object.fields() {
                let file = acquire_from_sender(fd).map_err(Error::Acquire)?;
                references.push(DirectFdReference {
                    object_offset: object.offset(),
                    file,
                });
            }
        }
        Ok(Self {
            snapshot,
            references,
        })
    }

    pub fn snapshot(&self) -> &'a TransactionSnapshot {
        self.snapshot
    }
    pub fn references(&self) -> &[DirectFdReference] {
        &self.references
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::net::UnixStream};

    fn snapshot(fds: &[u32]) -> TransactionSnapshot {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for fd in fds {
            offsets.extend_from_slice(&(data.len() as u64).to_le_bytes());
            data.extend_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&fd.to_le_bytes());
            data.extend_from_slice(&[0xff; 12]);
        }
        TransactionSnapshot::capture(&data, &offsets, 0).unwrap()
    }

    #[test]
    fn sender_scoped_lookup_retains_files_until_preparation_drop() {
        let input = snapshot(&[900_000, 900_000]);
        let (sender, mut peer) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        let retained = DirectFdReferences::acquire(&input, true, |number| {
            assert_eq!(number, 900_000); // Deliberately not a usable local FD.
            sender.as_fd().try_clone_to_owned()
        })
        .unwrap();
        assert!(std::ptr::eq(retained.snapshot(), &input));
        assert_eq!(
            retained
                .references()
                .iter()
                .map(|r| r.object_offset())
                .collect::<Vec<_>>(),
            [0, 24]
        );
        for reference in retained.references() {
            use std::os::fd::AsRawFd;
            let flags = unsafe { libc::fcntl(reference.file().as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        }
        drop(sender);
        assert_eq!(
            peer.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(retained);
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn acquisition_failure_closes_partial_references() {
        let input = snapshot(&[1, 2]);
        let (sender, mut peer) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        let mut first: Option<OwnedFd> = Some(sender.into());
        let result = DirectFdReferences::acquire(&input, true, |number| {
            if number == 1 {
                Ok(first.take().unwrap())
            } else {
                Err(io::Error::from_raw_os_error(libc::EBADF))
            }
        });
        assert!(matches!(result, Err(Error::Acquire(_))));
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn policy_and_sg_requirements_checked_before_acquisition() {
        let input = snapshot(&[1]);
        assert!(matches!(
            DirectFdReferences::acquire(&input, false, |_| panic!("denied")),
            Err(Error::NotAccepted)
        ));
        let mut data = [0; 32];
        data[..4].copy_from_slice(&objects::Kind::FdArray.tag().to_le_bytes());
        let sg = TransactionSnapshot::capture(&data, &0u64.to_le_bytes(), 0).unwrap();
        assert!(matches!(
            DirectFdReferences::acquire(&sg, true, |_| panic!("SG")),
            Err(Error::ScatterGatherRequired)
        ));
        let empty = snapshot(&[]);
        assert!(
            DirectFdReferences::acquire(&empty, false, |_| panic!("empty"))
                .unwrap()
                .references()
                .is_empty()
        );
    }
}
