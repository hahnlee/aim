//! Real ELF resident reuse without promoting its exports to the global group.
use darwin_art_elf_loader::*;
use std::{num::NonZeroUsize, sync::Arc};

struct Resident(GlobalElfImage);
impl SymbolResolver for Resident {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        assert_eq!(request.needed_libraries, ["liblocal-child.so"]);
        let address = self
            .0
            .lookup_exported(request.symbol.as_bytes(), None)
            .map_err(|error| ResolveError::Rejected(error.to_string()))?;
        Ok(address
            .and_then(NonZeroUsize::new)
            .map(|address| unsafe { ResolvedSymbol::new(address) }))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3);
    let mut resident_namespace = ClosedElfNamespace::new();
    resident_namespace.add_elf("liblocal-child.so", std::fs::read(&args[2])?)?;
    let original = resident_namespace.load("liblocal-child.so")?;
    let selected = original.global_image("liblocal-child.so").unwrap();
    let mut namespace = ClosedElfNamespace::new();
    namespace.add_elf("liblocal-root.so", std::fs::read(&args[1])?)?;
    namespace.add_provider_with_dependencies(
        "liblocal-child.so",
        selected.needed_libraries().to_vec(),
    )?;
    let mut resolver = Resident(selected.clone());
    let graph = namespace.load_with_globals_and_owners(
        "liblocal-root.so",
        &mut resolver,
        None,
        &[],
        vec![Arc::new(selected)],
    )?;
    drop(resolver);
    drop(original);
    drop(resident_namespace);
    // Child was bound to its original parent_value=11, not rebound to root's
    // parent_value=37. Its mapping must remain callable through the new owner.
    assert_eq!(graph.call_root_exported_i32("local_group_value")?, 16);
    println!("resident-group: existing binding and retained execution result=16 PASS");
    Ok(())
}
