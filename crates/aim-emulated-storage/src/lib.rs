//! The emulated external storage's FUSE daemon, in MediaProvider's
//! FuseDaemon's place on the app host (docs/app-host.md): vold mounts the
//! FUSE connection at `/mnt/user/<user>/emulated` and hands it over; each
//! request is answered by the same operation on the volume's lower path
//! (`/data/media`), as the daemon's own (Linux FUSE 7.39, `<linux/fuse.h>`).
//!
//! Access is a user's to its own user directory, as FuseDaemon keeps each
//! user to its own; the root, system, media and shell uids reach every
//! user's. Apps' own `Android/data` and `Android/obb` directories are
//! vold's bind mounts over the FUSE tree, so their requests never come
//! here. Which app may read or write which shared file (MediaProvider's
//! ownership and its media database) is not modelled yet: an app reaches
//! all of its user's shared files (#1455).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `<linux/fuse.h>`'s opcodes.
pub mod op {
    pub const LOOKUP: u32 = 1;
    pub const FORGET: u32 = 2;
    pub const GETATTR: u32 = 3;
    pub const SETATTR: u32 = 4;
    pub const READLINK: u32 = 5;
    pub const SYMLINK: u32 = 6;
    pub const MKNOD: u32 = 8;
    pub const MKDIR: u32 = 9;
    pub const UNLINK: u32 = 10;
    pub const RMDIR: u32 = 11;
    pub const RENAME: u32 = 12;
    pub const LINK: u32 = 13;
    pub const OPEN: u32 = 14;
    pub const READ: u32 = 15;
    pub const WRITE: u32 = 16;
    pub const STATFS: u32 = 17;
    pub const RELEASE: u32 = 18;
    pub const FSYNC: u32 = 20;
    pub const FLUSH: u32 = 25;
    pub const INIT: u32 = 26;
    pub const OPENDIR: u32 = 27;
    pub const READDIR: u32 = 28;
    pub const RELEASEDIR: u32 = 29;
    pub const FSYNCDIR: u32 = 30;
    pub const ACCESS: u32 = 34;
    pub const CREATE: u32 = 35;
    pub const INTERRUPT: u32 = 36;
    pub const DESTROY: u32 = 38;
    pub const BATCH_FORGET: u32 = 42;
    pub const RENAME2: u32 = 45;
}

/// Linux's errno values, as replies carry them (negated).
mod errno {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const EIO: i32 = 5;
    pub const EACCES: i32 = 13;
    pub const EEXIST: i32 = 17;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENOSPC: i32 = 28;
    pub const ENOSYS: i32 = 38;
    pub const ENOTEMPTY: i32 = 39;
    pub const ENAMETOOLONG: i32 = 36;
}

/// `<asm-generic/fcntl.h>`'s open flags.
const O_ACCMODE: u32 = 3;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;
/// `FATTR_*`: what a `SETATTR` sets.
const FATTR_MODE: u32 = 1;
const FATTR_UID: u32 = 2;
const FATTR_GID: u32 = 4;
const FATTR_SIZE: u32 = 8;
const FATTR_ATIME: u32 = 16;
const FATTR_MTIME: u32 = 32;
const FATTR_FH: u32 = 64;
const FATTR_ATIME_NOW: u32 = 128;
const FATTR_MTIME_NOW: u32 = 256;
/// `RENAME_NOREPLACE`.
const RENAME_NOREPLACE: u32 = 1;
/// `FUSE_ASYNC_READ`, `FUSE_BIG_WRITES`: what `INIT` agrees to.
const INIT_FLAGS: u32 = 1 | 32;
/// The largest write it takes, and so the size of a request it reads.
pub const MAX_WRITE: u32 = 128 * 1024;
pub const BUFFER_SIZE: usize = MAX_WRITE as usize + 4096;
/// `AID_ROOT`, `AID_SYSTEM`, `AID_MEDIA_RW`, `AID_SHELL`;
/// `UserHandle.PER_USER_RANGE`.
const PRIVILEGED: [u32; 4] = [0, 1000, 1023, 2000];
const PER_USER_RANGE: u32 = 100_000;
/// A mode's type bits.
const S_IFMT: u32 = 0o170000;

/// A request's header (`fuse_in_header`).
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub opcode: u32,
    pub unique: u64,
    pub node: u64,
    pub uid: u32,
}

/// The daemon of one volume: its lower path, nodes and open files.
pub struct Daemon {
    lower: PathBuf,
    nodes: Mutex<Nodes>,
    handles: Mutex<Handles>,
}

/// The nodes the kernel holds: by id, their path below the lower root
/// and how many lookups it holds; the root is node 1, always.
struct Nodes {
    by_id: HashMap<u64, (PathBuf, u64)>,
    by_path: HashMap<PathBuf, u64>,
    next: u64,
}

enum Handle {
    File(File),
    /// A directory's entries as `OPENDIR` saw them: name, inode, type.
    Dir(Vec<(Vec<u8>, u64, u32)>),
}

struct Handles {
    open: HashMap<u64, Handle>,
    next: u64,
}

type Result<T> = std::result::Result<T, i32>;

impl Daemon {
    /// The daemon of the volume whose lower path is `lower`.
    pub fn new(lower: impl Into<PathBuf>) -> Daemon {
        let mut nodes = Nodes { by_id: HashMap::new(), by_path: HashMap::new(), next: 2 };
        nodes.by_id.insert(1, (PathBuf::new(), 1));
        nodes.by_path.insert(PathBuf::new(), 1);
        Daemon { lower: lower.into(), nodes: Mutex::new(nodes), handles: Mutex::new(Handles { open: HashMap::new(), next: 1 }) }
    }

    /// The reply to the request `message` (header and body), or `None` for
    /// a request that takes none.
    pub fn handle(&self, message: &[u8]) -> Option<Vec<u8>> {
        let Some(header) = header(message) else { return None };
        let body = &message[40..message.len().min(u32_at(message, 0)? as usize)];
        match header.opcode {
            op::FORGET => {
                self.forget(header.node, u64_at(body, 0).unwrap_or(0));
                return None;
            }
            op::BATCH_FORGET => {
                let count = u32_at(body, 0).unwrap_or(0) as usize;
                for i in 0..count {
                    if let (Some(node), Some(n)) = (u64_at(body, 8 + 16 * i), u64_at(body, 16 + 16 * i)) {
                        self.forget(node, n);
                    }
                }
                return None;
            }
            op::INTERRUPT => return None,
            _ => {}
        }
        let result = self.dispatch(&header, body);
        let mut reply = Vec::with_capacity(16 + result.as_ref().map_or(0, Vec::len));
        let (error, payload) = match result {
            Ok(payload) => (0, payload),
            Err(e) => (-e, Vec::new()),
        };
        reply.extend(((16 + payload.len()) as u32).to_le_bytes());
        reply.extend(error.to_le_bytes());
        reply.extend(header.unique.to_le_bytes());
        reply.extend(payload);
        Some(reply)
    }

    fn dispatch(&self, h: &Header, body: &[u8]) -> Result<Vec<u8>> {
        match h.opcode {
            op::INIT => Ok(init(body)),
            op::DESTROY => Ok(Vec::new()),
            op::LOOKUP => {
                let path = self.child(h, body)?;
                self.entry(&path)
            }
            op::GETATTR => {
                let path = self.path(h)?;
                Ok(attr_out(&fs::symlink_metadata(self.lower(&path)).map_err(error)?))
            }
            op::SETATTR => self.setattr(h, body),
            op::READLINK => {
                let path = self.path(h)?;
                Ok(fs::read_link(self.lower(&path)).map_err(error)?.as_os_str().as_bytes().to_vec())
            }
            op::SYMLINK => {
                let (name, target) = two_names(body)?;
                let path = self.named(h, name)?;
                std::os::unix::fs::symlink(OsStr::from_bytes(target), self.lower(&path)).map_err(error)?;
                self.entry(&path)
            }
            op::MKNOD => {
                // Regular files only, as FuseDaemon makes them.
                let mode = u32_at(body, 0).ok_or(errno::EINVAL)?;
                let umask = u32_at(body, 8).ok_or(errno::EINVAL)?;
                if mode & S_IFMT != 0o100000 {
                    return Err(errno::EPERM);
                }
                let path = self.named(h, name(&body[16..])?)?;
                OpenOptions::new().write(true).create_new(true).mode(mode & !umask & 0o7777).open(self.lower(&path)).map_err(error)?;
                self.entry(&path)
            }
            op::MKDIR => {
                let mode = u32_at(body, 0).ok_or(errno::EINVAL)?;
                let umask = u32_at(body, 4).ok_or(errno::EINVAL)?;
                let path = self.named(h, name(&body[8..])?)?;
                fs::DirBuilder::new().mode(mode & !umask & 0o7777).create(self.lower(&path)).map_err(error)?;
                self.entry(&path)
            }
            op::UNLINK | op::RMDIR => {
                let path = self.child(h, body)?;
                let lower = self.lower(&path);
                if h.opcode == op::UNLINK { fs::remove_file(lower) } else { fs::remove_dir(lower) }.map_err(error)?;
                Ok(Vec::new())
            }
            op::RENAME | op::RENAME2 => {
                let (new_dir, flags, names) = if h.opcode == op::RENAME {
                    (u64_at(body, 0).ok_or(errno::EINVAL)?, 0, &body[8..])
                } else {
                    (u64_at(body, 0).ok_or(errno::EINVAL)?, u32_at(body, 8).ok_or(errno::EINVAL)?, &body[16..])
                };
                if flags & !RENAME_NOREPLACE != 0 {
                    return Err(errno::EINVAL);
                }
                let (old, new) = two_names(names)?;
                let from = self.named(h, old)?;
                let to = self.named(&Header { node: new_dir, ..*h }, new)?;
                if flags & RENAME_NOREPLACE != 0 && fs::symlink_metadata(self.lower(&to)).is_ok() {
                    return Err(errno::EEXIST);
                }
                fs::rename(self.lower(&from), self.lower(&to)).map_err(error)?;
                self.renamed(&from, &to);
                Ok(Vec::new())
            }
            op::LINK => {
                let old = self.path(&Header { node: u64_at(body, 0).ok_or(errno::EINVAL)?, ..*h })?;
                let path = self.named(h, name(&body[8..])?)?;
                fs::hard_link(self.lower(&old), self.lower(&path)).map_err(error)?;
                self.entry(&path)
            }
            op::OPEN => {
                let path = self.path(h)?;
                let flags = u32_at(body, 0).ok_or(errno::EINVAL)?;
                let file = options(flags).open(self.lower(&path)).map_err(error)?;
                Ok(open_out(self.keep(Handle::File(file))))
            }
            op::CREATE => {
                let flags = u32_at(body, 0).ok_or(errno::EINVAL)?;
                let mode = u32_at(body, 4).ok_or(errno::EINVAL)?;
                let umask = u32_at(body, 8).ok_or(errno::EINVAL)?;
                let path = self.named(h, name(&body[16..])?)?;
                let file = options(flags | O_CREAT).mode(mode & !umask & 0o7777).open(self.lower(&path)).map_err(error)?;
                let mut out = self.entry(&path)?;
                out.extend(open_out(self.keep(Handle::File(file))));
                Ok(out)
            }
            op::READ => {
                let (fh, offset, size) = (u64_at(body, 0).ok_or(errno::EINVAL)?, u64_at(body, 8).ok_or(errno::EINVAL)?, u32_at(body, 16).ok_or(errno::EINVAL)?);
                let mut data = vec![0; size.min(MAX_WRITE) as usize];
                let n = self.with_file(fh, |f| read_at(f, &mut data, offset))?;
                data.truncate(n);
                Ok(data)
            }
            op::WRITE => {
                let (fh, offset, size) = (u64_at(body, 0).ok_or(errno::EINVAL)?, u64_at(body, 8).ok_or(errno::EINVAL)?, u32_at(body, 16).ok_or(errno::EINVAL)? as usize);
                let data = body.get(40..40 + size).ok_or(errno::EINVAL)?;
                self.with_file(fh, |f| f.write_all_at(data, offset).map_err(error))?;
                let mut out = (size as u32).to_le_bytes().to_vec();
                out.extend([0; 4]);
                Ok(out)
            }
            op::FLUSH => Ok(Vec::new()),
            op::FSYNC => {
                let fh = u64_at(body, 0).ok_or(errno::EINVAL)?;
                self.with_file(fh, |f| f.sync_all().map_err(error))?;
                Ok(Vec::new())
            }
            op::RELEASE | op::RELEASEDIR => {
                let fh = u64_at(body, 0).ok_or(errno::EINVAL)?;
                self.handles.lock().unwrap().open.remove(&fh);
                Ok(Vec::new())
            }
            op::OPENDIR => {
                let path = self.path(h)?;
                Ok(open_out(self.keep(Handle::Dir(self.entries(h, &path)?))))
            }
            op::READDIR => {
                let (fh, offset, size) = (u64_at(body, 0).ok_or(errno::EINVAL)?, u64_at(body, 8).ok_or(errno::EINVAL)?, u32_at(body, 16).ok_or(errno::EINVAL)? as usize);
                let handles = self.handles.lock().unwrap();
                let Some(Handle::Dir(entries)) = handles.open.get(&fh) else { return Err(errno::EINVAL) };
                Ok(dirents(entries, offset as usize, size))
            }
            op::FSYNCDIR => Ok(Vec::new()),
            op::STATFS => statfs(&self.lower),
            op::ACCESS => {
                self.path(h)?;
                Ok(Vec::new())
            }
            _ => Err(errno::ENOSYS),
        }
    }

    /// The lower path of `path` below the root.
    fn lower(&self, path: &Path) -> PathBuf {
        self.lower.join(path)
    }

    /// The path of the request's node, if its caller may reach it.
    fn path(&self, h: &Header) -> Result<PathBuf> {
        let path = self.nodes.lock().unwrap().by_id.get(&h.node).map(|(p, _)| p.clone()).ok_or(errno::ENOENT)?;
        permit(h.uid, &path)?;
        Ok(path)
    }

    /// The path of the child `name` of the request's node.
    fn named(&self, h: &Header, name: &[u8]) -> Result<PathBuf> {
        if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
            return Err(errno::EINVAL);
        }
        if name.len() > 255 {
            return Err(errno::ENAMETOOLONG);
        }
        let path = self.path(h)?.join(OsStr::from_bytes(name));
        permit(h.uid, &path)?;
        Ok(path)
    }

    /// The child a request's body names.
    fn child(&self, h: &Header, body: &[u8]) -> Result<PathBuf> {
        self.named(h, name(body)?)
    }

    /// `fuse_entry_out` of `path`, its node's lookup counted.
    fn entry(&self, path: &Path) -> Result<Vec<u8>> {
        let meta = fs::symlink_metadata(self.lower(path)).map_err(error)?;
        let mut nodes = self.nodes.lock().unwrap();
        let id = match nodes.by_path.get(path) {
            Some(&id) => id,
            None => {
                let id = nodes.next;
                nodes.next += 1;
                nodes.by_path.insert(path.to_path_buf(), id);
                nodes.by_id.insert(id, (path.to_path_buf(), 0));
                id
            }
        };
        if let Some(node) = nodes.by_id.get_mut(&id) {
            node.1 += 1;
        }
        let mut out = Vec::with_capacity(128);
        out.extend(id.to_le_bytes());
        out.extend(0u64.to_le_bytes()); // generation
        out.extend(0u64.to_le_bytes()); // entry valid
        out.extend(0u64.to_le_bytes()); // attr valid
        out.extend(0u32.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(attr(&meta));
        Ok(out)
    }

    fn forget(&self, node: u64, n: u64) {
        let mut nodes = self.nodes.lock().unwrap();
        if node == 1 {
            return;
        }
        let gone = match nodes.by_id.get_mut(&node) {
            Some(entry) => {
                entry.1 = entry.1.saturating_sub(n);
                (entry.1 == 0).then(|| entry.0.clone())
            }
            None => None,
        };
        if let Some(path) = gone {
            nodes.by_id.remove(&node);
            if nodes.by_path.get(&path) == Some(&node) {
                nodes.by_path.remove(&path);
            }
        }
    }

    /// The nodes at and below `from` are at `to` now.
    fn renamed(&self, from: &Path, to: &Path) {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(id) = nodes.by_path.remove(to) {
            // The replaced node keeps its id until forgotten, at no path.
            if let Some(entry) = nodes.by_id.get_mut(&id) {
                entry.0 = PathBuf::from("\0replaced");
            }
        }
        let moved: Vec<(PathBuf, u64)> = nodes.by_path.iter().filter(|(p, _)| p.starts_with(from)).map(|(p, id)| (p.clone(), *id)).collect();
        for (path, id) in moved {
            let new = to.join(path.strip_prefix(from).unwrap());
            nodes.by_path.remove(&path);
            nodes.by_path.insert(new.clone(), id);
            if let Some(entry) = nodes.by_id.get_mut(&id) {
                entry.0 = new;
            }
        }
    }

    fn keep(&self, handle: Handle) -> u64 {
        let mut handles = self.handles.lock().unwrap();
        let fh = handles.next;
        handles.next += 1;
        handles.open.insert(fh, handle);
        fh
    }

    fn with_file<T>(&self, fh: u64, f: impl FnOnce(&File) -> Result<T>) -> Result<T> {
        let file = match self.handles.lock().unwrap().open.get(&fh) {
            Some(Handle::File(file)) => file.try_clone().map_err(error)?,
            _ => return Err(errno::EINVAL),
        };
        f(&file)
    }

    /// A directory's entries, `.` and `..` first; at the root, only the
    /// users the caller may reach.
    fn entries(&self, h: &Header, path: &Path) -> Result<Vec<(Vec<u8>, u64, u32)>> {
        let lower = self.lower(path);
        let meta = fs::metadata(&lower).map_err(error)?;
        let mut entries = vec![(b".".to_vec(), meta.ino(), 4), (b"..".to_vec(), meta.ino(), 4)];
        for entry in fs::read_dir(&lower).map_err(error)? {
            let entry = entry.map_err(error)?;
            if permit(h.uid, &path.join(entry.file_name())).is_err() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            entries.push((entry.file_name().as_bytes().to_vec(), meta.ino(), dirent_type(meta.mode())));
        }
        Ok(entries)
    }

    fn setattr(&self, h: &Header, body: &[u8]) -> Result<Vec<u8>> {
        let path = self.path(h)?;
        let lower = self.lower(&path);
        let valid = u32_at(body, 0).ok_or(errno::EINVAL)?;
        if valid & (FATTR_UID | FATTR_GID) != 0 {
            return Err(errno::EPERM);
        }
        if valid & FATTR_SIZE != 0 {
            let size = u64_at(body, 16).ok_or(errno::EINVAL)?;
            match (valid & FATTR_FH != 0).then(|| u64_at(body, 8)).flatten() {
                Some(fh) => self.with_file(fh, |f| f.set_len(size).map_err(error))?,
                None => OpenOptions::new().write(true).open(&lower).and_then(|f| f.set_len(size)).map_err(error)?,
            }
        }
        if valid & FATTR_MODE != 0 {
            let mode = u32_at(body, 68).ok_or(errno::EINVAL)?;
            fs::set_permissions(&lower, fs::Permissions::from_mode(mode & 0o7777)).map_err(error)?;
        }
        if valid & (FATTR_ATIME | FATTR_MTIME) != 0 {
            let time = |set: u32, now: u32, seconds: usize, nanos: usize| -> Result<libc::timespec> {
                Ok(if valid & set == 0 {
                    libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT }
                } else if valid & now != 0 {
                    libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_NOW }
                } else {
                    libc::timespec {
                        tv_sec: u64_at(body, seconds).ok_or(errno::EINVAL)? as libc::time_t,
                        tv_nsec: u32_at(body, nanos).ok_or(errno::EINVAL)? as _,
                    }
                })
            };
            let times = [time(FATTR_ATIME, FATTR_ATIME_NOW, 32, 56)?, time(FATTR_MTIME, FATTR_MTIME_NOW, 40, 60)?];
            let c = std::ffi::CString::new(lower.as_os_str().as_bytes()).map_err(|_| errno::EINVAL)?;
            // SAFETY: a path and two times, read during the call.
            if unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) } != 0 {
                return Err(error(io::Error::last_os_error()));
            }
        }
        Ok(attr_out(&fs::symlink_metadata(&lower).map_err(error)?))
    }
}

/// Whether `uid` may reach `path` below the root: its own user's
/// directory and what is in it.
fn permit(uid: u32, path: &Path) -> Result<()> {
    if PRIVILEGED.contains(&uid) {
        return Ok(());
    }
    match path.components().next() {
        None => Ok(()),
        Some(user) if user.as_os_str() == OsStr::new(&(uid / PER_USER_RANGE).to_string()) => Ok(()),
        Some(_) => Err(errno::ENOENT),
    }
}

/// `fuse_in_header`.
pub fn header(message: &[u8]) -> Option<Header> {
    Some(Header { opcode: u32_at(message, 4)?, unique: u64_at(message, 8)?, node: u64_at(message, 16)?, uid: u32_at(message, 24)? })
}

/// `INIT`'s reply (`fuse_init_out`): this protocol's version at most, the
/// flags both sides have, its largest write.
fn init(body: &[u8]) -> Vec<u8> {
    let minor = u32_at(body, 4).unwrap_or(39).min(39);
    let flags = u32_at(body, 12).unwrap_or(0) & INIT_FLAGS;
    let mut out = Vec::with_capacity(64);
    for v in [7, minor, MAX_WRITE, flags] {
        out.extend(v.to_le_bytes());
    }
    out.extend(12u16.to_le_bytes()); // max background
    out.extend(9u16.to_le_bytes()); // congestion threshold
    out.extend(MAX_WRITE.to_le_bytes());
    out.extend(1u32.to_le_bytes()); // time granularity
    out.extend(((MAX_WRITE / 4096) as u16).to_le_bytes()); // max pages
    out.extend(0u16.to_le_bytes()); // map alignment
    out.extend(0u32.to_le_bytes()); // flags2
    out.extend([0; 28]);
    out
}

/// `fuse_attr` of `meta`.
fn attr(meta: &fs::Metadata) -> Vec<u8> {
    let mut out = Vec::with_capacity(88);
    for v in [meta.ino(), meta.size(), meta.blocks(), meta.atime() as u64, meta.mtime() as u64, meta.ctime() as u64] {
        out.extend(v.to_le_bytes());
    }
    for v in [meta.atime_nsec() as u32, meta.mtime_nsec() as u32, meta.ctime_nsec() as u32, meta.mode(), meta.nlink() as u32, meta.uid(), meta.gid(), meta.rdev() as u32, meta.blksize() as u32, 0] {
        out.extend(v.to_le_bytes());
    }
    out
}

/// `fuse_attr_out` of `meta`.
fn attr_out(meta: &fs::Metadata) -> Vec<u8> {
    let mut out = Vec::with_capacity(104);
    out.extend(0u64.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(attr(meta));
    out
}

/// `fuse_open_out` of the handle `fh`.
fn open_out(fh: u64) -> Vec<u8> {
    let mut out = fh.to_le_bytes().to_vec();
    out.extend([0; 8]);
    out
}

/// `fuse_dirent`s from `offset` that fit in `size` bytes.
fn dirents(entries: &[(Vec<u8>, u64, u32)], offset: usize, size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, (name, ino, kind)) in entries.iter().enumerate().skip(offset) {
        let len = (24 + name.len()).next_multiple_of(8);
        if out.len() + len > size {
            break;
        }
        out.extend(ino.to_le_bytes());
        out.extend(((i + 1) as u64).to_le_bytes());
        out.extend((name.len() as u32).to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(name);
        out.resize(out.len().next_multiple_of(8), 0);
    }
    out
}

/// `DT_*` of a mode.
fn dirent_type(mode: u32) -> u32 {
    (mode & S_IFMT) >> 12
}

/// `fuse_statfs_out` of the volume at `path`.
fn statfs(path: &Path) -> Result<Vec<u8>> {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| errno::EINVAL)?;
    // SAFETY: an out structure the call fills.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(error(io::Error::last_os_error()));
    }
    let mut out = Vec::with_capacity(80);
    for v in [s.f_blocks as u64, s.f_bfree as u64, s.f_bavail as u64, s.f_files as u64, s.f_ffree as u64] {
        out.extend(v.to_le_bytes());
    }
    for v in [s.f_bsize as u32, s.f_namemax as u32, s.f_frsize as u32, 0] {
        out.extend(v.to_le_bytes());
    }
    out.extend([0; 24]);
    Ok(out)
}

/// The open options of Linux open `flags`.
fn options(flags: u32) -> OpenOptions {
    let mut o = OpenOptions::new();
    match flags & O_ACCMODE {
        O_WRONLY => o.write(true),
        O_RDWR => o.read(true).write(true),
        _ => o.read(true),
    };
    if flags & O_CREAT != 0 {
        if flags & O_EXCL != 0 { o.create_new(true) } else { o.create(true) };
        o.write(true);
    }
    if flags & O_TRUNC != 0 {
        o.truncate(true);
    }
    if flags & O_APPEND != 0 {
        o.append(true);
    }
    o
}

fn read_at(file: &File, data: &mut [u8], offset: u64) -> Result<usize> {
    let mut n = 0;
    while n < data.len() {
        match file.read_at(&mut data[n..], offset + n as u64) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(error(e)),
        }
    }
    Ok(n)
}

/// A name ending at its NUL.
fn name(body: &[u8]) -> Result<&[u8]> {
    body.iter().position(|&b| b == 0).map(|end| &body[..end]).ok_or(errno::EINVAL)
}

/// Two NUL-ended names, one after the other.
fn two_names(body: &[u8]) -> Result<(&[u8], &[u8])> {
    let first = name(body)?;
    Ok((first, name(&body[first.len() + 1..])?))
}

/// Linux's errno of `e`: its own on Linux, else by its kind.
fn error(e: io::Error) -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(raw) = e.raw_os_error() {
        return raw;
    }
    use io::ErrorKind::*;
    match e.kind() {
        NotFound => errno::ENOENT,
        PermissionDenied => errno::EACCES,
        AlreadyExists => errno::EEXIST,
        NotADirectory => errno::ENOTDIR,
        IsADirectory => errno::EISDIR,
        DirectoryNotEmpty => errno::ENOTEMPTY,
        InvalidInput | InvalidFilename => errno::EINVAL,
        StorageFull => errno::ENOSPC,
        _ => errno::EIO,
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests;
