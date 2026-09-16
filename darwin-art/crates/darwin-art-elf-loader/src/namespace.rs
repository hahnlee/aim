use super::{
    DsoLifecycle, ExportedSymbol, LoadError, LoadedElf, RejectAllResolver, ResolveError,
    ResolvedSymbol, StagedElf, SymbolRequest, SymbolResolver, VersionRequirement,
};
use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
#[path = "namespace_android_versions.rs"]
mod android_versions;
#[path = "namespace_globals.rs"]
mod globals;
#[path = "namespace_lookup_layout.rs"]
mod lookup_layout;
#[cfg(test)]
#[path = "namespace_native_owners_tests.rs"]
mod native_owners_tests;
#[path = "namespace_residents.rs"]
mod residents;
pub use globals::GlobalElfImage;
#[path = "namespace_scope_config.rs"]
mod scope_config;
pub use scope_config::NamespaceScopes;
#[path = "namespace_bindings.rs"]
mod bindings;
#[path = "namespace_mapping_lease.rs"]
mod mapping_lease;
pub use mapping_lease::RetainedElfMapping;
#[path = "namespace_external_owners.rs"]
mod external_owners;
#[path = "namespace_image_resource.rs"]
pub(crate) mod image_resource;
#[path = "namespace_mappings.rs"]
mod mappings;
#[path = "namespace_selection_metadata.rs"]
mod selection_metadata;

/// An explicitly populated, closed ELF namespace.
///
/// Every ELF object is supplied under its Android logical SONAME. An embedded
/// `DT_SONAME`, when present, must match; Android path-loaded DSOs may omit it.
/// No path search, dyld lookup, `dlopen`, or process-global symbol fallback is performed.
#[derive(Default)]
pub struct ClosedElfNamespace {
    sources: HashMap<String, Vec<u8>>,
    providers: HashSet<String>,
    resident_dependencies: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
pub enum NamespaceError {
    Scope(&'static str),
    DuplicateSoname(String),
    UnknownDependency {
        requested_by: String,
        soname: String,
    },
    SonameMismatch {
        supplied: String,
        embedded: Option<String>,
    },
    Load {
        soname: String,
        source: LoadError,
    },
    Lifecycle {
        soname: String,
        message: String,
    },
}

pub(crate) struct ScopeLinkOptions<'a> {
    pub(crate) namespace_scopes: Option<&'a NamespaceScopes>,
    pub(crate) appcompat_16kb: Option<bool>,
}

impl fmt::Display for NamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(detail) => write!(formatter, "invalid namespace scopes: {detail}"),
            Self::DuplicateSoname(soname) => {
                write!(formatter, "duplicate namespace SONAME: {soname}")
            }
            Self::UnknownDependency {
                requested_by,
                soname,
            } => {
                write!(
                    formatter,
                    "{requested_by} requires unknown dependency {soname}"
                )
            }
            Self::SonameMismatch { supplied, embedded } => write!(
                formatter,
                "namespace key {supplied} does not match embedded DT_SONAME {:?}",
                embedded
            ),
            Self::Load { soname, source } => write!(formatter, "failed to load {soname}: {source}"),
            Self::Lifecycle { soname, message } => {
                write!(
                    formatter,
                    "failed to publish lifecycle for {soname}: {message}"
                )
            }
        }
    }
}

impl StdError for NamespaceError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Load { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl ClosedElfNamespace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one immutable ELF byte source. An embedded `DT_SONAME` is checked at load time.
    pub fn add_elf(
        &mut self,
        soname: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), NamespaceError> {
        let soname = soname.into();
        if soname.is_empty() || self.sources.contains_key(&soname) {
            return Err(NamespaceError::DuplicateSoname(soname));
        }
        if self.providers.contains(&soname) {
            return Err(NamespaceError::DuplicateSoname(soname));
        }
        self.sources.insert(soname, bytes.into());
        Ok(())
    }

    /// Registers a virtual DSO SONAME whose imports are served by `load_with_resolver`.
    pub fn add_provider(&mut self, soname: impl Into<String>) -> Result<(), NamespaceError> {
        let soname = soname.into();
        if soname.is_empty()
            || self.sources.contains_key(&soname)
            || !self.providers.insert(soname.clone())
        {
            return Err(NamespaceError::DuplicateSoname(soname));
        }
        Ok(())
    }

    pub fn load(&self, root: &str) -> Result<LoadedElfGraph, NamespaceError> {
        self.load_with_resolver(root, &mut RejectAllResolver)
    }

    /// Loads a root and its complete recursive `DT_NEEDED` closure.
    ///
    /// The optional resolver is the namespace's explicit non-ELF provider source (for example,
    /// a Bionic facade). It is consulted only after lookup in the root's ELF local
    /// group and is never retained after eager relocation.
    pub fn load_with_resolver(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        self.load_with_resolver_and_lifecycle(root, external, None)
    }

    pub fn load_with_resolver_and_lifecycle(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        self.load_with_globals(root, external, lifecycle, &[])
    }

    /// Load after the Android namespace owner selects an ordered global group.
    /// Selected mappings stay alive through this graph's finalizers and unload.
    pub fn load_with_globals(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
        globals: &[GlobalElfImage],
    ) -> Result<LoadedElfGraph, NamespaceError> {
        self.load_with_globals_and_owners(root, external, lifecycle, globals, Vec::new())
    }

    /// Retain real native dependency owners through initialization, rollback,
    /// all graph/image clones and ELF finalizers. These owners grant lifetime
    /// only: namespace admission and lookup order remain separate contracts.
    /// Owners must permit release from the thread dropping the last graph.
    pub fn load_with_globals_and_owners(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
        globals: &[GlobalElfImage],
        native_owners: Vec<Arc<dyn std::any::Any + Send + Sync>>,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        let graph =
            self.link_with_globals_and_owners(root, external, lifecycle, globals, native_owners)?;
        graph.initialize()?;
        Ok(graph)
    }

    /// Map and relocate a graph without executing constructors. The linker
    /// owner must publish it before initialize, serialize competing opens,
    /// and retract failed initialization from its registry. Not dlopen itself.
    pub fn link_with_globals_and_owners(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
        globals: &[GlobalElfImage],
        native_owners: Vec<Arc<dyn std::any::Any + Send + Sync>>,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        self.link_with_scopes(root, external, lifecycle, globals, native_owners, None)
    }

    /// Link with explicit Android namespace scope decisions. None denotes this
    /// type's original single-namespace contract, not a missing-metadata fallback.
    /// Mapping lifetime is still whole-graph; this does not implement group close.
    pub fn link_with_scopes(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
        globals: &[GlobalElfImage],
        native_owners: Vec<Arc<dyn std::any::Any + Send + Sync>>,
        namespace_scopes: Option<&NamespaceScopes>,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        self.link_with_scopes_and_appcompat(
            root,
            external,
            lifecycle,
            globals,
            native_owners,
            ScopeLinkOptions {
                namespace_scopes,
                appcompat_16kb: None,
            },
        )
    }

    /// Link with explicit namespace scopes and a per-discovered-graph 16KiB
    /// app-compat policy snapshot. None retains this namespace's legacy
    /// staging behavior; Some(false/true) selects Android's disabled/enabled
    /// mode for every image staged by this graph.
    pub(crate) fn link_with_scopes_and_appcompat(
        &self,
        root: &str,
        external: &mut dyn SymbolResolver,
        lifecycle: Option<Arc<dyn DsoLifecycle>>,
        globals: &[GlobalElfImage],
        native_owners: Vec<Arc<dyn std::any::Any + Send + Sync>>,
        scope_options: ScopeLinkOptions<'_>,
    ) -> Result<LoadedElfGraph, NamespaceError> {
        let ScopeLinkOptions {
            namespace_scopes,
            appcompat_16kb,
        } = scope_options;
        self.validate_resident_dependencies(globals)?;
        let mut global_catalog = globals
            .iter()
            .map(GlobalElfImage::catalog)
            .collect::<Result<Vec<_>, _>>()?;
        let mut global_versions = globals
            .iter()
            .map(GlobalElfImage::android_version_definitions)
            .collect::<Result<Vec<_>, _>>()?;
        let mut global_sources: Vec<_> = (0..globals.len())
            .map(symbols::BindingSource::Resident)
            .collect();
        let mut providers = self.providers.clone();
        providers.extend(globals.iter().map(|image| image.soname.clone()));
        let mut builder = GraphBuilder::default();
        if providers.contains(root) {
            return Err(NamespaceError::SonameMismatch {
                supplied: root.to_owned(),
                embedded: None,
            });
        }
        builder.stage_recursive(root, root, &self.sources, &providers, appcompat_16kb)?;

        let layout = builder.compute_scopes(
            builder.indices[root],
            &self.providers,
            &self.resident_dependencies,
            namespace_scopes,
        )?;
        let group_roots: HashMap<String, String> = layout
            .owners
            .iter()
            .enumerate()
            .map(|(image, &owner)| (layout.names[image].clone(), layout.names[owner].clone()))
            .collect();
        let object_sonames = layout.names;
        let scopes = layout.scopes;
        let mapping_owners = layout.owners;
        let mut mapping_dependencies: Vec<Vec<usize>> = layout
            .dependencies
            .into_iter()
            .take(builder.objects.len())
            .map(|children| {
                children
                    .into_iter()
                    .filter(|&i| i < builder.objects.len())
                    .collect()
            })
            .collect();
        if let Some(policy) = namespace_scopes {
            policy.validate(&object_sonames, globals.len())?;
        }
        let mut catalog = builder.export_catalog()?;
        let mut versions = builder.android_version_definitions()?;
        catalog.resize_with(object_sonames.len(), Vec::new);
        versions.resize_with(object_sonames.len(), || None);
        // Resident globals precede newly discovered DF_1_GLOBAL images.
        // Build this before relocating any member so every requester sees the
        // same group, regardless of dependency-first relocation order.
        let resident_global_count = global_catalog.len();
        let mut bindings = vec![Vec::new(); builder.objects.len()];
        let namespace_global_order = if namespace_scopes.is_some() {
            builder
                .compute_scopes(
                    builder.indices[root],
                    &self.providers,
                    &self.resident_dependencies,
                    None,
                )?
                .scopes[builder.indices[root]]
                .clone()
        } else {
            Vec::new()
        };
        for &index in &scopes[builder.indices[root]] {
            if namespace_scopes.is_some() {
                break;
            }
            if index < builder.objects.len()
                && builder.objects[index].staged.image.dynamic_flags_1() & super::DF_1_GLOBAL != 0
            {
                global_catalog.push(catalog[index].clone());
                global_versions.push(versions[index].clone());
                global_sources.push(symbols::BindingSource::Local(index));
            }
        }
        for &index in &builder.dependency_order {
            let scoped_catalog;
            let scoped_sources;
            let scoped_versions;
            let current_sources;
            let current_versions;
            let current_globals = if let Some(policy) = namespace_scopes {
                let namespace = policy.namespace(&object_sonames[index])?;
                let mut selected: Vec<_> = policy.globals[&namespace]
                    .iter()
                    .map(|&i| global_catalog[i].clone())
                    .collect();
                let mut sources: Vec<_> = policy.globals[&namespace]
                    .iter()
                    .map(|&i| global_sources[i])
                    .collect();
                let mut selected_versions: Vec<_> = policy.globals[&namespace]
                    .iter()
                    .map(|&i| global_versions[i].clone())
                    .collect();
                // Newly staged DF_1_GLOBAL members belong to their primary
                // namespace; a public dependency link doesn't promote them.
                for &i in &namespace_global_order {
                    let Some(object) = builder.objects.get(i) else {
                        continue;
                    };
                    if policy.namespace(&object_sonames[i])? == namespace
                        && object.staged.image.dynamic_flags_1() & super::DF_1_GLOBAL != 0
                    {
                        selected.push(catalog[i].clone());
                        selected_versions.push(versions[i].clone());
                        sources.push(symbols::BindingSource::Local(i));
                    }
                }
                debug_assert_eq!(global_catalog.len(), resident_global_count);
                scoped_catalog = selected;
                scoped_sources = sources;
                scoped_versions = selected_versions;
                current_sources = &scoped_sources;
                current_versions = &scoped_versions;
                &scoped_catalog
            } else {
                current_sources = &global_sources;
                current_versions = &global_versions;
                &global_catalog
            };
            let mut resolver = GraphResolver {
                global_sources: current_sources,
                bindings: Vec::new(),
                requester: index,
                object_sonames: &object_sonames,
                scopes: &scopes,
                catalog: &catalog,
                versions: &versions,
                global_catalog: current_globals,
                global_versions: current_versions,
                provider_sonames: &self.providers,
                external,
            };
            let staged = &mut builder.objects[index].staged;
            staged
                .image
                .finish_load(&mut resolver, staged.page_size, &staged.page_protections)
                .map_err(|source| NamespaceError::Load {
                    soname: builder.objects[index].soname.clone(),
                    source,
                })?;
            bindings[index] = resolver.bindings;
            for source in &bindings[index] {
                if let symbols::BindingSource::Local(child) = *source
                    && child < builder.objects.len()
                    && !mapping_dependencies[index].contains(&child)
                {
                    mapping_dependencies[index].push(child);
                }
            }
        }

        // Publish every live reservation only after the full graph has relocated, but before
        // the first constructor can register an image-local `__dso_handle`. Attached lifecycle
        // owners are dropped transactionally if a later publication or constructor fails.
        if let Some(lifecycle) = lifecycle {
            for &index in &builder.dependency_order {
                builder.objects[index]
                    .staged
                    .image
                    .publish_dso_lifecycle(Arc::clone(&lifecycle))
                    .map_err(|message| NamespaceError::Lifecycle {
                        soname: builder.objects[index].soname.clone(),
                        message,
                    })?;
            }
        }

        if let Ok(specification) = std::env::var("DARWIN_ART_ELF_PREFLIGHT_I32")
            && let Some((soname, symbol)) = specification.split_once(':')
            && let Some(object) = builder
                .objects
                .iter()
                .find(|object| object.soname == soname)
        {
            if soname == "libcrypto.so" {
                let base = object
                    .staged
                    .image
                    .debug_mapped_pointer(0)
                    .map_err(|source| NamespaceError::Load {
                        soname: object.soname.clone(),
                        source,
                    })?;
                for slot in [
                    0x17ab70, 0x17abd0, 0x17abe0, 0x17ac38, 0x17ad90, 0x17adf8, 0x17ae00, 0x17ae08,
                    0x17ae10,
                ] {
                    let value =
                        object
                            .staged
                            .image
                            .debug_read_mapped_u64(slot)
                            .map_err(|source| NamespaceError::Load {
                                soname: object.soname.clone(),
                                source,
                            })?;
                    eprintln!(
                        "DARWIN ELF preflight slot: base={base:#x} slot={slot:#x} value={value:#x} offset={:#x}",
                        value.wrapping_sub(base as u64)
                    );
                }
                let instructions =
                    object
                        .staged
                        .image
                        .debug_read_mapped_u64(0xd35dc)
                        .map_err(|source| NamespaceError::Load {
                            soname: object.soname.clone(),
                            source,
                        })?;
                let (guard_address, guard_value) = object.staged.image.debug_stack_guard();
                let entry =
                    object
                        .staged
                        .image
                        .debug_read_mapped_u64(0xd35b4)
                        .map_err(|source| NamespaceError::Load {
                            soname: object.soname.clone(),
                            source,
                        })?;
                let diagnostic_patch =
                    object
                        .staged
                        .image
                        .debug_read_mapped_u64(0xd36ec)
                        .map_err(|source| NamespaceError::Load {
                            soname: object.soname.clone(),
                            source,
                        })?;
                let epilogue =
                    object
                        .staged
                        .image
                        .debug_read_mapped_u64(0xd3940)
                        .map_err(|source| NamespaceError::Load {
                            soname: object.soname.clone(),
                            source,
                        })?;
                eprintln!(
                    "DARWIN ELF preflight guard: address={guard_address:#x} value={guard_value:#x} entry={entry:#018x} instructions={instructions:#018x} diagnostic={diagnostic_patch:#018x} epilogue={epilogue:#018x}"
                );
            }
            let result = object
                .staged
                .image
                .call_exported_i32_before_initializers(symbol)
                .map_err(|source| NamespaceError::Load {
                    soname: object.soname.clone(),
                    source,
                })?;
            eprintln!("DARWIN ELF preflight: soname={soname} symbol={symbol} result={result}");
        }

        let root_index = *builder
            .indices
            .get(root)
            .expect("staged root must have an index");
        let load_order = builder
            .discovery_order
            .iter()
            .map(|&index| builder.objects[index].soname.clone())
            .collect();
        let initialization_order = builder
            .dependency_order
            .iter()
            .map(|&index| builder.objects[index].soname.clone())
            .collect();
        let unload_order = builder
            .dependency_order
            .iter()
            .rev()
            .map(|&index| builder.objects[index].soname.clone())
            .collect();
        let needed: Vec<_> = builder
            .objects
            .iter()
            .map(|object| object.staged.image.needed_libraries().to_vec())
            .collect();
        let objects = builder
            .objects
            .into_iter()
            .map(|object| object.staged.image)
            .collect();
        let external = Arc::new(mappings::ExternalOwners {
            globals: globals.to_vec(),
            native: native_owners,
        });
        let retention = external_owners::ExternalRetention::new(
            external.clone(),
            &object_sonames,
            &needed,
            &bindings,
        );
        let objects = mappings::GroupedMappings::with_resources(
            objects,
            &mapping_owners,
            &builder.dependency_order,
            &mapping_dependencies,
            &retention,
        );
        let selection = Arc::new(selection_metadata::SelectionMetadata {
            indices: builder.indices.clone(),
            group_roots: group_roots.clone(),
            identities: (0..mapping_owners.len())
                .map(|i| objects.identity(i).unwrap())
                .collect(),
        });
        Ok(LoadedElfGraph {
            inner: Arc::new(GraphInner {
                selection,
                objects,
                bindings,
                binding_names: object_sonames,
                external,
                indices: builder.indices,
                root_index,
                group_roots,
                load_order,
                initialization_order,
                unload_order,
                drop_order: builder.dependency_order,
                finalization: crate::finalization::Finalization::default(),
            }),
        })
    }
}

/// One loaded graph owner. Clones share its mappings until the final clone is closed.
///
/// Separate calls to `ClosedElfNamespace::load` create independent graphs; this is deliberately
/// not a namespace-global `dlopen` handle cache.
#[derive(Clone)]
pub struct LoadedElfGraph {
    inner: Arc<GraphInner>,
}

impl LoadedElfGraph {
    /// Original local-group root for a newly mapped member. This identity
    /// survives selected-image retention; it is not an independent unmap lease.
    pub fn local_group_root(&self, soname: &str) -> Option<&str> {
        self.inner.group_roots.get(soname).map(String::as_str)
    }
    /// Run all graph members' destructor passes without releasing mappings.
    /// This does not implement dlclose reference counting or NODELETE policy.
    /// # Safety
    /// The linker owner must establish unload eligibility, serialize guest
    /// operations, and prohibit subsequent initialization/use of torn-down
    /// library state. It retains this group through callbacks and unpublication.
    pub unsafe fn finalize(&self) {
        self.inner.finalize();
    }

    /// Run constructors in dependency order after linker publication. Requires
    /// owner-level open serialization; a repeated/concurrent initialization
    /// is rejected, not treated as completion. No graph mutex spans callbacks.
    pub fn initialize(&self) -> Result<(), NamespaceError> {
        for (soname, &index) in self
            .inner
            .initialization_order
            .iter()
            .zip(&self.inner.drop_order)
        {
            let image = self.inner.objects[index].as_ref().expect("retained image");
            image
                .run_initializers_for_graph()
                .map_err(|source| NamespaceError::Load {
                    soname: soname.clone(),
                    source,
                })?;
        }
        // Preserve eager API's transactional finalizer arming.
        for &index in &self.inner.drop_order {
            self.inner.objects[index]
                .as_ref()
                .expect("retained image")
                .arm_finalizers();
        }
        Ok(())
    }
    /// Query a mapped member by its admitted logical SONAME, including DSOs
    /// without an embedded DT_SONAME. Graph ownership keeps the image live.
    pub fn dynamic_flags_1(&self, soname: &str) -> Option<u64> {
        let index = *self.inner.indices.get(soname)?;
        self.inner.objects[index]
            .as_ref()
            .map(LoadedElf::dynamic_flags_1)
    }
    pub fn load_order(&self) -> &[String] {
        &self.inner.load_order
    }

    pub fn initialization_order(&self) -> &[String] {
        &self.inner.initialization_order
    }

    /// Returns the deterministic reverse-constructor finalization and mapping teardown order.
    pub fn unload_order(&self) -> &[String] {
        &self.inner.unload_order
    }

    pub fn reference_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    pub fn call_root_exported_i32(&self, name: &str) -> Result<i32, LoadError> {
        self.inner.objects[self.inner.root_index]
            .as_ref()
            .expect("live graph object")
            .call_exported_i32(name)
    }

    pub fn lookup_root_exported(&self, name: &str) -> Result<usize, LoadError> {
        self.lookup_root_exported_bytes(name.as_bytes())
    }

    pub fn lookup_root_exported_bytes(&self, name: &[u8]) -> Result<usize, LoadError> {
        self.inner.objects[self.inner.root_index]
            .as_ref()
            .expect("live graph object")
            .lookup_exported_bytes(name)
    }

    pub fn lookup_root_symbol(&self, name: &str) -> Result<usize, LoadError> {
        self.lookup_root_symbol_bytes(name.as_bytes())
    }

    pub fn lookup_root_symbol_bytes(&self, name: &[u8]) -> Result<usize, LoadError> {
        self.inner.objects[self.inner.root_index]
            .as_ref()
            .expect("live graph object")
            .lookup_any_exported_bytes(name)
    }

    pub fn close(self) {}
}

struct GraphInner {
    selection: Arc<selection_metadata::SelectionMetadata>,
    bindings: Vec<Vec<symbols::BindingSource>>,
    binding_names: Vec<String>,
    group_roots: HashMap<String, String>,
    finalization: crate::finalization::Finalization,
    // Mapping owners also retain these resources past a released graph view.
    // Selected images retain this resource owner and their mapping component,
    // not the whole GraphInner.
    external: Arc<mappings::ExternalOwners>,
    indices: HashMap<String, usize>,
    objects: mappings::GroupedMappings,
    root_index: usize,
    load_order: Vec<String>,
    initialization_order: Vec<String>,
    unload_order: Vec<String>,
    drop_order: Vec<usize>,
}

// SAFETY: LoadedElf is Send; mappings/metadata are fixed after relocation.
// Initialization state is atomic, and no mutable reference crosses callbacks.
// The linker owner must serialize initialization and execution of guest code.
unsafe impl Send for GraphInner {}
unsafe impl Sync for GraphInner {}

impl GraphInner {
    fn finalize(&self) {
        // Keep every mapping until all local finalizers have run, matching
        // Android's separate destructor pass before soinfo_free/unmap.
        self.finalization.run(|| {
            for &index in self.drop_order.iter().rev() {
                if let Some(image) = &self.objects[index] {
                    image.finalize_once();
                }
            }
        });
    }
}
impl Drop for GraphInner {
    fn drop(&mut self) {
        // Mapping owners finalize only when their dependency-qualified leases
        // drain. A selected physical mapping may outlive this graph view.
        self.objects.clear();
    }
}

struct GraphObject {
    soname: String,
    staged: StagedElf,
    dependencies: Vec<usize>,
}

#[derive(Default)]
struct GraphBuilder {
    objects: Vec<GraphObject>,
    indices: HashMap<String, usize>,
    visiting: HashSet<String>,
    visited: HashSet<String>,
    discovery_order: Vec<usize>,
    dependency_order: Vec<usize>,
}

impl GraphBuilder {
    fn stage_recursive(
        &mut self,
        requested_by: &str,
        soname: &str,
        sources: &HashMap<String, Vec<u8>>,
        providers: &HashSet<String>,
        appcompat_16kb: Option<bool>,
    ) -> Result<usize, NamespaceError> {
        if let Some(&index) = self.indices.get(soname) {
            return Ok(index);
        }
        let bytes = sources
            .get(soname)
            .ok_or_else(|| NamespaceError::UnknownDependency {
                requested_by: requested_by.to_owned(),
                soname: soname.to_owned(),
            })?;
        let staged = LoadedElf::stage_with_appcompat(bytes, appcompat_16kb).map_err(|source| {
            NamespaceError::Load {
                soname: soname.to_owned(),
                source,
            }
        })?;
        if staged
            .image
            .soname()
            .is_some_and(|embedded| embedded != soname)
        {
            return Err(NamespaceError::SonameMismatch {
                supplied: soname.to_owned(),
                embedded: staged.image.soname().map(ToOwned::to_owned),
            });
        }
        let needed = staged.image.needed_libraries().to_vec();
        let index = self.objects.len();
        self.indices.insert(soname.to_owned(), index);
        self.discovery_order.push(index);
        self.visiting.insert(soname.to_owned());
        self.objects.push(GraphObject {
            soname: soname.to_owned(),
            staged,
            dependencies: Vec::new(),
        });
        for dependency in needed {
            if providers.contains(&dependency) {
                continue;
            }
            let dependency_index =
                self.stage_recursive(soname, &dependency, sources, providers, appcompat_16kb)?;
            self.objects[index].dependencies.push(dependency_index);
        }
        self.visiting.remove(soname);
        if self.visited.insert(soname.to_owned()) {
            self.dependency_order.push(index);
        }
        Ok(index)
    }

    fn compute_scopes(
        &self,
        root: usize,
        providers: &HashSet<String>,
        resident_dependencies: &HashMap<String, Vec<String>>,
        namespace_scopes: Option<&NamespaceScopes>,
    ) -> Result<lookup_layout::Layout, NamespaceError> {
        // Explicit admission scopes determine local group ownership; the legacy
        // closed-namespace call supplies one namespace, not per-DSO subtrees.
        let files: Vec<_> = self
            .objects
            .iter()
            .map(|object| {
                (
                    object.soname.clone(),
                    object.staged.image.needed_libraries().to_vec(),
                )
            })
            .collect();
        lookup_layout::layout_scoped(
            root,
            &files,
            providers,
            resident_dependencies,
            namespace_scopes,
        )
    }

    fn export_catalog(&self) -> Result<Vec<Vec<ExportedSymbol>>, NamespaceError> {
        self.objects
            .iter()
            .map(|object| {
                object
                    .staged
                    .image
                    .exported_symbols()
                    .map_err(|source| NamespaceError::Load {
                        soname: object.soname.clone(),
                        source,
                    })
            })
            .collect()
    }

    fn android_version_definitions(
        &self,
    ) -> Result<Vec<Option<HashMap<u16, String>>>, NamespaceError> {
        self.objects
            .iter()
            .map(|object| {
                object
                    .staged
                    .image
                    .android_version_definitions()
                    .map_err(|source| NamespaceError::Load {
                        soname: object.soname.clone(),
                        source,
                    })
            })
            .collect()
    }
}

#[path = "namespace_symbols.rs"]
mod symbols;
use symbols::GraphResolver;
