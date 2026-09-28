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
fn set_guest(host: Host, a: Attr) -> bool {
    let b = encode(a.over(guest_attr(host).unwrap_or_default()));
    // SAFETY: host path or fd, local buffer.
    let set = || unsafe {
        match host {
            Host::Image(_) => false,
            Host::Path(p) => {
                libc::setxattr(
                    p.as_ptr(),
                    GUEST.as_ptr(),
                    b.as_ptr().cast(),
                    b.len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                ) == 0
            }
            Host::Fd(fd) => {
                libc::fsetxattr(fd, GUEST.as_ptr(), b.as_ptr().cast(), b.len(), 0, 0) == 0
            }
        }
    };
    if set() {
        return true;
    }
    // Changing an attribute needs write access: a file the guest created
    // read-only (`open(O_CREAT, 0444)`) gets it for the change.
    if std::io::Error::last_os_error().raw_os_error() != Some(libc::EACCES) {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path or fd, local buffer; then plain chmods of it.
    let chmod = |m: u16| unsafe {
        match host {
            Host::Path(p) => libc::chmod(p.as_ptr(), m),
            Host::Fd(fd) => libc::fchmod(fd, m),
            Host::Image(_) => -1,
        }
    };
    let got = unsafe {
        match host {
            Host::Path(p) => libc::lstat(p.as_ptr(), &mut st),
            Host::Fd(fd) => libc::fstat(fd, &mut st),
            Host::Image(_) => -1,
        }
    };
    let mode = st.st_mode & 0o7777;
    if got < 0 || st.st_mode & libc::S_IFMT == libc::S_IFLNK || chmod(mode | 0o200) < 0 {
        return false;
    }
    let ok = set();
    chmod(mode);
    ok
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
    let map = &t.as_ref().unwrap().map;
    if map.is_empty() || !guest.starts_with('/') {
        return Attr::default();
    }
    let key = vfs::attr_key(if guest.len() > 1 {
        guest.trim_end_matches('/')
    } else {
        guest
    });
    map.get(&key).copied().unwrap_or_default()
}

fn table_record(guest: &str, a: Attr) {
    let Some(path) = file() else {
        return;
    };
    let field = |v: Option<u32>| v.map_or("-".to_string(), |v| v.to_string());
    let line = format!(
        "{}\t{}\t{}\t{}\n",
        vfs::attr_key(guest),
        field(a.uid),
        field(a.gid),
        a.mode.map_or("-".to_string(), |m| format!("{m:o}"))
    );
    // One append-mode write per line, so concurrent writers do not
    // interleave.
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// An inode's attributes as last read, valid while its ctime is (every
/// attribute change updates it, in any process).
struct Known {
    ctime: (i64, i64),
    guest: Option<Attr>,
    original: Option<Attr>,
}

/// Reading an attribute costs about 15 µs on the development Mac, 10 to 30
/// times a stat, so each
/// process keeps what it read, by (dev, ino).
static INODES: Mutex<Option<HashMap<(i32, u64), Known>>> = Mutex::new(None);
const INODES_MAX: usize = 1 << 16;

/// The guest and original attributes of `host`, whose stat is `st`.
fn inode_attrs(host: Host, st: &libc::stat) -> (Option<Attr>, Option<Attr>) {
    let key = (st.st_dev, st.st_ino);
    let ctime = (st.st_ctime, st.st_ctime_nsec);
    if let Some(k) = INODES.lock().unwrap().as_ref().and_then(|m| m.get(&key))
        && k.ctime == ctime
    {
        return (k.guest, k.original);
    }
    let guest = guest_attr(host);
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

/// Record a guest chown/chmod of `host` (only under a path map): on the
/// inode, or in the table when it cannot carry the attribute.
pub fn record(host: Host, guest: impl FnOnce() -> String, a: Attr) {
    if recording() && !set_guest(host, a) {
        table_record(&guest(), a);
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
pub fn created(host: Host, guest: impl FnOnce() -> String) {
    if !recording() {
        return;
    }
    let (uid, gid) = ids(FS);
    record(
        host,
        guest,
        Attr {
            uid: Some(uid),
            gid: Some(gid),
            mode: None,
        },
    );
}

/// Linux permission check of `mode` bits (R 4, W 2, X 1) for the guest
/// identity against the guest view of a stat.
pub fn permits(st: &libc::stat, want: u32, uid: u32, gid: u32) -> bool {
    if want == 0 {
        return true;
    }
    let m = st.st_mode as u32;
    if uid == 0 {
        // root: everything but execute needs no bit; execute needs one.
        return want & 1 == 0 || m & 0o111 != 0 || m & libc::S_IFMT as u32 == libc::S_IFDIR as u32;
    }
    let bits = if st.st_uid == uid {
        (m >> 6) & 7
    } else if st.st_gid == gid {
        (m >> 3) & 7
    } else {
        m & 7
    };
    bits & want == want
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
        assert!(permits(&st, 6, 1000, 5));
        assert!(permits(&st, 4, 7, 1001));
        assert!(!permits(&st, 2, 7, 1001));
        assert!(!permits(&st, 4, 7, 7));
        assert!(permits(&st, 6, 0, 0));
        assert!(!permits(&st, 1, 0, 0));
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
