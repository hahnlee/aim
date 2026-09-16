use darwin_art_elf_loader::{
    ClosedElfNamespace, DsoLifecycle, NamespaceScopes, ResolveError, ResolvedSymbol, SymbolRequest,
    SymbolResolver,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Lifecycle(Mutex<Vec<std::ops::Range<usize>>>);
impl DsoLifecycle for Lifecycle {
    fn publish_image(&self, _: std::ops::Range<usize>) -> Result<(), String> {
        Ok(())
    }
    fn finalize_image(&self, range: std::ops::Range<usize>) -> Result<(), String> {
        self.0.lock().unwrap().push(range);
        Ok(())
    }
}
struct NoImports;
impl SymbolResolver for NoImports {
    fn resolve(&mut self, _: SymbolRequest<'_>) -> Result<Option<ResolvedSymbol>, ResolveError> {
        Ok(None)
    }
}
fn test_external_retention(
    ns: &ClosedElfNamespace,
    global_ns: &ClosedElfNamespace,
    policy: &NamespaceScopes,
    root: &str,
    child: &str,
) {
    for (owner, expected) in [(10, 16), (20, 78)] {
        let lifecycle = Arc::new(Lifecycle::default());
        let global = global_ns
            .load_with_globals("libglobal.so", &mut NoImports, Some(lifecycle.clone()), &[])
            .unwrap();
        let selected = global.global_image("libglobal.so").unwrap();
        drop(global);
        let mut policy = policy.clone();
        policy.globals = HashMap::from([(10, vec![]), (20, vec![])]);
        policy.globals.insert(owner, vec![0]);
        let graph = ns
            .link_with_scopes(
                root,
                &mut NoImports,
                None,
                &[selected],
                vec![],
                Some(&policy),
            )
            .unwrap();
        graph.initialize().unwrap();
        let child = graph.global_image(child).unwrap();
        let address = child
            .lookup_exported(b"child_value", None)
            .unwrap()
            .unwrap();
        drop(graph);
        assert_eq!(
            lifecycle.0.lock().unwrap().len(),
            if owner == 20 { 0 } else { 1 }
        );
        let call: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        assert_eq!(call(), expected);
        drop(child);
        assert_eq!(lifecycle.0.lock().unwrap().len(), 1);
    }
    println!(
        "external retention: unused global released; bound global survives child execution PASS"
    );
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(matches!(args.len(), 3 | 4));
    let root = "liblocal-root.so";
    let child = "liblocal-child.so";
    let mut ns = ClosedElfNamespace::new();
    ns.add_elf(root, std::fs::read(&args[1]).unwrap()).unwrap();
    ns.add_elf(child, std::fs::read(&args[2]).unwrap()).unwrap();
    let mut policy = NamespaceScopes {
        primary: HashMap::from([(root.into(), 10), (child.into(), 20)]),
        accessible: HashMap::from([
            (10, HashSet::from([root.into(), child.into()])),
            (20, HashSet::from([child.into()])),
        ]),
        globals: HashMap::from([(10, vec![]), (20, vec![])]),
    };
    let lifecycle = Arc::new(Lifecycle::default());
    let graph = ns
        .link_with_scopes(
            root,
            &mut NoImports,
            Some(lifecycle.clone()),
            &[],
            vec![],
            Some(&policy),
        )
        .unwrap();
    graph.initialize().unwrap();
    assert_eq!(graph.local_group_root(root), Some(root));
    assert_eq!(graph.local_group_root(child), Some(child));
    let root_image = graph.global_image(root).unwrap();
    let child_image = graph.global_image(child).unwrap();
    assert!(!root_image.same_local_group(&child_image));
    assert!(child_image.same_local_group(&child_image.clone()));
    assert!(
        graph
            .relocation_elf_dependencies(root)
            .unwrap()
            .iter()
            .any(|image| image.same_image(&child_image))
    );
    assert!(
        !graph
            .relocation_elf_dependencies(child)
            .unwrap()
            .iter()
            .any(|image| image.same_image(&root_image))
    );
    // Child relocation uses its own namespace's parent_value=11, not app=37.
    assert_eq!(
        graph.call_root_exported_i32("local_group_value").unwrap(),
        16
    );
    println!("namespace scopes: actual child relocation isolated from parent result16 PASS");
    let root_address = graph.lookup_root_exported("local_group_value").unwrap();
    let retained_child = graph.retain_mapping(child).unwrap();
    let selected_child = child_image.clone();
    let child_address = retained_child.lookup_exported("child_value").unwrap();
    drop(root_image);
    drop(child_image);
    drop(graph);
    {
        let finalized = lifecycle.0.lock().unwrap();
        assert_eq!(finalized.len(), 1);
        assert!(finalized[0].contains(&root_address));
        assert!(!finalized[0].contains(&child_address));
    }
    assert_eq!(retained_child.call_exported_i32("child_value").unwrap(), 16);
    drop(retained_child);
    assert_eq!(lifecycle.0.lock().unwrap().len(), 1);
    assert_eq!(
        selected_child
            .lookup_exported(b"child_value", None)
            .unwrap(),
        Some(child_address)
    );
    assert!(selected_child.same_image(&selected_child.local_group_root_image().unwrap()));
    drop(selected_child);
    {
        let finalized = lifecycle.0.lock().unwrap();
        assert_eq!(finalized.len(), 2);
        assert!(finalized[1].contains(&child_address));
    }
    println!("mapping lease: parent finalized; retained child executes16 then finalizes once PASS");
    if args.len() == 4 {
        let mut global_ns = ClosedElfNamespace::new();
        global_ns
            .add_elf("libglobal.so", std::fs::read(&args[3]).unwrap())
            .unwrap();
        test_external_retention(&ns, &global_ns, &policy, root, child);
        let global = global_ns.load("libglobal.so").unwrap();
        let globals = [global.global_image("libglobal.so").unwrap()];
        drop(global);
        for (owner, expected) in [(10, 16), (20, 78)] {
            policy.globals = HashMap::from([(10, vec![]), (20, vec![])]);
            policy.globals.insert(owner, vec![0]);
            let graph = ns
                .link_with_scopes(root, &mut NoImports, None, &globals, vec![], Some(&policy))
                .unwrap();
            graph.initialize().unwrap();
            assert_eq!(
                graph.call_root_exported_i32("local_group_value").unwrap(),
                expected
            );
            let dependencies = graph.relocation_elf_dependencies(child).unwrap();
            assert_eq!(
                dependencies
                    .iter()
                    .any(|image| image.same_image(&globals[0])),
                owner == 20,
                "record only the actually bound resident global"
            );
        }
        println!(
            "namespace globals: app-only global cannot interpose child; child global result78 PASS"
        );
    }
}
