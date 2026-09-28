//! Selected resident images and their lifetime, not a process-global dlopen.
use super::*;
#[path = "namespace_group_finalization.rs"]
mod group_finalization;
#[path = "namespace_image_address.rs"]
mod image_address;

/// One explicitly selected image and its dependency-qualified mapping lease.
/// Only this image's exports enter the new lookup group.
#[derive(Clone)]
pub struct GlobalElfImage {
    mapping: mappings::MappingLease,
    metadata: Arc<selection_metadata::SelectionMetadata>,
    external: Arc<mappings::ExternalOwners>,
    pub(super) soname: String,
}

impl LoadedElfGraph {
    pub fn global_image(&self, soname: &str) -> Option<GlobalElfImage> {
        let mapping = self
            .inner
            .objects
            .retain(*self.inner.indices.get(soname)?)?;
        Some(GlobalElfImage {
            external: mapping.external()?,
            mapping,
            metadata: self.inner.selection.clone(),
            soname: soname.to_owned(),
        })
    }
}

impl GlobalElfImage {
    /// Nonzero identity in this loader instance, never reused or persisted.
    /// It identifies the Android local group, not its cycle-retention component.
    pub fn local_group_id(&self) -> u64 {
        self.mapping.group_id()
    }
    pub fn is_local_group_root(&self) -> bool {
        self.metadata.group_roots.get(&self.soname) == Some(&self.soname)
    }
    pub(crate) fn soname(&self) -> &str {
        &self.soname
    }
    pub(crate) fn retained_native_owners(&self) -> &[Arc<dyn std::any::Any + Send + Sync>] {
        &self.external.native
    }
    /// Actual mapped-image identity, not SONAME/path equality. Independent
    /// loads of identical bytes are different; retained clones are identical.
    pub fn same_image(&self, other: &Self) -> bool {
        self.mapping
            .identity()
            .same_image(&other.mapping.identity())
    }
    /// Compare the actual retained mapping group, not a matching root name.
    pub fn same_local_group(&self, other: &Self) -> bool {
        self.mapping.same_group(&other.mapping)
    }

    /// Compare against the graph's retained ELF dependency, never against a
    /// fresh namespace search. None means the edge is absent or its owner is
    /// not represented as an ELF image here (e.g. a native provider), or
    /// same-name global candidates have different identities.
    pub fn matches_dependency(&self, name: &str, candidate: &Self) -> Option<bool> {
        if !self.needed_libraries().iter().any(|needed| needed == name) {
            return None;
        }
        if let Some(&index) = self.metadata.indices.get(name) {
            return self
                .metadata
                .identities
                .get(index)
                .map(|original| original.same_image(&candidate.mapping.identity()));
        }
        let mut candidates = self
            .external
            .globals
            .iter()
            .filter(|image| image.soname == name);
        let original = candidates.next()?;
        if candidates.any(|other| !original.same_image(other)) {
            return None;
        }
        Some(original.same_image(candidate))
    }
    /// Retain the original declared ELF dependency, without a namespace search.
    /// Native/unknown/ambiguous edges return None rather than selecting by name.
    pub fn dependency_image(&self, name: &str) -> Option<Self> {
        if !self.needed_libraries().iter().any(|needed| needed == name) {
            return None;
        }
        if let Some(&index) = self.metadata.indices.get(name) {
            let mapping = self
                .mapping
                .select_dependency(self.metadata.identities.get(index)?)?;
            let external = mapping.external()?;
            return Some(Self {
                mapping,
                metadata: self.metadata.clone(),
                external,
                soname: name.to_owned(),
            });
        }
        let mut candidates = self
            .external
            .globals
            .iter()
            .filter(|image| image.soname == name);
        let original = candidates.next()?;
        if candidates.any(|other| !original.same_image(other)) {
            return None;
        }
        Some(original.clone())
    }
    /// Original DT_NEEDED order, not the graph's initialization order or the
    /// union of its images. Namespace admission still decides each edge.
    pub fn needed_libraries(&self) -> &[String] {
        self.mapping.image().needed_libraries()
    }

    /// Lookup only this admitted image, never its retained private dependencies.
    /// A missing export is not a loader error. Versioned requests require an
    /// exact version; unversioned requests exclude hidden versions.
    pub fn lookup_exported(
        &self,
        name: &[u8],
        version: Option<&str>,
    ) -> Result<Option<usize>, NamespaceError> {
        Ok(selected_export(&self.catalog()?, name, version))
    }

    /// Lookup only this admitted image using Android/Bionic's version policy.
    ///
    /// This intentionally remains additive to `lookup_exported`: the latter
    /// is the strict selected-image API used by existing callers. Android
    /// matching treats an image without DT_VERSYM as matching any explicit
    /// version; with DT_VERSYM it resolves the requested name through the
    /// actual DT_VERDEF table, falls back to VER_NDX_GLOBAL, and compares the
    /// masked symbol version index (so hidden exports can satisfy explicit
    /// requests, while VER_NDX_LOCAL never satisfies GLOBAL1).
    pub fn lookup_android_exported(
        &self,
        name: &[u8],
        version: Option<&str>,
    ) -> Result<Option<usize>, NamespaceError> {
        let catalog = self.catalog()?;
        let definitions = self
            .mapping
            .image()
            .android_version_definitions()
            .map_err(|source| NamespaceError::Load {
                soname: self.soname.clone(),
                source,
            })?;
        Ok(android_versions::selected_export(
            &catalog,
            definitions.as_ref(),
            name,
            version,
        ))
    }

    pub(super) fn catalog(&self) -> Result<Vec<ExportedSymbol>, NamespaceError> {
        self.mapping
            .image()
            .exported_symbols()
            .map_err(|source| NamespaceError::Load {
                soname: self.soname.clone(),
                source,
            })
    }

    pub(super) fn android_version_definitions(
        &self,
    ) -> Result<Option<HashMap<u16, String>>, NamespaceError> {
        self.mapping
            .image()
            .android_version_definitions()
            .map_err(|source| NamespaceError::Load {
                soname: self.soname.clone(),
                source,
            })
    }

    pub(crate) fn dynamic_flags_1(&self) -> u64 {
        self.mapping.image().dynamic_flags_1()
    }

    /// Retain the root of this same original local group, not a fresh lookup.
    pub fn local_group_root_image(&self) -> Option<Self> {
        let name = self.metadata.group_roots.get(&self.soname)?;
        let &index = self.metadata.indices.get(name)?;
        Some(Self {
            mapping: self
                .mapping
                .select_member(self.metadata.identities.get(index)?)?,
            metadata: self.metadata.clone(),
            external: self.external.clone(),
            soname: name.clone(),
        })
    }
}

fn selected_export(
    catalog: &[ExportedSymbol],
    name: &[u8],
    version: Option<&str>,
) -> Option<usize> {
    catalog.iter().find_map(|export| {
        let matches = match version {
            Some(version) => export.version.as_deref() == Some(version),
            None => !export.version_hidden,
        };
        (export.name == name && matches).then_some(export.address)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_exports_preserve_version_visibility() {
        let catalog = [ExportedSymbol {
            name: b"entry".to_vec(),
            address: 42,
            version: Some("V1".into()),
            version_hidden: true,
            version_index: Some(crate::VERSYM_HIDDEN | 2),
        }];
        assert_eq!(selected_export(&catalog, b"entry", Some("V1")), Some(42));
        assert_eq!(selected_export(&catalog, b"entry", None), None);
        assert_eq!(selected_export(&catalog, b"entry", Some("V2")), None);
        assert_eq!(selected_export(&catalog, b"private_dependency", None), None);
        let mut visible = catalog;
        visible[0].version_hidden = false;
        assert_eq!(selected_export(&visible, b"entry", None), Some(42));
    }
}
