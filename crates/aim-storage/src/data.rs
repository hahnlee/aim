//! The guest's writable data: one case-sensitive APFS image per data
//! directory, `<data>.asif` beside it, mounted hidden at `<data>` while a
//! boot runs (docs/storage.md).
//!
//! - **Created on demand** as a sparse image (ASIF) that occupies only what
//!   the guest wrote. Its size, what the guest sees as `/data`'s, is only a
//!   [`ceiling`]: the size of the host volume holding it, to which an
//!   image found smaller is grown at attach. The Mac's free space is the
//!   real limit; the guest's syscall layer keeps it from taking the last of
//!   it (`aim-linux-abi`'s `sys/space.rs`).
//! - **Space returns:** APFS TRIMs freed blocks, and an ASIF image punches
//!   them out of its file. A sparse bundle keeps them (neither TRIM nor
//!   `hdiutil compact` frees its bands on macOS 26 and later), which is why
//!   this is not one. APFS only TRIMs once a 32nd of its container has
//!   been freed, so at stop, when the file holds more than
//!   [`RECLAIM_THRESHOLD`] beyond what the volume uses, a 24th of it is
//!   allocated (not written) and freed again, and a full sync commits
//!   that: the unmount then TRIMs everything free ("compaction").
//! - **One user:** the attaching process holds an exclusive lock on
//!   `<data>.lock` (the disk image helper locks the image file itself). An
//!   attachment found at the next start belongs to a process
//!   that died without detaching (a crash); it is detached, and the image
//!   attached again, which lets APFS replay its journal.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::disk::{self, Attach};

const VOLUME: &str = "aim-data";
/// Space the image's file may hold beyond the volume's use before a stop
/// compacts it.
pub const RECLAIM_THRESHOLD: u64 = 512 << 20;
/// The part of its container APFS frees before it TRIMs is a 32nd
/// (measured on macOS 27: of a 64 GB container, 1.75 GiB freed stay in the
/// file and 2 GiB are returned; of 16 GB, 300 MB stay and 700 MB return);
/// compaction frees a 24th.
const TRIM_BATCH_DIVISOR: u64 = 24;
/// How long a busy volume is waited for at detach.
const PATIENCE: Duration = Duration::from_secs(20);
/// How long a synced volume is waited for at stop. The security agent
/// keeps a boot's freshly written files open longer than a stop should
/// wait (still scanning after 21 s); once the volume is synced, the forced
/// unmount after this only closes its reads.
const SYNCED_PATIENCE: Duration = Duration::from_secs(2);

/// The size the data image `image` may reach: that of the host volume
/// holding it.
pub fn ceiling(image: &Path) -> Result<u64, String> {
    let host = image.parent().unwrap_or(Path::new("."));
    disk::capacity(host).ok_or_else(|| format!("{}: no volume size", host.display()))
}

/// The image of the data directory `dir`.
pub fn image_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".asif");
    dir.with_file_name(name)
}

/// The boot's runtime directory of the data directory `dir` (guest-init's
/// `/dev`, sockets, identities and logs), beside it rather than in the
/// image: a boot lays it out, and starts, while the image is attached.
pub fn runtime_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".run");
    dir.with_file_name(name)
}

fn lock_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    dir.with_file_name(name)
}

/// `dir` with its parent resolved, which is where its image is mounted (the
/// mount point must be a real path: linux-run's path maps compare host
/// paths).
pub fn mount_point(dir: &Path) -> Result<PathBuf, String> {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return Err(format!("{}: not a data directory path", dir.display()));
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let parent = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    Ok(parent.join(name))
}

/// An attached data image; dropping it detaches it.
#[derive(Debug)]
pub struct DataImage {
    dir: PathBuf,
    image: PathBuf,
    device: Option<String>,
    _lock: File,
}

impl DataImage {
    /// Attaches the image of `dir` at `dir`, creating it first if needed.
    pub fn attach(dir: &Path) -> Result<Self, String> {
        let dir = mount_point(dir)?;
        let image = image_of(&dir);
        let ceiling = ceiling(&image)?;
        if !image.exists() {
            if fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some())
                && !disk::is_mount_point(&dir)
            {
                return Err(format!(
                    "{}: holds guest data from before data images (docs/storage.md); \
                     move it away or remove it",
                    dir.display()
                ));
            }
            disk::create_case_sensitive(&image, &ceiling.to_string(), VOLUME)?;
        }
        let lock_path = lock_of(&dir);
        let lock = File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;
        // SAFETY: flock on an open descriptor.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(format!("{}: in use by another guest-init", image.display()));
        }
        for stale in disk::attachments_of(&image)? {
            eprintln!(
                "aim-storage: {} was left attached ({}); detaching it",
                image.display(),
                stale.device
            );
            disk::detach(&stale.device, PATIENCE)?;
        }
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let how = Attach {
            mount: Some(&dir),
            ..Default::default()
        };
        let mut device = match disk::attach(&image, how) {
            Ok(device) => device,
            Err(first) => {
                // A volume a crash left inconsistent: repair it, then retry.
                repair(&image).map_err(|e| format!("{first}; repair: {e}"))?;
                disk::attach(&image, how)?
            }
        };
        // Smaller than the host volume (made with an older ceiling, or
        // moved to a larger disk): grown while detached.
        if disk::capacity(&dir).is_some_and(|size| size < ceiling) {
            disk::detach(&device, PATIENCE)?;
            disk::resize(&image, ceiling)?;
            device = disk::attach(&image, how)?;
        }
        Ok(Self {
            dir,
            image,
            device: Some(device),
            _lock: lock,
        })
    }

    /// The mount point: the data directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn image(&self) -> &Path {
        &self.image
    }

    /// Detaches the image, compacting it first when it holds more than
    /// [`RECLAIM_THRESHOLD`] of freed space.
    pub fn detach(mut self) -> Result<(), String> {
        self.detach_now()
    }

    fn detach_now(&mut self) -> Result<(), String> {
        let Some(device) = self.device.take() else {
            return Ok(());
        };
        if let Ok(Usage {
            allocated,
            used: Some(used),
        }) = usage(&self.dir)
            && allocated > used + RECLAIM_THRESHOLD
            && let Some(size) = disk::capacity(&self.dir)
            && let Err(e) = release_to_trim(&self.dir, size / TRIM_BATCH_DIVISOR)
        {
            eprintln!("aim-storage: compacting {}: {e}", self.image.display());
        }
        let patience = match sync_volume(&self.dir) {
            Ok(()) => SYNCED_PATIENCE,
            Err(e) => {
                eprintln!("aim-storage: syncing {}: {e}", self.dir.display());
                PATIENCE
            }
        };
        disk::detach(&device, patience)
    }
}

impl Drop for DataImage {
    fn drop(&mut self) {
        if let Err(e) = self.detach_now() {
            eprintln!("aim-storage: detach {}: {e}", self.image.display());
        }
    }
}

/// Allocates `batch` bytes in the volume at `mount` (without writing
/// them), frees them and commits that, so that APFS TRIMs all free space
/// when it unmounts. The file is unlinked while open: a preallocation
/// released at close counts as nothing freed. Without the full sync the
/// TRIM waits for the next mount.
fn release_to_trim(mount: &Path, batch: u64) -> Result<(), String> {
    let path = mount.join(".aim-trim");
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // As much as the volume has, if it has less.
    let mut store = libc::fstore_t {
        fst_flags: 0,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: batch as i64,
        fst_bytesalloc: 0,
    };
    // SAFETY: an open descriptor and a local fstore_t.
    let status = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    let error = std::io::Error::last_os_error();
    let removed = fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()));
    drop(file);
    removed?;
    if status != 0 {
        return Err(format!("F_PREALLOCATE: {error}"));
    }
    let root = File::open(mount).map_err(|e| format!("{}: {e}", mount.display()))?;
    // SAFETY: an open descriptor.
    if unsafe { libc::fcntl(root.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
        return Err(format!("F_FULLFSYNC: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Writes everything of the volume at `mount` to its disk and waits for
/// that (`sync_volume_np(3)`, a full sync).
fn sync_volume(mount: &Path) -> Result<(), String> {
    const SYNC_VOLUME_FULLSYNC: libc::c_int = 0x01;
    const SYNC_VOLUME_WAIT: libc::c_int = 0x02;
    unsafe extern "C" {
        fn sync_volume_np(path: *const libc::c_char, flags: libc::c_int) -> libc::c_int;
    }
    let path =
        std::ffi::CString::new(mount.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: a NUL-terminated path; libSystem's function.
    let status = unsafe { sync_volume_np(path.as_ptr(), SYNC_VOLUME_FULLSYNC | SYNC_VOLUME_WAIT) };
    // It returns an errno value, not -1.
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status).to_string());
    }
    Ok(())
}

/// Checks and repairs the APFS volumes of `image`, attached without a
/// mount.
fn repair(image: &Path) -> Result<(), String> {
    let device = disk::attach(image, Attach::default())?;
    let devices = disk::attachments_of(image)?
        .into_iter()
        .find(|a| a.device == device)
        .map(|a| a.devices)
        .unwrap_or_default();
    let mut result = Ok(());
    for volume in devices
        .iter()
        .filter(|d| d.trim_start_matches("/dev/disk").contains('s'))
    {
        let status = std::process::Command::new("fsck_apfs")
            .args(["-y", volume])
            .status()
            .map_err(|e| format!("fsck_apfs: {e}"));
        if !status.as_ref().is_ok_and(|s| s.success()) {
            result = Err(format!("fsck_apfs -y {volume} failed"));
        }
    }
    disk::detach(&device, PATIENCE)?;
    result
}

/// Removes the data directory `dir`, its image and its runtime directory,
/// detaching the image first. Nothing of it may be in use.
pub fn remove(dir: &Path) -> Result<(), String> {
    let dir = mount_point(dir)?;
    let image = image_of(&dir);
    if image.exists() {
        for attached in disk::attachments_of(&image)? {
            disk::detach(&attached.device, PATIENCE)?;
        }
        fs::remove_file(&image).map_err(|e| format!("{}: {e}", image.display()))?;
    }
    let _ = fs::remove_file(lock_of(&dir));
    let _ = fs::remove_dir_all(runtime_of(&dir));
    // Empty once detached; a plain directory from before data images
    // holds the data itself.
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// Space of a data image.
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    /// What its file occupies on the host.
    pub allocated: u64,
    /// What the guest's files occupy, while it is attached.
    pub used: Option<u64>,
}

pub fn usage(dir: &Path) -> Result<Usage, String> {
    let dir = mount_point(dir)?;
    let image = image_of(&dir);
    let allocated = disk::allocated(&image).map_err(|e| format!("{}: {e}", image.display()))?;
    let used = disk::is_mount_point(&dir)
        .then(|| disk::used(&dir))
        .flatten();
    Ok(Usage { allocated, used })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_sits_beside_its_directory() {
        assert_eq!(
            image_of(Path::new("/t/boot/data")),
            PathBuf::from("/t/boot/data.asif")
        );
        assert_eq!(
            runtime_of(Path::new("/t/boot/data")),
            PathBuf::from("/t/boot/data.run")
        );
    }

    #[test]
    fn syncs_a_volume_by_any_path_on_it() {
        sync_volume(&std::env::temp_dir()).unwrap();
        assert!(sync_volume(Path::new("/nonexistent/aim")).is_err());
    }
}
