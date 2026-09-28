//! Ordinary BC reference updates in the authenticated receiver's handle table.
use crate::{
    command::{self, Kind},
    reference_table::{self, Counts, ReferenceTable, Strength},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Framing(command::DecodeError),
    NotReferenceUpdate(Kind),
    /// Must be routed through the device context manager, not an ordinary table.
    ContextManagerRequired,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// One diagnosed/executed record, not aggregate ioctl write consumption.
    pub bytes: usize,
    pub update: Result<(Counts, Counts), reference_table::Error>,
}

/// The caller selects the table from its authenticated session. Complete invalid
/// ordinary-handle updates are diagnosed without mutation, as binder_thread_write
/// does; framing errors and unsupported context-manager routing remain errors.
pub fn dispatch(table: &mut ReferenceTable, input: &[u8]) -> Result<Outcome, Error> {
    let (command, bytes) = command::decode(input).map_err(Error::Framing)?;
    let (strength, increment) = match command.kind {
        Kind::Increfs => (Strength::Weak, true),
        Kind::Acquire => (Strength::Strong, true),
        Kind::Release => (Strength::Strong, false),
        Kind::Decrefs => (Strength::Weak, false),
        kind => return Err(Error::NotReferenceUpdate(kind)),
    };
    let handle = u32::from_le_bytes(command.payload[..4].try_into().unwrap());
    if handle == 0 {
        return Err(Error::ContextManagerRequired);
    }
    let update = table.counts(handle).and_then(|counts| {
        // binder_update_ref_for_handle uses binder_get_ref_olocked(..., strong).
        // Do not confuse transaction-driven retain with BC_ACQUIRE promotion.
        if strength == Strength::Strong && counts.strong == 0 {
            return Err(reference_table::Error::StrongRequired);
        }
        if increment {
            table.increment(handle, strength)
        } else {
            table.decrement(handle, strength)
        }
    });
    Ok(Outcome { bytes, update })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node_owner::NodeOwner, objects};
    fn record(kind: Kind, handle: u32) -> Vec<u8> {
        [kind.word().to_le_bytes(), handle.to_le_bytes()].concat()
    }
    fn table(strength: Strength) -> (NodeOwner, ReferenceTable) {
        let mut owner = NodeOwner::new();
        let mut data = [0; 24];
        data[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        data[8..16].copy_from_slice(&123u64.to_le_bytes());
        let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
        let node = owner.resolve(&objects[0]).unwrap();
        let mut table = ReferenceTable::default();
        table.retain(node, strength).unwrap();
        (owner, table)
    }
    #[test]
    fn framing_diagnostics_and_strong_gate_preserve_counts() {
        let (_owner, mut table) = table(Strength::Weak);
        for kind in [Kind::Increfs, Kind::Acquire, Kind::Release, Kind::Decrefs] {
            let record = record(kind, 1);
            for length in 0..8 {
                assert_eq!(
                    dispatch(&mut table, &record[..length]),
                    Err(Error::Framing(command::DecodeError::Truncated))
                );
            }
        }
        for kind in [Kind::Acquire, Kind::Release] {
            let result = dispatch(&mut table, &record(kind, 1)).unwrap();
            assert_eq!(result.bytes, 8);
            assert_eq!(result.update, Err(reference_table::Error::StrongRequired));
        }
        assert_eq!(table.counts(1).unwrap(), Counts { strong: 0, weak: 1 });
        assert_eq!(
            dispatch(&mut table, &record(Kind::Increfs, 0)),
            Err(Error::ContextManagerRequired)
        );
        assert_eq!(
            dispatch(&mut table, &record(Kind::Increfs, 77))
                .unwrap()
                .update,
            Err(reference_table::Error::UnknownHandle)
        );
        assert_eq!(
            dispatch(&mut table, &Kind::EnterLooper.word().to_le_bytes()),
            Err(Error::NotReferenceUpdate(Kind::EnterLooper))
        );
    }
    #[test]
    fn unaligned_stream_updates_drive_owner_notifications_and_final_release() {
        let (owner, mut table) = table(Strength::Strong);
        let mut output = [0; 40];
        assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 40);
        owner.acknowledge(123, 0, Strength::Strong).unwrap();
        owner.acknowledge(123, 0, Strength::Weak).unwrap();
        let mut stream = vec![0xff];
        for kind in [
            Kind::Increfs,
            Kind::Acquire,
            Kind::Release,
            Kind::Release,
            Kind::Decrefs,
        ] {
            stream.extend_from_slice(&record(kind, 1));
        }
        let mut consumed = 1;
        while consumed < stream.len() {
            let result = dispatch(&mut table, &stream[consumed..]).unwrap();
            assert!(result.update.is_ok());
            assert_eq!(result.bytes, 8);
            consumed += result.bytes;
        }
        assert_eq!(table.counts(1), Err(reference_table::Error::UnknownHandle));
        assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 40);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            0x80107209
        );
        assert_eq!(
            u32::from_le_bytes(output[20..24].try_into().unwrap()),
            0x8010720a
        );
        assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
    }
}
