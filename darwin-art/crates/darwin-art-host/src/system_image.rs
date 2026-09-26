//! Shared immutable system-image installation owned by the embedded runtime.
//! No profile/APK policy and no automatic replacement of existing installations.
use sha2::{Digest, Sha256};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REQUIRED: &[&str] = &[
    "apex/apex-info-list.xml",
    "linkerconfig/ld.config.txt",
    "linkerconfig/apex.libraries.config.txt",
    "system/etc/public.libraries.txt",
    "system/etc/linker.config.pb",
    "apex/com.android.art/apex_manifest.pb",
    "apex/com.android.runtime/apex_manifest.pb",
    "apex/com.android.conscrypt/apex_manifest.pb",
    "apex/com.android.i18n/apex_manifest.pb",
    "apex/com.android.tzdata/apex_manifest.pb",
    "system/etc/fonts.xml",
    "system/etc/font_fallback.xml",
    "system/fonts/Roboto-Regular.ttf",
    "system/framework/framework-res.apk",
];

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

fn validate(root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(invalid("system image root is not a real directory"));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        // `/system_ext` is the device's link to the system_ext partition.
        if entry.file_name() == "system_ext" {
            if !entry.file_type()?.is_symlink()
                || fs::read_link(entry.path())? != Path::new("system/system_ext")
            {
                return Err(invalid(
                    "system image system_ext entry is not the partition link",
                ));
            }
            continue;
        }
        // `product` carries the product partition's framework overlays and
        // `vendor` the device's feature declarations.
        if !["apex", "system", "linkerconfig", "product", "vendor"]
            .iter()
            .any(|name| entry.file_name() == *name)
            || !entry.file_type()?.is_dir()
        {
            return Err(invalid("system image contains an unexpected root entry"));
        }
    }
    for name in REQUIRED {
        let metadata = fs::symlink_metadata(root.join(name)).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("required system-image file {name}: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(invalid("system image lacks a required regular file"));
        }
    }
    Ok(())
}

fn seal(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            seal(&entry.path())?;
        } else if kind.is_file() {
            let mode = entry.metadata()?.permissions().mode() & !0o222;
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode))?;
        } else if !kind.is_symlink() {
            return Err(invalid("unexpected special file in system image"));
        }
    }
    let mode = fs::symlink_metadata(path)?.permissions().mode() & !0o222;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

// Only called for a newly created private staging tree. Never follows links.
fn writable_directories(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let _ = fs::set_permissions(
        path,
        fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
    );
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            writable_directories(&entry.path());
        }
    }
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        writable_directories(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Install a trusted bundled archive once by content identity. Cross-process
/// locking and rename publish a complete tree; old versions stay untouched.
pub fn prepare(archive: &Path, store: &Path) -> io::Result<PathBuf> {
    Ok(prepare_with_identity(archive, store)?.root)
}

/// Identity is computed from the opened archive, never parsed from a receipt
/// or supplied by the launching application. Keep it with the installed root.
pub struct PreparedSystemImage {
    pub root: PathBuf,
    pub content_id: [u8; 32],
}

pub fn prepare_with_identity(archive: &Path, store: &Path) -> io::Result<PreparedSystemImage> {
    if !archive.is_absolute() || !store.is_absolute() || store.parent().is_none() {
        return Err(invalid(
            "system image archive/store must be explicit absolute paths",
        ));
    }
    if store
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid(
            "system image store must not contain parent traversal",
        ));
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(archive)?;
    if !source.metadata()?.is_file() {
        return Err(invalid("system image archive is not a regular file"));
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let count = source.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let digest = hash.finalize();
    let content_id: [u8; 32] = digest.into();
    let identity = format!("{digest:x}");
    source.seek(SeekFrom::Start(0))?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(store)?;
    let store_metadata = fs::symlink_metadata(store)?;
    if !store_metadata.is_dir() {
        return Err(invalid("system image store is not a directory"));
    }
    // SAFETY: geteuid has no pointer arguments or ownership side effects.
    if store_metadata.uid() != unsafe { libc::geteuid() } || store_metadata.mode() & 0o022 != 0 {
        return Err(invalid(
            "system image store must be owned by this user and not publicly writable",
        ));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(store.join("install.lock"))?;
    // SAFETY: owned lock FD remains live through extraction/publication.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let installed = store.join(&identity);
    if installed.try_exists()? {
        if !fs::symlink_metadata(&installed)?.is_dir()
            || fs::read_to_string(installed.join("receipt"))? != identity
        {
            return Err(invalid(
                "existing system image installation has an invalid receipt",
            ));
        }
        validate(&installed.join("root"))?;
        return Ok(PreparedSystemImage {
            root: installed.join("root"),
            content_id,
        });
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid("system clock precedes epoch"))?
        .as_nanos();
    let stage_path = store.join(format!(".install-{}-{nonce}", std::process::id()));
    DirBuilder::new().mode(0o700).create(&stage_path)?;
    let stage = Staging(stage_path);
    let root = stage.0.join("root");
    fs::create_dir(&root)?;
    // Feed the same opened file that was hashed, not a pathname reopened by tar.
    // bsdtar's normal secure extraction rejects traversal and symlink writes;
    // never use -P/--insecure. Android absolute link *targets* remain unchanged.
    let output = darwin_art_profile::spawn_owned(
        Command::new("/usr/bin/tar")
            .args(["-xf", "-", "--no-same-owner", "-C"])
            .arg(&root)
            .stdin(Stdio::from(source))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?
    .wait_with_output()?;
    if !output.status.success() {
        return Err(invalid(&format!(
            "system image extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    validate(&root)?;
    let mut receipt = File::create(stage.0.join("receipt"))?;
    receipt.write_all(identity.as_bytes())?;
    receipt.sync_all()?;
    seal(&stage.0)?;
    fs::rename(&stage.0, &installed)?;
    Ok(PreparedSystemImage {
        root: installed.join("root"),
        content_id,
    })
}

#[cfg(test)]
#[path = "system_image_tests.rs"]
mod tests;
