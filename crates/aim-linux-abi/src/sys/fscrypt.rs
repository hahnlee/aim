//! fscrypt on the data filesystem (Linux v6.12 `fs/crypto/`,
//! `Documentation/filesystems/fscrypt.rst`; ADR 0016, option B): the
//! keyring ioctls, encryption policies set on empty directories and
//! inherited by what is created below them, and their queries. The bytes
//! stay plaintext on the host; confidentiality at rest is the Mac's.
//!
//! An inode's `struct fscrypt_context` (its policy and its own nonce) is
//! the host attribute `dev.aim.fscrypt-context`, as ext4 keeps it in an
//! xattr. The keyring and the table of policy roots belong to the owner
//! guest-init prepares (`aim_storage::fscrypt`), shared by every process.
//! Until a directory of the filesystem has a policy, nothing here reads an
//! attribute.
//!
//! Not served yet: `FS_ENCRYPT_FL`, `STATX_ATTR_ENCRYPTED` and EXDEV for a
//! link or rename across policies (#2845), locked directories (ENOKEY,
//! no-key names, #2846).

use std::ffi::CStr;
use std::os::fd::AsRawFd;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use aim_storage::fscrypt::{Owner, Removal, Spec};

use super::attrs::{self, Host};
use super::cred::Identity;
use super::fsops::{fd_guest, inode_mutation, metadata_fd_copy, writable_fd_metadata};
use crate::errno::{self, Errno};

const SET_POLICY: u64 = 0x800c6613;
const GET_POLICY: u64 = 0x400c6615;
const GET_POLICY_EX: u64 = 0xc0096616;
const ADD_KEY: u64 = 0xc0506617;
const REMOVE_KEY: u64 = 0xc0406618;
const REMOVE_KEY_ALL_USERS: u64 = 0xc0406619;
const GET_KEY_STATUS: u64 = 0xc080661a;
const GET_NONCE: u64 = 0x8010661b;

const CONTEXT: &CStr = c"dev.aim.fscrypt-context";
const NONCE: usize = 16;
const PAD_MASK: u8 = 0x03;
const DIRECT_KEY: u8 = 0x04;
const IV_INO_LBLK_64: u8 = 0x08;
const IV_INO_LBLK_32: u8 = 0x10;
const CAP_FOWNER: u32 = 3;
const CAP_SYS_ADMIN: u32 = 21;
const EOVERFLOW: Errno = 75;
const ENOKEY: Errno = 126;
const ENOTEMPTY: Errno = 39;

fn io(error: std::io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}

fn owner() -> Result<Option<&'static Owner>, Errno> {
    static OWNER: OnceLock<Result<Option<Owner>, Errno>> = OnceLock::new();
    let Some(runtime) = crate::vfs::runtime_dir() else {
        return Ok(None);
    };
    OWNER
        .get_or_init(|| Owner::locate(runtime).map_err(io))
        .as_ref()
        .map(Option::as_ref)
        .map_err(|error| *error)
}

/// Whether any directory has a policy. Roots are never forgotten, so a
/// process asks the owner only until one exists.
fn active() -> Result<bool, Errno> {
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    if ACTIVE.load(Ordering::Relaxed) {
        return Ok(true);
    }
    let Some(owner) = owner()? else {
        return Ok(false);
    };
    let active = owner.has_roots().map_err(io)?;
    ACTIVE.store(active, Ordering::Relaxed);
    Ok(active)
}

/// The policy part of a context (`struct fscrypt_policy_v1/v2`), or None
/// for a context that is not valid (`fscrypt_context_is_valid`).
fn policy(context: &[u8]) -> Option<Vec<u8>> {
    match (context.first(), context.len()) {
        (Some(1), 28) => Some([&[0], &context[1..12]].concat()),
        (Some(2), 40) => Some(context[..24].to_vec()),
        _ => None,
    }
}

fn context(host: Host) -> Option<Vec<u8>> {
    let mut b = [0u8; 40];
    attrs::get(host, CONTEXT, &mut b).map(|n| b[..n].to_vec())
}

fn host_type(host: Host) -> Result<u16, Errno> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe {
        match host {
            Host::Fd(fd) => libc::fstat(fd, &mut st),
            Host::Path(path) | Host::Image(path) => libc::lstat(path.as_ptr(), &mut st),
        }
    };
    if result < 0 {
        return Err(errno::last());
    }
    Ok(st.st_mode & libc::S_IFMT)
}

/// Only regular files, directories and symlinks are encrypted.
fn encryptable(kind: u16) -> bool {
    matches!(kind, libc::S_IFREG | libc::S_IFDIR | libc::S_IFLNK)
}

fn store(host: Host, context: &[u8]) -> Result<(), Errno> {
    if attrs::set(host, CONTEXT, context)? {
        Ok(())
    } else {
        Err(errno::EOPNOTSUPP)
    }
}

/// `context` with a fresh nonce.
fn renewed(context: &[u8]) -> Vec<u8> {
    let mut context = context.to_vec();
    let at = context.len() - NONCE;
    unsafe { libc::arc4random_buf(context[at..].as_mut_ptr().cast(), NONCE) };
    context
}

/// `fscrypt_prepare_new_inode`: `child`, just created in `parent`, takes
/// the parent's policy with a nonce of its own.
pub(super) fn inherit(parent: Host, child: Host) -> Result<(), Errno> {
    if !active()? {
        return Ok(());
    }
    let Some(context) = context(parent) else {
        return Ok(());
    };
    if !encryptable(host_type(child)?) {
        return Ok(());
    }
    store(child, &renewed(&context))
}

/// `fscrypt_supported_policy` for ext4 with 4096-byte blocks and 32-bit
/// inode numbers.
fn supported(policy: &[u8]) -> bool {
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

fn empty_directory(fd: i32) -> Result<bool, Errno> {
    let _own = super::fork::spawn::own_fds();
    let copy = unsafe {
        libc::openat(
            fd,
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if copy < 0 {
        return Err(errno::last());
    }
    let dir = unsafe { libc::fdopendir(copy) };
    if dir.is_null() {
        let error = errno::last();
        unsafe { libc::close(copy) };
        return Err(error);
    }
    let mut empty = true;
    loop {
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            empty = false;
            break;
        }
    }
    unsafe { libc::closedir(dir) };
    Ok(empty)
}

fn capable(id: &Identity, cap: u32) -> bool {
    id.cap_eff & (1 << cap) != 0
}

/// `fscrypt_ioctl_set_policy`.
fn set_policy(owner: &Owner, fd: i32, arg: u64, id: &Identity) -> Result<i64, Errno> {
    let version = super::user_memory::read_exact(arg, 1)?[0];
    let size = match version {
        0 => 12,
        2 => 24,
        _ => return Err(errno::EINVAL),
    };
    let wanted = super::user_memory::read_exact(arg, size)?;
    let anchor = metadata_fd_copy(fd).map_err(|error| (-error) as Errno)?;
    let fd = anchor.as_raw_fd();
    let guest = fd_guest(fd).unwrap_or_default();
    let result = inode_mutation(Host::Fd(fd), &guest, true, |stat| {
        if stat.st_uid != id.uid[attrs::FS] && !capable(id, CAP_FOWNER) {
            return -(errno::EACCES as i64);
        }
        if let Err(error) = writable_fd_metadata(fd, stat) {
            return error;
        }
        let change = || {
            if let Some(existing) = context(Host::Fd(fd)) {
                return match policy(&existing) {
                    Some(existing) if existing == wanted => Ok(0),
                    _ => Err(errno::EEXIST),
                };
            }
            if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
                return Err(errno::ENOTDIR);
            }
            if !empty_directory(fd)? {
                return Err(ENOTEMPTY);
            }
            if !supported(&wanted) {
                return Err(errno::EINVAL);
            }
            // `fscrypt_verify_key_added`: a v2 policy names a key its
            // setter added.
            if version == 2 {
                let spec = Spec::Identifier(wanted[8..24].try_into().unwrap());
                let added = owner.key_status(spec, id.uid[attrs::FS]).map_err(io)?;
                if !added.is_some_and(|status| status.added_by_self) && !capable(id, CAP_FOWNER) {
                    return Err(ENOKEY);
                }
            }
            let context = match version {
                0 => [&[1], &wanted[1..], &[0; NONCE][..]].concat(),
                _ => [&wanted[..], &[0; NONCE][..]].concat(),
            };
            owner.add_root(&guest).map_err(io)?;
            store(Host::Fd(fd), &renewed(&context))?;
            Ok(0)
        };
        change().unwrap_or_else(|error: Errno| -(error as i64))
    });
    if result < 0 {
        Err((-result) as Errno)
    } else {
        Ok(result)
    }
}

fn existing(fd: i32) -> Result<Vec<u8>, Errno> {
    if !active()? {
        return Err(errno::ENODATA);
    }
    context(Host::Fd(fd)).ok_or(errno::ENODATA)
}

/// `struct fscrypt_key_specifier`: type, reserved, then the descriptor or
/// identifier in a 32-byte union.
fn spec(bytes: &[u8]) -> Result<Spec, Errno> {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    match (word(0), word(4)) {
        (1, 0) => Ok(Spec::Descriptor(bytes[8..16].try_into().unwrap())),
        (2, 0) => Ok(Spec::Identifier(bytes[8..24].try_into().unwrap())),
        _ => Err(errno::EINVAL),
    }
}

fn zero(bytes: &[u8]) -> Result<(), Errno> {
    if bytes.iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(errno::EINVAL)
    }
}

/// `fscrypt_ioctl_add_key`: a v2 key is known by the identifier derived
/// from it, which is returned, and held by each user who adds it.
fn add_key(owner: &Owner, arg: u64, id: &Identity) -> Result<i64, Errno> {
    let argument = super::user_memory::read_exact(arg, 80)?;
    let spec = spec(&argument[..40])?;
    zero(&argument[48..80])?;
    if matches!(spec, Spec::Descriptor(_)) && !capable(id, CAP_SYS_ADMIN) {
        return Err(errno::EACCES);
    }
    let word = |at: usize| u32::from_le_bytes(argument[at..at + 4].try_into().unwrap());
    let (raw_size, key_id) = (word(40), word(44));
    if key_id != 0 {
        // A "fscrypt-provisioning" key by serial: no process keyring holds
        // one, as add_key(2) is not served.
        return Err(if raw_size != 0 { errno::EINVAL } else { ENOKEY });
    }
    if !(16..=64).contains(&raw_size) {
        return Err(errno::EINVAL);
    }
    let raw = super::user_memory::read_exact(arg + 80, raw_size as usize)?;
    match spec {
        Spec::Descriptor(_) => owner.add_key(spec, None).map_err(io)?,
        Spec::Identifier(_) => {
            let identifier = aim_storage::fscrypt::key_identifier(&raw);
            owner
                .add_key(Spec::Identifier(identifier), Some(id.uid[attrs::FS]))
                .map_err(io)?;
            super::user_memory::write_exact(arg + 8, &identifier)?;
        }
    }
    Ok(0)
}

/// `fscrypt_ioctl_remove_key[_all_users]`. A removed key is gone at once:
/// files stay readable, so none keeps it busy.
fn remove_key(owner: &Owner, arg: u64, all_users: bool, id: &Identity) -> Result<i64, Errno> {
    let argument = super::user_memory::read_exact(arg, 64)?;
    let spec = spec(&argument[..40])?;
    zero(&argument[44..64])?;
    if (matches!(spec, Spec::Descriptor(_)) || all_users) && !capable(id, CAP_SYS_ADMIN) {
        return Err(errno::EACCES);
    }
    let flags: u32 = match owner
        .remove_key(spec, id.uid[attrs::FS], all_users)
        .map_err(io)?
    {
        Removal::NotAdded => return Err(ENOKEY),
        Removal::OtherUsers => 0x2,
        Removal::Removed => 0,
    };
    super::user_memory::write_exact(arg + 40, &flags.to_le_bytes())?;
    Ok(0)
}

/// `fscrypt_ioctl_get_key_status`.
fn key_status(owner: &Owner, arg: u64, id: &Identity) -> Result<i64, Errno> {
    let argument = super::user_memory::read_exact(arg, 64)?;
    let spec = spec(&argument[..40])?;
    zero(&argument[40..64])?;
    let mut out = [0u8; 64];
    let (status, flags, users) = match owner.key_status(spec, id.uid[attrs::FS]).map_err(io)? {
        None => (1u32, 0u32, 0),
        Some(status) => (2, status.added_by_self as u32, status.users),
    };
    for (at, word) in [status, flags, users].into_iter().enumerate() {
        out[at * 4..at * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    super::user_memory::write_exact(arg + 64, &out)?;
    Ok(0)
}

/// The request's answer if it is an fscrypt ioctl on a file of a
/// filesystem that has fscrypt (ext4 here, writable).
pub(super) fn ioctl(fd: i32, request: u64, arg: u64) -> Option<i64> {
    ioctl_as(fd, request, arg, &super::cred::current())
}

fn ioctl_as(fd: i32, request: u64, arg: u64, id: &Identity) -> Option<i64> {
    if !matches!(
        request,
        SET_POLICY
            | GET_POLICY
            | GET_POLICY_EX
            | ADD_KEY
            | REMOVE_KEY
            | REMOVE_KEY_ALL_USERS
            | GET_KEY_STATUS
            | GET_NONCE
    ) || !super::fileattr::ext4(fd)
    {
        return None;
    }
    let owner = match owner() {
        Ok(Some(owner)) => owner,
        Ok(None) => return None,
        Err(error) => return Some(-(error as i64)),
    };
    let result = match request {
        SET_POLICY => set_policy(owner, fd, arg, id),
        GET_POLICY => existing(fd).and_then(|context| {
            let policy = policy(&context).ok_or(errno::EINVAL)?;
            if policy[0] != 0 {
                return Err(EOVERFLOW);
            }
            super::user_memory::write_exact(arg, &policy).map(|_| 0)
        }),
        GET_POLICY_EX => super::user_memory::read_exact(arg, 8).and_then(|size| {
            let available = u64::from_le_bytes(size.try_into().unwrap());
            let policy = policy(&existing(fd)?).ok_or(errno::EINVAL)?;
            if available < policy.len() as u64 {
                return Err(EOVERFLOW);
            }
            let out = [&(policy.len() as u64).to_le_bytes()[..], &policy].concat();
            super::user_memory::write_exact(arg, &out).map(|_| 0)
        }),
        GET_NONCE => existing(fd).and_then(|context| {
            policy(&context).ok_or(errno::EINVAL)?;
            super::user_memory::write_exact(arg, &context[context.len() - NONCE..]).map(|_| 0)
        }),
        ADD_KEY => add_key(owner, arg, id),
        REMOVE_KEY => remove_key(owner, arg, false, id),
        REMOVE_KEY_ALL_USERS => remove_key(owner, arg, true, id),
        _ => key_status(owner, arg, id),
    };
    Some(result.unwrap_or_else(|error| -(error as i64)))
}

/// Ported from xfstests' encryption group (generic/395, 396 and 399) and
/// `fscrypt.rst`'s error table.
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::path::Path;

    const AT: u64 = crate::vfs::LINUX_AT_FDCWD as u64;
    const BASE: &str = "/data/fscrypt-tests";

    fn identity(uid: u32, cap_eff: u64) -> Identity {
        Identity {
            uid: [uid; 4],
            cap_eff,
            ..Default::default()
        }
    }
    fn root() -> Identity {
        identity(0, !0)
    }
    fn guest(name: &str) -> CString {
        CString::new(format!("{BASE}/{name}")).unwrap()
    }
    fn open(view: &Path, name: &str) -> std::fs::File {
        std::fs::File::open(view.join("data/fscrypt-tests").join(name)).unwrap()
    }
    fn call(file: &std::fs::File, request: u64, arg: &mut [u8], id: &Identity) -> i64 {
        ioctl_as(file.as_raw_fd(), request, arg.as_mut_ptr() as u64, id).unwrap()
    }
    fn mkdir(name: &str) {
        let path = guest(name);
        let args = [AT, path.as_ptr() as u64, 0o755, 0, 0, 0];
        assert_eq!(super::super::fsops::mkdirat(args), 0, "{name}");
    }
    fn create(name: &str) {
        let path = guest(name);
        let fd = super::super::fs::openat([AT, path.as_ptr() as u64, 0o101, 0o644, 0, 0]);
        assert!(fd >= 0, "{name}: {fd}");
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
    fn v1(contents: u8, filenames: u8, flags: u8, descriptor: u8) -> Vec<u8> {
        [&[0, contents, filenames, flags][..], &[descriptor; 8]].concat()
    }
    fn v2(contents: u8, filenames: u8, flags: u8, identifier: [u8; 16]) -> Vec<u8> {
        [
            &[2, contents, filenames, flags, 0, 0, 0, 0][..],
            &identifier,
        ]
        .concat()
    }
    fn get_ex(file: &std::fs::File, size: u64) -> Result<Vec<u8>, i64> {
        let mut arg = [&size.to_le_bytes()[..], &[0; 24]].concat();
        match call(file, GET_POLICY_EX, &mut arg, &root()) {
            0 => {
                let size = u64::from_le_bytes(arg[..8].try_into().unwrap()) as usize;
                Ok(arg[8..8 + size].to_vec())
            }
            error => Err(error),
        }
    }
    fn nonce(file: &std::fs::File) -> Result<[u8; 16], i64> {
        let mut arg = [0u8; 16];
        match call(file, GET_NONCE, &mut arg, &root()) {
            0 => Ok(arg),
            error => Err(error),
        }
    }
    fn key_arg(identifier: [u8; 16]) -> Vec<u8> {
        let mut arg = vec![0u8; 128];
        arg[0] = 2;
        arg[8..24].copy_from_slice(&identifier);
        arg
    }
    fn add(file: &std::fs::File, raw: &[u8], id: &Identity) -> Result<[u8; 16], i64> {
        let mut arg = key_arg([0; 16]);
        arg.truncate(80);
        arg[40..44].copy_from_slice(&(raw.len() as u32).to_le_bytes());
        arg.extend_from_slice(raw);
        match call(file, ADD_KEY, &mut arg, id) {
            0 => Ok(arg[8..24].try_into().unwrap()),
            error => Err(error),
        }
    }
    fn status(file: &std::fs::File, identifier: [u8; 16], id: &Identity) -> [u32; 3] {
        let mut arg = key_arg(identifier);
        assert_eq!(call(file, GET_KEY_STATUS, &mut arg, id), 0);
        let word = |at: usize| u32::from_le_bytes(arg[at..at + 4].try_into().unwrap());
        [word(64), word(68), word(72)]
    }
    fn remove(
        file: &std::fs::File,
        identifier: [u8; 16],
        all: bool,
        id: &Identity,
    ) -> Result<u32, i64> {
        let mut arg = key_arg(identifier);
        arg.truncate(64);
        let request = if all {
            REMOVE_KEY_ALL_USERS
        } else {
            REMOVE_KEY
        };
        match call(file, request, &mut arg, id) {
            0 => Ok(u32::from_le_bytes(arg[40..44].try_into().unwrap())),
            error => Err(error),
        }
    }
    fn errno(error: Errno) -> i64 {
        -(error as i64)
    }

    #[test]
    fn keyring_identifies_v2_keys_and_tracks_users() {
        let (_guard, view) = crate::vfs::test_view();
        let _ = std::fs::remove_dir_all(view.join("data/fscrypt-tests"));
        std::fs::create_dir_all(view.join("data/fscrypt-tests/keys")).unwrap();
        let dir = open(view, "keys");
        let raw = [0x42u8; 64];
        let (user, other) = (identity(1000, 0), identity(1001, 0));
        let identifier = add(&dir, &raw, &user).unwrap();
        assert_eq!(identifier, aim_storage::fscrypt::key_identifier(&raw));
        // status, status_flags (ADDED_BY_SELF), user_count.
        assert_eq!(status(&dir, identifier, &user), [2, 1, 1]);
        assert_eq!(status(&dir, identifier, &other), [2, 0, 1]);
        assert_eq!(add(&dir, &raw, &other), Ok(identifier));
        assert_eq!(status(&dir, identifier, &other), [2, 1, 2]);
        // Removing a claim the caller does not hold, or for all users
        // without CAP_SYS_ADMIN.
        assert_eq!(remove(&dir, identifier, false, &root()), Err(errno(ENOKEY)));
        assert_eq!(
            remove(&dir, identifier, true, &user),
            Err(errno(errno::EACCES))
        );
        // OTHER_USERS, then gone.
        assert_eq!(remove(&dir, identifier, false, &user), Ok(0x2));
        assert_eq!(status(&dir, identifier, &user), [2, 0, 1]);
        assert_eq!(remove(&dir, identifier, false, &other), Ok(0));
        assert_eq!(status(&dir, identifier, &other), [1, 0, 0]);
        assert_eq!(remove(&dir, identifier, false, &other), Err(errno(ENOKEY)));
        add(&dir, &raw, &user).unwrap();
        add(&dir, &raw, &other).unwrap();
        assert_eq!(remove(&dir, identifier, true, &root()), Ok(0));
        assert_eq!(status(&dir, identifier, &user), [1, 0, 0]);

        // Argument errors.
        assert_eq!(add(&dir, &raw[..15], &user), Err(errno(errno::EINVAL)));
        assert_eq!(add(&dir, &[1; 65], &user), Err(errno(errno::EINVAL)));
        let mut bad = key_arg([0; 16]);
        bad[0] = 3;
        assert_eq!(
            call(&dir, GET_KEY_STATUS, &mut bad, &user),
            errno(errno::EINVAL)
        );
        let mut bad = key_arg(identifier);
        bad[40] = 1;
        assert_eq!(
            call(&dir, GET_KEY_STATUS, &mut bad, &user),
            errno(errno::EINVAL)
        );
        // A v1 key needs CAP_SYS_ADMIN to add or remove, and has no users.
        let mut v1_key = vec![0u8; 80 + 32];
        v1_key[0] = 1;
        v1_key[8..16].copy_from_slice(&[9; 8]);
        v1_key[40] = 32;
        assert_eq!(
            call(&dir, ADD_KEY, &mut v1_key.clone(), &user),
            errno(errno::EACCES)
        );
        assert_eq!(call(&dir, ADD_KEY, &mut v1_key, &root()), 0);
        let mut v1_status = vec![0u8; 128];
        v1_status[..40].copy_from_slice(&v1_key[..40]);
        assert_eq!(call(&dir, GET_KEY_STATUS, &mut v1_status, &user), 0);
        assert_eq!(&v1_status[64..76], &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let mut v1_remove = v1_status[..64].to_vec();
        assert_eq!(
            call(&dir, REMOVE_KEY, &mut v1_remove, &user),
            errno(errno::EACCES)
        );
        assert_eq!(call(&dir, REMOVE_KEY, &mut v1_remove, &root()), 0);
        // A provisioning key by serial: none exists.
        let mut serial = key_arg([0; 16]);
        serial.truncate(80);
        serial[44] = 1;
        assert_eq!(call(&dir, ADD_KEY, &mut serial, &user), errno(ENOKEY));
    }

    #[test]
    fn policies_are_set_on_empty_directories_and_inherited() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("data/fscrypt-tests");
        let _ = std::fs::remove_dir_all(&host);
        std::fs::create_dir_all(&host).unwrap();
        for name in ["v1", "v2", "plain", "busy", "other"] {
            mkdir(name);
        }
        create("busy/file");
        create("plain/file");
        let (v1_dir, v2_dir, plain) = (open(view, "v1"), open(view, "v2"), open(view, "plain"));
        let policy = v1(1, 4, 0x2, 1);

        // generic/396 and fscrypt.rst's errors.
        let set = |file: &std::fs::File, policy: &[u8], id: &Identity| {
            call(file, SET_POLICY, &mut policy.to_vec(), id)
        };
        assert_eq!(
            ioctl_as(v1_dir.as_raw_fd(), SET_POLICY, 1, &root()),
            Some(errno(errno::EFAULT))
        );
        let mut bad_version = policy.clone();
        bad_version[0] = 1;
        assert_eq!(set(&v1_dir, &bad_version, &root()), errno(errno::EINVAL));
        assert_eq!(set(&v1_dir, &v1(1, 1, 0, 1), &root()), errno(errno::EINVAL));
        assert_eq!(
            set(&v1_dir, &v1(1, 4, 0x20, 1), &root()),
            errno(errno::EINVAL)
        );
        // DIRECT_KEY needs Adiantum; the IV_INO_LBLK flags need v2.
        assert_eq!(
            set(&v1_dir, &v1(1, 4, DIRECT_KEY, 1), &root()),
            errno(errno::EINVAL)
        );
        assert_eq!(
            set(&v1_dir, &v1(1, 4, IV_INO_LBLK_64, 1), &root()),
            errno(errno::EINVAL)
        );
        assert_eq!(
            set(&open(view, "busy/file"), &policy, &root()),
            errno(errno::ENOTDIR)
        );
        assert_eq!(set(&open(view, "busy"), &policy, &root()), errno(ENOTEMPTY));
        assert_eq!(
            set(&v1_dir, &policy, &identity(1000, 0)),
            errno(errno::EACCES)
        );
        assert_eq!(get_ex(&v1_dir, 24), Err(errno(errno::ENODATA)));
        assert_eq!(nonce(&plain), Err(errno(errno::ENODATA)));

        // generic/395: set, get back, set again; a different policy is EEXIST.
        assert_eq!(set(&v1_dir, &policy, &root()), 0);
        assert_eq!(set(&v1_dir, &policy, &identity(0, 0)), 0);
        assert_eq!(
            set(&v1_dir, &v1(1, 4, 0x3, 1), &root()),
            errno(errno::EEXIST)
        );
        assert_eq!(get_ex(&v1_dir, 12), Ok(policy.clone()));
        assert_eq!(get_ex(&v1_dir, 11), Err(errno(EOVERFLOW)));
        let mut legacy = [0u8; 12];
        assert_eq!(call(&v1_dir, GET_POLICY, &mut legacy, &root()), 0);
        assert_eq!(legacy.to_vec(), policy);

        // v2: the key must be added by the setter (or CAP_FOWNER).
        let identifier = aim_storage::fscrypt::key_identifier(&[7; 32]);
        let policy2 = v2(1, 4, 0x2, identifier);
        let no_fowner = identity(0, 0);
        assert_eq!(set(&v2_dir, &policy2, &no_fowner), errno(ENOKEY));
        let mut reserved = policy2.clone();
        reserved[6] = 1;
        assert_eq!(set(&v2_dir, &reserved, &root()), errno(errno::EINVAL));
        let both = v2(1, 4, DIRECT_KEY | IV_INO_LBLK_64, identifier);
        assert_eq!(set(&v2_dir, &both, &root()), errno(errno::EINVAL));
        add(&v2_dir, &[7; 32], &no_fowner).unwrap();
        assert_eq!(set(&v2_dir, &policy2, &no_fowner), 0);
        assert_eq!(get_ex(&v2_dir, 24), Ok(policy2.clone()));
        assert_eq!(
            call(&v2_dir, GET_POLICY, &mut legacy, &root()),
            errno(EOVERFLOW)
        );
        assert_eq!(
            set(&open(view, "other"), &v2(1, 10, 0, identifier), &root()),
            0
        );

        // Inheritance with fresh nonces (generic/399); special files are not encrypted.
        mkdir("v1/sub");
        create("v1/sub/a");
        create("v1/b");
        let target = c"b";
        let link = guest("v1/link");
        assert_eq!(
            super::super::fsops::symlinkat([
                target.as_ptr() as u64,
                AT,
                link.as_ptr() as u64,
                0,
                0,
                0
            ]),
            0
        );
        let fifo = guest("v1/fifo");
        assert_eq!(
            super::super::fsops::mknodat([AT, fifo.as_ptr() as u64, 0o10644, 0, 0, 0]),
            0
        );
        let mut nonces = vec![nonce(&v1_dir).unwrap()];
        for name in ["v1/sub", "v1/sub/a", "v1/b"] {
            let file = open(view, name);
            assert_eq!(get_ex(&file, 24), Ok(policy.clone()), "{name}");
            nonces.push(nonce(&file).unwrap());
        }
        let host_path = |name: &str| {
            CString::new(host.join(name).into_os_string().into_encoded_bytes()).unwrap()
        };
        assert!(context(Host::Path(&host_path("v1/link"))).is_some());
        assert!(context(Host::Path(&host_path("v1/fifo"))).is_none());
        nonces.sort();
        nonces.dedup();
        assert_eq!(nonces.len(), 4);

        std::fs::remove_dir_all(&host).unwrap();
    }
}
