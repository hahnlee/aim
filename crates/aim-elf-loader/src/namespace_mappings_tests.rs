use super::*;
use std::sync::{Arc, Mutex};

// This is a checked-in, dependency-free ARM64 ET_DYN fixture. Loading it
// exercises the same reservation/drop path as production images without
// fabricating LoadedElf's mmap-owned state in a test.
const FIXTURE: &[u8] =
    include_bytes!("../../../tools/android-arm64-so-inspect/tests/fixtures/libfixture_dep.so");

fn image() -> LoadedElf {
    LoadedElf::load(FIXTURE).expect("checked-in ARM64 ELF fixture loads")
}

struct Lifecycle {
    id: usize,
    finalized: Arc<Mutex<Vec<usize>>>,
}

impl DsoLifecycle for Lifecycle {
    fn publish_image(&self, _range: std::ops::Range<usize>) -> Result<(), String> {
        Ok(())
    }

    fn finalize_image(&self, _range: std::ops::Range<usize>) -> Result<(), String> {
        self.finalized.lock().unwrap().push(self.id);
        Ok(())
    }
}

fn image_with_lifecycle(id: usize, finalized: &Arc<Mutex<Vec<usize>>>) -> LoadedElf {
    let mut image = image();
    image
        .publish_dso_lifecycle(Arc::new(Lifecycle {
            id,
            finalized: Arc::clone(finalized),
        }))
        .expect("lifecycle publication succeeds");
    image
}

#[test]
fn new_preserves_group_layout_and_local_image_locations() {
    let mappings = GroupedMappings::new(vec![image(), image(), image()], &[7, 7, 9], &[2, 0, 1]);

    assert_eq!(mappings.groups.len(), 2);
    assert_eq!(mappings.locations, vec![(0, 0), (0, 1), (1, 0)]);
    assert_eq!(mappings.groups[0].as_ref().unwrap().images.len(), 2);
    assert_eq!(mappings.groups[0].as_ref().unwrap().order, vec![0, 1]);
    assert_eq!(mappings.groups[1].as_ref().unwrap().images.len(), 1);
    assert_eq!(mappings.groups[1].as_ref().unwrap().order, vec![0]);
    assert!(mappings[0].is_some());
    assert!(mappings[1].is_some());
    assert!(mappings[2].is_some());
}

#[test]
fn identity_distinguishes_local_group_image_and_independent_load() {
    let mappings = GroupedMappings::new(vec![image(), image(), image()], &[7, 7, 9], &[0, 1, 2]);

    assert!(mappings.same_group(0, &mappings, 1));
    assert!(!mappings.same_group(0, &mappings, 2));
    assert!(mappings.same_image(0, &mappings, 0));
    assert!(!mappings.same_image(0, &mappings, 1));
    assert!(!mappings.same_group(99, &mappings, 0));
    assert!(!mappings.same_image(0, &mappings, 99));

    let independent = GroupedMappings::new(vec![image(), image()], &[7, 7], &[0, 1]);
    assert!(!mappings.same_group(0, &independent, 0));
    assert!(!mappings.same_image(0, &independent, 0));
}

#[test]
fn retained_local_group_members_and_clones_share_nonzero_group_id() {
    let mappings = GroupedMappings::new(vec![image(), image(), image()], &[7, 7, 9], &[0, 1, 2]);
    let root = mappings.retain(0).unwrap();
    let member = mappings.retain(1).unwrap();
    let independent_group = mappings.retain(2).unwrap();
    let clone = root.clone();

    assert_ne!(root.group_id(), 0);
    assert_eq!(root.group_id(), member.group_id());
    assert_eq!(root.group_id(), clone.group_id());
    assert_ne!(root.group_id(), independent_group.group_id());
    assert!(independent_group.group_id() > root.group_id());
}

#[test]
fn independent_mapping_groups_have_distinct_group_ids() {
    let first = GroupedMappings::new(vec![image()], &[7], &[0]);
    let second = GroupedMappings::new(vec![image()], &[7], &[0]);
    let first = first.retain(0).unwrap();
    let second = second.retain(0).unwrap();

    assert_ne!(first.group_id(), 0);
    assert_ne!(second.group_id(), 0);
    assert_ne!(first.group_id(), second.group_id());
    assert!(second.group_id() > first.group_id());
}

#[test]
fn cross_group_lifetime_scc_preserves_original_group_ids() {
    let mappings = GroupedMappings::with_dependencies(
        vec![image(), image()],
        &[0, 1],
        &[1, 0],
        &[vec![1], vec![0]],
        None,
    );
    let root = mappings.retain(0).unwrap();
    let child = mappings.retain(1).unwrap();
    let root_id = root.group_id();
    let child_id = child.group_id();

    assert_ne!(root_id, 0);
    assert_ne!(child_id, 0);
    assert_ne!(root_id, child_id);
    assert!(child_id > root_id);
    drop(mappings);
    assert!(!root.image().exported_symbols().unwrap().is_empty());
    assert!(!child.image().exported_symbols().unwrap().is_empty());
}

#[test]
fn retained_views_share_group_identity_without_releasing_source_mappings() {
    let source = GroupedMappings::new(vec![image(), image()], &[7, 7], &[0, 1]);
    let retained = |image| GroupedMappings {
        groups: source.groups.iter().cloned().collect(),
        locations: vec![source.locations[image]],
        release_order: source.release_order.clone(),
        owners: source.owners.clone(),
        group_owners: source.group_owners.clone(),
    };
    let first = retained(0);
    let second = retained(0);

    assert!(first.same_group(0, &second, 0));
    assert!(first.same_image(0, &second, 0));
    drop(first);
    assert!(source[0].is_some());
    assert!(source[1].is_some());
    drop(second);
    assert!(source[0].is_some());
    assert!(source[1].is_some());
}

#[test]
fn clear_finalizes_groups_in_reverse_dependency_and_member_order() {
    let finalized = Arc::new(Mutex::new(Vec::new()));
    let mut mappings = GroupedMappings::new(
        vec![
            image_with_lifecycle(0, &finalized),
            image_with_lifecycle(1, &finalized),
            image_with_lifecycle(2, &finalized),
        ],
        &[7, 7, 9],
        &[0, 1, 2],
    );

    mappings.clear();
    assert_eq!(&*finalized.lock().unwrap(), &[2, 1, 0]);
    assert!(mappings.groups.iter().all(Option::is_none));
}

#[test]
fn retained_child_does_not_keep_parent_alive() {
    let finalized = Arc::new(Mutex::new(Vec::new()));
    let mappings = GroupedMappings::with_dependencies(
        vec![
            image_with_lifecycle(0, &finalized),
            image_with_lifecycle(1, &finalized),
        ],
        &[0, 1],
        &[1, 0],
        &[vec![1], vec![]],
        None,
    );
    let child = mappings.retain(1).unwrap();
    drop(mappings);
    assert_eq!(&*finalized.lock().unwrap(), &[0]);
    assert!(!child.image().exported_symbols().unwrap().is_empty());
    drop(child);
    assert_eq!(&*finalized.lock().unwrap(), &[0, 1]);
}

#[test]
fn retained_parent_keeps_dependencies_and_cycles_do_not_leak() {
    for cycle in [false, true] {
        let finalized = Arc::new(Mutex::new(Vec::new()));
        let mappings = GroupedMappings::with_dependencies(
            vec![
                image_with_lifecycle(0, &finalized),
                image_with_lifecycle(1, &finalized),
            ],
            &[0, 1],
            &[1, 0],
            &[vec![1], if cycle { vec![0] } else { vec![] }],
            None,
        );
        assert!(!mappings.same_group(0, &mappings, 1));
        let lease = mappings.retain(if cycle { 1 } else { 0 }).unwrap();
        drop(mappings);
        assert!(finalized.lock().unwrap().is_empty());
        drop(lease);
        assert_eq!(&*finalized.lock().unwrap(), &[0, 1]);
    }
}

#[test]
fn external_resources_outlive_graph_until_last_mapping_lease() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Resource(Arc<AtomicUsize>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let external = Arc::new(ExternalOwners {
        globals: vec![],
        native: vec![Arc::new(Resource(drops.clone()))],
    });
    let mappings = GroupedMappings::with_dependencies(
        vec![image(), image()],
        &[0, 1],
        &[1, 0],
        &[vec![1], vec![]],
        Some(external),
    );
    let child = mappings.retain(1).unwrap();
    drop(mappings);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    std::thread::spawn(move || drop(child)).join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn dependency_selection_cannot_escape_retained_owner_dag() {
    let mappings = GroupedMappings::with_dependencies(
        vec![image(), image(), image(), image()],
        &[0, 1, 2, 3],
        &[2, 1, 0, 3],
        &[vec![1], vec![2], vec![], vec![]],
        None,
    );
    let parent = mappings.retain(0).unwrap();
    let child = mappings.retain(1).unwrap();
    let leaf = mappings.retain(2).unwrap();
    let unrelated = mappings.retain(3).unwrap();
    assert!(parent.select_dependency(&child.identity()).is_some());
    assert!(parent.select_dependency(&leaf.identity()).is_some());
    assert!(parent.select_dependency(&unrelated.identity()).is_none());
    assert!(child.select_dependency(&parent.identity()).is_none());
    let selected = parent.select_dependency(&child.identity()).unwrap();
    let parent_identity = parent.identity();
    drop(mappings);
    drop(parent);
    assert!(selected.select_dependency(&parent_identity).is_none());
    assert!(selected.same_group(&child));
}

#[test]
fn concurrent_child_release_cannot_unmap_during_parent_finalizer() {
    use std::sync::mpsc;
    use std::time::Duration;
    struct BlockingFinalizer {
        entered: mpsc::Sender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
    }
    impl DsoLifecycle for BlockingFinalizer {
        fn publish_image(&self, _: std::ops::Range<usize>) -> Result<(), String> {
            Ok(())
        }
        fn finalize_image(&self, _: std::ops::Range<usize>) -> Result<(), String> {
            self.entered.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(())
        }
    }
    let (entered, waiting) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let mut parent = image();
    parent
        .publish_dso_lifecycle(Arc::new(BlockingFinalizer {
            entered,
            resume: Mutex::new(resumed),
        }))
        .unwrap();
    let finalized = Arc::new(Mutex::new(Vec::new()));
    let mappings = GroupedMappings::with_dependencies(
        vec![parent, image_with_lifecycle(1, &finalized)],
        &[0, 1],
        &[1, 0],
        &[vec![1], vec![]],
        None,
    );
    let parent = mappings.retain(0).unwrap();
    let child = mappings.retain(1).unwrap();
    drop(mappings);
    let worker = std::thread::spawn(move || drop(parent));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(child);
    assert!(finalized.lock().unwrap().is_empty());
    resume.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(&*finalized.lock().unwrap(), &[1]);
}
