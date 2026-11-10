//! fscrypt on the data filesystem (Linux v6.12 `fs/crypto/`,
//! `Documentation/filesystems/fscrypt.rst`; ADR 0016, option B): the
//! keyring ioctls, encryption policies set on empty directories and
//! inherited by what is created below them, their queries, and EXDEV for a
//! link or rename across policies. The bytes stay plaintext on the host;
//! confidentiality at rest is the Mac's.
//!
//! An inode's `struct fscrypt_context` (its policy and its own nonce) is
//! the host attribute `dev.aim.fscrypt-context`, as ext4 keeps it in an
//! xattr. The keyring and the table of policy roots belong to the owner
//! guest-init prepares (`aim_storage::fscrypt`), shared by every process.
//! Until a directory of the filesystem has a policy, nothing here reads an
//! attribute.
//!
//! An inode whose key is absent is locked (`fs/crypto/hooks.c`,
//! `fname.c`): a locked directory lists no-key names, finds entries only
//! by them, and refuses new entries with ENOKEY; a locked regular file
//! cannot be opened, and a locked symlink's target reads as its no-key
//! form. The host keeps plaintext, so the owner's per-volume secret stands
//! in for the ciphertext a no-key name encodes. Path walks ask only while
//! the owner's shared page says a key some policy names is absent.
//! Removing a key locks its inodes at once: files already open keep
//! working, as Linux's busy files do, but the key is reported absent, not
//! FILES_BUSY.

use std::ffi::{CStr, CString};
use std::os::fd::AsRawFd;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use aim_storage::fscrypt::{CONTEXT, Owner, Removal, Spec, policy, supported};
use sha2::{Digest, Sha256};

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
/// `FS_ENCRYPT_FL` and `STATX_ATTR_ENCRYPTED`.
pub(super) const ENCRYPTED: u32 = 0x800;

const NONCE: usize = 16;
const PAD_MASK: u8 = 0x03;
const CAP_FOWNER: u32 = 3;
const CAP_SYS_ADMIN: u32 = 21;
const EOVERFLOW: Errno = 75;
const ENOKEY: Errno = 126;
const ENOTEMPTY: Errno = 39;
const NAME_MAX: usize = 255;
/// ext4's longest symlink target with 4096-byte blocks, less the length
/// word of `struct fscrypt_symlink_data`.
const SYMLINK_MAX: usize = 4094;
/// `struct fscrypt_nokey_name`: the directory hash (two words, left zero),
/// the ciphertext, or its first 149 bytes then the SHA-256 of the rest.
const NOKEY_HASH: usize = 8;
const NOKEY_BYTES: usize = 149;
const NOKEY_MAX: usize = NOKEY_HASH + NOKEY_BYTES + 32;

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

/// Whether the filesystem is encrypted, so that a directory may have a
/// policy. It stays so, so a process asks the owner only until it is.
pub(super) fn active() -> Result<bool, Errno> {
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    if ACTIVE.load(Ordering::Relaxed) {
        return Ok(true);
    }
    let Some(owner) = owner()? else {
        return Ok(false);
    };
    let active = owner.has_policies().map_err(io)?;
    ACTIVE.store(active, Ordering::Relaxed);
    Ok(active)
}

fn context(host: Host) -> Option<Vec<u8>> {
    let mut b = [0u8; 40];
    attrs::get(host, CONTEXT, &mut b).map(|n| b[..n].to_vec())
}

fn host_type(host: Host) -> Result<u16, Errno> {
    Ok(host_stat(host)?.st_mode & libc::S_IFMT)
}

fn host_stat(host: Host) -> Result<libc::stat, Errno> {
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
    Ok(st)
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

/// Whether `host` has a policy (`IS_ENCRYPTED`).
pub(super) fn encrypted(host: Host) -> Result<bool, Errno> {
    Ok(active()? && context(host).is_some())
}

/// `fscrypt_prepare_new_inode`: `child`, just created in `parent` (on
/// host device `device`), takes the parent's policy with a nonce of its
/// own.
pub(super) fn inherit(parent: Host, device: i32, child: Host) -> Result<(), Errno> {
    if !active()? || owner()?.is_none_or(|owner| owner.device() != device as u64) {
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

/// `fscrypt_has_permitted_context`: whether `child` may be linked or
/// renamed into `parent`. A child that is gone is the operation's to
/// report.
fn permitted(parent: Host, child: Host) -> Result<bool, Errno> {
    let Some(parent) = context(parent) else {
        return Ok(true);
    };
    match host_type(child) {
        Ok(kind) if encryptable(kind) => {}
        _ => return Ok(true),
    }
    Ok(context(child)
        .is_some_and(|child| policy(&child).is_some_and(|c| policy(&parent) == Some(c))))
}

/// The directory a host path is in.
fn parent_of(host: &CStr) -> CString {
    let bytes = host.to_bytes();
    let end = bytes.iter().rposition(|byte| *byte == b'/').unwrap_or(0);
    CString::new(if end == 0 { &b"/"[..] } else { &bytes[..end] }).unwrap()
}

/// `fscrypt_prepare_link`: `child` (a path or an open file) linked as
/// `new`.
pub(super) fn link(child: Host, new: &CStr) -> Result<(), Errno> {
    if !active()? {
        return Ok(());
    }
    let parent = parent_of(new);
    if !permitted(Host::Path(&parent), child)? {
        return Err(errno::EXDEV);
    }
    moved(Host::Path(&parent), child)
}

/// Records `child` as a policy root if it has a policy and `parent`, which
/// it is being linked or moved into, has none.
fn moved(parent: Host, child: Host) -> Result<(), Errno> {
    if context(parent).is_some() {
        return Ok(());
    }
    let (Some(owner), Some(spec)) = (
        owner()?,
        context(child).as_deref().and_then(Spec::of_context),
    ) else {
        return Ok(());
    };
    owner
        .policy_root(spec, host_stat(child)?.st_ino)
        .map_err(io)
}

/// `fscrypt_prepare_rename`: neither directory may be locked, and between
/// different directories what moves into an encrypted one must have its
/// policy.
pub(super) fn rename(old: &CStr, new: &CStr, exchange: bool) -> Result<(), Errno> {
    if !active()? {
        return Ok(());
    }
    let (old_dir, new_dir) = (parent_of(old), parent_of(new));
    if locking()?
        && (locked(Host::Path(&old_dir))?.is_some() || locked(Host::Path(&new_dir))?.is_some())
    {
        return Err(ENOKEY);
    }
    let identity = |path: &CStr| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::stat(path.as_ptr(), &mut st) } == 0).then_some((st.st_dev, st.st_ino))
    };
    if identity(&old_dir) == identity(&new_dir) {
        return Ok(());
    }
    if !permitted(Host::Path(&new_dir), Host::Path(old))?
        || exchange && !permitted(Host::Path(&old_dir), Host::Path(new))?
    {
        return Err(errno::EXDEV);
    }
    moved(Host::Path(&new_dir), Host::Path(old))?;
    if exchange {
        moved(Host::Path(&old_dir), Host::Path(new))?;
    }
    Ok(())
}

/// The names in the directory `path` at `fd`, all or the first.
fn entries(fd: i32, path: &CStr, all: bool) -> Result<Vec<CString>, Errno> {
    let _own = super::fork::spawn::own_fds();
    let copy = unsafe {
        libc::openat(
            fd,
            path.as_ptr(),
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
    let mut names = Vec::new();
    while all || names.is_empty() {
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    unsafe { libc::closedir(dir) };
    Ok(names)
}

/// A create that raced the emptiness check of a policy being set: as on
/// Linux the directory was not empty, so it and what inherited its policy
/// meanwhile lose it again. (A create whose inheritance read the policy
/// before this and stores it after keeps it.)
fn withdraw(fd: i32, names: &[CString]) -> Result<(), Errno> {
    attrs::remove(Host::Fd(fd), CONTEXT)?;
    let mut path = [0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, path.as_mut_ptr()) } < 0 {
        return Err(errno::last());
    }
    let dir = CStr::from_bytes_until_nul(&path).map_err(|_| errno::EINVAL)?;
    for name in names {
        let child = [dir.to_bytes(), b"/", name.to_bytes()].concat();
        let child = CString::new(child).map_err(|_| errno::EINVAL)?;
        attrs::remove(Host::Path(&child), CONTEXT)?;
    }
    Ok(())
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
            if !entries(fd, c".", false)?.is_empty() {
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
            store(Host::Fd(fd), &renewed(&context))?;
            // Creates do not take the inode lock; one may have come between
            // the check and the store.
            let names = entries(fd, c".", true)?;
            if !names.is_empty() {
                withdraw(fd, &names)?;
                return Err(ENOTEMPTY);
            }
            // Recorded once the policy is there: the owner drops roots that
            // have none.
            let spec = Spec::of_context(&context).ok_or(errno::EINVAL)?;
            if let Err(error) = owner.policy_root(spec, stat.st_ino) {
                withdraw(fd, &[])?;
                return Err(io(error));
            }
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

/// Whether an inode may be locked: a key some policy root names is
/// absent. Until one is, nothing asks.
pub(crate) fn locking() -> Result<bool, Errno> {
    Ok(owner()?.is_some_and(Owner::locking))
}

/// Whether an inode is locked.
pub(crate) enum Lock {
    /// It has no policy.
    Plain,
    /// It is encrypted and its key is present. Everything below a directory
    /// has its policy, as Linux keeps a tree's (`fscrypt_has_permitted_context`
    /// on lookup, link and rename), so no inode in its tree is locked while
    /// the key stays.
    Unlocked,
    /// Its key is absent: the owner and its context.
    Locked(&'static Owner, Vec<u8>),
}

/// Whether the inode at the host path `path` is locked.
pub(crate) fn path_lock(path: &CStr) -> Result<Lock, Errno> {
    lock(Host::Path(path))
}

fn lock(host: Host) -> Result<Lock, Errno> {
    let Some(owner) = owner()? else {
        return Ok(Lock::Plain);
    };
    let Some(context) = context(host) else {
        return Ok(Lock::Plain);
    };
    let Some(spec) = Spec::of_context(&context) else {
        return Ok(Lock::Plain);
    };
    Ok(if owner.has_key(spec).map_err(io)? {
        Lock::Unlocked
    } else {
        Lock::Locked(owner, context)
    })
}

/// The context of `host` if its key is absent.
fn locked(host: Host) -> Result<Option<(&'static Owner, Vec<u8>)>, Errno> {
    Ok(match lock(host)? {
        Lock::Locked(owner, context) => Some((owner, context)),
        Lock::Plain | Lock::Unlocked => None,
    })
}

/// `fscrypt_fname_encrypted_size`, padded with NULs, then enciphered: the
/// stand-in for the ciphertext of `plain` in the inode of `context`.
fn ciphertext(owner: &Owner, context: &[u8], plain: &[u8], max: usize) -> Vec<u8> {
    let padding = 4usize << (context[3] & PAD_MASK);
    let size = plain.len().max(16).next_multiple_of(padding).min(max);
    let mut bytes = plain.to_vec();
    bytes.resize(size.max(plain.len()), 0);
    owner.cipher(&context[context.len() - NONCE..], &mut bytes);
    bytes
}

/// `fscrypt_fname_disk_to_usr` without the key: the no-key name of
/// `plain`, base64url-encoded `struct fscrypt_nokey_name`.
fn nokey(owner: &Owner, context: &[u8], plain: &[u8], max: usize) -> Vec<u8> {
    base64url(&nokey_struct(owner, context, plain, max))
}

/// The `struct fscrypt_nokey_name` of `plain`.
fn nokey_struct(owner: &Owner, context: &[u8], plain: &[u8], max: usize) -> Vec<u8> {
    let ciphertext = ciphertext(owner, context, plain, max);
    let mut name = vec![0u8; NOKEY_HASH];
    if ciphertext.len() <= NOKEY_BYTES {
        name.extend_from_slice(&ciphertext);
    } else {
        name.extend_from_slice(&ciphertext[..NOKEY_BYTES]);
        name.extend_from_slice(&Sha256::digest(&ciphertext[NOKEY_BYTES..]));
    }
    name
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `fscrypt_base64url_encode`: RFC 4648 base64url without padding.
fn base64url(bytes: &[u8]) -> Vec<u8> {
    let (mut out, mut bits, mut value) = (Vec::new(), 0, 0u32);
    for byte in bytes {
        value = value << 8 | *byte as u32;
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(BASE64URL[(value >> bits) as usize & 0x3f]);
        }
    }
    if bits > 0 {
        out.push(BASE64URL[(value << (6 - bits)) as usize & 0x3f]);
    }
    out
}

/// `fscrypt_base64url_decode`: None for a character outside the alphabet
/// or bits left over.
fn base64url_decode(text: &[u8]) -> Option<Vec<u8>> {
    let (mut out, mut bits, mut value) = (Vec::new(), 0, 0u32);
    for c in text {
        value = value << 6 | BASE64URL.iter().position(|a| a == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((value >> bits) as u8);
        }
    }
    (value & ((1 << bits) - 1) == 0).then_some(out)
}

/// `fscrypt_setup_filename` and `fscrypt_match_name` without the key: the
/// entry of the locked directory `dir` the no-key name `name` designates,
/// None where it can designate none.
fn plaintext(
    owner: &Owner,
    context: &[u8],
    dir: &CStr,
    name: &[u8],
) -> Result<Option<Vec<u8>>, Errno> {
    let Some(decoded) = base64url_decode(name) else {
        return Ok(None);
    };
    if decoded.len() <= NOKEY_HASH
        || decoded.len() > NOKEY_HASH + NOKEY_BYTES && decoded.len() != NOKEY_MAX
    {
        return Ok(None);
    }
    if decoded.len() == NOKEY_MAX {
        // Only a long name's: compare the entries' own.
        return Ok(entries(libc::AT_FDCWD, dir, true)?
            .into_iter()
            .map(CString::into_bytes)
            .find(|entry| {
                nokey_struct(owner, context, entry, NAME_MAX)[NOKEY_HASH..] == decoded[NOKEY_HASH..]
            }));
    }
    let mut plain = decoded[NOKEY_HASH..].to_vec();
    owner.cipher(&context[context.len() - NONCE..], &mut plain);
    plain.truncate(
        plain
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |at| at + 1),
    );
    let valid = !plain.is_empty()
        && !plain.contains(&0)
        && !plain.contains(&b'/')
        && plain != b"."
        && plain != b".."
        && ciphertext(owner, context, &plain, NAME_MAX) == decoded[NOKEY_HASH..];
    Ok(valid.then_some(plain))
}

/// The host name of `name` looked up in the host directory `dir`, whose
/// lock is `lock`; None where `dir` is not locked and the names are the
/// host's. A locked directory finds entries only by no-key names: any
/// other is ENOENT, and creating (`new`) any entry but an existing one is
/// ENOKEY (`fscrypt_prepare_lookup`, `fscrypt_prepare_new_inode`).
pub(crate) fn host_name(
    lock: &Lock,
    dir: &CStr,
    name: &[u8],
    new: bool,
) -> Result<Option<Vec<u8>>, Errno> {
    let Lock::Locked(owner, context) = lock else {
        return Ok(None);
    };
    let plain = plaintext(owner, context, dir, name)?;
    if new {
        let exists = plain.as_ref().is_some_and(|plain| {
            let path = [dir.to_bytes(), b"/", plain].concat();
            CString::new(path).is_ok_and(|path| host_type(Host::Path(&path)).is_ok())
        });
        if !exists {
            return Err(ENOKEY);
        }
    }
    plain.map(Some).ok_or(errno::ENOENT)
}

/// The name the guest sees for the entry `name` of a directory whose lock
/// is `lock`: its no-key name if the directory is locked.
pub(crate) fn guest_name(lock: &Lock, name: &[u8]) -> Vec<u8> {
    match lock {
        Lock::Locked(owner, context) => nokey(owner, context, name, NAME_MAX),
        Lock::Plain | Lock::Unlocked => name.to_vec(),
    }
}

/// readdir of the directory `fd`: no-key names if it is locked.
pub(super) fn list(fd: i32, entries: &mut [super::dir::Entry]) -> Result<(), Errno> {
    if !locking()? {
        return Ok(());
    }
    let Some((owner, context)) = locked(Host::Fd(fd))? else {
        return Ok(());
    };
    for entry in entries {
        if entry.name != b"." && entry.name != b".." {
            entry.name = nokey(owner, &context, &entry.name, NAME_MAX);
        }
    }
    Ok(())
}

/// `fscrypt_get_symlink`: a locked symlink's target is its no-key form.
pub(crate) fn link_target(link: &CStr, target: Vec<u8>) -> Result<Vec<u8>, Errno> {
    if !locking()? {
        return Ok(target);
    }
    Ok(match locked(Host::Path(link))? {
        Some((owner, context)) => nokey(owner, &context, &target, SYMLINK_MAX),
        None => target,
    })
}

/// Before the entry `host` is unlinked or removed: the inode of a policy
/// root it may be, to report to [`removed`] once it is gone.
pub(super) fn removing(host: &CStr) -> Result<Option<u64>, Errno> {
    if !locking()? || context(Host::Path(host)).is_none() {
        return Ok(None);
    }
    Ok(Some(host_stat(Host::Path(host))?.st_ino))
}

/// After [`removing`]'s entry is gone: while a key is absent, a root that
/// went with it no longer keeps lookups asking.
pub(super) fn removed(inode: Option<u64>) -> Result<(), Errno> {
    match (inode, owner()?) {
        (Some(inode), Some(owner)) => owner.removed(inode).map_err(io),
        _ => Ok(()),
    }
}

/// `fscrypt_file_open` (also for execve) and `fscrypt_prepare_setattr` of a
/// size: a locked regular file can neither be opened nor truncated. Only a
/// writable area can be on the data filesystem, and nothing below an
/// encrypted directory whose key is present is locked.
pub(super) fn open(resolved: &crate::vfs::Resolved) -> Result<(), Errno> {
    let host = resolved.host.as_c_str();
    if resolved.area == crate::vfs::Area::Writable
        && !resolved.unlocked
        && locking()?
        && host_type(Host::Path(host)).is_ok_and(|kind| kind == libc::S_IFREG)
        && locked(Host::Path(host))?.is_some()
    {
        return Err(ENOKEY);
    }
    Ok(())
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
            // `fscrypt_ioctl_get_policy`: only v1 fits.
            if policy[0] != 0 {
                return Err(errno::EINVAL);
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

/// Ported from xfstests' encryption group (generic/395-399, 429 and the
/// key removal of 580) and `fscrypt.rst`'s error table.
#[cfg(test)]
mod tests {
    use super::*;
    use aim_storage::fscrypt::{DIRECT_KEY, IV_INO_LBLK_64};
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
    fn add_v1(file: &std::fs::File, descriptor: u8) {
        let mut arg = vec![0u8; 80 + 32];
        arg[0] = 1;
        arg[8..16].copy_from_slice(&[descriptor; 8]);
        arg[40] = 32;
        assert_eq!(call(file, ADD_KEY, &mut arg, &root()), 0);
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
        // Its key, without which nothing can be created below it.
        add_v1(&v1_dir, 1);

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
            errno(errno::EINVAL)
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

        // FS_ENCRYPT_FL and STATX_ATTR_ENCRYPTED.
        let mut flags = 0u32;
        let flags_arg = (&mut flags as *mut u32) as u64;
        let b = open(view, "v1/b");
        assert_eq!(
            super::super::fileattr::ioctl(
                b.as_raw_fd(),
                super::super::fileattr::GETFLAGS,
                flags_arg
            ),
            Some(0)
        );
        assert_eq!(flags & ENCRYPTED, ENCRYPTED);
        let mut statx = [0u8; 256];
        let b_path = guest("v1/b");
        let statx_args = |path: &CString, out: &mut [u8; 256]| {
            [
                AT,
                path.as_ptr() as u64,
                0,
                0xfff,
                out.as_mut_ptr() as u64,
                0,
            ]
        };
        assert_eq!(super::super::fs::statx(statx_args(&b_path, &mut statx)), 0);
        assert_eq!(statx[9] & 0x08, 0x08);
        let plain_path = guest("plain/file");
        assert_eq!(
            super::super::fs::statx(statx_args(&plain_path, &mut statx)),
            0
        );
        assert_eq!(statx[9] & 0x08, 0);

        // generic/398-like moves: EXDEV into a directory of another policy,
        // or of an unencrypted file into an encrypted one; out of one is
        // allowed, as are special files.
        let path = |name: &str| guest(name);
        let rename = |from: &str, to: &str, flags: u64| {
            let (from, to) = (path(from), path(to));
            super::super::fsops::renameat2([
                AT,
                from.as_ptr() as u64,
                AT,
                to.as_ptr() as u64,
                flags,
                0,
            ])
        };
        let link = |from: &str, to: &str| {
            let (from, to) = (path(from), path(to));
            super::super::fsops::linkat([AT, from.as_ptr() as u64, AT, to.as_ptr() as u64, 0, 0])
        };
        let exdev = errno(errno::EXDEV);
        assert_eq!(rename("v1/b", "v2/b", 0), exdev);
        assert_eq!(rename("plain/file", "v1/file", 0), exdev);
        assert_eq!(link("plain/file", "v1/file"), exdev);
        assert_eq!(link("v1/b", "v2/b"), exdev);
        assert_eq!(rename("v1/fifo", "v2/fifo", 0), 0);
        create("v2/c");
        // RENAME_EXCHANGE checks both directions.
        assert_eq!(rename("plain/file", "v2/c", 2), exdev);
        assert_eq!(link("v1/b", "v1/sub/b"), 0);
        assert_eq!(rename("v1/sub/a", "plain/a", 0), 0);
        assert_eq!(get_ex(&open(view, "plain/a"), 24), Ok(policy));

        std::fs::remove_dir_all(&host).unwrap();
    }

    #[test]
    fn a_create_racing_the_policy_withdraws_it() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("data/fscrypt-tests");
        let _ = std::fs::remove_dir_all(&host);
        std::fs::create_dir_all(&host).unwrap();
        mkdir("raced");
        let dir = open(view, "raced");
        let policy = v1(1, 4, 0, 3);
        add_v1(&dir, 3);
        assert_eq!(call(&dir, SET_POLICY, &mut policy.clone(), &root()), 0);
        // What the race leaves before the re-check: the directory's policy
        // and a child that inherited it.
        create("raced/late");
        assert!(context(Host::Fd(open(view, "raced/late").as_raw_fd())).is_some());
        let names = entries(dir.as_raw_fd(), c".", true).unwrap();
        assert_eq!(names, [c"late".to_owned()]);
        withdraw(dir.as_raw_fd(), &names).unwrap();
        assert_eq!(get_ex(&dir, 24), Err(errno(errno::ENODATA)));
        assert_eq!(
            get_ex(&open(view, "raced/late"), 24),
            Err(errno(errno::ENODATA))
        );
        std::fs::remove_dir_all(&host).unwrap();
    }

    /// Guest syscalls on a path under the test directory.
    fn at(name: &str, mut call: impl FnMut(u64) -> i64) -> i64 {
        let path = guest(name);
        call(path.as_ptr() as u64)
    }
    fn open_at(name: &str, flags: u64) -> i64 {
        let fd = at(name, |path| {
            super::super::fs::openat([AT, path, flags, 0o644, 0, 0])
        });
        if fd >= 0 {
            assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
            return 0;
        }
        fd
    }
    /// The names getdents lists in the directory, without `.` and `..`.
    fn list(name: &str) -> Vec<String> {
        let fd = at(name, |path| {
            super::super::fs::openat([AT, path, 0o40000, 0, 0, 0])
        });
        assert!(fd >= 0, "{name}: {fd}");
        let mut names = Vec::new();
        let mut buffer = vec![0u8; 16 << 10];
        loop {
            let args = [
                fd as u64,
                buffer.as_mut_ptr() as u64,
                buffer.len() as u64,
                0,
                0,
                0,
            ];
            let n = super::super::dir::getdents64(args);
            assert!(n >= 0, "{n}");
            if n == 0 {
                break;
            }
            let mut at = 0;
            while at < n as usize {
                let length = u16::from_le_bytes([buffer[at + 16], buffer[at + 17]]) as usize;
                let name = CStr::from_bytes_until_nul(&buffer[at + 19..at + length]).unwrap();
                let name = name.to_str().unwrap().to_string();
                if name != "." && name != ".." {
                    names.push(name);
                }
                at += length;
            }
        }
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        names.sort();
        names
    }
    fn kind(name: &str) -> Result<u32, i64> {
        super::super::fs::stat_at(AT as i32, guest(name).as_bytes(), 0x100)
            .map(|stat| (stat.st_mode & libc::S_IFMT) as u32)
    }
    fn readlink(name: &str) -> Vec<u8> {
        let mut target = [0u8; 512];
        let n = at(name, |path| {
            super::super::fs::readlinkat([AT, path, target.as_mut_ptr() as u64, 512, 0, 0])
        });
        assert!(n > 0, "{name}: {n}");
        target[..n as usize].to_vec()
    }
    /// An encrypted directory `name` with a file, a subdirectory with a
    /// file, a symlink and a file of a long name, under a v2 key the root
    /// user added; returns the key's identifier.
    fn populate(view: &Path, name: &str, raw: &[u8]) -> [u8; 16] {
        mkdir(name);
        let dir = open(view, name);
        let identifier = add(&dir, raw, &root()).unwrap();
        let policy = v2(1, 4, 0x2, identifier);
        assert_eq!(call(&dir, SET_POLICY, &mut policy.clone(), &root()), 0);
        create(&format!("{name}/file"));
        std::fs::write(
            view.join("data/fscrypt-tests").join(name).join("file"),
            b"secret",
        )
        .unwrap();
        mkdir(&format!("{name}/sub"));
        create(&format!("{name}/sub/inner"));
        create(&format!("{name}/{}", "l".repeat(200)));
        let target = c"file";
        assert_eq!(
            at(&format!("{name}/link"), |path| {
                super::super::fsops::symlinkat([target.as_ptr() as u64, AT, path, 0, 0, 0])
            }),
            0
        );
        identifier
    }
    const PLAIN: [&str; 3] = ["file", "link", "sub"];
    fn plain_names() -> Vec<String> {
        let mut names: Vec<String> = PLAIN.iter().map(|name| name.to_string()).collect();
        names.push("l".repeat(200));
        names.sort();
        names
    }

    #[test]
    fn a_locked_directory_lists_no_key_names_that_unlink() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("data/fscrypt-tests");
        let _ = std::fs::remove_dir_all(&host);
        std::fs::create_dir_all(&host).unwrap();
        mkdir("plain");
        create("plain/file");
        let identifier = populate(view, "locked", &[0x5a; 64]);
        let dir = open(view, "locked");
        assert_eq!(list("locked"), plain_names());
        assert_eq!(remove(&dir, identifier, false, &root()), Ok(0));
        assert!(locking().unwrap());

        // generic/397: no-key names, stable, none the plaintext; the
        // long name's is the hashed form, at most NAME_MAX.
        let names = list("locked");
        assert_eq!(names.len(), 4);
        assert_eq!(list("locked"), names);
        for name in &names {
            assert!(!plain_names().contains(name), "{name}");
            assert!(name.bytes().all(|byte| BASE64URL.contains(&byte)), "{name}");
            assert!(name.len() <= NAME_MAX);
        }
        assert!(names.iter().any(|name| name.len() == NOKEY_MAX / 3 * 4));
        let path = |name: &String| format!("locked/{name}");
        let by_kind = |wanted: u32| {
            names
                .iter()
                .filter(|name| kind(&path(name)) == Ok(wanted))
                .cloned()
                .collect::<Vec<_>>()
        };
        let (files, dirs, links) = (
            by_kind(libc::S_IFREG as u32),
            by_kind(libc::S_IFDIR as u32),
            by_kind(libc::S_IFLNK as u32),
        );
        assert_eq!((files.len(), dirs.len(), links.len()), (2, 1, 1));
        // Plaintext names and names that decode to no entry are not there.
        assert_eq!(kind("locked/file"), Err(errno(errno::ENOENT)));
        assert_eq!(
            kind("locked/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            Err(errno(errno::ENOENT))
        );

        // fscrypt.rst: without the key, open of a regular file, every
        // create, link and rename are ENOKEY; readlink gives the no-key
        // form; stat and opening a directory work.
        let enokey = errno(ENOKEY);
        assert_eq!(open_at(&path(&files[0]), 0), enokey);
        assert_eq!(open_at(&path(&files[0]), 0o1101), enokey);
        // execve opens the file; truncate changes its size.
        let file = guest(&path(&files[0]));
        assert_eq!(
            super::super::exec::resolve(AT as i32, file.as_bytes(), 0).err(),
            Some(enokey)
        );
        assert_eq!(
            super::super::fsops::truncate([file.as_ptr() as u64, 0, 0, 0, 0, 0]),
            enokey
        );
        assert_eq!(open_at(&path(&dirs[0]), 0o40000), 0);
        assert_eq!(open_at("locked/new", 0o101), enokey);
        assert_eq!(open_at("locked/new", 0o301), enokey);
        assert_eq!(open_at(&path(&files[0]), 0o301), errno(errno::EEXIST));
        assert_eq!(
            at("locked/new", |p| super::super::fsops::mkdirat([
                AT, p, 0o755, 0, 0, 0
            ])),
            enokey
        );
        assert_eq!(
            at("locked/new", |p| super::super::fsops::mknodat([
                AT, p, 0o10644, 0, 0, 0
            ])),
            enokey
        );
        let target = c"x";
        assert_eq!(
            at("locked/new", |p| super::super::fsops::symlinkat([
                target.as_ptr() as u64,
                AT,
                p,
                0,
                0,
                0
            ])),
            enokey
        );
        let (plain, new, file) = (
            guest("plain/file"),
            guest("locked/new"),
            guest(&path(&files[0])),
        );
        let link = |from: &CString, to: &CString| {
            super::super::fsops::linkat([AT, from.as_ptr() as u64, AT, to.as_ptr() as u64, 0, 0])
        };
        let rename = |from: &CString, to: &CString| {
            let args = [AT, from.as_ptr() as u64, AT, to.as_ptr() as u64, 0, 0];
            super::super::fsops::renameat2(args)
        };
        assert_eq!(link(&plain, &new), enokey);
        assert_eq!(rename(&file, &new), enokey);
        assert_eq!(rename(&file, &guest("plain/moved")), enokey);
        let symlink = readlink(&path(&links[0]));
        assert_ne!(symlink, b"file");
        assert!(symlink.iter().all(|byte| BASE64URL.contains(byte)));
        // A relative lookup from the locked directory, opened by no-key name.
        let sub = guest(&path(&dirs[0]));
        let fd = super::super::fs::openat([AT, sub.as_ptr() as u64, 0o40000, 0, 0, 0]);
        assert!(fd >= 0);
        let inner = list(&path(&dirs[0]));
        assert_eq!(inner.len(), 1);
        let inner_name = CString::new(inner[0].clone()).unwrap();
        let unlink = |dirfd: u64, name: &CString, flags: u64| {
            super::super::fsops::unlinkat([dirfd, name.as_ptr() as u64, flags, 0, 0, 0])
        };
        assert_eq!(unlink(fd as u64, &inner_name, 0), 0);
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);

        // Every entry goes by its no-key name, the long one too.
        for name in &names {
            let flags = if dirs.contains(name) { 0x200 } else { 0 };
            assert_eq!(unlink(AT, &guest(&path(name)), flags), 0, "{name}");
        }
        assert!(list("locked").is_empty());
        // The root goes, and with it the absent key: lookups stop asking.
        let roots = |identifier: [u8; 16]| {
            let table = std::fs::read_to_string(view.join("fscrypt/policies")).unwrap();
            let spec: String = identifier
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            table.contains(&format!("i{spec} "))
        };
        assert!(roots(identifier));
        assert_eq!(
            at("locked", |p| super::super::fsops::unlinkat([
                AT, p, 0x200, 0, 0, 0
            ])),
            0
        );
        assert!(!roots(identifier));
        // Other tests' roots are gone with their directories.
        assert!(!locking().unwrap());
        std::fs::remove_dir_all(&host).unwrap();
    }

    #[test]
    fn a_tree_whose_key_is_present_keeps_its_names_while_another_is_absent() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("data/fscrypt-tests");
        let _ = std::fs::remove_dir_all(&host);
        std::fs::create_dir_all(&host).unwrap();
        populate(view, "open", &[0x4d; 64]);
        let raw = [0x6b; 64];
        let identifier = populate(view, "shut", &raw);
        let dir = open(view, "shut");
        assert_eq!(remove(&dir, identifier, false, &root()), Ok(0));
        assert!(locking().unwrap());
        // Below the directory whose key is present: the host's names, a
        // symlink's own target, new entries.
        assert_eq!(list("open/sub"), ["inner"]);
        assert_eq!(kind("open/sub/inner"), Ok(libc::S_IFREG as u32));
        assert_eq!(readlink("open/link"), b"file");
        assert_eq!(open_at("open/file", 0), 0);
        assert_eq!(open_at("open/sub/new", 0o101), 0);
        // Out of it by `..` into the locked one: no-key names again.
        assert_eq!(kind("open/sub/../../shut/file"), Err(errno(errno::ENOENT)));
        assert_eq!(open_at("open/../shut/new", 0o101), errno(ENOKEY));
        let names = list("open/../shut");
        assert_eq!(names.len(), 4);
        assert!(names.iter().all(|name| !plain_names().contains(name)));
        let file = names
            .iter()
            .find(|name| kind(&format!("shut/{name}")) == Ok(libc::S_IFREG as u32))
            .unwrap();
        assert_eq!(open_at(&format!("open/../shut/{file}"), 0), errno(ENOKEY));
        add(&dir, &raw, &root()).unwrap();
        assert!(!locking().unwrap());
        std::fs::remove_dir_all(&host).unwrap();
    }

    #[test]
    fn adding_the_key_again_unlocks_and_removing_it_locks() {
        let (_guard, view) = crate::vfs::test_view();
        let host = view.join("data/fscrypt-tests");
        let _ = std::fs::remove_dir_all(&host);
        std::fs::create_dir_all(&host).unwrap();
        let raw = [0x3c; 32];
        let identifier = populate(view, "edir", &raw);
        let dir = open(view, "edir");
        let (user, other) = (identity(1000, 0), identity(1001, 0));
        add(&dir, &raw, &user).unwrap();
        add(&dir, &raw, &other).unwrap();
        // generic/580: the key stays while any user holds it.
        assert_eq!(remove(&dir, identifier, false, &root()), Ok(0x2));
        assert_eq!(remove(&dir, identifier, false, &user), Ok(0x2));
        assert_eq!(list("edir"), plain_names());
        assert_eq!(open_at("edir/file", 0), 0);
        assert_eq!(remove(&dir, identifier, false, &other), Ok(0));

        // generic/429: without the key, no-key names, and the plaintext
        // names are gone.
        let locked = list("edir");
        assert_ne!(locked, plain_names());
        for name in PLAIN {
            assert_eq!(kind(&format!("edir/{name}")), Err(errno(errno::ENOENT)));
        }
        // With it again, the plaintext names and contents.
        add(&dir, &raw, &user).unwrap();
        assert_eq!(list("edir"), plain_names());
        assert_eq!(std::fs::read(host.join("edir/file")).unwrap(), b"secret");
        assert_eq!(open_at("edir/file", 0), 0);
        assert_eq!(readlink("edir/link"), b"file");
        assert_eq!(
            kind(&format!("edir/{}", locked[0])),
            Err(errno(errno::ENOENT))
        );
        // The same no-key names when locked again.
        assert_eq!(remove(&dir, identifier, false, &user), Ok(0));
        assert_eq!(list("edir"), locked);
        std::fs::remove_dir_all(&host).unwrap();
    }

    #[test]
    fn no_key_names_decode_only_to_their_entry() {
        let _guard = crate::vfs::test_view();
        let owner = owner().unwrap().unwrap();
        let context = [&[2u8, 1, 4, 0x2, 0, 0, 0, 0][..], &[1; 16], &[7; 16]].concat();
        let dir = c"/nonexistent";
        for plain in [&b"a"[..], b"name.with.dots", &[b'x'; 120], &[0xff; 3]] {
            let name = nokey(owner, &context, plain, NAME_MAX);
            assert_eq!(
                plaintext(owner, &context, dir, &name).unwrap().as_deref(),
                Some(plain)
            );
        }
        // Padding to 16 bytes (flags 0x2): a 33-byte name takes 48.
        assert_eq!(ciphertext(owner, &context, &[b'x'; 33], NAME_MAX).len(), 48);
        assert_eq!(
            ciphertext(owner, &context, &[b'x'; 250], NAME_MAX).len(),
            255
        );
        let other = [&context[..24], &[8; 16]].concat();
        assert_ne!(
            nokey(owner, &context, b"a", NAME_MAX),
            nokey(owner, &other, b"a", NAME_MAX)
        );
        // Not base64url, left-over bits, too short, a length between the
        // two forms, or a ciphertext that is not a name's.
        let short = base64url(&[0; NOKEY_HASH]);
        let between = base64url(&[0; NOKEY_HASH + NOKEY_BYTES + 1]);
        let mut slash = b"a/b".to_vec();
        slash.resize(16, 0);
        owner.cipher(&context[24..], &mut slash);
        let slash = base64url(&[&[0; NOKEY_HASH][..], &slash].concat());
        for name in [&b"a+b"[..], b"AB", &short, &between, &slash] {
            assert_eq!(plaintext(owner, &context, dir, name).unwrap(), None);
        }
        assert_eq!(base64url(b"\xfb\xff"), b"-_8");
        assert_eq!(base64url_decode(b"-_8").unwrap(), b"\xfb\xff");
        assert_eq!(base64url_decode(b"-_9"), None);
    }
}
