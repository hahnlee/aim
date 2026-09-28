//! Guest ownership and modes: `<runtime>/fs-attrs`
//! (`docs/guest-init-contract.md` section 6).
//!
//! The host cannot chown to Android ids, so init's (and the guests')
//! `chown`/`chmod` results are recorded as `<guest path>\t<uid|->\t<gid|->\t
//! <octal mode|->` lines; the latest line wins field by field. The guest
//! path is the file's path in the path map's areas (`vfs::attr_key`), so a
//! bind mount shows its source's owner, and a stub in a process's own tmpfs
//! does not change what the same guest path shows elsewhere. `stat`
//! reports these. Files of the image keep their original owner and mode in
//! the `dev.aim.android-inode` attribute android-image-extract
//! writes (`original`); other paths with no entry (and every path without a
//! path map) report root:root with the host permission bits.

use std::collections::HashMap;
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

struct Table {
    /// (size, mtime) of the file when read.
    stamp: (i64, i64),
    map: HashMap<String, Attr>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn file() -> Option<PathBuf> {
    vfs::runtime_dir().map(|d| d.join("fs-attrs"))
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

/// The recorded attributes of a guest path.
pub fn lookup(guest: &str) -> Attr {
    let Some(path) = file() else {
        return Attr::default();
    };
    let Ok(md) = std::fs::metadata(&path) else {
        return Attr::default();
    };
    let key = vfs::attr_key(if guest.len() > 1 {
        guest.trim_end_matches('/')
    } else {
        guest
    });
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
    t.as_ref()
        .and_then(|t| t.map.get(&key).copied())
        .unwrap_or_default()
}

/// Where the host file of a stat is, to read its original inode.
pub enum Host<'a> {
    /// A path, not following a final symlink.
    Path(&'a std::ffi::CStr),
    Fd(i32),
    /// Not an image file.
    None,
}

const INODE: &std::ffi::CStr = c"dev.aim.android-inode";

/// The owner and mode an image file had in the original image
/// (`tools/android-image-extract/src/inode_metadata.rs`).
fn original(host: Host) -> Option<Attr> {
    let mut b = [0u8; 20];
    // SAFETY: host path or fd, local buffer.
    let n = unsafe {
        match host {
            Host::Path(p) => libc::getxattr(
                p.as_ptr(),
                INODE.as_ptr(),
                b.as_mut_ptr().cast(),
                b.len(),
                0,
                libc::XATTR_NOFOLLOW,
            ),
            Host::Fd(fd) => {
                libc::fgetxattr(fd, INODE.as_ptr(), b.as_mut_ptr().cast(), b.len(), 0, 0)
            }
            Host::None => return None,
        }
    };
    decode_original(&b[..n.max(0) as usize])
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

/// Apply the guest view to a host stat of `guest`: recorded attributes
/// first, then an image file's original ones; otherwise Android files are
/// root's. The host owner is never the guest's.
pub fn apply(guest: &str, st: &mut libc::stat) {
    apply_host(guest, Host::None, st);
}

/// `apply`, reading the original inode of an image file at `host`.
pub fn apply_host(guest: &str, host: Host, st: &mut libc::stat) {
    let mut a = lookup(guest);
    if (a.uid.is_none() || a.gid.is_none() || a.mode.is_none())
        && vfs::runtime_dir().is_some()
        && let Some(o) = original(host)
    {
        a.uid = a.uid.or(o.uid);
        a.gid = a.gid.or(o.gid);
        a.mode = a.mode.or(o.mode);
    }
    st.st_uid = a.uid.unwrap_or(0);
    st.st_gid = a.gid.unwrap_or(0);
    if let Some(m) = a.mode {
        st.st_mode = (st.st_mode & libc::S_IFMT) | (m as u16 & 0o7777);
    }
}

/// Whether guest ownership and modes are recorded (under a path map).
pub fn recording() -> bool {
    file().is_some()
}

/// Record a guest chown/chmod (only under a path map).
pub fn record(guest: &str, a: Attr) {
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

pub const REAL: usize = 0;
pub const EFFECTIVE: usize = 1;
pub const FS: usize = 3;

/// The guest's (uid, gid) of a kind (`REAL`, `EFFECTIVE`, `FS`).
pub fn ids(kind: usize) -> (u32, u32) {
    let id = super::cred::current();
    (id.uid[kind], id.gid[kind])
}

/// Whether nothing exists at a host path (so an O_CREAT would create it).
pub fn absent(host: &std::ffi::CStr) -> bool {
    if vfs::runtime_dir().is_none() {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    unsafe { libc::lstat(host.as_ptr(), &mut st) < 0 }
}

/// The guest created `guest`: it belongs to the guest's identity, as on
/// Linux (only under a path map).
pub fn created(guest: &str) {
    if vfs::runtime_dir().is_none() {
        return;
    }
    record(
        guest,
        Attr {
            uid: Some(ids(FS).0),
            gid: Some(ids(FS).1),
            mode: None,
        },
    );
}

/// `from` was renamed to `to`: its recorded attributes move along.
pub fn renamed(from: &str, to: &str) {
    if vfs::runtime_dir().is_none() {
        return;
    }
    let a = lookup(from);
    if a != Attr::default() {
        record(to, a);
    }
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
