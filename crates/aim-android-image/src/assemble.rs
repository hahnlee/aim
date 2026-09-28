//! Builds the derived tree.
//!
//! Strategy: `clonefile(2)` the whole original tree, apply the overlay to the
//! clone, make it read-only, then publish it with an exclusive rename.
//!
//! - A clone of an APFS directory hierarchy shares every file's data blocks
//!   copy-on-write, so a multi-GB original costs only metadata, and one
//!   syscall creates it.
//! - The clone has its own inodes, so the overlay and the permission changes
//!   never reach the original. Hard links would share inodes and a `chmod`
//!   would rewrite the original's modes; symlinks into the original would
//!   make the derived tree depend on paths outside it.
//! - Clones require the original and OUTDIR to be on the same APFS volume.
//!   There is deliberately no copying fallback: a silent multi-GB copy is the
//!   cost this design exists to avoid.
//!
//! - The original is resolved first: `clonefile(2)` does not follow a
//!   symlink, so cloning a link to the original would give a link, and the
//!   overlay would be applied to the original through it.
//!
//! The tree is built in a hidden sibling of OUTDIR (same volume, so the
//! final rename is atomic) and published with `renamex_np(RENAME_EXCL)`, so
//! a concurrent build of the same identity cannot be clobbered and readers
//! never see a partial tree.

use crate::identity::{Identity, read_tree_identity};
use crate::manifest::Kind;
use crate::plan::{IDENTITY_FILE, Plan, RECEIPT_FILE, hash_file};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DIR_MODE: u32 = 0o555;
const FILE_MODE: u32 = 0o444;
const EXEC_FILE_MODE: u32 = 0o555;
const BUILD_DIR_MODE: u32 = 0o755;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// OUTDIR already held this identity; nothing was touched.
    Reused,
    Built,
}

/// Builds the derived image of `plan` over `original` at `out`, or reuses an
/// existing `out` with the same identity.
pub fn assemble(
    plan: &Plan,
    original: &Path,
    identity: &Identity,
    out: &Path,
) -> Result<Outcome, String> {
    if let Some(outcome) = existing(out, identity)? {
        return Ok(outcome);
    }
    let original =
        &fs::canonicalize(original).map_err(|error| format!("{}: {error}", original.display()))?;
    let name = out
        .file_name()
        .ok_or_else(|| format!("{}: OUTDIR needs a final path component", out.display()))?;
    let parent = match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if resolve(&parent.join(name)).starts_with(original) {
        return Err(format!(
            "{}: OUTDIR is the original {} or inside it",
            out.display(),
            original.display()
        ));
    }
    fs::create_dir_all(&parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let staging = parent.join(format!(
        ".{}.tmp-{}-{nanos}",
        name.to_string_lossy(),
        std::process::id()
    ));

    let built = build(plan, original, identity, &staging).and_then(|()| publish(&staging, out));
    match built {
        Ok(()) => Ok(Outcome::Built),
        Err(error) => {
            let _ = force_remove(&staging);
            // Lost a race with a concurrent build of the same identity.
            if let Ok(Some(outcome)) = existing(out, identity) {
                return Ok(outcome);
            }
            Err(error)
        }
    }
}

/// `path` with its longest existing prefix resolved (symlinks, `..`).
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut prefix = path;
    loop {
        if let Ok(real) = fs::canonicalize(prefix) {
            return rest.iter().rev().fold(real, |p, name| p.join(name));
        }
        match (prefix.parent(), prefix.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                prefix = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// `Some(Reused)` if `out` already holds `identity`, `None` if it does not
/// exist, and an error if it holds anything else.
fn existing(out: &Path, identity: &Identity) -> Result<Option<Outcome>, String> {
    match fs::symlink_metadata(out) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", out.display())),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!("{} exists and is not a directory", out.display()));
        }
        Ok(_) => {}
    }
    match read_tree_identity(out)? {
        Some(found) if found == identity.hex => Ok(Some(Outcome::Reused)),
        Some(found) => Err(format!(
            "{} holds derived image {found}, not {}; images are never rebuilt in place, \
             choose another OUTDIR or remove it",
            out.display(),
            identity.hex
        )),
        None => Err(format!(
            "{} exists but has no {IDENTITY_FILE}; refusing to overwrite it",
            out.display()
        )),
    }
}

fn build(plan: &Plan, original: &Path, identity: &Identity, staging: &Path) -> Result<(), String> {
    clone_tree(original, staging)?;
    // Everything below writes through `staging`; it must be the clone.
    match fs::symlink_metadata(staging) {
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            return Err(format!(
                "{}: the clone is not a directory",
                staging.display()
            ));
        }
    }
    let context = |path: &Path| {
        let path = path.to_path_buf();
        move |error: io::Error| format!("{}: {error}", path.display())
    };
    set_mode(staging, BUILD_DIR_MODE).map_err(context(staging))?;

    for step in &plan.steps {
        let target = staging.join(step.relative());
        let parent = target.parent().expect("guest paths have a parent");
        make_ancestors_writable(staging, parent)?;
        match step.kind {
            Kind::Remove => {
                if fs::symlink_metadata(&target)
                    .map_err(context(&target))?
                    .is_dir()
                {
                    force_remove(&target).map_err(context(&target))?;
                } else {
                    fs::remove_file(&target).map_err(context(&target))?;
                }
            }
            Kind::Replace | Kind::Add => {
                if step.kind == Kind::Replace {
                    fs::remove_file(&target).map_err(context(&target))?;
                } else {
                    fs::create_dir_all(parent).map_err(context(parent))?;
                }
                let source = step.source.as_ref().expect("add and replace have a source");
                fs::copy(&source.resolved, &target).map_err(context(&source.resolved))?;
                // The identity covers the hash taken at validation; refuse a
                // source that changed since.
                let (sha256, _) = hash_file(&target).map_err(context(&target))?;
                if sha256 != source.sha256 {
                    return Err(format!(
                        "source `{}` changed during assembly (sha256 {} became {sha256})",
                        source.declared, source.sha256
                    ));
                }
            }
        }
    }

    for name in [IDENTITY_FILE, RECEIPT_FILE] {
        let path = staging.join(name);
        match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(format!("{}: {error}", path.display()));
            }
            _ => {}
        }
    }
    fs::write(staging.join(RECEIPT_FILE), &identity.receipt).map_err(context(staging))?;
    fs::write(staging.join(IDENTITY_FILE), format!("{}\n", identity.hex))
        .map_err(context(staging))?;
    seal(staging)
}

/// Clones `original` to the absent path `staging`.
#[cfg(target_os = "macos")]
fn clone_tree(original: &Path, staging: &Path) -> Result<(), String> {
    // <sys/clonefile.h>
    const CLONE_NOFOLLOW: u32 = 0x0001;
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let source = c_path(original)?;
    let destination = c_path(staging)?;
    // SAFETY: both arguments are NUL-terminated paths that outlive the call.
    let status = unsafe {
        libc::clonefile(
            source.as_ptr(),
            destination.as_ptr(),
            CLONE_NOFOLLOW | CLONE_NOOWNERCOPY,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    let hint = match error.raw_os_error() {
        Some(libc::EXDEV) | Some(libc::ENOTSUP) => {
            "; the original and OUTDIR must be on the same APFS volume"
        }
        _ => "",
    };
    Err(format!(
        "clonefile {} -> {}: {error}{hint}",
        original.display(),
        staging.display()
    ))
}

#[cfg(not(target_os = "macos"))]
fn clone_tree(_original: &Path, _staging: &Path) -> Result<(), String> {
    Err("android-image assembles with APFS clonefile(2), which needs macOS".into())
}

#[cfg(target_os = "macos")]
fn publish(staging: &Path, out: &Path) -> Result<(), String> {
    let from = c_path(staging)?;
    let to = c_path(out)?;
    // SAFETY: both arguments are NUL-terminated paths that outlive the call.
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "publish {} -> {}: {}",
            staging.display(),
            out.display(),
            io::Error::last_os_error()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn publish(_staging: &Path, _out: &Path) -> Result<(), String> {
    Err("android-image publishes with renamex_np(2), which needs macOS".into())
}

fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{}: path contains NUL", path.display()))
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Makes `directory` and its existing ancestors below `root` writable. A
/// read-only original clones read-only.
fn make_ancestors_writable(root: &Path, directory: &Path) -> Result<(), String> {
    let relative = directory
        .strip_prefix(root)
        .expect("inside the staging tree");
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {
                set_mode(&current, BUILD_DIR_MODE)
                    .map_err(|error| format!("{}: {error}", current.display()))?;
            }
            _ => break,
        }
    }
    Ok(())
}

/// Makes the finished tree read-only: directories 0555, files 0444, or 0555
/// when the file had any execute bit. Setuid, setgid and sticky bits are
/// dropped. Symlinks are left alone.
fn seal(root: &Path) -> Result<(), String> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("{}: {error}", directory.display()))?
                .path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if metadata.is_dir() {
                directories.push(path);
            } else if !metadata.file_type().is_symlink() {
                let mode = if metadata.permissions().mode() & 0o111 != 0 {
                    EXEC_FILE_MODE
                } else {
                    FILE_MODE
                };
                set_mode(&path, mode).map_err(|error| format!("{}: {error}", path.display()))?;
            }
        }
    }
    // Children first, so a directory is never sealed before its contents.
    for directory in directories.iter().rev() {
        set_mode(directory, DIR_MODE)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
    }
    Ok(())
}

/// Removes a tree, making its directories writable first.
pub fn force_remove(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    };
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        set_mode(&directory, BUILD_DIR_MODE)?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    fs::remove_dir_all(path)
}
