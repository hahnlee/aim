//! Dispatch one complete BC_INCREFS_DONE/BC_ACQUIRE_DONE record to its owner.
use crate::{
    command::{self, Kind},
    node_owner::{self, NodeOwner},
    reference_table::Strength,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Framing(command::DecodeError),
    NotAcknowledgement(Kind),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Recognized command bytes executed/diagnosed, not total ioctl consumption.
    pub bytes: usize,
    /// Original driver diagnoses invalid ACKs and continues the write stream.
    /// Expose the diagnostic rather than turning them into silent successes.
    pub acknowledgement: Result<(), node_owner::Error>,
}

/// The session owner is selected by the authenticated device connection, never
/// by caller-provided identity fields. Raw input is framed here so forged Command
/// values cannot bypass complete-payload checks. No following record is consumed.
pub fn dispatch(owner: &NodeOwner, input: &[u8]) -> Result<Outcome, Error> {
    let (command, bytes) = command::decode(input).map_err(Error::Framing)?;
    let strength = match command.kind {
        Kind::IncrefsDone => Strength::Weak,
        Kind::AcquireDone => Strength::Strong,
        kind => return Err(Error::NotAcknowledgement(kind)),
    };
    let pointer = u64::from_le_bytes(command.payload[..8].try_into().unwrap());
    let cookie = u64::from_le_bytes(command.payload[8..16].try_into().unwrap());
    Ok(Outcome {
        bytes,
        acknowledgement: owner.acknowledge(pointer, cookie, strength),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        objects::{self, Kind as ObjectKind},
        reference_table::ReferenceTable,
    };
    fn owner() -> (NodeOwner, ReferenceTable) {
        let mut owner = NodeOwner::new();
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&ObjectKind::Binder.tag().to_le_bytes());
        bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&456u64.to_le_bytes());
        let object = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        let node = owner.resolve(&object[0]).unwrap();
        let mut refs = ReferenceTable::default();
        refs.retain(node, Strength::Strong).unwrap();
        assert_eq!(
            owner.read_node_notifications(123, &mut [0; 40]).unwrap(),
            40
        );
        (owner, refs)
    }
    fn record(kind: Kind, cookie: u64) -> Vec<u8> {
        let mut bytes = kind.word().to_le_bytes().to_vec();
        bytes.extend_from_slice(&123u64.to_le_bytes());
        bytes.extend_from_slice(&cookie.to_le_bytes());
        bytes
    }
    #[test]
    fn truncated_records_do_not_ack_and_concatenated_records_stop_exactly() {
        let (owner, refs) = owner();
        for kind in [Kind::IncrefsDone, Kind::AcquireDone] {
            let bytes = record(kind, 456);
            for length in 0..20 {
                assert_eq!(
                    dispatch(&owner, &bytes[..length]),
                    Err(Error::Framing(command::DecodeError::Truncated))
                );
            }
        }
        let mut stream = vec![0xff];
        stream.extend_from_slice(&record(Kind::IncrefsDone, 456));
        stream.extend_from_slice(&record(Kind::AcquireDone, 456));
        let first = dispatch(&owner, &stream[1..]).unwrap();
        assert_eq!(
            first,
            Outcome {
                bytes: 20,
                acknowledgement: Ok(())
            }
        );
        let second = dispatch(&owner, &stream[21..]).unwrap();
        assert_eq!(second, first);
        drop(refs);
        assert_eq!(
            owner.read_node_notifications(123, &mut [0; 40]).unwrap(),
            40
        );
    }
    #[test]
    fn invalid_ack_reports_diagnostic_without_consuming_pending_ack() {
        let (owner, _refs) = owner();
        assert_eq!(
            dispatch(&owner, &record(Kind::AcquireDone, 999)).unwrap(),
            Outcome {
                bytes: 20,
                acknowledgement: Err(node_owner::Error::CookieMismatch)
            }
        );
        let valid = record(Kind::AcquireDone, 456);
        assert_eq!(
            dispatch(&NodeOwner::new(), &valid).unwrap().acknowledgement,
            Err(node_owner::Error::UnknownNode)
        );
        assert!(dispatch(&owner, &valid).unwrap().acknowledgement.is_ok());
        assert_eq!(
            dispatch(&owner, &valid).unwrap().acknowledgement,
            Err(node_owner::Error::NoPendingAcknowledgement)
        );
        let other = Kind::EnterLooper.word().to_le_bytes();
        assert_eq!(
            dispatch(&owner, &other),
            Err(Error::NotAcknowledgement(Kind::EnterLooper))
        );
        assert!(
            dispatch(&owner, &record(Kind::IncrefsDone, 456))
                .unwrap()
                .acknowledgement
                .is_ok()
        );
    }
}
