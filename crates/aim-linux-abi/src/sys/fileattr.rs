//! FS_IOC_GETFLAGS and FS_IOC_SETFLAGS (Linux v6.12 `fs/ioctl.c`,
//! `fs/file_attr.c`) with the rules of the filesystem the guest sees: ext4's
//! (`fs/ext4/ioctl.c`) where statfs reports ext4, shmem's (`mm/shmem.c`) on
//! tmpfs and memfd. Inode flags are the host's immutable and append-only
//! flags and fs-verity. Only immutable is changed here: Darwin's
//! append-only refuses a touch with the current time, which Linux allows,
//! and a tmpfs or memfd host file must stay removable by its owner, as
//! Linux discards a tmpfs whatever its flags. Changing a flag this
//! filesystem cannot honour is EOPNOTSUPP, as ext4 answers a feature it
//! lacks (casefold without the feature, for one).
use super::attrs::{self, Host};
use super::cred::Identity;
use super::fdtab::Kind;
use super::fsops::{fd_guest, inode_mutation, metadata_fd_copy, writable_fd_metadata};
use crate::errno::{self, Errno};
use std::os::fd::AsRawFd;

pub(super) const GETFLAGS: u64 = 0x80086601;
const SETFLAGS: u64 = 0x40086602;
const IMMUTABLE: u32 = 0x10;
const APPEND: u32 = 0x20;
pub(super) const VERITY: u32 = 0x100000;
const CAP_FOWNER: u64 = 1 << 3;
const CAP_LINUX_IMMUTABLE: u64 = 1 << 9;
const HOST_IMMUTABLE: u32 = libc::UF_IMMUTABLE | libc::SF_IMMUTABLE;
const HOST_APPEND: u32 = libc::UF_APPEND | libc::SF_APPEND;

/// Whether an inode is immutable or append-only, so that its mode, owner
/// and extended attributes stay as they are (`may_setattr`,
/// `xattr_permission`); Darwin refuses only some of those changes.
pub(super) fn fixed(stat: &libc::stat) -> bool {
    stat.st_flags & (HOST_IMMUTABLE | HOST_APPEND) != 0
}

/// A filesystem's `fileattr_set` rules.
struct Rules {
    /// Flags a request may hold (others are EOPNOTSUPP).
    accepted: u32,
    /// Flags a request changes; the rest keep their state.
    modifiable: u32,
    /// Flags this layer changes on the host.
    honoured: u32,
    /// `ext4_ioctl_check_immutable`.
    immutable_check: bool,
}
/// `EXT4_FL_USER_VISIBLE`, `EXT4_FL_USER_MODIFIABLE`.
static EXT4: Rules = Rules {
    accepted: 0x725b_dbff,
    modifiable: 0x604b_c0ff,
    honoured: IMMUTABLE,
    immutable_check: true,
};
/// `SHMEM_FL_USER_MODIFIABLE`: immutable, append-only, nodump, noatime.
static SHMEM: Rules = Rules {
    accepted: 0xf0,
    modifiable: 0xf0,
    honoured: 0,
    immutable_check: false,
};

/// The request's answer if it is one of these on an inode with fileattr
/// operations. Device nodes (ashmem, dma-buf and evdev among them, whatever
/// backs them on the host), the kernel filesystems (procfs, sysfs,
/// selinuxfs, cgroup, bpf) and the layer's synthesized files have none, and
/// their own ioctl answers.
pub(super) fn ioctl(fd: i32, request: u64, arg: u64) -> Option<i64> {
    ioctl_as(fd, request, arg, &super::cred::current())
}
fn ioctl_as(fd: i32, request: u64, arg: u64, id: &Identity) -> Option<i64> {
    if !matches!(request, GETFLAGS | SETFLAGS) {
        return None;
    }
    let rules = rules(fd)?;
    Some(if request == GETFLAGS {
        get(fd, arg)
    } else {
        set(fd, arg, id, rules)
    })
}

fn rules(fd: i32) -> Option<&'static Rules> {
    if super::memfd::key(fd).is_some() {
        return Some(&SHMEM);
    }
    if !matches!(
        super::fdtab::get(fd),
        None | Some(Kind::Regular(_) | Kind::Dir(_))
    ) || super::fdtab::character_reference(fd).is_some()
        || super::dir::synthesized_path(fd).is_some()
    {
        return None;
    }
    // The guest's inode type: a device's host file is a character device.
    let stat = super::fs::stat_fd_kernel(fd).ok()?;
    if !matches!(stat.st_mode & libc::S_IFMT, libc::S_IFREG | libc::S_IFDIR)
        || super::dma_heap::is_buffer(&stat)
    {
        return None;
    }
    let guest = fd_guest(fd)?;
    if crate::vfs::lookup(&guest).1 == crate::vfs::Area::Kernfs
        || super::selinuxfs::statfs_magic(&guest).is_some()
    {
        return None;
    }
    match crate::vfs::fstype(&guest).as_deref() {
        Some("tmpfs") => Some(&SHMEM),
        Some(kind) if super::fs::kernel_fs_magic(kind).is_some() => None,
        _ => Some(&EXT4),
    }
}

/// Whether `fd` is a file of a writable ext4, which has fscrypt.
pub(super) fn ext4(fd: i32) -> bool {
    rules(fd).is_some_and(|rules| std::ptr::eq(rules, &EXT4))
        && super::fs::stat_fd_kernel(fd)
            .is_ok_and(|stat| !crate::vfs::on_read_only_root(stat.st_dev))
}

fn flags(fd: i32, stat: &libc::stat) -> Result<u32, Errno> {
    let mut flags = 0;
    if super::fscrypt::encrypted(Host::Fd(fd))? {
        flags |= super::fscrypt::ENCRYPTED;
    }
    if stat.st_flags & HOST_IMMUTABLE != 0 {
        flags |= IMMUTABLE;
    }
    if stat.st_flags & HOST_APPEND != 0 {
        flags |= APPEND;
    }
    if super::regular_file::verity_stat(stat)? == Some(true) {
        flags |= VERITY;
    }
    Ok(flags)
}

fn get(fd: i32, arg: u64) -> i64 {
    let stat = match super::fs::stat_fd_kernel(fd) {
        Ok(stat) => stat,
        Err(error) => return error,
    };
    flags(fd, &stat)
        .and_then(|flags| super::user_memory::write_exact(arg, &flags.to_le_bytes()))
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64))
}

/// `ioctl_setflags`: the owner (or CAP_FOWNER) sets them on a writable
/// mount, CAP_LINUX_IMMUTABLE changes immutable or append-only, and an
/// ext4 immutable inode takes no other change (`ext4_ioctl_check_immutable`).
fn set(fd: i32, arg: u64, id: &Identity, rules: &Rules) -> i64 {
    let requested = match super::user_memory::read_exact(arg, 4) {
        Ok(bytes) => u32::from_le_bytes(bytes.try_into().unwrap()),
        Err(error) => return -(error as i64),
    };
    let anchor = match metadata_fd_copy(fd) {
        Ok(anchor) => anchor,
        Err(error) => return error,
    };
    let fd = anchor.as_raw_fd();
    let guest = fd_guest(fd).unwrap_or_default();
    inode_mutation(Host::Fd(fd), &guest, true, |stat| {
        if let Err(error) = writable_fd_metadata(fd, stat) {
            return error;
        }
        if stat.st_uid != id.uid[attrs::FS] && id.cap_eff & CAP_FOWNER == 0 {
            return -(errno::EPERM as i64);
        }
        let old = match flags(fd, stat) {
            Ok(flags) => flags,
            Err(error) => return -(error as i64),
        };
        if (old ^ requested) & (IMMUTABLE | APPEND) != 0 && id.cap_eff & CAP_LINUX_IMMUTABLE == 0 {
            return -(errno::EPERM as i64);
        }
        if requested & !rules.accepted != 0 {
            return -(errno::EOPNOTSUPP as i64);
        }
        // Visible flags that cannot be set (fs-verity) keep their state.
        let masked = requested & rules.modifiable;
        if rules.immutable_check
            && old & masked & IMMUTABLE != 0
            && old & !IMMUTABLE != masked & !IMMUTABLE
        {
            return -(errno::EPERM as i64);
        }
        let new = masked | old & !rules.modifiable;
        if (old ^ new) & !rules.honoured != 0 {
            return -(errno::EOPNOTSUPP as i64);
        }
        if old == new {
            return 0;
        }
        let host = if new & IMMUTABLE == 0 {
            stat.st_flags & !HOST_IMMUTABLE
        } else {
            stat.st_flags | libc::UF_IMMUTABLE
        };
        if unsafe { libc::fchflags(fd, host) } < 0 {
            return -(errno::last() as i64);
        }
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn get_flags(fd: i32) -> Result<u32, i64> {
        let mut flags = 0u32;
        match ioctl_as(
            fd,
            GETFLAGS,
            (&mut flags as *mut u32) as u64,
            &Identity::default(),
        ) {
            Some(0) => Ok(flags),
            Some(error) => Err(error),
            None => Err(0),
        }
    }
    fn set_flags(fd: i32, flags: u32, id: &Identity) -> Option<i64> {
        ioctl_as(fd, SETFLAGS, (&flags as *const u32) as u64, id)
    }

    /// `may_setattr` and `xattr_permission` on an immutable or append-only
    /// inode, including a chmod that leaves the host mode as it is.
    fn assert_unchangeable(path: &std::path::Path, fd: i32) {
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        let root = Identity {
            cap_eff: !0,
            ..Default::default()
        };
        let eperm = -(errno::EPERM as i64);
        let fd = unsafe { libc::dup(fd) };
        super::super::fdtab::publish_guest(fd).unwrap();
        let args = |value: u64| [fd as u64, value, u32::MAX as u64, 0, 0, 0];
        assert_eq!(
            super::super::fsops::fchmod_as(args(mode as u64), &root),
            eperm
        );
        assert_eq!(super::super::fsops::fchown_as(args(0), &root), eperm);
        let value = b"v";
        let set = [
            fd as u64,
            c"user.fileattr".as_ptr() as u64,
            value.as_ptr() as u64,
            1,
            0,
            0,
        ];
        assert_eq!(super::super::xattr::setxattr(7, set), eperm);
        assert_eq!(super::super::xattr::removexattr(16, set), eperm);
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    #[test]
    fn flags_follow_ext4_permission_and_feature_semantics() {
        let (_guard, view) = crate::vfs::test_view();
        let dir = view.join("data/fileattr");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file");
        std::fs::write(&path, b"bytes").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let directory = std::fs::File::open(&dir).unwrap();
        let (fd, dir_fd) = (file.as_raw_fd(), directory.as_raw_fd());
        let root = Identity {
            cap_eff: CAP_FOWNER | CAP_LINUX_IMMUTABLE,
            ..Default::default()
        };
        let uid = super::super::fs::stat_fd_kernel(fd).unwrap().st_uid;
        let owner = Identity {
            uid: [uid; 4],
            cap_eff: 0,
            ..Default::default()
        };
        let stranger = Identity {
            uid: [uid + 1; 4],
            cap_eff: 0,
            ..Default::default()
        };

        assert_eq!(get_flags(fd), Ok(0));
        assert_eq!(get_flags(dir_fd), Ok(0));
        assert_eq!(
            ioctl_as(fd, GETFLAGS, 1, &root),
            Some(-(errno::EFAULT as i64))
        );
        assert_eq!(
            ioctl_as(fd, SETFLAGS, 1, &root),
            Some(-(errno::EFAULT as i64))
        );
        // Setting the flags it has back is a change of none.
        assert_eq!(set_flags(fd, 0, &owner), Some(0));
        assert_eq!(set_flags(fd, 0, &stranger), Some(-(errno::EPERM as i64)));
        // chattr +F: no casefold here; flags ext4 never shows.
        assert_eq!(
            set_flags(dir_fd, 0x4000_0000, &root),
            Some(-(errno::EOPNOTSUPP as i64))
        );
        assert_eq!(
            set_flags(fd, 0x8000_0000, &root),
            Some(-(errno::EOPNOTSUPP as i64))
        );
        // Visible but not settable: fs-verity is not enabled by SETFLAGS.
        assert_eq!(set_flags(fd, VERITY, &root), Some(0));
        assert_eq!(get_flags(fd), Ok(0));
        // Immutable and append-only need CAP_LINUX_IMMUTABLE; Darwin's
        // append-only refuses a touch Linux allows, so it is not set here.
        assert_eq!(set_flags(fd, APPEND, &owner), Some(-(errno::EPERM as i64)));
        assert_eq!(
            set_flags(fd, APPEND, &root),
            Some(-(errno::EOPNOTSUPP as i64))
        );
        assert_eq!(
            set_flags(fd, IMMUTABLE, &owner),
            Some(-(errno::EPERM as i64))
        );
        assert_eq!(set_flags(fd, IMMUTABLE, &root), Some(0));
        assert_eq!(get_flags(fd), Ok(IMMUTABLE));
        assert_eq!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        // An immutable inode takes no other change but losing immutable.
        assert_eq!(
            set_flags(fd, IMMUTABLE | 0x40, &root),
            Some(-(errno::EPERM as i64))
        );
        assert_unchangeable(&path, fd);
        assert_eq!(set_flags(fd, 0, &root), Some(0));
        assert_eq!(get_flags(fd), Ok(0));
        // A host append-only flag shows, and keeps mode, owner and xattrs.
        assert_eq!(unsafe { libc::fchflags(fd, libc::UF_APPEND) }, 0);
        assert_eq!(get_flags(fd), Ok(APPEND));
        assert_unchangeable(&path, fd);
        assert_eq!(unsafe { libc::fchflags(fd, 0) }, 0);
        std::fs::write(&path, b"writable again").unwrap();

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inodes_without_fileattr_operations_answer_their_own_ioctl() {
        let (_guard, view) = crate::vfs::test_view();
        let root = Identity {
            cap_eff: !0,
            ..Default::default()
        };
        let none = |fd: i32| {
            assert_eq!(get_flags(fd), Err(0));
            assert_eq!(set_flags(fd, IMMUTABLE, &root), None);
        };
        let (read, write) = std::io::pipe().unwrap();
        none(read.as_raw_fd());
        drop((read, write));
        // Device nodes whose host files are regular: ashmem and dma-buf.
        let region = aim_ashmem::read_only_region(b"region").unwrap();
        none(region.as_raw_fd());
        let heap = super::super::dma_heap::open("/dev/dma_heap/system", 0).unwrap();
        none(heap as i32);
        let mut allocation = [0u8; 24];
        allocation[..8].copy_from_slice(&4096u64.to_le_bytes());
        assert_eq!(
            super::super::dma_heap::ioctl(heap as i32, 0xc018_4800, allocation.as_mut_ptr() as u64),
            Some(0)
        );
        let buffer = u32::from_le_bytes(allocation[8..12].try_into().unwrap()) as i32;
        none(buffer);
        // A synthesized /proc file, and a sysfs directory with a host tree.
        let content = super::super::procfs::content_fd(b"status", true) as i32;
        none(content);
        let sys = view.join("fileattr-sys");
        std::fs::create_dir_all(sys.join("fs")).unwrap();
        crate::vfs::add_mount(
            "/sys",
            sys.clone(),
            crate::vfs::Area::Kernfs,
            "sysfs",
            "sysfs",
        )
        .unwrap();
        let kernfs = std::fs::File::open(sys.join("fs")).unwrap();
        none(kernfs.as_raw_fd());
        // The guest's close: a host close would leave their entries in the
        // fd table for the next file that reuses the number.
        for fd in [heap as i32, buffer, content] {
            assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
        assert!(crate::vfs::remove_mount("/sys").unwrap());
        std::fs::remove_dir_all(sys).unwrap();
    }

    #[test]
    fn tmpfs_follows_shmem_rules() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("fileattr-tmpfs");
        std::fs::create_dir_all(&host).unwrap();
        crate::vfs::add_mount(
            "/data/fileattr-tmpfs",
            host.clone(),
            crate::vfs::Area::Writable,
            "tmpfs",
            "tmpfs",
        )
        .unwrap();
        std::fs::write(host.join("file"), b"bytes").unwrap();
        let file = std::fs::File::open(host.join("file")).unwrap();
        let fd = file.as_raw_fd();
        let root = Identity {
            cap_eff: !0,
            ..Default::default()
        };
        assert_eq!(get_flags(fd), Ok(0));
        assert_eq!(set_flags(fd, 0, &root), Some(0));
        // ext4 drops a visible flag it cannot set; shmem refuses it.
        assert_eq!(
            set_flags(fd, VERITY, &root),
            Some(-(errno::EOPNOTSUPP as i64))
        );
        // The host file stays removable when the tmpfs goes away.
        assert_eq!(
            set_flags(fd, IMMUTABLE, &root),
            Some(-(errno::EOPNOTSUPP as i64))
        );
        assert_eq!(get_flags(fd), Ok(0));
        drop(file);
        assert!(crate::vfs::remove_mount("/data/fileattr-tmpfs").unwrap());
        std::fs::remove_dir_all(host).unwrap();
    }
}
