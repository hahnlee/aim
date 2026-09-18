//! Connection lifetime and the nonblocking delivery/teardown serialization gate.
//! This owns device resources, not peer authentication or framework policy.
use crate::{
    node_owner::Node,
    session::{self, Session, transaction_target::TargetReferences},
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};

#[cfg(target_os = "macos")]
use crate::transaction_queue;

pub struct Connection {
    session: Mutex<Option<Session>>,
    threads: Mutex<HashMap<u64, Arc<Mutex<crate::thread::Thread>>>>,
    work_signal: Arc<crate::work_signal::Signal>,
    death_work: Mutex<crate::death_work::Queue>,
    pool: Mutex<pool::Pool>,
    #[cfg(target_os = "macos")]
    transactions: Mutex<Option<transaction_queue::Queue>>,
}
mod owner;
mod pool;
mod threads;
#[cfg(target_os = "macos")]
mod transactions;
pub(crate) mod work_wait;
pub use owner::ConnectionOwner;
#[cfg(test)]
pub(crate) use pool::BR_SPAWN_LOOPER;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Closed,
    Poisoned,
    WrongOwner,
    AlreadyRegistered,
    Command(session::Error),
    Thread(crate::thread::Error),
    ContextNode(session::context_manager::Error),
    Io(i32),
    #[cfg(target_os = "macos")]
    MappingAlreadyInstalled,
    #[cfg(target_os = "macos")]
    MappingNotInstalled,
    #[cfg(target_os = "macos")]
    TransactionQueue(transaction_queue::Error),
    ObjectTransfer(crate::transaction_objects::Error),
    Reference(crate::reference_table::Error),
    UnknownRemoteCall,
    OutOfMemory,
    Death(crate::death_work::Error),
}

impl Connection {
    pub(crate) fn hold_local_node(
        &self,
        local: crate::authority_protocol::LocalNodeToken,
        kind: crate::node_owner::HoldKind,
    ) -> Result<crate::node_owner::LocalHold, Error> {
        let session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let node = session
            .as_ref()
            .ok_or(Error::Closed)?
            .lookup_local_node(local)
            .map_err(|_| Error::WrongOwner)?;
        node.hold(kind)
            .map_err(|error| Error::Io(error.raw_os_error().unwrap_or(libc::EIO)))
    }

    pub(crate) fn bind_routing_id(&self, id: crate::routing_id::ConnectionId) -> Result<(), Error> {
        self.session
            .lock()
            .map_err(|_| Error::Poisoned)?
            .as_ref()
            .ok_or(Error::Closed)?
            .bind_routing_id(id)
            .map_err(|_| Error::AlreadyRegistered)
    }

    pub(crate) fn execute_context_reference(
        &self,
        input: &[u8],
        manager: Option<Arc<Node>>,
    ) -> Result<session::Outcome, Error> {
        let mut guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard
            .as_mut()
            .ok_or(Error::Closed)?
            .execute_context_reference(input, manager)
            .map(session::Outcome::ContextReference)
            .map_err(Error::Command)
    }
    pub(crate) fn ensure_open(&self) -> Result<(), Error> {
        let guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard.as_ref().ok_or(Error::Closed).map(|_| ())
    }
    pub(crate) fn remote_node_for_handle(
        &self,
        handle: u32,
    ) -> Result<crate::authority_protocol::NodeToken, Error> {
        self.session
            .lock()
            .map_err(|_| Error::Poisoned)?
            .as_ref()
            .ok_or(Error::Closed)?
            .remote_node_for_handle(handle)
            .map_err(Error::Reference)
    }
    pub(crate) fn create_context_manager_refs(
        &self,
        object: &crate::objects::Object<'_>,
    ) -> Result<crate::node_owner::ContextManagerRefs, Error> {
        let mut guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard
            .as_mut()
            .ok_or(Error::Closed)?
            .create_context_manager_refs(object)
            .map_err(Error::ContextNode)
    }
    fn new(session: Session) -> Arc<Self> {
        let work_signal = Arc::new(crate::work_signal::Signal::default());
        session.attach_work_signal(Arc::clone(&work_signal));
        Arc::new(Self {
            session: Mutex::new(Some(session)),
            threads: Mutex::new(HashMap::new()),
            work_signal,
            death_work: Mutex::new(crate::death_work::Queue::default()),
            pool: Mutex::new(pool::Pool::default()),
            #[cfg(target_os = "macos")]
            transactions: Mutex::new(None),
        })
    }

    /// Retained target leases keep metadata alive, not an open connection.
    /// Resource cleanup runs outside the gate after atomically closing it.
    pub fn disconnect(&self) {
        self.work_signal.close();
        let removed = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        drop(removed);
        self.threads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        #[cfg(target_os = "macos")]
        {
            let removed = self
                .transactions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            drop(removed);
        }
    }

    pub fn execute(&self, input: &[u8]) -> Result<session::Outcome, Error> {
        let mut guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard
            .as_mut()
            .ok_or(Error::Closed)?
            .execute(input)
            .map_err(Error::Command)
    }

    pub(crate) fn read_node_work(&self, output: &mut [u8]) -> Result<usize, Error> {
        let guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard
            .as_ref()
            .ok_or(Error::Closed)?
            .read_node_work(output)
            .map_err(|error| Error::Io(error.raw_os_error().unwrap_or(libc::EIO)))
    }

    pub(crate) fn enqueue_dead_binder(&self, cookie: u64) -> Result<(), Error> {
        self.death_work
            .lock()
            .map_err(|_| Error::Poisoned)?
            .enqueue_dead(cookie)
            .map_err(Error::Death)?;
        self.work_signal.notify();
        Ok(())
    }

    pub(crate) fn enqueue_clear_death_done(&self, cookie: u64) -> Result<(), Error> {
        self.death_work
            .lock()
            .map_err(|_| Error::Poisoned)?
            .enqueue_clear_done(cookie)
            .map_err(Error::Death)?;
        self.work_signal.notify();
        Ok(())
    }

    pub(crate) fn acknowledge_dead_binder(&self, cookie: u64) -> Result<(), Error> {
        self.death_work
            .lock()
            .map_err(|_| Error::Poisoned)?
            .acknowledge_dead(cookie)
            .map_err(Error::Death)
    }

    pub(crate) fn read_death_work(&self, output: &mut [u8]) -> Result<usize, Error> {
        Ok(self
            .death_work
            .lock()
            .map_err(|_| Error::Poisoned)?
            .read(output))
    }

    pub fn bind_target(
        self: &Arc<Self>,
        references: TargetReferences,
    ) -> Result<TargetConnection, Error> {
        let guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        let session = guard.as_ref().ok_or(Error::Closed)?;
        if !session.owns_node(references.node()) {
            return Err(Error::WrongOwner);
        }
        Ok(TargetConnection {
            connection: Arc::clone(self),
            references,
        })
    }
}

/// A target node and its verified owning connection, held through preparation.
pub struct TargetConnection {
    connection: Arc<Connection>,
    references: TargetReferences,
}
impl TargetConnection {
    pub(crate) fn node(&self) -> &Node {
        self.references.node()
    }

    /// Run a short queue/publication commit while disconnect is excluded.
    /// Must not block, call Java, perform IPC or reenter any connection/registry. A
    /// callback failure must roll back its own publication; no retry is implied.
    /// This gate alone does not implement a transaction queue or BR delivery.
    pub fn commit<T>(
        &self,
        publish: impl FnOnce(&Node) -> io::Result<T>,
    ) -> Result<io::Result<T>, Error> {
        let guard = self
            .connection
            .session
            .lock()
            .map_err(|_| Error::Poisoned)?;
        guard.as_ref().ok_or(Error::Closed)?;
        Ok(publish(self.references.node()))
    }
}

#[cfg(test)]
mod tests;
