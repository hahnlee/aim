use super::*;

fn dependency(name: &[u8]) -> Vec<u8> {
    let mut strings = name.to_vec();
    strings.push(0);
    let mut image = crate::metadata::tests::image(&strings);
    image[152..160].copy_from_slice(&80u64.to_le_bytes());
    image[160..168].copy_from_slice(&80u64.to_le_bytes());
    image[224..232].copy_from_slice(&1i64.to_le_bytes());
    image[232..240].copy_from_slice(&0u64.to_le_bytes());
    image
}

#[test]
fn cycle_edges_require_admission_and_same_identity() {
    let path = std::env::temp_dir().join(format!("elf-edge-admission-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("root"), dependency(b"child.so")).unwrap();
    std::fs::write(path.join("other-root"), dependency(b"child.so")).unwrap();
    std::fs::write(path.join("child"), dependency(b"root.so")).unwrap();
    for mode in 0..4 {
        let mut calls = 0;
        let result =
            discover_admitted_graph(b"root.so", HashSet::new(), &mut false, |name, origin| {
                calls += 1;
                let (file, id) = match origin {
                    None => ("root", 41),
                    Some(origin) if name == b"child.so" => {
                        assert_eq!(origin.image, 41);
                        ("child", 42)
                    }
                    Some(origin) => {
                        assert_eq!(name, b"root.so");
                        assert_eq!(origin.image, 42);
                        if mode == 1 {
                            return Err(FfiFailure::Io("edge denied".into()));
                        }
                        (
                            if mode == 3 { "other-root" } else { "root" },
                            if mode == 2 { 99 } else { 41 },
                        )
                    }
                };
                Ok((File::open(path.join(file)).unwrap(), id))
            });
        assert_eq!(calls, 3, "must consult opener for the cycle edge");
        assert_eq!(result.is_ok(), mode == 0);
        if let Ok(graph) = result {
            assert_eq!(graph.sources.len(), 2);
        }
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn resident_edges_revalidate_and_hold_owners() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let path = std::env::temp_dir().join(format!("elf-resident-edge-{}", std::process::id()));
    let mut bytes = dependency(b"native.so");
    // Add a second DT_NEEDED, relocating strings past the expanded dynamic table.
    bytes[184..192].copy_from_slice(&0x1140u64.to_le_bytes());
    bytes[152..160].copy_from_slice(&96u64.to_le_bytes());
    bytes[160..168].copy_from_slice(&96u64.to_le_bytes());
    bytes[240..248].copy_from_slice(&1i64.to_le_bytes());
    bytes[248..272].fill(0);
    bytes[320..330].copy_from_slice(b"native.so\0");
    std::fs::write(&path, bytes).unwrap();
    for mode in 0..3 {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut calls = 0;
        let graph = discover_resident_graph(b"root.so", &mut false, |name, origin| {
            if origin.is_none() {
                return Ok(AdmittedImage::File(File::open(&path).unwrap(), 1));
            }
            calls += 1;
            assert_eq!(name, b"native.so");
            if mode == 1 && calls == 2 {
                return Err(FfiFailure::Io("second edge denied".into()));
            }
            Ok(AdmittedImage::Resident {
                image: if mode == 2 && calls == 2 { 99 } else { 2 },
                lease: Box::new(Lease(Arc::clone(&drops))),
                needed: Vec::new(),
            })
        });
        assert_eq!(calls, 2);
        assert_eq!(graph.is_ok(), mode == 0);
        if let Ok(graph) = &graph {
            assert_eq!(graph.sources.len(), 1);
            assert_eq!(graph._residents.len(), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
        drop(graph);
        assert_eq!(drops.load(Ordering::SeqCst), if mode == 1 { 1 } else { 2 });
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn resident_dependencies_traverse_cycles_and_reject_changed_identity_or_edges() {
    let path = std::env::temp_dir().join(format!("elf-resident-cycle-{}", std::process::id()));
    std::fs::write(&path, dependency(b"a.so")).unwrap();
    for mode in 0..4 {
        let mut edges = Vec::new();
        let result = discover_resident_graph(b"root.so", &mut false, |name, origin| {
            let Some(origin) = origin else {
                return Ok(AdmittedImage::File(File::open(&path).unwrap(), 1));
            };
            edges.push((origin.image, name.to_vec()));
            let repeated = edges.len() == 3;
            let (image, needed) = if name == b"a.so" {
                (
                    if repeated && mode == 1 { 99 } else { 2 },
                    if repeated && mode == 2 {
                        b"changed.so".to_vec()
                    } else if mode == 3 {
                        b"../escape.so".to_vec()
                    } else {
                        b"b.so".to_vec()
                    },
                )
            } else {
                assert_eq!(name, b"b.so");
                (3, b"a.so".to_vec())
            };
            Ok(AdmittedImage::Resident {
                image,
                needed: vec![needed],
                lease: Box::new(()),
            })
        });
        assert_eq!(result.is_ok(), mode == 0);
        if mode == 3 {
            assert_eq!(edges.len(), 1);
        } else {
            assert_eq!(
                edges,
                [
                    (1, b"a.so".to_vec()),
                    (2, b"b.so".to_vec()),
                    (3, b"a.so".to_vec())
                ]
            );
        }
        if let Ok(graph) = result {
            assert_eq!(graph.sources.len(), 1);
            assert_eq!(graph._residents.len(), 2);
            assert_eq!(graph._residents[0].needed[0].as_bytes(), b"b.so");
            assert_eq!(graph._residents[1].needed[0].as_bytes(), b"a.so");
        }
    }
    std::fs::remove_file(path).unwrap();
}
