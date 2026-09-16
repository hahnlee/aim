//! Resource lifetime tests with the checked-in dependency-free ARM64 image.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Owner(Arc<AtomicUsize>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn native_owner_survives_graph_and_selected_image_clones() {
    let drops = Arc::new(AtomicUsize::new(0));
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
    let graph = namespace
        .load_with_globals_and_owners(
            "libfixture_dep.so",
            &mut RejectAllResolver,
            None,
            &[],
            vec![Arc::new(Owner(drops.clone()))],
        )
        .unwrap();
    let clone = graph.clone();
    let selected = graph.global_image("libfixture_dep.so").unwrap();
    drop(graph);
    drop(clone);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    // Last selected image can be dropped by a different owning thread.
    std::thread::spawn(move || drop(selected)).join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_graph_load_releases_native_owners() {
    let drops = Arc::new(AtomicUsize::new(0));
    let namespace = ClosedElfNamespace::new();
    let result = namespace.load_with_globals_and_owners(
        "missing.so",
        &mut RejectAllResolver,
        None,
        &[],
        vec![Arc::new(Owner(drops.clone()))],
    );
    assert!(result.is_err());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
