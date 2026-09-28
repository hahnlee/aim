//! `binder_proc` receive queue and delivered-buffer ownership. Incoming work
//! may be read by an eligible thread, but its allocation remains process-owned
//! until the exact client pointer is returned through BC_FREE_BUFFER.

use crate::{
    owned_transaction::{self, OwnedTransaction},
    receive_arena::ReceiveArena,
    transaction_snapshot::TransactionPayload,
    transaction_wire::{self, Metadata},
};
use std::os::fd::OwnedFd;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

pub struct Queue {
    arena: Arc<Mutex<ReceiveArena>>,
    pending: VecDeque<Pending>,
    delivered: HashMap<usize, OwnedTransaction>,
}

struct Pending {
    allocation: OwnedTransaction,
    header: Header,
    route: Route,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Route {
    pub call: Option<crate::thread::CallId>,
    pub target_thread: Option<u64>,
    pub deferred_completion: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Delivery {
    pub bytes: usize,
    pub call: Option<crate::thread::CallId>,
    pub reply: bool,
    pub completion: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Next {
    pub call: Option<crate::thread::CallId>,
    pub reply: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub reply: bool,
    pub target_pointer: u64,
    pub target_cookie: u64,
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Allocation(owned_transaction::Error),
    UnknownBuffer,
    DuplicateBuffer,
    OutOfMemory,
}

impl Queue {
    pub fn new(capacity: usize) -> std::io::Result<Self> {
        Ok(Self {
            arena: Arc::new(Mutex::new(ReceiveArena::new(capacity)?)),
            pending: VecDeque::new(),
            delivered: HashMap::new(),
        })
    }

    pub fn client_address(&self) -> Result<usize, Error> {
        self.arena
            .lock()
            .map(|arena| arena.client_address())
            .map_err(|_| Error::Allocation(owned_transaction::Error::Poisoned))
    }

    pub fn duplicate_backing(&self) -> Result<OwnedFd, Error> {
        self.arena
            .lock()
            .map_err(|_| Error::Allocation(owned_transaction::Error::Poisoned))?
            .duplicate_backing()
            .map_err(|_| Error::OutOfMemory)
    }

    /// Allocate and retain before publishing work. Failure leaves no visible
    /// queue entry and the RAII allocation rolls back.
    pub fn enqueue(
        &mut self,
        snapshot: &dyn TransactionPayload,
        header: Header,
    ) -> Result<usize, Error> {
        self.enqueue_rewritten(snapshot, header, &[])
    }

    pub fn enqueue_rewritten(
        &mut self,
        snapshot: &dyn TransactionPayload,
        header: Header,
        rewrites: &[crate::transaction_objects::Rewrite],
    ) -> Result<usize, Error> {
        self.enqueue_routed(snapshot, header, rewrites, Route::default())
    }

    pub(crate) fn enqueue_routed(
        &mut self,
        snapshot: &dyn TransactionPayload,
        header: Header,
        rewrites: &[crate::transaction_objects::Rewrite],
        route: Route,
    ) -> Result<usize, Error> {
        self.enqueue_routed_with_fds(snapshot, header, rewrites, route, Vec::new())
    }

    pub(crate) fn enqueue_routed_with_fds(
        &mut self,
        snapshot: &dyn TransactionPayload,
        header: Header,
        rewrites: &[crate::transaction_objects::Rewrite],
        route: Route,
        files: Vec<crate::installed_fds::InstalledFd>,
    ) -> Result<usize, Error> {
        let allocation = OwnedTransaction::prepare_rewritten_with_fds(
            Arc::clone(&self.arena),
            snapshot,
            rewrites,
            files,
        )
        .map_err(Error::Allocation)?;
        let address = allocation.buffer_address();
        self.pending
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.pending.push_back(Pending {
            allocation,
            header,
            route,
        });
        Ok(address)
    }

    /// Publish at most one complete BR record. A short read never removes work
    /// from the queue and never changes receive-buffer ownership.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        Ok(self
            .read_for_thread(0, true, output)?
            .map_or(0, |delivery| delivery.bytes))
    }

    /// Kernel Binder delivery rule: targeted work (replies, completions) goes
    /// to its thread; an untargeted one-way transaction is asynchronous
    /// process work and only goes to a thread that may take process work
    /// (a looper with no transaction in flight), never to a thread blocked
    /// waiting for its own reply.
    fn deliverable(pending: &Pending, thread_id: u64, accept_async: bool) -> bool {
        match pending.route.target_thread {
            Some(id) => id == thread_id,
            None => accept_async || pending.header.reply || pending.header.flags & 1 == 0,
        }
    }

    pub(crate) fn next_route(&self, thread_id: u64, accept_async: bool) -> Option<Next> {
        self.pending
            .iter()
            .find(|pending| Self::deliverable(pending, thread_id, accept_async))
            .map(|pending| Next {
                call: pending.route.call,
                reply: pending.header.reply,
            })
    }

    pub(crate) fn read_for_thread(
        &mut self,
        thread_id: u64,
        accept_async: bool,
        output: &mut [u8],
    ) -> Result<Option<Delivery>, Error> {
        let Some(position) = self
            .pending
            .iter()
            .position(|pending| Self::deliverable(pending, thread_id, accept_async))
        else {
            return Ok(None);
        };
        if self.pending[position].route.deferred_completion {
            if output.len() < 4 {
                return Ok(None);
            }
            self.pending[position].route.deferred_completion = false;
            output[..4].copy_from_slice(&crate::thread::BR_TRANSACTION_COMPLETE.to_le_bytes());
            return Ok(Some(Delivery {
                bytes: 4,
                call: self.pending[position].route.call,
                reply: true,
                completion: true,
            }));
        }
        if output.len() < transaction_wire::RECORD_SIZE {
            return Ok(None);
        }
        self.delivered
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        let pending = self
            .pending
            .remove(position)
            .expect("selected live queue entry");
        let address = pending.allocation.buffer_address();
        let data_size = pending.allocation.layout().data().len() as u64;
        let offsets_size = pending.allocation.layout().offsets().len() as u64;
        let offsets = pending.allocation.offsets_address() as u64;
        let encoded = transaction_wire::encode_transaction(
            output,
            pending.header.reply,
            Metadata {
                target_pointer: pending.header.target_pointer,
                target_cookie: pending.header.target_cookie,
                code: pending.header.code,
                flags: pending.header.flags,
                sender_pid: pending.header.sender_pid,
                sender_euid: pending.header.sender_euid,
                data_size,
                offsets_size,
                buffer: address as u64,
                offsets,
            },
        )
        .expect("size checked before queue mutation");
        if self.delivered.insert(address, pending.allocation).is_some() {
            return Err(Error::DuplicateBuffer);
        }
        Ok(Some(Delivery {
            bytes: encoded,
            call: pending.route.call,
            reply: pending.header.reply,
            completion: false,
        }))
    }

    pub fn free(&mut self, address: usize) -> Result<(), Error> {
        let allocation = self
            .delivered
            .remove(&address)
            .ok_or(Error::UnknownBuffer)?;
        allocation.release().map_err(Error::Allocation)
    }

    pub(crate) fn cancel_pending_call(&mut self, call: crate::thread::CallId) -> bool {
        let Some(position) = self
            .pending
            .iter()
            .position(|pending| !pending.header.reply && pending.route.call == Some(call))
        else {
            return false;
        };
        self.pending.remove(position);
        true
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn has_untargeted(&self) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.route.target_thread.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{transaction_snapshot::TransactionSnapshot, transfer_image::TransferImage};

    fn header() -> Header {
        Header {
            reply: false,
            target_pointer: 0x1111,
            target_cookie: 0x2222,
            code: 7,
            flags: 8,
            sender_pid: 9,
            sender_euid: 10,
        }
    }

    #[test]
    fn short_read_preserves_order_and_free_requires_delivered_exact_start() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut queue = Queue::new(32).unwrap();
        let first = TransactionSnapshot::capture(b"first", &[], 0).unwrap();
        let second = TransactionSnapshot::capture(b"second", &[], 0).unwrap();
        let first_address = queue.enqueue(&first, header()).unwrap();
        let second_address = queue
            .enqueue(
                &second,
                Header {
                    code: 11,
                    ..header()
                },
            )
            .unwrap();
        assert!(queue.has_pending());
        assert_eq!(queue.read(&mut [0xcc; 67]).unwrap(), 0);
        assert_eq!(queue.free(first_address), Err(Error::UnknownBuffer));
        let mut output = [0; 68];
        assert_eq!(queue.read(&mut output).unwrap(), 68);
        assert_eq!(u32::from_le_bytes(output[20..24].try_into().unwrap()), 7);
        assert_eq!(
            u64::from_le_bytes(output[52..60].try_into().unwrap()),
            first_address as u64
        );
        assert_eq!(queue.free(first_address + 1), Err(Error::UnknownBuffer));
        queue.free(first_address).unwrap();
        assert_eq!(queue.read(&mut output).unwrap(), 68);
        assert_eq!(u32::from_le_bytes(output[20..24].try_into().unwrap()), 11);
        assert_eq!(
            u64::from_le_bytes(output[52..60].try_into().unwrap()),
            second_address as u64
        );
        queue.free(second_address).unwrap();
        assert!(!queue.has_pending());
    }

    #[test]
    fn teardown_releases_pending_and_delivered_allocations() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut queue = Queue::new(16).unwrap();
        let snapshot = TransactionSnapshot::capture(b"12345678", &[], 0).unwrap();
        queue.enqueue(&snapshot, header()).unwrap();
        queue.enqueue(&snapshot, header()).unwrap();
        let mut output = [0; 68];
        queue.read(&mut output).unwrap();
        drop(queue);
    }

    #[test]
    fn immutable_transfer_image_enters_receive_mapping_without_intermediate_snapshot() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let snapshot = TransactionSnapshot::capture(b"remote parcel", &[], 0).unwrap();
        let image = TransferImage::capture(&snapshot, &[]).unwrap();
        let mut queue = Queue::new(32).unwrap();
        let address = queue.enqueue(&image, header()).unwrap();
        drop(image);

        let mut output = [0; transaction_wire::RECORD_SIZE];
        assert_eq!(queue.read(&mut output).unwrap(), output.len());
        assert_eq!(
            &queue.arena.lock().unwrap().bytes(address).unwrap()[..13],
            b"remote parcel"
        );
        queue.free(address).unwrap();
    }

    #[test]
    fn synchronous_reply_publishes_deferred_completion_before_buffer() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut queue = Queue::new(32).unwrap();
        let snapshot = TransactionSnapshot::capture(b"reply", &[], 0).unwrap();
        let address = queue
            .enqueue_routed(
                &snapshot,
                Header {
                    reply: true,
                    ..header()
                },
                &[],
                Route {
                    call: Some(crate::thread::CallId::local(7)),
                    target_thread: Some(11),
                    deferred_completion: true,
                },
            )
            .unwrap();
        assert!(
            queue
                .read_for_thread(12, true, &mut [0; transaction_wire::RECORD_SIZE])
                .unwrap()
                .is_none()
        );
        assert!(
            queue
                .read_for_thread(11, true, &mut [0; 3])
                .unwrap()
                .is_none()
        );
        let mut completion = [0; 4];
        let delivery = queue
            .read_for_thread(11, true, &mut completion)
            .unwrap()
            .unwrap();
        assert!(delivery.completion);
        assert_eq!(
            u32::from_le_bytes(completion),
            crate::thread::BR_TRANSACTION_COMPLETE
        );
        assert_eq!(queue.free(address), Err(Error::UnknownBuffer));

        let mut reply = [0; transaction_wire::RECORD_SIZE];
        let delivery = queue
            .read_for_thread(11, true, &mut reply)
            .unwrap()
            .unwrap();
        assert!(!delivery.completion);
        assert_eq!(
            u32::from_le_bytes(reply[..4].try_into().unwrap()),
            transaction_wire::BR_REPLY
        );
        queue.free(address).unwrap();
    }

    #[test]
    fn oneway_is_process_work_and_never_nests_on_a_waiting_thread() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let mut queue = Queue::new(32).unwrap();
        let snapshot = TransactionSnapshot::capture(b"event", &[], 0).unwrap();
        let oneway = queue
            .enqueue(
                &snapshot,
                Header {
                    flags: 1,
                    ..header()
                },
            )
            .unwrap();
        // A thread blocked waiting for its own reply must not execute it.
        assert!(queue.next_route(11, false).is_none());
        let mut output = [0; transaction_wire::RECORD_SIZE];
        assert!(
            queue
                .read_for_thread(11, false, &mut output)
                .unwrap()
                .is_none()
        );
        // A synchronous call may still be taken by that thread (nesting).
        let sync = queue.enqueue(&snapshot, header()).unwrap();
        let delivery = queue
            .read_for_thread(11, false, &mut output)
            .unwrap()
            .unwrap();
        assert!(!delivery.reply);
        queue.free(sync).unwrap();
        // An idle looper takes the one-way transaction.
        assert!(
            queue
                .read_for_thread(12, true, &mut output)
                .unwrap()
                .is_some()
        );
        queue.free(oneway).unwrap();
        assert!(!queue.has_pending());
    }
}
