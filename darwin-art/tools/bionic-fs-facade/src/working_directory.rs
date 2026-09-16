//! Guest process working-directory policy. Never changes macOS process cwd.
//! Updates are serialized and published only after directory authorization.
//! The authorized directory FD lives with cwd; pathname snapshots are not an
//! authority for metadata. GuestRoot cwd names are derived from the live FD.
use super::{ANDROID_ENOENT, ANDROID_ENOTDIR, ANDROID_EOPNOTSUPP, ANDROID_ERANGE, Facade};
use darwin_art_fs_broker::guest_path::{GuestRoot, MountOrigin};
use std::ffi::{c_char, c_int};
use std::fs::{File, Metadata};
use std::ptr;
use std::sync::{Arc, Mutex};

#[cfg(test)]
#[path = "cwd_snapshot_tests.rs"]
mod cwd_snapshot_tests;

pub(super) struct WorkingDirectory {
    state: Mutex<CwdState>,
    root: Option<Arc<GuestRoot>>,
}

struct CwdState {
    origin: Option<MountOrigin>,
    path: Vec<u8>,
    directory: File,
}

/// One coherent observation for calls needing both mount selection and I/O.
/// The path is for namespace policy; relative I/O must use `directory`.
pub(super) struct CwdSnapshot {
    pub(super) path: Vec<u8>,
    pub(super) directory: File,
    pub(super) origin: Option<MountOrigin>,
}

impl WorkingDirectory {
    pub(super) fn new(
        path: Vec<u8>,
        directory: File,
        root: Option<Arc<GuestRoot>>,
        origin: Option<MountOrigin>,
    ) -> Self {
        Self {
            state: Mutex::new(CwdState {
                path,
                directory,
                origin,
            }),
            root,
        }
    }

    // Other subsystems may observe a snapshot, but cannot mutate cwd state.
    pub(super) fn snapshot(&self) -> Result<Vec<u8>, ()> {
        let state = self.state.lock().map_err(|_| ())?;
        match &self.root {
            Some(root) => root.directory_path(&state.directory).map_err(|_| ()),
            None => Ok(state.path.clone()),
        }
    }

    pub(super) fn directory(&self) -> std::io::Result<File> {
        self.lease().map(|(file, _)| file)
    }

    pub(super) fn named_lease(&self) -> std::io::Result<CwdSnapshot> {
        let state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("cwd state poisoned"))?;
        let path = match &self.root {
            Some(root) => root.directory_path(&state.directory)?,
            None => state.path.clone(),
        };
        Ok(CwdSnapshot {
            path,
            directory: state.directory.try_clone()?,
            origin: state.origin.clone(),
        })
    }

    /// Capture the descriptor and its resolver identity under the same lock.
    pub(super) fn lease(&self) -> std::io::Result<(File, Option<MountOrigin>)> {
        let state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("cwd state poisoned"))?;
        Ok((state.directory.try_clone()?, state.origin.clone()))
    }

    pub(super) fn metadata(&self) -> std::io::Result<Metadata> {
        self.state
            .lock()
            .map_err(|_| std::io::Error::other("cwd state poisoned"))?
            .directory
            .metadata()
    }
}

impl Facade {
    pub(super) fn fchdir(&self, fd: c_int) -> c_int {
        let (directory, origin) = {
            let table = match self.descriptors.lock() {
                Ok(table) => table,
                Err(_) => return self.fail_capability(),
            };
            match table.entries.get(&fd) {
                Some(super::Descriptor::File(file) | super::Descriptor::PrivateFile(file)) => {
                    match file.try_clone() {
                        Ok(file) => (file, table.origin(fd).cloned()),
                        Err(error) => return self.fail_io(&error),
                    }
                }
                Some(_) => return self.fail(ANDROID_ENOTDIR),
                None => return self.fail(super::ANDROID_EBADF),
            }
        };
        let Some(root) = &self.namespace.guest_root else {
            return self.fail(ANDROID_EOPNOTSUPP);
        };
        let path = match root.directory_path(&directory) {
            Ok(path) => path,
            Err(error) => return self.fail_io(&error),
        };
        let mut state = match self.namespace.cwd.state.lock() {
            Ok(state) => state,
            Err(_) => return self.fail_capability(),
        };
        *state = CwdState {
            path,
            directory,
            origin,
        };
        0
    }

    pub(super) fn chdir(&self, path: &[u8]) -> c_int {
        // chdir is process-global in Bionic. Holding this lock through the
        // authorization walk gives concurrent calls a total update order.
        let mut cwd = match self.namespace.cwd.state.lock() {
            Ok(cwd) => cwd,
            Err(_) => return self.fail_capability(),
        };
        if path.is_empty() {
            return self.fail(ANDROID_ENOENT);
        }
        if let Some(root) = &self.namespace.guest_root {
            let resolved = match &cwd.origin {
                Some(origin) => root
                    .open_relative_with_origin(&cwd.directory, origin, path, false)
                    .map(|(file, origin)| (file, Some(origin))),
                None => root
                    .open_relative(&cwd.directory, path, false)
                    .map(|file| (file, None)),
            };
            let (opened, origin) = match resolved {
                Ok(opened) => opened,
                Err(error) => return self.fail_io(&error),
            };
            if !opened.metadata().is_ok_and(|metadata| metadata.is_dir()) {
                return self.fail(ANDROID_ENOTDIR);
            }
            let path = match root.directory_path(&opened) {
                Ok(path) => path,
                Err(error) => return self.fail_io(&error),
            };
            *cwd = CwdState {
                origin,
                path,
                directory: opened,
            };
            return 0;
        }
        let resolution = match self.resolve_from(&cwd.path, path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        let opened = match self.namespace.broker.open(&resolution.relative_path) {
            Ok(opened) => opened,
            Err(error) => return self.fail_broker(&error),
        };
        if !opened.metadata().is_dir() {
            return self.fail(ANDROID_ENOTDIR);
        }
        *cwd = CwdState {
            origin: None,
            path: resolution.normalized_path,
            directory: opened.into_file(),
        };
        0
    }

    pub(super) unsafe fn getcwd(&self, buffer: *mut c_char, size: usize) -> *mut c_char {
        if buffer.is_null() {
            // Bionic's allocation extension needs the coherent Bionic allocator,
            // which this isolated facade deliberately does not own.
            self.fail(ANDROID_EOPNOTSUPP);
            return ptr::null_mut();
        }
        let state = match self.namespace.cwd.state.lock() {
            Ok(cwd) => cwd,
            Err(_) => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        let path = match &self.namespace.guest_root {
            Some(root) => match root.directory_path(&state.directory) {
                Ok(path) => path,
                Err(error) => {
                    self.fail_io(&error);
                    return ptr::null_mut();
                }
            },
            None => state.path.clone(),
        };
        let cwd = &path;
        let required = match cwd.len().checked_add(1) {
            Some(required) => required,
            None => {
                self.fail_capability();
                return ptr::null_mut();
            }
        };
        if size < required {
            self.fail(ANDROID_ERANGE);
            return ptr::null_mut();
        }
        // SAFETY: the ABI requires buffer to be writable for size bytes; the
        // checked required length is at most size and includes the trailing NUL.
        unsafe {
            ptr::copy_nonoverlapping(cwd.as_ptr(), buffer.cast::<u8>(), cwd.len());
            buffer.add(cwd.len()).write(0);
        }
        buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::fs::{self, File};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::symlink;

    fn cwd(facade: &Facade) -> Vec<u8> {
        let mut out = [0i8; 4096];
        assert_eq!(
            unsafe { facade.getcwd(out.as_mut_ptr(), out.len()) },
            out.as_mut_ptr()
        );
        unsafe { CStr::from_ptr(out.as_ptr()) }.to_bytes().to_vec()
    }

    #[test]
    fn chdir_resolves_guest_links_and_preserves_state_on_failure() {
        let path = std::env::temp_dir().join(format!(
            "darwin-cwd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(path.join("a/b")).unwrap();
        fs::write(path.join("file"), b"data").unwrap();
        symlink("/a/b", path.join("link")).unwrap();
        symlink("/missing-host-target", path.join("outside")).unwrap();
        let host_cwd = std::env::current_dir().unwrap();
        let facade = Facade::new(File::open(&path).unwrap(), b"/", b"/").unwrap();
        assert_eq!(facade.chdir(b"/link/.."), 0);
        assert_eq!(cwd(&facade), b"/a");
        assert_eq!(facade.chdir(b"b"), 0);
        assert_eq!(cwd(&facade), b"/a/b");
        for rejected in [b"".as_slice(), b"/file", b"/outside"] {
            assert_eq!(facade.chdir(rejected), -1);
            assert_eq!(cwd(&facade), b"/a/b");
        }
        let original_inode = fs::metadata(path.join("a/b")).unwrap().ino();
        let directory_fd = facade.open(b"/a/b", 0);
        assert!(directory_fd >= 0);
        fs::write(path.join("a/b/entry"), b"original").unwrap();
        fs::rename(path.join("a/b"), path.join("moved")).unwrap();
        fs::create_dir(path.join("a/b")).unwrap();
        fs::write(path.join("a/b/entry"), b"wrong").unwrap();
        assert_ne!(
            fs::metadata(path.join("a/b")).unwrap().ino(),
            original_inode
        );
        let mut status = std::mem::MaybeUninit::<crate::AndroidStat>::uninit();
        assert_eq!(
            unsafe { facade.fstatat(-100, b"", status.as_mut_ptr(), 0x1000) },
            0
        );
        assert_eq!(unsafe { status.assume_init() }.st_ino, original_inode);
        for fd in [-100, directory_fd] {
            assert_eq!(
                unsafe { facade.fstatat(fd, b"entry", status.as_mut_ptr(), 0) },
                0
            );
            assert_eq!(unsafe { status.assume_init() }.st_size, 8);
            let opened = facade.openat_with_mode(fd, b"entry", 0, 0);
            assert!(opened >= 0);
            let mut bytes = [0u8; 8];
            assert_eq!(
                unsafe { facade.read(opened, bytes.as_mut_ptr().cast(), bytes.len()) },
                8
            );
            assert_eq!(&bytes, b"original");
            assert_eq!(facade.close(opened), 0);
        }
        assert_eq!(facade.close(directory_fd), 0);
        assert_eq!(cwd(&facade), b"/moved");
        let target_fd = facade.open(b"/a", 0);
        assert!(target_fd >= 0);
        assert_eq!(facade.fchdir(target_fd), 0);
        assert_eq!(facade.close(target_fd), 0);
        assert_eq!(cwd(&facade), b"/a");
        assert_eq!(facade.chdir(b"b"), 0);
        assert_eq!(cwd(&facade), b"/a/b");
        assert_eq!(std::env::current_dir().unwrap(), host_cwd);
        fs::remove_dir_all(path).unwrap();
    }
}
