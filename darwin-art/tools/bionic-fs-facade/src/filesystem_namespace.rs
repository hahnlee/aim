//! Process mount namespace: retained root authorities, mount policy and cwd.
//! Construction is transactional. All mounts are attached before cwd resolves;
//! no process-global state is published until every authority has validated.
use super::{MountKind, MountTable, PrivateDataRoot, ReadOnlyBroker};
use crate::working_directory::WorkingDirectory;
use darwin_art_fs_broker::guest_path::GuestRoot;
use std::fs::File;
use std::sync::Arc;

pub(super) struct FilesystemNamespace {
    pub prefix: MountTable,
    pub broker: ReadOnlyBroker,
    pub guest_root: Option<Arc<GuestRoot>>,
    pub cwd: WorkingDirectory,
    /// Installed package code is mounted read-only at `/data/app` (mount 4).
    pub packages: bool,
}

/// Guest prefix of installed package code, as under Android's `/data/app`.
pub(super) const PACKAGE_PREFIX: &[u8] = b"/data/app";

impl FilesystemNamespace {
    #[cfg(test)]
    pub fn new(
        root: File,
        guest_mount: &[u8],
        cwd: &[u8],
        private: Option<&PrivateDataRoot>,
    ) -> Result<Self, &'static str> {
        Self::with_storage(root, guest_mount, cwd, private, None, None)
    }

    pub fn with_storage(
        root: File,
        guest_mount: &[u8],
        cwd: &[u8],
        private: Option<&PrivateDataRoot>,
        storage: Option<&super::writable_mount::WritableMount>,
        packages: Option<&File>,
    ) -> Result<Self, &'static str> {
        if storage.is_some() && guest_mount != b"/" {
            return Err("shared storage requires a complete guest namespace");
        }
        if packages.is_some() && guest_mount != b"/" {
            return Err("installed packages require a complete guest namespace");
        }
        let mut prefix = MountTable::new();
        prefix
            .add_mount(1, MountKind::Immutable, false, guest_mount)
            .map_err(|_| "invalid guest mount")?;
        prefix
            .add_mount(2, MountKind::Private, true, b"/data")
            .map_err(|_| "invalid private data mount")?;
        if let Some(storage) = storage {
            if storage.guest_prefix() != b"/storage" {
                return Err("invalid shared storage prefix");
            }
            prefix
                .add_mount(3, MountKind::Shared, true, storage.guest_prefix())
                .map_err(|_| "invalid shared storage mount")?;
        }
        if packages.is_some() {
            prefix
                .add_mount(4, MountKind::Immutable, false, PACKAGE_PREFIX)
                .map_err(|_| "invalid installed package mount")?;
        }
        prefix.seal().map_err(|_| "could not seal guest mount")?;
        if !cwd.starts_with(b"/") || cwd.contains(&0) {
            return Err("initial cwd must be an absolute guest path");
        }
        let mut guest_root = if guest_mount == b"/" {
            Some(
                GuestRoot::from_directory(
                    root.try_clone()
                        .map_err(|_| "could not duplicate guest root")?,
                )
                .map_err(|_| "invalid guest root")?,
            )
        } else {
            None
        };
        if let (Some(guest), Some(private)) = (&mut guest_root, private) {
            guest
                .mount_directory(
                    private.guest_prefix(),
                    private
                        .directory()
                        .try_clone()
                        .map_err(|_| "could not duplicate data root")?,
                )
                .map_err(|_| "could not mount data root")?;
        }
        if let (Some(guest), Some(packages)) = (&mut guest_root, packages) {
            guest
                .mount_directory(
                    PACKAGE_PREFIX,
                    packages
                        .try_clone()
                        .map_err(|_| "could not duplicate package root")?,
                )
                .map_err(|_| "could not mount installed packages")?;
        }
        let broker = ReadOnlyBroker::from_directory(root).map_err(|_| "invalid mount root")?;
        if let (Some(guest), Some(storage)) = (&mut guest_root, storage) {
            guest
                .mount_directory(
                    storage.guest_prefix(),
                    storage
                        .directory()
                        .try_clone()
                        .map_err(|_| "could not duplicate storage root")?,
                )
                .map_err(|_| "could not mount shared storage")?;
        }
        let (canonical, opened, origin) = if let Some(guest) = &guest_root {
            // Preserve symlink-before-.. semantics and virtual /data mounts.
            let resolved = guest
                .open(cwd)
                .map_err(|_| "cwd cannot be securely opened")?;
            let policy = prefix
                .resolve(b"/", &resolved.canonical_path)
                .map_err(|_| "cwd is outside guest mount")?;
            if policy.mount_id != 1
                && !(policy.mount_id == 2 && private.is_some())
                && !(policy.mount_id == 3 && storage.is_some())
            {
                return Err("cwd has no retained mount authority");
            }
            let origin = resolved.origin().clone();
            (resolved.canonical_path, resolved.node, Some(origin))
        } else {
            let initial = prefix
                .resolve(cwd, b".")
                .map_err(|_| "cwd is outside guest mount")?;
            if initial.mount_id != 1 || initial.writable {
                return Err("cwd is outside immutable guest mount");
            }
            let opened = broker
                .open(&initial.relative_path)
                .map_err(|_| "cwd cannot be securely opened")?;
            (initial.normalized_path, opened, None)
        };
        if !opened.metadata().is_dir() {
            return Err("cwd is not a directory");
        }
        let guest_root = guest_root.map(Arc::new);
        let cwd = WorkingDirectory::new(canonical, opened.into_file(), guest_root.clone(), origin);
        Ok(Self {
            prefix,
            broker,
            guest_root,
            cwd,
            packages: packages.is_some(),
        })
    }
}

#[cfg(test)]
#[path = "filesystem_namespace_tests.rs"]
mod tests;
