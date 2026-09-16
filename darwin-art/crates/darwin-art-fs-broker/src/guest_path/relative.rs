//! Descriptor-relative guest lookup. No canonical pathname reopen for dirfd.
use super::*;
use std::os::unix::fs::MetadataExt;

fn same_directory(a: &File, b: &File) -> io::Result<bool> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

impl GuestRoot {
    /// Inspect only the namespace mount table at one retained parent/leaf.
    /// This does not open the leaf or impose read permission on a write-only
    /// open. None is not authorization: the caller must already own the parent.
    pub fn mounted_origin_at(
        &self,
        directory: &File,
        name: &[u8],
    ) -> io::Result<Option<MountOrigin>> {
        if name.is_empty() || name.contains(&0) || name.contains(&b'/') {
            return Err(io::Error::from_raw_os_error(22));
        }
        self.relative_mount(directory, name)
            .map(|mount| mount.map(|(_, origin)| origin))
    }

    fn check_directory_boundary(&self, directory: &File) -> io::Result<()> {
        let mut cursor = directory.try_clone()?;
        if !cursor.metadata()?.is_dir() {
            return Err(io::Error::from_raw_os_error(20));
        }
        for _ in 0..4096 {
            if same_directory(&cursor, &self.root.root)? {
                return Ok(());
            }
            for mount in self.mounts.values() {
                if same_directory(&cursor, &mount.root)? {
                    return Ok(());
                }
            }
            let parent = open_component(
                &cursor,
                b"..",
                O_RDONLY | O_DIRECTORY | O_CLOEXEC,
                None,
                "check guest ancestry",
            )
            .map_err(io_error)?;
            if same_directory(&cursor, &parent)? {
                break;
            }
            cursor = parent;
        }
        Err(io::Error::from_raw_os_error(13))
    }

    fn relative_parent(&self, directory: &File) -> io::Result<(File, Option<MountOrigin>)> {
        if same_directory(directory, &self.root.root)? {
            return Ok((directory.try_clone()?, None));
        }
        for (path, mount) in &self.mounts {
            if same_directory(directory, &mount.root)? {
                let split = path
                    .iter()
                    .rposition(|b| *b == b'/')
                    .expect("absolute mount");
                let parent = if split == 0 {
                    b"/".as_slice()
                } else {
                    &path[..split]
                };
                let opened = self.open(parent)?;
                return Ok((opened.node.into_file(), Some(opened.origin)));
            }
        }
        let parent = open_component(
            directory,
            b"..",
            O_RDONLY | O_DIRECTORY | O_CLOEXEC,
            None,
            "open guest parent",
        )
        .map_err(io_error)?;
        self.check_directory_boundary(&parent)?;
        Ok((parent, None))
    }

    fn relative_mount(
        &self,
        directory: &File,
        name: &[u8],
    ) -> io::Result<Option<(File, MountOrigin)>> {
        for (path, mount) in &self.mounts {
            let split = path
                .iter()
                .rposition(|b| *b == b'/')
                .expect("absolute mount");
            if &path[split + 1..] != name {
                continue;
            }
            let parent = if split == 0 {
                b"/".as_slice()
            } else {
                &path[..split]
            };
            let parent = self.open(parent)?.node.into_file();
            if same_directory(directory, &parent)? {
                return Ok(Some((
                    mount.root.try_clone()?,
                    MountOrigin {
                        namespace: self.namespace.clone(),
                        mount: Some(path.clone()),
                    },
                )));
            }
        }
        Ok(None)
    }

    /// Resolve from a live directory descriptor, including rename/replacement.
    /// Absolute symlinks restart at this guest root. Returned file is owned.
    pub fn open_relative(
        &self,
        directory: &File,
        path: &[u8],
        no_follow: bool,
    ) -> io::Result<File> {
        self.walk_relative(directory, path, no_follow, None)
            .map(|(file, _)| file)
    }

    /// Preserve the origin retained alongside `directory` by its FD owner.
    /// A different resolver's token is rejected. This is mount identity only,
    /// not evidence of image authenticity or permission to project metadata.
    pub fn open_relative_with_origin(
        &self,
        directory: &File,
        origin: &MountOrigin,
        path: &[u8],
        no_follow: bool,
    ) -> io::Result<(File, MountOrigin)> {
        if !std::sync::Arc::ptr_eq(&origin.namespace, &self.namespace) {
            return Err(io::Error::from_raw_os_error(13));
        }
        let (file, origin) =
            self.walk_relative(directory, path, no_follow, Some(origin.clone()))?;
        Ok((file, origin.expect("retained origin is never cleared")))
    }

    fn walk_relative(
        &self,
        directory: &File,
        path: &[u8],
        no_follow: bool,
        mut origin: Option<MountOrigin>,
    ) -> io::Result<(File, Option<MountOrigin>)> {
        if path.is_empty() {
            return Err(io::Error::from_raw_os_error(2));
        }
        if path.contains(&0) {
            return Err(io::Error::from_raw_os_error(22));
        }
        if path.len() >= 4096 {
            return Err(io::Error::from_raw_os_error(63));
        }
        if path.starts_with(b"/") {
            let opened = if no_follow {
                self.open_no_follow(path)?
            } else {
                self.open(path)?
            };
            return Ok((opened.node.into_file(), Some(opened.origin)));
        }
        self.check_directory_boundary(directory)?;
        let mut current = directory.try_clone()?;
        let mut pending: VecDeque<Vec<u8>> = path
            .split(|b| *b == b'/')
            .filter(|part| !part.is_empty())
            .map(Vec::from)
            .collect();
        let mut require_directory = path.ends_with(b"/");
        let mut links = 0;
        while let Some(component) = pending.pop_front() {
            if component == b"." {
                continue;
            }
            if component == b".." {
                let (parent, parent_origin) = self.relative_parent(&current)?;
                current = parent;
                if parent_origin.is_some() {
                    origin = parent_origin;
                }
                continue;
            }
            if let Some((mount, mount_origin)) = self.relative_mount(&current, &component)? {
                current = mount;
                origin = Some(mount_origin);
                continue;
            }
            if no_follow && pending.is_empty() && !require_directory {
                return open_component(
                    &current,
                    &component,
                    O_RDONLY | O_NONBLOCK | O_CLOEXEC | 0x0020_0000,
                    None,
                    "open relative guest metadata",
                )
                .map(|file| (file, origin))
                .map_err(io_error);
            }
            let name = CString::new(component.clone()).expect("NUL checked");
            let mut target = [0u8; 4096];
            let count = unsafe {
                readlinkat(
                    current.as_raw_fd(),
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
                    current = self.root.root.try_clone()?;
                    origin = Some(MountOrigin {
                        namespace: self.namespace.clone(),
                        mount: None,
                    });
                }
                if pending.is_empty() && target.ends_with(b"/") {
                    require_directory = true;
                }
                for part in target.split(|b| *b == b'/').filter(|p| !p.is_empty()).rev() {
                    pending.push_front(part.to_vec());
                }
                if pending.iter().map(|p| p.len() + 1).sum::<usize>() >= 4096 {
                    return Err(io::Error::from_raw_os_error(63));
                }
                continue;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(22) {
                return Err(error);
            }
            let mut flags = O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC;
            if !pending.is_empty() || require_directory {
                flags |= O_DIRECTORY;
            }
            current = open_component(
                &current,
                &component,
                flags,
                None,
                "open relative guest node",
            )
            .map_err(io_error)?;
        }
        if current.metadata()?.is_dir() {
            // Traversal may finish on a cloned cwd/mount authority. Opening
            // that directory must not share its directory-stream offset.
            return open_component(
                &current,
                b".",
                O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                None,
                "open resolved relative directory",
            )
            .map(|file| (file, origin))
            .map_err(io_error);
        }
        Ok((current, origin))
    }
}
