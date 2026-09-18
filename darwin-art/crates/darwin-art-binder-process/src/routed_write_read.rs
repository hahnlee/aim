//! Command-phased execution of the original Binder write/read ABI. Android
//! command parsing and state changes stay in the Binder device; only a fully
//! captured authority transaction crosses the process transport boundary.

use crate::{AuthorityTransport, Client, Error};
use darwin_art_binder_device::{
    command::{self, Kind},
    device, ioctl,
    ioctl::ClientMemory,
    write_read::Consumed,
};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum PhaseMemoryError<E> {
    Client(E),
    Internal,
}

#[derive(Debug)]
pub enum ExecuteError<E> {
    Header(ioctl::HeaderError),
    OutOfMemory,
    ClientMemory(E),
    Framing(command::DecodeError),
    RemoteCapture(device::RemoteCommandError<E>),
    DeathDevice(device::Error<std::convert::Infallible>),
    Remote(Error),
    Local(device::Error<PhaseMemoryError<E>>),
    Read(device::Error<E>),
}

impl<T: AuthorityTransport> Client<T> {
    /// Executes each write record exactly once, never holds Binder device locks
    /// across authority I/O, and performs the read half only after all writes.
    pub fn execute_write_read<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
    ) -> Result<(), ExecuteError<M::Error>> {
        let request = ioctl::Request::decode(header).map_err(ExecuteError::Header)?;
        request
            .encode_consumed(header, Consumed::default())
            .map_err(ExecuteError::Header)?;
        let mut write = allocate(request.write.size)?;
        if !write.is_empty() {
            memory
                .copy_from(request.write.address, &mut write)
                .map_err(ExecuteError::ClientMemory)?;
        }
        if request.read.size != 0 {
            memory
                .check_write(request.read.address, request.read.size)
                .map_err(ExecuteError::ClientMemory)?;
        }

        let mut consumed = Consumed::default();
        while consumed.write < write.len() {
            let remaining = &write[consumed.write..];
            let (decoded, bytes) = command::decode(remaining).map_err(ExecuteError::Framing)?;
            let remote_transaction =
                if matches!(decoded.kind, Kind::Transaction | Kind::TransactionSg) {
                    self.state
                        .device
                        .capture_remote_command(remaining, memory)
                        .map_err(ExecuteError::RemoteCapture)?
                } else {
                    None
                };
            let remote_reply = if matches!(decoded.kind, Kind::Reply | Kind::ReplySg) {
                self.state
                    .device
                    .capture_remote_reply_command(thread_id, remaining, memory)
                    .map_err(ExecuteError::RemoteCapture)?
            } else {
                None
            };
            if decoded.kind == Kind::RequestDeath {
                let (handle, cookie) = death_payload(decoded.payload);
                let target = self
                    .state
                    .device
                    .remote_node_for_handle(handle)
                    .map_err(ExecuteError::DeathDevice)?;
                self.request_death(target, cookie)
                    .map_err(ExecuteError::Remote)?;
                consumed.write += bytes;
            } else if decoded.kind == Kind::ClearDeath {
                let (handle, cookie) = death_payload(decoded.payload);
                let target = self
                    .state
                    .device
                    .remote_node_for_handle(handle)
                    .map_err(ExecuteError::DeathDevice)?;
                self.clear_death(target, cookie)
                    .map_err(ExecuteError::Remote)?;
                self.state
                    .device
                    .enqueue_clear_death_done(cookie)
                    .map_err(ExecuteError::DeathDevice)?;
                consumed.write += bytes;
            } else if decoded.kind == Kind::DeadBinderDone {
                let cookie = u64::from_le_bytes(decoded.payload.try_into().unwrap());
                self.state
                    .device
                    .acknowledge_dead_binder(cookie)
                    .map_err(ExecuteError::DeathDevice)?;
                consumed.write += bytes;
            } else if let Some(remote) = remote_transaction {
                debug_assert_eq!(remote.bytes(), bytes);
                self.submit_remote_command(thread_id, &remote)
                    .map_err(ExecuteError::Remote)?;
                consumed.write += bytes;
            } else if let Some(remote) = remote_reply {
                debug_assert_eq!(remote.bytes(), bytes);
                self.complete_remote_reply(thread_id, &remote)
                    .map_err(ExecuteError::Remote)?;
                consumed.write += bytes;
            } else {
                let local =
                    execute_local_record(&self.state.device, thread_id, remaining, bytes, memory);
                match local {
                    Ok(local_consumed) => consumed.write += local_consumed,
                    Err((local_consumed, error)) => {
                        consumed.write += local_consumed;
                        request
                            .encode_consumed(header, consumed)
                            .map_err(ExecuteError::Header)?;
                        return Err(ExecuteError::Local(error));
                    }
                }
            }
            request
                .encode_consumed(header, consumed)
                .map_err(ExecuteError::Header)?;
        }

        if request.read.size != 0 {
            let mut read_header = [0; ioctl::WRITE_READ_SIZE];
            read_header[24..32].copy_from_slice(&(request.read.size as u64).to_le_bytes());
            read_header[40..48].copy_from_slice(&request.read.address.to_le_bytes());
            let read_result =
                self.state
                    .device
                    .execute_write_read(thread_id, &mut read_header, memory);
            consumed.read =
                usize::try_from(u64::from_le_bytes(read_header[32..40].try_into().unwrap()))
                    .map_err(|_| ExecuteError::Header(ioctl::HeaderError::ConsumedOutOfRange))?;
            request
                .encode_consumed(header, consumed)
                .map_err(ExecuteError::Header)?;
            read_result.map_err(ExecuteError::Read)?;
        }
        Ok(())
    }

    pub fn execute_write_read_blocking<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
        timeout: Duration,
    ) -> Result<device::WriteReadStatus, ExecuteError<M::Error>> {
        self.execute_write_read_wait(thread_id, header, memory, Some(timeout))
    }

    /// Original blocking Binder ioctl: execute writes once, then await eligible
    /// read work without imposing an application idle lifetime.
    pub fn execute_write_read_indefinite<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
    ) -> Result<device::WriteReadStatus, ExecuteError<M::Error>> {
        self.execute_write_read_wait(thread_id, header, memory, None)
    }

    fn execute_write_read_wait<M: ClientMemory>(
        &self,
        thread_id: u64,
        header: &mut [u8],
        memory: &mut M,
        timeout: Option<Duration>,
    ) -> Result<device::WriteReadStatus, ExecuteError<M::Error>> {
        let deadline = timeout
            .map(|timeout| {
                Instant::now()
                    .checked_add(timeout)
                    .ok_or(ExecuteError::Read(device::Error::InvalidArgument))
            })
            .transpose()?;
        let request = ioctl::Request::decode(header).map_err(ExecuteError::Header)?;
        let mut generation = self
            .state
            .device
            .work_generation::<M::Error>()
            .map_err(ExecuteError::Read)?;
        self.execute_write_read(thread_id, header, memory)?;
        if request.read.size == 0 || consumed_read(header) != 0 {
            return Ok(device::WriteReadStatus::Completed);
        }

        loop {
            let waited = match deadline {
                Some(deadline) => self.state.device.wait_for_work_for_thread::<M::Error>(
                    thread_id,
                    generation,
                    deadline.saturating_duration_since(Instant::now()),
                ),
                None => self
                    .state
                    .device
                    .wait_for_work_for_thread_indefinite::<M::Error>(thread_id, generation),
            };
            match waited.map_err(ExecuteError::Read)? {
                device::WorkWait::Closed => return Ok(device::WriteReadStatus::Closed),
                device::WorkWait::TimedOut => return Ok(device::WriteReadStatus::TimedOut),
                device::WorkWait::Changed => {}
            }
            generation = self
                .state
                .device
                .work_generation::<M::Error>()
                .map_err(ExecuteError::Read)?;
            let mut read_only = [0; ioctl::WRITE_READ_SIZE];
            read_only[24..32].copy_from_slice(&(request.read.size as u64).to_le_bytes());
            read_only[40..48].copy_from_slice(&request.read.address.to_le_bytes());
            self.execute_write_read(thread_id, &mut read_only, memory)?;
            header[32..40].copy_from_slice(&read_only[32..40]);
            if consumed_read(&read_only) != 0 {
                return Ok(device::WriteReadStatus::Completed);
            }
        }
    }
}

fn death_payload(payload: &[u8]) -> (u32, u64) {
    (
        u32::from_le_bytes(payload[..4].try_into().unwrap()),
        u64::from_le_bytes(payload[4..12].try_into().unwrap()),
    )
}

fn consumed_read(header: &[u8]) -> u64 {
    u64::from_le_bytes(header[32..40].try_into().expect("validated ioctl header"))
}

fn execute_local_record<M: ClientMemory>(
    device: &device::OpenConnection,
    thread_id: u64,
    record: &[u8],
    bytes: usize,
    memory: &mut M,
) -> Result<usize, (usize, device::Error<PhaseMemoryError<M::Error>>)> {
    let mut header = [0; ioctl::WRITE_READ_SIZE];
    header[..8].copy_from_slice(&(bytes as u64).to_le_bytes());
    header[16..24].copy_from_slice(&1_u64.to_le_bytes());
    let mut phase = PhaseMemory {
        client: memory,
        record: &record[..bytes],
        copied: false,
    };
    let result = device.execute_write_read(thread_id, &mut header, &mut phase);
    let consumed =
        usize::try_from(u64::from_le_bytes(header[8..16].try_into().unwrap())).unwrap_or(0);
    result.map(|_| consumed).map_err(|error| (consumed, error))
}

struct PhaseMemory<'a, M> {
    client: &'a mut M,
    record: &'a [u8],
    copied: bool,
}

impl<M: ClientMemory> ClientMemory for PhaseMemory<'_, M> {
    type Error = PhaseMemoryError<M::Error>;

    fn copy_from(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Self::Error> {
        if !self.copied {
            if destination.len() != self.record.len() {
                return Err(PhaseMemoryError::Internal);
            }
            destination.copy_from_slice(self.record);
            self.copied = true;
            Ok(())
        } else {
            self.client
                .copy_from(address, destination)
                .map_err(PhaseMemoryError::Client)
        }
    }

    fn check_write(&mut self, address: u64, size: usize) -> Result<(), Self::Error> {
        self.client
            .check_write(address, size)
            .map_err(PhaseMemoryError::Client)
    }

    fn copy_to(&mut self, address: u64, source: &[u8]) -> Result<(), Self::Error> {
        self.client
            .copy_to(address, source)
            .map_err(PhaseMemoryError::Client)
    }
}

fn allocate<E>(size: usize) -> Result<Vec<u8>, ExecuteError<E>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| ExecuteError::OutOfMemory)?;
    bytes.resize(size, 0);
    Ok(bytes)
}
