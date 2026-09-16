//! Stable identity for a trusted, immutable runtime bundle.
//!
//! This is an identity/fingerprint check, not a complete TOCTOU-proof
//! executable-launch protocol.  Callers must still retain their trusted
//! bundle ownership and launch contract after this function returns.

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const HASH_DOMAIN: &[u8] = b"darwin-art-runtime-identity-v1";
const STREAM_BUFFER_SIZE: usize = 64 * 1024;

/// Inventory non-system Mach-O dependencies of the selected host and runtime
/// providers before fingerprinting. Callers must supply every explicit dlopen
/// root and non-Mach-O boot/data/provider input selected for the system process.
/// This does not reproduce dyld environment injection or make a mutable bundle
/// into an immutable launch snapshot. Unsupported load formats fail closed.
pub fn fingerprint_runtime(
    image: &crate::system_image::PreparedSystemImage,
    executable: &Path,
    native_roots: &[std::path::PathBuf],
    files: &[(&str, &Path)],
    directories: &[(&str, &Path)],
) -> io::Result<[u8; 32]> {
    if native_roots.is_empty() {
        return Err(invalid_input("runtime native roots are required"));
    }
    let native = crate::runtime_native_inventory::collect(executable, native_roots)?;
    let roles: Vec<_> = (0..native.len())
        .map(|index| format!("native-dependency-{index}"))
        .collect();
    let mut all_files: Vec<_> = roles
        .iter()
        .zip(&native)
        .map(|(role, path)| (role.as_str(), path.as_path()))
        .collect();
    all_files.extend_from_slice(files);
    let mut all_directories = vec![("prepared-system-image", image.root.as_path())];
    all_directories.extend_from_slice(directories);
    fingerprint(image.content_id, &all_files, &all_directories)
}

/// Hash the immutable runtime identity in caller-supplied order.
///
/// Files contribute their role, canonical host path and content digest.
/// Directories contribute their role, canonical host path and the directory's
/// device/inode identity.  Directory contents are deliberately not scanned or
/// copied.  File descriptors are held while files are read, then dropped before
/// this function returns.
pub fn fingerprint(
    system_image_id: [u8; 32],
    files: &[(&str, &Path)],
    directories: &[(&str, &Path)],
) -> io::Result<[u8; 32]> {
    let mut roles = HashSet::with_capacity(files.len().saturating_add(directories.len()));
    for (role, _) in files.iter().chain(directories.iter()) {
        if role.is_empty() {
            return Err(invalid_input("runtime identity role must not be empty"));
        }
        if !roles.insert(*role) {
            return Err(invalid_input("runtime identity roles must be unique"));
        }
    }
    for (_, path) in files.iter().chain(directories.iter()) {
        if !path.is_absolute() {
            return Err(invalid_input("runtime identity paths must be absolute"));
        }
    }

    let mut hash = Sha256::new();
    field(&mut hash, HASH_DOMAIN);
    field(&mut hash, &system_image_id);
    field(&mut hash, &(files.len() as u64).to_le_bytes());
    for (role, path) in files {
        let (canonical_path, digest) = file_identity(path)?;
        field(&mut hash, b"file");
        field(&mut hash, role.as_bytes());
        field(&mut hash, canonical_path.as_os_str().as_bytes());
        field(&mut hash, &digest);
    }
    field(&mut hash, &(directories.len() as u64).to_le_bytes());
    for (role, path) in directories {
        let (canonical_path, device, inode) = directory_identity(path)?;
        field(&mut hash, b"directory");
        field(&mut hash, role.as_bytes());
        field(&mut hash, canonical_path.as_os_str().as_bytes());
        field(&mut hash, &device.to_le_bytes());
        field(&mut hash, &inode.to_le_bytes());
    }
    Ok(hash.finalize().into())
}

fn file_identity(path: &Path) -> io::Result<(std::path::PathBuf, [u8; 32])> {
    // Reject symlink and special leaves before opening; in particular, opening
    // a FIFO for reading could otherwise wait for an unrelated writer.  The
    // no-follow FD open below remains authoritative against a replacement
    // racing this advisory preflight.
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid_input("runtime identity file is not a regular file"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(invalid_input("runtime identity file is not a regular file"));
    }

    let canonical = fs::canonicalize(path)?;
    let canonical_before = fs::metadata(&canonical)?;
    if !same_node(&before, &canonical_before) {
        return Err(invalid_input(
            "runtime identity file path changed before reading",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; STREAM_BUFFER_SIZE];
    let mut read_bytes = 0_u64;
    loop {
        let count = (&file).read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        read_bytes = read_bytes
            .checked_add(count as u64)
            .ok_or_else(|| invalid_input("runtime identity file is too large"))?;
    }
    let after = file.metadata()?;
    let canonical_after = fs::metadata(&canonical)?;
    if !same_metadata(&before, &after)
        || !same_node(&after, &canonical_after)
        || read_bytes != after.len()
    {
        return Err(invalid_input("runtime identity file changed while reading"));
    }
    Ok((canonical, hasher.finalize().into()))
}

fn directory_identity(path: &Path) -> io::Result<(std::path::PathBuf, u64, u64)> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() {
        return Err(invalid_input(
            "runtime identity directory is not a directory",
        ));
    }
    let canonical = fs::canonicalize(path)?;
    let canonical_metadata = fs::metadata(&canonical)?;
    if !same_node(&metadata, &canonical_metadata) {
        return Err(invalid_input(
            "runtime identity directory path changed while opening",
        ));
    }
    Ok((canonical, metadata.dev(), metadata.ino()))
}

fn same_node(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}

fn same_metadata(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.is_file()
        && after.is_file()
        && before.len() == after.len()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "darwin-art-runtime-identity-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            Self { root }
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }

        fn directory(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir(&path).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn fingerprint_is_deterministic_and_preserves_identical_file_content_identity() {
        let fixture = Fixture::new();
        let first = fixture.file("first", b"same bytes");
        let second = fixture.file("second", b"same bytes");
        let directory = fixture.directory("bundle");
        let a = fingerprint([7; 32], &[("runtime", &first)], &[("root", &directory)]).unwrap();
        let b = fingerprint([7; 32], &[("runtime", &first)], &[("root", &directory)]).unwrap();
        assert_eq!(a, b);

        // The file digest is content-based; replacing the file with identical
        // bytes changes only transient metadata, not the final file digest.
        fs::remove_file(&first).unwrap();
        let replacement = fixture.file("first", b"same bytes");
        assert_eq!(
            fingerprint(
                [7; 32],
                &[("runtime", &replacement)],
                &[("root", &directory)]
            )
            .unwrap(),
            a
        );
        assert_ne!(
            fingerprint([7; 32], &[("runtime", &second)], &[("root", &directory)]).unwrap(),
            a,
            "canonical path remains part of file identity"
        );
    }

    #[test]
    fn system_id_role_and_caller_order_change_identity() {
        let fixture = Fixture::new();
        let first = fixture.file("first", b"one");
        let second = fixture.file("second", b"two");
        let base = fingerprint([1; 32], &[("one", &first), ("two", &second)], &[]).unwrap();
        assert_ne!(
            base,
            fingerprint([2; 32], &[("one", &first), ("two", &second)], &[]).unwrap()
        );
        assert_ne!(
            base,
            fingerprint([1; 32], &[("two", &second), ("one", &first)], &[]).unwrap()
        );
        assert_ne!(
            base,
            fingerprint([1; 32], &[("renamed", &first), ("two", &second)], &[]).unwrap()
        );
    }

    #[test]
    fn directory_identity_includes_canonical_path_and_dev_inode() {
        let fixture = Fixture::new();
        let first = fixture.directory("first");
        let initial = fingerprint([3; 32], &[], &[("root", &first)]).unwrap();
        let renamed = fixture.root.join("renamed");
        fs::rename(&first, &renamed).unwrap();
        assert_ne!(
            initial,
            fingerprint([3; 32], &[], &[("root", &renamed)]).unwrap()
        );
        fs::remove_dir(&renamed).unwrap();
        let recreated = fixture.directory("first");
        assert_ne!(
            initial,
            fingerprint([3; 32], &[], &[("root", &recreated)]).unwrap()
        );
    }

    #[test]
    fn rejects_roles_paths_symlinks_and_special_files() {
        let fixture = Fixture::new();
        let file = fixture.file("file", b"bytes");
        let directory = fixture.directory("directory");
        assert!(fingerprint([0; 32], &[("", &file)], &[]).is_err());
        assert!(fingerprint([0; 32], &[("same", &file)], &[("same", &directory)]).is_err());
        assert!(fingerprint([0; 32], &[("relative", Path::new("file"))], &[]).is_err());

        let symlink = fixture.root.join("symlink");
        std::os::unix::fs::symlink(&file, &symlink).unwrap();
        assert!(fingerprint([0; 32], &[("symlink", &symlink)], &[]).is_err());
        let directory_link = fixture.root.join("directory-link");
        std::os::unix::fs::symlink(&directory, &directory_link).unwrap();
        assert!(fingerprint([0; 32], &[], &[("directory-link", &directory_link)]).is_err());

        let fifo = fixture.root.join("fifo");
        let fifo_bytes = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_bytes.as_ptr(), 0o600) }, 0);
        assert!(fingerprint([0; 32], &[("fifo", &fifo)], &[]).is_err());
    }

    #[test]
    fn file_content_change_changes_identity() {
        let fixture = Fixture::new();
        let file = fixture.file("file", b"before");
        let first = fingerprint([4; 32], &[("file", &file)], &[]).unwrap();
        let mut output = File::create(&file).unwrap();
        output.write_all(b"after").unwrap();
        output.sync_all().unwrap();
        assert_ne!(
            first,
            fingerprint([4; 32], &[("file", &file)], &[]).unwrap()
        );
    }

    #[test]
    fn file_mode_does_not_make_regular_file_special() {
        let fixture = Fixture::new();
        let file = fixture.file("file", b"bytes");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(fingerprint([5; 32], &[("file", &file)], &[]).is_ok());
    }
}
