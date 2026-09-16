//! Physical mapping ownership grouped by the linker's original local roots.
//! Whole-graph and dependency-qualified leases share the same physical owners.
//! Android close eligibility remains separate from this resource owner.
use super::*;
use std::ops::Index;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_GROUP_ID: AtomicU64 = AtomicU64::new(1);
#[path = "namespace_mapping_components.rs"]
mod components;
#[path = "namespace_mapping_dependency.rs"]
mod dependency;

pub(super) use super::external_owners::ExternalOwners;

// No strong dependency cycle: each owner is one SCC in the mapping dependency
// graph. Original Android group identities remain MappingGroup identities.
struct MappingOwner {
    groups: Vec<Arc<MappingGroup>>,
    _dependencies: Vec<Arc<MappingOwner>>,
    _external: Option<Arc<ExternalOwners>>,
}
impl Drop for MappingOwner {
    fn drop(&mut self) {
        // All local destructors run while all members and dependencies exist.
        for group in &self.groups {
            group.finalize();
        }
        self.groups.clear();
        // Dependencies/external resources drop only after the local unmap pass.
    }
}

#[derive(Clone)]
pub(super) struct MappingIdentity {
    group: std::sync::Weak<MappingGroup>,
    member: usize,
}
impl MappingIdentity {
    pub(super) fn same_image(&self, other: &Self) -> bool {
        self.member == other.member && self.group.ptr_eq(&other.group)
    }
}

#[derive(Clone)]
pub(super) struct MappingLease {
    group: Arc<MappingGroup>,
    member: usize,
    // Drops after the direct group reference above.
    _owner: Arc<MappingOwner>,
}
impl MappingLease {
    pub(super) fn group_id(&self) -> u64 {
        self.group.id
    }
    pub(super) fn external(&self) -> Option<Arc<ExternalOwners>> {
        self._owner._external.clone()
    }
    pub(super) fn identity(&self) -> MappingIdentity {
        MappingIdentity {
            group: Arc::downgrade(&self.group),
            member: self.member,
        }
    }
    pub(super) fn same_group(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.group, &other.group)
    }
    pub(super) fn select_member(&self, identity: &MappingIdentity) -> Option<Self> {
        if !Arc::downgrade(&self.group).ptr_eq(&identity.group)
            || identity.member >= self.group.images.len()
        {
            return None;
        }
        let mut selected = self.clone();
        selected.member = identity.member;
        Some(selected)
    }
    pub(super) fn image(&self) -> &LoadedElf {
        self.group.images[self.member]
            .as_ref()
            .expect("retained mapping")
    }
    pub(super) fn finalize_in_order(&self, members: &[Self]) -> bool {
        let indices: std::collections::HashSet<_> =
            members.iter().map(|member| member.member).collect();
        if indices.len() != self.group.images.len()
            || members.len() != indices.len()
            || members.iter().any(|member| !self.same_group(member))
        {
            return false;
        }
        self.group.finalization.run(|| {
            for member in members {
                member.image().finalize_once();
            }
        });
        true
    }
}

struct MappingGroup {
    id: u64,
    finalization: crate::finalization::Finalization,
    images: Vec<Option<LoadedElf>>,
    order: Vec<usize>,
}
// Same immutable-after-relocation contract as GraphInner. Foreign execution
// is serialized by the linker owner; initializer/finalizer guards are atomic.
unsafe impl Send for MappingGroup {}
unsafe impl Sync for MappingGroup {}
impl MappingGroup {
    fn finalize(&self) {
        self.finalization.run(|| {
            for &index in self.order.iter().rev() {
                if let Some(image) = &self.images[index] {
                    image.finalize_once();
                }
            }
        });
    }
}
impl Drop for MappingGroup {
    fn drop(&mut self) {
        self.finalize();
        for &index in self.order.iter().rev() {
            drop(self.images[index].take());
        }
    }
}

#[derive(Default)]
pub(super) struct GroupedMappings {
    groups: Vec<Option<Arc<MappingGroup>>>,
    locations: Vec<(usize, usize)>,
    release_order: Vec<usize>,
    owners: Vec<Option<Arc<MappingOwner>>>,
    group_owners: Vec<usize>,
}
impl GroupedMappings {
    pub(super) fn identity(&self, image: usize) -> Option<MappingIdentity> {
        let &(group, member) = self.locations.get(image)?;
        Some(MappingIdentity {
            group: Arc::downgrade(self.groups[group].as_ref()?),
            member,
        })
    }
    pub(super) fn retain(&self, image: usize) -> Option<MappingLease> {
        let &(group, member) = self.locations.get(image)?;
        Some(MappingLease {
            group: self.groups[group].as_ref()?.clone(),
            member,
            _owner: self.owners[self.group_owners[group]].as_ref()?.clone(),
        })
    }
    #[cfg(test)]
    pub(super) fn same_group(&self, image: usize, other: &Self, candidate: usize) -> bool {
        let (Some(&(group, _)), Some(&(other_group, _))) =
            (self.locations.get(image), other.locations.get(candidate))
        else {
            return false;
        };
        match (&self.groups[group], &other.groups[other_group]) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
    #[cfg(test)]
    pub(super) fn same_image(&self, image: usize, other: &Self, candidate: usize) -> bool {
        self.same_group(image, other, candidate)
            && self.locations[image].1 == other.locations[candidate].1
    }
    #[cfg(test)]
    pub(super) fn new(images: Vec<LoadedElf>, owners: &[usize], order: &[usize]) -> Self {
        let edges = vec![Vec::new(); images.len()];
        Self::with_dependencies(images, owners, order, &edges, None)
    }
    #[cfg(test)]
    pub(super) fn with_dependencies(
        images: Vec<LoadedElf>,
        owners: &[usize],
        order: &[usize],
        dependencies: &[Vec<usize>],
        external: Option<Arc<ExternalOwners>>,
    ) -> Self {
        Self::build(images, owners, order, dependencies, |_| external.clone())
    }
    pub(super) fn with_resources(
        images: Vec<LoadedElf>,
        owners: &[usize],
        order: &[usize],
        dependencies: &[Vec<usize>],
        external: &super::external_owners::ExternalRetention,
    ) -> Self {
        Self::build(images, owners, order, dependencies, |images| {
            Some(external.for_images(images))
        })
    }
    fn build(
        images: Vec<LoadedElf>,
        owners: &[usize],
        order: &[usize],
        dependencies: &[Vec<usize>],
        external: impl Fn(&[usize]) -> Option<Arc<ExternalOwners>>,
    ) -> Self {
        assert_eq!(images.len(), owners.len());
        let mut groups: Vec<MappingGroup> = Vec::new();
        let mut by_root = HashMap::new();
        let mut locations = Vec::new();
        for (image, &owner) in images.into_iter().zip(owners) {
            let group = *by_root.entry(owner).or_insert_with(|| {
                groups.push(MappingGroup {
                    finalization: crate::finalization::Finalization::default(),
                    id: NEXT_GROUP_ID
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                        .expect("mapping group identity exhausted"),
                    images: Vec::new(),
                    order: Vec::new(),
                });
                groups.len() - 1
            });
            locations.push((group, groups[group].images.len()));
            groups[group].images.push(Some(image));
        }
        for &index in order {
            let (group, local) = locations[index];
            groups[group].order.push(local);
        }
        let mut release_order = Vec::new();
        for &index in order.iter().rev() {
            let group = locations[index].0;
            if !release_order.contains(&group) {
                release_order.push(group);
            }
        }
        let groups: Vec<_> = groups
            .into_iter()
            .map(|group| Some(Arc::new(group)))
            .collect();
        let mut edges = vec![Vec::new(); groups.len()];
        assert_eq!(dependencies.len(), locations.len());
        for (parent, children) in dependencies.iter().enumerate() {
            let a = locations[parent].0;
            for &child in children {
                let b = locations[child].0;
                if a != b && !edges[a].contains(&b) {
                    edges[a].push(b);
                }
            }
        }
        let plan = components::plan(&edges);
        let mut retained: Vec<Option<Arc<MappingOwner>>> = vec![None; plan.members.len()];
        let mut group_owners = vec![0; groups.len()];
        for component in (0..plan.members.len()).rev() {
            let members = &plan.members[component];
            for &group in members {
                group_owners[group] = component;
            }
            let local = release_order
                .iter()
                .filter(|g| members.contains(g))
                .map(|&g| groups[g].as_ref().unwrap().clone())
                .collect();
            retained[component] = Some(Arc::new(MappingOwner {
                groups: local,
                _dependencies: plan.dependencies[component]
                    .iter()
                    .map(|&child| retained[child].as_ref().unwrap().clone())
                    .collect(),
                _external: external(
                    &locations
                        .iter()
                        .enumerate()
                        .filter(|(_, (group, _))| members.contains(group))
                        .map(|(image, _)| image)
                        .collect::<Vec<_>>(),
                ),
            }));
        }
        Self {
            groups,
            locations,
            release_order,
            owners: retained,
            group_owners,
        }
    }
    pub(super) fn clear(&mut self) {
        for &group in &self.release_order {
            drop(self.groups[group].take());
        }
        for owner in &mut self.owners {
            drop(owner.take());
        }
    }
}
impl Index<usize> for GroupedMappings {
    type Output = Option<LoadedElf>;
    fn index(&self, index: usize) -> &Self::Output {
        let (group, local) = self.locations[index];
        &self.groups[group]
            .as_ref()
            .expect("retained mapping group")
            .images[local]
    }
}
impl Drop for GroupedMappings {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(all(test, target_arch = "aarch64"))]
#[path = "namespace_mappings_tests.rs"]
mod tests;
