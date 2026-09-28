//! Profile-wide transaction routing identities and outstanding call authority.
//!
//! These IDs are opaque capabilities inside this process today. Their wire
//! representation is deliberately not exposed yet; the daemon protocol must
//! authenticate a connection before associating any serialized identifier.

use super::*;

#[derive(Clone)]
pub(super) struct CallRoute {
    pub(super) sender: crate::routing_id::ConnectionId,
    pub(super) thread_id: u64,
    pub(super) target: crate::routing_id::NodeId,
}

#[derive(Default)]
pub(super) struct CallTable {
    next: u64,
    routes: HashMap<crate::thread::CallId, CallRoute>,
}

impl CallTable {
    pub(super) fn register(
        &mut self,
        route: CallRoute,
    ) -> Result<crate::thread::CallId, SubmitError> {
        let serial = self
            .next
            .checked_add(1)
            .ok_or(SubmitError::CallIdsExhausted)?;
        self.routes
            .try_reserve(1)
            .map_err(|_| SubmitError::OutOfMemory)?;
        let call = crate::thread::CallId::local(serial);
        self.routes.insert(call, route);
        self.next = serial;
        Ok(call)
    }

    pub(super) fn get(&self, call: crate::thread::CallId) -> Option<CallRoute> {
        self.routes.get(&call).cloned()
    }

    pub(super) fn remove(&mut self, call: crate::thread::CallId) -> Option<CallRoute> {
        self.routes.remove(&call)
    }

    pub(super) fn take_target(
        &mut self,
        target: crate::routing_id::ConnectionId,
    ) -> Vec<(crate::thread::CallId, CallRoute)> {
        let calls: Vec<_> = self
            .routes
            .iter()
            .filter(|(_, route)| route.target.owner() == target)
            .map(|(&call, route)| (call, route.clone()))
            .collect();
        for (call, _) in &calls {
            self.routes.remove(call);
        }
        calls
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.routes.len()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}
