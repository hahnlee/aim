//! Preserve the original admitted descriptor under its exact closed-graph key.
use super::*;
pub(super) fn retain_files(
    graph: &DarwinArtElfDiscoveredGraph,
) -> Result<Vec<Arc<dyn std::any::Any + Send + Sync>>, FfiFailure> {
    if graph._names.len() != graph.source_files.len() {
        return Err(FfiFailure::Invalid("source descriptor/name count mismatch"));
    }
    graph
        ._names
        .iter()
        .zip(&graph.source_files)
        .map(|(name, file)| {
            Ok(Arc::new(crate::namespace::image_resource::ImageResource {
                name: name
                    .to_str()
                    .map_err(|_| FfiFailure::Invalid("source name is not UTF-8"))?
                    .to_owned(),
                kind: crate::namespace::image_resource::ResourceKind::SourceFile,
                value: file.clone(),
            }) as Arc<dyn std::any::Any + Send + Sync>)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admitted_descriptor_survives_discovery_then_closes_with_its_resource() {
        let name = b"libfixture_dep.so";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/android-arm64-so-inspect/tests/fixtures/libfixture_dep.so");
        let mut is_elf = false;
        let graph = ffi_graph_discovery::discover_admitted_graph(
            name,
            HashSet::new(),
            &mut is_elf,
            |requested, _| {
                assert_eq!(requested, name);
                Ok((File::open(&path).unwrap(), 1))
            },
        )
        .ok()
        .unwrap();
        assert!(is_elf);
        let original = Arc::downgrade(&graph.source_files[0]);
        let owners = retain_files(&graph).ok().unwrap();
        drop(graph);
        assert!(original.upgrade().is_some());
        let resource = owners[0]
            .downcast_ref::<crate::namespace::image_resource::ImageResource>()
            .unwrap();
        assert_eq!(resource.name, "libfixture_dep.so");
        assert!(
            resource
                .value
                .downcast_ref::<File>()
                .unwrap()
                .metadata()
                .is_ok()
        );
        drop(owners);
        assert!(original.upgrade().is_none());
    }
}
