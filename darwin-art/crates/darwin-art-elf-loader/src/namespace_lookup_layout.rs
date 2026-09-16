//! Local-group slots preserve DT_NEEDED order regardless of image representation.
use std::collections::{HashMap, HashSet};
#[path = "namespace_group_layout.rs"]
mod groups;
pub(super) struct Layout {
    pub names: Vec<String>,
    pub scopes: Vec<Vec<usize>>,
    pub owners: Vec<usize>,
    pub dependencies: Vec<Vec<usize>>,
}

#[cfg(test)]
pub(super) fn layout(
    root: usize,
    files: &[(String, Vec<String>)],
    providers: &HashSet<String>,
) -> (Vec<String>, Vec<Vec<usize>>) {
    layout_with_residents(root, files, providers, &HashMap::new())
}

#[cfg(test)]
pub(super) fn layout_with_residents(
    root: usize,
    files: &[(String, Vec<String>)],
    providers: &HashSet<String>,
    residents: &HashMap<String, Vec<String>>,
) -> (Vec<String>, Vec<Vec<usize>>) {
    let layout = layout_scoped(root, files, providers, residents, None).unwrap();
    (layout.names, layout.scopes)
}

pub(super) fn layout_scoped(
    root: usize,
    files: &[(String, Vec<String>)],
    providers: &HashSet<String>,
    residents: &HashMap<String, Vec<String>>,
    policy: Option<&super::NamespaceScopes>,
) -> Result<Layout, super::NamespaceError> {
    let mut names: Vec<_> = files.iter().map(|(name, _)| name.clone()).collect();
    let mut native: Vec<_> = providers.iter().cloned().collect();
    native.sort(); // Stable slot identity; traversal order comes from DT_NEEDED.
    names.extend(native);
    let indices: HashMap<_, _> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut dependencies = vec![Vec::new(); names.len()];
    for (i, (_, needed)) in files.iter().enumerate() {
        dependencies[i] = needed
            .iter()
            .filter_map(|n| indices.get(n.as_str()).copied())
            .collect();
    }
    for (name, needed) in residents {
        if let Some(&index) = indices.get(name.as_str()) {
            dependencies[index] = needed
                .iter()
                .filter_map(|name| indices.get(name.as_str()).copied())
                .collect();
        }
    }
    // Resident global images are not local entries; they have their own catalog.
    // Existing closed-namespace entry point has one owner. The group planner
    // also handles explicit multi-namespace inputs; bridging those identities
    // from discovery remains separate from this single-owner adapter.
    let namespaces = names
        .iter()
        .map(|name| match policy {
            Some(policy) => policy.namespace(name),
            None => Ok(0),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let layout = groups::plan(
        root,
        files.len(),
        &namespaces,
        &dependencies,
        |namespace, image| policy.is_none_or(|policy| policy.permits(namespace, &names[image])),
    )
    .map_err(super::NamespaceError::Scope)?;
    if policy.is_none() {
        debug_assert_eq!(layout.roots, [root]);
        debug_assert!(layout.owners.iter().all(|&owner| owner == root));
    }
    Ok(Layout {
        names,
        scopes: layout.scopes,
        owners: layout.owners,
        dependencies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_subtrees_keep_breadth_first_order_and_cycles() {
        let files = vec![("root".into(), vec!["a".into(), "sibling".into()])];
        let providers = HashSet::from(["a".into(), "b".into(), "sibling".into(), "unused".into()]);
        let residents = HashMap::from([
            ("a".into(), vec!["b".into()]),
            ("b".into(), vec!["a".into()]),
        ]);
        let (names, scopes) = layout_with_residents(0, &files, &providers, &residents);
        let order: Vec<_> = scopes[0].iter().map(|i| names[*i].as_str()).collect();
        assert_eq!(order, ["root", "a", "sibling", "b"]);
    }
    #[test]
    fn native_slots_follow_breadth_first_edges_not_storage_order() {
        let files = vec![
            ("root".into(), vec!["native".into(), "child".into()]),
            ("child".into(), vec!["root".into(), "nested".into()]),
        ];
        let providers = HashSet::from(["native".into(), "nested".into(), "unused".into()]);
        let (names, scopes) = layout(0, &files, &providers);
        let order: Vec<_> = scopes[0].iter().map(|i| names[*i].as_str()).collect();
        assert_eq!(order, ["root", "native", "child", "nested"]);
        assert_eq!(scopes[0], scopes[1]);
    }
}
