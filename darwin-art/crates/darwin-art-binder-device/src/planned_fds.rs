//! File references for the exact validated pointer/FD-array plan. Installation
//! into the receiver descriptor table and delivery ownership are separate.
use crate::{object_fields::Fields, pointer_fixups::PointerFixups, transaction_fds::Error};
use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    DirectObject,
    Array,
}

pub struct Reference {
    destination: usize,
    origin: Origin,
    file: OwnedFd,
}
impl Reference {
    pub fn destination(&self) -> usize {
        self.destination
    }
    /// FDA descriptors need buffer-release cleanup, unlike standalone FD objects.
    pub fn origin(&self) -> Origin {
        self.origin
    }
    pub fn file(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

pub struct PlannedFds<'p, 's, 'a> {
    plan: &'p PointerFixups<'s, 'a>,
    references: Vec<Reference>,
}
impl<'p, 's, 'a> PlannedFds<'p, 's, 'a> {
    /// Lookup belongs to the authenticated sender, never the receiving process's
    /// raw FD namespace. Every successful lookup transfers owned retention here.
    pub fn acquire(
        plan: &'p PointerFixups<'s, 'a>,
        accepts_fds: bool,
        mut lookup: impl FnMut(u32) -> io::Result<OwnedFd>,
    ) -> Result<Self, Error> {
        let objects = plan
            .capture()
            .snapshot()
            .objects()
            .map_err(Error::Objects)?;
        let direct = objects
            .iter()
            .filter(|o| matches!(o.fields(), Fields::Fd { .. }))
            .count();
        let count = direct
            .checked_add(plan.fd_slots().len())
            .ok_or(Error::OutOfMemory)?;
        if count != 0 && !accepts_fds {
            return Err(Error::NotAccepted);
        }
        let mut references = Vec::new();
        references
            .try_reserve_exact(count)
            .map_err(|_| Error::OutOfMemory)?;
        let mut slots = plan.fd_slots().iter().peekable();
        for (index, object) in objects.iter().enumerate() {
            if let Fields::Fd { fd, .. } = object.fields() {
                references.push(Reference {
                    destination: object.offset() + 8,
                    origin: Origin::DirectObject,
                    file: lookup(fd).map_err(Error::Acquire)?,
                });
            }
            while slots
                .peek()
                .is_some_and(|slot| slot.object_index() == index)
            {
                let slot = slots.next().unwrap();
                references.push(Reference {
                    destination: slot.destination(),
                    origin: Origin::Array,
                    file: lookup(slot.sender_fd()).map_err(Error::Acquire)?,
                });
            }
        }
        Ok(Self { plan, references })
    }
    pub fn plan(&self) -> &'p PointerFixups<'s, 'a> {
        self.plan
    }
    pub fn references(&self) -> &[Reference] {
        &self.references
    }
}

#[cfg(test)]
mod tests;
