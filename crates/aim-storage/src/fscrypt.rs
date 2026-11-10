//! The fscrypt state of the data filesystem (ADR 0016): its keyring, its
//! policy roots and the secret that names entries of locked directories,
//! shared by every guest process as the kernel keeps them per superblock.
//!
//! guest-init prepares them when it mounts the data directory
//! ([`Owner::prepare`]): the keyring is empty at every boot, as a mounted
//! filesystem's is, and lives in the runtime directory; the table
//! `policies` of policy roots and the per-volume `secret` persist with the
//! data. The locator `<runtime>/fscrypt-root` names the persistent
//! directory, as `fs-verity-root` names the verity store. A change of the
//! keyring or the table is serialized by `flock` on the keyring's lock file
//! and replaces the file by a rename, so a writer killed mid-way leaves the
//! previous one whole.
//!
//! The keyring keeps key specifiers and, for v2 keys, the users who added
//! them; never a raw key. The data stays plaintext on the host (ADR 0016,
//! option B), and an identifier is all that tells keys apart.
//!
//! A policy root is an inode with a policy whose parent has none: a
//! directory given a policy, or an encrypted inode moved or linked out of
//! its tree. Each change publishes, in the shared page `fscrypt-state`, a
//! generation and how many keys the roots name are absent: while none is,
//! no directory is locked and lookups need not ask (the steady state once
//! user 0 is unlocked). A boot drops the roots that are gone, such as the
//! previous boot's `/data/per_boot`.

use std::ffi::{CStr, OsStr};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha512};

use crate::private_fd::PrivateFile;

const LOCATOR: &str = "fscrypt-root";
const KEYRING: &str = "fscrypt-keyring";
const KEYRING_LOCK: &str = "fscrypt-keyring.lock";
const KEYRING_NEXT: &str = "fscrypt-keyring.next";
const STATE: &str = "fscrypt-state";
const TABLE: &str = "policies";
const TABLE_NEXT: &str = "policies.next";
const CE_PROTECTOR: &str = "ce-protector";
const CE_PROTECTOR_NEXT: &str = "ce-protector.next";
const SECRET: &str = "secret";
const SECRET_NEXT: &str = "secret.next";
const MAGIC: &[u8] = b"AIMFSCRYPT01\0";
const PAGE: usize = 4096;
/// The host attribute holding an inode's `struct fscrypt_context`.
pub const CONTEXT: &CStr = c"dev.aim.fscrypt-context";
/// A context's nonce, and the policy flags `supported` checks.
const NONCE: usize = 16;
const PAD_MASK: u8 = 0x03;
pub const DIRECT_KEY: u8 = 0x04;
pub const IV_INO_LBLK_64: u8 = 0x08;
pub const IV_INO_LBLK_32: u8 = 0x10;

unsafe extern "C" {
    /// `fsgetpath(2)`: the path of an inode of a mounted volume.
    fn fsgetpath(buf: *mut libc::c_char, size: usize, fsid: *mut libc::fsid_t, id: u64) -> isize;
}

/// `struct fscrypt_key_specifier`: a v1 key by its descriptor, a v2 key by
/// its identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    Descriptor([u8; 8]),
    Identifier([u8; 16]),
}

impl Spec {
    /// The key a `struct fscrypt_context` names; None for a context that
    /// is not valid.
    pub fn of_context(context: &[u8]) -> Option<Self> {
        match (context.first(), context.len()) {
            (Some(1), 28) => Some(Spec::Descriptor(context[4..12].try_into().unwrap())),
            (Some(2), 40) => Some(Spec::Identifier(context[8..24].try_into().unwrap())),
            _ => None,
        }
    }

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

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    spec: String,
    /// None for a v1 key, which has no users.
    users: Option<Vec<u32>>,
}

/// A policy root: the key its policy names and its host inode number.
#[derive(Clone, PartialEq, Eq)]
struct Root {
    spec: String,
    inode: u64,
}

pub struct Owner {
    table: PathBuf,
    keyring: PathBuf,
    runtime: PathBuf,
    device: u64,
    secret: [u8; 32],
    fsid: libc::fsid_t,
    /// The shared page: the generation, then the number of absent keys.
    state: *const [AtomicU64; 2],
    /// The keyring's specifiers as this process last read them, with the
    /// generation then.
    added: Mutex<(u64, Vec<String>)>,
}

// SAFETY: the page is shared memory read and written only atomically.
unsafe impl Send for Owner {}
unsafe impl Sync for Owner {}

impl Drop for Owner {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.state as *mut _, PAGE) };
    }
}

/// The policy part of a context (`struct fscrypt_policy_v1/v2`), or None
/// for a context that is not valid (`fscrypt_context_is_valid`).
pub fn policy(context: &[u8]) -> Option<Vec<u8>> {
    match (context.first(), context.len()) {
        (Some(1), 28) => Some([&[0], &context[1..12]].concat()),
        (Some(2), 40) => Some(context[..24].to_vec()),
        _ => None,
    }
}

/// `fscrypt_supported_policy` for ext4 with 4096-byte blocks: ext4 has
/// stable inode numbers of 32 bits, which the IV_INO_LBLK flags need. The
/// host's inode numbers are stable, and no IV is derived from them, since
/// the bytes stay plaintext.
pub fn supported(policy: &[u8]) -> bool {
    let (contents, filenames, flags) = (policy[1], policy[2], policy[3]);
    let v1_modes = matches!((contents, filenames), (1, 4) | (5, 6) | (9, 9));
    // Only Adiantum has the IV size DIRECT_KEY needs, for both.
    let direct = flags & DIRECT_KEY == 0 || (contents, filenames) == (9, 9);
    if policy[0] == 0 {
        return v1_modes && direct && flags & !(PAD_MASK | DIRECT_KEY) == 0;
    }
    let modes = v1_modes || matches!((contents, filenames), (1, 10) | (7, 8));
    let exclusive = [DIRECT_KEY, IV_INO_LBLK_64, IV_INO_LBLK_32]
        .iter()
        .filter(|flag| flags & **flag != 0)
        .count();
    let log2_data_unit = policy[4];
    modes
        && exclusive <= 1
        && direct
        && flags & !(PAD_MASK | DIRECT_KEY | IV_INO_LBLK_64 | IV_INO_LBLK_32) == 0
        && (log2_data_unit == 0
            || (9..=12).contains(&log2_data_unit)
                && (log2_data_unit == 12 || flags & IV_INO_LBLK_32 == 0))
        && policy[5..8] == [0; 3]
}

/// The context of the host inode `path`, none if it has no policy.
pub fn context(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut context = [0u8; 40];
    let n = unsafe {
        libc::getxattr(
            path.as_ptr(),
            CONTEXT.as_ptr(),
            context.as_mut_ptr().cast(),
            context.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if n < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOATTR) => Ok(None),
            _ => Err(error),
        };
    }
    Ok(Some(context[..n as usize].to_vec()))
}

/// Stores `context` on the host inode `path`, a fresh nonce in it. Setting
/// an attribute needs write access, which a file's host mode may deny its
/// owner for a moment.
fn store(path: &Path, context: &[u8]) -> io::Result<()> {
    let mut context = context.to_vec();
    let at = context.len() - NONCE;
    unsafe { libc::arc4random_buf(context[at..].as_mut_ptr().cast(), NONCE) };
    let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let set = || {
        let set = unsafe {
            libc::setxattr(
                name.as_ptr(),
                CONTEXT.as_ptr(),
                context.as_ptr().cast(),
                context.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if set < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    match set() {
        Err(error) if error.raw_os_error() == Some(libc::EACCES) => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Err(error);
            }
            let mode = metadata.permissions().mode();
            fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200))?;
            let result = set();
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
            result
        }
        result => result,
    }
}

fn withdraw(path: &Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    if unsafe { libc::removexattr(path.as_ptr(), CONTEXT.as_ptr(), libc::XATTR_NOFOLLOW) } < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOATTR) {
            return Err(error);
        }
    }
    Ok(())
}

/// `fscrypt_prepare_new_inode` for an inode a host-side owner made (init's
/// builtins): `child`, just created in `parent`, takes the parent's policy
/// with a nonce of its own.
pub fn inherit(parent: &Path, child: &Path) -> io::Result<()> {
    let Some(context) = context(parent)? else {
        return Ok(());
    };
    let kind = fs::symlink_metadata(child)?.file_type();
    if kind.is_dir() || kind.is_file() || kind.is_symlink() {
        store(child, &context)?;
    }
    Ok(())
}

impl Owner {
    /// Creates the persistent directory and its secret under `data`, and
    /// an empty keyring, the shared page and the locator under `runtime`,
    /// then drops the policy roots that are gone.
    pub fn prepare(runtime: &Path, data: &Path) -> io::Result<()> {
        let dir = data.join("fscrypt");
        fs::create_dir_all(&dir)?;
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        let dir = fs::canonicalize(dir)?;
        let secret = dir.join(SECRET);
        if !secret.exists() {
            // Written whole, then linked: a boot killed mid-way leaves
            // either no secret or all of it.
            let next = dir.join(SECRET_NEXT);
            let mut bytes = [0u8; 32];
            unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
            let mut file = open(&next, true)?;
            if unsafe { libc::ftruncate(file.as_raw_fd(), 0) } < 0 {
                return Err(io::Error::last_os_error());
            }
            file.write_all(&bytes)?;
            file.sync_all()?;
            match fs::hard_link(&next, &secret) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
                _ => fs::remove_file(next)?,
            }
        }
        read_secret(&secret)?;
        let mut locator = MAGIC.to_vec();
        locator.extend_from_slice(dir.as_os_str().as_bytes());
        for (name, bytes) in [
            (KEYRING, &b""[..]),
            (KEYRING_LOCK, b""),
            (STATE, &[0; PAGE]),
            (LOCATOR, &locator),
        ] {
            let path = runtime.join(name);
            fs::write(&path, bytes)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        let owner =
            Self::locate(runtime)?.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        owner.change(|_, _| Ok(()))
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
        let secret = read_secret(&dir.join(SECRET))?;
        let mut fsid: libc::statfs = unsafe { std::mem::zeroed() };
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes())?;
        if unsafe { libc::statfs(path.as_ptr(), &mut fsid) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let page = open(&runtime.join(STATE), false)?;
        let state = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                page.as_raw_fd(),
                0,
            )
        };
        if state == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(Self {
            table: dir.join(TABLE),
            keyring: runtime.join(KEYRING),
            runtime: runtime.to_path_buf(),
            device: fs::symlink_metadata(dir)?.dev(),
            secret,
            fsid: fsid.f_fsid,
            state: state.cast(),
            added: Mutex::new((0, Vec::new())),
        }))
    }

    /// The host device of the data filesystem: no inode elsewhere has a
    /// policy.
    pub fn device(&self) -> u64 {
        self.device
    }

    /// Marks the filesystem as encrypted (`mount_all` of a `/data` with
    /// `fileencryption=`) before any directory gets a policy: from then on
    /// [`has_policies`](Self::has_policies) answers true, so a boot that
    /// ends between making the first encrypted directory and recording its
    /// policy is not taken for one from before file-based encryption.
    pub fn encrypt(&self) -> io::Result<()> {
        let _lock = Lock::exclusive(&open(&self.runtime.join(KEYRING_LOCK), false)?)?;
        if self.has_policies()? {
            return Ok(());
        }
        replace(&self.table.with_file_name(TABLE_NEXT), &self.table, "")
    }

    /// Records `name` as what protects the users' CE keys of the
    /// filesystem (the LockSettings of the image that first mounts it),
    /// unless something is already: returns that one if it is another.
    pub fn claim_ce_protection(&self, name: &str) -> io::Result<Option<String>> {
        let path = self.table.with_file_name(CE_PROTECTOR);
        let mut recorded = String::new();
        match open(&path, false) {
            Ok(mut file) => file.read_to_string(&mut recorded)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                replace(&self.table.with_file_name(CE_PROTECTOR_NEXT), &path, name)?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok((recorded != name).then_some(recorded))
    }

    /// Whether the filesystem is encrypted ([`encrypt`](Self::encrypt), or
    /// a directory was given a policy): until it is, no inode has a policy
    /// to read or inherit.
    pub fn has_policies(&self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.table) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Records the policy root `inode`, whose policy names `spec`.
    pub fn policy_root(&self, spec: Spec, inode: u64) -> io::Result<()> {
        self.change(|_, roots| {
            let root = Root {
                spec: spec.encode(),
                inode,
            };
            if !roots.contains(&root) {
                roots.push(root);
            }
            Ok(())
        })
    }

    /// `FS_IOC_SET_ENCRYPTION_POLICY` by a host-side owner with root's
    /// capabilities (init's `FscryptSetDirectoryPolicy`, which may name a
    /// key it did not add): `dir` gets `policy` (a `struct
    /// fscrypt_policy_v1` or `_v2`) if it has none, which needs it to be an
    /// empty directory; one it has already must be the same (EEXIST).
    pub fn set_policy(&self, dir: &Path, policy: &[u8]) -> io::Result<()> {
        let error = io::Error::from_raw_os_error;
        let new = match (policy.first(), policy.len()) {
            (Some(0), 12) => [&[1], &policy[1..], &[0; NONCE][..]].concat(),
            (Some(2), 24) => [policy, &[0; NONCE][..]].concat(),
            _ => return Err(error(libc::EINVAL)),
        };
        let metadata = fs::symlink_metadata(dir)?;
        if let Some(existing) = context(dir)? {
            if self::policy(&existing).as_deref() != Some(policy) {
                return Err(error(libc::EEXIST));
            }
            // A boot that ended between storing the policy and recording
            // it leaves the root to record now.
            let spec = Spec::of_context(&existing).ok_or_else(|| error(libc::EINVAL))?;
            let recorded = self
                .roots()?
                .iter()
                .any(|root| root.inode == metadata.ino());
            if recorded || dir.parent().map(context).transpose()?.flatten().is_some() {
                return Ok(());
            }
            return self.policy_root(spec, metadata.ino());
        }
        if !metadata.is_dir() {
            return Err(error(libc::ENOTDIR));
        }
        let empty = || Ok::<_, io::Error>(fs::read_dir(dir)?.next().is_none());
        if !empty()? {
            return Err(error(libc::ENOTEMPTY));
        }
        if !supported(policy) {
            return Err(error(libc::EINVAL));
        }
        store(dir, &new)?;
        // A guest process may have created an entry meanwhile: as on Linux
        // the directory was not empty, so it and what inherited its policy
        // lose it again.
        if !empty()? {
            withdraw(dir)?;
            for entry in fs::read_dir(dir)? {
                withdraw(&entry?.path())?;
            }
            return Err(error(libc::ENOTEMPTY));
        }
        let spec = Spec::of_context(&new).ok_or_else(|| error(libc::EINVAL))?;
        self.policy_root(spec, metadata.ino()).inspect_err(|_| {
            let _ = withdraw(dir);
        })
    }

    /// Whether a key some policy root names is absent: until one is, no
    /// directory is locked.
    pub fn locking(&self) -> bool {
        unsafe { (*self.state)[1].load(Ordering::Acquire) != 0 }
    }

    /// Whether the key is in the keyring, for any user.
    pub fn has_key(&self, spec: Spec) -> io::Result<bool> {
        let generation = unsafe { (*self.state)[0].load(Ordering::Acquire) };
        let mut added = self.added.lock().unwrap();
        if added.0 != generation {
            let specs = read(&mut open(&self.keyring, false)?)?;
            *added = (
                generation,
                specs.into_iter().map(|entry| entry.spec).collect(),
            );
        }
        Ok(added.1.contains(&spec.encode()))
    }

    /// Enciphers, or deciphers, `bytes` (a name in a directory, or a
    /// symlink's target) of the inode with `nonce`: the stand-in for
    /// fscrypt's ciphertext, deterministic per inode as Linux's is and keyed
    /// by the volume's secret, so only the owner can decode it.
    pub fn cipher(&self, nonce: &[u8], bytes: &mut [u8]) {
        for (block, chunk) in bytes.chunks_mut(64).enumerate() {
            let stream = hmac(
                &self.secret,
                &[b"fscrypt name\0", nonce, &(block as u32).to_le_bytes()],
            );
            chunk
                .iter_mut()
                .zip(stream)
                .for_each(|(byte, key)| *byte ^= key);
        }
    }

    /// Adds `user`'s claim to a key (v2), or the key (v1, `user` None).
    /// Adding a key again succeeds.
    pub fn add_key(&self, spec: Spec, user: Option<u32>) -> io::Result<()> {
        self.change(|entries, _| {
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
        self.change(|entries, _| {
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

    /// Applies a change to the keyring and the roots, after dropping the
    /// roots that are gone, then publishes the generation and the number of
    /// absent keys if anything changed (or nothing was published since the
    /// boot, generation 0).
    fn change<T>(
        &self,
        apply: impl FnOnce(&mut Vec<Entry>, &mut Vec<Root>) -> io::Result<T>,
    ) -> io::Result<T> {
        let _lock = Lock::exclusive(&open(&self.runtime.join(KEYRING_LOCK), false)?)?;
        let mut entries = read(&mut open(&self.keyring, false)?)?;
        let keys = entries.clone();
        let before = self.roots()?;
        let mut roots = Vec::new();
        for root in &before {
            if live(root, self.fsid)? {
                roots.push(root.clone());
            }
        }
        let result = apply(&mut entries, &mut roots)?;
        if entries != keys {
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
            replace(&self.runtime.join(KEYRING_NEXT), &self.keyring, &text)?;
        }
        if roots != before {
            let text: String = roots
                .iter()
                .map(|root| format!("{} {}\n", root.spec, root.inode))
                .collect();
            replace(&self.table.with_file_name(TABLE_NEXT), &self.table, &text)?;
        }
        let state = unsafe { &*self.state };
        if entries == keys && roots == before && state[0].load(Ordering::Acquire) != 0 {
            return Ok(result);
        }
        let mut absent: Vec<&String> = roots
            .iter()
            .map(|root| &root.spec)
            .filter(|spec| !entries.iter().any(|entry| &entry.spec == *spec))
            .collect();
        absent.sort();
        absent.dedup();
        state[1].store(absent.len() as u64, Ordering::Release);
        state[0].fetch_add(1, Ordering::AcqRel);
        Ok(result)
    }

    /// Drops the root `inode` if it is one and is gone (its last link was
    /// just removed).
    pub fn removed(&self, inode: u64) -> io::Result<()> {
        if self.roots()?.iter().any(|root| root.inode == inode) {
            self.change(|_, _| Ok(()))?;
        }
        Ok(())
    }

    /// The table of policy roots: one line each, its key's encoded
    /// specifier and its inode number.
    fn roots(&self) -> io::Result<Vec<Root>> {
        let mut text = String::new();
        match open(&self.table, false) {
            Ok(mut file) => file.read_to_string(&mut text)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        text.lines()
            .map(|line| {
                let (spec, inode) = line
                    .split_once(' ')
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
                let inode = inode
                    .parse()
                    .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
                Ok(Root {
                    spec: spec.to_string(),
                    inode,
                })
            })
            .collect()
    }
}

/// The volume's secret; one that is not 32 bytes is corrupt.
fn read_secret(path: &Path) -> io::Result<[u8; 32]> {
    let mut secret = Vec::new();
    open(path, false)?.read_to_end(&mut secret)?;
    secret.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the fscrypt secret {} is not 32 bytes", path.display()),
        )
    })
}

/// Writes `text` to `next` and renames it over `path`.
fn replace(next: &Path, path: &Path, text: &str) -> io::Result<()> {
    let mut file = open(next, true)?;
    if unsafe { libc::ftruncate(file.as_raw_fd(), 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    file.write_all(text.as_bytes())?;
    fs::rename(next, path)
}

/// Whether the root's inode still exists on the volume `fsid` with a
/// policy naming its key (the inode numbers of APFS are not reused).
fn live(root: &Root, mut fsid: libc::fsid_t) -> io::Result<bool> {
    let mut path = [0 as libc::c_char; libc::PATH_MAX as usize];
    if unsafe { fsgetpath(path.as_mut_ptr(), path.len(), &mut fsid, root.inode) } < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOENT) => Ok(false),
            _ => Err(error),
        };
    }
    let mut context = [0u8; 40];
    let n = unsafe {
        libc::getxattr(
            path.as_ptr(),
            CONTEXT.as_ptr(),
            context.as_mut_ptr().cast(),
            context.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if n < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOATTR | libc::ERANGE) => Ok(false),
            _ => Err(error),
        };
    }
    Ok(Spec::of_context(&context[..n as usize]).is_some_and(|spec| spec.encode() == root.spec))
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
    fn a_boot_empties_the_keyring_and_drops_the_roots_that_are_gone() {
        let dir = crate::scratch_dir("fscrypt-owner");
        let (runtime, data) = (dir.join("run"), dir.join("data"));
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(data.join("root")).unwrap();
        Owner::prepare(&runtime, &data).unwrap();
        for name in [KEYRING, KEYRING_LOCK, LOCATOR, STATE] {
            let mode = fs::metadata(runtime.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
        let owner = Owner::locate(&runtime).unwrap().unwrap();
        assert_eq!(owner.device(), fs::metadata(&data).unwrap().dev());
        assert!(!owner.has_policies().unwrap());
        assert!(!owner.locking());
        // A v1 context naming the descriptor [1; 8], as the layer stores it.
        let root = data.join("root");
        let context = [&[1u8, 1, 4, 0][..], &[1; 8], &[0; 16]].concat();
        let path = std::ffi::CString::new(root.as_os_str().as_bytes()).unwrap();
        let set = unsafe {
            libc::setxattr(
                path.as_ptr(),
                CONTEXT.as_ptr(),
                context.as_ptr().cast(),
                context.len(),
                0,
                0,
            )
        };
        assert_eq!(set, 0);
        let v1 = Spec::Descriptor([1; 8]);
        let inode = fs::metadata(&root).unwrap().ino();
        owner.policy_root(v1, inode).unwrap();
        assert!(owner.has_policies().unwrap());
        assert!(owner.locking());
        assert!(!owner.has_key(v1).unwrap());
        owner.add_key(v1, None).unwrap();
        assert!(!owner.locking());
        assert!(owner.has_key(v1).unwrap());
        // The next boot: the keyring is empty, the root is kept.
        Owner::prepare(&runtime, &data).unwrap();
        assert_eq!(owner.key_status(v1, 0).unwrap(), None);
        assert!(!owner.has_key(v1).unwrap());
        assert!(owner.locking());
        // A root removed during the boot (as init removes the previous
        // boot's /data/per_boot) is dropped, and with it the absent key.
        let generation = unsafe { (*owner.state)[0].load(Ordering::Acquire) };
        owner.removed(inode + 1).unwrap();
        assert_eq!(
            unsafe { (*owner.state)[0].load(Ordering::Acquire) },
            generation
        );
        fs::remove_dir(&root).unwrap();
        owner.removed(inode).unwrap();
        assert!(owner.has_policies().unwrap());
        assert!(!owner.locking());
        Owner::prepare(&runtime, &data).unwrap();
        assert!(!owner.locking());
        // The secret persists, so names stay stable across boots.
        let mut name = *b"name";
        owner.cipher(&[0; 16], &mut name);
        let again = Owner::locate(&runtime).unwrap().unwrap();
        again.cipher(&[0; 16], &mut name);
        assert_eq!(&name, b"name");
        fs::remove_dir_all(dir).unwrap();
    }

    /// A policy stored on a directory whose recording a boot did not
    /// reach is recorded when it is set again, and only for a root.
    #[test]
    fn a_policy_set_again_records_its_root() {
        let dir = crate::scratch_dir("fscrypt-rerecord");
        let (runtime, data) = (dir.join("run"), dir.join("data"));
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(data.join("misc/vold")).unwrap();
        Owner::prepare(&runtime, &data).unwrap();
        let owner = Owner::locate(&runtime).unwrap().unwrap();
        owner.encrypt().unwrap();
        assert!(owner.has_policies().unwrap());
        assert!(owner.roots().unwrap().is_empty());
        let policy = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16]].concat();
        let misc = data.join("misc");
        store(&misc, &[&policy[..], &[0; NONCE]].concat()).unwrap();
        inherit(&misc, &misc.join("vold")).unwrap();
        owner.set_policy(&misc.join("vold"), &policy).unwrap();
        assert!(owner.roots().unwrap().is_empty());
        owner.set_policy(&misc, &policy).unwrap();
        let roots = owner.roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].inode, fs::metadata(&misc).unwrap().ino());
        assert!(owner.locking());
        owner.set_policy(&misc, &policy).unwrap();
        assert!(owner.roots().unwrap() == roots);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_secret_is_whole_or_absent_and_a_corrupt_one_fails_the_boot() {
        let dir = crate::scratch_dir("fscrypt-secret");
        // A boot killed before linking the secret left only the next one.
        fs::create_dir_all(dir.join("fscrypt")).unwrap();
        fs::write(dir.join("fscrypt").join(SECRET_NEXT), b"short").unwrap();
        Owner::prepare(&dir, &dir).unwrap();
        assert_eq!(
            fs::read(dir.join("fscrypt").join(SECRET)).unwrap().len(),
            32
        );
        assert!(!dir.join("fscrypt").join(SECRET_NEXT).exists());
        fs::write(dir.join("fscrypt").join(SECRET), b"short").unwrap();
        let error = Owner::prepare(&dir, &dir).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
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
