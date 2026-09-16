//! Checked framing of the pinned 64-bit Android Binder write command stream.
//! Never dereferences transaction pointers or assumes aligned client buffers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Transaction,
    Reply,
    AcquireResult,
    FreeBuffer,
    Increfs,
    Acquire,
    Release,
    Decrefs,
    IncrefsDone,
    AcquireDone,
    AttemptAcquire,
    RegisterLooper,
    EnterLooper,
    ExitLooper,
    RequestDeath,
    ClearDeath,
    DeadBinderDone,
    TransactionSg,
    ReplySg,
    RequestFreeze,
    ClearFreeze,
    FreezeDone,
}

pub const KINDS: [Kind; 22] = [
    Kind::Transaction,
    Kind::Reply,
    Kind::AcquireResult,
    Kind::FreeBuffer,
    Kind::Increfs,
    Kind::Acquire,
    Kind::Release,
    Kind::Decrefs,
    Kind::IncrefsDone,
    Kind::AcquireDone,
    Kind::AttemptAcquire,
    Kind::RegisterLooper,
    Kind::EnterLooper,
    Kind::ExitLooper,
    Kind::RequestDeath,
    Kind::ClearDeath,
    Kind::DeadBinderDone,
    Kind::TransactionSg,
    Kind::ReplySg,
    Kind::RequestFreeze,
    Kind::ClearFreeze,
    Kind::FreezeDone,
];

impl Kind {
    pub const fn payload_size(self) -> usize {
        match self {
            Self::Transaction | Self::Reply => 64,
            Self::TransactionSg | Self::ReplySg => 72,
            Self::IncrefsDone | Self::AcquireDone => 16,
            // binder_handle_cookie is explicitly packed in the original UAPI.
            Self::RequestDeath | Self::ClearDeath | Self::RequestFreeze | Self::ClearFreeze => 12,
            Self::FreeBuffer | Self::AttemptAcquire | Self::DeadBinderDone | Self::FreezeDone => 8,
            Self::RegisterLooper | Self::EnterLooper | Self::ExitLooper => 0,
            _ => 4,
        }
    }

    pub const fn word(self) -> u32 {
        let size = self.payload_size() as u32;
        let direction = if size == 0 { 0 } else { 1 << 30 };
        direction | (size << 16) | (u32::from_le_bytes([0, b'c', 0, 0])) | self as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    UnknownCommand(u32),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Command<'a> {
    pub kind: Kind,
    pub payload: &'a [u8],
}

/// Returns one record and its encoded byte count. This is NOT write_consumed:
/// the eventual device owner must commit consumption only after executing it.
pub fn decode(input: &[u8]) -> Result<(Command<'_>, usize), DecodeError> {
    let word = u32::from_le_bytes(
        input
            .get(..4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    );
    let kind = KINDS
        .iter()
        .copied()
        .find(|kind| kind.word() == word)
        .ok_or(DecodeError::UnknownCommand(word))?;
    let end = 4 + kind.payload_size();
    let payload = input.get(4..end).ok_or(DecodeError::Truncated)?;
    Ok((Command { kind, payload }, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_commands_unaligned_and_every_truncation() {
        for kind in KINDS {
            let mut bytes = vec![0xff];
            bytes.extend_from_slice(&kind.word().to_le_bytes());
            bytes.resize(5 + kind.payload_size(), 0xa5);
            let record = &bytes[1..];
            for length in 0..record.len() {
                assert_eq!(decode(&record[..length]), Err(DecodeError::Truncated));
            }
            let (command, count) = decode(record).unwrap();
            assert_eq!(count, record.len());
            assert_eq!(command.kind, kind);
            assert_eq!(command.payload, &record[4..]);
            assert_eq!(command.payload.as_ptr(), record[4..].as_ptr());
        }
    }

    #[test]
    fn forged_size_direction_and_unknown_number_rejected() {
        for word in [
            Kind::Transaction.word() ^ (1 << 16),
            Kind::Transaction.word() ^ (3 << 30),
            0x6300 | 22,
        ] {
            assert_eq!(
                decode(&word.to_le_bytes()),
                Err(DecodeError::UnknownCommand(word))
            );
        }
    }

    #[test]
    fn concatenated_records_stop_at_exact_boundaries() {
        let bytes: Vec<_> = KINDS
            .iter()
            .flat_map(|kind| {
                let mut record = kind.word().to_le_bytes().to_vec();
                record.resize(4 + kind.payload_size(), 0);
                record
            })
            .collect();
        let mut offset = 0;
        for kind in KINDS {
            let (command, length) = decode(&bytes[offset..]).unwrap();
            assert_eq!(command.kind, kind);
            offset += length;
        }
        assert_eq!(offset, bytes.len());
    }
}
