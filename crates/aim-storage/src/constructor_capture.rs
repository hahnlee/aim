//! A CLI-owned immutable constructor output, published before package services.
use crate::{
    copy,
    guest_inode::{self, GuestInode},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};
pub const FILES: [&str; 5] = [
    "system/packages.xml",
    "system/packages.xml.reservecopy",
    "system/users/0/package-restrictions.xml",
    "system/users/0/package-restrictions.xml.reservecopy",
    "system/packages.list",
];
const MAGIC: &[u8; 8] = b"AIMPCAP1";
const DIRECTORIES: [&str; 3] = ["system", "system/users", "system/users/0"];
pub struct Sink {
    path: PathBuf,
    identity: (u64, u64),
    published: Mutex<bool>,
}
pub struct Capture {
    pub epoch: u64,
    pub controller_version: i64,
    pub directory: PathBuf,
}
fn error(message: impl ToString) -> String {
    message.to_string()
}
impl Sink {
    /// Claim a new host-only directory. Existing paths are never reused.
    pub fn claim(path: &Path) -> Result<Self, String> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(error)?;
        let path = path.canonicalize().map_err(error)?;
        let meta = fs::metadata(&path).map_err(error)?;
        Ok(Self {
            path,
            identity: (meta.dev(), meta.ino()),
            published: Mutex::new(false),
        })
    }
    pub fn freeze(
        &self,
        data: &Path,
        epoch: u64,
        controller_version: i64,
    ) -> Result<Capture, String> {
        let mut published = self.published.lock().unwrap();
        if *published || epoch == 0 || controller_version < 0 {
            return Err("invalid/repeated constructor capture".into());
        }
        let meta = fs::symlink_metadata(&self.path).map_err(error)?;
        if (meta.dev(), meta.ino()) != self.identity
            || !meta.is_dir()
            || meta.mode() & 0o777 != 0o700
        {
            return Err("constructor output capability changed".into());
        }
        for file in [
            "system/packages-backup.xml",
            "system/users/0/package-restrictions-backup.xml",
        ] {
            if data.join(file).exists() {
                return Err("constructor settings write is incomplete".into());
            }
        }
        let data = data.canonicalize().map_err(error)?;
        if self.path.starts_with(&data) || data.starts_with(&self.path) {
            return Err("capture output overlaps guest data".into());
        }
        let before = read_entries(&data)?;
        if before[0].0 != before[1].0 || before[2].0 != before[3].0 {
            return Err("constructor reserve differs".into());
        }
        let pending = self.path.join("pending");
        fs::create_dir(&pending).map_err(error)?;
        let paths: Vec<_> = FILES.iter().map(Path::new).collect();
        copy::copy_paths(&data, &pending, &paths)?;
        if read_entries(&data)? != before || read_entries(&pending)? != before {
            return Err("constructor settings changed during capture".into());
        }
        let mut record = Vec::from(MAGIC.as_slice());
        record.extend(epoch.to_le_bytes());
        record.extend(controller_version.to_le_bytes());
        for (hash, inode, mode) in &before {
            record.extend(hash);
            record.extend(guest_inode::encode(*inode));
            record.extend(mode.to_le_bytes());
        }
        for file in FILES {
            fs::File::open(pending.join(file))
                .and_then(|file| file.sync_all())
                .map_err(error)?;
        }
        for directory in ["system/users/0", "system/users", "system", ""] {
            fs::File::open(pending.join(directory))
                .and_then(|file| file.sync_all())
                .map_err(error)?;
        }
        fs::rename(&pending, self.path.join("settings")).map_err(error)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.path.join("capture.pending"))
            .map_err(error)?;
        file.write_all(&record)
            .and_then(|()| file.sync_all())
            .map_err(error)?;
        fs::rename(self.path.join("capture.pending"), self.path.join("capture")).map_err(error)?;
        fs::File::open(&self.path)
            .and_then(|file| file.sync_all())
            .map_err(error)?;
        *published = true;
        Ok(Capture {
            epoch,
            controller_version,
            directory: self.path.join("settings"),
        })
    }
}
use std::os::unix::fs::DirBuilderExt;
fn read_entries(data: &Path) -> Result<Vec<([u8; 32], GuestInode, u32)>, String> {
    FILES
        .iter()
        .chain(DIRECTORIES.iter())
        .map(|file| {
            let path = data.join(file);
            let meta = fs::symlink_metadata(&path).map_err(error)?;
            let directory = DIRECTORIES.contains(file);
            if if directory {
                !meta.is_dir()
            } else {
                !meta.is_file()
            } {
                return Err(format!("{file}: not a regular constructor file"));
            }
            let mut bytes = Vec::new();
            if !directory {
                fs::File::open(&path)
                    .and_then(|mut file| file.read_to_end(&mut bytes))
                    .map_err(error)?;
            }
            let mut hash = Sha256::new();
            hash.update(bytes);
            hash.update(meta.uid().to_le_bytes());
            hash.update(meta.gid().to_le_bytes());
            hash.update(xattrs(&path)?);
            let inode = guest_inode::read(&path)
                .map_err(error)?
                .ok_or_else(|| format!("{file}: missing guest inode metadata"))?;
            if inode.uid.is_none() || inode.gid.is_none() || inode.mode.is_none() {
                return Err(format!("{file}: incomplete guest inode metadata"));
            }
            Ok((hash.finalize().into(), inode, meta.mode() & 0o7777))
        })
        .collect()
}
// Include every xattr (including SELinux labels), not only the guest inode.
fn xattrs(path: &Path) -> Result<Vec<u8>, String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes()).map_err(error)?;
    let size =
        unsafe { libc::listxattr(path.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if size < 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    let mut names = vec![0; size as usize];
    let count = unsafe {
        libc::listxattr(
            path.as_ptr(),
            names.as_mut_ptr().cast(),
            names.len(),
            libc::XATTR_NOFOLLOW,
        )
    };
    if count < 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    names.truncate(count as usize);
    let mut names: Vec<_> = names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    let mut record = Vec::new();
    for name in names {
        let name = CString::new(name).map_err(error)?;
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if size < 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
        let mut value = vec![0; size as usize];
        let count = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if count < 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
        value.truncate(count as usize);
        record.extend((name.as_bytes().len() as u64).to_le_bytes());
        record.extend(name.as_bytes());
        record.extend((value.len() as u64).to_le_bytes());
        record.extend(value);
    }
    Ok(record)
}
/// Only the committed bundle is consumable; incomplete output fails closed.
pub fn read(path: &Path) -> Result<Option<Capture>, String> {
    let bytes = match fs::read(path.join("capture")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    if bytes.len() != 24 + (FILES.len() + DIRECTORIES.len()) * 56 || &bytes[..8] != MAGIC {
        return Err("invalid constructor capture record".into());
    }
    let epoch = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let controller_version = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
    if epoch == 0 || controller_version < 0 {
        return Err("invalid constructor capture epoch/controller".into());
    }
    let directory = path.join("settings");
    for (entry, record) in read_entries(&directory)?
        .iter()
        .zip(bytes[24..].chunks_exact(56))
    {
        if entry.0.as_slice() != &record[..32]
            || guest_inode::encode(entry.1).as_slice() != &record[32..52]
            || entry.2.to_le_bytes().as_slice() != &record[52..56]
        {
            return Err("immutable constructor capture changed".into());
        }
    }
    Ok(Some(Capture {
        epoch,
        controller_version,
        directory,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_committed_immutable_constructor_can_be_consumed() {
        let root = std::env::temp_dir().join(format!(
            "aim-constructor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let data = root.join("data");
        fs::create_dir(&data).unwrap();
        for file in FILES {
            let path = data.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"constructor").unwrap();
            guest_inode::record(
                &path,
                GuestInode {
                    uid: Some(1000),
                    gid: Some(1000),
                    mode: Some(0o660),
                },
            )
            .unwrap();
        }
        for directory in DIRECTORIES {
            guest_inode::record(
                &data.join(directory),
                GuestInode {
                    uid: Some(1000),
                    gid: Some(1000),
                    mode: Some(0o770),
                },
            )
            .unwrap();
        }
        let output = root.join("output");
        let sink = Sink::claim(&output).unwrap();
        assert!(Sink::claim(&output).is_err());
        assert!(read(&output).unwrap().is_none());
        fs::write(data.join("system/packages-backup.xml"), b"previous").unwrap();
        assert!(sink.freeze(&data, u64::MAX, 42).is_err());
        assert!(read(&output).unwrap().is_none());
        fs::remove_file(data.join("system/packages-backup.xml")).unwrap();
        let frozen = sink.freeze(&data, u64::MAX, 42).unwrap();
        assert!(sink.freeze(&data, 8, 42).is_err());
        for file in FILES {
            fs::write(data.join(file), b"app mutation").unwrap();
        }
        let capture = read(&output).unwrap().unwrap();
        assert_eq!((capture.epoch, capture.controller_version), (u64::MAX, 42));
        assert_eq!(
            fs::read(frozen.directory.join(FILES[0])).unwrap(),
            b"constructor"
        );
        guest_inode::record(
            &frozen.directory.join(FILES[0]),
            GuestInode {
                uid: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(read(&output).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
