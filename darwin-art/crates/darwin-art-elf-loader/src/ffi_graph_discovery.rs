//! Closed graph construction from caller-admitted files. This engine performs
//! no directory lookup. The opener owns namespace/path policy and receives the
//! referring image's SONAME (None only for the root).
//! Still a single closed SONAME scope, not a multi-namespace symbol resolver.
use super::*;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

pub(super) struct DiscoveryOrigin {
    pub image: u64,
    pub soname: Vec<u8>,
    pub runpath: Option<Vec<u8>>,
}

pub(super) enum AdmittedImage {
    File(File, u64),
    Resident {
        image: u64,
        lease: Box<dyn std::any::Any>,
        needed: Vec<Vec<u8>>,
    },
}

// No global provider-name allowlist: every dependency is admitted by parent.
pub(super) fn discover_resident_graph<F>(
    root: &[u8],
    root_is_elf: &mut bool,
    opener: F,
) -> Result<DarwinArtElfDiscoveredGraph, FfiFailure>
where
    F: FnMut(&[u8], Option<&DiscoveryOrigin>) -> Result<AdmittedImage, FfiFailure>,
{
    discover_with_admission(root, HashSet::new(), root_is_elf, opener)
}

pub(super) fn discover_admitted_graph<F>(
    root_component: &[u8],
    providers: HashSet<Vec<u8>>,
    root_is_elf: &mut bool,
    mut open_image: F,
) -> Result<DarwinArtElfDiscoveredGraph, FfiFailure>
where
    F: FnMut(&[u8], Option<&DiscoveryOrigin>) -> Result<(File, u64), FfiFailure>,
{
    discover_with_admission(root_component, providers, root_is_elf, |name, origin| {
        open_image(name, origin).map(|(file, id)| AdmittedImage::File(file, id))
    })
}

fn discover_with_admission<F>(
    root_component: &[u8],
    providers: HashSet<Vec<u8>>,
    root_is_elf: &mut bool,
    mut open_image: F,
) -> Result<DarwinArtElfDiscoveredGraph, FfiFailure>
where
    F: FnMut(&[u8], Option<&DiscoveryOrigin>) -> Result<AdmittedImage, FfiFailure>,
{
    validate_discovery_component(root_component, "root ELF filename")?;
    let mut queue = VecDeque::from([(
        root_component.to_vec(),
        None::<Vec<u8>>,
        None::<DiscoveryOrigin>,
    )]);
    let mut admitted = HashMap::<Vec<u8>, (u64, u64, u64)>::new();
    let mut discovered_sonames = HashSet::<Vec<u8>>::new();
    let mut names = Vec::<CString>::new();
    let mut graph_bytes = Vec::<Vec<u8>>::new();
    let mut source_images = Vec::new();
    let mut source_files = Vec::new();
    let mut total_size = 0_usize;
    let mut root_soname = None::<CString>;

    let mut first_component = true;
    let mut residents = Vec::<DiscoveredResident>::new();
    while let Some((component, expected_soname, needed_by)) = queue.pop_front() {
        // Admission is per dependency edge, even for a previously seen name.
        // Otherwise a first parent's grant silently authorizes later parents.
        let (file, image) = match open_image(&component, needed_by.as_ref())? {
            AdmittedImage::File(file, image) => (file, image),
            AdmittedImage::Resident {
                image,
                lease,
                needed,
            } => {
                if first_component || image == 0 || admitted.contains_key(&component) {
                    return Err(FfiFailure::Invalid(
                        "invalid/conflicting resident admission",
                    ));
                }
                let needed = needed
                    .into_iter()
                    .map(|name| {
                        validate_discovery_component(&name, "resident DT_NEEDED")?;
                        std::str::from_utf8(&name)
                            .map_err(|_| FfiFailure::Invalid("resident dependency is not UTF-8"))?;
                        CString::new(name)
                            .map_err(|_| FfiFailure::Invalid("resident dependency NUL"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(previous) = residents.iter().find(|r| r.name.as_bytes() == component) {
                    if previous.image != image || previous.needed != needed {
                        return Err(FfiFailure::Format(
                            "closed graph resident identity collision".into(),
                        ));
                    }
                } else {
                    if residents.len() + graph_bytes.len() >= MAX_DISCOVERY_FILES {
                        return Err(FfiFailure::Bounds(
                            "resident graph exceeds image cap".into(),
                        ));
                    }
                    for dependency in &needed {
                        queue.push_back((
                            dependency.as_bytes().to_vec(),
                            Some(dependency.as_bytes().to_vec()),
                            Some(DiscoveryOrigin {
                                image,
                                soname: component.clone(),
                                runpath: None,
                            }),
                        ));
                    }
                    residents.push(DiscoveredResident {
                        name: CString::new(component)
                            .map_err(|_| FfiFailure::Invalid("resident name"))?,
                        image,
                        _lease: lease,
                        needed,
                    });
                }
                continue;
            }
        };
        if residents.iter().any(|r| r.name.as_bytes() == component) {
            return Err(FfiFailure::Format(
                "resident/file identity collision".into(),
            ));
        }
        let metadata = file.metadata().map_err(|e| FfiFailure::Io(e.to_string()))?;
        let identity = (image, metadata.dev(), metadata.ino());
        if let Some(previous) = expected_soname.as_ref().and_then(|name| admitted.get(name)) {
            if *previous != identity {
                return Err(FfiFailure::Format(
                    "closed SONAME graph cannot merge different admitted image identities".into(),
                ));
            }
            continue;
        }
        if graph_bytes.len() + residents.len() >= MAX_DISCOVERY_FILES {
            return Err(FfiFailure::Bounds(format!(
                "ELF sibling graph exceeds the {MAX_DISCOVERY_FILES}-file cap"
            )));
        }
        let bytes = super::ffi_file::read_admitted_file(
            &file,
            first_component.then_some(&mut *root_is_elf),
        )?;
        if first_component {
            first_component = false;
            if !*root_is_elf {
                return Err(FfiFailure::Format("invalid ELF: bad magic".to_owned()));
            }
        }
        total_size = total_size
            .checked_add(bytes.len())
            .ok_or_else(|| FfiFailure::Bounds("ELF graph total size overflow".to_owned()))?;
        if total_size > MAX_DISCOVERY_TOTAL_SIZE {
            return Err(FfiFailure::Bounds(format!(
                "ELF sibling graph exceeds the {MAX_DISCOVERY_TOTAL_SIZE}-byte total cap"
            )));
        }
        let metadata = inspect_elf_metadata(&bytes).map_err(FfiFailure::Load)?;
        if let Some(embedded) = metadata.soname.as_ref() {
            validate_discovery_component(embedded, "embedded DT_SONAME")?;
            if let Some(expected) = expected_soname.as_ref()
                && embedded != expected
            {
                return Err(FfiFailure::Format(format!(
                    "dependency embedded DT_SONAME does not exactly match requested sibling {}",
                    String::from_utf8_lossy(expected)
                )));
            }
        }
        // DT_SONAME is optional for Android DSOs loaded by an explicit APK
        // path. Bionic retains the requested filename as that object's lookup
        // identity when it is absent. Dependencies similarly inherit their
        // exact DT_NEEDED component.
        let logical_soname = metadata
            .soname
            .clone()
            .unwrap_or_else(|| expected_soname.clone().unwrap_or_else(|| component.clone()));
        validate_discovery_component(&logical_soname, "logical ELF SONAME")?;
        if providers.contains(&logical_soname) {
            return Err(FfiFailure::Format(
                "real ELF graph member collides with a builtin provider SONAME".to_owned(),
            ));
        }
        if !discovered_sonames.insert(logical_soname.clone()) {
            return Err(FfiFailure::Format(
                "two graph paths produced the same logical SONAME".to_owned(),
            ));
        }
        admitted.insert(logical_soname.clone(), identity);
        std::str::from_utf8(&logical_soname).map_err(|_| {
            FfiFailure::Format(
                "logical SONAME is not UTF-8; the closed graph namespace cannot key it".to_owned(),
            )
        })?;
        let name =
            CString::new(logical_soname).map_err(|_| FfiFailure::Invalid("SONAME contains NUL"))?;
        if root_soname.is_none() {
            root_soname = Some(name.clone());
        }
        names.push(name);
        graph_bytes.push(bytes);
        source_images.push(image);
        source_files.push(Arc::new(file));

        for needed in metadata.needed_libraries {
            validate_discovery_component(&needed, "DT_NEEDED dependency filename")?;
            if providers.contains(&needed) {
                continue;
            }
            std::str::from_utf8(&needed).map_err(|_| {
                FfiFailure::Format(
                    "DT_NEEDED dependency filename is not UTF-8; the closed graph namespace cannot key it"
                        .to_owned(),
                )
            })?;
            queue.push_back((
                needed.clone(),
                Some(needed),
                Some(DiscoveryOrigin {
                    image,
                    soname: names.last().expect("parent admitted").as_bytes().to_vec(),
                    runpath: metadata.runpath.clone(),
                }),
            ));
        }
    }

    let mut sources = Vec::with_capacity(names.len());
    for (name, bytes) in names.iter().zip(&graph_bytes) {
        sources.push(DarwinArtElfGraphSource {
            soname: name.as_ptr(),
            bytes: bytes.as_ptr(),
            length: bytes.len(),
        });
    }
    Ok(DarwinArtElfDiscoveredGraph {
        namespace_scopes: None,
        appcompat_16kb: None,
        root_soname: root_soname.expect("nonempty discovery has one root"),
        _names: names,
        _bytes: graph_bytes,
        sources,
        source_images,
        source_files,
        _residents: residents,
    })
}

#[cfg(test)]
#[path = "ffi_graph_discovery_edge_tests.rs"]
mod edge_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependency_opener_receives_referrers_raw_runpath() {
        let path = std::env::temp_dir().join(format!(
            "darwin-origin-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        let mut root = crate::metadata::tests::image(b"$ORIGIN\0child.so\0");
        root[152..160].copy_from_slice(&80u64.to_le_bytes());
        root[160..168].copy_from_slice(&80u64.to_le_bytes());
        root[224..232].copy_from_slice(&1i64.to_le_bytes()); // DT_NEEDED
        root[232..240].copy_from_slice(&8u64.to_le_bytes());
        std::fs::write(path.join("root"), root).unwrap();
        std::fs::write(path.join("child"), crate::metadata::tests::image(b"\0")).unwrap();
        let mut calls = 0;
        let result =
            discover_admitted_graph(b"root.so", HashSet::new(), &mut false, |name, origin| {
                calls += 1;
                let file = if let Some(origin) = origin {
                    assert_eq!(name, b"child.so");
                    assert_eq!(origin.soname, b"root.so");
                    assert_eq!(origin.image, 41);
                    assert_eq!(origin.runpath.as_deref(), Some(&b"$ORIGIN"[..]));
                    "child"
                } else {
                    "root"
                };
                File::open(path.join(file))
                    .map(|file| (file, if origin.is_none() { 41 } else { 42 }))
                    .map_err(|error| FfiFailure::Io(error.to_string()))
            })
            .unwrap_or_else(|_| panic!("graph discovery failed"));
        assert_eq!(result.sources.len(), 2);
        assert_eq!(result.source_images, [41, 42]);
        let original = result.source_files[0].metadata().unwrap();
        std::fs::remove_file(path.join("root")).unwrap();
        std::fs::write(path.join("root"), b"replacement").unwrap();
        assert_ne!(
            std::fs::metadata(path.join("root")).unwrap().ino(),
            original.ino()
        );
        let mut file_identity = ffi_discovery_identity::DarwinArtElfFileIdentity::default();
        assert_eq!(
            unsafe {
                ffi_discovery_identity::darwin_art_elf_discovered_graph_file_identity(
                    &result,
                    0,
                    &mut file_identity,
                    ptr::null_mut(),
                )
            },
            DarwinArtElfStatus::Ok
        );
        assert_eq!(
            (
                file_identity.device,
                file_identity.inode,
                file_identity.offset
            ),
            (original.dev(), original.ino(), 0)
        );
        assert_eq!(
            unsafe {
                ffi_discovery_identity::darwin_art_elf_discovered_graph_file_identity(
                    &result,
                    99,
                    &mut file_identity,
                    ptr::null_mut(),
                )
            },
            DarwinArtElfStatus::InvalidArgument
        );
        assert_eq!(
            (
                file_identity.device,
                file_identity.inode,
                file_identity.offset
            ),
            (0, 0, 0)
        );
        let mut output = 0;
        for (index, expected) in [(0, 41), (1, 42)] {
            assert_eq!(
                unsafe {
                    ffi_discovery_identity::darwin_art_elf_discovered_graph_source_image(
                        &result,
                        index,
                        &mut output,
                        ptr::null_mut(),
                    )
                },
                DarwinArtElfStatus::Ok
            );
            assert_eq!(output, expected);
        }
        assert_eq!(
            unsafe {
                ffi_discovery_identity::darwin_art_elf_discovered_graph_source_image(
                    &result,
                    2,
                    &mut output,
                    ptr::null_mut(),
                )
            },
            DarwinArtElfStatus::InvalidArgument
        );
        assert_eq!(output, 0);
        output = 99;
        assert_eq!(
            unsafe {
                ffi_discovery_identity::darwin_art_elf_discovered_graph_source_image(
                    ptr::null(),
                    0,
                    &mut output,
                    ptr::null_mut(),
                )
            },
            DarwinArtElfStatus::InvalidArgument
        );
        assert_eq!(output, 0);
        assert_eq!(calls, 2);
        std::fs::remove_dir_all(path).unwrap();
    }
}
