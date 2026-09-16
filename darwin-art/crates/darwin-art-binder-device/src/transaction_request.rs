//! Write-side transaction headers. Addresses are opaque sender addresses, never
//! host pointers; sender PID/UID, target pointer and cookie are not authorities.
use crate::{
    command::{self, Kind},
    transaction_snapshot::{self, TransactionSnapshot},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Handle(u32),
    /// The reply destination comes from the authenticated thread's stack.
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SenderRange {
    pub address: u64,
    pub size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Framing(command::DecodeError),
    NotTransaction(Kind),
    CopiedLengthMismatch,
    SizeOverflow,
    Snapshot(transaction_snapshot::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    target: Target,
    code: u32,
    flags: u32,
    data: SenderRange,
    offsets: SenderRange,
    extra: u64,
}
impl Request {
    pub fn target(&self) -> Target {
        self.target
    }
    pub fn code(&self) -> u32 {
        self.code
    }
    pub fn flags(&self) -> u32 {
        self.flags
    }
    pub fn data(&self) -> SenderRange {
        self.data
    }
    pub fn offsets(&self) -> SenderRange {
        self.offsets
    }
    pub fn extra_size(&self) -> u64 {
        self.extra
    }

    /// Input slices must come from fault-safe copying in the authenticated
    /// sender address space. No raw client address is dereferenced here. Sizes
    /// must match the immutable header before structural snapshot validation.
    pub fn capture_copied(
        &self,
        data: &[u8],
        offsets: &[u8],
    ) -> Result<TransactionSnapshot, Error> {
        if data.len() as u64 != self.data.size || offsets.len() as u64 != self.offsets.size {
            return Err(Error::CopiedLengthMismatch);
        }
        let extra = usize::try_from(self.extra).map_err(|_| Error::SizeOverflow)?;
        TransactionSnapshot::capture(data, offsets, extra).map_err(Error::Snapshot)
    }
}

/// Decode one BC_TRANSACTION/REPLY, including SG forms. Framing alone is not
/// execution or write_consumed. Do not trust user header sender_pid/sender_euid.
pub fn decode(input: &[u8]) -> Result<(Request, usize), Error> {
    let (command, bytes) = command::decode(input).map_err(Error::Framing)?;
    let reply = match command.kind {
        Kind::Transaction | Kind::TransactionSg => false,
        Kind::Reply | Kind::ReplySg => true,
        kind => return Err(Error::NotTransaction(kind)),
    };
    let data = command.payload;
    let u32_at = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let u64_at = |offset| u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
    Ok((
        Request {
            target: if reply {
                Target::Reply
            } else {
                Target::Handle(u32_at(0))
            },
            code: u32_at(16),
            flags: u32_at(20),
            data: SenderRange {
                address: u64_at(48),
                size: u64_at(32),
            },
            offsets: SenderRange {
                address: u64_at(56),
                size: u64_at(40),
            },
            extra: if matches!(command.kind, Kind::TransactionSg | Kind::ReplySg) {
                u64_at(64)
            } else {
                0
            },
        },
        bytes,
    ))
}

#[cfg(test)]
mod tests;
