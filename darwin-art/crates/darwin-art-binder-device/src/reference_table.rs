//! Receiver-local ordinary handles, coupled to aggregate node demand.
//! Session work queues and context-manager handle0 remain separate prerequisites.
use crate::node_owner::{HoldKind, LocalHold, Node};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strength {
    Strong,
    Weak,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub strong: u32,
    pub weak: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    UnknownHandle,
    StrongRequired,
    Underflow,
    Overflow,
    NoHandles,
    NodeState,
    RemoteHandle,
}

enum Target {
    Local(Arc<Node>),
    Remote(crate::authority_protocol::NodeToken),
}

struct Reference {
    target: Target,
    counts: Counts,
}

pub enum ResolvedTarget {
    Local(LocalHold),
    Remote(crate::authority_protocol::NodeToken),
}
#[derive(Default)]
pub struct ReferenceTable {
    entries: BTreeMap<u32, Reference>,
}

impl ReferenceTable {
    /// Updates table counts and aggregate node demand. A device command still
    /// requires session work scheduling and correct read-side publication.
    pub fn retain(
        &mut self,
        node: Arc<Node>,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), Error> {
        self.retain_from(Target::Local(node), strength, 1)
    }

    /// Only the context owner may pass its current registered manager here.
    /// Original get_ref_desc_olocked searches from zero but never overwrites an
    /// occupied descriptor. Return the actual descriptor for mismatch diagnosis.
    pub fn retain_context_manager(
        &mut self,
        node: Arc<Node>,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), Error> {
        self.retain_from(Target::Local(node), strength, 0)
    }

    pub fn retain_remote(
        &mut self,
        node: crate::authority_protocol::NodeToken,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), Error> {
        self.retain_from(Target::Remote(node), strength, 1)
    }

    pub fn retain_remote_context_manager(
        &mut self,
        node: crate::authority_protocol::NodeToken,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), Error> {
        self.retain_from(Target::Remote(node), strength, 0)
    }

    fn retain_from(
        &mut self,
        target: Target,
        strength: Strength,
        start: u32,
    ) -> Result<(u32, Counts, Counts), Error> {
        if let Some((&handle, _)) = self
            .entries
            .iter()
            .find(|(_, reference)| same_target(&reference.target, &target))
        {
            let (before, after) = self.increment(handle, strength)?;
            return Ok((handle, before, after));
        }
        let mut handle = start;
        for &occupied in self.entries.keys() {
            if occupied < handle {
                continue;
            }
            if occupied > handle {
                break;
            }
            handle = occupied.checked_add(1).ok_or(Error::NoHandles)?;
        }
        let before = Counts::default();
        let mut after = before;
        *Self::count_mut(&mut after, strength) = 1;
        update_target(&target, before, after)?;
        self.entries.insert(
            handle,
            Reference {
                target,
                counts: after,
            },
        );
        Ok((handle, before, after))
    }

    /// Like binder_get_node_from_ref, a successful lookup holds a temporary
    /// node reference until the caller drops the result, not just host metadata.
    pub fn lookup(&self, handle: u32, strength: Strength) -> Result<LocalHold, Error> {
        match self.resolve(handle, strength)? {
            ResolvedTarget::Local(hold) => Ok(hold),
            ResolvedTarget::Remote(_) => Err(Error::RemoteHandle),
        }
    }

    pub fn resolve(&self, handle: u32, strength: Strength) -> Result<ResolvedTarget, Error> {
        let reference = self.entries.get(&handle).ok_or(Error::UnknownHandle)?;
        if strength == Strength::Strong && reference.counts.strong == 0 {
            return Err(Error::StrongRequired);
        }
        match &reference.target {
            Target::Local(node) => node
                .hold(HoldKind::Temporary)
                .map(ResolvedTarget::Local)
                .map_err(|_| Error::Overflow),
            Target::Remote(node) => Ok(ResolvedTarget::Remote(*node)),
        }
    }
    pub fn counts(&self, handle: u32) -> Result<Counts, Error> {
        Ok(self
            .entries
            .get(&handle)
            .ok_or(Error::UnknownHandle)?
            .counts)
    }
    pub fn remote_node(&self, handle: u32) -> Result<crate::authority_protocol::NodeToken, Error> {
        let reference = self.entries.get(&handle).ok_or(Error::UnknownHandle)?;
        match reference.target {
            Target::Remote(node) => Ok(node),
            Target::Local(_) => Err(Error::RemoteHandle),
        }
    }
    pub fn increment(
        &mut self,
        handle: u32,
        strength: Strength,
    ) -> Result<(Counts, Counts), Error> {
        let reference = self.entries.get_mut(&handle).ok_or(Error::UnknownHandle)?;
        let before = reference.counts;
        let mut after = before;
        let count = Self::count_mut(&mut after, strength);
        *count = count
            .checked_add(1)
            .filter(|value| *value <= i32::MAX as u32)
            .ok_or(Error::Overflow)?;
        update_target(&reference.target, before, after)?;
        reference.counts = after;
        Ok((before, after))
    }
    pub fn decrement(
        &mut self,
        handle: u32,
        strength: Strength,
    ) -> Result<(Counts, Counts), Error> {
        let reference = self.entries.get_mut(&handle).ok_or(Error::UnknownHandle)?;
        let before = reference.counts;
        let mut after = before;
        let count = Self::count_mut(&mut after, strength);
        *count = count.checked_sub(1).ok_or(Error::Underflow)?;
        update_target(&reference.target, before, after)?;
        reference.counts = after;
        if after == Counts::default() {
            self.entries.remove(&handle);
        }
        Ok((before, after))
    }
    fn count_mut(counts: &mut Counts, strength: Strength) -> &mut u32 {
        match strength {
            Strength::Strong => &mut counts.strong,
            Strength::Weak => &mut counts.weak,
        }
    }
}

impl Drop for ReferenceTable {
    fn drop(&mut self) {
        for reference in self.entries.values() {
            update_target(&reference.target, reference.counts, Counts::default())
                .expect("live reference table owns its node demand");
        }
    }
}

fn same_target(first: &Target, second: &Target) -> bool {
    match (first, second) {
        (Target::Local(first), Target::Local(second)) => Arc::ptr_eq(first, second),
        (Target::Remote(first), Target::Remote(second)) => first == second,
        (Target::Local(_), Target::Remote(_)) | (Target::Remote(_), Target::Local(_)) => false,
    }
}

fn update_target(target: &Target, before: Counts, after: Counts) -> Result<(), Error> {
    match target {
        Target::Local(node) => node
            .update_remote(before, after)
            .map_err(|_| Error::NodeState),
        Target::Remote(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests;
