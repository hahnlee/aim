use super::*;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Context {
    mode: u32,
    drops: Arc<AtomicUsize>,
    names: [*const c_char; 1],
}
unsafe extern "C" fn release(pointer: *mut c_void) {
    let drops = unsafe { Box::from_raw(pointer.cast::<Arc<AtomicUsize>>()) };
    drops.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "C" fn admit(
    context: *mut c_void,
    parent: u64,
    _: *const c_char,
    name: *const c_char,
    _: *const c_char,
    output: *mut Admission,
) -> i32 {
    let state = unsafe { &*context.cast::<Context>() };
    assert!(parent == 1 || (state.mode == 4 && parent == 2));
    assert_eq!(unsafe { CStr::from_ptr(name) }.to_bytes(), b"native.so");
    if state.mode == 2 {
        return -1;
    }
    unsafe {
        *output = Admission {
            kind: if state.mode == 1 { 99 } else { 2 },
            fd: -1,
            image: if state.mode == 3 { 0 } else { 2 },
            resident: Box::into_raw(Box::new(Arc::clone(&state.drops))).cast(),
            release: Some(release),
        };
    }
    0
}

#[test]
fn abi_transfers_resident_and_rolls_back_invalid_admission() {
    let path = std::env::temp_dir().join(format!("elf-resident-abi-{}", std::process::id()));
    let mut bytes = crate::metadata::tests::image(b"native.so\0");
    bytes[152..160].copy_from_slice(&80u64.to_le_bytes());
    bytes[160..168].copy_from_slice(&80u64.to_le_bytes());
    bytes[224..232].copy_from_slice(&1i64.to_le_bytes());
    bytes[232..240].fill(0);
    std::fs::write(&path, bytes).unwrap();
    let file = File::open(&path).unwrap();
    for mode in 0..4 {
        let mut state = Context {
            mode,
            drops: Arc::new(AtomicUsize::new(0)),
            names: [c"native.so".as_ptr()],
        };
        let mut graph = ptr::null_mut();
        let mut is_elf = 0;
        let status = unsafe {
            darwin_art_elf_discover_resident_graph(
                file.as_raw_fd(),
                1,
                b"root.so".as_ptr(),
                7,
                Some(admit),
                (&mut state as *mut Context).cast(),
                &mut is_elf,
                &mut graph,
                ptr::null_mut(),
            )
        };
        assert_eq!(is_elf, 1);
        assert_eq!(status == DarwinArtElfStatus::Ok, mode == 0);
        if mode == 0 {
            assert!(!graph.is_null());
            let mut count = 99;
            let mut name = ptr::null();
            let mut image = 0;
            let mut resident = ptr::null_mut();
            unsafe {
                assert_eq!(
                    darwin_art_elf_discovered_graph_resident_count(
                        graph,
                        &mut count,
                        ptr::null_mut()
                    ),
                    DarwinArtElfStatus::Ok
                );
                assert_eq!(count, 1);
                assert_eq!(
                    darwin_art_elf_discovered_graph_resident(
                        graph,
                        0,
                        &mut name,
                        &mut image,
                        &mut resident,
                        ptr::null_mut()
                    ),
                    DarwinArtElfStatus::Ok
                );
                assert_eq!(CStr::from_ptr(name).to_bytes(), b"native.so");
                assert_eq!(image, 2);
                let owner = &*resident.cast::<Arc<AtomicUsize>>();
                assert!(Arc::ptr_eq(owner, &state.drops));
                assert_eq!(
                    darwin_art_elf_discovered_graph_resident(
                        graph,
                        1,
                        &mut name,
                        &mut image,
                        &mut resident,
                        ptr::null_mut()
                    ),
                    DarwinArtElfStatus::InvalidArgument
                );
                assert!(name.is_null() && resident.is_null());
                assert_eq!(image, 0);
                assert_eq!(
                    darwin_art_elf_discovered_graph_resident_count(
                        ptr::null(),
                        &mut count,
                        ptr::null_mut()
                    ),
                    DarwinArtElfStatus::InvalidArgument
                );
                assert_eq!(count, 0);
            }
            assert_eq!(state.drops.load(Ordering::SeqCst), 0);
            drop(unsafe { Box::from_raw(graph) });
        } else {
            assert!(graph.is_null());
        }
        assert_eq!(state.drops.load(Ordering::SeqCst), usize::from(mode != 2));
    }
    drop(file);
    std::fs::remove_file(path).unwrap();
}

unsafe extern "C" fn metadata(
    context: *mut c_void,
    _: *mut c_void,
    names: *mut *const *const c_char,
    count: *mut usize,
) -> i32 {
    let state = unsafe { &*context.cast::<Context>() };
    if state.mode == 6 {
        return -1;
    }
    unsafe {
        *count = 1;
        *names = if state.mode == 5 {
            ptr::null()
        } else {
            state.names.as_ptr()
        };
    }
    0
}

#[test]
fn metadata_abi_traverses_cycle_and_rolls_back_callback_failure() {
    let path = std::env::temp_dir().join(format!("elf-resident-metadata-{}", std::process::id()));
    let mut bytes = crate::metadata::tests::image(b"native.so\0");
    bytes[152..160].copy_from_slice(&80u64.to_le_bytes());
    bytes[160..168].copy_from_slice(&80u64.to_le_bytes());
    bytes[224..232].copy_from_slice(&1i64.to_le_bytes());
    bytes[232..240].fill(0);
    std::fs::write(&path, bytes).unwrap();
    let file = File::open(&path).unwrap();
    for mode in 4..7 {
        let mut state = Context {
            mode,
            drops: Arc::new(AtomicUsize::new(0)),
            names: [c"native.so".as_ptr()],
        };
        let mut graph = ptr::null_mut();
        let mut is_elf = 0;
        let status = unsafe {
            darwin_art_elf_discover_resident_graph_with_edges(
                file.as_raw_fd(),
                1,
                b"root.so".as_ptr(),
                7,
                Some(admit),
                Some(metadata),
                (&mut state as *mut Context).cast(),
                &mut is_elf,
                &mut graph,
                ptr::null_mut(),
            )
        };
        assert_eq!(status == DarwinArtElfStatus::Ok, mode == 4);
        if mode == 4 {
            let graph = unsafe { Box::from_raw(graph) };
            assert_eq!(graph._residents.len(), 1);
            assert_eq!(graph._residents[0].needed[0].as_bytes(), b"native.so");
            assert_eq!(state.drops.load(Ordering::SeqCst), 1);
            drop(graph);
        } else {
            assert!(graph.is_null());
        }
        assert_eq!(
            state.drops.load(Ordering::SeqCst),
            if mode == 4 { 2 } else { 1 }
        );
    }
    drop(file);
    std::fs::remove_file(path).unwrap();
}
