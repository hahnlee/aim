//! Checked execution of one Binder `BINDER_WRITE_READ` request.
//!
//! Raw client pointers are resolved by the eventual device endpoint. This layer
//! owns the Linux Binder consumption contract over already-borrowed buffers:
//! only completely executed write records are consumed, and read publication is
//! reported independently. Framework services and transaction policy do not
//! belong here.

use crate::{
    command::{self, Kind},
    connection_registry,
    connection_registry::Registry,
    thread, transaction_request,
    transaction_snapshot::TransactionSnapshot,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Consumed {
    pub write: usize,
    pub read: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error<E> {
    pub consumed: Consumed,
    pub cause: Cause<E>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Cause<E> {
    Framing(command::DecodeError),
    Connection(connection_registry::Error),
    Thread(thread::Error),
    TransactionRequest(transaction_request::Error),
    TransactionCapture(E),
    #[cfg(target_os = "macos")]
    TransactionSubmit(connection_registry::SubmitError),
}

pub trait TransactionSource {
    type Error;

    fn capture(
        &mut self,
        request: &transaction_request::Request,
    ) -> Result<TransactionSnapshot, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SenderIdentity {
    pub pid: i32,
    pub euid: u32,
}

impl Registry {
    /// Execute the checked buffers behind one authenticated device open.
    ///
    /// A failed record is not included in `consumed.write`. Previously completed
    /// records remain committed, matching Binder's streaming write semantics.
    /// Reads never occur after a write failure, so callers can retry from the
    /// exact reported boundary without accidentally consuming pending work.
    pub fn write_read<T: TransactionSource>(
        &self,
        key: &connection_registry::Key,
        thread: &mut thread::Thread,
        write: &[u8],
        read: &mut [u8],
        transactions: &mut T,
        sender: SenderIdentity,
    ) -> Result<Consumed, Error<T::Error>> {
        let mut consumed = Consumed::default();
        while consumed.write < write.len() {
            let remaining = &write[consumed.write..];
            let (command, _) = command::decode(remaining).map_err(|error| Error {
                consumed,
                cause: Cause::Framing(error),
            })?;
            if matches!(
                command.kind,
                Kind::RegisterLooper | Kind::EnterLooper | Kind::ExitLooper
            ) {
                match self.execute_looper_command(key, thread, remaining) {
                    Ok(bytes) => consumed.write += bytes,
                    Err(cause) => {
                        let cause = match cause {
                            connection_registry::Error::Connection(
                                crate::connection::Error::Thread(cause),
                            ) => Cause::Thread(cause),
                            cause => Cause::Connection(cause),
                        };
                        return Err(Error { consumed, cause });
                    }
                }
            } else if matches!(
                command.kind,
                Kind::Transaction | Kind::TransactionSg | Kind::Reply | Kind::ReplySg
            ) {
                let (request, bytes) =
                    transaction_request::decode(remaining).map_err(|cause| Error {
                        consumed,
                        cause: Cause::TransactionRequest(cause),
                    })?;
                let snapshot = transactions.capture(&request).map_err(|cause| Error {
                    consumed,
                    cause: Cause::TransactionCapture(cause),
                })?;
                #[cfg(target_os = "macos")]
                if request.target() == transaction_request::Target::Reply {
                    let call = thread.current_incoming().map_err(|cause| Error {
                        consumed,
                        cause: Cause::Thread(cause),
                    })?;
                    let reservation = thread.reserve_submission(false).map_err(|cause| Error {
                        consumed,
                        cause: Cause::Thread(cause),
                    })?;
                    self.submit_reply(key, call, &request, &snapshot, sender, reservation)
                        .map_err(|cause| Error {
                            consumed,
                            cause: Cause::TransactionSubmit(cause),
                        })?;
                    thread.finish_incoming(call).map_err(|cause| Error {
                        consumed,
                        cause: Cause::Thread(cause),
                    })?;
                } else {
                    let reservation = thread
                        .reserve_submission(request.flags() & 1 == 0)
                        .map_err(|cause| Error {
                            consumed,
                            cause: Cause::Thread(cause),
                        })?;
                    self.submit_transaction(
                        key,
                        &request,
                        &snapshot,
                        sender.pid,
                        sender.euid,
                        reservation,
                    )
                    .map_err(|cause| Error {
                        consumed,
                        cause: Cause::TransactionSubmit(cause),
                    })?;
                }
                #[cfg(not(target_os = "macos"))]
                return Err(Error {
                    consumed,
                    cause: Cause::Connection(connection_registry::Error::Connection(
                        crate::connection::Error::Command(crate::session::Error::Unsupported(
                            command.kind,
                        )),
                    )),
                });
                consumed.write += bytes;
            } else if command.kind == Kind::FreeBuffer {
                #[cfg(target_os = "macos")]
                match self.free_transaction_buffer(
                    key,
                    usize::try_from(u64::from_le_bytes(command.payload.try_into().unwrap()))
                        .map_err(|_| Error {
                            consumed,
                            cause: Cause::Connection(connection_registry::Error::Connection(
                                crate::connection::Error::TransactionQueue(
                                    crate::transaction_queue::Error::UnknownBuffer,
                                ),
                            )),
                        })?,
                ) {
                    Ok(()) => consumed.write += 4 + command.payload.len(),
                    Err(cause) => {
                        return Err(Error {
                            consumed,
                            cause: Cause::Connection(cause),
                        });
                    }
                }
                #[cfg(not(target_os = "macos"))]
                return Err(Error {
                    consumed,
                    cause: Cause::Connection(connection_registry::Error::Connection(
                        crate::connection::Error::Command(crate::session::Error::Unsupported(
                            Kind::FreeBuffer,
                        )),
                    )),
                });
            } else {
                match self.execute(key, remaining) {
                    Ok(outcome) => consumed.write += outcome.bytes(),
                    Err(cause) => {
                        return Err(Error {
                            consumed,
                            cause: Cause::Connection(cause),
                        });
                    }
                }
            }
        }

        if !read.is_empty() {
            thread.ensure_read_eligible().map_err(|cause| Error {
                consumed,
                cause: Cause::Thread(cause),
            })?;
            let spawn_bytes = self
                .read_spawn_request(key, thread, read)
                .map_err(|cause| Error {
                    consumed,
                    cause: Cause::Connection(cause),
                })?;
            if spawn_bytes != 0 {
                consumed.read = spawn_bytes;
                return Ok(consumed);
            }
            // A newly exported local binder is still retained by the sender's
            // Parcel only until this ioctl completes. Publish BR_INCREFS /
            // BR_ACQUIRE before a transaction-completion record can let
            // libbinder release that Parcel; otherwise the later owner command
            // would contain a stale JavaBBinder cookie.
            let node_bytes = self.read_node_work(key, read).map_err(|cause| Error {
                consumed,
                cause: Cause::Connection(cause),
            })?;
            if node_bytes != 0 {
                consumed.read = node_bytes;
                return Ok(consumed);
            }
            let local_bytes = thread.read_local_work(read);
            if local_bytes != 0 {
                consumed.read = local_bytes;
                return Ok(consumed);
            }
            let death_bytes = self.read_death_work(key, read).map_err(|cause| Error {
                consumed,
                cause: Cause::Connection(cause),
            })?;
            if death_bytes != 0 {
                consumed.read = death_bytes;
                return Ok(consumed);
            }
            #[cfg(target_os = "macos")]
            let transaction_bytes =
                self.deliver_transaction(key, thread, read)
                    .map_err(|cause| match cause {
                        connection_registry::DeliveryError::Registry(cause) => Error {
                            consumed,
                            cause: Cause::Connection(cause),
                        },
                        connection_registry::DeliveryError::Thread(cause) => Error {
                            consumed,
                            cause: Cause::Thread(cause),
                        },
                    })?;
            #[cfg(not(target_os = "macos"))]
            let transaction_bytes = 0;
            consumed.read = transaction_bytes;
        }
        Ok(consumed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::Kind,
        connection::ConnectionOwner,
        node_owner::Node,
        objects::{self, Kind as ObjectKind},
        reference_table::Strength,
        session::Session,
    };
    use std::sync::Arc;

    struct NoTransactions;
    impl TransactionSource for NoTransactions {
        type Error = ();

        fn capture(
            &mut self,
            _: &transaction_request::Request,
        ) -> Result<TransactionSnapshot, Self::Error> {
            Err(())
        }
    }

    struct SnapshotSource(Result<TransactionSnapshot, &'static str>);
    impl TransactionSource for SnapshotSource {
        type Error = &'static str;

        fn capture(
            &mut self,
            _: &transaction_request::Request,
        ) -> Result<TransactionSnapshot, Self::Error> {
            std::mem::replace(&mut self.0, Err("already captured"))
        }
    }

    fn execute(
        registry: &Registry,
        key: &connection_registry::Key,
        thread: &mut thread::Thread,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<Consumed, Error<()>> {
        registry.write_read(
            key,
            thread,
            write,
            read,
            &mut NoTransactions,
            SenderIdentity { pid: 1, euid: 0 },
        )
    }

    fn registered(registry: &Registry) -> (connection_registry::Key, Arc<Node>) {
        let mut owner_session = Session::default();
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&ObjectKind::Binder.tag().to_le_bytes());
        bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&456u64.to_le_bytes());
        let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        let node = owner_session.resolve_local(&objects[0]).unwrap();
        let owner = ConnectionOwner::new(owner_session);
        let key = registry.register(owner).unwrap();
        (key, node)
    }

    fn reference(kind: Kind, handle: u32) -> Vec<u8> {
        [kind.word().to_le_bytes(), handle.to_le_bytes()].concat()
    }

    #[test]
    fn concatenated_write_reports_exact_failure_boundary_and_skips_read() {
        let registry = Registry::default();
        let (owner_key, node) = registered(&registry);
        let mut receiver_session = Session::default();
        receiver_session
            .retain_transferred(node, Strength::Strong)
            .unwrap();
        let receiver = ConnectionOwner::new(receiver_session);
        let receiver_key = registry.register(receiver).unwrap();

        let mut write = reference(Kind::Increfs, 1);
        write.extend_from_slice(&Kind::ExitLooper.word().to_le_bytes());
        let mut read = [0xcc; 40];
        let mut thread = thread::Thread::default();
        let error = execute(&registry, &receiver_key, &mut thread, &write, &mut read).unwrap_err();
        assert_eq!(error.consumed, Consumed { write: 8, read: 0 });
        assert_eq!(read, [0xcc; 40]);
        assert_eq!(
            error.cause,
            Cause::Thread(thread::Error::InvalidTransition {
                from: "none",
                command: Kind::ExitLooper
            })
        );
        registry.close(&receiver_key).unwrap();
        registry.close(&owner_key).unwrap();
    }

    #[test]
    fn successful_write_then_read_publishes_owner_work_once() {
        let registry = Registry::default();
        let (owner_key, node) = registered(&registry);
        let mut receiver_session = Session::default();
        receiver_session
            .retain_transferred(node, Strength::Strong)
            .unwrap();
        let receiver = ConnectionOwner::new(receiver_session);
        let receiver_key = registry.register(receiver).unwrap();

        let mut read = [0; 40];
        let mut thread = thread::Thread::default();
        let enter = Kind::EnterLooper.word().to_le_bytes();
        let outcome = execute(&registry, &owner_key, &mut thread, &enter, &mut read).unwrap();
        assert_eq!(outcome, Consumed { write: 4, read: 40 });
        assert_eq!(
            u32::from_le_bytes(read[..4].try_into().unwrap()),
            0x80107207
        );
        assert_eq!(
            execute(&registry, &owner_key, &mut thread, &[], &mut read).unwrap(),
            Consumed::default()
        );
        registry.close(&receiver_key).unwrap();
        registry.close(&owner_key).unwrap();
    }

    #[test]
    fn read_requires_the_calling_thread_to_join_the_pool() {
        let registry = Registry::default();
        let (key, _node) = registered(&registry);
        let mut thread = thread::Thread::default();
        let mut read = [0; 40];
        let error = execute(&registry, &key, &mut thread, &[], &mut read).unwrap_err();
        assert_eq!(error.consumed, Consumed::default());
        assert_eq!(
            error.cause,
            Cause::Thread(thread::Error::NotEligibleForRead)
        );
        registry.close(&key).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn spawn_request_precedes_process_transaction_and_register_is_admitted_once() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let registry = Registry::default();
        let (target_key, node) = registered(&registry);
        let mut sender_session = Session::default();
        sender_session
            .retain_transferred(node, Strength::Strong)
            .unwrap();
        let sender_key = registry
            .register(ConnectionOwner::new(sender_session))
            .unwrap();
        registry.install_receive_mapping(&target_key, 4096).unwrap();

        let mut target_thread = thread::Thread::default();
        target_thread
            .execute_looper(&Kind::EnterLooper.word().to_le_bytes())
            .unwrap();
        let mut owner_work = [0; 40];
        let drained = registry
            .write_read(
                &target_key,
                &mut target_thread,
                &[],
                &mut owner_work,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(drained.read, owner_work.len());

        registry.set_max_threads(&target_key, 1).unwrap();
        let mut transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        transaction.resize(4 + Kind::Transaction.payload_size(), 0);
        transaction[4..8].copy_from_slice(&1u32.to_le_bytes());
        transaction[20..24].copy_from_slice(&77u32.to_le_bytes());
        let mut sender_thread = thread::Thread::default();
        let mut no_read = [];
        registry
            .write_read(
                &sender_key,
                &mut sender_thread,
                &transaction,
                &mut no_read,
                &mut SnapshotSource(Ok(TransactionSnapshot::capture(&[], &[], 0).unwrap())),
                SenderIdentity {
                    pid: 333,
                    euid: 10_333,
                },
            )
            .unwrap();

        let mut output = [0; crate::transaction_wire::RECORD_SIZE];
        let spawn = registry
            .write_read(
                &target_key,
                &mut target_thread,
                &[],
                &mut output,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(spawn.read, 4);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            crate::connection::BR_SPAWN_LOOPER
        );

        let mut worker = thread::Thread::default();
        let register = Kind::RegisterLooper.word().to_le_bytes();
        let registered = registry
            .write_read(
                &target_key,
                &mut worker,
                &register,
                &mut no_read,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(registered.write, 4);
        let delivered = registry
            .write_read(
                &target_key,
                &mut worker,
                &[],
                &mut output,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(delivered.read, output.len());
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            crate::transaction_wire::BR_TRANSACTION
        );

        let mut extra = thread::Thread::default();
        let error = registry
            .write_read(
                &target_key,
                &mut extra,
                &register,
                &mut no_read,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap_err();
        assert_eq!(
            error.cause,
            Cause::Thread(thread::Error::UnrequestedRegistration)
        );
        registry.close(&sender_key).unwrap();
        registry.close(&target_key).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn transaction_stream_uses_trusted_identity_and_free_buffer_lifetime() {
        let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
        let registry = Registry::default();
        let (target_key, node) = registered(&registry);
        let mut sender_session = Session::default();
        sender_session
            .retain_transferred(node, Strength::Strong)
            .unwrap();
        let sender_key = registry
            .register(ConnectionOwner::new(sender_session))
            .unwrap();
        registry.install_receive_mapping(&target_key, 4096).unwrap();

        let mut transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        transaction.resize(4 + Kind::Transaction.payload_size(), 0);
        transaction[4..8].copy_from_slice(&1u32.to_le_bytes());
        transaction[20..24].copy_from_slice(&44u32.to_le_bytes());
        let mut sender_thread = thread::Thread::default();
        let mut no_read = [];
        let sent = registry
            .write_read(
                &sender_key,
                &mut sender_thread,
                &transaction,
                &mut no_read,
                &mut SnapshotSource(Ok(TransactionSnapshot::capture(b"parcel", &[], 0).unwrap())),
                SenderIdentity {
                    pid: 333,
                    euid: 10_333,
                },
            )
            .unwrap();
        assert_eq!(sent, Consumed { write: 68, read: 0 });

        let mut target_thread = thread::Thread::default();
        target_thread
            .execute_looper(&Kind::EnterLooper.word().to_le_bytes())
            .unwrap();
        let mut owner_work = [0; 40];
        let owner_work_read = registry
            .write_read(
                &target_key,
                &mut target_thread,
                &[],
                &mut owner_work,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(owner_work_read.read, owner_work.len());
        assert_eq!(
            u32::from_le_bytes(owner_work[..4].try_into().unwrap()),
            0x80107207
        );
        let mut output = [0; crate::transaction_wire::RECORD_SIZE];
        let received = registry
            .write_read(
                &target_key,
                &mut target_thread,
                &[],
                &mut output,
                &mut NoTransactions,
                SenderIdentity { pid: 1, euid: 0 },
            )
            .unwrap();
        assert_eq!(received.read, output.len());
        assert_eq!(i32::from_le_bytes(output[28..32].try_into().unwrap()), 333);
        assert_eq!(
            u32::from_le_bytes(output[32..36].try_into().unwrap()),
            10_333
        );
        let address = u64::from_le_bytes(output[52..60].try_into().unwrap());
        let free = [
            Kind::FreeBuffer.word().to_le_bytes().as_slice(),
            address.to_le_bytes().as_slice(),
        ]
        .concat();
        assert_eq!(
            execute(
                &registry,
                &target_key,
                &mut target_thread,
                &free,
                &mut no_read,
            )
            .unwrap(),
            Consumed { write: 12, read: 0 }
        );
        registry.close(&sender_key).unwrap();
        registry.close(&target_key).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn transaction_capture_failure_consumes_nothing_and_publishes_nothing() {
        let registry = Registry::default();
        let (target_key, node) = registered(&registry);
        let mut sender_session = Session::default();
        sender_session
            .retain_transferred(node, Strength::Strong)
            .unwrap();
        let sender_key = registry
            .register(ConnectionOwner::new(sender_session))
            .unwrap();
        registry.install_receive_mapping(&target_key, 4096).unwrap();
        let mut transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        transaction.resize(68, 0);
        transaction[4..8].copy_from_slice(&1u32.to_le_bytes());
        let mut thread = thread::Thread::default();
        let error = registry
            .write_read(
                &sender_key,
                &mut thread,
                &transaction,
                &mut [],
                &mut SnapshotSource(Err("fault")),
                SenderIdentity { pid: 3, euid: 4 },
            )
            .unwrap_err();
        assert_eq!(error.consumed, Consumed::default());
        assert_eq!(error.cause, Cause::TransactionCapture("fault"));
        let mut target_thread = thread::Thread::new(8).unwrap();
        assert_eq!(
            registry
                .deliver_transaction(&target_key, &mut target_thread, &mut [0; 68])
                .unwrap(),
            0
        );
        registry.close(&sender_key).unwrap();
        registry.close(&target_key).unwrap();
    }
}
