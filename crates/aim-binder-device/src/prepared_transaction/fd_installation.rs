//! Receiver-process local FD installation. Must execute inside the actual
//! receiver process; it cannot install an FD in another process's table.
use super::*;
use crate::planned_fds::{Origin, PlannedFds};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

pub struct InstalledFd {
    file: OwnedFd,
    origin: Origin,
}
impl InstalledFd {
    pub fn file(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
    pub fn origin(&self) -> Origin {
        self.origin
    }
}

/// Pending only: Drop rolls back every FD and the allocation. No commit/delivery
/// operation exists until Binder node ownership and read publication are wired.
pub struct LocalInstallation<'a> {
    prepared: PreparedTransaction<'a>,
    files: Vec<InstalledFd>,
}
impl<'a> LocalInstallation<'a> {
    pub fn prepare_in_current_process(
        arena: &'a mut ReceiveArena,
        refs: &PlannedFds<'_, '_, 'a>,
    ) -> io::Result<Self> {
        Self::prepare_with(arena, refs, |file| file.try_clone_to_owned())
    }

    fn prepare_with(
        arena: &'a mut ReceiveArena,
        refs: &PlannedFds<'_, '_, 'a>,
        mut duplicate: impl FnMut(BorrowedFd<'_>) -> io::Result<OwnedFd>,
    ) -> io::Result<Self> {
        let prepared = PreparedTransaction::with_pointer_fixups(arena, refs.plan())?;
        let mut pending = Self {
            prepared,
            files: Vec::new(),
        };
        pending
            .files
            .try_reserve_exact(refs.references().len())
            .map_err(io::Error::other)?;
        for reference in refs.references() {
            // try_clone_to_owned installs a distinct CLOEXEC descriptor atomically.
            let file = duplicate(reference.file())?;
            let number = u32::try_from(file.as_raw_fd()).map_err(io::Error::other)?;
            pending.prepared.arena.write(
                pending.prepared.address,
                reference.destination(),
                &number.to_le_bytes(),
            )?;
            pending.files.push(InstalledFd {
                file,
                origin: reference.origin(),
            });
        }
        Ok(pending)
    }
    pub fn data(&self) -> &[u8] {
        self.prepared.data()
    }
    pub fn files(&self) -> &[InstalledFd] {
        &self.files
    }
}

#[cfg(test)]
mod tests;
