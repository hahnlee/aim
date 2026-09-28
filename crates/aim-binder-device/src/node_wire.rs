//! 64-bit Linux Binder node-work response records: u32 command + ptr/cookie.
use crate::{node_owner::NodeOwner, node_work::Notification};
use std::io;

/// Checks the whole output extent before writing any bytes. No native struct
/// casts: each ptr/cookie follows its command at four-byte alignment.
pub fn encode(
    pointer: u64,
    cookie: u64,
    commands: &[Notification],
    output: &mut [u8],
) -> io::Result<usize> {
    let size = commands
        .len()
        .checked_mul(20)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    if output.len() < size {
        return Err(io::Error::from_raw_os_error(libc::ENOSPC));
    }
    for (command, record) in commands.iter().zip(output[..size].chunks_exact_mut(20)) {
        let number = match command {
            Notification::Increfs => 7u32,
            Notification::Acquire => 8,
            Notification::Release => 9,
            Notification::Decrefs => 10,
        };
        let word = (2u32 << 30) | (16 << 16) | (u32::from(b'r') << 8) | number;
        record[..4].copy_from_slice(&word.to_le_bytes());
        record[4..12].copy_from_slice(&pointer.to_le_bytes());
        record[12..20].copy_from_slice(&cookie.to_le_bytes());
    }
    Ok(size)
}

impl NodeOwner {
    /// Caller supplies already-safe local output memory, not a raw client pointer.
    /// ENOSPC leaves both output and pending node notification state unchanged.
    /// Scheduling which node/thread to serve belongs to the session work queue.
    pub fn read_node_notifications(&self, pointer: u64, output: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        self.publish_notifications(pointer, |cookie, commands| {
            written = encode(pointer, cookie, commands, output)?;
            Ok(())
        })?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        objects::{self, Kind},
        reference_table::{ReferenceTable, Strength},
    };
    #[test]
    fn insufficient_read_preserves_work_then_exact_records_and_ack_release() {
        let mut owner = NodeOwner::new();
        let mut data = [0; 24];
        data[..4].copy_from_slice(&Kind::Binder.tag().to_le_bytes());
        data[8..16].copy_from_slice(&123u64.to_le_bytes());
        data[16..24].copy_from_slice(&456u64.to_le_bytes());
        let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
        let node = owner.resolve(&objects[0]).unwrap();
        let mut refs = ReferenceTable::default();
        refs.retain(node, Strength::Strong).unwrap();
        let mut output = [0xcc; 44];
        for size in 0..40 {
            assert_eq!(
                owner
                    .read_node_notifications(123, &mut output[..size])
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::ENOSPC)
            );
            assert_eq!(output, [0xcc; 44]);
        }
        assert!(owner.acknowledge(123, 456, Strength::Strong).is_err());
        assert_eq!(owner.read_node_notifications(123, &mut output).unwrap(), 40);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            0x80107207
        );
        assert_eq!(
            u32::from_le_bytes(output[20..24].try_into().unwrap()),
            0x80107208
        );
        assert_eq!(&output[40..], &[0xcc; 4]);
        assert_eq!(owner.read_node_notifications(123, &mut []).unwrap(), 0);
        owner.acknowledge(123, 456, Strength::Weak).unwrap();
        owner.acknowledge(123, 456, Strength::Strong).unwrap();
        drop(refs);
        assert_eq!(owner.read_node_notifications(123, &mut output).unwrap(), 40);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            0x80107209
        );
        assert_eq!(
            u32::from_le_bytes(output[20..24].try_into().unwrap()),
            0x8010720a
        );
    }
}
