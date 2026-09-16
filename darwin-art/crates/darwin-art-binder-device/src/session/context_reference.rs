//! Reference updates for descriptor0 with context-manager-selected node identity.
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub bytes: usize,
    /// Actual descriptor, before and after counts. A nonzero descriptor after a
    /// handle0 acquire is a mismatch to diagnose, not permission to rebind0.
    pub update: Result<(u32, Counts, Counts), reference_table::Error>,
}
impl Session {
    pub(crate) fn execute_context_reference(
        &mut self,
        input: &[u8],
        manager: Option<Arc<Node>>,
    ) -> Result<Outcome, Error> {
        let (command, bytes) = command::decode(input).map_err(Error::Framing)?;
        let (strength, increment) = match command.kind {
            Kind::Increfs => (Strength::Weak, true),
            Kind::Acquire => (Strength::Strong, true),
            Kind::Release => (Strength::Strong, false),
            Kind::Decrefs => (Strength::Weak, false),
            kind => return Err(Error::Unsupported(kind)),
        };
        if u32::from_le_bytes(command.payload[..4].try_into().unwrap()) != 0 {
            return Err(Error::Unsupported(command.kind));
        }
        if increment && let Some(manager) = manager {
            return Ok(Outcome {
                bytes,
                update: self.references.retain_context_manager(manager, strength),
            });
        }
        let update = self.references.counts(0).and_then(|counts| {
            if strength == Strength::Strong && counts.strong == 0 {
                return Err(reference_table::Error::StrongRequired);
            }
            let (before, after) = if increment {
                self.references.increment(0, strength)?
            } else {
                self.references.decrement(0, strength)?
            };
            Ok((0, before, after))
        });
        Ok(Outcome { bytes, update })
    }
}
