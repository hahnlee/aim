//! Explicit path admission into an already-selected namespace. Namespace-link
//! selection belongs to the caller; this does not turn paths into SONAME search.
use super::*;
use std::os::{fd::IntoRawFd, unix::ffi::OsStrExt, unix::fs::MetadataExt};
type Open = unsafe extern "C" fn(*const c_char, *mut c_char, usize, *mut i32) -> i32;

fn is_virtual_system_provider(image: &NamespaceImage<ImageOwner>) -> bool {
    matches!(image.lease.kind, 3..=8) && image.lease.file_identity.is_none()
}
/// # Safety
/// Same guest filesystem callback/output contract as namespace_open. Registry
/// is live; callbacks run outside its lock. 0 owned fd, 1 absent, negative error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_open_path(
    registry: *const LinkerRegistry,
    id: u64,
    path: *const c_char,
    opener: Option<Open>,
    errno: Option<unsafe extern "C" fn() -> i32>,
    canonical: *mut c_char,
    capacity: usize,
    output: *mut i32,
) -> i32 {
    unsafe {
        darwin_art_linker_namespace_open_path_resident(
            registry,
            id,
            path,
            opener,
            errno,
            canonical,
            capacity,
            output,
            std::ptr::null_mut(),
            1,
        )
    }
}

/// # Safety
/// Same contract as open_path, with an optional writable resident output.
/// 2 transfers an existing visible image lease (fd remains -1, path empty).
/// AOSP checks inode residency before new-file path accessibility. Supplying
/// no resident output requests strict file admission without residency reuse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_open_path_resident(
    registry: *const LinkerRegistry,
    id: u64,
    path: *const c_char,
    opener: Option<Open>,
    errno: Option<unsafe extern "C" fn() -> i32>,
    canonical: *mut c_char,
    capacity: usize,
    output: *mut i32,
    resident: *mut *mut LinkerImageLease,
    search_links: u8,
) -> i32 {
    if !resident.is_null() {
        unsafe { *resident = std::ptr::null_mut() };
    }
    if output.is_null() || canonical.is_null() || capacity == 0 {
        return -1;
    }
    unsafe {
        *output = -1;
        *canonical = 0;
    }
    if search_links > 1 {
        return -1;
    }
    let (Some(registry), Some(opener), Some(errno)) = (unsafe { registry.as_ref() }, opener, errno)
    else {
        return -1;
    };
    if path.is_null() {
        return -1;
    }
    let path_bytes = unsafe { CStr::from_ptr(path) }.to_bytes().to_vec();
    let path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes.clone()));
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        if state.permits(NamespaceId::from_raw(id), &path).is_err() {
            return -1;
        }
        // Ported Android system libraries have a canonical guest pathname but
        // deliberately have no guest filesystem inode.  Their published
        // native-image identity is therefore the exact canonical path plus
        // namespace visibility.  Reuse that resident image before asking the
        // guest VFS to open a file which must not exist on macOS.
        if !resident.is_null() {
            match state.find_matching_scoped(
                NamespaceId::from_raw(id),
                search_links != 0,
                |image| {
                    is_virtual_system_provider(image)
                        && image.path.as_os_str().as_bytes() == path_bytes
                },
            ) {
                Ok(Some(image)) => {
                    unsafe {
                        *resident = Box::into_raw(Box::new(LinkerImageLease(image, None)));
                    }
                    return 2;
                }
                Ok(None) => {}
                Err(_) => return -1,
            }
        }
    }
    let (path, file) = match unsafe { open_file::open_guest_node(&path, opener, errno) } {
        Ok(opened) => opened,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 1,
        Err(_) => return -3,
    };
    let Ok(metadata) = file.metadata() else {
        return -3;
    };
    if !metadata.is_file() {
        return -3;
    }
    if !resident.is_null() {
        match unsafe {
            file_lookup::darwin_art_linker_namespace_find_file_scoped(
                registry,
                id,
                metadata.dev(),
                metadata.ino(),
                0,
                search_links,
                resident,
            )
        } {
            0 => return 2,
            1 => {}
            status => return status,
        }
    }
    {
        let Ok(state) = registry.0.lock() else {
            return -2;
        };
        if state.permits(NamespaceId::from_raw(id), &path) != Ok(true) {
            return -1;
        }
    }
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= capacity {
        return -1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), canonical.cast(), bytes.len());
        *canonical.add(bytes.len()) = 0;
        *output = file.into_raw_fd();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linker_namespace::ImageVisibility;
    use std::ffi::{CStr, CString, OsString};
    use std::fs::{self, File};
    use std::io::Read;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPEN_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_missing_open(
        _path: *const c_char,
        _canonical: *mut c_char,
        _capacity: usize,
        _output: *mut i32,
    ) -> i32 {
        OPEN_CALLS.fetch_add(1, Ordering::Relaxed);
        -1
    }

    unsafe extern "C" fn release_noop(_value: *mut c_void) {}

    fn test_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "darwin-linker-path-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    unsafe extern "C" fn open_host_file(
        path: *const c_char,
        canonical: *mut c_char,
        capacity: usize,
        output: *mut i32,
    ) -> i32 {
        if path.is_null() || canonical.is_null() || capacity == 0 || output.is_null() {
            return -1;
        }
        unsafe {
            *canonical = 0;
            *output = -1;
        }
        let path = PathBuf::from(OsString::from_vec(
            unsafe { CStr::from_ptr(path) }.to_bytes().to_vec(),
        ));
        let canonical_path = match path.canonicalize() {
            Ok(path) => path,
            Err(_) => return -1,
        };
        let file = match File::open(&canonical_path) {
            Ok(file) => file,
            Err(_) => return -1,
        };
        let bytes = canonical_path.as_os_str().as_bytes();
        if bytes.len() >= capacity {
            return -1;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), canonical.cast(), bytes.len());
            *canonical.add(bytes.len()) = 0;
            *output = file.into_raw_fd();
        }
        0
    }

    unsafe extern "C" fn open_unterminated_file(
        path: *const c_char,
        canonical: *mut c_char,
        capacity: usize,
        output: *mut i32,
    ) -> i32 {
        if path.is_null() || canonical.is_null() || capacity == 0 || output.is_null() {
            return -1;
        }
        unsafe {
            *output = -1;
        }
        let path = PathBuf::from(OsString::from_vec(
            unsafe { CStr::from_ptr(path) }.to_bytes().to_vec(),
        ));
        let file = match File::open(path) {
            Ok(file) => file,
            Err(_) => return -1,
        };
        // Deliberately violate the guest opener's canonical-path contract so
        // the path ABI's error cleanup is exercised after an owned fd exists.
        unsafe {
            std::ptr::write_bytes(canonical.cast::<u8>(), b'x', capacity);
            *output = file.into_raw_fd();
        }
        0
    }

    unsafe extern "C" fn errno_not_found() -> i32 {
        2
    }

    unsafe extern "C" fn errno_io() -> i32 {
        5
    }

    unsafe fn create_namespace(search: &Path) -> (*mut LinkerRegistry, u64, CString) {
        let registry = darwin_art_linker_registry_create();
        let search = CString::new(search.as_os_str().as_bytes()).unwrap();
        let mut id = 0;
        assert_eq!(
            unsafe {
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    search.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut id,
                )
            },
            0
        );
        (registry, id, search)
    }

    #[test]
    fn exact_native_system_path_reuses_resident_without_guest_file() {
        OPEN_CALLS.store(0, Ordering::Relaxed);
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let namespace = (&*registry)
                .0
                .lock()
                .unwrap()
                .create(NamespaceConfig::default(), None)
                .unwrap();
            let image = Arc::new(NamespaceImage {
                visibility: ImageVisibility::default(),
                soname: "libEGL.so".into(),
                path: PathBuf::from("/system/lib64/libEGL.so"),
                lease: Arc::new(ImageOwner {
                    target_sdk: None,
                    group: None,
                    group_root_flags: None,
                    group_opens: None,
                    group_incoming: None,
                    group_dependencies_complete: false,
                    group_retired: None,
                    _group_edges: None,
                    file_identity: None,
                    value: 1,
                    kind: 4,
                    primary_namespace: namespace.raw(),
                    release: release_noop,
                }),
            });
            (&*registry)
                .0
                .lock()
                .unwrap()
                .publish(namespace, image)
                .unwrap();

            let mut canonical = [b'X' as i8; 64];
            let mut fd = 123;
            let mut resident = std::ptr::null_mut();
            assert_eq!(
                darwin_art_linker_namespace_open_path_resident(
                    registry,
                    namespace.raw(),
                    c"/system/lib64/libEGL.so".as_ptr(),
                    Some(count_missing_open),
                    Some(errno_not_found),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut fd,
                    &mut resident,
                    1,
                ),
                2
            );
            assert_eq!(OPEN_CALLS.load(Ordering::Relaxed), 0);
            assert_eq!(fd, -1);
            assert_eq!(canonical[0], 0);
            assert!(!resident.is_null());
            darwin_art_linker_image_release(resident);

            for (index, alias) in [
                c"/system/lib64/./libEGL.so",
                c"/system/lib64/libEGL.so/",
                c"/system/lib64/../lib64/libEGL.so",
                c"/vendor/lib64/libEGL.so",
            ]
            .into_iter()
            .enumerate()
            {
                resident = std::ptr::null_mut();
                assert_eq!(
                    darwin_art_linker_namespace_open_path_resident(
                        registry,
                        namespace.raw(),
                        alias.as_ptr(),
                        Some(count_missing_open),
                        Some(errno_not_found),
                        canonical.as_mut_ptr(),
                        canonical.len(),
                        &mut fd,
                        &mut resident,
                        1,
                    ),
                    1
                );
                assert_eq!(OPEN_CALLS.load(Ordering::Relaxed), index + 1);
                assert!(resident.is_null());
            }
            darwin_art_linker_registry_destroy(registry);
        }
    }

    #[test]
    fn allowed_path_returns_canonical_guest_path_and_fd_lifetime() {
        let root = test_root("allowed");
        let app = root.join("app");
        fs::create_dir(&app).unwrap();
        let file_path = app.join("liballowed.so");
        fs::write(&file_path, b"still-open").unwrap();

        unsafe {
            let (registry, namespace, _search) = create_namespace(&app);
            let path = CString::new(file_path.as_os_str().as_bytes()).unwrap();
            let mut canonical = [b'X' as i8; 4096];
            let mut fd = 123;
            assert_eq!(
                darwin_art_linker_namespace_open_path(
                    registry,
                    namespace,
                    path.as_ptr(),
                    Some(open_host_file),
                    Some(errno_not_found),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut fd,
                ),
                0
            );
            assert_eq!(
                CStr::from_ptr(canonical.as_ptr()).to_bytes(),
                file_path.as_os_str().as_bytes()
            );
            let mut opened = File::from_raw_fd(fd);
            fs::remove_file(&file_path).unwrap();
            darwin_art_linker_registry_destroy(registry);
            let mut contents = String::new();
            opened.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, "still-open");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canonical_symlink_escape_is_denied_and_clears_outputs() {
        let root = test_root("escape");
        let app = root.join("app");
        let private = root.join("private");
        fs::create_dir(&app).unwrap();
        fs::create_dir(&private).unwrap();
        let target = private.join("secret.so");
        fs::write(&target, b"private").unwrap();
        let link = app.join("libescape.so");
        std::os::unix::fs::symlink("../private/secret.so", &link).unwrap();

        unsafe {
            let (registry, namespace, _search) = create_namespace(&app);
            let path = CString::new(link.as_os_str().as_bytes()).unwrap();
            let mut canonical = [b'X' as i8; 4096];
            let mut fd = 123;
            assert_eq!(
                darwin_art_linker_namespace_open_path(
                    registry,
                    namespace,
                    path.as_ptr(),
                    Some(open_host_file),
                    Some(errno_not_found),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut fd,
                ),
                -1
            );
            assert_eq!(fd, -1);
            assert_eq!(canonical[0], 0);
            darwin_art_linker_registry_destroy(registry);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_and_opener_errors_clear_outputs() {
        let root = test_root("errors");
        let app = root.join("app");
        fs::create_dir(&app).unwrap();
        let file_path = app.join("libbroken.so");
        fs::write(&file_path, b"broken-contract").unwrap();

        unsafe {
            let (registry, namespace, _search) = create_namespace(&app);
            let path = CString::new(file_path.as_os_str().as_bytes()).unwrap();
            let mut canonical = [b'X' as i8; 32];
            let mut fd = 123;
            assert_eq!(
                darwin_art_linker_namespace_open_path(
                    registry,
                    namespace,
                    path.as_ptr(),
                    Some(open_unterminated_file),
                    Some(errno_io),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut fd,
                ),
                -3
            );
            assert_eq!(fd, -1);
            assert_eq!(canonical[0], 0);

            canonical.fill(b'Y' as i8);
            fd = 456;
            assert_eq!(
                darwin_art_linker_namespace_open_path(
                    registry,
                    namespace,
                    std::ptr::null(),
                    Some(open_host_file),
                    Some(errno_not_found),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut fd,
                ),
                -1
            );
            assert_eq!(fd, -1);
            assert_eq!(canonical[0], 0);
            darwin_art_linker_registry_destroy(registry);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
