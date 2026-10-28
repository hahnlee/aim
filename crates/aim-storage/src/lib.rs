//! Case-sensitive APFS disk images for the guest (docs/storage.md).
//!
//! Android's filesystems are case-sensitive, the Mac's usually are not: an
//! original image holds names that differ only in case, and apps create
//! them in `/data`. The guest's files therefore live in disk images the
//! user attaches without admin rights, hidden from the Finder:
//!
//! - [`system`]: the pinned original image as a compressed read-only image
//!   with its translation cache, and the derived image as a shadow over it;
//! - [`data`]: the writable data of one data directory, a sparse image;
//! - [`copy`]: copies between them that keep what the guest sees.

pub mod copy;
pub mod data;
pub mod disk;
pub mod guest_inode;
pub mod system;

pub mod boot_id;
pub mod mount_namespace;
pub mod process_namespace;

pub mod socket_inode;

pub mod inode_lease;

pub mod fsverity;

pub mod private_fd;

pub mod verity_control;

pub mod posix_control;

pub mod posix_broker;

pub mod socket_queue_root;

pub mod constructor_capture;

pub mod pty_owner;

pub mod inherited;

pub mod pipe_identity;

pub mod bpf_object;

pub mod cgroup_v1;
pub mod cgroup_v2;

/// A new test directory of this process's own, `<prefix>-<pid>-<n>` in the
/// system's temporary directory: created exclusively, skipping names that
/// exist, so tests running at once and a killed earlier run with the same
/// pid never share one (#2117).
#[cfg(test)]
pub(crate) fn scratch_dir(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => return dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("{}: {error}", dir.display()),
        }
    }
}
