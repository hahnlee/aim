//! The guest owner and mode of a host inode in a writable area: the
//! `dev.aim.guest-inode` attribute the syscall layer reads for `stat`
//! (`docs/guest-init-contract.md` section 6; the layer's `sys/attrs.rs`
//! has the same codec).
//!
//! "DAGI", version 1, a presence byte (uid 1, gid 2, mode 4), two zero
//! bytes, then uid, gid and mode as little-endian words; 20 bytes. A field
//! nobody recorded is absent.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const NAME: &[u8] = b"dev.aim.guest-inode\0";
const XATTR_NOFOLLOW: c_int = 1;

unsafe extern "C" {
    fn getxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
}

/// Recorded owner and mode; None where nobody recorded one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuestInode {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mode: Option<u32>,
}

pub fn encode(a: GuestInode) -> [u8; 20] {
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

pub fn decode(b: &[u8]) -> Option<GuestInode> {
    if b.len() != 20 || &b[..5] != b"DAGI\x01" || b[5] & !7 != 0 {
        return None;
    }
    let field = |bit: u8, o: usize| {
        (b[5] & bit != 0).then(|| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()))
    };
    Some(GuestInode {
        uid: field(1, 8),
        gid: field(2, 12),
        mode: field(4, 16).map(|m| m & 0o7777),
    })
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}

/// The attribute of `path` (not following a final symlink), if any.
pub fn read(path: &Path) -> io::Result<Option<GuestInode>> {
    let p = c_path(path)?;
    let mut b = [0u8; 20];
    // SAFETY: NUL-terminated path and name, local buffer.
    let n = unsafe {
        getxattr(
            p.as_ptr(),
            NAME.as_ptr().cast(),
            b.as_mut_ptr().cast(),
            b.len(),
            0,
            XATTR_NOFOLLOW,
        )
    };
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENOATTR) {
            Ok(None)
        } else {
            Err(e)
        };
    }
    Ok(decode(&b[..n as usize]))
}

/// Records `a`'s fields over those `path` already has.
pub fn record(path: &Path, a: GuestInode) -> io::Result<()> {
    let old = read(path)?.unwrap_or_default();
    let b = encode(GuestInode {
        uid: a.uid.or(old.uid),
        gid: a.gid.or(old.gid),
        mode: a.mode.or(old.mode),
    });
    let p = c_path(path)?;
    // SAFETY: NUL-terminated path and name, exact readable span.
    let set = || unsafe {
        if setxattr(
            p.as_ptr(),
            NAME.as_ptr().cast(),
            b.as_ptr().cast(),
            b.len(),
            0,
            XATTR_NOFOLLOW,
        ) == 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    };
    match set() {
        // Changing an attribute needs write access, which a read-only
        // host mode denies the owner: it gets it for the change.
        Err(e) if e.raw_os_error() == Some(libc::EACCES) => {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(path)?;
            if meta.file_type().is_symlink() {
                return Err(e);
            }
            let mode = meta.permissions().mode() & 0o7777;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200))?;
            let result = set();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
            result
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_encoding_as_the_syscall_layer() {
        let a = GuestInode {
            uid: Some(1000),
            gid: Some(1001),
            mode: Some(0o771),
        };
        assert_eq!(
            encode(a),
            *b"DAGI\x01\x07\0\0\xe8\x03\0\0\xe9\x03\0\0\xf9\x01\0\0"
        );
        assert_eq!(decode(&encode(a)), Some(a));
        let partial = GuestInode {
            uid: None,
            gid: Some(3),
            mode: None,
        };
        assert_eq!(decode(&encode(partial)), Some(partial));
    }

    #[test]
    fn fields_merge_on_the_inode() {
        let path = std::env::temp_dir().join(format!("guest-inode-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        assert_eq!(read(&path).unwrap(), None);
        let owner = GuestInode {
            uid: Some(10123),
            gid: Some(10123),
            mode: None,
        };
        record(&path, owner).unwrap();
        record(
            &path,
            GuestInode {
                mode: Some(0o700),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            read(&path).unwrap(),
            Some(GuestInode {
                mode: Some(0o700),
                ..owner
            })
        );
        // A read-only host mode does not stop it, and stays.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        record(
            &path,
            GuestInode {
                uid: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(read(&path).unwrap().unwrap().uid, Some(0));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o444);
        std::fs::remove_file(&path).unwrap();
    }
}
