//! Android owner and group of nodes on the writable private `/data` mount.
//!
//! The host owns every backing file as the Darwin user, so the Android
//! uid/gid live beside the data in a host-private extended attribute. Like the
//! fs-verity digest (`dev.aim.fsverity`), its namespace is not `user.`,
//! so the guest attribute calls never list, read or replace it. A node without
//! the attribute belongs to this process's Android uid and gid.
//!
//! The credentials are the launcher's `AIM_ANDROID_UID`, captured once
//! when the facade is built; guest code that later edits its environment does
//! not change them. Without that identity (tools and tests), stat keeps the
//! host owner and chown remains unsupported, since no authority exists to
//! decide it. The read-only image and the other writable mounts keep their
//! existing metadata.
use super::ownership_policy::{
    self, CAP_CHOWN, CAP_FSETID, InodeOwnership, OwnershipRequest, TrustedCredentials,
};
use super::*;
use aim_fs_broker::guest_path::MountOrigin;

/// Host extended attribute holding the Android owner: uid then gid, each a
/// little-endian u32. Any other length is treated as absent.
const OWNER_ATTRIBUTE: &CStr = c"dev.aim.owner";
const OWNER_ATTRIBUTE_SIZE: usize = 8;
const ANDROID_ROOT_UID: u32 = 0;
const ANDROID_SYSTEM_UID: u32 = 1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct GuestOwner {
    pub(super) uid: u32,
    pub(super) gid: u32,
}

/// The trusted Android identity of this process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProcessCredentials {
    uid: u32,
    gid: u32,
    effective_capabilities: u64,
}

impl ProcessCredentials {
    pub(super) fn from_environment() -> Option<Self> {
        std::env::var("AIM_ANDROID_UID")
            .ok()?
            .parse::<u32>()
            .ok()
            .map(Self::for_uid)
    }

    /// An app's primary gid equals its uid, and it has no supplementary groups
    /// here. Root and the system server hold CAP_CHOWN and CAP_FSETID, as the
    /// zygote grants system_server; apps hold neither.
    pub(super) fn for_uid(uid: u32) -> Self {
        let effective_capabilities = if uid == ANDROID_ROOT_UID || uid == ANDROID_SYSTEM_UID {
            (1_u64 << CAP_CHOWN) | (1_u64 << CAP_FSETID)
        } else {
            0
        };
        Self {
            uid,
            gid: uid,
            effective_capabilities,
        }
    }

    pub(super) fn default_owner(self) -> GuestOwner {
        GuestOwner {
            uid: self.uid,
            gid: self.gid,
        }
    }

    fn trusted(self) -> TrustedCredentials<'static> {
        TrustedCredentials {
            fsuid: self.uid,
            fsgid: self.gid,
            supplementary_groups: &[],
            effective_capabilities: self.effective_capabilities,
        }
    }
}

fn host_path(file: &File) -> Option<PathBuf> {
    let mut buffer = [0u8; libc::MAXPATHLEN as usize];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes into the buffer.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } != 0 {
        return None;
    }
    let length = buffer.iter().position(|byte| *byte == 0)?;
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        buffer[..length].to_vec(),
    )))
}

fn read_owner(file: &File) -> std::io::Result<Option<GuestOwner>> {
    let mut value = [0u8; OWNER_ATTRIBUTE_SIZE];
    // SAFETY: live descriptor, terminated name, buffer of `value.len()`.
    let length = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            OWNER_ATTRIBUTE.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if length < 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOATTR | libc::ERANGE) => Ok(None),
            _ => Err(error),
        };
    }
    if length as usize != OWNER_ATTRIBUTE_SIZE {
        return Ok(None);
    }
    let (uid, gid) = value.split_at(4);
    Ok(Some(GuestOwner {
        uid: u32::from_le_bytes(uid.try_into().expect("four bytes")),
        gid: u32::from_le_bytes(gid.try_into().expect("four bytes")),
    }))
}

fn write_owner(file: &File, owner: GuestOwner) -> std::io::Result<()> {
    let mut value = [0u8; OWNER_ATTRIBUTE_SIZE];
    value[..4].copy_from_slice(&owner.uid.to_le_bytes());
    value[4..].copy_from_slice(&owner.gid.to_le_bytes());
    // SAFETY: live descriptor, terminated name, readable value slice.
    if unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            OWNER_ATTRIBUTE.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl Facade {
    /// Whether `file` is a node of the private `/data` mount. The host names
    /// are compared as the kernel reports them, so a link in the configured
    /// root path cannot hide a node from the owner view.
    fn is_private_node(&self, file: &File) -> bool {
        let Some(root) = self.private_root.as_ref() else {
            return false;
        };
        match (host_path(root.directory()), host_path(file)) {
            (Some(root), Some(node)) => node.starts_with(root),
            _ => false,
        }
    }

    /// The Android owner of a private-mount node, or none where the host
    /// owner is still reported.
    fn private_owner(&self, file: &File) -> Option<GuestOwner> {
        let credentials = self.credentials?;
        if !self.is_private_node(file) {
            return None;
        }
        // An unreadable attribute is not an ownership claim.
        Some(
            read_owner(file)
                .ok()
                .flatten()
                .unwrap_or(credentials.default_owner()),
        )
    }

    /// Android stat of an opened host node: its host metadata with the
    /// private mount's Android owner.
    pub(super) fn android_stat(&self, file: &File, metadata: &Metadata) -> AndroidStat {
        let mut status = metadata_to_android(metadata);
        if let Some(owner) = self.private_owner(file) {
            status.st_uid = owner.uid;
            status.st_gid = owner.gid;
        }
        status
    }

    /// As [`Self::android_stat`] for a node whose guest mount is known: a
    /// node resolved through another mount skips the host-path comparison.
    pub(super) fn android_stat_from(
        &self,
        origin: Option<&MountOrigin>,
        file: &File,
        metadata: &Metadata,
    ) -> AndroidStat {
        let other_mount = origin.is_some_and(|origin| {
            self.private_root
                .as_ref()
                .is_none_or(|root| origin.mount_path() != root.guest_prefix())
        });
        if other_mount {
            return metadata_to_android(metadata);
        }
        self.android_stat(file, metadata)
    }

    /// The owner a new overlay node starts with.
    pub(super) fn overlay_creator(&self) -> Option<GuestOwner> {
        self.credentials.map(ProcessCredentials::default_owner)
    }

    /// Authorize and compute an ownership change for the current owner and
    /// Linux mode; the caller commits the result.
    pub(super) fn authorize_chown(
        &self,
        current: GuestOwner,
        mode: u32,
        owner: u32,
        group: u32,
    ) -> Result<InodeOwnership, c_int> {
        let credentials = self.credentials.ok_or(ANDROID_EOPNOTSUPP)?;
        ownership_policy::authorize(
            credentials.trusted(),
            InodeOwnership {
                uid: current.uid,
                gid: current.gid,
                mode,
            },
            OwnershipRequest {
                uid: owner,
                gid: group,
            },
        )
        .map_err(|error| error.errno())
    }

    /// fchown of a host node: private-mount nodes store the authorized owner
    /// and drop the set-ID bits the policy clears; every other host node is
    /// read-only here.
    pub(super) fn chown_host_node(&self, file: &File, owner: u32, group: u32) -> c_int {
        let Some(credentials) = self.credentials else {
            return self.fail(ANDROID_EROFS);
        };
        if !self.is_private_node(file) {
            return self.fail(ANDROID_EROFS);
        }
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return self.fail_io(&error),
        };
        let current = match read_owner(file) {
            Ok(stored) => stored.unwrap_or(credentials.default_owner()),
            Err(error) => return self.fail_io(&error),
        };
        let authorized = match self.authorize_chown(current, metadata.mode(), owner, group) {
            Ok(authorized) => authorized,
            Err(error) => return self.fail(error),
        };
        let owner = GuestOwner {
            uid: authorized.uid,
            gid: authorized.gid,
        };
        if let Err(error) = write_owner(file, owner) {
            return self.fail_io(&error);
        }
        if authorized.mode != metadata.mode() {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                file.set_permissions(fs::Permissions::from_mode(authorized.mode & 0o7777))
            {
                return self.fail_io(&error);
            }
        }
        0
    }
}
