//! Bounded metadata intake for native SCM admission.
//!
//! Metadata is an authenticated response envelope, not an authority source.
//! This owner reads it with pread so a caller's file offset is unchanged and
//! never accepts an unbounded or non-regular carrier.

use crate::ProfileError;
use super::super::credentials::Credentials;
use darwin_art_scm_transfer::{AuthorityEpoch, TransferKey};
use std::{collections::HashSet, io, os::fd::RawFd};

const MAX_METADATA_SIZE: usize = 8 + 1024;

pub(super) fn read(fd: RawFd) -> Result<Vec<u8>, ProfileError> {
    if fd < 0 {
        return Err(invalid("metadata descriptor is negative"));
    }
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("metadata descriptor is not a regular file"));
    }
    if stat.st_size <= 0 || stat.st_size as u128 > MAX_METADATA_SIZE as u128 {
        return Err(invalid("metadata size exceeds bounded envelope"));
    }
    let size = stat.st_size as usize;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| invalid("metadata allocation failed"))?;
    bytes.resize(size, 0);
    let mut offset = 0usize;
    while offset != size {
        let result = unsafe {
            libc::pread(
                fd,
                bytes[offset..].as_mut_ptr().cast(),
                size - offset,
                offset as libc::off_t,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if result == 0 {
            return Err(invalid("metadata descriptor ended before its full size"));
        }
        offset += result as usize;
    }
    Ok(bytes)
}

pub(super) struct PreparedMetadata {
    pub key: TransferKey,
    pub payload_count: usize,
    pub managed: Vec<(u64, u128)>,
    pub credentials: Credentials,
}

/// Decode only the bounded prepared response needed by the native callback.
/// This deliberately does not mint or authorize anything; the authenticated
/// service remains the source of all holder and transfer state.
pub(super) fn decode_prepared(bytes: &[u8]) -> Result<PreparedMetadata, ProfileError> {
    if bytes.len() < 8 || bytes.len() > MAX_METADATA_SIZE || bytes[0] != 2 || bytes[1] != 2 {
        return Err(invalid("invalid prepared metadata header"));
    }
    if bytes[2..4] != [0, 0] {
        return Err(invalid("prepared metadata reserved bits are non-zero"));
    }
    let body_length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if body_length > 1024 || body_length + 8 != bytes.len() {
        return Err(invalid("prepared metadata length is invalid"));
    }
    let mut reader = Reader::new(&bytes[8..]);
    let authority = reader.u128()?;
    let ticket = reader.u64()?;
    let payload_count = reader.u16()? as usize;
    let managed_count = reader.u16()? as usize;
    if authority == 0
        || ticket == 0
        || payload_count > 16
        || managed_count > payload_count
    {
        return Err(invalid("prepared metadata key or count is invalid"));
    }
    let mut ordinals = HashSet::new();
    let mut holders = HashSet::new();
    ordinals
        .try_reserve(managed_count)
        .map_err(|_| invalid("prepared ordinal allocation failed"))?;
    holders
        .try_reserve(managed_count)
        .map_err(|_| invalid("prepared holder allocation failed"))?;
    let mut managed = Vec::new();
    managed
        .try_reserve_exact(managed_count)
        .map_err(|_| invalid("prepared manifest allocation failed"))?;
    for _ in 0..managed_count {
        let ordinal = reader.u64()?;
        let holder = reader.u128()?;
        if ordinal >= payload_count as u64
            || holder == 0
            || !ordinals.insert(ordinal)
            || !holders.insert(holder)
        {
            return Err(invalid("prepared metadata manifest is invalid"));
        }
        managed.push((ordinal, holder));
    }
    let credentials = Credentials {
        pid: i32::from_le_bytes(reader.take(4)?.try_into().unwrap()),
        uid: u32::from_le_bytes(reader.take(4)?.try_into().unwrap()),
        gid: u32::from_le_bytes(reader.take(4)?.try_into().unwrap()),
    };
    if !credentials.valid() {
        return Err(invalid("prepared metadata credentials are invalid"));
    }
    reader.finish()?;
    Ok(PreparedMetadata {
        key: TransferKey {
            authority: AuthorityEpoch {
                instance: authority,
            },
            ticket,
        },
        payload_count,
        managed,
        credentials,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ProfileError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| invalid("prepared metadata length overflow"))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| invalid("prepared metadata is truncated"))?;
        self.position = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, ProfileError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, ProfileError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn u128(&mut self) -> Result<u128, ProfileError> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().unwrap()))
    }

    fn finish(self) -> Result<(), ProfileError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid("prepared metadata has trailing bytes"))
        }
    }
}

fn invalid(message: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("SCM native endpoint: {message}"))
}
