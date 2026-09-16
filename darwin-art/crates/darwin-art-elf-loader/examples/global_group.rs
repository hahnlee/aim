use darwin_art_elf_loader::{
    ClosedElfNamespace, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
struct NoImports;
impl SymbolResolver for NoImports {
    fn resolve(&mut self, _: SymbolRequest<'_>) -> Result<Option<ResolvedSymbol>, ResolveError> {
        Ok(None)
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("expected ROOT CHILD GLOBAL".into());
    }
    let mut source = ClosedElfNamespace::default();
    source.add_elf("libglobal.so", std::fs::read(&args[3])?)?;
    let global = source.load("libglobal.so")?;
    let selected = global.global_image("libglobal.so").ok_or("missing image")?;
    assert!(global.global_image("absent.so").is_none());
    let mut local = ClosedElfNamespace::default();
    local.add_elf("liblocal-root.so", std::fs::read(&args[1])?)?;
    local.add_elf("liblocal-child.so", std::fs::read(&args[2])?)?;
    let graph = local.load_with_globals("liblocal-root.so", &mut NoImports, None, &[selected])?;
    // Selected residents retain mappings, not the original graph view.
    assert_eq!(global.reference_count(), 1);
    {
        let root = graph.global_image("liblocal-root.so").unwrap();
        let original = global.global_image("libglobal.so").unwrap();
        let separate = source.load("libglobal.so")?;
        let other = separate.global_image("libglobal.so").unwrap();
        let declared = root
            .needed_libraries()
            .iter()
            .any(|name| name == "libglobal.so");
        assert_eq!(
            root.matches_dependency("libglobal.so", &original),
            declared.then_some(true)
        );
        assert_eq!(
            root.matches_dependency("libglobal.so", &other),
            declared.then_some(false)
        );
        if declared {
            let retained = root.dependency_image("libglobal.so").unwrap();
            assert!(retained.same_image(&original));
            assert!(!retained.same_image(&other));
            let ambiguous = local.load_with_globals(
                "liblocal-root.so",
                &mut NoImports,
                None,
                &[original.clone(), other.clone()],
            )?;
            let selected = ambiguous.global_image("liblocal-root.so").unwrap();
            assert_eq!(selected.matches_dependency("libglobal.so", &original), None);
            assert_eq!(selected.matches_dependency("libglobal.so", &other), None);
            assert!(selected.dependency_image("libglobal.so").is_none());
            assert_eq!(ambiguous.call_root_exported_i32("local_group_value")?, 78);
            println!("global identities: ambiguous DT_NEEDED owner is not guessed PASS");
        }
    }
    assert_eq!(graph.load_order().len(), 2); // Resident global is not mapped twice.
    drop(global);
    drop(source);
    drop(local);
    assert_eq!(graph.call_root_exported_i32("local_group_value")?, 78);
    drop(graph);
    println!("global-group: global-before-local, retained after source release, result=78 PASS");
    Ok(())
}
