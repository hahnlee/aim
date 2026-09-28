//! A filesystem-agnostic tree walk and its host materialization.
//!
//! The materializer only creates new entries (`create_new`, `create_dir`,
//! `symlink`) under directories it created itself, so Android symlink targets
//! are never followed on the host. Host permission bits get the original
//! `mode & 0o777`; the complete original mode and uid/gid are kept on files
//! and directories in aim's `dev.aim.android-inode` attribute.
//!
//! An inode's original SELinux label (`security.selinux`, bytes exactly as
//! stored, usually NUL-terminated) is kept on files, directories and symlinks
//! in the host attribute `dev.aim.xattr.security.selinux`, the name the
//! syscall layer maps the guest's `security.selinux` to. Labels are written
//! before the host mode is made read-only, and once per hard-linked inode.
use crate::inode_metadata::{AndroidInodeMetadata, write_new};
use crate::{Result, invalid};
use std::collections::HashMap;
use std::ffi::{CString, OsStr, c_char, c_int, c_void};
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, BufWriter, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

/// Host attribute carrying the guest's `security.selinux`.
pub const LABEL_XATTR: &str = "dev.aim.xattr.security.selinux";
const LABEL_XATTR_C: &[u8] = b"dev.aim.xattr.security.selinux\0";
const XATTR_NOFOLLOW: c_int = 1;
const XATTR_CREATE: c_int = 2;
unsafe extern "C" {
    fn fsetxattr(
        fd: c_int,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
}

/// Label an open file or directory.
fn label_fd(file: &File, label: &[u8]) -> io::Result<()> {
    // SAFETY: borrowed descriptor, NUL-terminated name and an exact readable
    // span; CREATE refuses to overwrite an existing label.
    let status = unsafe {
        fsetxattr(
            file.as_raw_fd(),
            LABEL_XATTR_C.as_ptr().cast(),
            label.as_ptr().cast(),
            label.len(),
            0,
            XATTR_CREATE,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Label a symlink itself, never its target.
fn label_link(path: &Path, label: &[u8]) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: NUL-terminated path and name and an exact readable span;
    // NOFOLLOW addresses the link, CREATE refuses to overwrite.
    let status = unsafe {
        setxattr(
            path.as_ptr(),
            LABEL_XATTR_C.as_ptr().cast(),
            label.as_ptr().cast(),
            label.len(),
            0,
            XATTR_NOFOLLOW | XATTR_CREATE,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub nlink: u32,
}

impl Node {
    pub fn kind(&self) -> u32 {
        self.mode & S_IFMT
    }
}

/// A read-only filesystem addressed by opaque node ids.
pub trait Tree {
    fn root(&self) -> u64;
    fn node(&self, id: u64) -> Result<Node>;
    /// Directory entries other than `.` and `..`.
    fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>>;
    fn read_link(&self, id: u64) -> Result<Vec<u8>>;
    fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64>;
    /// The inode's `security.selinux` value as stored, if it has one.
    fn label(&self, _id: u64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

/// Read a whole regular file into memory, bounded.
pub fn read_file(tree: &dyn Tree, id: u64, limit: u64) -> Result<Vec<u8>> {
    let node = tree.node(id)?;
    if node.kind() != S_IFREG || node.size > limit {
        return Err(invalid("not a regular file within the size limit"));
    }
    let mut out = Vec::with_capacity(node.size as usize);
    tree.copy_file(id, &mut out)?;
    Ok(out)
}

/// Resolve an absolute path without following symlinks.
pub fn lookup(tree: &dyn Tree, path: &str) -> Result<Option<u64>> {
    let mut id = tree.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if tree.node(id)?.kind() != S_IFDIR {
            return Ok(None);
        }
        match tree
            .children(id)?
            .into_iter()
            .find(|(name, _)| name == part.as_bytes())
        {
            Some((_, child)) => id = child,
            None => return Ok(None),
        }
    }
    Ok(Some(id))
}

#[derive(Default, Debug, Clone)]
pub struct Report {
    pub directories: u64,
    pub files: u64,
    pub hardlinks: u64,
    pub symlinks: u64,
    pub bytes: u64,
    /// Device nodes, FIFOs and sockets, which are not created on the host.
    pub special: Vec<(PathBuf, u32)>,
}

impl Report {
    pub fn merge(&mut self, other: Report) {
        self.directories += other.directories;
        self.files += other.files;
        self.hardlinks += other.hardlinks;
        self.symlinks += other.symlinks;
        self.bytes += other.bytes;
        self.special.extend(other.special);
    }
}

/// Writes trees into host directories; directory modes are applied by
/// `finish`, after every tree (including nested mounts) has been written.
#[derive(Default)]
pub struct Materializer {
    directories: Vec<(PathBuf, Node, Option<Vec<u8>>)>,
    current: Report,
    total: Report,
}

fn metadata(node: &Node) -> AndroidInodeMetadata {
    AndroidInodeMetadata {
        uid: node.uid,
        gid: node.gid,
        mode: node.mode,
    }
}

fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

impl Materializer {
    /// Write `tree` into `destination`, which must be absent or an empty
    /// real directory (a mount point created by an enclosing tree).
    /// Returns this tree's counts; `finish` returns the totals.
    pub fn extract(&mut self, tree: &dyn Tree, destination: &Path) -> Result<Report> {
        self.current = Report::default();
        let root = tree.root();
        let node = tree.node(root)?;
        if node.kind() != S_IFDIR {
            return Err(invalid("filesystem root is not a directory"));
        }
        match fs::symlink_metadata(destination) {
            Ok(existing) => {
                if !existing.is_dir() || fs::read_dir(destination)?.next().is_some() {
                    return Err(invalid(format!(
                        "{} is not an empty directory mount point",
                        destination.display()
                    )));
                }
                // The mounted root's metadata replaces the mount point's.
                self.directories.retain(|(path, _, _)| path != destination);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                DirBuilder::new().mode(0o700).create(destination)?;
            }
            Err(error) => return Err(error.into()),
        }
        self.directories
            .push((destination.to_owned(), node, tree.label(root)?));
        self.current.directories += 1;
        let mut links = HashMap::new();
        self.walk(tree, root, destination, &mut links, 0)?;
        let unit = std::mem::take(&mut self.current);
        self.total.merge(unit.clone());
        Ok(unit)
    }

    fn walk(
        &mut self,
        tree: &dyn Tree,
        id: u64,
        directory: &Path,
        links: &mut HashMap<u64, PathBuf>,
        depth: usize,
    ) -> Result<()> {
        if depth > 128 {
            return Err(invalid("directory nesting is too deep"));
        }
        let mut children = tree.children(id)?;
        children.sort();
        for (name, child) in children {
            if !valid_name(&name) {
                return Err(invalid(format!("invalid entry name {name:?}")));
            }
            let path = directory.join(OsStr::from_bytes(&name));
            let node = tree.node(child)?;
            let created = match node.kind() {
                S_IFDIR => {
                    let label = tree.label(child)?;
                    DirBuilder::new().mode(0o700).create(&path).map(|_| {
                        self.current.directories += 1;
                        self.directories.push((path.clone(), node, label));
                        true
                    })
                }
                S_IFREG => self.file(tree, child, node, &path, links),
                S_IFLNK => {
                    let target = tree.read_link(child)?;
                    if target.is_empty() || target.contains(&0) {
                        return Err(invalid(format!("invalid symlink {}", path.display())));
                    }
                    let label = tree.label(child)?;
                    symlink(OsStr::from_bytes(&target), &path).and_then(|_| {
                        if let Some(label) = &label {
                            label_link(&path, label)?;
                        }
                        self.current.symlinks += 1;
                        Ok(false)
                    })
                }
                _ => {
                    self.current.special.push((path, node.mode));
                    continue;
                }
            };
            match created {
                Ok(true) => self.walk(tree, child, &path, links, depth + 1)?,
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    // Android's filesystems are case-sensitive: on a host
                    // filesystem that is not, a name differing only in case
                    // collides, and the image would lose a file.
                    return Err(invalid(format!(
                        "{} collides with an existing name; extract onto a \
                         case-sensitive filesystem (docs/storage.md)",
                        path.display()
                    )));
                }
                Err(error) => {
                    return Err(invalid(format!("create {}: {error}", path.display())));
                }
            }
        }
        Ok(())
    }

    fn file(
        &mut self,
        tree: &dyn Tree,
        id: u64,
        node: Node,
        path: &Path,
        links: &mut HashMap<u64, PathBuf>,
    ) -> io::Result<bool> {
        if let Some(first) = links.get(&id).filter(|_| node.nlink > 1) {
            fs::hard_link(first, path)?;
            self.current.hardlinks += 1;
            return Ok(false);
        }
        let label = tree
            .label(id)
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let written = tree
            .copy_file(id, &mut out)
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
        if written != node.size {
            return Err(io::Error::other(format!(
                "{}: size mismatch",
                path.display()
            )));
        }
        let file = out.into_inner().map_err(|error| error.into_error())?;
        write_new(&file, metadata(&node))?;
        if let Some(label) = &label {
            label_fd(&file, label)?;
        }
        file.set_permissions(Permissions::from_mode(node.mode & 0o777))?;
        if node.nlink > 1 {
            links.insert(id, path.to_owned());
        }
        self.current.files += 1;
        self.current.bytes += written;
        Ok(false)
    }

    /// Apply directory metadata deepest-first and return the report.
    pub fn finish(mut self) -> Result<Report> {
        self.directories
            .sort_by_key(|(path, _, _)| std::cmp::Reverse(path.components().count()));
        for (path, node, label) in &self.directories {
            let directory = File::open(path)?;
            write_new(&directory, metadata(node))?;
            if let Some(label) = label {
                label_fd(&directory, label)?;
            }
            directory.set_permissions(Permissions::from_mode(node.mode & 0o777))?;
        }
        Ok(self.total)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "android-image-extract-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    type Children = Vec<(Vec<u8>, u64)>;

    /// In-memory tree: id -> (node, children, payload).
    pub(crate) struct Memory(pub Vec<(Node, Children, Vec<u8>)>);

    impl Tree for Memory {
        fn root(&self) -> u64 {
            0
        }
        fn node(&self, id: u64) -> Result<Node> {
            Ok(self.0[id as usize].0)
        }
        fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>> {
            Ok(self.0[id as usize].1.clone())
        }
        fn read_link(&self, id: u64) -> Result<Vec<u8>> {
            Ok(self.0[id as usize].2.clone())
        }
        fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64> {
            out.write_all(&self.0[id as usize].2)?;
            Ok(self.0[id as usize].2.len() as u64)
        }
    }

    fn node(mode: u32, size: usize, nlink: u32) -> Node {
        Node {
            mode,
            uid: 0,
            gid: 2000,
            size: size as u64,
            nlink,
        }
    }

    #[test]
    fn materializes_links_modes_and_skips_specials() {
        let tree = Memory(vec![
            (
                node(S_IFDIR | 0o755, 0, 2),
                vec![
                    (b"bin".to_vec(), 1),
                    (b"sh".to_vec(), 3),
                    (b"null".to_vec(), 4),
                    (b"mnt".to_vec(), 5),
                ],
                vec![],
            ),
            (
                node(S_IFDIR | 0o751, 0, 2),
                vec![(b"toybox".to_vec(), 2), (b"ls".to_vec(), 2)],
                vec![],
            ),
            (node(S_IFREG | 0o4755, 3, 2), vec![], b"elf".to_vec()),
            (
                node(S_IFLNK | 0o777, 0, 1),
                vec![],
                b"/system/bin/sh".to_vec(),
            ),
            (node(0o020666, 0, 1), vec![], vec![]),
            (node(S_IFDIR | 0o700, 0, 2), vec![], vec![]),
        ]);
        let out = temp_dir("tree");
        let mut materializer = Materializer::default();
        materializer.extract(&tree, &out).unwrap();
        // A nested tree lands in the empty mount point.
        let nested = Memory(vec![(node(S_IFDIR | 0o555, 0, 2), vec![], vec![])]);
        materializer.extract(&nested, &out.join("mnt")).unwrap();
        assert!(materializer.extract(&nested, &out.join("bin")).is_err());
        let report = materializer.finish().unwrap();
        assert_eq!((report.files, report.hardlinks, report.symlinks), (1, 1, 1));
        assert_eq!(report.special.len(), 1);
        assert_eq!(fs::read(out.join("bin/ls")).unwrap(), b"elf");
        assert_eq!(
            fs::read_link(out.join("sh")).unwrap(),
            Path::new("/system/bin/sh")
        );
        let mode = |p: &str| {
            fs::symlink_metadata(out.join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(
            (mode("bin"), mode("bin/toybox"), mode("mnt")),
            (0o751, 0o755, 0o555)
        );
        let original = crate::inode_metadata::read(&File::open(out.join("bin/toybox")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!((original.mode, original.gid), (S_IFREG | 0o4755, 2000));
        fs::set_permissions(out.join("mnt"), Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(&out).unwrap();
    }

    /// `Memory` whose every node `id` is labelled `u:object_r:t<id>:s0\0`,
    /// except ids listed as unlabelled.
    struct Labelled(Memory, Vec<u64>);

    impl Tree for Labelled {
        fn root(&self) -> u64 {
            0
        }
        fn node(&self, id: u64) -> Result<Node> {
            self.0.node(id)
        }
        fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>> {
            self.0.children(id)
        }
        fn read_link(&self, id: u64) -> Result<Vec<u8>> {
            self.0.read_link(id)
        }
        fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64> {
            self.0.copy_file(id, out)
        }
        fn label(&self, id: u64) -> Result<Option<Vec<u8>>> {
            Ok((!self.1.contains(&id)).then(|| format!("u:object_r:t{id}:s0\0").into_bytes()))
        }
    }

    /// Reads the host label attribute without following symlinks.
    pub(crate) fn host_label(path: &Path) -> Option<Vec<u8>> {
        unsafe extern "C" {
            fn getxattr(
                path: *const c_char,
                name: *const c_char,
                value: *mut c_void,
                size: usize,
                position: u32,
                options: c_int,
            ) -> isize;
        }
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut value = vec![0u8; 256];
        // SAFETY: NUL-terminated strings and a writable span of value.len().
        let length = unsafe {
            getxattr(
                path.as_ptr(),
                LABEL_XATTR_C.as_ptr().cast(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                XATTR_NOFOLLOW,
            )
        };
        (length >= 0).then(|| {
            value.truncate(length as usize);
            value
        })
    }

    #[test]
    fn writes_labels_before_read_only_modes() {
        let tree = Labelled(
            Memory(vec![
                (
                    node(S_IFDIR | 0o555, 0, 2),
                    vec![
                        (b"bin".to_vec(), 1),
                        (b"sh".to_vec(), 3),
                        (b"plain".to_vec(), 4),
                    ],
                    vec![],
                ),
                (
                    node(S_IFDIR | 0o555, 0, 2),
                    vec![(b"toybox".to_vec(), 2), (b"ls".to_vec(), 2)],
                    vec![],
                ),
                (node(S_IFREG | 0o444, 3, 2), vec![], b"elf".to_vec()),
                (node(S_IFLNK | 0o777, 0, 1), vec![], b"/nowhere".to_vec()),
                (node(S_IFREG | 0o444, 1, 1), vec![], b"x".to_vec()),
            ]),
            vec![4],
        );
        let out = temp_dir("labels");
        let mut materializer = Materializer::default();
        materializer.extract(&tree, &out).unwrap();
        materializer.finish().unwrap();
        let label = |p: &str| host_label(&out.join(p));
        assert_eq!(label(""), Some(b"u:object_r:t0:s0\0".to_vec()));
        assert_eq!(label("bin"), Some(b"u:object_r:t1:s0\0".to_vec()));
        assert_eq!(label("bin/ls"), Some(b"u:object_r:t2:s0\0".to_vec()));
        assert_eq!(label("bin/toybox"), label("bin/ls"));
        assert_eq!(label("sh"), Some(b"u:object_r:t3:s0\0".to_vec()));
        assert_eq!(label("plain"), None);
        // The inode attribute is still written alongside the label.
        let original = crate::inode_metadata::read(&File::open(out.join("bin/ls")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(original.mode, S_IFREG | 0o444);
        for dir in ["", "bin"] {
            fs::set_permissions(out.join(dir), Permissions::from_mode(0o700)).unwrap();
        }
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn a_name_that_collides_on_the_host_fails_the_extraction() {
        // Two entries of one name stand for `Ringtone.ogg` and `ringtone.ogg`
        // on a case-insensitive host.
        let tree = Memory(vec![
            (
                node(S_IFDIR | 0o755, 0, 2),
                vec![(b"a.ogg".to_vec(), 1), (b"a.ogg".to_vec(), 1)],
                vec![],
            ),
            (node(S_IFREG | 0o644, 1, 1), vec![], b"x".to_vec()),
        ]);
        let out = temp_dir("collision");
        let error = Materializer::default()
            .extract(&tree, &out)
            .unwrap_err()
            .to_string();
        assert!(error.contains("case-sensitive"), "{error}");
        fs::remove_dir_all(&out).unwrap();
    }
}
