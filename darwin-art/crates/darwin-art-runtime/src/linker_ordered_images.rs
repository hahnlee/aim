//! Namespace soinfo-list order, distinct from a registry-wide image union.
use super::*;

impl<T> NamespaceRegistry<T> {
    /// Includes local images. The caller applies linear dlsym eligibility while
    /// walking; sorting, SONAME deduplication and following namespace links would
    /// change Android lookup precedence. Leases survive later removal.
    pub fn ordered_images(
        &self,
        id: NamespaceId,
    ) -> Result<Vec<Arc<NamespaceImage<T>>>, NamespaceError> {
        self.namespaces
            .get(&id)
            .map(|ns| ns.images.clone())
            .ok_or(NamespaceError::UnknownNamespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(name: &str) -> Arc<NamespaceImage<()>> {
        Arc::new(NamespaceImage {
            visibility: ImageVisibility::default(),
            soname: name.into(),
            path: format!("/system/{name}").into(),
            lease: Arc::new(()),
        })
    }
    #[test]
    fn order_identity_namespace_and_retention_are_preserved() {
        let mut registry = NamespaceRegistry::default();
        let first = registry.create(NamespaceConfig::default(), None).unwrap();
        let other = registry.create(NamespaceConfig::default(), None).unwrap();
        let z = image("z.so");
        let a = image("a.so");
        let same_name = image("z.so");
        registry
            .publish_batch(&[
                (other, image("foreign.so")),
                (first, z.clone()),
                (first, a.clone()),
                (first, same_name.clone()),
                (first, z.clone()),
            ])
            .unwrap();
        let snapshot = registry.ordered_images(first).unwrap();
        assert_eq!(snapshot.len(), 3);
        assert!(Arc::ptr_eq(&snapshot[0], &z));
        assert!(Arc::ptr_eq(&snapshot[1], &a));
        assert!(Arc::ptr_eq(&snapshot[2], &same_name));
        let child = registry
            .create(NamespaceConfig::default(), Some(first))
            .unwrap();
        registry
            .publish_batch(&[(first, image("later.so"))])
            .unwrap();
        assert_eq!(registry.ordered_images(child).unwrap().len(), 3);
        registry.unpublish_images(&[z.clone()]);
        assert_eq!(registry.ordered_images(first).unwrap().len(), 3);
        drop(registry);
        assert!(Arc::ptr_eq(&snapshot[0], &z));
        let empty: NamespaceRegistry<()> = NamespaceRegistry::default();
        assert!(empty.ordered_images(first).is_err());
    }
}
