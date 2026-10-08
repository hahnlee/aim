//! Guest ownership and modes (`docs/guest-init-contract.md` section 6).
//!
//! The host cannot chown to Android ids, so a guest's (and init's)
//! `chown`/`chmod` results and the owner of a file it creates are kept on
//! the host inode itself, in the `dev.aim.guest-inode` attribute. A stat
//! reads one attribute of the file it stats, whichever mount reached it (a
//! bind shows its source's owner; a stub in a process's own tmpfs is
//! another inode), and the owner lives as long as the file, across boots.
//! The attribute moves with a rename and is shared by hard links.
//!
//! Paths with no writable host inode (the read-only image, which init
//! `chown`s and `chmod`s, and synthesized `/proc` and `/sys` entries) are
//! recorded in `<runtime>/fs-attrs` instead, as `<guest path>\t<uid|->\t
//! <gid|->\t<octal mode|->` lines keyed by the path in its area
//! (`vfs::attr_key`); the latest line wins field by field. That table is
//! small and written almost only by init at boot.
//!
//! Precedence: the guest attribute (its unrecorded fields are root's and
//! the host permission bits); without one, the table, then an image file's
//! original owner and mode (the `dev.aim.android-inode` attribute
//! android-image-extract writes), then root:root with the host permission
//! bits. Without a path map nothing is recorded.

use std::collections::HashMap;
use std::ffi::CStr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::vfs;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Attr {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mode: Option<u32>,
}

impl Attr {
    /// `self`'s recorded fields over `base`'s.
    fn over(self, base: Attr) -> Attr {
        Attr {
            uid: self.uid.or(base.uid),
            gid: self.gid.or(base.gid),
            mode: self.mode.or(base.mode),
        }
    }

    fn complete(&self) -> bool {
        self.uid.is_some() && self.gid.is_some() && self.mode.is_some()
    }
}

/// The host inode of a stat or a change.
#[derive(Clone, Copy)]
pub enum Host<'a> {
    /// A file of the read-only image, which carries no guest attribute.
    Image(&'a CStr),
    /// A path, not following a final symlink.
    Path(&'a CStr),
    Fd(i32),
}

const GUEST: &CStr = c"dev.aim.guest-inode";
const INODE: &CStr = c"dev.aim.android-inode";

/// `dev.aim.guest-inode`: "DAGI", version 1, a presence byte (uid 1, gid 2,
/// mode 4), two zero bytes, then uid, gid and mode as little-endian words;
/// 20 bytes. guest-init writes the same (its `guest_inode.rs`).
fn encode(a: Attr) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[..5].copy_from_slice(b"DAGI\x01");
    for (i, (bit, v)) in [(1, a.uid), (2, a.gid), (4, a.mode)]
        .into_iter()
        .enumerate()
    {
        if let Some(v) = v {
            b[5] |= bit;
            b[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
    }
    b
}

fn decode(b: &[u8]) -> Option<Attr> {
    if b.len() != 20 || &b[..5] != b"DAGI\x01" || b[5] & !7 != 0 {
        return None;
    }
    let field = |bit: u8, o: usize| {
        (b[5] & bit != 0).then(|| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()))
    };
    Some(Attr {
        uid: field(1, 8),
        gid: field(2, 12),
        mode: field(4, 16).map(|m| m & 0o7777),
    })
}

/// Reads attribute `name` of `host` into `b`: the length read, if any.
fn get(host: Host, name: &CStr, b: &mut [u8]) -> Option<usize> {
    // SAFETY: host path or fd, local buffer.
    let n = unsafe {
        match host {
            Host::Image(p) | Host::Path(p) => libc::getxattr(
                p.as_ptr(),
                name.as_ptr(),
                b.as_mut_ptr().cast(),
                b.len(),
                0,
                libc::XATTR_NOFOLLOW,
            ),
            Host::Fd(fd) => {
                libc::fgetxattr(fd, name.as_ptr(), b.as_mut_ptr().cast(), b.len(), 0, 0)
            }
        }
    };
    (n >= 0).then_some(n as usize)
}

fn guest_attr(host: Host) -> Option<Attr> {
    if matches!(host, Host::Image(_)) {
        return None;
    }
    let mut b = [0u8; 20];
    let n = get(host, GUEST, &mut b)?;
    decode(&b[..n])
}

/// The owner and mode an image file had in the original image
/// (`tools/android-image-extract/src/inode_metadata.rs`).
fn original(host: Host) -> Option<Attr> {
    let mut b = [0u8; 20];
    let n = get(host, INODE, &mut b)?;
    decode_original(&b[..n])
}

/// "DARI", version 1, then uid, gid and mode as little-endian words.
fn decode_original(b: &[u8]) -> Option<Attr> {
    if b.len() != 20 || &b[..8] != b"DARI\x01\0\0\0" {
        return None;
    }
    let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    Some(Attr {
        uid: Some(word(8)),
        gid: Some(word(12)),
        mode: Some(word(16) & 0o7777),
    })
}

/// Stores `a`'s recorded fields over the guest attribute of `host`.
fn set_guest(host: Host, a: Attr) -> Result<bool, crate::errno::Errno> {
    if matches!(host, Host::Image(_)) { return Ok(false); }
    let b = encode(a.over(guest_attr(host).unwrap_or_default()));
    let set = || {
        let result = unsafe { match host {
            Host::Path(p) => libc::setxattr(p.as_ptr(), GUEST.as_ptr(), b.as_ptr().cast(), b.len(), 0, libc::XATTR_NOFOLLOW),
            Host::Fd(fd) => libc::fsetxattr(fd, GUEST.as_ptr(), b.as_ptr().cast(), b.len(), 0, 0),
            Host::Image(_) => unreachable!(),
        } };
        if result == 0 { Ok(()) } else { Err(crate::errno::last()) }
    };
    let error = match set() { Ok(()) => return Ok(true), Err(error) => error };
    if matches!(error, crate::errno::EROFS | 95) { return Ok(false); }
    if !matches!(error, crate::errno::EACCES | crate::errno::EPERM) { return Err(error); }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let chmod = |mode| unsafe { match host {
        Host::Path(p) => libc::chmod(p.as_ptr(), mode),
        Host::Fd(fd) => libc::fchmod(fd, mode),
        Host::Image(_) => unreachable!(),
    } };
    let got = unsafe { match host {
        Host::Path(p) => libc::lstat(p.as_ptr(), &mut st),
        Host::Fd(fd) => libc::fstat(fd, &mut st),
        Host::Image(_) => unreachable!(),
    } };
    if got < 0 { return Err(crate::errno::last()); }
    if st.st_mode & libc::S_IFMT == libc::S_IFLNK { return Ok(false); }
    if error == crate::errno::EPERM { return Err(error); }
    let mode = st.st_mode & 0o7777;
    if chmod(mode | 0o200) < 0 { return Err(crate::errno::last()); }
    let result = set();
    if chmod(mode) < 0 { return Err(crate::errno::last()); }
    result.map(|()| true)
}

struct Table {
    /// (size, mtime) of the file when read.
    stamp: (i64, i64),
    map: HashMap<String, Attr>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn file() -> Option<PathBuf> {
    vfs::runtime_dir().map(|d| d.join("fs-attrs"))
}

#[cfg(test)]
pub(crate) fn tests_parse(text: &str) -> usize {
    parse(text).len()
}

fn parse(text: &str) -> HashMap<String, Attr> {
    let mut map: HashMap<String, Attr> = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let [path, uid, gid, mode] = f[..] else {
            continue;
        };
        let e = map
            .entry(path.trim_end_matches('/').to_string())
            .or_default();
        if let Ok(v) = uid.parse() {
            e.uid = Some(v);
        }
        if let Ok(v) = gid.parse() {
            e.gid = Some(v);
        }
        if let Ok(v) = u32::from_str_radix(mode, 8) {
            e.mode = Some(v);
        }
    }
    map
}

/// The table's attributes of a guest path.
fn table_lookup(guest: &str) -> Attr {
    if !guest.starts_with('/') { return Attr::default(); }
    table_lookup_key(&vfs::attr_key(if guest.len() > 1 { guest.trim_end_matches('/') } else { guest }))
}
fn table_lookup_key(key: &str) -> Attr {
    let Some(path) = file() else {
        return Attr::default();
    };
    let Ok(md) = std::fs::metadata(&path) else {
        return Attr::default();
    };
    use std::os::unix::fs::MetadataExt;
    let stamp = (
        md.size() as i64,
        md.mtime_nsec() + md.mtime() * 1_000_000_000,
    );
    let mut t = TABLE.lock().unwrap();
    if t.as_ref().is_none_or(|t| t.stamp != stamp) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        *t = Some(Table {
            stamp,
            map: parse(&text),
        });
    }
    t.as_ref().unwrap().map.get(key).copied().unwrap_or_default()
}

// Darwin devfs does not carry writable guest xattrs. Its allocated character
// inode has a real generation identity; a reused slave number is a new inode.
fn device_key(st: &libc::stat) -> String {
    format!("/@devfs/{}/{}/{}/{}/{}", st.st_dev, st.st_ino, st.st_gen, st.st_birthtime, st.st_birthtime_nsec)
}
/// Only actual PTY allocation creates a devfs owner binding. Other character
/// devices retain their existing inode/xattr/table metadata path.
pub(super) fn allocated_character(host: Host, attributes: Attr) -> Result<(), crate::errno::Errno> {
    if !recording() { return Ok(()); }
    let stat = host_stat(host)?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFCHR { return Err(crate::errno::EINVAL); }
    with_inode_lock(host, &stat, || table_record_key(&device_key(&stat), attributes))
}
pub(super) fn character_attributes(host: Host) -> Result<Option<Attr>, crate::errno::Errno> {
    let stat = host_stat(host)?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFCHR { return Ok(None); }
    let attributes = table_lookup_key(&device_key(&stat));
    Ok((attributes != Attr::default()).then_some(attributes))
}

fn table_record(guest: &str, a: Attr) -> Result<(), crate::errno::Errno> {
    table_record_key(&vfs::attr_key(guest), a)
}
fn table_record_key(key: &str, a: Attr) -> Result<(), crate::errno::Errno> {
    let path = file().ok_or(crate::errno::EIO)?;
    let field = |v: Option<u32>| v.map_or("-".to_string(), |v| v.to_string());
    let line = format!(
        "{}\t{}\t{}\t{}\n",
        key,
        field(a.uid),
        field(a.gid),
        a.mode.map_or("-".to_string(), |m| format!("{m:o}"))
    );
    // One append-mode write per line, so concurrent writers do not
    // interleave.
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    file.write_all(line.as_bytes())
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))
}

/// An inode's attributes as last read, valid while its ctime is (every
/// attribute change updates it, in any process).
struct Known {
    ctime: (i64, i64),
    guest: Option<Attr>,
    original: Option<Attr>,
}

/// Reading an attribute costs about 15 µs on the development Mac, 10 to 30
/// times a stat, so each process keeps what it read, by (dev, ino), and a
/// fork child starts with its parent's (an app with what zygote read).
static INODES: Mutex<Option<HashMap<(i32, u64), Known>>> = Mutex::new(None);
const INODES_MAX: usize = 1 << 16;

/// Fork: the attributes read so far. They stay valid in the child as they
/// do here, while the inode's ctime is unchanged.
/// One inode in the fork state: dev, ino, ctime seconds and nanoseconds,
/// a presence byte (guest 1, original 2), then both attributes encoded.
const RECORD: usize = 4 + 8 + 8 + 8 + 1 + 20 + 20;

pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let inodes = INODES.lock().unwrap();
    let mut b = Vec::with_capacity(inodes.as_ref().map_or(0, |m| m.len()) * RECORD);
    for (&(dev, ino), k) in inodes.iter().flatten() {
        b.extend_from_slice(&dev.to_le_bytes());
        b.extend_from_slice(&ino.to_le_bytes());
        b.extend_from_slice(&k.ctime.0.to_le_bytes());
        b.extend_from_slice(&k.ctime.1.to_le_bytes());
        b.push(k.guest.is_some() as u8 | (k.original.is_some() as u8) << 1);
        b.extend_from_slice(&encode(k.guest.unwrap_or_default()));
        b.extend_from_slice(&encode(k.original.unwrap_or_default()));
    }
    w.bytes(&b);
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let b = r.bytes();
    let word = |b: &[u8]| i64::from_le_bytes(b.try_into().unwrap());
    let known: HashMap<_, _> = b
        .chunks_exact(RECORD)
        .map(|b| {
            let present = b[28];
            let attr = |bit: u8, at: usize| {
                (present & bit != 0).then(|| decode(&b[at..at + 20]).unwrap_or_default())
            };
            let key = (
                i32::from_le_bytes(b[..4].try_into().unwrap()),
                word(&b[4..12]) as u64,
            );
            let k = Known {
                ctime: (word(&b[12..20]), word(&b[20..28])),
                guest: attr(1, 29),
                original: attr(2, 49),
            };
            (key, k)
        })
        .collect();
    *INODES.lock().unwrap() = (!known.is_empty()).then_some(known);
}

/// The guest and original attributes of `host`, whose stat is `st`.
fn inode_attrs(host: Host, st: &libc::stat) -> (Option<Attr>, Option<Attr>) {
    let key = (st.st_dev, st.st_ino);
    let ctime = (st.st_ctime, st.st_ctime_nsec);
    if let Some(k) = INODES.lock().unwrap().as_ref().and_then(|m| m.get(&key))
        && k.ctime == ctime
    {
        return (k.guest, k.original);
    }
    // The image's volume is read-only: no guest attribute to read there.
    let guest = if vfs::on_read_only_root(st.st_dev) {
        None
    } else {
        guest_attr(host)
    };
    let original = if guest.is_none() {
        original(host)
    } else {
        None
    };
    let mut inodes = INODES.lock().unwrap();
    let map = inodes.get_or_insert_with(HashMap::new);
    if map.len() >= INODES_MAX {
        map.clear();
    }
    map.insert(
        key,
        Known {
            ctime,
            guest,
            original,
        },
    );
    (guest, original)
}

/// The recorded owner and mode of `host`, whose stat is `st` (None where
/// nobody recorded one); `guest` names it for the table.
fn lookup_stat(host: Host, guest: impl FnOnce() -> String, st: &libc::stat) -> Attr {
    if !recording() {
        return Attr::default();
    }
    if st.st_mode & libc::S_IFMT == libc::S_IFCHR {
        let device = table_lookup_key(&device_key(st));
        if device != Attr::default() { return device; }
    }
    let (inode, original) = inode_attrs(host, st);
    if let Some(a) = inode {
        return a;
    }
    let a = table_lookup(&guest());
    if a.complete() {
        return a;
    }
    original.map_or(a, |o| a.over(o))
}

/// The recorded owner and mode of `host`.
#[cfg(test)]
pub fn lookup(host: Host, guest: impl FnOnce() -> String) -> Attr {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path or fd, local buffer.
    let got = unsafe {
        match host {
            Host::Image(p) | Host::Path(p) => libc::lstat(p.as_ptr(), &mut st),
            Host::Fd(fd) => libc::fstat(fd, &mut st),
        }
    };
    if got < 0 {
        return Attr::default();
    }
    lookup_stat(host, guest, &st)
}

/// Apply the guest view to `st`, a host stat of `host`. The host owner is
/// never the guest's.
pub fn apply(host: Host, guest: impl FnOnce() -> String, st: &mut libc::stat) {
    let a = lookup_stat(host, guest, st);
    st.st_uid = a.uid.unwrap_or(0);
    st.st_gid = a.gid.unwrap_or(0);
    if let Some(m) = a.mode {
        st.st_mode = (st.st_mode & libc::S_IFMT) | (m as u16 & 0o7777);
    }
}

/// Whether guest ownership and modes are recorded (under a path map).
pub fn recording() -> bool {
    vfs::runtime_dir().is_some()
}

thread_local! {
    static HELD_INODE_LOCKS: std::cell::RefCell<Vec<(i32,u64)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn host_stat(host: Host) -> Result<libc::stat, crate::errno::Errno> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { match host {
        Host::Fd(fd) => libc::fstat(fd, &mut stat),
        Host::Path(path) | Host::Image(path) => libc::lstat(path.as_ptr(), &mut stat),
    } };
    if result < 0 { Err(crate::errno::last()) } else { Ok(stat) }
}

/// Serialize metadata on the actual inode across guest processes and bind aliases.
/// Same-thread nested recording shares the already-held lock.
pub fn with_inode_lock<T>(host: Host, stat: &libc::stat,
        action: impl FnOnce() -> Result<T, crate::errno::Errno>) -> Result<T, crate::errno::Errno> {
    if !recording() { return action(); }
    let key = (stat.st_dev, stat.st_ino);
    let actual = host_stat(host)?;
    if (actual.st_dev, actual.st_ino) != key { return Err(crate::errno::from_darwin(libc::ESTALE)); }
    if HELD_INODE_LOCKS.with(|held| held.borrow().contains(&key)) { return action(); }
    let _own = super::fork::spawn::own_fds();
    let directory = vfs::runtime_dir().ok_or(crate::errno::EIO)?.join("inode-locks");
    std::fs::create_dir_all(&directory)
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    let path = directory.join(format!("{:x}-{:x}",key.0 as u32,key.1));
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    use std::os::fd::AsRawFd;
    use std::os::fd::{FromRawFd,IntoRawFd};
    struct PrivateLock(std::fs::File);
    impl Drop for PrivateLock { fn drop(&mut self) { super::fdtab::unhide(self.0.as_raw_fd()); } }
    let file = PrivateLock(unsafe { std::fs::File::from_raw_fd(super::fdtab::hide(file.into_raw_fd())) });
    loop {
        if unsafe { libc::flock(file.0.as_raw_fd(),libc::LOCK_EX) } == 0 { break; }
        let error = crate::errno::last(); if error != crate::errno::EINTR { return Err(error); }
    }
    struct Held((i32,u64));
    impl Drop for Held { fn drop(&mut self) {
        HELD_INODE_LOCKS.with(|held| held.borrow_mut().retain(|key|*key!=self.0));
    } }
    let actual = host_stat(host)?;
    if (actual.st_dev, actual.st_ino) != key { return Err(crate::errno::from_darwin(libc::ESTALE)); }
    HELD_INODE_LOCKS.with(|held| held.borrow_mut().push(key));
    let _held = Held(key);
    action()
}

/// Record a guest chown/chmod of `host` (only under a path map): on the
/// inode, or in the table when it cannot carry the attribute.
pub fn record_checked(host: Host, guest: impl FnOnce() -> String, a: Attr) -> Result<(), crate::errno::Errno> {
    if !recording() { return Ok(()); }
    let stat = host_stat(host)?;
    with_inode_lock(host, &stat, || {
        if stat.st_mode & libc::S_IFMT == libc::S_IFCHR {
            let previous = table_lookup_key(&device_key(&stat));
            if previous != Attr::default() {
                return table_record_key(&device_key(&stat), a.over(previous));
            }
        }
        if set_guest(host, a)? { return Ok(()); }
        table_record(&guest(), a)
    })
}

pub fn record(host: Host, guest: impl FnOnce() -> String, a: Attr) {
    if let Err(error) = record_checked(host, guest, a) {
        eprintln!("guest inode metadata recording failed: errno={error}");
    }
}

pub const REAL: usize = 0;
pub const EFFECTIVE: usize = 1;
pub const FS: usize = 3;

/// The guest's (uid, gid) of a kind (`REAL`, `EFFECTIVE`, `FS`).
pub fn ids(kind: usize) -> (u32, u32) {
    let id = super::cred::current();
    (id.uid[kind], id.gid[kind])
}

/// Whether nothing exists at a host path (so an O_CREAT would create it).
pub fn absent(host: &CStr) -> bool {
    if !recording() {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    unsafe { libc::lstat(host.as_ptr(), &mut st) < 0 }
}

/// The guest created `host`: it belongs to the guest's identity, as on
/// Linux (only under a path map).
pub fn created(host: Host, guest: impl FnOnce() -> String) -> Result<(), crate::errno::Errno> {
    if !recording() {
        return Ok(());
    }
    let guest = guest();
    let (uid, mut gid) = ids(FS);
    let parent_name = guest.rsplit_once('/').map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .ok_or(crate::errno::EINVAL)?;
    let parent = vfs::resolve(vfs::LINUX_AT_FDCWD, parent_name.as_bytes(), true)?;
    let mut parent_stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(parent.host.as_ptr(), &mut parent_stat) } < 0 {
        return Err(crate::errno::last());
    }
    apply(Host::Path(&parent.host), || parent.guest.clone(), &mut parent_stat);
    let mut mode = None;
    if parent_stat.st_mode & libc::S_ISGID != 0 {
        gid = parent_stat.st_gid;
        let mut child: libc::stat = unsafe { std::mem::zeroed() };
        let result = unsafe { match host {
            Host::Fd(fd) => libc::fstat(fd, &mut child),
            Host::Path(path) | Host::Image(path) => libc::lstat(path.as_ptr(), &mut child),
        } };
        if result < 0 { return Err(crate::errno::last()); }
        if child.st_mode & libc::S_IFMT == libc::S_IFDIR {
            mode = Some((child.st_mode as u32 & 0o7777) | libc::S_ISGID as u32);
        }
    }
    record_checked(
        host,
        || guest,
        Attr {
            uid: Some(uid),
            gid: Some(gid),
            mode,
        },
    )
}

/// Linux permission check of `mode` bits (R 4, W 2, X 1) for the guest
/// identity against the guest view of a stat.
/// Linux inode_permission using filesystem identity, or access(2)'s chosen identity.
pub fn permits(st: &libc::stat, want: u32, id: &super::cred::Identity, kind: usize) -> bool {
    let mode = st.st_mode as u32;
    let uid = id.uid[kind];
    let bits = if st.st_uid == uid {
        (mode >> 6) & 7
    } else if st.st_gid == id.gid[kind] || id.groups.contains(&st.st_gid) {
        (mode >> 3) & 7
    } else { mode & 7 };
    if bits & want == want { return true; }
    let capabilities = if kind == REAL {
        if uid == 0 { id.cap_perm } else { 0 }
    } else { id.cap_eff };
    let directory = mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32;
    if capabilities & (1 << 1) != 0 && (want & 1 == 0 || directory || mode & 0o111 != 0) {
        return true;
    }
    capabilities & (1 << 2) != 0 && want & 2 == 0 && (want & 1 == 0 || directory)
}

/// A resolved prefix has no remaining symlinks. Do not recurse through stat_at.
pub(super) fn path_stat(guest: &str) -> Result<libc::stat, crate::errno::Errno> {
    if let Some(route) = vfs::fuse_route(guest) {
        return super::fuse_client::stat(&route, None, None);
    }
    if let Some(stat) = super::procfs::stat(guest, true) { return stat; }
    let (host, area) = vfs::lookup(guest);
    use std::os::unix::ffi::OsStrExt;
    let host = std::ffi::CString::new(host.as_os_str().as_bytes()).map_err(|_| crate::errno::EINVAL)?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(host.as_ptr(), &mut stat) } < 0 { return Err(crate::errno::last()); }
    let host = if area == vfs::Area::Image { Host::Image(&host) } else { Host::Path(&host) };
    apply(host, || guest.to_owned(), &mut stat);
    Ok(stat)
}

pub(super) fn search(guest: &str, id: &super::cred::Identity, kind: usize) -> Result<(), crate::errno::Errno> {
    if !recording() { return Ok(()); }
    // Without default_permissions the original FUSE daemon owns access checks.
    if vfs::fuse_route(guest).is_some_and(|route| !route.default_permissions) { return Ok(()); }
    let stat = path_stat(guest)?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR { return Err(crate::errno::ENOTDIR); }
    if permits(&stat, 1, id, kind) { Ok(()) } else { Err(crate::errno::EACCES) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_line_wins_field_by_field() {
        let m =
            parse("/dev/socket/logd\t1036\t1036\t666\n/dev/socket/logd\t-\t1000\t-\nbad line\n");
        assert_eq!(
            m["/dev/socket/logd"],
            Attr {
                uid: Some(1036),
                gid: Some(1000),
                mode: Some(0o666)
            }
        );
    }

    #[test]
    fn guest_attribute_round_trips_with_absent_fields() {
        for a in [
            Attr::default(),
            Attr {
                uid: Some(10123),
                gid: None,
                mode: Some(0o4750),
            },
            Attr {
                uid: Some(0),
                gid: Some(u32::MAX - 1),
                mode: None,
            },
        ] {
            assert_eq!(decode(&encode(a)), Some(a));
        }
        let b = encode(Attr {
            uid: Some(1000),
            gid: Some(1001),
            mode: Some(0o771),
        });
        // guest-init's encoding (guest_inode.rs) of the same record.
        assert_eq!(b, *b"DAGI\x01\x07\0\0\xe8\x03\0\0\xe9\x03\0\0\xf9\x01\0\0");
        assert_eq!(decode(&b[..19]), None);
        let mut bad = b;
        bad[5] = 8;
        assert_eq!(decode(&bad), None);
    }

    #[test]
    fn permission_bits() {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = libc::S_IFREG | 0o640;
        st.st_uid = 1000;
        st.st_gid = 1001;
        let permits = |stat: &libc::stat, mask, uid, gid| {
            let mut id = super::super::cred::Identity::default();
            id.uid = [uid; 4]; id.gid = [gid; 4];
            if uid != 0 { id.cap_eff = 0; id.cap_perm = 0; }
            super::permits(stat, mask, &id, FS)
        };
        assert!(permits(&st, 6, 1000, 5));
        assert!(permits(&st, 4, 7, 1001));
        assert!(!permits(&st, 2, 7, 1001));
        assert!(!permits(&st, 4, 7, 7));
        assert!(permits(&st, 6, 0, 0));
        assert!(!permits(&st, 1, 0, 0));
    }

    #[test]
    fn dac_identity_groups_and_capability_execute_rules() {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        stat.st_mode = libc::S_IFREG | 0o040; stat.st_uid=10100; stat.st_gid=10200;
        let mut id = super::super::cred::Identity { uid:[10300;4],gid:[10300;4],cap_eff:0,cap_perm:0,..Default::default() };
        assert!(!permits(&stat,4,&id,FS)); id.groups.push(10200); assert!(permits(&stat,4,&id,FS));
        id.uid[FS]=10100; assert!(!permits(&stat,4,&id,FS));
        id.cap_eff=1<<1; assert!(permits(&stat,6,&id,FS)); assert!(!permits(&stat,1,&id,FS));
        stat.st_mode |= 1; assert!(permits(&stat,1,&id,FS));
        id.cap_eff=1<<2; stat.st_mode=libc::S_IFREG;
        assert!(permits(&stat,4,&id,FS)); assert!(!permits(&stat,2,&id,FS)); assert!(!permits(&stat,1,&id,FS));
        stat.st_mode=libc::S_IFDIR; assert!(permits(&stat,1,&id,FS));
        id.cap_perm=1<<1; id.uid[REAL]=10300; assert!(!permits(&stat,2,&id,REAL));
        id.uid[REAL]=0; assert!(permits(&stat,2,&id,REAL));
    }

    #[test]
    #[ignore = "subprocess helper exercised by inode_lock_serializes_aliases_across_processes"]
    fn inode_lock_child_probe() {
        use std::io::BufRead;
        let mut input = std::io::BufReader::new(std::io::stdin());
        let mut root = String::new(); let mut map = String::new(); let mut path = String::new();
        input.read_line(&mut root).unwrap(); input.read_line(&mut map).unwrap(); input.read_line(&mut path).unwrap();
        crate::vfs::init(std::path::Path::new(root.trim()),Some(std::path::Path::new(map.trim()))).unwrap();
        let file = std::fs::File::open(path.trim()).unwrap();
        use std::os::fd::AsRawFd;
        let host = Host::Fd(file.as_raw_fd()); let stat = host_stat(host).unwrap();
        println!("inode-child-ready"); std::io::stdout().flush().unwrap();
        with_inode_lock(host,&stat,|| {
            record_checked(host, || "/data/inode-child-alias".into(),
                Attr { gid:Some(10777), ..Default::default() })
        }).unwrap();
        println!("inode-child-acquired");
    }

    #[test]
    fn inode_lock_serializes_aliases_across_processes() {
        use std::io::{BufRead,Write};
        use std::os::fd::AsRawFd;
        let (_guard, root) = crate::vfs::test_view();
        let first = root.join("inode-parent-anchor"); let alias = root.join("inode-child-alias");
        std::fs::write(&first,b"owned").unwrap(); std::fs::hard_link(&first,&alias).unwrap();
        let file = std::fs::File::open(&first).unwrap();
        let host=Host::Fd(file.as_raw_fd()); let stat=host_stat(host).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","sys::attrs::tests::inode_lock_child_probe","--ignored","--nocapture"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        with_inode_lock(host,&stat,|| {
            let mut input=child.stdin.take().unwrap();
            writeln!(input,"{}",crate::vfs::root().display()).unwrap();
            writeln!(input,"{}",crate::vfs::runtime_dir().unwrap().join("path-map").display()).unwrap();
            writeln!(input,"{}",alias.display()).unwrap(); drop(input);
            let mut output=std::io::BufReader::new(child.stdout.take().unwrap());
            let mut line=String::new();
            loop { line.clear(); assert!(output.read_line(&mut line).unwrap()>0); if line.contains("inode-child-ready") { break; } }
            child.stdout=Some(output.into_inner());
            std::thread::sleep(std::time::Duration::from_millis(40));
            assert!(child.try_wait().unwrap().is_none(),"child bypassed inode lock");
            record_checked(host,|| "/data/inode-parent-anchor".into(),Attr {uid:Some(10666),..Default::default()})
        }).unwrap();
        let output=child.wait_with_output().unwrap(); assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stdout));
        let recorded=guest_attr(host).unwrap(); assert_eq!(recorded.uid,Some(10666)); assert_eq!(recorded.gid,Some(10777));
        std::fs::remove_file(alias).unwrap(); std::fs::remove_file(first).unwrap();
    }

    #[test]
    fn checked_record_rejects_invalid_fd_and_table_write_failure() {
        let (_guard, _root) = crate::vfs::test_view();
        assert_eq!(record_checked(Host::Fd(-1), || "/data/invalid-descriptor".into(),
            Attr { mode:Some(0o600), ..Default::default() }), Err(crate::errno::EBADF));
        let path = file().unwrap();
        let backup = path.with_extension("checked-record-backup");
        let previous = path.exists();
        if previous { std::fs::rename(&path, &backup).unwrap(); }
        std::fs::create_dir(&path).unwrap();
        let fixture = path.with_extension("readonly-fixture");
        std::fs::write(&fixture,b"readonly").unwrap();
        use std::os::unix::ffi::OsStrExt;
        let image = std::ffi::CString::new(fixture.as_os_str().as_bytes()).unwrap();
        let result = record_checked(Host::Image(&image), || "/readonly-fixture".into(),
            Attr { mode:Some(0o600), ..Default::default() });
        std::fs::remove_dir(&path).unwrap();
        std::fs::remove_file(fixture).unwrap();
        if previous { std::fs::rename(&backup, &path).unwrap(); }
        assert_eq!(result, Err(crate::errno::EISDIR));
    }

    #[test]
    fn a_fork_child_keeps_what_was_read_and_sees_later_changes() {
        use crate::sys::fork_state::{Reader, Writer};
        use std::os::unix::ffi::OsStrExt;
        let (_view, dir) = vfs::test_view();
        let f = dir.join(format!("data/fork-attrs-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let h = std::ffi::CString::new(f.as_os_str().as_bytes()).unwrap();
        let a = Attr {
            uid: Some(10123),
            gid: Some(10123),
            mode: Some(0o640),
        };
        record(Host::Path(&h), String::new, a);
        assert_eq!(lookup(Host::Path(&h), String::new), a);
        let mut w = Writer::default();
        fork_save(&mut w);
        let blob = w.into_bytes();
        *INODES.lock().unwrap() = None;
        let mut r = Reader::new(&blob);
        fork_restore(&mut r);
        assert!(r.ok());
        // The child has the parent's reading of the inode...
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: host path, local buffer.
        assert_eq!(unsafe { libc::lstat(h.as_ptr(), &mut st) }, 0);
        let inodes = INODES.lock().unwrap();
        let known = &inodes.as_ref().unwrap()[&(st.st_dev, st.st_ino)];
        assert_eq!((known.guest, known.original), (Some(a), None));
        drop(inodes);
        // ...until the inode changes, from any process: a chmod updates
        // the attribute and the ctime.
        record(
            Host::Path(&h),
            String::new,
            Attr {
                mode: Some(0o600),
                ..Attr::default()
            },
        );
        assert_eq!(
            lookup(Host::Path(&h), String::new),
            Attr {
                mode: Some(0o600),
                ..a
            }
        );
        std::fs::remove_file(&f).unwrap();
    }

    /// What carrying the attributes costs a fork (#478): `cargo test -p
    /// aim-linux-abi --lib --release fork_carry_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn fork_carry_cost() {
        use crate::sys::fork_state::{Reader, Writer};
        let _view = vfs::test_view();
        let a = Attr {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o644),
        };
        for n in [1_000u64, 10_000] {
            *INODES.lock().unwrap() = Some(
                (0..n)
                    .map(|i| {
                        let original = Some(a);
                        let k = Known {
                            ctime: (i as i64, 0),
                            guest: None,
                            original,
                        };
                        ((1, i), k)
                    })
                    .collect(),
            );
            let start = std::time::Instant::now();
            let mut w = Writer::default();
            fork_save(&mut w);
            let blob = w.into_bytes();
            let saved = start.elapsed();
            fork_restore(&mut Reader::new(&blob));
            eprintln!(
                "{n} inodes: {} bytes, save {} us, restore {} us",
                blob.len(),
                saved.as_micros(),
                (start.elapsed() - saved).as_micros()
            );
        }
        *INODES.lock().unwrap() = None;
    }

    #[test]
    fn original_inodes_decode() {
        // clatd's directory in the tethering APEX: clat:system 040750.
        let b = b"DARI\x01\0\0\0\x05\x04\0\0\xe8\x03\0\0\xe8\x41\0\0";
        assert_eq!(
            decode_original(b),
            Some(Attr {
                uid: Some(1029),
                gid: Some(1000),
                mode: Some(0o750),
            })
        );
        assert_eq!(decode_original(&b[..19]), None);
        assert_eq!(
            decode_original(b"DARI\x02\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            None
        );
    }
}
