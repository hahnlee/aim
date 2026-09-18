//! Process-local ownership between the Binder device and profile authority.
//! Android Binder policy stays in the device/framework; Unix transport stays in
//! `darwin-art-profile`.

#![deny(unsafe_op_in_unsafe_fn)]

mod authority_lifetime;
mod client;
mod descriptor_retention;
mod descriptor_transport;
mod dispatcher;
mod fd_endpoint;
mod routed_write_read;
mod transport;

pub use authority_lifetime::AuthorityLifetime;
pub use client::{Client, Error, ProcessClient, SubmissionOutcome, connect, connect_process};
pub use descriptor_transport::{
    DescriptorBundleApi, DescriptorRetainedApi, DescriptorTransferBinding, ExportedDescriptor,
    RetainedDescriptorLease, RetainedExportedDescriptor,
};
pub use dispatcher::{Dispatcher, ProcessDispatcher};
pub use fd_endpoint::{BinderFdEndpoint, BinderFdError, BrokerApi, OwnerCallbacks};
pub use routed_write_read::{ExecuteError, PhaseMemoryError};
pub use transport::{AuthorityTransport, DescriptorApi, TransportError};

#[cfg(test)]
mod tests {
    use super::*;
    use darwin_art_binder_device::{
        authority_protocol::{
            ConnectionToken, LocalNodeToken, Message, NodeToken, TransactionFailure, TransferToken,
        },
        command::Kind,
        device::{BINDER_SET_CONTEXT_MGR_EXT, Device, ProcessIdentity},
        ioctl::ClientMemory,
        remote_transaction::RemoteTransaction,
        transaction_snapshot::TransactionSnapshot,
        transfer_image::TransferImage,
    };
    use std::{
        collections::BTreeMap,
        sync::{Mutex, mpsc},
    };

    struct FakeTransport {
        connection: ConnectionToken,
        outgoing: mpsc::SyncSender<Message>,
        incoming: Mutex<mpsc::Receiver<Message>>,
    }

    struct Memory(BTreeMap<u64, Vec<u8>>);

    impl ClientMemory for Memory {
        type Error = &'static str;

        fn copy_from(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Self::Error> {
            let source = self.0.get(&address).ok_or("fault")?;
            if source.len() != destination.len() {
                return Err("size");
            }
            destination.copy_from_slice(source);
            Ok(())
        }

        fn check_write(&mut self, address: u64, size: usize) -> Result<(), Self::Error> {
            self.0
                .get(&address)
                .filter(|region| region.len() >= size)
                .map(|_| ())
                .ok_or("fault")
        }

        fn copy_to(&mut self, address: u64, source: &[u8]) -> Result<(), Self::Error> {
            self.0
                .get_mut(&address)
                .and_then(|region| region.get_mut(..source.len()))
                .ok_or("fault")?
                .copy_from_slice(source);
            Ok(())
        }
    }

    impl AuthorityTransport for FakeTransport {
        fn connection(&self) -> ConnectionToken {
            self.connection
        }

        fn android_uid(&self) -> u32 {
            1000
        }

        fn send(&self, message: Message) -> Result<(), TransportError> {
            self.outgoing
                .send(message)
                .map_err(|_| TransportError("outgoing closed".into()))
        }

        fn receive(&self) -> Result<Message, TransportError> {
            self.incoming
                .lock()
                .map_err(|_| TransportError("incoming poisoned".into()))?
                .recv()
                .map_err(|_| TransportError("incoming closed".into()))
        }

        fn deposit_transfer(
            &self,
            _: TransferToken,
            _: &TransferImage,
        ) -> Result<(), TransportError> {
            Ok(())
        }

        fn take_transfer(
            &self,
            _: ConnectionToken,
            _: TransferToken,
        ) -> Result<TransferImage, TransportError> {
            Err(TransportError("test has no inbound transfer".into()))
        }
    }

    fn endpoint() -> (
        Client<FakeTransport>,
        Dispatcher<FakeTransport>,
        mpsc::Receiver<Message>,
        mpsc::SyncSender<Message>,
    ) {
        let (outgoing_tx, outgoing_rx) = mpsc::sync_channel(4);
        let (incoming_tx, incoming_rx) = mpsc::sync_channel(4);
        let transport = FakeTransport {
            connection: ConnectionToken::from_nonzero(4).unwrap(),
            outgoing: outgoing_tx,
            incoming: Mutex::new(incoming_rx),
        };
        let device = Device::default()
            .open(ProcessIdentity::new(90, 1000).unwrap())
            .unwrap();
        let (client, dispatcher) = Client::new(transport, device);
        (client, dispatcher, outgoing_rx, incoming_tx)
    }

    #[test]
    fn sole_dispatcher_demultiplexes_concurrent_responses_in_wire_order() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let connection = ConnectionToken::from_nonzero(4).unwrap();
        let local = LocalNodeToken::from_nonzero(8).unwrap();
        let published = NodeToken::new(connection, local);
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let response = match outgoing.recv().unwrap() {
                    Message::PublishNode { local } => Message::NodePublished {
                        node: NodeToken::new(connection, local),
                    },
                    Message::GetContextManager => Message::ContextManagerFound {
                        node: Some(published),
                    },
                    other => panic!("unexpected request: {other:?}"),
                };
                incoming.send(response).unwrap();
            }
        });
        let dispatcher = std::thread::spawn(move || {
            dispatcher.dispatch_next().unwrap();
            dispatcher.dispatch_next().unwrap();
        });
        let first_client = client.clone();
        let first = std::thread::spawn(move || first_client.publish_node(local).unwrap());
        let second_client = client.clone();
        let second = std::thread::spawn(move || second_client.get_context_manager().unwrap());
        assert_eq!(first.join().unwrap(), published);
        assert_eq!(second.join().unwrap(), Some(published));
        server.join().unwrap();
        dispatcher.join().unwrap();
    }

    #[test]
    fn mismatched_response_fails_waiter_instead_of_cross_delivery() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let local = LocalNodeToken::from_nonzero(9).unwrap();
        let waiter = std::thread::spawn(move || client.publish_node(local));
        assert!(matches!(
            outgoing.recv().unwrap(),
            Message::PublishNode { .. }
        ));
        incoming
            .send(Message::ContextManagerFound { node: None })
            .unwrap();
        assert!(matches!(
            dispatcher.dispatch_next(),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(waiter.join().unwrap(), Err(Error::Protocol(_))));
    }

    #[test]
    fn context_manager_ioctl_commits_only_after_authority_ack_and_aborts_on_failure() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let mut object = [0_u8; 24];
        object[..4].copy_from_slice(
            &darwin_art_binder_device::objects::Kind::Binder
                .tag()
                .to_le_bytes(),
        );
        object[8..16].copy_from_slice(&0x1111_u64.to_le_bytes());
        object[16..24].copy_from_slice(&0x2222_u64.to_le_bytes());

        let first_client = client.clone();
        let first_object = object;
        let first = std::thread::spawn(move || {
            first_client.set_context_manager_control(BINDER_SET_CONTEXT_MGR_EXT, &first_object)
        });
        let Message::SetContextManager { .. } = outgoing
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("context-manager reservation did not reach authority")
        };
        incoming
            .send(Message::ContextManagerFound { node: None })
            .unwrap();
        assert!(matches!(
            dispatcher.dispatch_next(),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(first.join().unwrap(), Err(Error::Protocol(_))));

        // Protocol mismatch poisons the authority channel. A retry must abort
        // its local reservation, but cannot send on that closed channel.
        for _ in 0..2 {
            assert!(matches!(
                client.set_context_manager_control(BINDER_SET_CONTEXT_MGR_EXT, &object),
                Err(Error::Closed)
            ));
            assert!(matches!(
                outgoing.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
        // Successful authority ACK is a separate fresh connection, not a
        // fabricated recovery of the poisoned protocol stream.
        let (client, dispatcher, outgoing, incoming) = endpoint();

        let second_client = client.clone();
        let second_object = object;
        let second = std::thread::spawn(move || {
            second_client.set_context_manager_control(BINDER_SET_CONTEXT_MGR_EXT, &second_object)
        });
        let Message::SetContextManager { local } = outgoing
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
        else {
            panic!("fresh context-manager reservation did not reach authority")
        };
        let node = NodeToken::new(ConnectionToken::from_nonzero(4).unwrap(), local);
        incoming.send(Message::ContextManagerSet { node }).unwrap();
        dispatcher.dispatch_next().unwrap();
        assert_eq!(second.join().unwrap().unwrap(), node);

        assert!(matches!(
            client.set_context_manager_control(BINDER_SET_CONTEXT_MGR_EXT, &object),
            Err(Error::Device(_))
        ));
    }

    #[test]
    fn accepted_call_commits_before_following_target_death_dispatch() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let target = NodeToken::new(
            ConnectionToken::from_nonzero(7).unwrap(),
            LocalNodeToken::from_nonzero(3).unwrap(),
        );
        let call =
            darwin_art_binder_device::authority_protocol::CallToken::from_nonzero(44).unwrap();
        let sender = client.clone();
        let submitter = std::thread::spawn(move || {
            let snapshot =
                darwin_art_binder_device::transaction_snapshot::TransactionSnapshot::capture(
                    b"request",
                    &[],
                    0,
                )
                .unwrap();
            sender.submit_transaction(12, target, &snapshot, &[], 9, 0)
        });
        assert!(matches!(
            outgoing.recv().unwrap(),
            Message::RouteTransaction {
                caller_thread: 12,
                code: 9,
                flags: 0,
                ..
            }
        ));
        incoming
            .send(Message::RouteAccepted { call: Some(call) })
            .unwrap();
        incoming
            .send(Message::TargetDead {
                call,
                target_thread: 12,
            })
            .unwrap();
        dispatcher.dispatch_next().unwrap();
        dispatcher.dispatch_next().unwrap();
        assert_eq!(
            submitter.join().unwrap().unwrap(),
            SubmissionOutcome::Accepted(Some(call))
        );
    }

    #[test]
    fn rejected_route_returns_binder_error_without_closing_authority() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let target = NodeToken::new(
            ConnectionToken::from_nonzero(7).unwrap(),
            LocalNodeToken::from_nonzero(3).unwrap(),
        );
        let sender = client.clone();
        let submitter = std::thread::spawn(move || {
            let snapshot = TransactionSnapshot::capture(b"request", &[], 0).unwrap();
            sender.submit_transaction(12, target, &snapshot, &[], 9, 0)
        });
        assert!(matches!(
            outgoing.recv().unwrap(),
            Message::RouteTransaction {
                caller_thread: 12,
                ..
            }
        ));
        incoming
            .send(Message::RouteRejected {
                reason: TransactionFailure::DeadReply,
            })
            .unwrap();
        dispatcher.dispatch_next().unwrap();
        assert_eq!(
            submitter.join().unwrap().unwrap(),
            SubmissionOutcome::Rejected(TransactionFailure::DeadReply)
        );

        let mut memory = Memory(BTreeMap::from([(0x2100, vec![0; 4])]));
        let mut header = [0_u8; 48];
        header[24..32].copy_from_slice(&4_u64.to_le_bytes());
        header[40..48].copy_from_slice(&0x2100_u64.to_le_bytes());
        client
            .execute_write_read(12, &mut header, &mut memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(memory.0[&0x2100][..4].try_into().unwrap()),
            darwin_art_binder_device::thread::BR_DEAD_REPLY
        );

        let requester = client.clone();
        let later = std::thread::spawn(move || requester.get_context_manager());
        assert_eq!(outgoing.recv().unwrap(), Message::GetContextManager);
        incoming
            .send(Message::ContextManagerFound { node: None })
            .unwrap();
        dispatcher.dispatch_next().unwrap();
        assert_eq!(later.join().unwrap().unwrap(), None);
    }

    #[test]
    fn authority_eof_terminates_accepted_call_and_rejects_later_requests() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let lifetime = client.authority_lifetime();
        assert!(lifetime.live());
        let target = NodeToken::new(
            ConnectionToken::from_nonzero(7).unwrap(),
            LocalNodeToken::from_nonzero(3).unwrap(),
        );
        let call =
            darwin_art_binder_device::authority_protocol::CallToken::from_nonzero(45).unwrap();
        let sender = client.clone();
        let submitter = std::thread::spawn(move || {
            let snapshot = TransactionSnapshot::capture(b"request", &[], 0).unwrap();
            sender.submit_transaction(12, target, &snapshot, &[], 9, 0)
        });
        assert!(matches!(
            outgoing.recv().unwrap(),
            Message::RouteTransaction {
                caller_thread: 12,
                ..
            }
        ));
        incoming
            .send(Message::RouteAccepted { call: Some(call) })
            .unwrap();
        dispatcher.dispatch_next().unwrap();
        assert_eq!(
            submitter.join().unwrap().unwrap(),
            SubmissionOutcome::Accepted(Some(call))
        );

        drop(incoming);
        assert!(matches!(
            dispatcher.dispatch_next(),
            Err(Error::Transport(_))
        ));
        assert!(matches!(client.get_context_manager(), Err(Error::Closed)));
        assert!(!lifetime.live());

        let mut memory = Memory(BTreeMap::from([(0x2200, vec![0; 4])]));
        let mut header = [0_u8; 48];
        header[24..32].copy_from_slice(&4_u64.to_le_bytes());
        header[40..48].copy_from_slice(&0x2200_u64.to_le_bytes());
        client
            .execute_write_read(12, &mut header, &mut memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(memory.0[&0x2200][..4].try_into().unwrap()),
            darwin_art_binder_device::thread::BR_TRANSACTION_COMPLETE
        );
        header[32..40].fill(0);
        client
            .execute_write_read(12, &mut header, &mut memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(memory.0[&0x2200][..4].try_into().unwrap()),
            darwin_art_binder_device::thread::BR_DEAD_REPLY
        );
    }

    #[test]
    fn authority_lifetime_explicit_close_is_sticky_across_retained_clones() {
        let (client, _dispatcher, outgoing, _incoming) = endpoint();
        let lifetime = client.authority_lifetime();
        let copy = lifetime.clone();
        assert!(copy.live());
        client.close_authority().unwrap();
        assert_eq!(outgoing.recv().unwrap(), Message::CloseConnection);
        assert!(!lifetime.live());
        assert!(!copy.live());
        assert!(!client.authority_lifetime().live());
    }

    #[test]
    fn authority_lifetime_does_not_keep_actual_connection_owner_alive() {
        let (client, dispatcher, _outgoing, _incoming) = endpoint();
        let lifetime = client.authority_lifetime();
        drop(client);
        assert!(lifetime.live()); // Sole dispatcher still owns the connection.
        drop(dispatcher);
        assert!(!lifetime.live());
    }

    #[test]
    fn original_write_read_routes_remote_handle_without_holding_device_lock() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let target = NodeToken::new(
            ConnectionToken::from_nonzero(7).unwrap(),
            LocalNodeToken::from_nonzero(3).unwrap(),
        );
        assert_eq!(
            client
                .device()
                .install_remote_context_manager(
                    target,
                    darwin_art_binder_device::reference_table::Strength::Strong,
                )
                .unwrap(),
            0
        );
        let server = std::thread::spawn(move || {
            assert!(matches!(
                outgoing.recv().unwrap(),
                Message::RouteTransaction {
                    target: seen,
                    caller_thread: 14,
                    code: 77,
                    flags: 1,
                    ..
                } if seen == target
            ));
            incoming
                .send(Message::RouteAccepted { call: None })
                .unwrap();
        });
        let dispatcher = std::thread::spawn(move || dispatcher.dispatch_next().unwrap());

        let mut write = Kind::EnterLooper.word().to_le_bytes().to_vec();
        let mut transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        transaction.resize(4 + Kind::Transaction.payload_size(), 0);
        transaction[20..24].copy_from_slice(&77_u32.to_le_bytes());
        transaction[24..28].copy_from_slice(&1_u32.to_le_bytes());
        write.extend_from_slice(&transaction);
        let mut memory = Memory(BTreeMap::from([(0x1000, write), (0x2000, vec![0; 4])]));
        let mut header = [0_u8; 48];
        header[..8].copy_from_slice(&72_u64.to_le_bytes());
        header[16..24].copy_from_slice(&0x1000_u64.to_le_bytes());
        header[24..32].copy_from_slice(&4_u64.to_le_bytes());
        header[40..48].copy_from_slice(&0x2000_u64.to_le_bytes());
        client
            .execute_write_read(14, &mut header, &mut memory)
            .unwrap();
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 72);
        assert_eq!(u64::from_le_bytes(header[32..40].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(memory.0[&0x2000][..4].try_into().unwrap()),
            darwin_art_binder_device::thread::BR_TRANSACTION_COMPLETE
        );
        server.join().unwrap();
        dispatcher.join().unwrap();
    }

    #[test]
    fn original_remote_reply_commits_incoming_stack_only_after_authority_ack() {
        let (client, dispatcher, outgoing, incoming) = endpoint();
        let mut local_node = [0_u8; 24];
        local_node[..4].copy_from_slice(
            &darwin_art_binder_device::objects::Kind::Binder
                .tag()
                .to_le_bytes(),
        );
        local_node[8..16].copy_from_slice(&0x1111_u64.to_le_bytes());
        local_node[16..24].copy_from_slice(&0x2222_u64.to_le_bytes());
        client
            .device()
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut local_node)
            .unwrap();
        client.device().map_receive::<()>(4096).unwrap();
        let call =
            darwin_art_binder_device::authority_protocol::CallToken::from_nonzero(55).unwrap();
        let snapshot = TransactionSnapshot::capture(b"request", &[], 0).unwrap();
        let image = TransferImage::capture(&snapshot, &[]).unwrap();
        client
            .device()
            .deliver_remote_transaction(
                &image,
                ConnectionToken::from_nonzero(4).unwrap(),
                RemoteTransaction {
                    call: Some(call),
                    sender: ConnectionToken::from_nonzero(9).unwrap(),
                    sender_pid: 101,
                    sender_euid: 10_101,
                    target: LocalNodeToken::from_nonzero(1).unwrap(),
                    code: 3,
                    flags: 0,
                },
            )
            .unwrap();
        let mut receive_memory = Memory(BTreeMap::from([
            (0x3000, Kind::EnterLooper.word().to_le_bytes().to_vec()),
            (
                0x3100,
                vec![0; darwin_art_binder_device::transaction_wire::RECORD_SIZE],
            ),
        ]));
        let mut receive_header = [0_u8; 48];
        receive_header[..8].copy_from_slice(&4_u64.to_le_bytes());
        receive_header[16..24].copy_from_slice(&0x3000_u64.to_le_bytes());
        receive_header[24..32].copy_from_slice(
            &(darwin_art_binder_device::transaction_wire::RECORD_SIZE as u64).to_le_bytes(),
        );
        receive_header[40..48].copy_from_slice(&0x3100_u64.to_le_bytes());
        client
            .device()
            .execute_write_read(16, &mut receive_header, &mut receive_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(receive_memory.0[&0x3100][..4].try_into().unwrap()),
            darwin_art_binder_device::transaction_wire::BR_TRANSACTION
        );

        let server = std::thread::spawn(move || {
            assert!(matches!(
                outgoing.recv().unwrap(),
                Message::CompleteReply {
                    call: seen,
                    code: 88,
                    flags: 0,
                    ..
                } if seen == call
            ));
            incoming.send(Message::ReplyAccepted { call }).unwrap();
        });
        let dispatcher = std::thread::spawn(move || dispatcher.dispatch_next().unwrap());
        let mut reply = Kind::Reply.word().to_le_bytes().to_vec();
        reply.resize(4 + Kind::Reply.payload_size(), 0);
        reply[20..24].copy_from_slice(&88_u32.to_le_bytes());
        let mut reply_memory = Memory(BTreeMap::from([(0x4000, reply), (0x4100, vec![0; 4])]));
        let mut reply_header = [0_u8; 48];
        reply_header[..8].copy_from_slice(&68_u64.to_le_bytes());
        reply_header[16..24].copy_from_slice(&0x4000_u64.to_le_bytes());
        reply_header[24..32].copy_from_slice(&4_u64.to_le_bytes());
        reply_header[40..48].copy_from_slice(&0x4100_u64.to_le_bytes());
        client
            .execute_write_read(16, &mut reply_header, &mut reply_memory)
            .unwrap();
        assert_eq!(
            u64::from_le_bytes(reply_header[8..16].try_into().unwrap()),
            68
        );
        assert_eq!(
            u64::from_le_bytes(reply_header[32..40].try_into().unwrap()),
            4
        );
        assert_eq!(
            u32::from_le_bytes(reply_memory.0[&0x4100][..4].try_into().unwrap()),
            darwin_art_binder_device::thread::BR_TRANSACTION_COMPLETE
        );
        server.join().unwrap();
        dispatcher.join().unwrap();
    }

    #[test]
    fn routed_blocking_read_times_out_then_observes_device_work_without_replaying_write() {
        let (client, _dispatcher, _outgoing, _incoming) = endpoint();
        let mut local_node = [0_u8; 24];
        local_node[..4].copy_from_slice(
            &darwin_art_binder_device::objects::Kind::Binder
                .tag()
                .to_le_bytes(),
        );
        client
            .device()
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut local_node)
            .unwrap();
        client.device().map_receive::<()>(4096).unwrap();
        let mut join_memory = Memory(BTreeMap::from([(
            0x5000,
            Kind::EnterLooper.word().to_le_bytes().to_vec(),
        )]));
        let mut join = [0_u8; 48];
        join[..8].copy_from_slice(&4_u64.to_le_bytes());
        join[16..24].copy_from_slice(&0x5000_u64.to_le_bytes());
        client
            .execute_write_read(18, &mut join, &mut join_memory)
            .unwrap();

        let mut memory = Memory(BTreeMap::from([(
            0x5100,
            vec![0; darwin_art_binder_device::transaction_wire::RECORD_SIZE],
        )]));
        let mut header = [0_u8; 48];
        header[24..32].copy_from_slice(
            &(darwin_art_binder_device::transaction_wire::RECORD_SIZE as u64).to_le_bytes(),
        );
        header[40..48].copy_from_slice(&0x5100_u64.to_le_bytes());
        assert!(matches!(
            client.execute_write_read_blocking(
                18,
                &mut header,
                &mut memory,
                std::time::Duration::MAX
            ),
            Err(ExecuteError::Read(
                darwin_art_binder_device::device::Error::InvalidArgument
            ))
        ));
        assert_eq!(
            client
                .execute_write_read_blocking(
                    18,
                    &mut header,
                    &mut memory,
                    std::time::Duration::ZERO,
                )
                .unwrap(),
            darwin_art_binder_device::device::WriteReadStatus::TimedOut
        );
        let snapshot = TransactionSnapshot::capture(b"wake", &[], 0).unwrap();
        let image = TransferImage::capture(&snapshot, &[]).unwrap();
        client
            .device()
            .deliver_remote_transaction(
                &image,
                ConnectionToken::from_nonzero(4).unwrap(),
                RemoteTransaction {
                    call: None,
                    sender: ConnectionToken::from_nonzero(9).unwrap(),
                    sender_pid: 0,
                    sender_euid: 10_101,
                    target: LocalNodeToken::from_nonzero(1).unwrap(),
                    code: 4,
                    flags: 1,
                },
            )
            .unwrap();
        assert_eq!(
            client
                .execute_write_read_blocking(
                    18,
                    &mut header,
                    &mut memory,
                    std::time::Duration::from_secs(1),
                )
                .unwrap(),
            darwin_art_binder_device::device::WriteReadStatus::Completed
        );
        assert_eq!(
            u32::from_le_bytes(memory.0[&0x5100][..4].try_into().unwrap()),
            darwin_art_binder_device::transaction_wire::BR_TRANSACTION
        );
        assert_eq!(u64::from_le_bytes(join[8..16].try_into().unwrap()), 4);
        let address = u64::from_le_bytes(memory.0[&0x5100][52..60].try_into().unwrap());
        let mut free = Kind::FreeBuffer.word().to_le_bytes().to_vec();
        free.extend_from_slice(&address.to_le_bytes());
        memory.0.insert(0x5400, free);
        let mut release = [0_u8; 48];
        release[..8].copy_from_slice(&12_u64.to_le_bytes());
        release[16..24].copy_from_slice(&0x5400_u64.to_le_bytes());
        client
            .execute_write_read(18, &mut release, &mut memory)
            .unwrap();
        // Exercise the production untimed path. The write phase joins once;
        // wakeup processing must not replay EnterLooper after queued read work.
        let waiting_client = client.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut memory = Memory(BTreeMap::from([
                (0x5200, Kind::EnterLooper.word().to_le_bytes().to_vec()),
                (
                    0x5300,
                    vec![0; darwin_art_binder_device::transaction_wire::RECORD_SIZE],
                ),
            ]));
            let mut header = [0_u8; 48];
            header[..8].copy_from_slice(&4_u64.to_le_bytes());
            header[16..24].copy_from_slice(&0x5200_u64.to_le_bytes());
            header[24..32].copy_from_slice(
                &(darwin_art_binder_device::transaction_wire::RECORD_SIZE as u64).to_le_bytes(),
            );
            header[40..48].copy_from_slice(&0x5300_u64.to_le_bytes());
            let result = waiting_client.execute_write_read_indefinite(19, &mut header, &mut memory);
            done_tx
                .send((result, header, memory.0.remove(&0x5300).unwrap()))
                .unwrap();
        });
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        client
            .device()
            .deliver_remote_transaction(
                &image,
                ConnectionToken::from_nonzero(4).unwrap(),
                RemoteTransaction {
                    call: None,
                    sender: ConnectionToken::from_nonzero(9).unwrap(),
                    sender_pid: 0,
                    sender_euid: 10_101,
                    target: LocalNodeToken::from_nonzero(1).unwrap(),
                    code: 5,
                    flags: 1,
                },
            )
            .unwrap();
        let (result, header, bytes) = done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            result.unwrap(),
            darwin_art_binder_device::device::WriteReadStatus::Completed
        );
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(bytes[..4].try_into().unwrap()),
            darwin_art_binder_device::transaction_wire::BR_TRANSACTION
        );
        waiter.join().unwrap();
    }
}
