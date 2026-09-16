//! Native linker namespace ownership, below AOSP LibraryNamespaces policy.
//!
//! Callers supply resolved paths and real loaded-image leases. This module does
//! not invent public libraries, translate guest paths, or call dyld globally.
//! IDs are never reused; shared namespaces retain their creation-time image
//! snapshot. NativeLoader's SDK/APEX/ClassLoader policy remains upstream.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[path = "linker_links.rs"]
mod links;
#[path = "linker_ordered_images.rs"]
mod ordered_images;
#[path = "linker_publication.rs"]
mod publication;
use links::Link;

#[path = "linker_search.rs"]
mod search;
pub use search::{NamespaceSearchPlan, OpenedNamespaceFile};
#[path = "linker_runpath.rs"]
mod runpath;
pub use runpath::{expand_runpath, resolve_runpath_directories};
#[path = "linker_visibility.rs"]
mod visibility;
pub use visibility::ImageVisibility;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NamespaceId(u64);

impl NamespaceId {
    pub(crate) fn from_raw(value: u64) -> Self {
        Self(value)
    }
    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct NamespaceConfig {
    pub isolated: bool,
    pub search_paths: Vec<PathBuf>,
    pub default_search_paths: Vec<PathBuf>,
    pub permitted_paths: Vec<PathBuf>,
    pub allowed_libraries: BTreeSet<String>,
}

fn resolved(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

impl NamespaceConfig {
    /// Ordered local candidates only; callers must open/canonicalize through
    /// guest VFS and check access before accepting one. DT_RUNPATH grants no
    /// permission by itself. Link traversal belongs to the namespace owner.
    pub fn search_candidates(
        &self,
        soname: &str,
        runpath: &[PathBuf],
    ) -> Result<Vec<PathBuf>, NamespaceError> {
        if !library_name(soname) {
            return Err(NamespaceError::InvalidLibraryName);
        }
        if runpath.iter().any(|path| !resolved(path)) {
            return Err(NamespaceError::UnresolvedPath);
        }
        Ok(self
            .search_paths
            .iter()
            .chain(runpath)
            .chain(&self.default_search_paths)
            .map(|path| path.join(soname))
            .collect())
    }

    fn validate(&self) -> Result<(), NamespaceError> {
        if self
            .allowed_libraries
            .iter()
            .any(|name| !library_name(name))
        {
            return Err(NamespaceError::InvalidLibraryName);
        }
        if self
            .search_paths
            .iter()
            .chain(&self.default_search_paths)
            .chain(&self.permitted_paths)
            .any(|p| !resolved(p))
        {
            return Err(NamespaceError::UnresolvedPath);
        }
        Ok(())
    }

    // Matches bionic linker_namespaces.cpp's file-access rules: search paths
    // admit only direct children; permitted paths admit descendants. Input must
    // be canonicalized through the guest filesystem before this check so a
    // symlink cannot turn an allowed spelling into an unauthorized mapping.
    pub fn permits(&self, path: &Path) -> bool {
        if !resolved(path) {
            return false;
        }
        if !self.isolated {
            return true;
        }
        if !self.allowed_libraries.is_empty()
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| self.allowed_libraries.contains(name))
        {
            return false;
        }
        self.search_paths
            .iter()
            .chain(&self.default_search_paths)
            .any(|dir| path.parent() == Some(dir.as_path()))
            || self
                .permitted_paths
                .iter()
                .any(|dir| path != dir && path.starts_with(dir))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum NamespaceError {
    UnknownNamespace,
    UnresolvedPath,
    InvalidLibraryName,
    EmptyLink,
    DuplicateExport,
    AccessDenied,
    IdentityExhausted,
    InvalidRunpath,
    ForeignInheritedImage,
}

/// `lease` retains the actual loaded image; the namespace must not retain a
/// bare pointer whose mapping can disappear underneath a lookup.
pub struct NamespaceImage<T> {
    pub visibility: ImageVisibility,
    pub soname: String,
    pub path: PathBuf,
    pub lease: Arc<T>,
}

struct Namespace<T> {
    config: NamespaceConfig,
    images: Vec<Arc<NamespaceImage<T>>>,
    links: Vec<Link>,
}

/// Mutations require exclusive access; the native boundary serializes its
/// registry, rather than interleaving partially published creates/links.
pub struct NamespaceRegistry<T> {
    namespaces: BTreeMap<NamespaceId, Namespace<T>>,
    exports: BTreeMap<String, NamespaceId>,
}

impl<T> Default for NamespaceRegistry<T> {
    fn default() -> Self {
        Self {
            namespaces: BTreeMap::new(),
            exports: BTreeMap::new(),
        }
    }
}

pub(crate) fn library_name(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\0', ':'])
}

impl<T> NamespaceRegistry<T> {
    /// Replace only resolved LD search paths; existing shared children keep
    /// their creation-time configuration snapshot.
    pub fn replace_search_paths(
        &mut self,
        id: NamespaceId,
        paths: Vec<PathBuf>,
    ) -> Result<(), NamespaceError> {
        if paths.iter().any(|path| !resolved(path)) {
            return Err(NamespaceError::UnresolvedPath);
        }
        let owner = self
            .namespaces
            .get_mut(&id)
            .ok_or(NamespaceError::UnknownNamespace)?;
        owner.config.search_paths = paths;
        Ok(())
    }
    /// Snapshot configured default paths, excluding LD_LIBRARY_PATH and links.
    pub fn default_library_paths(&self, id: NamespaceId) -> Result<Vec<PathBuf>, NamespaceError> {
        self.namespaces
            .get(&id)
            .map(|ns| ns.config.default_search_paths.clone())
            .ok_or(NamespaceError::UnknownNamespace)
    }
    /// Snapshot AOSP get_shared_group using actual per-image visibility.
    /// `default_parent` identifies the process default namespace, not its name.
    pub fn shared_group(
        &self,
        parent: NamespaceId,
        default_parent: bool,
    ) -> Result<Vec<Arc<NamespaceImage<T>>>, NamespaceError> {
        let owner = self
            .namespaces
            .get(&parent)
            .ok_or(NamespaceError::UnknownNamespace)?;
        Ok(owner
            .images
            .iter()
            .filter(|image| image.visibility.in_shared_group(default_parent))
            .cloned()
            .collect())
    }
    /// Non-shared Android namespace creation inherits the parent's shared
    /// group, not its paths or links. The linker supplies that group from its
    /// actual soinfo flags: DF_1_GLOBAL for the default namespace, RTLD_GLOBAL
    /// otherwise. Do not infer membership from a filename or dyld visibility.
    /// Validate every lease before creation, so errors cannot publish a partial
    /// namespace. Later parent loads do not alter the child's snapshot.
    pub fn create_with_shared_group(
        &mut self,
        config: NamespaceConfig,
        parent: NamespaceId,
        group: &[Arc<NamespaceImage<T>>],
    ) -> Result<NamespaceId, NamespaceError> {
        let owner = self
            .namespaces
            .get(&parent)
            .ok_or(NamespaceError::UnknownNamespace)?;
        if group.iter().any(|image| {
            !owner
                .images
                .iter()
                .any(|resident| Arc::ptr_eq(resident, image))
        }) {
            return Err(NamespaceError::ForeignInheritedImage);
        }
        let mut inherited = Vec::new();
        for image in group {
            if !inherited
                .iter()
                .any(|resident| Arc::ptr_eq(resident, image))
            {
                inherited.push(Arc::clone(image));
            }
        }
        let id = self.create(config, None)?;
        self.namespaces
            .get_mut(&id)
            .expect("newly created namespace")
            .images = inherited;
        Ok(id)
    }
    /// Snapshot ordered direct-link search scopes. File I/O happens after the
    /// registry lock is released; each result retains its actual target owner.
    pub fn search_plan(
        &self,
        id: NamespaceId,
        soname: &str,
        runpath: &[PathBuf],
    ) -> Result<NamespaceSearchPlan, NamespaceError> {
        if !library_name(soname) {
            return Err(NamespaceError::InvalidLibraryName);
        }
        let namespace = self
            .namespaces
            .get(&id)
            .ok_or(NamespaceError::UnknownNamespace)?;
        let mut scopes = vec![(id, namespace.config.clone())];
        for link in &namespace.links {
            if link.permits(soname) {
                let target = self
                    .namespaces
                    .get(&link.target)
                    .ok_or(NamespaceError::UnknownNamespace)?;
                scopes.push((link.target, target.config.clone()));
            }
        }
        search::NamespaceSearchPlan::new(scopes, soname, runpath)
    }

    pub fn permits(&self, id: NamespaceId, path: &Path) -> Result<bool, NamespaceError> {
        Ok(self
            .namespaces
            .get(&id)
            .ok_or(NamespaceError::UnknownNamespace)?
            .config
            .permits(path))
    }
    pub(crate) fn contains_image(
        &self,
        id: NamespaceId,
        image: &Arc<NamespaceImage<T>>,
    ) -> Result<bool, NamespaceError> {
        Ok(self
            .namespaces
            .get(&id)
            .ok_or(NamespaceError::UnknownNamespace)?
            .images
            .iter()
            .any(|entry| Arc::ptr_eq(entry, image)))
    }
    /// With `shared_parent`, snapshot its images, paths and direct links, as
    /// Android 16 linker.cpp does (the public header's path comment is stale).
    /// Later parent loads/links do not change this snapshot. Linked namespace
    /// targets remain live, not copies. Non-shared group inheritance is supplied as
    /// explicit image leases by the linker owner, not inferred from a parent.
    pub fn create(
        &mut self,
        mut config: NamespaceConfig,
        shared_parent: Option<NamespaceId>,
    ) -> Result<NamespaceId, NamespaceError> {
        config.validate()?;
        let (images, links) = match shared_parent {
            Some(parent) => {
                let parent = self
                    .namespaces
                    .get(&parent)
                    .ok_or(NamespaceError::UnknownNamespace)?;
                config
                    .search_paths
                    .extend(parent.config.search_paths.iter().cloned());
                config
                    .default_search_paths
                    .extend(parent.config.default_search_paths.iter().cloned());
                config
                    .permitted_paths
                    .extend(parent.config.permitted_paths.iter().cloned());
                (parent.images.clone(), parent.links.clone())
            }
            None => (Vec::new(), Vec::new()),
        };
        let id = NEXT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| NamespaceError::IdentityExhausted)?;
        let id = NamespaceId(id);
        self.namespaces.insert(
            id,
            Namespace {
                config,
                images,
                links,
            },
        );
        Ok(id)
    }

    pub fn export(&mut self, name: String, id: NamespaceId) -> Result<(), NamespaceError> {
        if !self.namespaces.contains_key(&id) {
            return Err(NamespaceError::UnknownNamespace);
        }
        if name.is_empty() || name.contains('\0') {
            return Err(NamespaceError::InvalidLibraryName);
        }
        if self.exports.contains_key(&name) {
            return Err(NamespaceError::DuplicateExport);
        }
        self.exports.insert(name, id);
        Ok(())
    }

    pub fn exported(&self, name: &str) -> Option<NamespaceId> {
        self.exports.get(name).copied()
    }

    pub fn publish(
        &mut self,
        id: NamespaceId,
        image: Arc<NamespaceImage<T>>,
    ) -> Result<(), NamespaceError> {
        self.publish_batch(&[(id, image)])
    }

    pub fn find(
        &self,
        id: NamespaceId,
        soname: &str,
    ) -> Result<Option<Arc<NamespaceImage<T>>>, NamespaceError> {
        self.find_matching(id, |image| image.soname == soname)
    }

    // Internal predicate never runs foreign callbacks. Link visibility depends
    // on the matched image's SONAME even when matching file identity.
    pub(crate) fn find_matching(
        &self,
        id: NamespaceId,
        matches: impl Fn(&NamespaceImage<T>) -> bool,
    ) -> Result<Option<Arc<NamespaceImage<T>>>, NamespaceError> {
        self.find_matching_scoped(id, true, matches)
    }

    pub(crate) fn find_matching_scoped(
        &self,
        id: NamespaceId,
        search_links: bool,
        matches: impl Fn(&NamespaceImage<T>) -> bool,
    ) -> Result<Option<Arc<NamespaceImage<T>>>, NamespaceError> {
        let namespace = self
            .namespaces
            .get(&id)
            .ok_or(NamespaceError::UnknownNamespace)?;
        if let Some(image) = namespace.images.iter().find(|image| matches(image)) {
            return Ok(Some(Arc::clone(image)));
        }
        // Links grant named or explicitly all target libraries, never target links. This
        // avoids leaking private transitive dependencies through a public DSO.
        if !search_links {
            return Ok(None);
        }
        for link in &namespace.links {
            let target = self
                .namespaces
                .get(&link.target)
                .ok_or(NamespaceError::UnknownNamespace)?;
            if let Some(image) = target
                .images
                .iter()
                .find(|image| matches(image) && link.permits(&image.soname))
            {
                return Ok(Some(Arc::clone(image)));
            }
        }
        Ok(None)
    }

    /// Outstanding lookup leases remain valid after registry teardown. Native
    /// shutdown must drain those users before unloading the final image lease.
    pub fn clear(&mut self) {
        self.exports.clear();
        self.namespaces.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(name: &str, path: &str) -> Arc<NamespaceImage<u32>> {
        Arc::new(NamespaceImage {
            visibility: ImageVisibility::default(),
            soname: name.into(),
            path: path.into(),
            lease: Arc::new(7),
        })
    }
    #[test]
    fn nonshared_child_inherits_only_authoritative_parent_group() {
        let mut registry = NamespaceRegistry::default();
        let parent = registry.create(NamespaceConfig::default(), None).unwrap();
        let global = image("libglobal.so", "/system/lib64/libglobal.so");
        let local = image("liblocal.so", "/system/lib64/liblocal.so");
        registry.publish(parent, global.clone()).unwrap();
        registry.publish(parent, local).unwrap();
        let child = registry
            .create_with_shared_group(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/app/lib".into()],
                    ..Default::default()
                },
                parent,
                &[global.clone()],
            )
            .unwrap();
        assert!(Arc::ptr_eq(
            &registry.find(child, "libglobal.so").unwrap().unwrap(),
            &global
        ));
        assert!(registry.find(child, "liblocal.so").unwrap().is_none());
        assert!(
            !registry
                .permits(child, Path::new("/system/lib64/new.so"))
                .unwrap()
        );
        registry
            .publish(parent, image("liblate.so", "/system/lib64/liblate.so"))
            .unwrap();
        assert!(registry.find(child, "liblate.so").unwrap().is_none());
        let count = registry.namespaces.len();
        // Same spelling is not the same loaded image/lease.
        let impostor = image("libglobal.so", "/system/lib64/libglobal.so");
        assert_eq!(
            registry.create_with_shared_group(NamespaceConfig::default(), parent, &[impostor]),
            Err(NamespaceError::ForeignInheritedImage)
        );
        assert_eq!(registry.namespaces.len(), count);
        registry.clear();
        assert_eq!(*global.lease, 7);
    }
    #[test]
    fn isolated_paths_are_not_prefix_grants() {
        let config = NamespaceConfig {
            isolated: true,
            search_paths: vec!["/app/lib".into()],
            permitted_paths: vec!["/data/app".into()],
            ..Default::default()
        };
        assert!(config.permits(Path::new("/app/lib/liba.so")));
        assert!(!config.permits(Path::new("/app/lib/nested/liba.so")));
        assert!(config.permits(Path::new("/data/app/nested/liba.so")));
        assert!(!config.permits(Path::new("/data/application/liba.so")));
        assert!(!config.permits(Path::new("/data/app/../outside/liba.so")));
    }
    #[test]
    fn local_search_order_does_not_grant_runpath_access() {
        let config = NamespaceConfig {
            isolated: true,
            search_paths: vec!["/ld".into()],
            default_search_paths: vec!["/default".into()],
            permitted_paths: vec!["/permitted".into()],
            ..Default::default()
        };
        assert_eq!(
            config
                .search_candidates("liba.so", &["/run".into()])
                .unwrap(),
            vec![
                PathBuf::from("/ld/liba.so"),
                "/run/liba.so".into(),
                "/default/liba.so".into()
            ]
        );
        assert!(!config.permits(Path::new("/run/liba.so")));
        assert!(config.permits(Path::new("/default/liba.so")));
        assert_eq!(
            config.search_candidates("../liba.so", &[]),
            Err(NamespaceError::InvalidLibraryName)
        );
        assert_eq!(
            config.search_candidates("liba.so", &["relative".into()]),
            Err(NamespaceError::UnresolvedPath)
        );
    }
    #[test]
    fn publication_checks_actual_basename_and_export_identity() {
        let mut registry = NamespaceRegistry::default();
        let config = NamespaceConfig {
            isolated: true,
            search_paths: vec!["/app/lib".into()],
            allowed_libraries: BTreeSet::from(["liballowed.so".into()]),
            ..Default::default()
        };
        let id = registry.create(config, None).unwrap();
        // SONAME cannot impersonate the actual file's allowed basename.
        assert_eq!(
            registry.publish(id, image("liballowed.so", "/app/lib/libother.so")),
            Err(NamespaceError::AccessDenied)
        );
        assert_eq!(
            registry.publish(id, image("liballowed.so", "/outside/liballowed.so")),
            Err(NamespaceError::AccessDenied)
        );
        registry
            .publish(id, image("liballowed.so", "/app/lib/liballowed.so"))
            .unwrap();
        registry.export("app".into(), id).unwrap();
        assert_eq!(
            registry.export("app".into(), id),
            Err(NamespaceError::DuplicateExport)
        );
        assert_eq!(registry.exported("app"), Some(id));
        let invalid = NamespaceConfig {
            search_paths: vec!["relative".into()],
            ..Default::default()
        };
        assert_eq!(
            registry.create(invalid, None),
            Err(NamespaceError::UnresolvedPath)
        );
    }
    #[test]
    fn explicit_links_do_not_reexport() {
        let mut registry = NamespaceRegistry::default();
        let a = registry.create(Default::default(), None).unwrap();
        let b = registry.create(Default::default(), None).unwrap();
        let c = registry.create(Default::default(), None).unwrap();
        registry
            .publish(b, image("libpublic.so", "/system/libpublic.so"))
            .unwrap();
        registry
            .publish(c, image("libprivate.so", "/system/libprivate.so"))
            .unwrap();
        registry
            .link(b, c, BTreeSet::from(["libprivate.so".into()]))
            .unwrap();
        registry
            .link(
                a,
                b,
                BTreeSet::from(["libpublic.so".into(), "libprivate.so".into()]),
            )
            .unwrap();
        assert!(registry.find(a, "libpublic.so").unwrap().is_some());
        assert!(registry.find(a, "libprivate.so").unwrap().is_none());
        assert!(registry.find(b, "libprivate.so").unwrap().is_some());
        assert_eq!(
            registry.link(a, c, BTreeSet::new()),
            Err(NamespaceError::EmptyLink)
        );
    }
    #[test]
    fn shared_creation_copies_parent_paths_and_links_not_later_changes() {
        let mut registry = NamespaceRegistry::default();
        let parent = registry
            .create(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/parent/lib".into()],
                    permitted_paths: vec!["/parent/data".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let public = registry.create(Default::default(), None).unwrap();
        let later = registry.create(Default::default(), None).unwrap();
        registry
            .link(parent, public, BTreeSet::from(["libpublic.so".into()]))
            .unwrap();
        let child = registry
            .create(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/child/lib".into()],
                    permitted_paths: vec!["/child/data".into()],
                    ..Default::default()
                },
                Some(parent),
            )
            .unwrap();
        let config = &registry.namespaces[&child].config;
        assert_eq!(
            config.search_paths,
            vec![PathBuf::from("/child/lib"), PathBuf::from("/parent/lib")]
        );
        assert_eq!(
            config.permitted_paths,
            vec![PathBuf::from("/child/data"), PathBuf::from("/parent/data")]
        );
        assert!(config.permits(Path::new("/parent/lib/liba.so")));
        assert!(config.permits(Path::new("/parent/data/nested/liba.so")));
        assert!(!config.permits(Path::new("/parent/library/liba.so")));
        // Copied links refer to live target namespaces, including future loads.
        registry
            .publish(public, image("libpublic.so", "/system/libpublic.so"))
            .unwrap();
        assert!(registry.find(child, "libpublic.so").unwrap().is_some());
        registry
            .link(parent, later, BTreeSet::from(["liblate.so".into()]))
            .unwrap();
        registry
            .publish(later, image("liblate.so", "/system/liblate.so"))
            .unwrap();
        assert!(registry.find(parent, "liblate.so").unwrap().is_some());
        assert!(registry.find(child, "liblate.so").unwrap().is_none());
        registry
            .namespaces
            .get_mut(&parent)
            .unwrap()
            .config
            .search_paths
            .push("/new/lib".into());
        assert_eq!(
            registry.permits(child, Path::new("/new/lib/liba.so")),
            Ok(false)
        );
    }

    #[test]
    fn shared_snapshot_and_lookup_lease_survive_owner_clear() {
        let mut registry = NamespaceRegistry::default();
        let parent = registry.create(Default::default(), None).unwrap();
        let loaded = image("libearly.so", "/system/libearly.so");
        let weak = Arc::downgrade(&loaded.lease);
        registry.publish(parent, loaded).unwrap();
        let child = registry.create(Default::default(), Some(parent)).unwrap();
        registry
            .publish(parent, image("liblate.so", "/system/liblate.so"))
            .unwrap();
        assert!(registry.find(child, "liblate.so").unwrap().is_none());
        let lease = registry.find(child, "libearly.so").unwrap().unwrap();
        registry.export("system".into(), parent).unwrap();
        registry.clear();
        assert!(registry.exported("system").is_none());
        assert!(weak.upgrade().is_some());
        drop(lease);
        assert!(weak.upgrade().is_none());
        let next = registry.create(Default::default(), None).unwrap();
        assert_ne!(next, parent);
        assert!(matches!(
            registry.find(parent, "libearly.so"),
            Err(NamespaceError::UnknownNamespace)
        ));
    }
}
