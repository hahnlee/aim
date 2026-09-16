//! Canonical guest path queries, separate from descriptor and mutation policy.
use super::*;

impl Facade {
    pub(super) unsafe fn realpath(&self, path: &[u8], resolved: *mut c_char) -> *mut c_char {
        if resolved.is_null() {
            self.fail(ANDROID_EOPNOTSUPP); // Allocating form needs guest allocator.
            return ptr::null_mut();
        }
        if path.is_empty() {
            self.fail(ANDROID_ENOENT);
            return ptr::null_mut();
        }
        let canonical = if path == b"/proc/self" {
            // Existing process-owned procfs identity.
            format!("/proc/{}", std::process::id()).into_bytes()
        } else if let Some(root) = &self.namespace.guest_root {
            let absolute = if path.starts_with(b"/") {
                path.to_vec()
            } else {
                let Ok(cwd) = self.namespace.cwd.snapshot() else {
                    self.fail_capability();
                    return ptr::null_mut();
                };
                let mut absolute = cwd.clone();
                if !absolute.ends_with(b"/") {
                    absolute.push(b'/');
                }
                absolute.extend(path);
                absolute
            };
            // Do not lexically collapse '..' before resolving symlinks.
            match root.open(&absolute) {
                Ok(opened) => opened.canonical_path,
                Err(error) => {
                    self.fail_io(&error);
                    return ptr::null_mut();
                }
            }
        } else {
            // Legacy single-submount installs do not authorize a whole guest
            // root. Preserve their strict no-link capability rather than
            // interpreting absolute targets relative to the wrong mount.
            let resolution = match self.resolve(path) {
                Ok(value) => value,
                Err(error) => {
                    self.fail(error);
                    return ptr::null_mut();
                }
            };
            if let Err(error) = self.namespace.broker.open(&resolution.relative_path) {
                self.fail_broker(&error);
                return ptr::null_mut();
            }
            resolution.normalized_path
        };
        // Both resolvers bound their output to Android PATH_MAX minus NUL.
        unsafe {
            ptr::copy_nonoverlapping(canonical.as_ptr(), resolved.cast::<u8>(), canonical.len());
            resolved.add(canonical.len()).write(0);
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn realpath_resolves_links_before_parent_components() {
        let root = std::env::temp_dir().join(format!(
            "darwin-realpath-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/target"), b"data").unwrap();
        symlink("/a/b", root.join("link")).unwrap();
        let facade = Facade::new(File::open(&root).unwrap(), b"/", b"/").unwrap();
        let mut out = [0i8; 4096];
        assert_eq!(
            unsafe { facade.realpath(b"link/../target", out.as_mut_ptr()) },
            out.as_mut_ptr()
        );
        assert_eq!(
            unsafe { CStr::from_ptr(out.as_ptr()) }.to_bytes(),
            b"/a/target"
        );
        assert!(unsafe { facade.realpath(b"", out.as_mut_ptr()) }.is_null());
        fs::remove_dir_all(root).unwrap();
    }
}
