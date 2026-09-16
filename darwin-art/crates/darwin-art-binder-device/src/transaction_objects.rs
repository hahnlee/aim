//! Binder node-object translation between authenticated process namespaces.
//! Pointer/SG and FD objects have separate owners and are rejected here until
//! their complete plans are composed by transaction execution.

use crate::{
    node_owner::Node,
    object_fields::Fields,
    objects::{Kind, Object},
    reference_table::{self, Strength},
    session::Session,
};
use std::sync::Arc;

pub struct Resolved {
    node: Arc<Node>,
    strength: Strength,
    flags: u32,
    offset: usize,
}

pub struct Rewrite {
    offset: usize,
    bytes: [u8; 24],
}

impl Rewrite {
    pub(crate) fn new(offset: usize, bytes: [u8; 24]) -> Self {
        Self { offset, bytes }
    }
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn replace_object(offset: usize, bytes: [u8; 24]) -> Self {
        Self { offset, bytes }
    }
}

pub struct Rollback {
    pub(crate) handle: u32,
    pub(crate) strength: Strength,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Objects(crate::objects::Error),
    Unsupported(Kind),
    SenderNode(crate::node_owner::Error),
    SenderReference(reference_table::Error),
    ReceiverReference(reference_table::Error),
    Remote(crate::remote_objects::Error),
}

impl Session {
    pub fn resolve_transaction_object(&mut self, object: &Object<'_>) -> Result<Resolved, Error> {
        let (node, strength, flags) = match object.fields() {
            Fields::Node { weak, flags, .. } => (
                self.resolve_local(object).map_err(Error::SenderNode)?,
                if weak {
                    Strength::Weak
                } else {
                    Strength::Strong
                },
                flags,
            ),
            Fields::Handle {
                weak,
                flags,
                handle,
                ..
            } => {
                let strength = if weak {
                    Strength::Weak
                } else {
                    Strength::Strong
                };
                let hold = self
                    .references
                    .lookup(handle, strength)
                    .map_err(Error::SenderReference)?;
                (Arc::clone(hold.node()), strength, flags)
            }
            _ => return Err(Error::Unsupported(object.kind())),
        };
        Ok(Resolved {
            node,
            strength,
            flags,
            offset: object.offset(),
        })
    }

    pub fn install_transaction_object(
        &mut self,
        resolved: &Resolved,
    ) -> Result<(Rewrite, Option<Rollback>), Error> {
        let mut bytes = [0; 24];
        bytes[4..8].copy_from_slice(&resolved.flags.to_le_bytes());
        let rollback = if self.owns_node(&resolved.node) {
            let kind = match resolved.strength {
                Strength::Strong => Kind::Binder,
                Strength::Weak => Kind::WeakBinder,
            };
            bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
            bytes[8..16].copy_from_slice(&resolved.node.pointer().to_le_bytes());
            bytes[16..24].copy_from_slice(&resolved.node.cookie().to_le_bytes());
            None
        } else {
            let kind = match resolved.strength {
                Strength::Strong => Kind::Handle,
                Strength::Weak => Kind::WeakHandle,
            };
            let (handle, _, _) = self
                .retain_transferred(Arc::clone(&resolved.node), resolved.strength)
                .map_err(Error::ReceiverReference)?;
            bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
            bytes[8..12].copy_from_slice(&handle.to_le_bytes());
            Some(Rollback {
                handle,
                strength: resolved.strength,
            })
        };
        Ok((
            Rewrite {
                offset: resolved.offset,
                bytes,
            },
            rollback,
        ))
    }

    pub fn rollback_transaction_object(&mut self, rollback: Rollback) -> Result<(), Error> {
        self.references
            .decrement(rollback.handle, rollback.strength)
            .map(|_| ())
            .map_err(Error::ReceiverReference)
    }
}

pub fn validate_supported(objects: &[Object<'_>]) -> Result<(), Error> {
    for object in objects {
        if !matches!(
            object.kind(),
            Kind::Binder | Kind::WeakBinder | Kind::Handle | Kind::WeakHandle
        ) {
            return Err(Error::Unsupported(object.kind()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects;

    fn object(kind: Kind, value: u64, cookie: u64) -> ([u8; 24], [u8; 8]) {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
        bytes[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&value.to_le_bytes());
        bytes[16..24].copy_from_slice(&cookie.to_le_bytes());
        (bytes, 0u64.to_le_bytes())
    }

    #[test]
    fn local_binder_becomes_receiver_handle_and_rollback_removes_it() {
        let mut sender = Session::default();
        let mut receiver = Session::default();
        let (bytes, offsets) = object(Kind::Binder, 0x1111, 0x2222);
        let objects = objects::validate(&bytes, &offsets).unwrap();
        let resolved = sender.resolve_transaction_object(&objects[0]).unwrap();
        let (rewrite, rollback) = receiver.install_transaction_object(&resolved).unwrap();
        assert_eq!(
            u32::from_le_bytes(rewrite.bytes[0..4].try_into().unwrap()),
            Kind::Handle.tag()
        );
        assert_eq!(
            u32::from_le_bytes(rewrite.bytes[8..12].try_into().unwrap()),
            1
        );
        receiver
            .rollback_transaction_object(rollback.unwrap())
            .unwrap();
        assert!(matches!(
            receiver.references.counts(1),
            Err(reference_table::Error::UnknownHandle)
        ));
    }

    #[test]
    fn handle_returned_to_owner_becomes_original_local_binder() {
        let mut owner = Session::default();
        let (binder, offsets) = object(Kind::Binder, 0x3333, 0x4444);
        let objects = objects::validate(&binder, &offsets).unwrap();
        let node = owner.resolve_local(&objects[0]).unwrap();
        let mut sender = Session::default();
        sender.retain_transferred(node, Strength::Strong).unwrap();
        let (handle, offsets) = object(Kind::Handle, 1, 0);
        let objects = objects::validate(&handle, &offsets).unwrap();
        let resolved = sender.resolve_transaction_object(&objects[0]).unwrap();
        let (rewrite, rollback) = owner.install_transaction_object(&resolved).unwrap();
        assert!(rollback.is_none());
        assert_eq!(
            u32::from_le_bytes(rewrite.bytes[0..4].try_into().unwrap()),
            Kind::Binder.tag()
        );
        assert_eq!(
            u64::from_le_bytes(rewrite.bytes[8..16].try_into().unwrap()),
            0x3333
        );
        assert_eq!(
            u64::from_le_bytes(rewrite.bytes[16..24].try_into().unwrap()),
            0x4444
        );
    }
}
