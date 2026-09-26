//! Complete file-level overlay for the pinned ART runtime source tree.
//!
//! Unchanged upstream files are represented by symlinks so the overlay stays
//! small and tracks the pinned source directly. Files named by the runtime
//! patch manifest are copied first, making every patch target a real file.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use crate::Result;
use sha2::{Digest, Sha256};

use super::manifest::{PATCHED_RUNTIME_PATCHES, PATCHED_RUNTIME_SOURCES};

pub(crate) fn create_candidate(runtime: &Path, candidate: &Path) -> Result<()> {
    create_overlay(runtime, candidate, PATCHED_RUNTIME_SOURCES)
}

pub(crate) fn validate_patch_targets(root: &Path) -> Result<()> {
    let patched = PATCHED_RUNTIME_SOURCES
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    for patch in PATCHED_RUNTIME_PATCHES {
        let patch_path = root.join(patch);
        for line in fs::read_to_string(&patch_path)?.lines() {
            let Some(path) = line
                .strip_prefix("--- a/runtime/")
                .or_else(|| line.strip_prefix("+++ b/runtime/"))
            else {
                continue;
            };
            if path != "/dev/null" && !patched.contains(path) {
                return Err(format!(
                    "runtime patch target {path} is not in PATCHED_RUNTIME_SOURCES ({patch})"
                )
                .into());
            }
        }
    }
    Ok(())
}

pub(crate) fn is_complete(runtime: &Path, overlay: &Path) -> Result<bool> {
    let patched = PATCHED_RUNTIME_SOURCES
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    if !overlay.is_dir() {
        return Ok(false);
    }
    if !matches_tree(runtime, overlay, Path::new(""), &patched)? {
        return Ok(false);
    }
    for source in PATCHED_RUNTIME_SOURCES {
        let path = overlay.join(source);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Add the pinned source revision and complete relative-path inventory to the
/// shadow identity. This prevents a sparse-overlay cache hit after an upstream
/// source layout or content change.
pub(crate) fn update_identity(digest: &mut Sha256, runtime: &Path) -> Result<()> {
    let revision = fs::read_to_string(runtime.join(".source-revision"))?;
    digest.update(b"upstream-revision\0");
    digest.update(revision.trim().as_bytes());
    digest.update(b"\0source-inventory-v1\0");
    inventory(runtime, Path::new(""), digest)
}

pub(crate) fn publish(runtime: &Path, candidate: &Path, destination: &Path) -> Result<()> {
    if !is_complete(runtime, candidate)? {
        return Err(format!(
            "incomplete ART source overlay candidate: {}",
            candidate.display()
        )
        .into());
    }
    publish_entry(candidate, destination)
}

fn create_overlay(runtime: &Path, candidate: &Path, patched_sources: &[&str]) -> Result<()> {
    if !runtime.is_dir() {
        return Err(format!(
            "pinned ART runtime directory is missing: {}",
            runtime.display()
        )
        .into());
    }
    let patched = patched_sources.iter().copied().collect::<HashSet<_>>();
    for source in patched_sources {
        let path = runtime.join(source);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "patched ART source is not a regular file: {}",
                path.display()
            )
            .into());
        }
    }
    fs::create_dir_all(candidate)?;
    mirror_directory(runtime, candidate, Path::new(""), &patched)
}

fn mirror_directory(
    runtime: &Path,
    candidate: &Path,
    relative: &Path,
    patched: &HashSet<&str>,
) -> Result<()> {
    let source_dir = runtime.join(relative);
    let candidate_dir = candidate.join(relative);
    fs::create_dir_all(&candidate_dir)?;
    for entry in fs::read_dir(&source_dir)? {
        let entry = entry?;
        let source = entry.path();
        let child = relative.join(entry.file_name());
        let destination = candidate.join(&child);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            mirror_directory(runtime, candidate, &child, patched)?;
        } else if metadata.file_type().is_symlink() {
            if fs::metadata(&source)?.is_dir() {
                return Err(format!(
                    "directory symlink is not allowed in ART source overlay: {}",
                    source.display()
                )
                .into());
            }
            // Pin file symlinks to the immutable upstream entry itself. This
            // keeps a relative upstream target relative to the upstream
            // directory rather than accidentally rebasing it in the overlay.
            symlink(&source, destination)?;
        } else if metadata.is_file() {
            let child_string = child.to_string_lossy();
            if patched.contains(child_string.as_ref()) {
                fs::copy(&source, destination)?;
            } else {
                symlink(&source, destination)?;
            }
        } else {
            return Err(format!("unsupported ART source entry: {}", source.display()).into());
        }
    }
    Ok(())
}

fn matches_tree(
    runtime: &Path,
    overlay: &Path,
    relative: &Path,
    patched: &HashSet<&str>,
) -> Result<bool> {
    let source_dir = runtime.join(relative);
    let overlay_dir = overlay.join(relative);
    for entry in fs::read_dir(&source_dir)? {
        let entry = entry?;
        let source = entry.path();
        let child = relative.join(entry.file_name());
        let destination = overlay.join(&child);
        let source_metadata = fs::symlink_metadata(&source)?;
        let destination_metadata = match fs::symlink_metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if source_metadata.is_dir() {
            if !destination_metadata.is_dir()
                || destination_metadata.file_type().is_symlink()
                || !matches_tree(runtime, overlay, &child, patched)?
            {
                return Ok(false);
            }
        } else if source_metadata.is_file() {
            let child_string = child.to_string_lossy();
            if patched.contains(child_string.as_ref()) {
                if !destination_metadata.is_file() || destination_metadata.file_type().is_symlink()
                {
                    return Ok(false);
                }
            } else if !destination_metadata.file_type().is_symlink()
                || fs::read_link(&destination)? != source
            {
                return Ok(false);
            }
        } else if source_metadata.file_type().is_symlink() {
            if fs::metadata(&source)?.is_dir()
                || !destination_metadata.file_type().is_symlink()
                || fs::read_link(&destination)? != source
            {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    for entry in fs::read_dir(&overlay_dir)? {
        let entry = entry?;
        let source = source_dir.join(entry.file_name());
        if fs::symlink_metadata(source).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn publish_entry(candidate: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(candidate)?;
    if metadata.is_dir() {
        if let Ok(existing) = fs::symlink_metadata(destination)
            && (!existing.is_dir() || existing.file_type().is_symlink())
        {
            remove_entry(destination)?;
        }
        fs::create_dir_all(destination)?;
        let mut published_names = std::collections::HashSet::new();
        for entry in fs::read_dir(candidate)? {
            let entry = entry?;
            published_names.insert(entry.file_name());
            publish_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        let existing_entries = fs::read_dir(destination)?.collect::<std::io::Result<Vec<_>>>()?;
        for entry in existing_entries {
            if !published_names.contains(&entry.file_name()) {
                remove_entry(&entry.path())?;
            }
        }
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(candidate)?;
        let unchanged = fs::symlink_metadata(destination).is_ok_and(|existing| {
            existing.file_type().is_symlink()
                && fs::read_link(destination).is_ok_and(|current| current == target)
        });
        if !unchanged {
            if destination.exists() || fs::symlink_metadata(destination).is_ok() {
                remove_entry(destination)?;
            }
            symlink(target, destination)?;
        }
    } else {
        if fs::symlink_metadata(destination)
            .is_ok_and(|existing| existing.is_dir() || existing.file_type().is_symlink())
        {
            remove_entry(destination)?;
        }
        copy_if_changed(candidate, destination)?;
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn copy_if_changed(source: &Path, destination: &Path) -> Result<()> {
    let source_bytes = fs::read(source)?;
    if fs::symlink_metadata(destination)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        && fs::read(destination)? == source_bytes
    {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary =
        destination.with_extension(format!("darwin-art-overlay-tmp-{}", std::process::id()));
    fs::write(&temporary, source_bytes)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn inventory(runtime: &Path, relative: &Path, digest: &mut Sha256) -> Result<()> {
    let mut entries = fs::read_dir(runtime.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        let child = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        digest.update(child.to_string_lossy().as_bytes());
        if metadata.is_dir() {
            digest.update(b"/dir\0");
            inventory(runtime, &child, digest)?;
        } else if metadata.file_type().is_symlink() {
            if fs::metadata(&source)?.is_dir() {
                return Err(format!(
                    "directory symlink is not allowed in ART source inventory: {}",
                    source.display()
                )
                .into());
            }
            digest.update(b"/symlink\0");
            digest.update(fs::read_link(&source)?.to_string_lossy().as_bytes());
            digest.update([0]);
        } else if metadata.is_file() {
            digest.update(b"/file\0");
            digest.update(fs::read(&source)?);
            digest.update([0]);
        } else {
            return Err(format!("unsupported ART source entry: {}", source.display()).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            // Parallel tests can read the same clock value (#21).
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "art-overlay-{}-{nonce}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temp tree");
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn overlay_resolves_unpatched_siblings_and_real_patched_files() {
        let temp = TempTree::new();
        let upstream = temp.0.join("runtime");
        let candidate = temp.0.join("candidate");
        fs::create_dir_all(upstream.join("gc")).expect("gc");
        fs::write(upstream.join("gc/untouched.h"), b"upstream").expect("write");
        symlink(upstream.join("gc/untouched.h"), upstream.join("gc/alias.h"))
            .expect("file symlink");
        fs::write(upstream.join("patched.h"), b"patched").expect("write");
        create_overlay(&upstream, &candidate, &["patched.h"]).expect("overlay");

        assert!(
            fs::symlink_metadata(candidate.join("patched.h"))
                .expect("patched")
                .file_type()
                .is_file()
        );
        assert!(
            !fs::symlink_metadata(candidate.join("patched.h"))
                .expect("patched")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(candidate.join("gc/untouched.h")).expect("sibling"),
            upstream.join("gc/untouched.h")
        );
        assert_eq!(
            fs::read(candidate.join("gc/untouched.h")).expect("pristine"),
            b"upstream"
        );
        assert_eq!(
            fs::read_link(candidate.join("gc/alias.h")).expect("file symlink"),
            upstream.join("gc/alias.h")
        );
    }

    #[test]
    fn publish_preserves_unchanged_symlink_mtime_and_upstream_bytes() {
        let temp = TempTree::new();
        let upstream = temp.0.join("runtime");
        let first = temp.0.join("first");
        let second = temp.0.join("second");
        let destination = temp.0.join("published");
        fs::create_dir_all(&upstream).expect("runtime");
        fs::write(upstream.join("untouched.h"), b"stable").expect("write");
        fs::write(upstream.join("patched.h"), b"changed").expect("write");
        create_overlay(&upstream, &first, &["patched.h"]).expect("first");
        publish_entry(&first, &destination).expect("publish first");
        let before = fs::symlink_metadata(destination.join("untouched.h"))
            .expect("mtime")
            .mtime();
        create_overlay(&upstream, &second, &["patched.h"]).expect("second");
        publish_entry(&second, &destination).expect("publish second");
        fs::write(destination.join("stale.h"), b"stale").expect("stale");
        publish_entry(&second, &destination).expect("reconcile");
        assert_eq!(
            fs::symlink_metadata(destination.join("untouched.h"))
                .expect("mtime")
                .mtime(),
            before
        );
        assert_eq!(
            fs::read(destination.join("patched.h")).expect("bytes"),
            b"changed"
        );
        assert!(!destination.join("stale.h").exists());
    }

    #[test]
    fn directory_symlink_is_rejected() {
        let temp = TempTree::new();
        let upstream = temp.0.join("runtime");
        let candidate = temp.0.join("candidate");
        fs::create_dir_all(upstream.join("gc")).expect("gc");
        fs::write(upstream.join("patched.h"), b"patched").expect("write");
        symlink(upstream.join("gc"), upstream.join("gc-link")).expect("directory symlink");
        assert!(create_overlay(&upstream, &candidate, &["patched.h"]).is_err());
    }
}
