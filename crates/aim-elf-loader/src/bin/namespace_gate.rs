use aim_elf_loader::{
    Capability, ClosedElfNamespace, DsoLifecycle, LoadError, NamespaceError, ResolveError,
    ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::{Arc, Mutex};

const PARENT: &str = "libgraph_parent.so";
const DEP_A: &str = "libgraph_dep_a.so";
const DEP_B: &str = "libgraph_dep_b.so";
const CYCLE_A: &str = "libgraph_cycle_a.so";
const CYCLE_B: &str = "libgraph_cycle_b.so";
const ABSOLUTE: &str = "libgraph_absolute.so";
const LIFECYCLE_PARENT: &str = "liblifecycle_parent.so";
const LIFECYCLE_DEP_A: &str = "liblifecycle_dep_a.so";
const LIFECYCLE_DEP_B: &str = "liblifecycle_dep_b.so";
const LIFECYCLE_SINK: &str = "liblifecycle_sink.so";
static LIFECYCLE_EVENTS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() != 13 {
        return Err("usage: elf-namespace-gate PARENT DEP_A DEP_A_ALT DEP_A_WRONG DEP_B CYCLE_A CYCLE_B UNKNOWN_PARENT ABSOLUTE LIFECYCLE_PARENT LIFECYCLE_DEP_A LIFECYCLE_DEP_B".into());
    }
    let parent = fs::read(&arguments[1])?;
    let dep_a = fs::read(&arguments[2])?;
    let dep_a_alt = fs::read(&arguments[3])?;
    let dep_a_wrong = fs::read(&arguments[4])?;
    let dep_b = fs::read(&arguments[5])?;
    let cycle_a = fs::read(&arguments[6])?;
    let cycle_b = fs::read(&arguments[7])?;
    let unknown_parent = fs::read(&arguments[8])?;
    let absolute = fs::read(&arguments[9])?;
    let lifecycle_parent = fs::read(&arguments[10])?;
    let lifecycle_dep_a = fs::read(&arguments[11])?;
    let lifecycle_dep_b = fs::read(&arguments[12])?;

    let first = make_graph(&parent, &dep_a, &dep_b)?.load(PARENT)?;
    let first_value = first.call_root_exported_i32("graph_value")?;
    if first.load_order() != [PARENT, DEP_B, DEP_A]
        || first.initialization_order() != [DEP_A, DEP_B, PARENT]
        || first.unload_order() != [PARENT, DEP_B, DEP_A]
        || first_value != 62
    {
        return Err(format!(
            "recursive load/scope/constructor order mismatch: load={:?} init={:?} unload={:?} value={first_value}",
            first.load_order(), first.initialization_order(), first.unload_order()
        ).into());
    }
    let retained = first.clone();
    if first.reference_count() != 2 {
        return Err("graph retain did not increment the reference count".into());
    }
    first.close();
    if retained.reference_count() != 1 || retained.call_root_exported_i32("graph_value")? != 62 {
        return Err("graph close released live mappings too early".into());
    }
    retained.close();

    let isolated = make_graph(&parent, &dep_a_alt, &dep_b)?.load(PARENT)?;
    if isolated.call_root_exported_i32("graph_value")? != 202 {
        return Err("same-SONAME namespaces leaked into each other".into());
    }

    let mut virtualized = ClosedElfNamespace::new();
    virtualized.add_elf(PARENT, parent.as_slice())?;
    virtualized.add_elf(DEP_A, dep_a.as_slice())?;
    virtualized.add_provider(DEP_B)?;
    let mut provider = DepBProvider::default();
    let virtualized = virtualized.load_with_resolver(PARENT, &mut provider)?;
    if virtualized.call_root_exported_i32("graph_value")? != 37 || provider.requests != 2 {
        return Err("explicit virtual DSO provider source mismatch".into());
    }

    match make_graph(&parent, &dep_a_wrong, &dep_b)?.load(PARENT) {
        Err(NamespaceError::Load {
            source:
                aim_elf_loader::LoadError::Resolver {
                    source: ResolveError::VersionMismatch { .. },
                    ..
                },
            ..
        }) => {}
        Err(error) => return Err(format!("wrong version mismatch error: {error}").into()),
        Ok(_) => return Err("mismatched GNU symbol version loaded".into()),
    }

    let mut cycle = ClosedElfNamespace::new();
    cycle.add_elf(CYCLE_A, cycle_a)?;
    cycle.add_elf(CYCLE_B, cycle_b)?;
    let cycle = cycle.load(CYCLE_A)?;
    if cycle.load_order() != [CYCLE_A, CYCLE_B]
        || cycle.initialization_order() != [CYCLE_B, CYCLE_A]
        || cycle.call_root_exported_i32("cycle_a_value")? != 22
    {
        return Err("cycle graph was not accepted deterministically".into());
    }

    let mut unknown = ClosedElfNamespace::new();
    unknown.add_elf(PARENT, unknown_parent)?;
    match unknown.load(PARENT) {
        Err(NamespaceError::UnknownDependency { soname, .. }) if soname == DEP_B => {}
        Err(error) => {
            return Err(format!("unknown dependency returned wrong error: {error}").into());
        }
        Ok(_) => return Err("unknown dependency escaped the closed namespace".into()),
    }

    let mut absolute_namespace = ClosedElfNamespace::new();
    absolute_namespace.add_elf(ABSOLUTE, absolute)?;
    match absolute_namespace.load(ABSOLUTE) {
        Err(NamespaceError::Load {
            source: LoadError::Capability(Capability::AbsoluteSymbolDefinition),
            ..
        }) => {}
        Err(error) => return Err(format!("absolute symbol returned wrong error: {error}").into()),
        Ok(_) => return Err("SHN_ABS definition did not fail closed".into()),
    }

    run_lifecycle_graph(&lifecycle_parent, &lifecycle_dep_a, &lifecycle_dep_b)?;

    println!(
        "elf-namespace-gate: recursive=parent+2deps scope=BFS version=GNU weak=zero \
         constructors=dependency-first cycle=accepted isolation=same-SONAME \
         provider=explicit rollback=RAII owner=Arc-clone close=last \
         finalizers=dso-callbacks-before-dependent-first+array-reverse+DT_FINI mapping-unload=reverse \
         protected=supported ABS=reject closed=no-dyld"
    );
    Ok(())
}

unsafe extern "C" fn lifecycle_record(value: i32) {
    LIFECYCLE_EVENTS
        .lock()
        .expect("lifecycle recorder poisoned")
        .push(value);
}

struct LifecycleProvider;

#[derive(Default)]
struct ImageLifecycleRecorder {
    ids: Mutex<HashMap<usize, i32>>,
}

impl DsoLifecycle for ImageLifecycleRecorder {
    fn publish_image(&self, range: Range<usize>) -> Result<(), String> {
        let mut ids = self.ids.lock().map_err(|_| "image recorder poisoned")?;
        let id = i32::try_from(ids.len() + 1).map_err(|_| "image id overflow")?;
        ids.insert(range.start, id);
        LIFECYCLE_EVENTS.lock().unwrap().push(500 + id);
        Ok(())
    }

    fn finalize_image(&self, range: Range<usize>) -> Result<(), String> {
        let id = self
            .ids
            .lock()
            .map_err(|_| "image recorder poisoned")?
            .remove(&range.start)
            .ok_or_else(|| "image was not published".to_owned())?;
        LIFECYCLE_EVENTS.lock().unwrap().push(400 + id);
        Ok(())
    }
}

impl SymbolResolver for LifecycleProvider {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if !request
            .needed_libraries
            .iter()
            .any(|soname| soname == LIFECYCLE_SINK)
            || request.symbol != "lifecycle_record"
            || request.version.is_some()
        {
            return Err(ResolveError::Rejected(format!(
                "unexpected lifecycle graph request: {}",
                request.symbol
            )));
        }
        let address = NonZeroUsize::new(lifecycle_record as usize)
            .ok_or_else(|| ResolveError::Rejected("null lifecycle recorder".to_owned()))?;
        // SAFETY: the recorder is process-static and implements Android AArch64 void(int).
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn run_lifecycle_graph(
    parent: &[u8],
    dep_a: &[u8],
    dep_b: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    LIFECYCLE_EVENTS.lock().unwrap().clear();

    // A provider/relocation failure occurs before any constructor, so teardown must stay silent.
    let namespace = make_lifecycle_graph(parent, dep_a, dep_b)?;
    match namespace.load(LIFECYCLE_PARENT) {
        Err(NamespaceError::Load {
            source: LoadError::UnresolvedSymbol { .. },
            ..
        }) => {}
        Err(error) => return Err(format!("lifecycle relocation failure mismatch: {error}").into()),
        Ok(_) => return Err("lifecycle graph loaded without its explicit provider".into()),
    }
    if !LIFECYCLE_EVENTS.lock().unwrap().is_empty() {
        return Err("failed graph executed finalizers".into());
    }

    for explicit_close in [true, false] {
        let namespace = make_lifecycle_graph(parent, dep_a, dep_b)?;
        let mut provider = LifecycleProvider;
        let lifecycle: Arc<dyn DsoLifecycle> = Arc::new(ImageLifecycleRecorder::default());
        let graph = namespace.load_with_resolver_and_lifecycle(
            LIFECYCLE_PARENT,
            &mut provider,
            Some(lifecycle),
        )?;
        let event_count_after_load = LIFECYCLE_EVENTS.lock().unwrap().len();
        if graph.initialization_order() != [LIFECYCLE_DEP_A, LIFECYCLE_DEP_B, LIFECYCLE_PARENT]
            || graph.unload_order() != [LIFECYCLE_PARENT, LIFECYCLE_DEP_B, LIFECYCLE_DEP_A]
            || graph.call_root_exported_i32("lifecycle_graph_value")? != 30
        {
            return Err("lifecycle graph order/export mismatch".into());
        }
        let retained = graph.clone();
        graph.close();
        if LIFECYCLE_EVENTS.lock().unwrap().len() != event_count_after_load {
            return Err("non-final graph clone executed finalizers".into());
        }
        if explicit_close {
            retained.close();
        } else {
            drop(retained);
        }
    }

    let once = [
        501, 502, 503, 403, 302, 301, 309, 402, 202, 201, 209, 401, 102, 101, 109,
    ];
    let expected: Vec<_> = once.into_iter().chain(once).collect();
    if *LIFECYCLE_EVENTS.lock().unwrap() != expected {
        return Err(format!(
            "graph finalizer order/exactly-once mismatch: {:?}",
            *LIFECYCLE_EVENTS.lock().unwrap()
        )
        .into());
    }
    Ok(())
}

fn make_lifecycle_graph(
    parent: &[u8],
    dep_a: &[u8],
    dep_b: &[u8],
) -> Result<ClosedElfNamespace, NamespaceError> {
    let mut namespace = ClosedElfNamespace::new();
    namespace.add_elf(LIFECYCLE_PARENT, parent)?;
    namespace.add_elf(LIFECYCLE_DEP_A, dep_a)?;
    namespace.add_elf(LIFECYCLE_DEP_B, dep_b)?;
    namespace.add_provider(LIFECYCLE_SINK)?;
    Ok(namespace)
}

fn make_graph(
    parent: &[u8],
    dep_a: &[u8],
    dep_b: &[u8],
) -> Result<ClosedElfNamespace, NamespaceError> {
    let mut namespace = ClosedElfNamespace::new();
    namespace.add_elf(PARENT, parent)?;
    // Deliberately add sources in an order unrelated to DT_NEEDED traversal.
    namespace.add_elf(DEP_A, dep_a)?;
    namespace.add_elf(DEP_B, dep_b)?;
    Ok(namespace)
}

unsafe extern "C" fn provided_dep_b_value() -> i32 {
    7
}

#[derive(Default)]
struct DepBProvider {
    requests: usize,
}

impl SymbolResolver for DepBProvider {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        self.requests += 1;
        if request.needed_libraries != [DEP_B, DEP_A] {
            return Err(ResolveError::Rejected(
                "virtual provider received the wrong dependency scope".to_owned(),
            ));
        }
        if request.symbol == "optional_graph_value" {
            return Ok(None);
        }
        if request.symbol != "dep_b_value" || request.version.is_some() {
            return Err(ResolveError::Rejected(format!(
                "unexpected virtual provider request: {}",
                request.symbol
            )));
        }
        let address = NonZeroUsize::new(provided_dep_b_value as usize)
            .ok_or_else(|| ResolveError::Rejected("null fixture provider".to_owned()))?;
        // SAFETY: the function has static lifetime and the exact no-argument i32 fixture ABI.
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}
