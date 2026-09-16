//! Private loader admission boundary. A successful call transfers an owned
//! host fd together with its guest path; no pathname reopen is required.
use super::*;

/// # Safety
/// Path is readable/NUL-terminated. Output buffers are writable for their
/// supplied capacities and do not overlap the input or each other. This is a
/// host-only ABI; the returned fd must not enter the guest virtual-fd table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_fs_open_native_image(
    path: *const c_char,
    canonical: *mut c_char,
    capacity: usize,
    out_fd: *mut c_int,
) -> c_int {
    unsafe { open_native_node(path, canonical, capacity, out_fd, false) }
}

/// # Safety
/// Same output ownership as open_native_image. Relative directory paths use
/// guest cwd; this API never consults host cwd.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_fs_open_native_directory(
    path: *const c_char,
    canonical: *mut c_char,
    capacity: usize,
    out_fd: *mut c_int,
) -> c_int {
    unsafe { open_native_node(path, canonical, capacity, out_fd, true) }
}

unsafe fn open_native_node(
    path: *const c_char,
    canonical: *mut c_char,
    capacity: usize,
    out_fd: *mut c_int,
    directory: bool,
) -> c_int {
    if out_fd.is_null() {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    }
    unsafe {
        *out_fd = -1;
    }
    if canonical.is_null() || capacity == 0 || path.is_null() {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    }
    unsafe {
        *canonical = 0;
    }
    let path = unsafe { CStr::from_ptr(path) }.to_bytes();
    if !directory && !path.starts_with(b"/") {
        Facade::set_android_errno(ANDROID_EINVAL);
        return -1;
    }
    with_active(-1, |facade| {
        if path.is_empty() {
            return facade.fail(ANDROID_ENOENT);
        }
        let Some(root) = &facade.namespace.guest_root else {
            return facade.fail(ANDROID_EOPNOTSUPP);
        };
        let absolute = if path.starts_with(b"/") {
            path.to_vec()
        } else {
            let Ok(cwd) = facade.namespace.cwd.snapshot() else {
                return facade.fail_capability();
            };
            let mut absolute = cwd.clone();
            if !absolute.ends_with(b"/") {
                absolute.push(b'/');
            }
            absolute.extend(path);
            absolute
        };
        let opened = match root.open(&absolute) {
            Ok(opened) => opened,
            Err(error) => return facade.fail_io(&error),
        };
        if directory && !opened.node.metadata().is_dir() {
            return facade.fail(ANDROID_ENOTDIR);
        }
        if !directory && !opened.node.metadata().is_file() {
            return facade.fail(ANDROID_EISDIR);
        }
        if opened.canonical_path.len() >= capacity {
            return facade.fail(ANDROID_ERANGE);
        }
        // All fallible operations precede publication. On failure the opened
        // file drops locally; on success the loader owns it independently of
        // the process facade lifetime. Namespace permission checking remains
        // the loader's responsibility using this same path/fd pair.
        unsafe {
            ptr::copy_nonoverlapping(
                opened.canonical_path.as_ptr(),
                canonical.cast(),
                opened.canonical_path.len(),
            );
            canonical.add(opened.canonical_path.len()).write(0);
            *out_fd = opened.node.into_file().into_raw_fd();
        }
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn image_fd_and_canonical_path_are_one_admission() {
        let root = std::env::temp_dir().join(format!(
            "darwin-image-admit-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("image"), b"original").unwrap();
        symlink("/image", root.join("link")).unwrap();
        let facade = Arc::new(Facade::new(File::open(&root).unwrap(), b"/", b"/").unwrap());
        let active = facade.activate();
        let mut output = [0i8; 4096];
        let mut fd = 123;
        assert_eq!(
            unsafe {
                darwin_art_fs_open_native_image(c"/link".as_ptr(), output.as_mut_ptr(), 2, &mut fd)
            },
            -1
        );
        assert_eq!(fd, -1);
        assert_eq!(output[0], 0);
        assert_eq!(
            unsafe {
                darwin_art_fs_open_native_image(
                    c"/".as_ptr(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut fd,
                )
            },
            -1
        );
        assert_eq!(fd, -1);
        assert_eq!(
            unsafe {
                darwin_art_fs_open_native_image(
                    c"/link".as_ptr(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut fd,
                )
            },
            0
        );
        assert_eq!(
            unsafe { CStr::from_ptr(output.as_ptr()) }.to_bytes(),
            b"/image"
        );
        drop(active);
        drop(facade);
        fs::rename(root.join("image"), root.join("old")).unwrap();
        fs::write(root.join("image"), b"replacement").unwrap();
        let mut file = unsafe { File::from_raw_fd(fd) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original");
        drop(file);
        fs::remove_dir_all(root).unwrap();
    }
}
