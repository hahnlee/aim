//! Read-only open admission. GuestRoot owns virtual link traversal; this
//! module retains the resulting file instead of reopening a host pathname.
use super::*;

impl Facade {
    pub(super) fn open_immutable(
        &self,
        path: &[u8],
        resolution: Resolution,
        flags: c_int,
        cwd: &super::working_directory::CwdSnapshot,
    ) -> c_int {
        let (file, origin) = if let Some(root) = &self.namespace.guest_root {
            let opened = if path.starts_with(b"/") {
                let result = if flags & O_NOFOLLOW != 0 {
                    root.open_no_follow(path)
                } else {
                    root.open(path)
                };
                result.map(|opened| {
                    let origin = opened.origin().clone();
                    (opened.node.into_file(), Some(origin))
                })
            } else {
                cwd.directory
                    .try_clone()
                    .map(|file| (file, cwd.origin.clone()))
                    .and_then(|(directory, origin)| match origin {
                        Some(origin) => root
                            .open_relative_with_origin(
                                &directory,
                                &origin,
                                path,
                                flags & O_NOFOLLOW != 0,
                            )
                            .map(|(file, origin)| (file, Some(origin))),
                        None => root
                            .open_relative(&directory, path, flags & O_NOFOLLOW != 0)
                            .map(|file| (file, None)),
                    })
            };
            match opened {
                Ok(file) => file,
                Err(error) => return self.fail_io(&error),
            }
        } else {
            // A legacy single-submount install cannot authorize absolute link
            // targets. Retain its strict no-link broker capability.
            let mut relative = resolution.relative_path;
            if resolution.requires_directory && !relative.is_empty() {
                relative.push(b'/');
            }
            match self.namespace.broker.open(&relative) {
                Ok(opened) => (opened.into_file(), None),
                Err(error) => return self.fail_broker(&error),
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return self.fail_io(&error),
        };
        if metadata.file_type().is_symlink() {
            return self.fail(40); // Android ELOOP, not Darwin's errno value.
        }
        if flags & O_DIRECTORY != 0 && !metadata.is_dir() {
            return self.fail(ANDROID_ENOTDIR);
        }
        if !metadata.is_file() && !metadata.is_dir() {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let mut descriptors = match self.descriptors.lock() {
            Ok(table) => table,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_origin(Descriptor::File(file), flags & O_CLOEXEC != 0, origin)
        {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }
}
