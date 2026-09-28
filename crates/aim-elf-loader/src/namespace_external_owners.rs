//! External retention follows actual binding slots and declared DT_NEEDED,
//! not the complete global symbol search snapshot.
use super::*;
pub(super) struct ExternalOwners {
    pub globals: Vec<GlobalElfImage>,
    pub native: Vec<Arc<dyn std::any::Any + Send + Sync>>,
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;
    #[test]
    fn keyed_resources_release_unused_files_and_providers() {
        use image_resource::{ImageResource, ResourceKind};
        use std::sync::Mutex;
        struct Resource(usize, Arc<Mutex<Vec<usize>>>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let keyed = |index, name: &str, kind| -> Arc<dyn std::any::Any + Send + Sync> {
            Arc::new(ImageResource {
                name: name.into(),
                kind,
                value: Arc::new(Resource(index, dropped.clone())),
            })
        };
        let all = Arc::new(ExternalOwners {
            globals: vec![],
            native: vec![
                keyed(0, "root", ResourceKind::SourceFile),
                keyed(1, "child", ResourceKind::SourceFile),
                keyed(2, "unused", ResourceKind::Provider),
                keyed(3, "used", ResourceKind::Provider),
                Arc::new(Resource(4, dropped.clone())), // Legacy unkeyed owner.
            ],
        });
        let retention = ExternalRetention::new(
            all.clone(),
            &[
                "root".into(),
                "child".into(),
                "unused".into(),
                "used".into(),
            ],
            &[vec!["child".into()], vec![]],
            &[vec![], vec![symbols::BindingSource::Local(3)]],
        );
        let child = retention.for_images(&[1]);
        assert_eq!(child.native.len(), 3);
        drop(retention);
        drop(all);
        assert_eq!(&*dropped.lock().unwrap(), &[0, 2]);
        drop(child);
        let mut ids = dropped.lock().unwrap().clone();
        ids.sort();
        assert_eq!(ids, [0, 1, 2, 3, 4]);
    }
    fn image() -> GlobalElfImage {
        let mut namespace = ClosedElfNamespace::new();
        namespace
            .add_elf(
                "libfixture_dep.so",
                include_bytes!(
                    "../../../tools/android-arm64-so-inspect/tests/fixtures/libfixture_dep.so"
                )
                .to_vec(),
            )
            .unwrap();
        namespace
            .load("libfixture_dep.so")
            .unwrap()
            .global_image("libfixture_dep.so")
            .unwrap()
    }
    #[test]
    fn exact_binding_slot_and_unrelocated_needed_are_both_retained() {
        let globals = vec![image(), image()];
        assert!(!globals[0].same_image(&globals[1]));
        let all = Arc::new(ExternalOwners {
            globals,
            native: vec![],
        });
        let retention = ExternalRetention::new(
            all.clone(),
            &["first".into(), "second".into(), "third".into()],
            &[vec![], vec!["libfixture_dep.so".into()], vec![]],
            &[vec![symbols::BindingSource::Resident(1)], vec![], vec![]],
        );
        let bound = retention.for_images(&[0]);
        assert_eq!(bound.globals.len(), 1);
        assert!(bound.globals[0].same_image(&all.globals[1]));
        assert_eq!(retention.for_images(&[1]).globals.len(), 2);
        assert!(retention.for_images(&[2]).globals.is_empty());
        assert_eq!(retention.for_images(&[0, 1, 0]).globals.len(), 2);
    }
}
pub(super) struct ExternalRetention {
    pub all: Arc<ExternalOwners>,
    global_dependencies: Vec<Vec<usize>>,
    native_dependencies: Vec<Vec<usize>>,
}
impl ExternalRetention {
    pub(super) fn new(
        all: Arc<ExternalOwners>,
        names: &[String],
        needed: &[Vec<String>],
        bindings: &[Vec<symbols::BindingSource>],
    ) -> Self {
        assert_eq!(needed.len(), bindings.len());
        assert!(names.len() >= needed.len());
        let native_dependencies = needed
            .iter()
            .zip(bindings)
            .enumerate()
            .map(|(image, (needed, bindings))| {
                let bound: Vec<_> = bindings
                    .iter()
                    .filter_map(|source| match *source {
                        symbols::BindingSource::Local(index) => Some(names[index].as_str()),
                        symbols::BindingSource::Resident(_) => None,
                    })
                    .collect();
                all.native
                    .iter()
                    .enumerate()
                    .filter_map(|(index, owner)| {
                        let required = owner
                            .downcast_ref::<image_resource::ImageResource>()
                            .is_none_or(|resource| {
                                resource.required_by(&names[image], needed, &bound)
                            });
                        required.then_some(index)
                    })
                    .collect()
            })
            .collect();
        let global_dependencies = needed
            .iter()
            .zip(bindings)
            .map(|(needed, bindings)| {
                let mut selected = Vec::new();
                for source in bindings {
                    if let symbols::BindingSource::Resident(index) = *source {
                        assert!(index < all.globals.len());
                        if !selected.contains(&index) {
                            selected.push(index);
                        }
                    }
                }
                // DT_NEEDED can require a DSO without a symbol relocation. Keep
                // all same-name candidates if ambiguous, never guess an identity.
                for (index, image) in all.globals.iter().enumerate() {
                    if needed.contains(&image.soname) && !selected.contains(&index) {
                        selected.push(index);
                    }
                }
                selected
            })
            .collect();
        Self {
            all,
            global_dependencies,
            native_dependencies,
        }
    }
    pub(super) fn for_images(&self, images: &[usize]) -> Arc<ExternalOwners> {
        let mut selected = Vec::new();
        let mut native = Vec::new();
        for &image in images {
            for &index in &self.native_dependencies[image] {
                if !native.contains(&index) {
                    native.push(index);
                }
            }
            for &index in &self.global_dependencies[image] {
                if !selected.contains(&index) {
                    selected.push(index);
                }
            }
        }
        Arc::new(ExternalOwners {
            globals: selected
                .into_iter()
                .map(|index| self.all.globals[index].clone())
                .collect(),
            // Only legacy unkeyed callers remain conservatively retained.
            native: native
                .into_iter()
                .map(|index| self.all.native[index].clone())
                .collect(),
        })
    }
}
