//! Commit admitted image groups under one exclusive registry operation.
use super::*;

impl<T> NamespaceRegistry<T> {
    pub(crate) fn resident_images(&self) -> impl Iterator<Item = &Arc<NamespaceImage<T>>> {
        self.namespaces
            .values()
            .flat_map(|namespace| namespace.images.iter())
    }
    /// Remove exact owned images from all namespace memberships, including
    /// shared snapshots created during initialization. Caller-held Arcs keep
    /// resources alive until after its registry lock is released. Existing
    /// lookup leases remain valid; equal names/paths do not identify an image.
    pub fn unpublish_images(&mut self, images: &[Arc<NamespaceImage<T>>]) -> usize {
        let mut removed = 0;
        for namespace in self.namespaces.values_mut() {
            let previous = namespace.images.len();
            namespace
                .images
                .retain(|entry| !images.iter().any(|image| Arc::ptr_eq(image, entry)));
            removed += previous - namespace.images.len();
        }
        removed
    }
    /// Validate all placements before modifying any namespace. Borrowed owners
    /// remain with the caller on failure; no resource callbacks execute here.
    /// Re-publication of the exact same Arc remains idempotent. This does not
    /// merge different images merely because their SONAMEs match.
    pub fn publish_batch(
        &mut self,
        images: &[(NamespaceId, Arc<NamespaceImage<T>>)],
    ) -> Result<(), NamespaceError> {
        for (id, image) in images {
            if !library_name(&image.soname) {
                return Err(NamespaceError::InvalidLibraryName);
            }
            let namespace = self
                .namespaces
                .get(id)
                .ok_or(NamespaceError::UnknownNamespace)?;
            if !namespace.config.permits(&image.path) {
                return Err(NamespaceError::AccessDenied);
            }
        }
        for (id, image) in images {
            let namespace = self.namespaces.get_mut(id).expect("validated namespace");
            if !namespace
                .images
                .iter()
                .any(|entry| Arc::ptr_eq(entry, image))
            {
                namespace.images.push(Arc::clone(image));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(name: &str, path: &str) -> Arc<NamespaceImage<()>> {
        Arc::new(NamespaceImage {
            visibility: ImageVisibility::default(),
            soname: name.into(),
            path: path.into(),
            lease: Arc::new(()),
        })
    }
    #[test]
    fn rollback_preserves_same_name_image_and_outstanding_lease() {
        let mut registry = NamespaceRegistry::default();
        let parent = registry.create(NamespaceConfig::default(), None).unwrap();
        let original = image("same.so", "/system/same.so");
        let failed = image("same.so", "/system/same.so");
        registry
            .publish_batch(&[(parent, original.clone()), (parent, failed.clone())])
            .unwrap();
        let shared = registry
            .create(NamespaceConfig::default(), Some(parent))
            .unwrap();
        let outstanding = failed.clone();
        assert_eq!(registry.unpublish_images(&[failed.clone()]), 2);
        for id in [parent, shared] {
            assert_eq!(registry.namespaces[&id].images.len(), 1);
            assert!(Arc::ptr_eq(
                &registry.find(id, "same.so").unwrap().unwrap(),
                &original
            ));
        }
        assert!(Arc::ptr_eq(&outstanding, &failed));
        assert_eq!(Arc::strong_count(&failed), 2);
        assert_eq!(registry.unpublish_images(&[failed]), 0);
        assert_eq!(Arc::strong_count(&outstanding), 1);
    }
    #[test]
    fn late_invalid_placement_never_publishes_earlier_members() {
        let mut registry = NamespaceRegistry::default();
        let first = registry.create(NamespaceConfig::default(), None).unwrap();
        let second = registry
            .create(
                NamespaceConfig {
                    isolated: true,
                    search_paths: vec!["/private".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let root = image("root.so", "/system/root.so");
        for (id, bad, expected) in [
            (
                second,
                image("child.so", "/outside/child.so"),
                NamespaceError::AccessDenied,
            ),
            (
                first,
                image("../child.so", "/system/child.so"),
                NamespaceError::InvalidLibraryName,
            ),
            (
                NamespaceId::from_raw(0),
                image("child.so", "/system/child.so"),
                NamespaceError::UnknownNamespace,
            ),
        ] {
            assert_eq!(
                registry.publish_batch(&[(first, root.clone()), (id, bad)]),
                Err(expected)
            );
            assert!(registry.find(first, "root.so").unwrap().is_none());
            assert_eq!(Arc::strong_count(&root), 1);
        }
        let child = image("child.so", "/private/child.so");
        let entries = [(first, root.clone()), (second, child.clone())];
        registry.publish_batch(&entries).unwrap();
        registry.publish_batch(&entries).unwrap();
        assert_eq!(registry.namespaces[&first].images.len(), 1);
        assert_eq!(registry.namespaces[&second].images.len(), 1);
        assert!(Arc::ptr_eq(
            &registry.find(first, "root.so").unwrap().unwrap(),
            &root
        ));
        assert!(registry.find(first, "child.so").unwrap().is_none());
        assert!(Arc::ptr_eq(
            &registry.find(second, "child.so").unwrap().unwrap(),
            &child
        ));
    }
}
