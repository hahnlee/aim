//! Actual ARM64 ELF: mapped ownership escapes before constructors, then runs once.
use darwin_art_elf_loader::*;
struct NoImports;
impl SymbolResolver for NoImports {
    fn resolve(&mut self, _: SymbolRequest<'_>) -> Result<Option<ResolvedSymbol>, ResolveError> {
        Ok(None)
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("ELF path");
    let mut namespace = ClosedElfNamespace::new();
    namespace.add_elf("libinitialization.so", std::fs::read(path)?)?;
    let linked = namespace.link_with_globals_and_owners(
        "libinitialization.so",
        &mut NoImports,
        None,
        &[],
        Vec::new(),
    )?;
    assert!(linked.call_root_exported_i32("initialized_value").is_err());
    let selected = linked.global_image("libinitialization.so").unwrap();
    let address = selected
        .lookup_exported(b"initialized_value", None)?
        .unwrap();
    // Test-only observation of zero-initialized data before constructors.
    let read: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    assert_eq!(unsafe { read() }, 0);
    let retained = linked.clone();
    drop(linked);
    retained.initialize()?;
    assert_eq!(retained.call_root_exported_i32("initialized_value")?, 42);
    assert!(retained.initialize().is_err());
    assert_eq!(unsafe { read() }, 42);
    drop(retained);
    assert_eq!(unsafe { read() }, 42); // selected image still retains mapping.
    println!(
        "linked-initialization: pre-init ownership, once-only constructor, retained result42 PASS"
    );
    Ok(())
}
