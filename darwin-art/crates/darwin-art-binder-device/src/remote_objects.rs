//! Binder node capabilities carried beside an immutable remote Parcel.
//! The authority assigns global node identities; the receiving binder_proc
//! alone chooses its numeric handles and rewrites the Parcel object words.

use crate::{
    authority_protocol::{LocalNodeToken, NodeToken},
    object_fields::Fields,
    objects::{Kind, Object},
    reference_table::{self, ResolvedTarget, Strength},
    session::Session,
    transaction_objects::{Rewrite, Rollback},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundSource {
    Local(LocalNodeToken),
    Remote(NodeToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutboundObject {
    pub offset: usize,
    pub source: OutboundSource,
    pub strength: Strength,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestObject {
    pub offset: usize,
    pub node: NodeToken,
    pub strength: Strength,
    pub flags: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Objects(crate::objects::Error),
    Unsupported(Kind),
    SenderNode(crate::node_owner::Error),
    SenderReference(reference_table::Error),
    ReceiverReference(reference_table::Error),
    ReceiverNode(crate::node_owner::Error),
    WrongOwner,
    InvalidManifest,
    OutOfMemory,
}

impl Session {
    pub fn resolve_remote_objects(
        &mut self,
        data: &[u8],
        offsets: &[u8],
    ) -> Result<Vec<OutboundObject>, Error> {
        let objects = crate::objects::validate(data, offsets).map_err(Error::Objects)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(objects.len())
            .map_err(|_| Error::OutOfMemory)?;
        for object in &objects {
            let (source, strength, flags) = match object.fields() {
                Fields::Node {
                    weak,
                    flags,
                    pointer,
                    cookie,
                } => {
                    let node = self.resolve_local(object).map_err(Error::SenderNode)?;
                    if std::env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
                        eprintln!(
                            "ART Binder object: publish local={:?} pointer=0x{pointer:x} cookie=0x{cookie:x}",
                            node.local_token()
                        );
                    }
                    (
                        OutboundSource::Local(node.local_token()),
                        strength(weak),
                        flags,
                    )
                }
                Fields::Handle {
                    weak,
                    flags,
                    handle,
                    ..
                } => {
                    let object_strength = strength(weak);
                    let source = match self
                        .references
                        .resolve(handle, object_strength)
                        .map_err(Error::SenderReference)?
                    {
                        ResolvedTarget::Remote(node) => OutboundSource::Remote(node),
                        ResolvedTarget::Local(hold) if self.owns_node(hold.node()) => {
                            OutboundSource::Local(hold.node().local_token())
                        }
                        ResolvedTarget::Local(_) => return Err(Error::WrongOwner),
                    };
                    (source, object_strength, flags)
                }
                Fields::Fd { .. } | Fields::Buffer { .. } | Fields::FdArray { .. } => continue,
            };
            output.push(OutboundObject {
                offset: object.offset(),
                source,
                strength,
                flags,
            });
        }
        Ok(output)
    }

    pub fn install_remote_object(
        &mut self,
        receiver: crate::authority_protocol::ConnectionToken,
        object: &ManifestObject,
    ) -> Result<(Rewrite, Option<Rollback>), Error> {
        let mut bytes = [0; 24];
        bytes[4..8].copy_from_slice(&object.flags.to_le_bytes());
        let rollback = if object.node.owner() == receiver {
            let node = self
                .lookup_local_node(object.node.local())
                .map_err(Error::ReceiverNode)?;
            let kind = match object.strength {
                Strength::Strong => Kind::Binder,
                Strength::Weak => Kind::WeakBinder,
            };
            bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
            bytes[8..16].copy_from_slice(&node.pointer().to_le_bytes());
            bytes[16..24].copy_from_slice(&node.cookie().to_le_bytes());
            None
        } else {
            let kind = match object.strength {
                Strength::Strong => Kind::Handle,
                Strength::Weak => Kind::WeakHandle,
            };
            let (handle, _, _) = self
                .retain_remote(object.node, object.strength)
                .map_err(Error::ReceiverReference)?;
            bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
            bytes[8..12].copy_from_slice(&handle.to_le_bytes());
            Some(Rollback {
                handle,
                strength: object.strength,
            })
        };
        Ok((Rewrite::new(object.offset, bytes), rollback))
    }
}

pub fn validate_manifest(objects: &[Object<'_>], manifest: &[ManifestObject]) -> Result<(), Error> {
    let reference_objects: Vec<_> = objects
        .iter()
        .filter(|object| {
            matches!(
                object.kind(),
                Kind::Binder | Kind::WeakBinder | Kind::Handle | Kind::WeakHandle
            )
        })
        .collect();
    if reference_objects.len() != manifest.len() {
        return Err(Error::InvalidManifest);
    }
    for (object, entry) in reference_objects.into_iter().zip(manifest) {
        let (weak, flags) = match object.fields() {
            Fields::Node { weak, flags, .. } | Fields::Handle { weak, flags, .. } => (weak, flags),
            _ => return Err(Error::Unsupported(object.kind())),
        };
        if object.offset() != entry.offset
            || strength(weak) != entry.strength
            || flags != entry.flags
        {
            return Err(Error::InvalidManifest);
        }
    }
    Ok(())
}

pub fn validate_delivery_objects(objects: &[Object<'_>]) -> Result<(), Error> {
    for object in objects {
        if !matches!(
            object.kind(),
            Kind::Binder | Kind::WeakBinder | Kind::Handle | Kind::WeakHandle | Kind::Fd
        ) {
            return Err(Error::Unsupported(object.kind()));
        }
    }
    Ok(())
}

fn strength(weak: bool) -> Strength {
    if weak {
        Strength::Weak
    } else {
        Strength::Strong
    }
}
