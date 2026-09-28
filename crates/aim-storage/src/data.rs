//! The guest's writable data: one case-sensitive APFS image per data
//! directory, `<data>.asif` beside it, mounted hidden at `<data>` while a
//! boot runs (docs/storage.md).
//!
//! - **Created on demand** as a sparse image (ASIF) of up to [`SIZE`]: it
//!   occupies only what the guest wrote.
//! - **Space returns:** APFS TRIMs freed blocks, and an ASIF image punches
//!   them out of its file. A sparse bundle keeps them (neither TRIM nor
//!   `hdiutil compact` frees its bands on macOS 26 and later), which is why
//!   this is not one. APFS only TRIMs once about 2 GiB have been freed, so
//!   at stop, when the file holds more than [`RECLAIM_THRESHOLD`] beyond
//!   what the volume uses, 2 GiB are allocated and freed again: the unmount
//!   then TRIMs everything free ("compaction").
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

/// The largest the guest's data can grow (the size `df /data` reports).
pub const SIZE: &str = "64g";
const VOLUME: &str = "aim-data";
/// Space the image's file may hold beyond the volume's use before a stop
/// compacts it.
pub const RECLAIM_THRESHOLD: u64 = 512 << 20;
/// What APFS frees before it TRIMs (measured on macOS 27: 1.75 GiB freed
/// stay in the file, 2 GiB are returned).
const TRIM_BATCH: i64 = 2 << 30;
/// How long a busy volume is waited for at detach.
const PATIENCE: Duration = Duration::from_secs(20);

/// The image of the data directory `dir`.
pub fn image_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".asif");
    dir.with_file_name(name)
}

fn lock_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    dir.with_file_name(name)
}

/// `dir` with its parent resolved (the mount point must be a real path:
/// linux-run's path maps compare host paths).
fn real(dir: &Path) -> Result<PathBuf, String> {
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
        let dir = real(dir)?;
        let image = image_of(&dir);
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
            disk::create_case_sensitive(&image, SIZE, VOLUME)?;
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
        let device = match disk::attach(&image, how) {
            Ok(device) => device,
            Err(first) => {
                // A volume a crash left inconsistent: repair it, then retry.
                repair(&image).map_err(|e| format!("{first}; repair: {e}"))?;
                disk::attach(&image, how)?
            }
        };
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
            && let Err(e) = release_to_trim(&self.dir)
        {
            eprintln!("aim-storage: compacting {}: {e}", self.image.display());
        }
        disk::detach(&device, PATIENCE)
    }
}

impl Drop for DataImage {
    fn drop(&mut self) {
        if let Err(e) = self.detach_now() {
            eprintln!("aim-storage: detach {}: {e}", self.image.display());
        }
    }
}

/// Allocates [`TRIM_BATCH`] in the volume at `mount` (without writing it)
/// and frees it, so that APFS TRIMs all free space when it unmounts.
fn release_to_trim(mount: &Path) -> Result<(), String> {
    let path = mount.join(".aim-trim");
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATEALL,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: TRIM_BATCH,
        fst_bytesalloc: 0,
    };
    // SAFETY: an open descriptor and a local fstore_t.
    let status = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    let error = std::io::Error::last_os_error();
    let _ = file.set_len(store.fst_bytesalloc as u64);
    let _ = file.sync_all();
    drop(file);
    fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if status != 0 {
        return Err(format!("F_PREALLOCATE: {error}"));
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
    for volume in devices.iter().filter(|d| d.contains('s')) {
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

/// Removes the data directory `dir` and its image, detaching it first.
/// Nothing of it may be in use.
pub fn remove(dir: &Path) -> Result<(), String> {
    let dir = real(dir)?;
    let image = image_of(&dir);
    if image.exists() {
        for attached in disk::attachments_of(&image)? {
            disk::detach(&attached.device, PATIENCE)?;
        }
        fs::remove_file(&image).map_err(|e| format!("{}: {e}", image.display()))?;
    }
    let _ = fs::remove_file(lock_of(&dir));
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
    let dir = real(dir)?;
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
    }
}
