//! Authenticated connection registration and teardown.

use super::*;

impl Registry {
    /// Caller authenticates the device open before registering its unique owner.
    pub fn register(&self, owner: ConnectionOwner) -> Result<Key, Error> {
        let mut entries = self.entries.lock().map_err(|_| Error::Poisoned)?;
        let (id, serial) =
            crate::routing_id::ConnectionId::next(entries.next).ok_or(Error::Exhausted)?;
        entries
            .owners
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        owner.bind_routing_id(id).map_err(Error::Connection)?;
        entries.owners.insert(id, owner);
        entries.next = serial;
        Ok(Key {
            registry: Arc::clone(&self.identity),
            id,
        })
    }

    pub(super) fn check(&self, key: &Key) -> Result<(), Error> {
        if Arc::ptr_eq(&self.identity, &key.registry) {
            Ok(())
        } else {
            Err(Error::ForeignRegistry)
        }
    }

    pub(super) fn lookup(&self, key: &Key) -> Result<Arc<Connection>, Error> {
        self.check(key)?;
        self.lookup_id(key.id)
    }

    /// Internal authority lookup after the caller's Registry capability has
    /// already been authenticated at the public operation boundary.
    pub(super) fn lookup_id(
        &self,
        id: crate::routing_id::ConnectionId,
    ) -> Result<Arc<Connection>, Error> {
        let entries = self.entries.lock().map_err(|_| Error::Poisoned)?;
        entries
            .owners
            .get(&id)
            .map(ConnectionOwner::handle)
            .ok_or(Error::UnknownConnection)
    }

    pub fn close(&self, key: &Key) -> Result<(), Error> {
        self.check(key)?;
        let (owner, dead_callers) = {
            let _gate = self.transaction_gate.lock().map_err(|_| Error::Poisoned)?;
            let mut entries = self.entries.lock().map_err(|_| Error::Poisoned)?;
            let owner = entries
                .owners
                .remove(&key.id)
                .ok_or(Error::UnknownConnection)?;
            #[cfg(target_os = "macos")]
            let dead_callers = {
                let mut calls = self.calls.lock().map_err(|_| Error::Poisoned)?;
                let dead = calls.take_target(key.id);
                dead.into_iter()
                    .filter_map(|(call, route)| {
                        entries
                            .owners
                            .get(&route.sender)
                            .map(|sender| (call, route.thread_id, sender.handle()))
                    })
                    .collect::<Vec<_>>()
            };
            #[cfg(not(target_os = "macos"))]
            let dead_callers = Vec::<(crate::thread::CallId, u64, Arc<Connection>)>::new();
            (owner, dead_callers)
        };
        // Never wait for a binder_thread while holding authority locks.
        drop(owner);
        for (call, thread_id, caller) in dead_callers {
            if let Some(thread) = caller
                .existing_thread(thread_id)
                .map_err(Error::Connection)?
            {
                thread
                    .lock()
                    .map_err(|_| Error::Poisoned)?
                    .enqueue_dead_reply(call)
                    .map_err(|_| Error::Connection(connection::Error::Io(libc::EINVAL)))?;
            }
        }
        self.clear_context_manager(key);
        Ok(())
    }
}
