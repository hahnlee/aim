//! Explicit directed linker grants, as configured by bionic. All means every
//! library in the direct target, not recursive namespace re-export.
use super::*;

#[derive(Clone)]
enum Libraries {
    Named(BTreeSet<String>),
    All,
}

#[derive(Clone)]
pub(super) struct Link {
    pub(super) target: NamespaceId,
    libraries: Libraries,
}

impl Link {
    pub(super) fn permits(&self, name: &str) -> bool {
        match &self.libraries {
            Libraries::Named(names) => names.contains(name),
            Libraries::All => true,
        }
    }
}

impl<T> NamespaceRegistry<T> {
    /// Android resolve_soname uses the requested basename before loading.
    /// Link order is configuration order; target links are never considered.
    pub(crate) fn path_target(
        &self,
        from: NamespaceId,
        path: &str,
        index: usize,
    ) -> Result<Option<NamespaceId>, NamespaceError> {
        let name = path.rsplit('/').next().unwrap_or("");
        if !library_name(name) {
            return Err(NamespaceError::InvalidLibraryName);
        }
        let namespace = self
            .namespaces
            .get(&from)
            .ok_or(NamespaceError::UnknownNamespace)?;
        Ok(namespace
            .links
            .iter()
            .filter(|link| link.permits(name))
            .nth(index)
            .map(|link| link.target))
    }

    pub fn link(
        &mut self,
        from: NamespaceId,
        to: NamespaceId,
        sonames: BTreeSet<String>,
    ) -> Result<(), NamespaceError> {
        if sonames.is_empty() {
            return Err(NamespaceError::EmptyLink);
        }
        if sonames.iter().any(|name| !library_name(name)) {
            return Err(NamespaceError::InvalidLibraryName);
        }
        self.add_link(from, to, Libraries::Named(sonames))
    }

    /// Only for an explicit allow_all_shared_libs configuration grant.
    /// Empty named lists are still errors, never an implicit wildcard.
    pub fn link_all(&mut self, from: NamespaceId, to: NamespaceId) -> Result<(), NamespaceError> {
        self.add_link(from, to, Libraries::All)
    }

    fn add_link(
        &mut self,
        from: NamespaceId,
        to: NamespaceId,
        libraries: Libraries,
    ) -> Result<(), NamespaceError> {
        if !self.namespaces.contains_key(&to) {
            return Err(NamespaceError::UnknownNamespace);
        }
        let namespace = self
            .namespaces
            .get_mut(&from)
            .ok_or(NamespaceError::UnknownNamespace)?;
        namespace.links.push(Link {
            target: to,
            libraries,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_link_file_search_does_not_reexport_target_links() {
        let mut registry: NamespaceRegistry<()> = NamespaceRegistry::default();
        let mut ids = Vec::new();
        for path in ["/app/lib", "/target/lib", "/private/lib"] {
            ids.push(
                registry
                    .create(
                        NamespaceConfig {
                            isolated: true,
                            search_paths: vec![path.into()],
                            ..Default::default()
                        },
                        None,
                    )
                    .unwrap(),
            );
        }
        registry.link_all(ids[0], ids[1]).unwrap();
        registry.link_all(ids[1], ids[2]).unwrap();
        assert_eq!(
            registry.path_target(ids[0], "/private/libx.so", 0).unwrap(),
            Some(ids[1])
        );
        assert_eq!(
            registry.path_target(ids[0], "/private/libx.so", 1).unwrap(),
            None
        );
        let mut attempted = Vec::new();
        let opened = registry
            .search_plan(ids[0], "libx.so", &[])
            .unwrap()
            .open(|path| {
                attempted.push(path.to_path_buf());
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            })
            .unwrap();
        assert!(opened.is_none());
        assert_eq!(
            attempted,
            vec![
                PathBuf::from("/app/lib/libx.so"),
                PathBuf::from("/target/lib/libx.so")
            ]
        );
    }

    #[test]
    fn all_links_are_direct_live_and_copied_by_shared_creation() {
        let mut registry = NamespaceRegistry::default();
        let app = registry.create(NamespaceConfig::default(), None).unwrap();
        let target = registry.create(NamespaceConfig::default(), None).unwrap();
        let private = registry.create(NamespaceConfig::default(), None).unwrap();
        assert_eq!(
            registry.link(app, target, BTreeSet::new()),
            Err(NamespaceError::EmptyLink)
        );
        registry.link_all(app, target).unwrap();
        registry.link_all(target, private).unwrap();
        let child = registry
            .create(NamespaceConfig::default(), Some(app))
            .unwrap();
        let image = |name: &str| {
            Arc::new(NamespaceImage {
                soname: name.into(),
                path: PathBuf::from("/lib").join(name),
                visibility: ImageVisibility::new(0, false),
                lease: Arc::new(7),
            })
        };
        let late = image("liblate.so");
        registry.publish(target, late.clone()).unwrap();
        registry.publish(private, image("libprivate.so")).unwrap();
        assert!(
            registry
                .find_matching_scoped(target, false, |image| image.soname == "libprivate.so")
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .find_matching_scoped(target, true, |image| image.soname == "libprivate.so")
                .unwrap()
                .is_some()
        );
        assert!(Arc::ptr_eq(
            &registry.find(child, "liblate.so").unwrap().unwrap(),
            &late
        ));
        assert!(registry.find(app, "libprivate.so").unwrap().is_none());
        assert!(registry.find(child, "libprivate.so").unwrap().is_none());
        assert_eq!(
            registry.link_all(app, NamespaceId::from_raw(0)),
            Err(NamespaceError::UnknownNamespace)
        );
    }
}
