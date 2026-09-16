//! Original local child/parent traversal for a quiescent, eligible linked group.
//! Android16 linker.cpp soinfo_unload_impl walks a queue, not reversed init order.
use super::*;
use std::collections::{HashSet, VecDeque};

fn local_order(root: usize, children: &[Vec<usize>]) -> Option<Vec<usize>> {
    if root >= children.len() {
        return None;
    }
    let mut parents = vec![HashSet::new(); children.len()];
    for (parent, children) in children.iter().enumerate() {
        for &child in children {
            parents.get_mut(child)?.insert(parent);
        }
    }
    let mut queue = VecDeque::from([root]);
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    while let Some(parent) = queue.pop_front() {
        if !visited.insert(parent) {
            continue;
        }
        order.push(parent);
        for &child in &children[parent] {
            parents[child].remove(&parent);
            if !visited.contains(&child) && parents[child].is_empty() {
                queue.push_back(child);
            }
        }
    }
    // Do not finalize a partial group then silently unmap its remaining members.
    // Parent-retained subcycles need the full linker unload coordinator.
    (order.len() == children.len()).then_some(order)
}

impl GlobalElfImage {
    /// Finalize this original local group synchronously, without unmapping it.
    /// # Safety
    /// Caller establishes zero references/unload eligibility, complete parent
    /// ownership, serialized linker operations and native execution quiescence.
    /// No subsequent use of finalized library state is allowed. External groups
    /// remain retained; their close/release is a separate later operation.
    pub unsafe fn finalize_local_group(&self) -> Result<(), &'static str> {
        let root = self
            .local_group_root_image()
            .ok_or("missing original root")?;
        let mut members: Vec<_> = self
            .metadata
            .indices
            .iter()
            .filter_map(|(name, &index)| {
                root.mapping
                    .select_member(self.metadata.identities.get(index)?)
                    .map(|mapping| (name, index, mapping))
            })
            .collect();
        members.sort_by_key(|(_, index, _)| *index);
        let indices: HashMap<_, _> = members
            .iter()
            .enumerate()
            .map(|(local, (name, _, _))| (name.as_str(), local))
            .collect();
        let root_index = *indices
            .get(root.soname.as_str())
            .ok_or("root absent from group")?;
        let children: Vec<Vec<_>> = members
            .iter()
            .map(|(_, _, mapping)| {
                mapping
                    .image()
                    .needed_libraries()
                    .iter()
                    .filter_map(|name| indices.get(name.as_str()).copied())
                    .collect()
            })
            .collect();
        let order = local_order(root_index, &children)
            .ok_or("local unload traversal retains members; coordinator required")?;
        let ordered: Vec<_> = order
            .into_iter()
            .map(|index| members[index].2.clone())
            .collect();
        if !root.mapping.finalize_in_order(&ordered) {
            return Err("incomplete original local group");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn siblings_and_diamond_follow_child_queue_not_reverse_initialization() {
        assert_eq!(
            local_order(0, &[vec![1, 2], vec![3], vec![3], vec![]]),
            Some(vec![0, 1, 2, 3])
        );
        assert_eq!(
            local_order(1, &[vec![], vec![0, 2], vec![]]),
            Some(vec![1, 0, 2])
        );
        assert_eq!(local_order(0, &[vec![1], vec![0]]), Some(vec![0, 1]));
        assert_eq!(local_order(0, &[vec![1], vec![2], vec![1]]), None);
    }
}
