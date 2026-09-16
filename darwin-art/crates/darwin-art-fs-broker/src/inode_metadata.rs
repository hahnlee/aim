//! Android image metadata attached to the backing inode, not its host pathname.
//! Only trusted image producers write this attribute. It does not authorize a
//! guest chown or implement mutable inode permission policy.
use std::ffi::{c_char, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

const NAME: &[u8] = b"com.darwin-art.android-inode\0";
const SIZE: usize = 20;
const ENOATTR: i32 = 93;
const XATTR_CREATE: c_int = 2;
unsafe extern "C" {
    fn fgetxattr(
        fd: c_int,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
    fn fsetxattr(
        fd: c_int,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AndroidInodeMetadata {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid Android inode metadata")
}

fn validate(file: &File, value: AndroidInodeMetadata) -> io::Result<()> {
    if value.mode & !0xffff != 0 || value.mode & 0o170000 != file.metadata()?.mode() & 0o170000 {
        return Err(invalid());
    }
    Ok(())
}

/// Absence means legacy/unmaterialized metadata, never assumed Android root.
/// Corrupt, oversized or unreadable metadata is an error, not absence.
pub fn read(file: &File) -> io::Result<Option<AndroidInodeMetadata>> {
    let mut bytes = [0u8; SIZE];
    // SAFETY: descriptor is borrowed for this call; name is NUL-terminated and
    // the output span is exactly SIZE initialized writable bytes.
    let length = unsafe {
        fgetxattr(
            file.as_raw_fd(),
            NAME.as_ptr().cast(),
            bytes.as_mut_ptr().cast(),
            SIZE,
            0,
            0,
        )
    };
    if length < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ENOATTR) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    if length as usize != SIZE || &bytes[..8] != b"DARI\x01\0\0\0" {
        return Err(invalid());
    }
    let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let value = AndroidInodeMetadata {
        uid: word(8),
        gid: word(12),
        mode: word(16),
    };
    validate(file, value)?;
    Ok(Some(value))
}

/// Set source ownership on a newly materialized inode, without changing Darwin
/// uid/gid or executable/set-ID permission bits. Existing attributes are never
/// overwritten. Caller syncs the file before publishing the completed image.
pub fn write_new(file: &File, value: AndroidInodeMetadata) -> io::Result<()> {
    validate(file, value)?;
    let mut bytes = [0u8; SIZE];
    bytes[..8].copy_from_slice(b"DARI\x01\0\0\0");
    bytes[8..12].copy_from_slice(&value.uid.to_le_bytes());
    bytes[12..16].copy_from_slice(&value.gid.to_le_bytes());
    bytes[16..20].copy_from_slice(&value.mode.to_le_bytes());
    // SAFETY: borrowed descriptor and exact readable initialized span; CREATE
    // makes the kernel operation fail rather than overwrite existing metadata.
    if unsafe {
        fsetxattr(
            file.as_raw_fd(),
            NAME.as_ptr().cast(),
            bytes.as_ptr().cast(),
            SIZE,
            0,
            XATTR_CREATE,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn retained_inode_roundtrip_and_no_host_ownership_change() {
        let path = std::env::temp_dir().join(format!(
            "android-inode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = File::create_new(&path).unwrap();
        let before = file.metadata().unwrap();
        assert_eq!(read(&file).unwrap(), None);
        let value = AndroidInodeMetadata {
            uid: 123456,
            gid: 234567,
            mode: 0o104750,
        };
        write_new(&file, value).unwrap();
        assert!(write_new(&file, value).is_err());
        assert_eq!(read(&file).unwrap(), Some(value));
        let moved = path.with_extension("moved");
        fs::rename(&path, &moved).unwrap();
        assert_eq!(read(&File::open(&moved).unwrap()).unwrap(), Some(value));
        fs::remove_file(&moved).unwrap();
        assert_eq!(read(&file).unwrap(), Some(value));
        let after = file.metadata().unwrap();
        assert_eq!(
            (before.uid(), before.gid(), before.mode()),
            (after.uid(), after.gid(), after.mode())
        );
        // A malformed or oversized attribute must never be treated as missing
        // and fall back to host ownership. Only this test bypasses write_new.
        for bytes in [vec![0; SIZE], vec![0; SIZE + 1], Vec::new()] {
            assert_eq!(
                unsafe {
                    fsetxattr(
                        file.as_raw_fd(),
                        NAME.as_ptr().cast(),
                        bytes.as_ptr().cast(),
                        bytes.len(),
                        0,
                        0,
                    )
                },
                0
            );
            assert!(read(&file).is_err());
        }
    }
}
