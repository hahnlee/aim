//! Binder device/process ownership above the command core and below Bionic FD
//! publication. Framework service policy and macOS client-memory access are not
//! owned here.

use crate::{
    connection::ConnectionOwner,
    connection_registry::{self, Registry},
    ioctl::{self, ClientMemory},
    security::{AndroidSystemPolicy, ContextManagerPolicy},
    session::Session,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
mod remote_command;
#[cfg(target_os = "macos")]
mod remote_delivery;
#[cfg(target_os = "macos")]
mod remote_references;
#[cfg(target_os = "macos")]
mod remote_reply;
#[cfg(target_os = "macos")]
mod remote_submission;

pub const PROTOCOL_VERSION: u32 = 8;
pub const BINDER_WRITE_READ: u64 = 0xc030_6201;
pub const BINDER_SET_MAX_THREADS: u64 = 0x4004_6205;
pub const BINDER_VERSION: u64 = 0xc004_6209;
pub const BINDER_SET_CONTEXT_MGR: u64 = 0x4004_6207;
pub const BINDER_SET_CONTEXT_MGR_EXT: u64 = 0x4018_620d;

#[cfg(target_os = "macos")]
pub use remote_command::{RemoteCommand, RemoteCommandError, RemoteReplyCommand};

pub struct Device {
    registry: Arc<Registry>,
    context_policy: Arc<dyn ContextManagerPolicy>,
}

pub struct OpenConnection {
    registry: Arc<Registry>,
    key: Option<connection_registry::Key>,
    identity: ProcessIdentity,
    context_policy: Arc<dyn ContextManagerPolicy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextManagerReservation(connection_registry::context_manager::Reservation);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pid: i32,
    euid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkWait {
    Changed,
    Closed,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteReadStatus {
    Completed,
    Closed,
    TimedOut,
}

impl ProcessIdentity {
    pub fn new(pid: i32, euid: u32) -> Option<Self> {
        (pid > 0).then_some(Self { pid, euid })
    }

    pub fn pid(self) -> i32 {
        self.pid
    }

    pub fn euid(self) -> u32 {
        self.euid
    }
}

#[derive(Debug)]
pub enum Error<E> {
    Closed,
    Poisoned,
    Registry(connection_registry::Error),
    ContextManager(connection_registry::context_manager::Error),
    Thread(crate::thread::Error),
    InvalidArgument,
    UnsupportedRequest(u64),
    Ioctl(ioctl::Error<E>),
}

impl Default for Device {
    fn default() -> Self {
        Self::with_policy(Arc::new(AndroidSystemPolicy))
    }
}

impl Device {
    pub fn with_policy(context_policy: Arc<dyn ContextManagerPolicy>) -> Self {
        Self {
            registry: Arc::new(Registry::default()),
            context_policy,
        }
    }

    pub fn open(
        &self,
        identity: ProcessIdentity,
    ) -> Result<OpenConnection, connection_registry::Error> {
        let key = self
            .registry
            .register(ConnectionOwner::new(Session::default()))?;
        Ok(OpenConnection {
            registry: Arc::clone(&self.registry),
            key: Some(key),
            identity,
            context_policy: Arc::clone(&self.context_policy),
        })
    }
}

impl OpenConnection {
    /// Take this generation before checking all eligible queues. Waiting on it
    /// after an empty read cannot miss a concurrent work publication.
    pub fn work_generation<E>(&self) -> Result<u64, Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry.work_generation(key).map_err(Error::Registry)
    }

    pub fn wait_for_work<E>(
        &self,
        generation: u64,
        timeout: Duration,
    ) -> Result<WorkWait, Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .wait_for_work(key, generation, timeout)
            .map(|outcome| match outcome {
                crate::work_signal::WaitOutcome::Changed => WorkWait::Changed,
                crate::work_signal::WaitOutcome::Closed => WorkWait::Closed,
                crate::work_signal::WaitOutcome::TimedOut => WorkWait::TimedOut,
            })
            .map_err(Error::Registry)
    }

    pub fn wait_for_work_for_thread<E>(
        &self,
        thread_id: u64,
        generation: u64,
        timeout: Duration,
    ) -> Result<WorkWait, Error<E>> {
        self.wait_for_work_thread(thread_id, generation, Some(timeout))
    }

    pub fn wait_for_work_for_thread_indefinite<E>(
        &self,
        thread_id: u64,
        generation: u64,
    ) -> Result<WorkWait, Error<E>> {
        self.wait_for_work_thread(thread_id, generation, None)
    }

    fn wait_for_work_thread<E>(
        &self,
        thread_id: u64,
        generation: u64,
        timeout: Option<Duration>,
    ) -> Result<WorkWait, Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let registration = self
            .registry
            .prepare_thread_work_wait(key, thread_id)
            .map_err(Error::Registry)?;
        let waited = match registration.wait(generation, timeout) {
            crate::work_signal::WaitOutcome::Changed => WorkWait::Changed,
            crate::work_signal::WaitOutcome::Closed => WorkWait::Closed,
            crate::work_signal::WaitOutcome::TimedOut => WorkWait::TimedOut,
        };
        registration
            .finish()
            .map_err(|error| Error::Registry(connection_registry::Error::Connection(error)))?;
        Ok(waited)
    }

    /// Back the original ProcessState mmap with one read-only client view. The
    /// writable alias remains private to the binder_proc transaction owner.
    pub fn map_receive<E>(&self, capacity: usize) -> Result<usize, Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .install_receive_mapping(key, capacity)
            .map_err(Error::Registry)
    }

    pub fn duplicate_receive_mapping<E>(&self) -> Result<std::os::fd::OwnedFd, Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .duplicate_receive_mapping(key)
            .map_err(Error::Registry)
    }

    /// Execute a non-`BINDER_WRITE_READ` ioctl over an already fault-safely
    /// copied argument. The platform adapter owns copying this small fixed ABI
    /// record back to the guest.
    pub fn execute_control<E>(&self, request: u64, argument: &mut [u8]) -> Result<(), Error<E>> {
        self.key.as_ref().ok_or(Error::Closed)?;
        match request {
            BINDER_VERSION if argument.len() == 4 => {
                argument.copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
                Ok(())
            }
            BINDER_SET_MAX_THREADS if argument.len() == 4 => {
                let value = u32::from_le_bytes(argument.try_into().unwrap());
                let key = self.key.as_ref().ok_or(Error::Closed)?;
                self.registry
                    .set_max_threads(key, value)
                    .map_err(Error::Registry)
            }
            BINDER_SET_CONTEXT_MGR if argument.len() == 4 => {
                let (reservation, _) = self.begin_context_manager_control(request, argument)?;
                self.commit_context_manager(reservation)
            }
            BINDER_SET_CONTEXT_MGR_EXT if argument.len() == 24 => {
                let (reservation, _) = self.begin_context_manager_control(request, argument)?;
                self.commit_context_manager(reservation)
            }
            BINDER_VERSION
            | BINDER_SET_MAX_THREADS
            | BINDER_SET_CONTEXT_MGR
            | BINDER_SET_CONTEXT_MGR_EXT => Err(Error::InvalidArgument),
            _ => Err(Error::UnsupportedRequest(request)),
        }
    }

    pub fn begin_context_manager_control<E>(
        &self,
        request: u64,
        argument: &[u8],
    ) -> Result<
        (
            ContextManagerReservation,
            crate::authority_protocol::LocalNodeToken,
        ),
        Error<E>,
    > {
        match request {
            BINDER_SET_CONTEXT_MGR if argument.len() == 4 => {
                let data = context_manager_object(0);
                let objects = crate::objects::validate(&data, &0u64.to_le_bytes())
                    .expect("internal context-manager object is valid");
                self.begin_context_manager(&objects[0])
            }
            BINDER_SET_CONTEXT_MGR_EXT if argument.len() == 24 => {
                let objects = crate::objects::validate(argument, &0u64.to_le_bytes())
                    .map_err(|_| Error::InvalidArgument)?;
                self.begin_context_manager(&objects[0])
            }
            _ => Err(Error::InvalidArgument),
        }
    }

    fn begin_context_manager<E>(
        &self,
        object: &crate::objects::Object<'_>,
    ) -> Result<
        (
            ContextManagerReservation,
            crate::authority_protocol::LocalNodeToken,
        ),
        Error<E>,
    > {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let identity = self.identity;
        let policy = Arc::clone(&self.context_policy);
        self.registry
            .begin_context_manager(key, object, |_| {
                policy.authorize(identity)?;
                Ok(identity.euid())
            })
            .map(|(reservation, local)| (ContextManagerReservation(reservation), local))
            .map_err(Error::ContextManager)
    }

    pub fn commit_context_manager<E>(
        &self,
        reservation: ContextManagerReservation,
    ) -> Result<(), Error<E>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .commit_context_manager(key, reservation.0)
            .map_err(Error::ContextManager)
    }

    pub fn abort_context_manager(&self, reservation: ContextManagerReservation) {
        if let Some(key) = self.key.as_ref() {
            self.registry.abort_context_manager(key, reservation.0);
        }
    }

    /// Execute the original IPCThreadState write/read request for the calling
    /// host thread. `thread_id` comes from the authenticated host runtime, never
    /// from a Binder transaction header.
    pub fn execute_write_read<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
    ) -> Result<(), Error<M::Error>> {
        self.execute_write_read_once(thread_id, header, memory)
    }

    /// Execute writes once, then wait and retry only the read half. `timeout`
    /// bounds host interruption/testing. The production routed guest-FD adapter
    /// uses its explicit indefinite read API instead of an idle deadline.
    pub fn execute_write_read_blocking<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
        timeout: Duration,
    ) -> Result<WriteReadStatus, Error<M::Error>> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(Error::InvalidArgument)?;
        let request = ioctl::Request::decode(header)
            .map_err(|error| Error::Ioctl(ioctl::Error::Header(error)))?;
        let mut generation = self.work_generation()?;
        self.execute_write_read_once(thread_id, header, memory)?;
        if request.read.size == 0 || consumed_read(header) != 0 {
            return Ok(WriteReadStatus::Completed);
        }

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.wait_for_work_for_thread(thread_id, generation, remaining)? {
                WorkWait::Closed => return Ok(WriteReadStatus::Closed),
                WorkWait::TimedOut => return Ok(WriteReadStatus::TimedOut),
                WorkWait::Changed => {}
            }
            generation = self.work_generation()?;
            let mut read_only = [0; ioctl::WRITE_READ_SIZE];
            read_only[24..32].copy_from_slice(&(request.read.size as u64).to_le_bytes());
            read_only[40..48].copy_from_slice(&request.read.address.to_le_bytes());
            let result = self.execute_write_read_once(thread_id, &mut read_only, memory);
            header[32..40].copy_from_slice(&read_only[32..40]);
            result?;
            if consumed_read(&read_only) != 0 {
                return Ok(WriteReadStatus::Completed);
            }
        }
    }

    fn execute_write_read_once<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
    ) -> Result<(), Error<M::Error>> {
        if thread_id == 0 {
            return Err(Error::InvalidArgument);
        }
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let thread = self
            .registry
            .thread(key, thread_id)
            .map_err(Error::Registry)?;
        let mut thread = thread.lock().map_err(|_| Error::Poisoned)?;
        ioctl::execute(
            &self.registry,
            key,
            &mut thread,
            header,
            memory,
            crate::write_read::SenderIdentity {
                pid: self.identity.pid,
                euid: self.identity.euid,
            },
        )
        .map_err(Error::Ioctl)
    }

    pub fn max_threads(&self) -> Result<Option<u32>, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry.max_threads(key).map_err(Error::Registry)
    }

    pub fn close(mut self) -> Result<(), connection_registry::Error> {
        let key = self.key.take().expect("owned connection closes once");
        self.registry.close(&key)
    }
}

fn consumed_read(header: &[u8]) -> u64 {
    u64::from_le_bytes(header[32..40].try_into().expect("validated ioctl header"))
}

fn context_manager_object(flags: u32) -> [u8; 24] {
    let mut data = [0; 24];
    data[..4].copy_from_slice(&crate::objects::Kind::Binder.tag().to_le_bytes());
    data[4..8].copy_from_slice(&flags.to_le_bytes());
    data
}

impl Drop for OpenConnection {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let _ = self.registry.close(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{command::Kind, write_read};
    use std::collections::BTreeMap;

    fn open() -> OpenConnection {
        Device::default()
            .open(ProcessIdentity::new(123, 10_123).unwrap())
            .unwrap()
    }

    #[derive(Default)]
    struct Memory(BTreeMap<u64, Vec<u8>>);
    impl ClientMemory for Memory {
        type Error = ();

        fn copy_from(&mut self, address: u64, output: &mut [u8]) -> Result<(), ()> {
            output.copy_from_slice(self.0.get(&address).ok_or(())?);
            Ok(())
        }
        fn check_write(&mut self, address: u64, size: usize) -> Result<(), ()> {
            (self.0.get(&address).ok_or(())?.len() >= size)
                .then_some(())
                .ok_or(())
        }
        fn copy_to(&mut self, address: u64, input: &[u8]) -> Result<(), ()> {
            self.0.get_mut(&address).ok_or(())?[..input.len()].copy_from_slice(input);
            Ok(())
        }
    }

    fn write_read_header(write_address: u64, read_address: u64) -> [u8; 48] {
        write_read_header_sized(write_address, 4, read_address, 40)
    }

    fn write_read_header_sized(
        write_address: u64,
        write_size: usize,
        read_address: u64,
        read_size: usize,
    ) -> [u8; 48] {
        let mut header = [0; 48];
        header[..8].copy_from_slice(&(write_size as u64).to_le_bytes());
        header[16..24].copy_from_slice(&write_address.to_le_bytes());
        header[24..32].copy_from_slice(&(read_size as u64).to_le_bytes());
        header[40..48].copy_from_slice(&read_address.to_le_bytes());
        header
    }

    #[test]
    fn control_abi_and_explicit_close() {
        let connection = open();
        let mut version = [0xff; 4];
        connection
            .execute_control::<std::convert::Infallible>(BINDER_VERSION, &mut version)
            .unwrap();
        assert_eq!(u32::from_le_bytes(version), PROTOCOL_VERSION);
        let mut maximum = 12u32.to_le_bytes();
        connection
            .execute_control::<std::convert::Infallible>(BINDER_SET_MAX_THREADS, &mut maximum)
            .unwrap();
        assert_eq!(connection.max_threads().unwrap(), Some(12));
        let mapping = connection
            .map_receive::<std::convert::Infallible>(4096)
            .unwrap();
        assert_ne!(mapping, 0);
        assert!(matches!(
            connection.map_receive::<std::convert::Infallible>(4096),
            Err(Error::Registry(connection_registry::Error::Connection(
                crate::connection::Error::MappingAlreadyInstalled
            )))
        ));
        connection.close().unwrap();
    }

    #[test]
    fn looper_membership_is_independent_per_host_thread() {
        let connection = open();
        let write_address = 0x1000;
        let read_address = 0x2000;
        let mut memory = Memory(BTreeMap::from([
            (
                write_address,
                Kind::EnterLooper.word().to_le_bytes().to_vec(),
            ),
            (read_address, vec![0; 40]),
        ]));
        let mut first = write_read_header(write_address, read_address);
        connection
            .execute_write_read(101, &mut first, &mut memory)
            .unwrap();
        let mut second = write_read_header(0, read_address);
        second[..8].fill(0);
        assert!(matches!(
            connection.execute_write_read(202, &mut second, &mut memory),
            Err(Error::Ioctl(ioctl::Error::Device(write_read::Error {
                cause: write_read::Cause::Thread(crate::thread::Error::NotEligibleForRead),
                ..
            })))
        ));
        connection.close().unwrap();
    }

    #[test]
    fn blocking_write_read_times_out_without_replaying_writes() {
        let connection = open();
        let mut memory = Memory(BTreeMap::from([
            (0x1000, Kind::EnterLooper.word().to_le_bytes().to_vec()),
            (0x2000, vec![0xcc; crate::transaction_wire::RECORD_SIZE]),
        ]));
        let mut header =
            write_read_header_sized(0x1000, 4, 0x2000, crate::transaction_wire::RECORD_SIZE);
        let before = header;
        assert!(matches!(
            connection.execute_write_read_blocking(7, &mut header, &mut memory, Duration::MAX),
            Err(Error::InvalidArgument)
        ));
        assert_eq!(header, before); // Invalid deadline never executes writes.
        assert_eq!(
            connection
                .execute_write_read_blocking(7, &mut header, &mut memory, Duration::ZERO)
                .unwrap(),
            WriteReadStatus::TimedOut
        );
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(header[32..40].try_into().unwrap()), 0);
        assert_eq!(
            memory.0[&0x2000],
            vec![0xcc; crate::transaction_wire::RECORD_SIZE]
        );
        connection.close().unwrap();
    }

    #[test]
    fn context_manager_and_app_exchange_transaction_only_through_device_abi() {
        let device = Device::default();
        let manager = device
            .open(ProcessIdentity::new(40, 1000).unwrap())
            .unwrap();
        let app = device
            .open(ProcessIdentity::new(50, 10_123).unwrap())
            .unwrap();
        let mut manager_object = context_manager_object(0x100);
        manager_object[8..16].copy_from_slice(&0xabcdu64.to_le_bytes());
        manager_object[16..24].copy_from_slice(&0xdef0u64.to_le_bytes());
        manager
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut manager_object)
            .unwrap();
        assert!(matches!(
            app.execute_control::<()>(BINDER_SET_CONTEXT_MGR, &mut [0; 4]),
            Err(Error::ContextManager(
                connection_registry::context_manager::Error::Busy
            ))
        ));
        manager.map_receive::<()>(4096).unwrap();
        app.map_receive::<()>(4096).unwrap();
        let manager_generation = manager.work_generation::<()>().unwrap();
        let app_generation = app.work_generation::<()>().unwrap();

        let mut transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        transaction.resize(68, 0);
        transaction[20..24].copy_from_slice(&88u32.to_le_bytes());
        transaction[28..32].copy_from_slice(&9999i32.to_le_bytes());
        transaction[32..36].copy_from_slice(&9999u32.to_le_bytes());
        transaction[36..44].copy_from_slice(&24u64.to_le_bytes());
        transaction[44..52].copy_from_slice(&8u64.to_le_bytes());
        transaction[52..60].copy_from_slice(&0x3000u64.to_le_bytes());
        transaction[60..68].copy_from_slice(&0x3100u64.to_le_bytes());
        let mut binder_object = [0; 24];
        binder_object[..4].copy_from_slice(&crate::objects::Kind::Binder.tag().to_le_bytes());
        binder_object[8..16].copy_from_slice(&0x7777u64.to_le_bytes());
        binder_object[16..24].copy_from_slice(&0x8888u64.to_le_bytes());
        let mut app_memory = Memory(BTreeMap::from([
            (0x1000, transaction),
            (0x3000, binder_object.to_vec()),
            (0x3100, 0u64.to_le_bytes().to_vec()),
        ]));
        let mut send = write_read_header_sized(0x1000, 68, 0, 0);
        app.execute_write_read(1, &mut send, &mut app_memory)
            .unwrap();
        assert_eq!(u64::from_le_bytes(send[8..16].try_into().unwrap()), 68);
        assert_eq!(
            manager
                .wait_for_work::<()>(manager_generation, Duration::ZERO)
                .unwrap(),
            WorkWait::Changed
        );
        assert_eq!(
            app.wait_for_work::<()>(app_generation, Duration::ZERO)
                .unwrap(),
            WorkWait::Changed
        );

        let enter = Kind::EnterLooper.word().to_le_bytes().to_vec();
        let mut manager_memory = Memory(BTreeMap::from([
            (0x4000, enter),
            (0x5000, vec![0; crate::transaction_wire::RECORD_SIZE]),
        ]));
        let mut receive =
            write_read_header_sized(0x4000, 4, 0x5000, crate::transaction_wire::RECORD_SIZE);
        manager
            .execute_write_read(2, &mut receive, &mut manager_memory)
            .unwrap();
        let record = &manager_memory.0[&0x5000];
        assert_eq!(
            u32::from_le_bytes(record[..4].try_into().unwrap()),
            crate::transaction_wire::BR_TRANSACTION
        );
        assert_eq!(
            u64::from_le_bytes(record[4..12].try_into().unwrap()),
            0xabcd
        );
        assert_eq!(
            u64::from_le_bytes(record[12..20].try_into().unwrap()),
            0xdef0
        );
        assert_eq!(u32::from_le_bytes(record[20..24].try_into().unwrap()), 88);
        assert_eq!(i32::from_le_bytes(record[28..32].try_into().unwrap()), 50);
        assert_eq!(
            u32::from_le_bytes(record[32..36].try_into().unwrap()),
            10_123
        );

        let address = u64::from_le_bytes(record[52..60].try_into().unwrap());
        // SAFETY: BR_TRANSACTION publishes a live read-only receiver mapping of
        // exactly data_size bytes; BC_FREE_BUFFER has not yet been issued.
        let delivered = unsafe { std::slice::from_raw_parts(address as *const u8, 24) };
        assert_eq!(
            u32::from_le_bytes(delivered[..4].try_into().unwrap()),
            crate::objects::Kind::Handle.tag()
        );
        assert_eq!(u32::from_le_bytes(delivered[8..12].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(delivered[16..24].try_into().unwrap()), 0);

        let mut reply = Kind::Reply.word().to_le_bytes().to_vec();
        reply.resize(68, 0);
        reply[36..44].copy_from_slice(&6u64.to_le_bytes());
        reply[52..60].copy_from_slice(&0x7000u64.to_le_bytes());
        manager_memory.0.insert(0x6800, reply);
        manager_memory.0.insert(0x7000, b"answer".to_vec());
        let mut wrong_thread_reply = write_read_header_sized(0x6800, 68, 0, 0);
        assert!(matches!(
            manager.execute_write_read(3, &mut wrong_thread_reply, &mut manager_memory),
            Err(Error::Ioctl(ioctl::Error::Device(write_read::Error {
                consumed: write_read::Consumed { write: 0, read: 0 },
                cause: write_read::Cause::Thread(crate::thread::Error::InvalidReplyStack),
            })))
        ));
        let mut reply_write = write_read_header_sized(0x6800, 68, 0, 0);
        let reply_generation = app.work_generation::<()>().unwrap();
        manager
            .execute_write_read(2, &mut reply_write, &mut manager_memory)
            .unwrap();
        assert_eq!(
            u64::from_le_bytes(reply_write[8..16].try_into().unwrap()),
            68
        );
        assert_eq!(
            app.wait_for_work::<()>(reply_generation, Duration::ZERO)
                .unwrap(),
            WorkWait::Changed
        );

        manager_memory.0.insert(0x7100, vec![0; 4]);
        let mut reply_completion = write_read_header_sized(0, 0, 0x7100, 4);
        manager
            .execute_write_read(2, &mut reply_completion, &mut manager_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(manager_memory.0[&0x7100][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );

        app_memory
            .0
            .insert(0x7200, vec![0; crate::transaction_wire::RECORD_SIZE]);
        let mut reply_read =
            write_read_header_sized(0, 0, 0x7200, crate::transaction_wire::RECORD_SIZE);
        app.execute_write_read(1, &mut reply_read, &mut app_memory)
            .unwrap();
        assert_eq!(
            u64::from_le_bytes(reply_read[32..40].try_into().unwrap()),
            40
        );
        assert_eq!(
            u32::from_le_bytes(app_memory.0[&0x7200][..4].try_into().unwrap()),
            0x80107207
        );
        // Node-owner notifications are a distinct 40-byte work record and
        // precede the transaction completion on the receiving thread.
        let mut reply_read =
            write_read_header_sized(0, 0, 0x7200, crate::transaction_wire::RECORD_SIZE);
        app.execute_write_read(1, &mut reply_read, &mut app_memory)
            .unwrap();
        assert_eq!(
            u64::from_le_bytes(reply_read[32..40].try_into().unwrap()),
            4
        );
        assert_eq!(
            u32::from_le_bytes(app_memory.0[&0x7200][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );
        let mut reply_read =
            write_read_header_sized(0, 0, 0x7200, crate::transaction_wire::RECORD_SIZE);
        app.execute_write_read(1, &mut reply_read, &mut app_memory)
            .unwrap();
        let reply_record = &app_memory.0[&0x7200];
        assert_eq!(
            u32::from_le_bytes(reply_record[..4].try_into().unwrap()),
            crate::transaction_wire::BR_REPLY
        );
        assert_eq!(
            u64::from_le_bytes(reply_record[4..12].try_into().unwrap()),
            0
        );
        let reply_address = u64::from_le_bytes(reply_record[52..60].try_into().unwrap());
        // SAFETY: caller's BR_REPLY mapping remains live until its exact free.
        assert_eq!(
            unsafe { std::slice::from_raw_parts(reply_address as *const u8, 6) },
            b"answer"
        );
        let mut free_reply = Kind::FreeBuffer.word().to_le_bytes().to_vec();
        free_reply.extend_from_slice(&reply_address.to_le_bytes());
        app_memory.0.insert(0x7300, free_reply);
        let mut release_reply = write_read_header_sized(0x7300, 12, 0, 0);
        app.execute_write_read(1, &mut release_reply, &mut app_memory)
            .unwrap();

        let mut free = Kind::FreeBuffer.word().to_le_bytes().to_vec();
        free.extend_from_slice(&address.to_le_bytes());
        manager_memory.0.insert(0x6000, free);
        let mut release = write_read_header_sized(0x6000, 12, 0, 0);
        manager
            .execute_write_read(2, &mut release, &mut manager_memory)
            .unwrap();
        assert_eq!(u64::from_le_bytes(release[8..16].try_into().unwrap()), 12);

        // If the caller dies after delivery, a legitimate late BC_REPLY is
        // consumed, completes the server's incoming stack and allocates no
        // reply buffer in the vanished process.
        let mut second_send = write_read_header_sized(0x1000, 68, 0, 0);
        app.execute_write_read(1, &mut second_send, &mut app_memory)
            .unwrap();
        let mut second_receive =
            write_read_header_sized(0, 0, 0x5000, crate::transaction_wire::RECORD_SIZE);
        manager
            .execute_write_read(2, &mut second_receive, &mut manager_memory)
            .unwrap();
        app.close().unwrap();
        let mut late_reply = write_read_header_sized(0x6800, 68, 0, 0);
        manager
            .execute_write_read(2, &mut late_reply, &mut manager_memory)
            .unwrap();
        let mut late_completion = write_read_header_sized(0, 0, 0x7100, 4);
        manager
            .execute_write_read(2, &mut late_completion, &mut manager_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(manager_memory.0[&0x7100][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );

        // Conversely, target death unwinds the exact originating
        // binder_thread after that call's ordinary submission completion.
        let second_app = device
            .open(ProcessIdentity::new(60, 10_124).unwrap())
            .unwrap();
        second_app.map_receive::<()>(4096).unwrap();
        let mut empty_transaction = Kind::Transaction.word().to_le_bytes().to_vec();
        empty_transaction.resize(68, 0);
        empty_transaction[20..24].copy_from_slice(&99u32.to_le_bytes());
        let mut second_app_memory = Memory(BTreeMap::from([
            (0x8000, empty_transaction),
            (0x8100, vec![0; 4]),
        ]));
        let mut third_send = write_read_header_sized(0x8000, 68, 0, 0);
        second_app
            .execute_write_read(4, &mut third_send, &mut second_app_memory)
            .unwrap();
        let death_generation = second_app.work_generation::<()>().unwrap();
        manager.close().unwrap();
        assert_eq!(
            second_app
                .wait_for_work::<()>(death_generation, Duration::ZERO)
                .unwrap(),
            WorkWait::Changed
        );
        let mut second_completion = write_read_header_sized(0, 0, 0x8100, 4);
        second_app
            .execute_write_read(4, &mut second_completion, &mut second_app_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(second_app_memory.0[&0x8100][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );
        let mut dead_reply = write_read_header_sized(0, 0, 0x8100, 4);
        second_app
            .execute_write_read(4, &mut dead_reply, &mut second_app_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(second_app_memory.0[&0x8100][..4].try_into().unwrap()),
            crate::thread::BR_DEAD_REPLY
        );
        second_app.close().unwrap();
    }

    #[test]
    fn application_identity_cannot_register_context_manager() {
        let app = Device::default()
            .open(ProcessIdentity::new(50, 10_000).unwrap())
            .unwrap();
        assert!(matches!(
            app.execute_control::<()>(BINDER_SET_CONTEXT_MGR, &mut [0; 4]),
            Err(Error::ContextManager(
                connection_registry::context_manager::Error::Security(libc::EPERM)
            ))
        ));
        app.close().unwrap();
    }

    #[test]
    fn authenticated_remote_image_is_published_to_the_target_binder_proc() {
        let manager = Device::default()
            .open(ProcessIdentity::new(40, 1000).unwrap())
            .unwrap();
        let mut manager_object = context_manager_object(0x100);
        manager_object[8..16].copy_from_slice(&0xabcdu64.to_le_bytes());
        manager_object[16..24].copy_from_slice(&0xdef0u64.to_le_bytes());
        manager
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut manager_object)
            .unwrap();
        manager.map_receive::<()>(4096).unwrap();

        let snapshot =
            crate::transaction_snapshot::TransactionSnapshot::capture(b"remote parcel", &[], 0)
                .unwrap();
        let image = crate::transfer_image::TransferImage::capture(&snapshot, &[]).unwrap();
        manager
            .deliver_remote_transaction(
                &image,
                crate::authority_protocol::ConnectionToken::from_nonzero(1).unwrap(),
                crate::remote_transaction::RemoteTransaction {
                    call: Some(crate::authority_protocol::CallToken::from_nonzero(9).unwrap()),
                    sender: crate::authority_protocol::ConnectionToken::from_nonzero(2).unwrap(),
                    sender_pid: 50,
                    sender_euid: 10_123,
                    target: crate::authority_protocol::LocalNodeToken::from_nonzero(1).unwrap(),
                    code: 88,
                    flags: 0,
                },
            )
            .unwrap();
        drop(image);

        let mut memory = Memory(BTreeMap::from([
            (0x9000, Kind::EnterLooper.word().to_le_bytes().to_vec()),
            (0x9100, vec![0; crate::transaction_wire::RECORD_SIZE]),
        ]));
        let mut receive =
            write_read_header_sized(0x9000, 4, 0x9100, crate::transaction_wire::RECORD_SIZE);
        manager
            .execute_write_read(7, &mut receive, &mut memory)
            .unwrap();
        let record = &memory.0[&0x9100];
        assert_eq!(
            u32::from_le_bytes(record[..4].try_into().unwrap()),
            crate::transaction_wire::BR_TRANSACTION
        );
        assert_eq!(u32::from_le_bytes(record[20..24].try_into().unwrap()), 88);
        assert_eq!(i32::from_le_bytes(record[28..32].try_into().unwrap()), 50);
        assert_eq!(
            u32::from_le_bytes(record[32..36].try_into().unwrap()),
            10_123
        );
        let address = u64::from_le_bytes(record[52..60].try_into().unwrap());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(address as *const u8, 13) },
            b"remote parcel"
        );
        manager.close().unwrap();
    }

    #[test]
    fn two_phase_remote_submission_receives_reply_on_the_exact_thread() {
        let connection = open();
        connection.map_receive::<()>(4096).unwrap();
        let submission = connection.begin_remote_submission(17, true).unwrap();
        let call = crate::authority_protocol::CallToken::from_nonzero(23).unwrap();
        connection
            .commit_remote_submission(17, submission, Some(call))
            .unwrap();

        let snapshot =
            crate::transaction_snapshot::TransactionSnapshot::capture(b"remote answer", &[], 0)
                .unwrap();
        let image = crate::transfer_image::TransferImage::capture(&snapshot, &[]).unwrap();
        connection
            .deliver_remote_reply(
                &image,
                crate::authority_protocol::ConnectionToken::from_nonzero(1).unwrap(),
                crate::remote_transaction::RemoteReply {
                    call,
                    source: crate::authority_protocol::ConnectionToken::from_nonzero(2).unwrap(),
                    sender_pid: 40,
                    sender_euid: 1000,
                    target_thread: 17,
                    code: 91,
                    flags: 2,
                },
            )
            .unwrap();

        let mut memory = Memory(BTreeMap::from([(
            0xa000,
            vec![0; crate::transaction_wire::RECORD_SIZE],
        )]));
        let mut completion = write_read_header_sized(0, 0, 0xa000, 4);
        connection
            .execute_write_read(17, &mut completion, &mut memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(memory.0[&0xa000][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );
        let mut reply = write_read_header_sized(0, 0, 0xa000, crate::transaction_wire::RECORD_SIZE);
        connection
            .execute_write_read(17, &mut reply, &mut memory)
            .unwrap();
        let record = &memory.0[&0xa000];
        assert_eq!(
            u32::from_le_bytes(record[..4].try_into().unwrap()),
            crate::transaction_wire::BR_REPLY
        );
        assert_eq!(u32::from_le_bytes(record[20..24].try_into().unwrap()), 91);
        assert_eq!(u32::from_le_bytes(record[24..28].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(record[28..32].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(record[32..36].try_into().unwrap()), 1000);
        let address = u64::from_le_bytes(record[52..60].try_into().unwrap());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(address as *const u8, 13) },
            b"remote answer"
        );
        connection.close().unwrap();
    }

    #[test]
    fn authority_nodes_are_installed_in_the_process_handle_namespace() {
        let connection = open();
        let owner = crate::authority_protocol::ConnectionToken::from_nonzero(8).unwrap();
        let context = crate::authority_protocol::NodeToken::new(
            owner,
            crate::authority_protocol::LocalNodeToken::from_nonzero(2).unwrap(),
        );
        let ordinary = crate::authority_protocol::NodeToken::new(
            owner,
            crate::authority_protocol::LocalNodeToken::from_nonzero(3).unwrap(),
        );
        assert_eq!(
            connection
                .install_remote_context_manager(context, crate::reference_table::Strength::Strong)
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .install_remote_reference(ordinary, crate::reference_table::Strength::Strong)
                .unwrap(),
            1
        );
        let request = |handle: u32| {
            let mut command = Kind::Transaction.word().to_le_bytes().to_vec();
            command.resize(4 + Kind::Transaction.payload_size(), 0);
            command[4..8].copy_from_slice(&handle.to_le_bytes());
            crate::transaction_request::decode(&command).unwrap().0
        };
        assert_eq!(
            connection
                .resolve_remote_transaction_target(&request(0))
                .unwrap(),
            Some(context)
        );
        assert_eq!(
            connection
                .resolve_remote_transaction_target(&request(1))
                .unwrap(),
            Some(ordinary)
        );
        connection.close().unwrap();
    }

    #[test]
    fn remote_sg_command_owns_all_pointed_sender_bytes_before_transport() {
        let connection = open();
        let target = crate::authority_protocol::NodeToken::new(
            crate::authority_protocol::ConnectionToken::from_nonzero(8).unwrap(),
            crate::authority_protocol::LocalNodeToken::from_nonzero(2).unwrap(),
        );
        connection
            .install_remote_context_manager(target, crate::reference_table::Strength::Strong)
            .unwrap();
        let mut data = vec![0_u8; 40];
        data[..4].copy_from_slice(&crate::objects::Kind::Buffer.tag().to_le_bytes());
        data[8..16].copy_from_slice(&0x4200_u64.to_le_bytes());
        data[16..24].copy_from_slice(&3_u64.to_le_bytes());
        let offsets = 0_u64.to_le_bytes().to_vec();
        let mut command = Kind::TransactionSg.word().to_le_bytes().to_vec();
        command.resize(4 + Kind::TransactionSg.payload_size(), 0);
        command[36..44].copy_from_slice(&(data.len() as u64).to_le_bytes());
        command[44..52].copy_from_slice(&(offsets.len() as u64).to_le_bytes());
        command[52..60].copy_from_slice(&0x4000_u64.to_le_bytes());
        command[60..68].copy_from_slice(&0x4100_u64.to_le_bytes());
        command[68..76].copy_from_slice(&8_u64.to_le_bytes());
        let mut memory = Memory(BTreeMap::from([
            (0x4000, data),
            (0x4100, offsets),
            (0x4200, b"abc".to_vec()),
        ]));
        let captured = connection
            .capture_remote_command(&command, &mut memory)
            .unwrap()
            .unwrap();
        assert_eq!(captured.target(), target);
        assert_eq!(captured.bytes(), command.len());
        assert_eq!(&captured.extra()[..3], b"abc");
        assert!(captured.extra()[3..].iter().all(|byte| *byte == 0));
        drop(memory);
        assert_eq!(&captured.extra()[..3], b"abc");
        connection.close().unwrap();
    }

    #[test]
    fn authority_death_events_unwind_remote_calls_in_device_owned_state() {
        let caller = open();
        let outgoing = crate::authority_protocol::CallToken::from_nonzero(31).unwrap();
        let submission = caller.begin_remote_submission(19, true).unwrap();
        caller
            .commit_remote_submission(19, submission, Some(outgoing))
            .unwrap();
        caller.deliver_remote_target_dead(19, outgoing).unwrap();

        let mut caller_memory = Memory(BTreeMap::from([(0xb000, vec![0; 4])]));
        let mut completion = write_read_header_sized(0, 0, 0xb000, 4);
        caller
            .execute_write_read(19, &mut completion, &mut caller_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(caller_memory.0[&0xb000][..4].try_into().unwrap()),
            crate::thread::BR_TRANSACTION_COMPLETE
        );
        let mut dead_reply = write_read_header_sized(0, 0, 0xb000, 4);
        caller
            .execute_write_read(19, &mut dead_reply, &mut caller_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(caller_memory.0[&0xb000][..4].try_into().unwrap()),
            crate::thread::BR_DEAD_REPLY
        );

        let callee = Device::default()
            .open(ProcessIdentity::new(40, 1000).unwrap())
            .unwrap();
        let mut node = context_manager_object(0x100);
        node[8..16].copy_from_slice(&0xabcdu64.to_le_bytes());
        node[16..24].copy_from_slice(&0xdef0u64.to_le_bytes());
        callee
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut node)
            .unwrap();
        callee.map_receive::<()>(4096).unwrap();
        let snapshot =
            crate::transaction_snapshot::TransactionSnapshot::capture(b"pending", &[], 0).unwrap();
        let image = crate::transfer_image::TransferImage::capture(&snapshot, &[]).unwrap();
        let incoming = crate::authority_protocol::CallToken::from_nonzero(32).unwrap();
        let remote = crate::remote_transaction::RemoteTransaction {
            call: Some(incoming),
            sender: crate::authority_protocol::ConnectionToken::from_nonzero(2).unwrap(),
            sender_pid: 50,
            sender_euid: 10_123,
            target: crate::authority_protocol::LocalNodeToken::from_nonzero(1).unwrap(),
            code: 7,
            flags: 0,
        };
        callee
            .deliver_remote_transaction(
                &image,
                crate::authority_protocol::ConnectionToken::from_nonzero(1).unwrap(),
                remote,
            )
            .unwrap();
        callee.deliver_remote_caller_dead(incoming).unwrap();
        assert!(matches!(
            callee.deliver_remote_caller_dead(incoming),
            Err(Error::Registry(connection_registry::Error::Connection(
                crate::connection::Error::UnknownRemoteCall
            )))
        ));

        let accepted = crate::authority_protocol::CallToken::from_nonzero(33).unwrap();
        callee
            .deliver_remote_transaction(
                &image,
                crate::authority_protocol::ConnectionToken::from_nonzero(1).unwrap(),
                crate::remote_transaction::RemoteTransaction {
                    call: Some(accepted),
                    ..remote
                },
            )
            .unwrap();
        let mut callee_memory = Memory(BTreeMap::from([
            (0xc000, Kind::EnterLooper.word().to_le_bytes().to_vec()),
            (0xc100, vec![0; crate::transaction_wire::RECORD_SIZE]),
        ]));
        let mut receive =
            write_read_header_sized(0xc000, 4, 0xc100, crate::transaction_wire::RECORD_SIZE);
        callee
            .execute_write_read(21, &mut receive, &mut callee_memory)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(callee_memory.0[&0xc100][..4].try_into().unwrap()),
            crate::transaction_wire::BR_TRANSACTION
        );
        callee.deliver_remote_caller_dead(accepted).unwrap();
        assert!(matches!(
            callee.deliver_remote_caller_dead(accepted),
            Err(Error::Registry(connection_registry::Error::Connection(
                crate::connection::Error::UnknownRemoteCall
            )))
        ));
        callee.close().unwrap();
        caller.close().unwrap();
    }
}
