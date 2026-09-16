#![forbid(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(not(target_os = "macos"))]
compile_error!("darwin-art-fs-broker requires macOS; no weaker fallback is implemented");

use std::error::Error;
use std::ffi::{CString, c_char, c_int};
use std::fmt;
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};

pub mod guest_path;
pub mod inode_metadata;

// Values from Darwin sys/fcntl.h. Keeping the small FFI surface local avoids a
// dependency and makes the exact authorization flags reviewable.
const O_RDONLY: c_int = 0x0000_0000;
const O_NONBLOCK: c_int = 0x0000_0004;
const O_NOFOLLOW: c_int = 0x0000_0100;
const O_DIRECTORY: c_int = 0x0010_0000;
const O_CLOEXEC: c_int = 0x0100_0000;

unsafe extern "C" {
    fn openat(fd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
}

/// A path rejection made before any filesystem lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathViolation {
    Absolute,
    ContainsNul,
    CurrentDirectory,
    ParentDirectory,
    EmptyComponent,
}

/// A failure from the read-only authorization gate.
#[derive(Debug)]
pub enum BrokerError {
    InvalidPath(PathViolation),
    NotDirectoryRoot,
    UnsupportedNodeType,
    Io {
        operation: &'static str,
        component_index: Option<usize>,
        source: io::Error,
    },
}

impl BrokerError {
    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Io { source, .. } => source.raw_os_error(),
            _ => None,
        }
    }
}

impl fmt::Display for BrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(reason) => write!(formatter, "invalid broker path: {reason:?}"),
            Self::NotDirectoryRoot => write!(formatter, "mount-root descriptor is not a directory"),
            Self::UnsupportedNodeType => {
                write!(
                    formatter,
                    "broker only admits regular files and directories"
                )
            }
            Self::Io {
                operation,
                component_index,
                source,
            } => match component_index {
                Some(index) => write!(
                    formatter,
                    "{operation} failed at path component {index}: {source}"
                ),
                None => write!(formatter, "{operation} failed: {source}"),
            },
        }
    }
}

impl Error for BrokerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A successfully authorized node. The metadata and readable descriptor refer
/// to the same opened object, even if its pathname is renamed afterwards.
#[derive(Debug)]
pub struct OpenedNode {
    file: File,
    metadata: Metadata,
}

impl OpenedNode {
    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn into_file(self) -> File {
        self.file
    }
}

/// A read-only component-walking broker rooted at an already-open directory.
///
/// The owned root descriptor is the authorization boundary. All guest paths
/// are relative to it and symlinks are unconditionally unsupported.
#[derive(Debug)]
pub struct ReadOnlyBroker {
    root: File,
}

impl ReadOnlyBroker {
    /// Takes ownership of a mount-root descriptor after verifying the object is
    /// a directory. How that descriptor is selected is intentionally outside
    /// this path resolver's authority.
    pub fn from_directory(root: File) -> Result<Self, BrokerError> {
        let metadata = root.metadata().map_err(|source| BrokerError::Io {
            operation: "fstat mount root",
            component_index: None,
            source,
        })?;
        if !metadata.is_dir() {
            return Err(BrokerError::NotDirectoryRoot);
        }
        Ok(Self { root })
    }

    /// Securely opens a regular file or directory using a guest-relative byte
    /// path. An empty path opens the mount root as a fresh read-only descriptor.
    pub fn open(&self, path: &[u8]) -> Result<OpenedNode, BrokerError> {
        self.open_with_observer(path, |_, _| Ok(()))
    }

    /// Returns metadata from the securely opened descriptor. There is no
    /// pathname-based stat after authorization.
    pub fn stat(&self, path: &[u8]) -> Result<Metadata, BrokerError> {
        Ok(self.open(path)?.metadata)
    }

    fn open_with_observer<F>(&self, path: &[u8], mut observer: F) -> Result<OpenedNode, BrokerError>
    where
        F: FnMut(WalkEvent, &File) -> io::Result<()>,
    {
        let parsed = ParsedPath::new(path)?;
        let mut directory = self.root.try_clone().map_err(|source| BrokerError::Io {
            operation: "duplicate mount root",
            component_index: None,
            source,
        })?;

        if parsed.components.is_empty() {
            let root = open_component(
                &directory,
                b".",
                O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                None,
                "open mount root",
            )?;
            observer(WalkEvent::LeafOpened, &root).map_err(|source| BrokerError::Io {
                operation: "walk observer",
                component_index: None,
                source,
            })?;
            return finish_open(root);
        }

        let last_index = parsed.components.len() - 1;
        for (index, component) in parsed.components[..last_index].iter().enumerate() {
            directory = open_component(
                &directory,
                component,
                O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                Some(index),
                "open intermediate directory",
            )?;
            observer(WalkEvent::IntermediateOpened(index), &directory).map_err(|source| {
                BrokerError::Io {
                    operation: "walk observer",
                    component_index: Some(index),
                    source,
                }
            })?;
        }

        let mut flags = O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC;
        if parsed.requires_directory {
            flags |= O_DIRECTORY;
        }
        let leaf = open_component(
            &directory,
            parsed.components[last_index],
            flags,
            Some(last_index),
            "open leaf",
        )?;
        observer(WalkEvent::LeafOpened, &leaf).map_err(|source| BrokerError::Io {
            operation: "walk observer",
            component_index: Some(last_index),
            source,
        })?;
        finish_open(leaf)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WalkEvent {
    IntermediateOpened(usize),
    LeafOpened,
}

struct ParsedPath<'a> {
    components: Vec<&'a [u8]>,
    requires_directory: bool,
}

impl<'a> ParsedPath<'a> {
    fn new(path: &'a [u8]) -> Result<Self, BrokerError> {
        if path.contains(&0) {
            return Err(BrokerError::InvalidPath(PathViolation::ContainsNul));
        }
        if path.starts_with(b"/") {
            return Err(BrokerError::InvalidPath(PathViolation::Absolute));
        }
        if path.is_empty() {
            return Ok(Self {
                components: Vec::new(),
                requires_directory: true,
            });
        }

        let requires_directory = path.ends_with(b"/");
        let body = if requires_directory {
            &path[..path.len() - 1]
        } else {
            path
        };
        if body.is_empty() {
            return Err(BrokerError::InvalidPath(PathViolation::EmptyComponent));
        }

        let mut components = Vec::new();
        for component in body.split(|byte| *byte == b'/') {
            let violation = match component {
                b"" => Some(PathViolation::EmptyComponent),
                b"." => Some(PathViolation::CurrentDirectory),
                b".." => Some(PathViolation::ParentDirectory),
                _ => None,
            };
            if let Some(violation) = violation {
                return Err(BrokerError::InvalidPath(violation));
            }
            components.push(component);
        }
        Ok(Self {
            components,
            requires_directory,
        })
    }
}

fn open_component(
    directory: &File,
    component: &[u8],
    flags: c_int,
    component_index: Option<usize>,
    operation: &'static str,
) -> Result<File, BrokerError> {
    let component = CString::new(component)
        .map_err(|_| BrokerError::InvalidPath(PathViolation::ContainsNul))?;
    // SAFETY: `component` is NUL terminated and lives through the call;
    // `directory` owns a valid descriptor. No mode vararg is required because
    // the flags never contain O_CREAT.
    let fd = unsafe { openat(directory.as_raw_fd(), component.as_ptr(), flags) };
    if fd < 0 {
        return Err(BrokerError::Io {
            operation,
            component_index,
            source: io::Error::last_os_error(),
        });
    }
    // SAFETY: a successful openat returns a new descriptor owned by this call.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn finish_open(file: File) -> Result<OpenedNode, BrokerError> {
    let metadata = file.metadata().map_err(|source| BrokerError::Io {
        operation: "fstat opened node",
        component_index: None,
        source,
    })?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(BrokerError::UnsupportedNodeType);
    }
    Ok(OpenedNode { file, metadata })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            for _ in 0..100 {
                let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "darwin-art-fs-broker-{label}-{}-{serial}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create temporary directory: {error}"),
                }
            }
            panic!("could not allocate a unique temporary directory");
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn broker(&self) -> ReadOnlyBroker {
            ReadOnlyBroker::from_directory(File::open(&self.0).expect("open mount root"))
                .expect("construct broker")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read_node(mut node: OpenedNode) -> Vec<u8> {
        let mut bytes = Vec::new();
        node.file.read_to_end(&mut bytes).expect("read opened node");
        bytes
    }

    #[test]
    fn byte_path_is_not_forced_through_rust_utf8_strings() {
        let root = TempDir::new("bytes");
        let name = OsString::from_vec(b"payload-\xc3\xa9".to_vec());
        fs::write(root.path().join(&name), b"opaque-name").expect("write byte-name fixture");

        let broker = root.broker();
        let opened = broker.open(b"payload-\xc3\xa9").expect("open byte path");
        assert_eq!(read_node(opened), b"opaque-name");

        // APFS commonly rejects this byte at the VFS boundary with EILSEQ. It
        // must still reach openat as bytes rather than being rejected or
        // normalized by the broker as malformed Rust text.
        assert!(matches!(
            broker.open(b"invalid-\xff"),
            Err(BrokerError::Io { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_or_escaping_path_syntax() {
        let root = TempDir::new("syntax");
        let broker = root.broker();
        let cases = [
            (&b"/absolute"[..], PathViolation::Absolute),
            (&b"a\0b"[..], PathViolation::ContainsNul),
            (&b"."[..], PathViolation::CurrentDirectory),
            (&b"a/../b"[..], PathViolation::ParentDirectory),
            (&b"a//b"[..], PathViolation::EmptyComponent),
        ];
        for (path, expected) in cases {
            assert!(matches!(
                broker.open(path),
                Err(BrokerError::InvalidPath(actual)) if actual == expected
            ));
        }
    }

    #[test]
    fn empty_path_opens_mount_root_read_only() {
        let root = TempDir::new("mount-root");
        let broker = root.broker();
        let opened = broker.open(b"").expect("open mount root");
        assert!(opened.metadata().is_dir());
        assert!(opened.file().write_all(b"not writable").is_err());
    }

    #[test]
    fn trailing_separator_requires_a_directory() {
        let root = TempDir::new("trailing-dir");
        fs::write(root.path().join("file"), b"data").expect("write fixture");
        fs::create_dir(root.path().join("directory")).expect("create fixture directory");
        let broker = root.broker();

        assert!(
            broker
                .open(b"directory/")
                .expect("open directory")
                .metadata()
                .is_dir()
        );
        assert!(matches!(broker.open(b"file/"), Err(BrokerError::Io { .. })));
    }

    #[test]
    fn intermediate_and_leaf_symlinks_never_succeed() {
        let root = TempDir::new("symlink-root");
        let outside = TempDir::new("symlink-outside");
        fs::write(outside.path().join("secret"), b"outside").expect("write outside fixture");
        symlink(outside.path(), root.path().join("jump")).expect("create intermediate symlink");
        symlink(outside.path().join("secret"), root.path().join("leaf"))
            .expect("create leaf symlink");
        let broker = root.broker();

        assert!(matches!(
            broker.open(b"jump/secret"),
            Err(BrokerError::Io { .. })
        ));
        assert!(matches!(broker.open(b"leaf"), Err(BrokerError::Io { .. })));
    }

    #[test]
    fn rename_replacement_cannot_redirect_an_opened_component() {
        let root = TempDir::new("rename-root");
        let replacement = TempDir::new("rename-replacement");
        fs::create_dir(root.path().join("stable")).expect("create original directory");
        fs::write(root.path().join("stable/value"), b"original").expect("write original fixture");
        fs::write(replacement.path().join("value"), b"replacement")
            .expect("write replacement fixture");
        let broker = root.broker();
        let mut swapped = false;

        let opened = broker
            .open_with_observer(b"stable/value", |event, _| {
                if event == WalkEvent::IntermediateOpened(0) {
                    fs::rename(
                        root.path().join("stable"),
                        root.path().join("opened-object"),
                    )?;
                    fs::rename(replacement.path(), root.path().join("stable"))?;
                    swapped = true;
                }
                Ok(())
            })
            .expect("walk must stay on opened directory descriptor");

        assert!(swapped);
        assert_eq!(read_node(opened), b"original");
        assert_eq!(
            fs::read(root.path().join("stable/value")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn leaf_stat_and_read_survive_path_replacement() {
        let root = TempDir::new("leaf-rename");
        fs::write(root.path().join("value"), b"old").expect("write original fixture");
        let broker = root.broker();

        let opened = broker
            .open_with_observer(b"value", |event, _| {
                if event == WalkEvent::LeafOpened {
                    fs::rename(root.path().join("value"), root.path().join("old-value"))?;
                    fs::write(root.path().join("value"), b"much-longer-replacement")?;
                }
                Ok(())
            })
            .expect("opened leaf remains valid");

        assert_eq!(opened.metadata().len(), 3);
        assert_eq!(
            opened.metadata().ino(),
            fs::metadata(root.path().join("old-value")).unwrap().ino()
        );
        assert_eq!(read_node(opened), b"old");
    }

    #[test]
    fn stat_uses_the_authorized_descriptor() {
        let root = TempDir::new("stat");
        fs::write(root.path().join("payload"), b"12345").expect("write fixture");
        let metadata = root.broker().stat(b"payload").expect("secure stat");
        assert!(metadata.is_file());
        assert_eq!(metadata.len(), 5);
    }

    #[test]
    fn non_directory_mount_root_is_rejected() {
        let root = TempDir::new("bad-root");
        let path = root.path().join("file");
        fs::write(&path, b"not a directory").expect("write fixture");
        assert!(matches!(
            ReadOnlyBroker::from_directory(File::open(path).unwrap()),
            Err(BrokerError::NotDirectoryRoot)
        ));
    }
}
