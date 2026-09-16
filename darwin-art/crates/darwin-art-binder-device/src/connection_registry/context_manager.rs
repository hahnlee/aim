//! One context manager per device context, with sticky effective-UID policy.
use super::*;
use crate::{node_owner::ContextManagerRefs, objects::Object};
mod reference_commands;
mod routing;
pub use routing::RoutingError;

pub(super) struct Manager {
    key: Key,
    _references: ContextManagerRefs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reservation(u64);

struct PendingManager {
    reservation: Reservation,
    manager: Manager,
}

#[derive(Default)]
pub(super) struct State {
    uid: Option<u32>,
    manager: Option<Manager>,
    pending: Option<PendingManager>,
    next_reservation: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Registry(super::Error),
    Busy,
    Security(i32),
    WrongUid,
    Node(connection::Error),
    ReservationExhausted,
    NoPendingReservation,
    Poisoned,
}
impl Registry {
    /// authorize must resolve authenticated Android effective UID for this key
    /// and enforce registration permission. Never use packet/host UID directly.
    /// It runs under the context mutex and must be nonblocking/nonreentrant.
    /// No default authorization or fabricated UID is supplied here.
    pub fn register_context_manager(
        &self,
        key: &Key,
        object: &Object<'_>,
        authorize: impl FnOnce(&Key) -> Result<u32, i32>,
    ) -> Result<(), Error> {
        let reservation = self.begin_context_manager(key, object, authorize)?.0;
        self.commit_context_manager(key, reservation)
    }

    /// Reserve the local context-manager node without publishing it. The
    /// process authority must acknowledge the matching LocalNodeToken before
    /// `commit_context_manager` makes handle 0 visible to this binder_proc.
    pub fn begin_context_manager(
        &self,
        key: &Key,
        object: &Object<'_>,
        authorize: impl FnOnce(&Key) -> Result<u32, i32>,
    ) -> Result<(Reservation, crate::authority_protocol::LocalNodeToken), Error> {
        let connection = self.lookup(key).map_err(Error::Registry)?;
        let mut context = self.context.lock().map_err(|_| Error::Poisoned)?;
        if context.manager.is_some() || context.pending.is_some() {
            return Err(Error::Busy);
        }
        let uid = authorize(key).map_err(Error::Security)?;
        if context.uid.is_some_and(|previous| previous != uid) {
            return Err(Error::WrongUid);
        }
        // Original driver binds UID before node allocation, even if it fails.
        context.uid = Some(uid);
        let references = connection
            .create_context_manager_refs(object)
            .map_err(Error::Node)?;
        let local = references.node().local_token();
        let next = context
            .next_reservation
            .checked_add(1)
            .ok_or(Error::ReservationExhausted)?;
        let reservation = Reservation(next);
        context.next_reservation = next;
        context.pending = Some(PendingManager {
            reservation,
            manager: Manager {
                key: key.clone(),
                _references: references,
            },
        });
        Ok((reservation, local))
    }

    pub fn commit_context_manager(&self, key: &Key, reservation: Reservation) -> Result<(), Error> {
        self.check(key).map_err(Error::Registry)?;
        let mut context = self.context.lock().map_err(|_| Error::Poisoned)?;
        let matches = context.pending.as_ref().is_some_and(|pending| {
            pending.reservation == reservation && pending.manager.key.id == key.id
        });
        if !matches {
            return Err(Error::NoPendingReservation);
        }
        let pending = context.pending.take().expect("matching pending manager");
        context.manager = Some(pending.manager);
        Ok(())
    }

    pub fn abort_context_manager(&self, key: &Key, reservation: Reservation) {
        let removed = {
            let mut context = self.context.lock().unwrap_or_else(|p| p.into_inner());
            if context.pending.as_ref().is_some_and(|pending| {
                pending.reservation == reservation && pending.manager.key.id == key.id
            }) {
                context.pending.take()
            } else {
                None
            }
        };
        drop(removed);
    }

    pub(super) fn clear_context_manager(&self, key: &Key) {
        let (removed, pending) = {
            let mut context = self.context.lock().unwrap_or_else(|p| p.into_inner());
            let manager = if context
                .manager
                .as_ref()
                .is_some_and(|manager| manager.key.id == key.id)
            {
                context.manager.take()
            } else {
                None
            };
            let pending = if context
                .pending
                .as_ref()
                .is_some_and(|pending| pending.manager.key.id == key.id)
            {
                context.pending.take()
            } else {
                None
            };
            (manager, pending)
        };
        // Caller already disconnected the owner; dropping refs cannot enqueue
        // callbacks to its dead session. Sticky UID intentionally remains.
        drop((removed, pending));
    }
}

#[cfg(test)]
mod tests;
