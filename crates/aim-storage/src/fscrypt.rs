//! The fscrypt state of the data filesystem (ADR 0016): its keyring and
//! whether any inode has a policy, shared by every guest process as the
//! kernel keeps them per superblock.
//!
//! guest-init prepares both when it mounts the data directory
//! ([`Owner::prepare`]): the keyring is empty at every boot, as a mounted
//! filesystem's is, and lives in the runtime directory; the marker
//! `policies`, created when a directory is first given a policy, persists
//! with the data. The locator `<runtime>/fscrypt-root` names the
//! persistent directory, as `fs-verity-root` names the verity store.
//! A change of the keyring is serialized by `flock` on its lock file and
//! replaces the keyring by a rename, so a writer killed mid-way leaves the
//! previous keyring whole.
//!
//! The keyring keeps key specifiers and, for v2 keys, the users who added
//! them; never a raw key. The data stays plaintext on the host (ADR 0016,
//! option B), and an identifier is all that tells keys apart.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha512};

use crate::private_fd::PrivateFile;

const LOCATOR: &str = "fscrypt-root";
const KEYRING: &str = "fscrypt-keyring";
const KEYRING_LOCK: &str = "fscrypt-keyring.lock";
const KEYRING_NEXT: &str = "fscrypt-keyring.next";
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
    marker: PathBuf,
    keyring: PathBuf,
    runtime: PathBuf,
    device: u64,
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
        for (name, bytes) in [
            (KEYRING, &b""[..]),
            (KEYRING_LOCK, b""),
            (LOCATOR, &locator),
        ] {
            let path = runtime.join(name);
            fs::write(&path, bytes)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
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
        let dir = Path::new(OsStr::from_bytes(dir));
        Ok(Some(Self {
            marker: dir.join("policies"),
            keyring: runtime.join(KEYRING),
            runtime: runtime.to_path_buf(),
            device: fs::symlink_metadata(dir)?.dev(),
        }))
    }

    /// The host device of the data filesystem: no inode elsewhere has a
    /// policy.
    pub fn device(&self) -> u64 {
        self.device
    }

    /// Whether any directory of the filesystem was given a policy: until
    /// one is, no inode has one to read or inherit.
    pub fn has_policies(&self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.marker) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Records that a directory is being given a policy.
    pub fn policy_set(&self) -> io::Result<()> {
        open(&self.marker, true).map(drop)
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
        let _lock = Lock::exclusive(&open(&self.runtime.join(KEYRING_LOCK), false)?)?;
        let mut entries = read(&mut open(&self.keyring, false)?)?;
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
        let next = self.runtime.join(KEYRING_NEXT);
        let mut file = open(&next, true)?;
        if unsafe { libc::ftruncate(file.as_raw_fd(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        file.write_all(text.as_bytes())?;
        fs::rename(next, &self.keyring)?;
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
    fn a_boot_empties_the_keyring_and_keeps_the_marker() {
        let dir = crate::scratch_dir("fscrypt-owner");
        let (runtime, data) = (dir.join("run"), dir.join("data"));
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&data).unwrap();
        Owner::prepare(&runtime, &data).unwrap();
        for name in [KEYRING, KEYRING_LOCK, LOCATOR] {
            let mode = fs::metadata(runtime.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
        let owner = Owner::locate(&runtime).unwrap().unwrap();
        assert_eq!(owner.device(), fs::metadata(&data).unwrap().dev());
        assert!(!owner.has_policies().unwrap());
        owner.policy_set().unwrap();
        let v1 = Spec::Descriptor([1; 8]);
        owner.add_key(v1, None).unwrap();
        assert!(owner.key_status(v1, 0).unwrap().is_some());
        Owner::prepare(&runtime, &data).unwrap();
        assert_eq!(owner.key_status(v1, 0).unwrap(), None);
        assert!(owner.has_policies().unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_writer_killed_mid_change_leaves_the_keyring_whole() {
        let dir = crate::scratch_dir("fscrypt-crash");
        Owner::prepare(&dir, &dir).unwrap();
        let owner = Owner::locate(&dir).unwrap().unwrap();
        let v2 = Spec::Identifier([7; 16]);
        owner.add_key(v2, Some(1000)).unwrap();
        // What a writer killed before its rename leaves: a partial next
        // keyring, which readers never see and the next change replaces.
        fs::write(dir.join(KEYRING_NEXT), b"i07070707 10").unwrap();
        let status = owner.key_status(v2, 1000).unwrap().unwrap();
        assert_eq!((status.users, status.added_by_self), (1, true));
        owner.add_key(v2, Some(1001)).unwrap();
        assert_eq!(owner.key_status(v2, 1001).unwrap().unwrap().users, 2);
        assert_eq!(
            owner.remove_key(v2, 1000, false).unwrap(),
            Removal::OtherUsers
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
