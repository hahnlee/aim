//! Linux Binder `binder_write_read` ABI at the client-memory boundary.
//!
//! This module never dereferences guest addresses. The macOS device adapter must
//! provide fault-safe copies through [`ClientMemory`]; all Binder process/thread
//! state stays in the Rust device core.

use crate::{
    connection_registry::{self, Registry},
    thread::Thread,
    write_read::{self, Consumed},
};

pub const WRITE_READ_SIZE: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub address: u64,
    pub size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub write: Range,
    pub read: Range,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HeaderError {
    Size,
    RangeOverflow,
    ConsumedOutOfRange,
}

#[derive(Debug)]
pub enum Error<E> {
    Header(HeaderError),
    OutOfMemory,
    ClientMemory(E),
    Device(write_read::Error<CaptureError<E>>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureError<E> {
    SizeOverflow,
    OutOfMemory,
    ClientMemory(E),
    Request(crate::transaction_request::Error),
}

pub trait ClientMemory {
    type Error;

    fn copy_from(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Self::Error>;
    fn check_write(&mut self, address: u64, size: usize) -> Result<(), Self::Error>;
    fn copy_to(&mut self, address: u64, source: &[u8]) -> Result<(), Self::Error>;
}

/// Snapshot the two sender ranges named by a decoded BC transaction. Neither
/// address is ever treated as a host pointer, and structural validation runs
/// only after both immutable copies succeed.
pub fn capture_transaction<M: ClientMemory>(
    request: &crate::transaction_request::Request,
    memory: &mut M,
) -> Result<crate::transaction_snapshot::TransactionSnapshot, CaptureError<M::Error>> {
    fn size<E>(value: u64) -> Result<usize, CaptureError<E>> {
        usize::try_from(value).map_err(|_| CaptureError::SizeOverflow)
    }
    fn buffer<E>(size: usize) -> Result<Vec<u8>, CaptureError<E>> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(size)
            .map_err(|_| CaptureError::OutOfMemory)?;
        output.resize(size, 0);
        Ok(output)
    }
    let mut data = buffer(size(request.data().size)?)?;
    let mut offsets = buffer(size(request.offsets().size)?)?;
    if !data.is_empty() {
        memory
            .copy_from(request.data().address, &mut data)
            .map_err(CaptureError::ClientMemory)?;
    }
    if !offsets.is_empty() {
        memory
            .copy_from(request.offsets().address, &mut offsets)
            .map_err(CaptureError::ClientMemory)?;
    }
    request
        .capture_copied(&data, &offsets)
        .map_err(CaptureError::Request)
}

impl Request {
    pub fn decode(header: &[u8]) -> Result<Self, HeaderError> {
        if header.len() != WRITE_READ_SIZE {
            return Err(HeaderError::Size);
        }
        let field =
            |offset: usize| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
        fn range(address: u64, wire_size: u64) -> Result<Range, HeaderError> {
            if wire_size != 0 && address.checked_add(wire_size).is_none() {
                return Err(HeaderError::RangeOverflow);
            }
            let size = usize::try_from(wire_size).map_err(|_| HeaderError::RangeOverflow)?;
            Ok(Range { address, size })
        }
        Ok(Self {
            write: range(field(16), field(0))?,
            read: range(field(40), field(24))?,
        })
    }

    /// Update only the two kernel-owned consumption fields. Caller copies the
    /// full header back through its checked platform boundary.
    pub fn encode_consumed(
        &self,
        header: &mut [u8],
        consumed: Consumed,
    ) -> Result<(), HeaderError> {
        if header.len() != WRITE_READ_SIZE {
            return Err(HeaderError::Size);
        }
        if consumed.write > self.write.size || consumed.read > self.read.size {
            return Err(HeaderError::ConsumedOutOfRange);
        }
        header[8..16].copy_from_slice(&(consumed.write as u64).to_le_bytes());
        header[32..40].copy_from_slice(&(consumed.read as u64).to_le_bytes());
        Ok(())
    }
}

impl<M: ClientMemory> write_read::TransactionSource for M {
    type Error = CaptureError<M::Error>;

    fn capture(
        &mut self,
        request: &crate::transaction_request::Request,
    ) -> Result<crate::transaction_snapshot::TransactionSnapshot, Self::Error> {
        capture_transaction(request, self)
    }
}

/// Execute one already-copied ioctl header and its referenced client buffers.
/// Consumption is encoded even when device execution fails after a partial
/// write, exactly so IPCThreadState can discard only committed commands.
pub fn execute<M: ClientMemory>(
    registry: &Registry,
    key: &connection_registry::Key,
    thread: &mut Thread,
    header: &mut [u8],
    memory: &mut M,
    sender: write_read::SenderIdentity,
) -> Result<(), Error<M::Error>> {
    let request = Request::decode(header).map_err(Error::Header)?;
    request
        .encode_consumed(header, Consumed::default())
        .map_err(Error::Header)?;
    let mut write = allocate::<M::Error>(request.write.size)?;
    let mut read = allocate::<M::Error>(request.read.size)?;
    if !write.is_empty() {
        memory
            .copy_from(request.write.address, &mut write)
            .map_err(Error::ClientMemory)?;
    }
    if !read.is_empty() {
        memory
            .check_write(request.read.address, request.read.size)
            .map_err(Error::ClientMemory)?;
    }

    let result = registry.write_read(key, thread, &write, &mut read, memory, sender);
    let consumed = match &result {
        Ok(consumed) => *consumed,
        Err(error) => error.consumed,
    };
    if consumed.read != 0 {
        memory
            .copy_to(request.read.address, &read[..consumed.read])
            .map_err(Error::ClientMemory)?;
    }
    request
        .encode_consumed(header, consumed)
        .map_err(Error::Header)?;
    result.map(|_| ()).map_err(Error::Device)
}

fn allocate<E>(size: usize) -> Result<Vec<u8>, Error<E>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| Error::OutOfMemory)?;
    bytes.resize(size, 0);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connection::ConnectionOwner, session::Session};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Memory {
        regions: HashMap<u64, Vec<u8>>,
    }
    impl ClientMemory for Memory {
        type Error = &'static str;

        fn copy_from(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Self::Error> {
            let source = self.regions.get(&address).ok_or("fault")?;
            if source.len() != destination.len() {
                return Err("size");
            }
            destination.copy_from_slice(source);
            Ok(())
        }

        fn check_write(&mut self, address: u64, size: usize) -> Result<(), Self::Error> {
            let destination = self.regions.get(&address).ok_or("fault")?;
            (destination.len() >= size).then_some(()).ok_or("size")
        }

        fn copy_to(&mut self, address: u64, source: &[u8]) -> Result<(), Self::Error> {
            let destination = self.regions.get_mut(&address).ok_or("fault")?;
            destination
                .get_mut(..source.len())
                .ok_or("size")?
                .copy_from_slice(source);
            Ok(())
        }
    }

    fn header(write_address: u64, write: usize, read_address: u64, read: usize) -> [u8; 48] {
        let mut header = [0; 48];
        header[0..8].copy_from_slice(&(write as u64).to_le_bytes());
        header[16..24].copy_from_slice(&write_address.to_le_bytes());
        header[24..32].copy_from_slice(&(read as u64).to_le_bytes());
        header[40..48].copy_from_slice(&read_address.to_le_bytes());
        header
    }

    #[test]
    fn exact_uapi_layout_and_range_validation() {
        let mut bytes = header(0x1000, 8, 0x2000, 40);
        bytes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        bytes[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        let request = Request::decode(&bytes).unwrap();
        assert_eq!(
            request.write,
            Range {
                address: 0x1000,
                size: 8
            }
        );
        assert_eq!(
            request.read,
            Range {
                address: 0x2000,
                size: 40
            }
        );
        request
            .encode_consumed(&mut bytes, Consumed { write: 4, read: 20 })
            .unwrap();
        assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(bytes[32..40].try_into().unwrap()), 20);
        assert_eq!(Request::decode(&bytes[..47]), Err(HeaderError::Size));
        assert!(matches!(
            Request::decode(&header(u64::MAX, 2, 0, 0)),
            Err(HeaderError::RangeOverflow)
        ));
    }

    #[test]
    fn ioctl_copies_through_platform_boundary_and_updates_consumption() {
        let registry = Registry::default();
        let key = registry
            .register(ConnectionOwner::new(Session::default()))
            .unwrap();
        let mut thread = Thread::default();
        let write_address = 0x1000;
        let read_address = 0x2000;
        let enter = crate::command::Kind::EnterLooper
            .word()
            .to_le_bytes()
            .to_vec();
        let mut memory = Memory {
            regions: HashMap::from([(write_address, enter), (read_address, vec![0xcc; 40])]),
        };
        let mut header = header(write_address, 4, read_address, 40);
        execute(
            &registry,
            &key,
            &mut thread,
            &mut header,
            &mut memory,
            write_read::SenderIdentity { pid: 1, euid: 0 },
        )
        .unwrap();
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(header[32..40].try_into().unwrap()), 0);
        assert_eq!(memory.regions[&read_address], [0xcc; 40]);
        registry.close(&key).unwrap();
    }

    #[test]
    fn client_fault_does_not_claim_any_consumption() {
        let registry = Registry::default();
        let key = registry
            .register(ConnectionOwner::new(Session::default()))
            .unwrap();
        let mut thread = Thread::default();
        let mut memory = Memory::default();
        let mut header = header(0xdead, 4, 0, 0);
        assert!(matches!(
            execute(
                &registry,
                &key,
                &mut thread,
                &mut header,
                &mut memory,
                write_read::SenderIdentity { pid: 1, euid: 0 },
            ),
            Err(Error::ClientMemory("fault"))
        ));
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 0);
        registry.close(&key).unwrap();
    }

    #[test]
    fn transaction_ranges_are_copied_before_structural_validation() {
        let mut command = crate::command::Kind::Transaction
            .word()
            .to_le_bytes()
            .to_vec();
        command.resize(68, 0);
        command[36..44].copy_from_slice(&24u64.to_le_bytes());
        command[44..52].copy_from_slice(&8u64.to_le_bytes());
        command[52..60].copy_from_slice(&0x3000u64.to_le_bytes());
        command[60..68].copy_from_slice(&0x4000u64.to_le_bytes());
        let request = crate::transaction_request::decode(&command).unwrap().0;
        let mut object = vec![0; 24];
        object[..4].copy_from_slice(&crate::objects::Kind::Binder.tag().to_le_bytes());
        let mut memory = Memory {
            regions: HashMap::from([
                (0x3000, object.clone()),
                (0x4000, 0u64.to_le_bytes().to_vec()),
            ]),
        };
        let snapshot = capture_transaction(&request, &mut memory).unwrap();
        object.fill(0xff);
        memory.regions.get_mut(&0x3000).unwrap().fill(0xee);
        assert_eq!(
            snapshot.objects().unwrap()[0].kind(),
            crate::objects::Kind::Binder
        );

        memory.regions.remove(&0x4000);
        assert!(matches!(
            capture_transaction(&request, &mut memory),
            Err(CaptureError::ClientMemory("fault"))
        ));
    }
}
