//! Read-only images: the system image (the pinned original as a compressed
//! case-sensitive APFS image with its translation cache) and the derived
//! image over it (the overlay, written into a shadow file). The volume
//! layout is `root/` (the guest's root), `root.identity` and `translated/`
//! (docs/storage.md).
//!
//! Such an image stays attached once attached: tests and boots of every
//! checkout read it concurrently, and attaching costs about a second.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::disk::{self, Attach, Attached};

/// The compression: lzfse. It builds the 4.6 GB volume in about 4 s and
/// reads at about 550 MiB/s cold on one thread; zlib (UDZO) is as large
/// and slower, lzma (ULMO) 20 % smaller but reads at 55 MiB/s even when
/// cached (docs/storage.md, "Measurements").
pub const FORMAT: &str = "ULFO";
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

/// Builds the compressed image `out`: `fill` writes the content into the
/// mounted root of a fresh writable case-sensitive volume, which is then
/// compressed. `out` appears complete or not at all.
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
    let (staging, mount, compressed) = (with(".tmp.asif"), with(".tmp"), with(".tmp.dmg"));
    for stale in [&staging, &compressed] {
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
        disk::convert(&staging, &compressed, FORMAT)?;
        fs::rename(&compressed, out).map_err(|e| format!("{}: {e}", out.display()))
    })();
    let _ = fs::remove_file(&staging);
    let _ = fs::remove_file(&compressed);
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
