//! Select an original image only within the already-retained dependency DAG.
//! Weak metadata identity alone cannot grant access to unrelated live groups.
use super::*;

impl MappingLease {
    pub(in crate::namespace) fn select_dependency(
        &self,
        identity: &MappingIdentity,
    ) -> Option<Self> {
        let mut pending = vec![&self._owner];
        let mut visited = std::collections::HashSet::new();
        while let Some(owner) = pending.pop() {
            if !visited.insert(Arc::as_ptr(owner)) {
                continue;
            }
            for group in &owner.groups {
                if Arc::downgrade(group).ptr_eq(&identity.group)
                    && identity.member < group.images.len()
                {
                    return Some(Self {
                        group: group.clone(),
                        member: identity.member,
                        _owner: owner.clone(),
                    });
                }
            }
            pending.extend(&owner._dependencies);
        }
        None
    }
}
