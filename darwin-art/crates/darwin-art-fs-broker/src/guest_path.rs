//! Guest-root path resolution. Absolute symlinks restart at the guest root,
//! never the host root. Mount selection remains the caller's responsibility.
//! The returned path describes this walk, not a pathname to reopen: consume
//! the returned descriptor so a later rename cannot substitute another image.

use super::*;
use std::collections::VecDeque;
mod directory_name;
mod origin;
#[cfg(test)]
mod origin_tests;
mod relative;
pub use origin::MountOrigin;

unsafe extern "C" {
    fn readlinkat(fd: c_int, name: *const c_char, out: *mut u8, size: usize) -> isize;
}

pub struct GuestRoot {
    namespace: std::sync::Arc<()>,
    root: ReadOnlyBroker,
    mounts: std::collections::BTreeMap<Vec<u8>, ReadOnlyBroker>,
}
pub struct ResolvedGuestFile {
    origin: MountOrigin,
    pub canonical_path: Vec<u8>,
    pub node: OpenedNode,
}

impl ResolvedGuestFile {
    pub fn origin(&self) -> &MountOrigin {
        &self.origin
    }
}

fn io_error(error: BrokerError) -> io::Error {
    match error {
        BrokerError::Io { source, .. } => source,
        other => io::Error::other(other),
    }
}

impl GuestRoot {
    pub fn from_directory(root: File) -> Result<Self, BrokerError> {
        ReadOnlyBroker::from_directory(root).map(|root| Self {
            namespace: std::sync::Arc::new(()),
            root,
            mounts: Default::default(),
        })
    }

    /// Configure before publishing this owner. Mount prefixes must be canonical
    /// guest paths; mounted roots are owned descriptors, never host strings.
    pub fn mount_directory(&mut self, prefix: &[u8], root: File) -> io::Result<()> {
        if prefix == b"/"
            || prefix.len() >= 4096
            || !prefix.starts_with(b"/")
            || prefix.contains(&0)
            || prefix[1..]
                .split(|b| *b == b'/')
                .any(|p| p.is_empty() || p == b"." || p == b"..")
            || self.mounts.contains_key(prefix)
        {
            return Err(io::Error::from_raw_os_error(22));
        }
        self.mounts.insert(
            prefix.to_vec(),
            ReadOnlyBroker::from_directory(root).map_err(io_error)?,
        );
        Ok(())
    }

    pub fn open(&self, path: &[u8]) -> io::Result<ResolvedGuestFile> {
        self.walk(path, false)
    }

    /// Open the final node itself for metadata. Intermediate symlinks retain
    /// guest-root semantics; a trailing slash still requires a directory.
    pub fn open_no_follow(&self, path: &[u8]) -> io::Result<ResolvedGuestFile> {
        self.walk(path, true)
    }

    fn walk(&self, path: &[u8], no_follow: bool) -> io::Result<ResolvedGuestFile> {
        if !path.starts_with(b"/") || path.contains(&0) {
            return Err(io::Error::from_raw_os_error(22));
        }
        if path.len() >= 4096 {
            return Err(io::Error::from_raw_os_error(63)); // Darwin ENAMETOOLONG
        }
        let mut pending: VecDeque<Vec<u8>> = path
            .split(|b| *b == b'/')
            .filter(|p| !p.is_empty())
            .map(Vec::from)
            .collect();
        let mut directories = vec![self.root.root.try_clone()?];
        let mut origins = vec![MountOrigin {
            namespace: self.namespace.clone(),
            mount: None,
        }];
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut links = 0;
        let mut require_directory = path.ends_with(b"/");
        while let Some(component) = pending.pop_front() {
            if component == b"." {
                continue;
            }
            if component == b".." {
                if directories.len() > 1 {
                    directories.pop();
                    origins.pop();
                    names.pop();
                }
                continue;
            }
            let directory = directories.last().expect("root remains live");
            let mut mount_path = canonical(&names);
            if mount_path.len() > 1 {
                mount_path.push(b'/');
            }
            mount_path.extend(&component);
            if let Some(mount) = self.mounts.get(&mount_path) {
                let origin = MountOrigin {
                    namespace: self.namespace.clone(),
                    mount: Some(mount_path),
                };
                let file = open_component(
                    &mount.root,
                    b".",
                    O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                    None,
                    "open guest mount directory",
                )
                .map_err(io_error)?;
                names.push(component);
                if pending.is_empty() {
                    return Ok(ResolvedGuestFile {
                        origin,
                        canonical_path: canonical(&names),
                        node: finish_open(file).map_err(io_error)?,
                    });
                }
                directories.push(file);
                origins.push(origin);
                continue;
            }
            let name = CString::new(component.clone()).expect("NUL rejected");
            if no_follow && pending.is_empty() && !require_directory {
                // Darwin O_SYMLINK opens the link itself; no target is ever
                // interpreted by host openat. Metadata comes from that lease,
                // avoiding a second lookup racing rename/replacement.
                const O_SYMLINK: c_int = 0x0020_0000;
                let file = open_component(
                    directory,
                    &component,
                    O_RDONLY | O_NONBLOCK | O_CLOEXEC | O_SYMLINK,
                    None,
                    "open guest metadata node",
                )
                .map_err(io_error)?;
                let metadata = file.metadata()?;
                names.push(component);
                return Ok(ResolvedGuestFile {
                    origin: origins.last().expect("root remains live").clone(),
                    canonical_path: canonical(&names),
                    node: OpenedNode { file, metadata },
                });
            }
            let mut target = [0u8; 4096];
            // SAFETY: live directory fd, terminated name and writable buffer.
            let count = unsafe {
                readlinkat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    target.as_mut_ptr(),
                    target.len(),
                )
            };
            if count >= 0 {
                links += 1;
                if links > 40 {
                    return Err(io::Error::from_raw_os_error(62));
                }
                if count as usize == target.len() {
                    return Err(io::Error::from_raw_os_error(63));
                }
                let target = &target[..count as usize];
                if target.starts_with(b"/") {
                    directories.truncate(1);
                    origins.truncate(1);
                    names.clear();
                }
                if pending.is_empty() && target.ends_with(b"/") {
                    require_directory = true;
                }
                for part in target.split(|b| *b == b'/').filter(|p| !p.is_empty()).rev() {
                    pending.push_front(part.to_vec());
                }
                let length = names
                    .iter()
                    .chain(pending.iter())
                    .map(|p| p.len() + 1)
                    .sum::<usize>();
                if length >= 4096 {
                    return Err(io::Error::from_raw_os_error(63));
                }
                continue;
            }
            let error = io::Error::last_os_error();
            // EINVAL means the node is not a symlink. Any other error belongs
            // to this lookup, not an invitation to try a host-global path.
            if error.raw_os_error() != Some(22) {
                return Err(error);
            }
            let mut flags = O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC;
            if !pending.is_empty() || require_directory {
                flags |= O_DIRECTORY;
            }
            let file = open_component(directory, &component, flags, None, "open guest component")
                .map_err(io_error)?;
            names.push(component);
            if pending.is_empty() {
                return Ok(ResolvedGuestFile {
                    origin: origins.last().expect("root remains live").clone(),
                    canonical_path: canonical(&names),
                    node: finish_open(file).map_err(io_error)?,
                });
            }
            directories.push(file);
            origins.push(origins.last().expect("root remains live").clone());
        }
        // An open must own a new directory offset, not dup the authority's
        // open-file description (successive readdir calls would share EOF).
        let file = open_component(
            directories.last().expect("root remains live"),
            b".",
            O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
            None,
            "open resolved guest directory",
        )
        .map_err(io_error)?;
        Ok(ResolvedGuestFile {
            origin: origins.last().expect("root remains live").clone(),
            canonical_path: canonical(&names),
            node: finish_open(file).map_err(io_error)?,
        })
    }
}

fn canonical(names: &[Vec<u8>]) -> Vec<u8> {
    let mut result = vec![b'/'];
    for (index, name) in names.iter().enumerate() {
        if index != 0 {
            result.push(b'/');
        }
        result.extend(name);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    struct Root(std::path::PathBuf);
    impl Root {
        fn new() -> Self {
            let id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "darwin-guest-path-{}-{id}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn resolver(&self) -> GuestRoot {
            GuestRoot::from_directory(File::open(&self.0).unwrap()).unwrap()
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn links_resolve_inside_guest_root_and_file_lease_survives_replacement() {
        let root = Root::new();
        fs::create_dir(root.0.join("lib")).unwrap();
        fs::write(root.0.join("lib/image"), b"original").unwrap();
        symlink("/lib", root.0.join("absolute")).unwrap();
        symlink("../lib/image", root.0.join("lib/relative")).unwrap();
        let resolver = root.resolver();
        let resolved = resolver.open(b"/absolute/relative").unwrap();
        assert_eq!(resolved.canonical_path, b"/lib/image");
        fs::rename(root.0.join("lib/image"), root.0.join("lib/old")).unwrap();
        fs::write(root.0.join("lib/image"), b"replacement").unwrap();
        let mut contents = String::new();
        resolved
            .node
            .into_file()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "original");
        // The stricter broker retains its original no-symlink contract.
        let strict = ReadOnlyBroker::from_directory(File::open(&root.0).unwrap()).unwrap();
        assert!(strict.open(b"absolute/image").is_err());
    }

    #[test]
    fn absolute_links_cross_owned_mounts_and_parent_returns_to_guest() {
        let root = Root::new();
        let data = Root::new();
        fs::write(root.0.join("system-file"), b"system").unwrap();
        fs::write(data.0.join("data-file"), b"data").unwrap();
        symlink("/data/data-file", root.0.join("to-data")).unwrap();
        symlink("../system-file", data.0.join("to-system")).unwrap();
        let mut resolver = root.resolver();
        assert!(
            resolver
                .mount_directory(b"/", File::open(&data.0).unwrap())
                .is_err()
        );
        resolver
            .mount_directory(b"/data", File::open(&data.0).unwrap())
            .unwrap();
        assert_eq!(
            resolver.open(b"/to-data").unwrap().canonical_path,
            b"/data/data-file"
        );
        assert_eq!(
            resolver.open(b"/data/to-system").unwrap().canonical_path,
            b"/system-file"
        );
        assert!(
            resolver
                .mount_directory(b"/data", File::open(&data.0).unwrap())
                .is_err()
        );
        assert!(
            resolver
                .mount_directory(b"/bad/../mount", File::open(&data.0).unwrap())
                .is_err()
        );
    }

    #[test]
    fn root_parent_loop_and_trailing_slash_semantics() {
        let root = Root::new();
        fs::write(root.0.join("file"), b"data").unwrap();
        symlink("loop", root.0.join("loop")).unwrap();
        symlink("/../../file", root.0.join("rooted")).unwrap();
        let resolver = root.resolver();
        assert_eq!(resolver.open(b"/rooted").unwrap().canonical_path, b"/file");
        assert_eq!(
            resolver.open(b"/loop").err().unwrap().raw_os_error(),
            Some(62)
        );
        assert!(resolver.open(b"/file/").is_err());
        assert!(resolver.open(b"/file/..").is_err());
        assert!(resolver.open(b"relative").is_err());
        assert!(resolver.open(b"/bad\0name").is_err());
        assert_eq!(resolver.open(b"/../").unwrap().canonical_path, b"/");
    }

    #[test]
    fn relative_directory_tracks_rename_and_guest_mount_boundaries() {
        let root = Root::new();
        let data = Root::new();
        fs::create_dir_all(root.0.join("old/child")).unwrap();
        fs::create_dir(root.0.join("new")).unwrap();
        fs::write(root.0.join("old/child/file"), b"original").unwrap();
        fs::write(root.0.join("new/parent-file"), b"parent").unwrap();
        fs::write(data.0.join("mounted"), b"mount").unwrap();
        symlink("/data/mounted", root.0.join("old/child/link")).unwrap();
        let mut resolver = root.resolver();
        resolver
            .mount_directory(b"/data", File::open(&data.0).unwrap())
            .unwrap();
        let directory = resolver.open(b"/old/child").unwrap().node.into_file();
        fs::rename(root.0.join("old/child"), root.0.join("new/child")).unwrap();
        fs::create_dir(root.0.join("old/child")).unwrap();
        fs::write(root.0.join("old/child/file"), b"wrong").unwrap();
        assert_eq!(resolver.directory_path(&directory).unwrap(), b"/new/child");
        let mut contents = String::new();
        resolver
            .open_relative(&directory, b"file", false)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "original");
        assert_eq!(
            resolver
                .open_relative(&directory, b"../parent-file", false)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            6
        );
        assert_eq!(
            resolver
                .open_relative(&directory, b"link", false)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            5
        );
        assert!(
            resolver
                .open_relative(&directory, b"link", true)
                .unwrap()
                .metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let mounted = resolver.open(b"/data").unwrap().node.into_file();
        assert_eq!(resolver.directory_path(&mounted).unwrap(), b"/data");
        assert_eq!(
            resolver
                .open_relative(&mounted, b"../new/parent-file", false)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            6
        );
        let root_fd = resolver.open(b"/").unwrap().node.into_file();
        assert!(
            resolver
                .open_relative(&root_fd, b"../../new", false)
                .unwrap()
                .metadata()
                .unwrap()
                .is_dir()
        );
        assert!(
            resolver
                .open_relative(&File::open("/").unwrap(), b"etc", false)
                .is_err()
        );
    }

    #[test]
    fn metadata_no_follow_keeps_final_link_and_follows_intermediate_links() {
        let root = Root::new();
        fs::create_dir(root.0.join("directory")).unwrap();
        symlink("/directory", root.0.join("parent")).unwrap();
        symlink("/absent", root.0.join("directory/link")).unwrap();
        let resolver = root.resolver();
        let node = resolver.open_no_follow(b"/parent/link").unwrap();
        assert_eq!(node.canonical_path, b"/directory/link");
        assert!(node.node.metadata().file_type().is_symlink());
        assert!(resolver.open(b"/parent/link").is_err());
        assert!(resolver.open_no_follow(b"/parent/link/").is_err());
        assert!(
            resolver
                .open_no_follow(b"/parent/")
                .unwrap()
                .node
                .metadata()
                .is_dir()
        );
        fs::remove_file(root.0.join("directory/link")).unwrap();
        fs::write(root.0.join("directory/link"), b"replacement").unwrap();
        assert!(
            node.node
                .into_file()
                .metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            resolver
                .open_no_follow(b"/parent/link")
                .unwrap()
                .node
                .metadata()
                .is_file()
        );
    }
}
