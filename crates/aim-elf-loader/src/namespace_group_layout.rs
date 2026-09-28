//! Namespace-specific local lookup groups. Indices are image identities, not
//! SONAME keys. This plans scopes only; it does not decide unload eligibility.
use std::collections::{HashSet, VecDeque};

pub(super) struct GroupLayout {
    // Step-5 candidates, including a root later reached by an earlier group.
    // owners, not this list, determines each new image's actual group root.
    pub roots: Vec<usize>,
    pub scopes: Vec<Vec<usize>>,
    // New files only; residents retain their original load-group owner.
    pub owners: Vec<usize>,
}

pub(super) fn plan(
    root: usize,
    file_count: usize,
    primary_namespaces: &[u64],
    dependencies: &[Vec<usize>],
    accessible: impl Fn(u64, usize) -> bool,
) -> Result<GroupLayout, &'static str> {
    let count = dependencies.len();
    if root >= file_count
        || file_count > count
        || primary_namespaces.len() != count
        || dependencies.iter().flatten().any(|&child| child >= count)
    {
        return Err("invalid namespace group graph");
    }
    // AOSP step 5: gather new cross-namespace roots in load-task BFS order.
    let mut roots = vec![root];
    let mut known_roots = HashSet::from([root]);
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([root]);
    while let Some(parent) = pending.pop_front() {
        if !visited.insert(parent) {
            continue;
        }
        for &child in &dependencies[parent] {
            if child < file_count
                && primary_namespaces[parent] != primary_namespaces[child]
                && known_roots.insert(child)
            {
                roots.push(child);
            }
            pending.push_back(child);
        }
    }
    let mut scopes = vec![Vec::new(); file_count];
    let mut owners = vec![usize::MAX; file_count];
    // AOSP step 6: each root walks only accessible nodes, pruning inaccessible
    // subtrees. Foreign/resident nodes may be lookup entries, not new owners.
    for &group_root in &roots {
        let namespace = primary_namespaces[group_root];
        let mut group = Vec::new();
        let mut visited = HashSet::new();
        let mut pending = VecDeque::from([group_root]);
        while let Some(image) = pending.pop_front() {
            if !visited.insert(image) || !accessible(namespace, image) {
                continue;
            }
            group.push(image);
            pending.extend(dependencies[image].iter().copied());
        }
        for &image in &group {
            if image < file_count
                && primary_namespaces[image] == namespace
                && owners[image] == usize::MAX
            {
                owners[image] = group_root;
                scopes[image] = group.clone();
            }
        }
    }
    if owners.contains(&usize::MAX) {
        return Err("new file has no accessible local group");
    }
    Ok(GroupLayout {
        roots,
        scopes,
        owners,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_namespace_remains_breadth_first_and_cycle_safe() {
        let edges = vec![vec![1, 2], vec![3], vec![3, 0], vec![]];
        let layout = plan(0, 4, &[10; 4], &edges, |_, _| true).unwrap();
        assert_eq!(layout.scopes, vec![vec![0, 1, 2, 3]; 4]);
    }
    #[test]
    fn boundary_scopes_prune_private_subtrees_and_keep_residents() {
        // app->public(system)->private(system); app also uses a resident.
        let edges = vec![vec![1, 3], vec![2], vec![], vec![]];
        let namespaces = [10, 20, 20, 30];
        let layout = plan(0, 3, &namespaces, &edges, |ns, image| {
            namespaces[image] == ns || (ns == 10 && matches!(image, 1 | 3))
        })
        .unwrap();
        assert_eq!(layout.roots, [0, 1]);
        assert_eq!(layout.owners, [0, 1, 1]);
        assert_eq!(layout.scopes[0], [0, 1, 3]);
        assert_eq!(layout.scopes[1], [1, 2]);
        assert_eq!(layout.scopes[2], [1, 2]);
    }
    #[test]
    fn cycles_and_namespace_reentry_do_not_reassign_images() {
        let edges = vec![vec![1], vec![2], vec![0]];
        let ns = [10, 20, 10];
        let layout = plan(0, 3, &ns, &edges, |_, _| true).unwrap();
        assert_eq!(layout.roots, [0, 1, 2]);
        assert_eq!(layout.owners, [0, 1, 0]);
        assert_eq!(layout.scopes[0], [0, 1, 2]);
        assert_eq!(layout.scopes[1], [1, 2, 0]);
        assert_eq!(layout.scopes[2], layout.scopes[0]);
    }
    #[test]
    fn invalid_edges_and_unowned_files_fail() {
        assert!(plan(0, 1, &[1], &[vec![1]], |_, _| true).is_err());
        assert!(plan(0, 2, &[1, 1], &[vec![], vec![]], |_, _| true).is_err());
        assert!(plan(0, 1, &[1], &[vec![]], |_, _| false).is_err());
    }
}
