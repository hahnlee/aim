//! binder_proc receive mapping, target resolution and atomic object-transfer
//! publication. Generic connection lifetime and reference commands stay in the
//! parent module.

use super::*;
use crate::transaction_snapshot::TransactionPayload;

impl Connection {
    pub(crate) fn resolve_remote_objects(
        &self,
        payload: &crate::transaction_snapshot::TransactionSnapshot,
    ) -> Result<Vec<crate::remote_objects::OutboundObject>, Error> {
        let mut session = self.session.lock().map_err(|_| Error::Poisoned)?;
        session
            .as_mut()
            .ok_or(Error::Closed)?
            .resolve_remote_objects(payload.data(), payload.offsets())
            .map_err(|error| {
                Error::ObjectTransfer(crate::transaction_objects::Error::Remote(error))
            })
    }

    pub fn install_receive_mapping(&self, capacity: usize) -> Result<usize, Error> {
        self.ensure_open()?;
        let queue = transaction_queue::Queue::new(capacity)
            .map_err(|error| Error::Io(error.raw_os_error().unwrap_or(libc::EIO)))?;
        let address = queue.client_address().map_err(Error::TransactionQueue)?;
        let mut slot = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        if slot.is_some() {
            return Err(Error::MappingAlreadyInstalled);
        }
        *slot = Some(queue);
        Ok(address)
    }

    pub fn duplicate_receive_mapping(&self) -> Result<std::os::fd::OwnedFd, Error> {
        self.ensure_open()?;
        self.transactions
            .lock()
            .map_err(|_| Error::Poisoned)?
            .as_ref()
            .ok_or(Error::MappingNotInstalled)?
            .duplicate_backing()
            .map_err(Error::TransactionQueue)
    }

    pub(crate) fn resolve_transaction_target(
        &self,
        request: &crate::transaction_request::Request,
    ) -> Result<TargetReferences, Error> {
        let guard = self.session.lock().map_err(|_| Error::Poisoned)?;
        guard
            .as_ref()
            .ok_or(Error::Closed)?
            .resolve_transaction_target(request)
            .map_err(|error| {
                Error::Io(match error {
                    crate::session::transaction_target::Error::ContextManagerRequired
                    | crate::session::transaction_target::Error::ReplyStackRequired
                    | crate::session::transaction_target::Error::SelfTransaction
                    | crate::session::transaction_target::Error::RemoteTarget => libc::EINVAL,
                    crate::session::transaction_target::Error::Reference(_)
                    | crate::session::transaction_target::Error::DeadTarget => libc::ESRCH,
                    crate::session::transaction_target::Error::HoldFailed(code) => code,
                })
            })
    }

    pub(crate) fn install_remote_reference(
        &self,
        node: crate::authority_protocol::NodeToken,
        strength: crate::reference_table::Strength,
        context_manager: bool,
    ) -> Result<u32, Error> {
        let mut session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let session = session.as_mut().ok_or(Error::Closed)?;
        let result = if context_manager {
            session.retain_remote_context_manager(node, strength)
        } else {
            session.retain_remote(node, strength)
        };
        result
            .map(|(handle, _, _)| handle)
            .map_err(Error::Reference)
    }

    pub(crate) fn resolve_remote_transaction_target(
        &self,
        request: &crate::transaction_request::Request,
    ) -> Result<Option<crate::authority_protocol::NodeToken>, Error> {
        let session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let session = session.as_ref().ok_or(Error::Closed)?;
        match session.resolve_transaction_route(request) {
            Ok(crate::session::transaction_target::ResolvedTransactionTarget::Remote(node)) => {
                Ok(Some(node))
            }
            Ok(crate::session::transaction_target::ResolvedTransactionTarget::Local(_)) => Ok(None),
            Err(error) => Err(Error::Io(match error {
                crate::session::transaction_target::Error::ContextManagerRequired
                | crate::session::transaction_target::Error::ReplyStackRequired
                | crate::session::transaction_target::Error::SelfTransaction
                | crate::session::transaction_target::Error::RemoteTarget => libc::EINVAL,
                crate::session::transaction_target::Error::Reference(_)
                | crate::session::transaction_target::Error::DeadTarget => libc::ESRCH,
                crate::session::transaction_target::Error::HoldFailed(code) => code,
            })),
        }
    }

    pub(crate) fn next_transaction_route(
        &self,
        thread_id: u64,
        accept_async: bool,
    ) -> Result<Option<transaction_queue::Next>, Error> {
        let transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        Ok(transactions
            .as_ref()
            .and_then(|queue| queue.next_route(thread_id, accept_async)))
    }

    pub(crate) fn has_untargeted_transaction(&self) -> Result<bool, Error> {
        let transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        Ok(transactions
            .as_ref()
            .is_some_and(transaction_queue::Queue::has_untargeted))
    }

    pub(crate) fn read_transaction_for_thread(
        &self,
        thread_id: u64,
        accept_async: bool,
        output: &mut [u8],
    ) -> Result<Option<transaction_queue::Delivery>, Error> {
        let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        match transactions.as_mut() {
            Some(queue) => queue
                .read_for_thread(thread_id, accept_async, output)
                .map_err(Error::TransactionQueue),
            None => Ok(None),
        }
    }

    pub(crate) fn free_transaction_buffer(&self, address: usize) -> Result<(), Error> {
        let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        let queue = transactions.as_mut().ok_or(Error::MappingNotInstalled)?;
        queue.free(address).map_err(Error::TransactionQueue)
    }

    pub(crate) fn cancel_remote_incoming(
        &self,
        call: crate::authority_protocol::CallToken,
    ) -> Result<(), Error> {
        let call = crate::thread::CallId::remote(call);
        {
            let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
            let queue = transactions.as_mut().ok_or(Error::MappingNotInstalled)?;
            if queue.cancel_pending_call(call) {
                return Ok(());
            }
        }

        let threads = self.threads.lock().map_err(|_| Error::Poisoned)?;
        for thread in threads.values() {
            let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
            if thread.cancel_incoming(call) {
                return Ok(());
            }
        }
        Err(Error::UnknownRemoteCall)
    }

    pub(crate) fn enqueue_translated_from(
        self: &Arc<Self>,
        sender: &Arc<Connection>,
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        header: transaction_queue::Header,
        route: transaction_queue::Route,
    ) -> Result<usize, Error> {
        if Arc::ptr_eq(sender, self) {
            return Err(Error::WrongOwner);
        }
        let objects = snapshot.objects().map_err(|error| {
            Error::ObjectTransfer(crate::transaction_objects::Error::Objects(error))
        })?;
        crate::transaction_objects::validate_supported(&objects).map_err(Error::ObjectTransfer)?;
        let mut resolved = Vec::new();
        resolved
            .try_reserve_exact(objects.len())
            .map_err(|_| Error::OutOfMemory)?;
        let mut sender_session = sender.session.lock().map_err(|_| Error::Poisoned)?;
        let sender_session = sender_session.as_mut().ok_or(Error::Closed)?;
        for object in &objects {
            resolved.push(
                sender_session
                    .resolve_transaction_object(object)
                    .map_err(Error::ObjectTransfer)?,
            );
        }

        let mut target_session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let target_session = target_session.as_mut().ok_or(Error::Closed)?;
        let mut rewrites = Vec::new();
        let mut rollbacks = Vec::new();
        rewrites
            .try_reserve_exact(resolved.len())
            .map_err(|_| Error::OutOfMemory)?;
        rollbacks
            .try_reserve_exact(resolved.len())
            .map_err(|_| Error::OutOfMemory)?;
        let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        let queue = transactions.as_mut().ok_or(Error::MappingNotInstalled)?;
        for object in &resolved {
            match target_session.install_transaction_object(object) {
                Ok((rewrite, rollback)) => {
                    rewrites.push(rewrite);
                    if let Some(rollback) = rollback {
                        rollbacks.push(rollback);
                    }
                }
                Err(error) => {
                    rollback_objects(target_session, rollbacks)?;
                    return Err(Error::ObjectTransfer(error));
                }
            }
        }
        let result = queue
            .enqueue_routed(snapshot, header, &rewrites, route)
            .map_err(Error::TransactionQueue);
        match result {
            Ok(address) => {
                self.work_signal.notify();
                Ok(address)
            }
            Err(error) => {
                rollback_objects(target_session, rollbacks)?;
                Err(error)
            }
        }
    }

    pub(crate) fn enqueue_remote_transaction(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteTransaction,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error> {
        let objects = payload.objects().map_err(|error| {
            Error::ObjectTransfer(crate::transaction_objects::Error::Objects(error))
        })?;
        crate::remote_objects::validate_delivery_objects(&objects).map_err(|error| {
            Error::ObjectTransfer(crate::transaction_objects::Error::Remote(error))
        })?;
        let mut session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let session = session.as_mut().ok_or(Error::Closed)?;
        let node = session
            .lookup_local_node(remote.target)
            .map_err(|_| Error::WrongOwner)?;
        if std::env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
            eprintln!(
                "ART Binder target: local={:?} pointer=0x{:x} cookie=0x{:x} code={}",
                remote.target,
                node.pointer(),
                node.cookie(),
                remote.code
            );
        }
        let mut rewrites = Vec::new();
        rewrites
            .try_reserve_exact(files.len())
            .map_err(|_| Error::OutOfMemory)?;
        rewrites.extend(files.iter().map(|file| file.rewrite()));
        let mut rollbacks = Vec::new();
        rewrites
            .try_reserve(payload.objects_manifest().len())
            .map_err(|_| Error::OutOfMemory)?;
        rollbacks
            .try_reserve_exact(payload.objects_manifest().len())
            .map_err(|_| Error::OutOfMemory)?;
        for object in payload.objects_manifest() {
            match session.install_remote_object(receiver, object) {
                Ok((rewrite, rollback)) => {
                    rewrites.push(rewrite);
                    if let Some(rollback) = rollback {
                        rollbacks.push(rollback);
                    }
                }
                Err(error) => {
                    rollback_objects(session, rollbacks)?;
                    return Err(Error::ObjectTransfer(
                        crate::transaction_objects::Error::Remote(error),
                    ));
                }
            }
        }
        let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        let result = transactions
            .as_mut()
            .ok_or(Error::MappingNotInstalled)?
            .enqueue_routed_with_fds(
                payload,
                transaction_queue::Header {
                    reply: false,
                    target_pointer: node.pointer(),
                    target_cookie: node.cookie(),
                    code: remote.code,
                    flags: remote.flags,
                    sender_pid: remote.sender_pid,
                    sender_euid: remote.sender_euid,
                },
                &rewrites,
                transaction_queue::Route {
                    call: remote.call.map(crate::thread::CallId::remote),
                    target_thread: None,
                    deferred_completion: false,
                },
                files,
            )
            .map_err(Error::TransactionQueue);
        match result {
            Ok(address) => {
                self.work_signal.notify();
                Ok(address)
            }
            Err(error) => {
                rollback_objects(session, rollbacks)?;
                Err(error)
            }
        }
    }

    pub(crate) fn enqueue_remote_reply(
        &self,
        payload: &crate::transfer_image::TransferImage,
        receiver: crate::authority_protocol::ConnectionToken,
        remote: crate::remote_transaction::RemoteReply,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error> {
        let objects = payload.objects().map_err(|error| {
            Error::ObjectTransfer(crate::transaction_objects::Error::Objects(error))
        })?;
        crate::remote_objects::validate_delivery_objects(&objects).map_err(|error| {
            Error::ObjectTransfer(crate::transaction_objects::Error::Remote(error))
        })?;
        let mut session = self.session.lock().map_err(|_| Error::Poisoned)?;
        let session = session.as_mut().ok_or(Error::Closed)?;
        let mut rewrites = Vec::new();
        rewrites
            .try_reserve_exact(files.len())
            .map_err(|_| Error::OutOfMemory)?;
        rewrites.extend(files.iter().map(|file| file.rewrite()));
        let mut rollbacks = Vec::new();
        rewrites
            .try_reserve(payload.objects_manifest().len())
            .map_err(|_| Error::OutOfMemory)?;
        rollbacks
            .try_reserve_exact(payload.objects_manifest().len())
            .map_err(|_| Error::OutOfMemory)?;
        for object in payload.objects_manifest() {
            match session.install_remote_object(receiver, object) {
                Ok((rewrite, rollback)) => {
                    rewrites.push(rewrite);
                    if let Some(rollback) = rollback {
                        rollbacks.push(rollback);
                    }
                }
                Err(error) => {
                    rollback_objects(session, rollbacks)?;
                    return Err(Error::ObjectTransfer(
                        crate::transaction_objects::Error::Remote(error),
                    ));
                }
            }
        }
        let mut transactions = self.transactions.lock().map_err(|_| Error::Poisoned)?;
        let result = transactions
            .as_mut()
            .ok_or(Error::MappingNotInstalled)?
            .enqueue_routed_with_fds(
                payload,
                transaction_queue::Header {
                    reply: true,
                    target_pointer: 0,
                    target_cookie: 0,
                    code: remote.code,
                    flags: remote.flags,
                    sender_pid: remote.sender_pid,
                    sender_euid: remote.sender_euid,
                },
                &rewrites,
                transaction_queue::Route {
                    call: Some(crate::thread::CallId::remote(remote.call)),
                    target_thread: Some(remote.target_thread),
                    deferred_completion: true,
                },
                files,
            )
            .map_err(Error::TransactionQueue);
        match result {
            Ok(address) => {
                self.work_signal.notify();
                Ok(address)
            }
            Err(error) => {
                rollback_objects(session, rollbacks)?;
                Err(error)
            }
        }
    }
}

impl TargetConnection {
    pub fn enqueue_transaction(
        &self,
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        header: transaction_queue::Header,
    ) -> Result<usize, Error> {
        let guard = self
            .connection
            .session
            .lock()
            .map_err(|_| Error::Poisoned)?;
        guard.as_ref().ok_or(Error::Closed)?;
        let mut transactions = self
            .connection
            .transactions
            .lock()
            .map_err(|_| Error::Poisoned)?;
        let address = transactions
            .as_mut()
            .ok_or(Error::MappingNotInstalled)?
            .enqueue(snapshot, header)
            .map_err(Error::TransactionQueue)?;
        self.connection.work_signal.notify();
        Ok(address)
    }

    pub(crate) fn enqueue_translated_transaction(
        &self,
        sender: &Arc<Connection>,
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        header: transaction_queue::Header,
        route: transaction_queue::Route,
    ) -> Result<usize, Error> {
        self.connection
            .enqueue_translated_from(sender, snapshot, header, route)
    }
}

fn rollback_objects(
    session: &mut Session,
    rollbacks: Vec<crate::transaction_objects::Rollback>,
) -> Result<(), Error> {
    for rollback in rollbacks.into_iter().rev() {
        session
            .rollback_transaction_object(rollback)
            .map_err(Error::ObjectTransfer)?;
    }
    Ok(())
}
