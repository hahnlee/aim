//! SELinux as the device runs it (ADR 0012: permissive), as far as
//! servicemanager's `Access` needs it:
//!
//! - selinuxfs at `/sys/fs/selinux` (libselinux finds it by its statfs
//!   magic) with `status` (the page `selinux_status_open` maps), `enforce`
//!   = 0 and `deny_unknown` = 0. The loaded policy defines no classes, so
//!   every `selinux_check_access` is an unknown class that is allowed;
//! - `/proc/<self>/attr/current`: the process's context (`--seclabel`).

use crate::errno::{self, ENOENT};

pub const SELINUX_MAGIC: u64 = 0xf97c_ff8c;
const MOUNT: &str = "/sys/fs/selinux";
const PAGE: usize = 16384;

/// The statfs type of `guest`, if it is on selinuxfs.
pub fn statfs_magic(guest: &str) -> Option<u64> {
    (guest == MOUNT || guest.strip_prefix(MOUNT)?.starts_with('/')).then_some(SELINUX_MAGIC)
}

/// Whether `guest` names this process's (or thread's) `attr/current`.
fn is_attr_current(guest: &str) -> bool {
    let Some(rest) = guest.strip_prefix("/proc/") else {
        return false;
    };
    let Some(who) = rest.strip_suffix("/attr/current") else {
        return false;
    };
    let pid = super::process::getpid().to_string();
    let who = match who.split_once("/task/") {
        Some((p, t)) if t.bytes().all(|b| b.is_ascii_digit()) => p,
        Some(_) => return false,
        None => who,
    };
    who == "self" || who == "thread-self" || who == pid
}

/// A read-only file with `bytes`, with no name left in the file system.
fn synthetic(bytes: &[u8], cloexec: bool) -> i64 {
    let mut path = std::env::temp_dir()
        .join("linux-abi-kernfs-XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    path.push(0);
    // SAFETY: mkstemp on our own template buffer; the file is ours.
    unsafe {
        let fd = libc::mkstemp(path.as_mut_ptr().cast());
        if fd < 0 {
            return -(errno::last() as i64);
        }
        libc::unlink(path.as_ptr().cast());
        libc::write(fd, bytes.as_ptr().cast(), bytes.len());
        libc::lseek(fd, 0, libc::SEEK_SET);
        if cloexec {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        fd as i64
    }
}

const O_CLOEXEC: u64 = 0o2000000;

/// `openat` of a file served here; None for other paths.
pub fn open(guest: &str, flags: u64) -> Option<i64> {
    let cloexec = flags & O_CLOEXEC != 0;
    if is_attr_current(guest) {
        let label = super::procfs::security_context();
        if label.is_empty() {
            return Some(-(ENOENT as i64));
        }
        let mut bytes = label.into_bytes();
        bytes.push(0);
        return Some(synthetic(&bytes, cloexec));
    }
    statfs_magic(guest)?;
    Some(match guest.strip_prefix(MOUNT) {
        Some("/enforce") | Some("/deny_unknown") => synthetic(b"0", cloexec),
        Some("/status") => {
            // struct selinux_kernel_status: version 1, sequence 0 (stable),
            // enforcing 0, policyload 0, deny_unknown 0.
            let mut page = vec![0u8; PAGE];
            page[0..4].copy_from_slice(&1u32.to_le_bytes());
            synthetic(&page, cloexec)
        }
        _ => -(ENOENT as i64),
    })
}
