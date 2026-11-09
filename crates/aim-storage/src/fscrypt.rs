//! The fscrypt state of the data filesystem (ADR 0016): its keyring and
//! the table of policy roots, shared by every guest process as the kernel
//! keeps them per superblock.
//!
//! guest-init prepares both when it mounts the data directory
//! ([`Owner::prepare`]): the keyring is empty at every boot, as a mounted
//! filesystem's is, and lives in the runtime directory; the roots table,
//! the guest paths of directories given a policy, persists with the data.
//! The locator `<runtime>/fscrypt-root` names the persistent directory, as
//! `fs-verity-root` names the verity store. Changes are serialized by
//! `flock` on the file they change.
//!
//! The keyring keeps key specifiers and, for v2 keys, the users who added
//! them; never a raw key. The data stays plaintext on the host (ADR 0016,
//! option B), and an identifier is all that tells keys apart.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha512};

use crate::private_fd::PrivateFile;

const LOCATOR: &str = "fscrypt-root";
const KEYRING: &str = "fscrypt-keyring";
const MAGIC: &[u8] = b"AIMFSCRYPT01\0";

/// `struct fscrypt_key_specifier`: a v1 key by its descriptor, a v2 key by
/// its identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    Descriptor([u8; 8]),
    Identifier([u8; 16]),
}

impl Spec {
    fn encode(&self) -> String {
        let (kind, bytes): (char, &[u8]) = match self {
            Spec::Descriptor(bytes) => ('d', bytes),
            Spec::Identifier(bytes) => ('i', bytes),
        };
        bytes.iter().fold(kind.to_string(), |mut text, byte| {
            text.push_str(&format!("{byte:02x}"));
            text
        })
    }
}

/// A key's state for one user (`FS_IOC_GET_ENCRYPTION_KEY_STATUS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Users who added a v2 key; 0 for a v1 key.
    pub users: u32,
    pub added_by_self: bool,
}

/// What removing a user's claim did (`FS_IOC_REMOVE_ENCRYPTION_KEY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// The key is not there, or this user never added it: ENOKEY.
    NotAdded,
    /// Other users still hold the key (`..._STATUS_FLAG_OTHER_USERS`).
    OtherUsers,
    Removed,
}

struct Entry {
    spec: String,
    /// None for a v1 key, which has no users.
    users: Option<Vec<u32>>,
}

pub struct Owner {
    roots: PathBuf,
    keyring: PathBuf,
}

impl Owner {
    /// Creates the persistent directory under `data`, an empty keyring for
    /// this boot and the locator, both under `runtime`.
    pub fn prepare(runtime: &Path, data: &Path) -> io::Result<()> {
        let dir = data.join("fscrypt");
        fs::create_dir_all(&dir)?;
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        let mut locator = MAGIC.to_vec();
        locator.extend_from_slice(fs::canonicalize(dir)?.as_os_str().as_bytes());
        fs::write(runtime.join(KEYRING), b"")?;
        fs::write(runtime.join(LOCATOR), locator)
    }

    /// The owner guest-init prepared under `runtime`, if any.
    pub fn locate(runtime: &Path) -> io::Result<Option<Self>> {
        let mut bytes = Vec::new();
        match open(&runtime.join(LOCATOR), false) {
            Ok(mut file) => file.read_to_end(&mut bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let dir = bytes
            .strip_prefix(MAGIC)
            .filter(|dir| !dir.is_empty() && !dir.contains(&0))
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        Ok(Some(Self {
            roots: Path::new(OsStr::from_bytes(dir)).join("roots"),
            keyring: runtime.join(KEYRING),
        }))
    }

    /// Whether any directory of the filesystem was given a policy: until
    /// one is, no inode has one to read or inherit.
    pub fn has_roots(&self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.roots) {
            Ok(metadata) => Ok(metadata.len() > 0),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Records the guest path of a directory given a policy.
    pub fn add_root(&self, guest: &str) -> io::Result<()> {
        let mut file = open(&self.roots, true)?;
        let _lock = Lock::exclusive(&file)?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(format!("{guest}\n").as_bytes())
    }

    /// Adds `user`'s claim to a key (v2), or the key (v1, `user` None).
    /// Adding a key again succeeds.
    pub fn add_key(&self, spec: Spec, user: Option<u32>) -> io::Result<()> {
        self.change(|entries| {
            let spec = spec.encode();
            let index = match entries.iter().position(|entry| entry.spec == spec) {
                Some(index) => index,
                None => {
                    entries.push(Entry {
                        spec,
                        users: user.map(|_| Vec::new()),
                    });
                    entries.len() - 1
                }
            };
            if let (Some(users), Some(user)) = (&mut entries[index].users, user)
                && !users.contains(&user)
            {
                users.push(user);
            }
            Ok(())
        })
    }

    /// Drops `user`'s claim to a key (every user's with `all_users`), and
    /// the key once nobody else holds it.
    pub fn remove_key(&self, spec: Spec, user: u32, all_users: bool) -> io::Result<Removal> {
        self.change(|entries| {
            let spec = spec.encode();
            let Some(index) = entries.iter().position(|entry| entry.spec == spec) else {
                return Ok(Removal::NotAdded);
            };
            if let Some(users) = &mut entries[index].users {
                if all_users {
                    users.clear();
                } else if let Some(at) = users.iter().position(|added| *added == user) {
                    users.remove(at);
                } else {
                    return Ok(Removal::NotAdded);
                }
                if !users.is_empty() {
                    return Ok(Removal::OtherUsers);
                }
            }
            entries.remove(index);
            Ok(Removal::Removed)
        })
    }

    /// The key's state for `user`; None when it is not in the keyring.
    pub fn key_status(&self, spec: Spec, user: u32) -> io::Result<Option<Status>> {
        let mut file = open(&self.keyring, false)?;
        let _lock = Lock::shared(&file)?;
        let spec = spec.encode();
        Ok(read(&mut file)?
            .into_iter()
            .find(|entry| entry.spec == spec)
            .map(|entry| {
                let users = entry.users.unwrap_or_default();
                Status {
                    users: users.len() as u32,
                    added_by_self: users.contains(&user),
                }
            }))
    }

    fn change<T>(&self, apply: impl FnOnce(&mut Vec<Entry>) -> io::Result<T>) -> io::Result<T> {
        let mut file = open(&self.keyring, false)?;
        let _lock = Lock::exclusive(&file)?;
        let mut entries = read(&mut file)?;
        let result = apply(&mut entries)?;
        let text: String = entries
            .iter()
            .map(|entry| {
                let mut line = entry.spec.clone();
                for user in entry.users.iter().flatten() {
                    line.push_str(&format!(" {user}"));
                }
                line + "\n"
            })
            .collect();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(text.as_bytes())?;
        if unsafe { libc::ftruncate(file.as_raw_fd(), text.len() as libc::off_t) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result)
    }
}

/// One line per key: its encoded specifier, then for a v2 key ('i') the
/// uids that added it.
fn read(file: &mut PrivateFile) -> io::Result<Vec<Entry>> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut text)?;
    text.lines()
        .map(|line| {
            let mut fields = line.split(' ');
            let spec = fields.next().unwrap_or_default().to_string();
            let users = fields
                .map(|uid| {
                    uid.parse()
                        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
                })
                .collect::<io::Result<Vec<u32>>>()?;
            let users = spec.starts_with('i').then_some(users);
            Ok(Entry { spec, users })
        })
        .collect()
}

fn open(path: &Path, create: bool) -> io::Result<PrivateFile> {
    PrivateFile::allocate(|| {
        File::options()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    })
}

struct Lock(i32);

impl Lock {
    fn exclusive(file: &PrivateFile) -> io::Result<Self> {
        Self::take(file, libc::LOCK_EX)
    }
    fn shared(file: &PrivateFile) -> io::Result<Self> {
        Self::take(file, libc::LOCK_SH)
    }
    fn take(file: &PrivateFile, kind: i32) -> io::Result<Self> {
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), kind) } == 0 {
                return Ok(Self(file.as_raw_fd()));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0, libc::LOCK_UN) };
    }
}

fn hmac(key: &[u8], message: &[&[u8]]) -> [u8; 64] {
    let mut block = [0u8; 128];
    if key.len() > block.len() {
        block[..64].copy_from_slice(&Sha512::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha512::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha512::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// A v2 key's identifier (`fs/crypto/keyring.c`): HKDF-SHA512 of the raw
/// key, extracted with the all-zero salt and expanded with the info
/// "fscrypt\0" and context 1 (`HKDF_CONTEXT_KEY_IDENTIFIER`) to 16 bytes.
pub fn key_identifier(raw: &[u8]) -> [u8; 16] {
    let pseudorandom = hmac(&[0; 64], &[raw]);
    let block = hmac(&pseudorandom, &[b"fscrypt\0", &[1], &[1]]);
    block[..16].try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_identifier_is_hkdf_sha512_with_the_fscrypt_info() {
        // The same construction computed with Python's hmac and hashlib.
        assert_eq!(
            key_identifier(&[0x11; 64]),
            [
                0x8c, 0x0d, 0xb1, 0x23, 0x7b, 0xaf, 0x96, 0x86, 0x81, 0xeb, 0xa8, 0xc1, 0x23, 0x9f,
                0x13, 0x2e
            ]
        );
    }

    /// The users of keys are the syscall layer's tests (`sys/fscrypt.rs`).
    #[test]
    fn a_boot_empties_the_keyring_and_keeps_the_roots() {
        let dir = crate::scratch_dir("fscrypt-owner");
        let (runtime, data) = (dir.join("run"), dir.join("data"));
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&data).unwrap();
        Owner::prepare(&runtime, &data).unwrap();
        let owner = Owner::locate(&runtime).unwrap().unwrap();
        assert!(!owner.has_roots().unwrap());
        owner.add_root("/data/a").unwrap();
        let v1 = Spec::Descriptor([1; 8]);
        owner.add_key(v1, None).unwrap();
        assert!(owner.key_status(v1, 0).unwrap().is_some());
        Owner::prepare(&runtime, &data).unwrap();
        assert_eq!(owner.key_status(v1, 0).unwrap(), None);
        assert!(owner.has_roots().unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
