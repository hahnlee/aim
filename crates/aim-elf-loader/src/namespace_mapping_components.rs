//! Condense physical retention cycles, without changing Android local-group
//! identities or implementing dlclose eligibility/reference counts.
pub(super) struct Components {
    pub members: Vec<Vec<usize>>,
    pub dependencies: Vec<Vec<usize>>,
}

pub(super) fn plan(edges: &[Vec<usize>]) -> Components {
    let n = edges.len();
    let mut reverse = vec![Vec::new(); n];
    for (parent, children) in edges.iter().enumerate() {
        for &child in children {
            reverse[child].push(parent);
        }
    }
    // Iterative Kosaraju avoids guest dependency depth consuming host stack.
    let mut seen = vec![false; n];
    let mut finish = Vec::new();
    for root in 0..n {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack = vec![(root, 0)];
        while let Some((node, cursor)) = stack.last_mut() {
            if *cursor == edges[*node].len() {
                finish.push(*node);
                stack.pop();
            } else {
                let child = edges[*node][*cursor];
                *cursor += 1;
                if !seen[child] {
                    seen[child] = true;
                    stack.push((child, 0));
                }
            }
        }
    }
    let mut owners = vec![usize::MAX; n];
    let mut members = Vec::new();
    for root in finish.into_iter().rev() {
        if owners[root] != usize::MAX {
            continue;
        }
        let owner = members.len();
        let mut group = Vec::new();
        let mut stack = vec![root];
        owners[root] = owner;
        while let Some(node) = stack.pop() {
            group.push(node);
            for &parent in &reverse[node] {
                if owners[parent] == usize::MAX {
                    owners[parent] = owner;
                    stack.push(parent);
                }
            }
        }
        members.push(group);
    }
    let mut dependencies = vec![Vec::new(); members.len()];
    for (parent, children) in edges.iter().enumerate() {
        for &child in children {
            let (a, b) = (owners[parent], owners[child]);
            if a != b && !dependencies[a].contains(&b) {
                // Components are source-first topological order.
                assert!(a < b);
                dependencies[a].push(b);
            }
        }
    }
    Components {
        members,
        dependencies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycles_condense_but_one_way_dependencies_stay_distinct() {
        let plan = plan(&[vec![1], vec![0, 2], vec![3], vec![2], vec![]]);
        assert_eq!(plan.members.len(), 3);
        let owner = |image| {
            plan.members
                .iter()
                .position(|g| g.contains(&image))
                .unwrap()
        };
        assert_eq!(owner(0), owner(1));
        assert_eq!(owner(2), owner(3));
        assert_ne!(owner(0), owner(2));
        assert_eq!(plan.dependencies[owner(0)], [owner(2)]);
        assert!(plan.dependencies[owner(4)].is_empty());
    }
}
