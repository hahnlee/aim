//! Read-only images: the system image (the pinned original as a read-only
//! case-sensitive APFS image with its translation cache) and the derived
//! image over it (the overlay, written into a shadow file). The volume
//! layout is `root/` (the guest's root), `root.identity` and `translated/`
//! (docs/storage.md).
//!
//! Such an image stays attached once attached: tests and boots of every
//! checkout read it concurrently, and attaching costs about a second.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::disk::{self, Attach, Attached};

/// Process-owned image use, shared by guests and exclusive for builders.
/// The lock is beside the base image so all shadows share its ownership.
pub struct ImageLease(File);
impl ImageLease {
    pub fn read(image: &Path) -> Result<Self, String> { Self::acquire(image, false) }
    /// Resolve a mounted guest root to its real backing base image. Plain
    /// extracted test directories have no disk-image ownership to acquire.
    pub fn read_root(root: &Path) -> Result<Option<Self>, String> {
        let root = fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let Some(image) = backing_image(&root, &disk::attached()?)? else { return Ok(None); };
        let lease = Self::read(&image)?;
        // A builder may have detached between inventory and acquiring the lock.
        if backing_image(&root, &disk::attached()?)?.as_ref() != Some(&image) {
            return Err(format!("{}: image attachment changed while acquiring ownership", root.display()));
        }
        Ok(Some(lease))
    }
    pub fn write(image: &Path) -> Result<Self, String> { Self::acquire(image, true) }
    fn acquire(image: &Path, write: bool) -> Result<Self, String> {
        let image = real(image);
        let mut name = image.file_name().unwrap_or_default().to_os_string();
        name.push(".use.lock");
        let path = image.with_file_name(name);
        let file = fs::OpenOptions::new().create(true).read(true).write(true).truncate(false)
            .open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let operation = if write { libc::LOCK_EX } else { libc::LOCK_SH };
        // SAFETY: flock on our open descriptor; no waiting or forced takeover.
        if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } != 0 {
            return Err(format!("{}: image is in use; stop its owner before rebuilding: {}", image.display(), std::io::Error::last_os_error()));
        }
        Ok(Self(file))
    }
}
fn backing_image(root: &Path, attached: &[Attached]) -> Result<Option<PathBuf>, String> {
    let deepest = attached.iter().flat_map(|a| a.mounts.iter().map(move |m| (a, m)))
        .filter(|(_, mount)| root.starts_with(mount))
        .map(|(_, mount)| mount.components().count()).max();
    let Some(depth) = deepest else { return Ok(None); };
    let mut image = None;
    for (a, mount) in attached.iter().flat_map(|a| a.mounts.iter().map(move |m| (a, m))) {
        if root.starts_with(mount) && mount.components().count() == depth {
            let candidate = real(&a.image);
            if image.as_ref().is_some_and(|old| old != &candidate) {
                return Err(format!("{}: ambiguous backing image attachment", root.display()));
            }
            image = Some(candidate);
        }
    }
    Ok(image)
}

impl Drop for ImageLease {
    fn drop(&mut self) {
        // SAFETY: unlock our own descriptor, also released by process death.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); }
    }
}

/// Uncompressed and read-only: a first read of a block is a plain read,
/// where a compressed image (lzfse, `ULFO`, half the size) decompresses
/// it (docs/storage.md, "Measurements").
pub const FORMAT: &str = "UDRO";
/// The ceiling of the writable volume an image is built in.
const BUILD_SIZE: &str = "32g";
/// A freshly written volume is scanned by the Mac's security agent, which
/// keeps files open for a while.
const PATIENCE: Duration = Duration::from_secs(120);

/// `path` with its parent resolved, whether or not it exists.
fn real(path: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    match (path.parent().map(fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

fn attachments(image: &Path, shadow: Option<&Path>) -> Result<Vec<Attached>, String> {
    let shadow = shadow.map(real);
    Ok(disk::attachments_of(image)?
        .into_iter()
        .filter(|a| a.shadow == shadow)
        .collect())
}

/// Attaches `image` (with the writes of `shadow`) at `mount`, read-only
/// or writable, detaching an attachment of the same pair that differs.
pub fn attach(
    image: &Path,
    shadow: Option<&Path>,
    mount: &Path,
    writable: bool,
) -> Result<(), String> {
    let wanted = fs::canonicalize(mount).ok();
    for a in attachments(image, shadow)? {
        if a.writable == writable && wanted.as_ref().is_some_and(|m| a.mounts.contains(m)) {
            return Ok(());
        }
        disk::detach(&a.device, PATIENCE)?;
    }
    if let Some(parent) = mount.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    disk::attach(
        image,
        Attach {
            read_only: !writable,
            shadow,
            mount: Some(mount),
        },
    )
    .map(|_| ())
}

/// Detaches every attachment of `image` with `shadow`.
pub fn detach(image: &Path, shadow: Option<&Path>) -> Result<(), String> {
    if !image.exists() {
        return Ok(());
    }
    for a in attachments(image, shadow)? {
        disk::detach(&a.device, PATIENCE)?;
    }
    Ok(())
}

/// Detaches every attachment of `image`, whatever its shadow: before the
/// image file is replaced.
pub fn detach_all(image: &Path) -> Result<(), String> {
    if !image.exists() {
        return Ok(());
    }
    for a in disk::attachments_of(image)? {
        disk::detach(&a.device, PATIENCE)?;
    }
    Ok(())
}

/// Builds the read-only image `out` in [`FORMAT`]: `fill` writes the
/// content into the mounted root of a fresh writable case-sensitive
/// volume, which is then converted. `out` appears complete or not at all.
pub fn build(
    out: &Path,
    volume: &str,
    fill: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let with = |suffix: &str| {
        let mut name = out.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        out.with_file_name(name)
    };
    let (staging, mount, converted) = (with(".tmp.asif"), with(".tmp"), with(".tmp.dmg"));
    for stale in [&staging, &converted] {
        if stale.exists() {
            detach(stale, None)?;
            fs::remove_file(stale).map_err(|e| format!("{}: {e}", stale.display()))?;
        }
    }
    disk::create_case_sensitive(&staging, BUILD_SIZE, volume)?;
    let result = (|| {
        attach(&staging, None, &mount, true)?;
        let filled = fill(&mount);
        // Unmounted with its content flushed, even when `fill` failed.
        detach(&staging, None)?;
        filled?;
        disk::convert(&staging, &converted, FORMAT)?;
        fs::rename(&converted, out).map_err(|e| format!("{}: {e}", out.display()))
    })();
    let _ = fs::remove_file(&staging);
    let _ = fs::remove_file(&converted);
    let _ = fs::remove_dir(&mount);
    result
}

/// Paths of an image volume mounted at `mount`.
pub fn root(mount: &Path) -> PathBuf {
    mount.join("root")
}

/// The translation cache of the volume mounted at `mount`.
pub fn translated(mount: &Path) -> PathBuf {
    mount.join("translated")
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    #[test]
    fn guest_root_resolves_base_image_even_with_a_shadow() {
        let dir = std::env::temp_dir().join("aim-root-resolution-fixture");
        let attachment = Attached { image: dir.join("base.dmg"), shadow: Some(dir.join("derived.shadow")),
            writable: false, device: "fixture".into(), devices: vec![], mounts: vec![dir.join("derived")] };
        assert_eq!(backing_image(&dir.join("derived/root"), &[attachment]).unwrap(), Some(dir.join("base.dmg")));
        assert_eq!(backing_image(&dir.join("plain/root"), &[]).unwrap(), None);
    }
    #[test]
    fn image_reader_refuses_rebuild_until_last_owner_releases() {
        let dir = std::env::temp_dir().join(format!("aim-image-lease-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("fixture.dmg");
        let first = ImageLease::read(&image).unwrap();
        let second = ImageLease::read(&image).unwrap();
        assert!(ImageLease::write(&image).is_err());
        drop(first);
        assert!(ImageLease::write(&image).is_err());
        drop(second);
        let writer = ImageLease::write(&image).unwrap();
        assert!(ImageLease::read(&image).is_err());
        drop(writer);
        fs::remove_dir_all(dir).unwrap();
    }
}
