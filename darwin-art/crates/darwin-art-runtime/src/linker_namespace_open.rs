//! Private composition of namespace search and the filesystem's fd/path ABI.
use super::*;
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::unix::ffi::OsStrExt;

type OpenImage = unsafe extern "C" fn(*const c_char, *mut c_char, usize, *mut i32) -> i32;
type AndroidErrno = unsafe extern "C" fn() -> i32;

// The returned path and fd describe one filesystem admission. Callbacks run
// outside the namespace mutex, including RUNPATH directory resolution.
pub(super) unsafe fn open_guest_node(
    candidate: &std::path::Path,
    opener: OpenImage,
    errno: AndroidErrno,
) -> io::Result<(PathBuf, File)> {
    let name = CString::new(candidate.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let mut path = [0i8; 4096];
    let mut fd = -1;
    let status = unsafe { opener(name.as_ptr(), path.as_mut_ptr(), path.len(), &mut fd) };
    let file = (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) });
    if status != 0 {
        let code = unsafe { errno() };
        return Err(if code == 2 || code == 20 {
            io::Error::from(io::ErrorKind::NotFound)
        } else {
            io::Error::other(format!("guest file open failed: Android errno {code}"))
        });
    }
    let file = file.ok_or_else(|| io::Error::other("successful opener returned no fd"))?;
    let end = path
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| io::Error::other("unterminated canonical path"))?;
    let bytes = path[..end].iter().map(|b| *b as u8).collect();
    Ok((PathBuf::from(std::ffi::OsString::from_vec(bytes)), file))
}

/// # Safety
/// Registry is live through the call, callbacks satisfy guest-image ABI, and
/// output buffers are writable/nonoverlapping. Success transfers a host fd.
/// Registry destruction must wait for this operation, including callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_namespace_open(
    registry: *mut LinkerRegistry,
    id: u64,
    soname: *const c_char,
    runpath: *const c_char,
    source_image: *const c_char,
    directory_opener: Option<OpenImage>,
    opener: Option<OpenImage>,
    errno: Option<AndroidErrno>,
    canonical: *mut c_char,
    capacity: usize,
    out_namespace: *mut u64,
    out_fd: *mut i32,
) -> i32 {
    if registry.is_null()
        || out_namespace.is_null()
        || out_fd.is_null()
        || canonical.is_null()
        || capacity == 0
    {
        return -1;
    }
    unsafe {
        *out_namespace = 0;
        *out_fd = -1;
        *canonical = 0;
    }
    let (Some(opener), Some(errno), Some(soname)) = (opener, errno, unsafe { text(soname) }) else {
        return -1;
    };
    let directories = if runpath.is_null() {
        Vec::new()
    } else {
        let Some(directory_opener) = directory_opener else {
            return -1;
        };
        if source_image.is_null() {
            return -1;
        }
        let source = PathBuf::from(std::ffi::OsString::from_vec(
            unsafe { CStr::from_ptr(source_image) }.to_bytes().to_vec(),
        ));
        match crate::linker_namespace::resolve_runpath_directories(
            unsafe { CStr::from_ptr(runpath) }.to_bytes(),
            &source,
            |candidate| unsafe { open_guest_node(candidate, directory_opener, errno) }.ok(),
        ) {
            Ok(directories) => directories,
            Err(_) => return -1,
        }
    };
    let plan = {
        let Ok(state) = (unsafe { &*registry }).0.lock() else {
            return -2;
        };
        match state.search_plan(NamespaceId::from_raw(id), &soname, &directories) {
            Ok(plan) => plan,
            Err(_) => return -1,
        }
    };
    let result = plan.open(|candidate| unsafe { open_guest_node(candidate, opener, errno) });
    match result {
        Ok(Some(opened)) => {
            let path = opened.canonical_path.as_os_str().as_bytes();
            if path.len() >= capacity {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(path.as_ptr(), canonical.cast(), path.len());
                canonical.add(path.len()).write(0);
                *out_namespace = opened.namespace.raw();
                *out_fd = opened.file.into_raw_fd();
            }
            0
        }
        Ok(None) => 1,
        Err(_) => -3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::symlink;
    unsafe extern "C" {
        fn darwin_art_fs_open_native_image(
            path: *const c_char,
            canonical: *mut c_char,
            capacity: usize,
            fd: *mut i32,
        ) -> i32;
        fn darwin_art_bionic_errno_load() -> i32;
        fn darwin_art_fs_open_native_directory(
            path: *const c_char,
            canonical: *mut c_char,
            capacity: usize,
            fd: *mut i32,
        ) -> i32;
    }
    #[test]
    fn actual_filesystem_and_namespace_admission_compose() {
        let path = std::env::temp_dir().join(format!(
            "darwin-ns-compose-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(path.join("system")).unwrap();
        std::fs::create_dir_all(path.join("app")).unwrap();
        std::fs::create_dir_all(path.join("private")).unwrap();
        std::fs::write(path.join("system/liba.so"), b"public-image").unwrap();
        std::fs::write(path.join("system/libb.so"), b"descendant-image").unwrap();
        std::fs::write(path.join("private/liba.so"), b"private-image").unwrap();
        symlink("/private/liba.so", path.join("app/liba.so")).unwrap();
        let facade = Arc::new(
            bionic_fs_facade::Facade::new(File::open(&path).unwrap(), b"/", b"/").unwrap(),
        );
        let active = facade.activate();
        unsafe {
            let registry = darwin_art_linker_registry_create();
            let mut app = 0;
            let mut system = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    c"/app".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut app
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    c"/system".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut system
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_namespace_link(registry, app, system, c"liba.so".as_ptr()),
                0
            );
            let mut canonical = [0i8; 4096];
            let mut owner = 0;
            let mut fd = -1;
            assert_eq!(
                darwin_art_linker_namespace_open(
                    registry,
                    app,
                    c"liba.so".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    None,
                    Some(darwin_art_fs_open_native_image),
                    Some(darwin_art_bionic_errno_load),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut owner,
                    &mut fd
                ),
                0
            );
            assert_eq!(owner, system);
            assert_eq!(
                CStr::from_ptr(canonical.as_ptr()).to_bytes(),
                b"/system/liba.so"
            );
            drop(File::from_raw_fd(fd));
            // RUNPATH exists, but does not grant access to a private directory.
            assert_eq!(
                darwin_art_linker_namespace_open(
                    registry,
                    app,
                    c"liba.so".as_ptr(),
                    c"$ORIGIN/../private".as_ptr(),
                    c"/app/root.so".as_ptr(),
                    Some(darwin_art_fs_open_native_directory),
                    Some(darwin_art_fs_open_native_image),
                    Some(darwin_art_bionic_errno_load),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut owner,
                    &mut fd,
                ),
                0
            );
            assert_eq!(owner, system);
            assert_eq!(
                CStr::from_ptr(canonical.as_ptr()).to_bytes(),
                b"/system/liba.so"
            );
            // Keep this fd for the existing post-destruction lifetime check.
            let public_fd = fd;
            let mut permitted = 0;
            assert_eq!(
                darwin_art_linker_namespace_create(
                    registry,
                    1,
                    std::ptr::null(),
                    std::ptr::null(),
                    c"/private".as_ptr(),
                    0,
                    &mut permitted,
                ),
                0
            );
            assert_eq!(
                darwin_art_linker_namespace_open(
                    registry,
                    permitted,
                    c"liba.so".as_ptr(),
                    c"$ORIGIN/../private".as_ptr(),
                    c"/app/root.so".as_ptr(),
                    Some(darwin_art_fs_open_native_directory),
                    Some(darwin_art_fs_open_native_image),
                    Some(darwin_art_bionic_errno_load),
                    canonical.as_mut_ptr(),
                    canonical.len(),
                    &mut owner,
                    &mut fd,
                ),
                0
            );
            assert_eq!(owner, permitted);
            assert_eq!(
                CStr::from_ptr(canonical.as_ptr()).to_bytes(),
                b"/private/liba.so"
            );
            let mut private = File::from_raw_fd(fd);
            let mut private_bytes = String::new();
            private.read_to_string(&mut private_bytes).unwrap();
            assert_eq!(private_bytes, "private-image");
            drop(private);
            // libb is not exported to app. A dependency of system-owned liba
            // must search from system, rather than reusing the root namespace.
            use super::super::discovery::*;
            let context = darwin_art_linker_discovery_create(
                registry,
                app,
                c"/app/root.so".as_ptr(),
                Some(darwin_art_fs_open_native_directory),
                Some(darwin_art_fs_open_native_image),
                Some(darwin_art_bionic_errno_load),
            );
            assert!(!context.is_null());
            let mut child = 0;
            let child_fd = darwin_art_linker_discovery_open(
                context.cast(),
                1,
                c"root.so".as_ptr(),
                c"liba.so".as_ptr(),
                std::ptr::null(),
                &mut child,
            );
            assert!(child_fd >= 0);
            assert_eq!(child, 2);
            drop(File::from_raw_fd(child_fd));
            let mut grandchild = 0;
            let descendant_fd = darwin_art_linker_discovery_open(
                context.cast(),
                child,
                c"liba.so".as_ptr(),
                c"libb.so".as_ptr(),
                std::ptr::null(),
                &mut grandchild,
            );
            assert!(descendant_fd >= 0);
            assert_eq!(grandchild, 3);
            assert_eq!(
                darwin_art_linker_discovery_open(
                    context.cast(),
                    999,
                    c"invalid".as_ptr(),
                    c"libb.so".as_ptr(),
                    std::ptr::null(),
                    &mut grandchild,
                ),
                -1
            );
            assert_eq!(grandchild, 0);
            darwin_art_linker_discovery_destroy(context);
            let mut descendant = File::from_raw_fd(descendant_fd);
            let mut descendant_bytes = String::new();
            descendant.read_to_string(&mut descendant_bytes).unwrap();
            assert_eq!(descendant_bytes, "descendant-image");
            drop(descendant);
            darwin_art_linker_registry_destroy(registry);
            drop(active);
            drop(facade);
            let mut file = File::from_raw_fd(public_fd);
            std::fs::remove_dir_all(&path).unwrap();
            let mut bytes = String::new();
            file.read_to_string(&mut bytes).unwrap();
            assert_eq!(bytes, "public-image");
        }
    }
}
