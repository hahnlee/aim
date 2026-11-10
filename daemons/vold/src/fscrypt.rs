//! vold's file-based encryption keys (android-16.0.0_r1: FsCrypt.cpp,
//! KeyUtil.cpp and libfscrypt's options and EnsurePolicy): the system DE
//! key and the per-boot key, each user's DE and CE keys stored under
//! `/data/misc/vold/user_keys`, installed in and evicted from the data
//! filesystem's keyring with the fscrypt ioctls, and the policies they make.
//!
//! Every user-key operation is a no-op unless FBE is enabled
//! (`ro.crypto.type=file`), as `IsFbeEnabled` makes it in the original.
//! This device has no adoptable storage, so a user has only the policies
//! of internal storage.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha512};

use crate::key_storage::{Auth, EMPTY_AUTH, KeyBuffer, KeyStorage, Keystore};

pub const SET_POLICY: u64 = 0x800c6613;
pub const ADD_KEY: u64 = 0xc0506617;
pub const REMOVE_KEY: u64 = 0xc0406618;

const MODE_AES_256_XTS: u8 = 1;
const MODE_AES_256_CTS: u8 = 4;
const MODE_ADIANTUM: u8 = 9;
const MODE_AES_256_HCTR2: u8 = 10;
const MODE_AES_256_HEH: u8 = 126;
const MODE_PRIVATE: u8 = 127;
const PAD_4: u8 = 0x00;
const PAD_16: u8 = 0x02;
const DIRECT_KEY: u8 = 0x04;
const IV_INO_LBLK_64: u8 = 0x08;
const IV_INO_LBLK_32: u8 = 0x10;
const KEY_SPEC_TYPE_DESCRIPTOR: u32 = 1;
const KEY_SPEC_TYPE_IDENTIFIER: u32 = 2;
const REMOVAL_FLAG_FILES_BUSY: u32 = 0x1;
const REMOVAL_FLAG_OTHER_USERS: u32 = 0x2;
/// `struct fscrypt_add_key_arg` up to its raw key.
const ADD_KEY_ARG: usize = 80;
const REMOVE_KEY_ARG: usize = 64;
const MAX_KEY_SIZE: usize = 64;
const API_Q: u32 = 29;

const CONTENTS_MODES: &[(&str, u8)] = &[
    ("aes-256-xts", MODE_AES_256_XTS),
    ("software", MODE_AES_256_XTS),
    ("adiantum", MODE_ADIANTUM),
    ("ice", MODE_PRIVATE),
];
const FILENAMES_MODES: &[(&str, u8)] = &[
    ("aes-256-cts", MODE_AES_256_CTS),
    ("aes-256-heh", MODE_AES_256_HEH),
    ("adiantum", MODE_ADIANTUM),
    ("aes-256-hctr2", MODE_AES_256_HCTR2),
];

fn other(what: impl std::fmt::Display) -> io::Error {
    io::Error::other(what.to_string())
}

/// libfscrypt's EncryptionOptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub version: u8,
    pub contents_mode: u8,
    pub filenames_mode: u8,
    pub flags: u8,
    pub use_hw_wrapped_key: bool,
    pub dusize_4k: bool,
}

impl Options {
    /// ParseOptionsForApiLevel: `contents:filenames:flags`, each optional.
    pub fn parse(first_api_level: u32, text: &str) -> io::Result<Self> {
        let bad = |what: &str| other(format!("{what}: invalid encryption options {text}"));
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() > 3 {
            return Err(bad("too many fields"));
        }
        let lookup = |modes: &[(&str, u8)], name: &str| {
            modes.iter().find(|(n, _)| *n == name).map(|(_, id)| *id)
        };
        let mut options = Options {
            version: if first_api_level > API_Q { 2 } else { 1 },
            contents_mode: MODE_AES_256_XTS,
            filenames_mode: MODE_AES_256_CTS,
            flags: 0,
            use_hw_wrapped_key: false,
            dusize_4k: false,
        };
        if !parts[0].is_empty() {
            options.contents_mode =
                lookup(CONTENTS_MODES, parts[0]).ok_or_else(|| bad("contents mode"))?;
        }
        if options.contents_mode == MODE_ADIANTUM {
            options.filenames_mode = MODE_ADIANTUM;
        }
        if let Some(name) = parts.get(1).filter(|p| !p.is_empty()) {
            options.filenames_mode =
                lookup(FILENAMES_MODES, name).ok_or_else(|| bad("file names mode"))?;
        }
        for flag in parts
            .get(2)
            .filter(|p| !p.is_empty())
            .into_iter()
            .flat_map(|p| p.split('+'))
        {
            match flag {
                "v1" => options.version = 1,
                "v2" => options.version = 2,
                "inlinecrypt_optimized" => options.flags |= IV_INO_LBLK_64,
                "emmc_optimized" => options.flags |= IV_INO_LBLK_32,
                "wrappedkey_v0" => options.use_hw_wrapped_key = true,
                "dusize_4k" => options.dusize_4k = true,
                _ => return Err(bad("unknown flag")),
            }
        }
        // Filenames were padded to 4 bytes only by v1 CTS policies of old
        // devices.
        options.flags |= if first_api_level <= API_Q
            && options.version == 1
            && options.filenames_mode == MODE_AES_256_CTS
        {
            PAD_4
        } else {
            PAD_16
        };
        let adiantum = (
            options.contents_mode == MODE_ADIANTUM,
            options.filenames_mode == MODE_ADIANTUM,
        );
        match adiantum {
            (true, true) => options.flags |= DIRECT_KEY,
            (false, false) => {}
            _ => return Err(bad("Adiantum must be both modes or neither")),
        }
        let iv_methods = [IV_INO_LBLK_64, IV_INO_LBLK_32, DIRECT_KEY]
            .iter()
            .filter(|f| options.flags & **f != 0)
            .count();
        if iv_methods > 1 {
            return Err(bad("more than one IV generation method"));
        }
        Ok(options)
    }

    /// OptionsToStringForApiLevel, as `/data/unencrypted/mode` records it.
    pub fn to_text(&self) -> io::Result<String> {
        fn name(modes: &[(&'static str, u8)], id: u8) -> io::Result<&'static str> {
            modes
                .iter()
                .find(|(_, i)| *i == id)
                .map(|(n, _)| *n)
                .ok_or_else(|| other(format!("no name for mode {id}")))
        }
        let mut text = format!(
            "{}:{}:v{}",
            name(CONTENTS_MODES, self.contents_mode)?,
            name(FILENAMES_MODES, self.filenames_mode)?,
            self.version
        );
        for (set, flag) in [
            (self.flags & IV_INO_LBLK_64 != 0, "+inlinecrypt_optimized"),
            (self.flags & IV_INO_LBLK_32 != 0, "+emmc_optimized"),
            (self.use_hw_wrapped_key, "+wrappedkey_v0"),
            (self.dusize_4k, "+dusize_4k"),
        ] {
            if set {
                text += flag;
            }
        }
        Ok(text)
    }
}

/// The `fileencryption=` options of /data's entry in an fstab, empty when
/// it has none; an error when there is no entry
/// (init_data_file_encryption_options).
pub fn data_encryption_options(fstab: &str) -> io::Result<&str> {
    let entry = fstab
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| {
            fields.first().is_some_and(|f| !f.starts_with('#')) && fields.get(1) == Some(&"/data")
        })
        .ok_or_else(|| other("No mount point entry for /data"))?;
    Ok(entry
        .get(4)
        .and_then(|flags| {
            flags
                .split(',')
                .find_map(|f| f.strip_prefix("fileencryption="))
        })
        .unwrap_or(""))
}

/// libfscrypt's EncryptionPolicy: options, and the key's descriptor (v1)
/// or identifier (v2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub options: Options,
    pub key_raw_ref: Vec<u8>,
}

impl Policy {
    /// `struct fscrypt_key_specifier` (buildKeySpecifier).
    fn key_spec(&self) -> io::Result<[u8; 40]> {
        let (kind, size) = match self.options.version {
            1 => (KEY_SPEC_TYPE_DESCRIPTOR, 8),
            2 => (KEY_SPEC_TYPE_IDENTIFIER, 16),
            v => return Err(other(format!("Invalid encryption policy version: {v}"))),
        };
        if self.key_raw_ref.len() != size {
            return Err(other("Invalid key specifier size"));
        }
        let mut spec = [0u8; 40];
        spec[..4].copy_from_slice(&kind.to_ne_bytes());
        spec[8..8 + size].copy_from_slice(&self.key_raw_ref);
        Ok(spec)
    }

    /// `struct fscrypt_policy_v1` or `_v2` (EnsurePolicy). No data unit
    /// size is set: this filesystem's blocks are 4096 bytes.
    pub fn kernel_policy(&self) -> io::Result<Vec<u8>> {
        let o = &self.options;
        match (o.version, self.key_raw_ref.len()) {
            // FSCRYPT_POLICY_V1 is 0 in the API.
            (1, 8) => Ok([
                &[0, o.contents_mode, o.filenames_mode, o.flags],
                &self.key_raw_ref[..],
            ]
            .concat()),
            (2, 16) => Ok([
                &[2, o.contents_mode, o.filenames_mode, o.flags, 0, 0, 0, 0],
                &self.key_raw_ref[..],
            ]
            .concat()),
            _ => Err(other("Invalid encryption policy")),
        }
    }
}

/// The fscrypt ioctls on an open directory of the data filesystem.
pub trait Ioctl {
    fn ioctl(&self, dir: &Path, request: u64, arg: &mut [u8]) -> io::Result<()>;
}

/// generateKeyRef: a v1 key's descriptor, the start of its double SHA-512.
fn key_descriptor(key: &[u8]) -> Vec<u8> {
    Sha512::digest(Sha512::digest(key))[..8].to_vec()
}

/// installKey: add `key` to the keyring of `mountpoint`; the policy names
/// it by the descriptor we chose (v1) or the identifier the kernel computed
/// (v2).
pub fn install_key(
    kernel: &impl Ioctl,
    mountpoint: &Path,
    options: &Options,
    key: &KeyBuffer,
) -> io::Result<Policy> {
    if options.use_hw_wrapped_key {
        return Err(other(
            "hardware-wrapped keys need inline encryption hardware",
        ));
    }
    let mut policy = Policy {
        options: *options,
        key_raw_ref: Vec::new(),
    };
    let mut arg = KeyBuffer(vec![0; ADD_KEY_ARG + key.0.len()]);
    match options.version {
        1 => {
            policy.key_raw_ref = key_descriptor(&key.0);
            arg.0[..40].copy_from_slice(&policy.key_spec()?);
        }
        2 => arg.0[..4].copy_from_slice(&KEY_SPEC_TYPE_IDENTIFIER.to_ne_bytes()),
        v => return Err(other(format!("Invalid encryption policy version: {v}"))),
    }
    arg.0[40..44].copy_from_slice(&(key.0.len() as u32).to_ne_bytes());
    arg.0[ADD_KEY_ARG..].copy_from_slice(&key.0);
    kernel.ioctl(mountpoint, ADD_KEY, &mut arg.0).map_err(|e| {
        other(format!(
            "Failed to install fscrypt key to {}: {e}",
            mountpoint.display()
        ))
    })?;
    if options.version == 2 {
        policy.key_raw_ref = arg.0[8..24].to_vec();
    }
    log::debug!(
        "Installed fscrypt key with ref {} to {}",
        hex(&policy.key_raw_ref),
        mountpoint.display()
    );
    Ok(policy)
}

/// evictKey: remove the key of `policy` from the keyring of `mountpoint`.
pub fn evict_key(kernel: &impl Ioctl, mountpoint: &Path, policy: &Policy) -> io::Result<()> {
    let mut arg = [0u8; REMOVE_KEY_ARG];
    arg[..40].copy_from_slice(&policy.key_spec()?);
    let reference = hex(&policy.key_raw_ref);
    kernel
        .ioctl(mountpoint, REMOVE_KEY, &mut arg)
        .map_err(|e| {
            other(format!(
                "Failed to evict fscrypt key with ref {reference} from {}: {e}",
                mountpoint.display()
            ))
        })?;
    let flags = u32::from_ne_bytes(arg[40..44].try_into().unwrap());
    if flags & REMOVAL_FLAG_OTHER_USERS != 0 {
        // Keys are only added and removed as root.
        log::error!("Unexpected case: key with ref {reference} is still added by other users!");
    } else if flags & REMOVAL_FLAG_FILES_BUSY != 0 {
        log::warn!("Files still open after removing key with ref {reference}");
    }
    log::debug!(
        "Evicted fscrypt key with ref {reference} from {}",
        mountpoint.display()
    );
    Ok(())
}

/// EnsurePolicy: set `policy` on the empty `dir`, or check that it has it.
pub fn ensure_policy(kernel: &impl Ioctl, policy: &Policy, dir: &Path) -> io::Result<()> {
    let mut arg = policy.kernel_policy()?;
    kernel.ioctl(dir, SET_POLICY, &mut arg).map_err(|e| {
        let reason = if e.raw_os_error() == Some(libc::EEXIST) {
            "The directory already has a different encryption policy.".to_string()
        } else {
            e.to_string()
        };
        io::Error::new(
            e.kind(),
            format!(
                "Failed to set encryption policy of {} to {} v{}: {reason}",
                dir.display(),
                hex(&policy.key_raw_ref),
                policy.options.version
            ),
        )
    })?;
    log::info!(
        "Encryption policy of {} is {}",
        dir.display(),
        hex(&policy.key_raw_ref)
    );
    Ok(())
}

/// A key directory, `0700` and vold's own: root's, as vold runs as root.
fn prepare_root_dir(dir: &Path) -> io::Result<()> {
    // SAFETY: plain identity queries.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    crate::storage::prepare_dir(dir, 0o700, uid, gid)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The keys FsCrypt.cpp keeps: the data filesystem's options, the system DE
/// policy, the users' installed DE and CE policies, CE keys not yet stored
/// and ephemeral users.
pub struct FsCrypt<K, I> {
    pub data: PathBuf,
    /// `IsFbeEnabled`, asked at each use: `ro.crypto.type` is set by
    /// init's `mount_all`, after vold starts.
    fbe: fn() -> bool,
    pub first_api_level: u32,
    pub storage: KeyStorage<K>,
    pub kernel: I,
    options: Option<Options>,
    pub device_policy: Option<Policy>,
    ephemeral: BTreeSet<u32>,
    new_ce_keys: BTreeMap<u32, KeyBuffer>,
    pub ce: BTreeMap<u32, Policy>,
    pub de: BTreeMap<u32, Policy>,
}

impl<K: Keystore, I: Ioctl> FsCrypt<K, I> {
    pub fn new(
        data: PathBuf,
        fbe: fn() -> bool,
        first_api_level: u32,
        storage: KeyStorage<K>,
        kernel: I,
    ) -> Self {
        FsCrypt {
            data,
            fbe,
            first_api_level,
            storage,
            kernel,
            options: None,
            device_policy: None,
            ephemeral: BTreeSet::new(),
            new_ce_keys: BTreeMap::new(),
            ce: BTreeMap::new(),
            de: BTreeMap::new(),
        }
    }

    fn user_keys(&self) -> PathBuf {
        self.data.join("misc/vold/user_keys")
    }

    fn de_key_path(&self, user: u32) -> PathBuf {
        self.user_keys().join(format!("de/{user}"))
    }

    fn ce_key_dir(&self, user: u32) -> PathBuf {
        self.user_keys().join(format!("ce/{user}"))
    }

    fn user_key_temp(&self) -> PathBuf {
        self.user_keys().join("temp")
    }

    fn options(&self) -> io::Result<Options> {
        self.options
            .ok_or_else(|| other("EncryptionOptions not initialized"))
    }

    /// generateStorageKey of makeGen(s_data_options).
    fn new_key(&self) -> io::Result<KeyBuffer> {
        self.options()?;
        crate::key_storage::random(MAX_KEY_SIZE)
    }

    fn install(&self, key: &KeyBuffer) -> io::Result<Policy> {
        install_key(&self.kernel, &self.data, &self.options()?, key)
    }

    /// `IsFbeEnabled`.
    pub fn fbe(&self) -> bool {
        (self.fbe)()
    }

    /// fscrypt_initialize_systemwide_keys (`fbeEnable`): the system DE key
    /// (`/data/unencrypted/key`) and a per-boot key, installed, with their
    /// references and the options recorded where init reads them.
    pub fn initialize_systemwide_keys(&mut self, encryption_options: &str) -> io::Result<()> {
        log::info!("fscrypt_initialize_systemwide_keys");
        let options = Options::parse(self.first_api_level, encryption_options)?;
        if options.flags & IV_INO_LBLK_32 != 0 {
            return Err(other(
                "emmc_optimized is only allowed on hardware limited to 32-bit DUNs",
            ));
        }
        self.options = Some(options);
        let unencrypted = self.data.join("unencrypted");
        let device_key = self.storage.retrieve_or_generate_key(
            &unencrypted.join("key"),
            &unencrypted.join("temp"),
            &EMPTY_AUTH,
            MAX_KEY_SIZE,
        )?;
        let device_policy = self.install(&device_key)?;
        crate::key_storage::write_file(
            &unencrypted.join("mode"),
            device_policy.options.to_text()?.as_bytes(),
        )?;
        crate::key_storage::write_file(&unencrypted.join("ref"), &device_policy.key_raw_ref)?;
        log::info!(
            "Wrote system DE key reference to: {}",
            unencrypted.join("ref").display()
        );
        self.device_policy = Some(device_policy);
        let per_boot = self.install(&self.new_key()?)?;
        crate::key_storage::write_file(&unencrypted.join("per_boot_ref"), &per_boot.key_raw_ref)?;
        Ok(())
    }

    fn create_de_key(&mut self, user: u32, ephemeral: bool) -> io::Result<()> {
        let key = self.new_key()?;
        if !ephemeral {
            self.storage.store_key_atomically(
                &self.de_key_path(user),
                &self.user_key_temp(),
                &EMPTY_AUTH,
                &key,
            )?;
        }
        let policy = self.install(&key)?;
        self.de.insert(user, policy);
        log::info!("Created DE key for user {user}");
        Ok(())
    }

    fn create_ce_key(&mut self, user: u32, ephemeral: bool) -> io::Result<KeyBuffer> {
        let key = self.new_key()?;
        if !ephemeral {
            prepare_root_dir(&self.ce_key_dir(user))?;
            // Stored by set_ce_key_protection, once there is a secret to
            // store it with.
            self.new_ce_keys.insert(user, key.clone());
        }
        let policy = self.install(&key)?;
        self.ce.insert(user, policy);
        log::info!("Created CE key for user {user}");
        Ok(key)
    }

    /// get_ce_key_paths: the user's CE key directories, newest first.
    fn ce_key_paths(&self, user: u32) -> Vec<PathBuf> {
        let dir = self.ce_key_dir(user);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            log::error!("Unable to open ce key directory: {}", dir.display());
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| e.file_name().as_encoded_bytes().first() == Some(&b'c'))
            .map(|e| e.path())
            .collect();
        paths.sort();
        paths.reverse();
        paths
    }

    fn ce_key_exists(&self, user: u32) -> bool {
        self.ce_key_dir(user).join("current").exists() || !self.ce_key_paths(user).is_empty()
    }

    /// fixate_user_ce_key: destroy every other key, name this one current.
    fn fixate_ce_key(&self, user: u32, to_fix: &Path, paths: &[PathBuf]) -> io::Result<()> {
        let dir = self.ce_key_dir(user);
        for other in paths.iter().filter(|p| *p != to_fix) {
            if let Err(e) = self.storage.destroy_key(other) {
                log::error!("Failed to destroy {}: {e}", other.display());
            }
        }
        let current = dir.join("current");
        if to_fix != current {
            std::fs::rename(to_fix, &current)?;
        }
        std::fs::File::open(&dir)?.sync_all()
    }

    /// read_and_fixate_user_ce_key: the first of the user's CE keys `auth`
    /// opens.
    fn read_and_fixate_ce_key(&self, user: u32, auth: &Auth) -> io::Result<KeyBuffer> {
        let paths = self.ce_key_paths(user);
        for path in &paths {
            if let Ok(key) = self.storage.retrieve_key(path, auth) {
                // As in the original, a failed fixation leaves the key
                // usable; the next one retries it.
                if let Err(e) = self.fixate_ce_key(user, path, &paths) {
                    log::error!("Failed to fixate CE key {}: {e}", path.display());
                }
                return Ok(key);
            }
        }
        Err(other(format!(
            "Failed to find working ce key for user {user}"
        )))
    }

    fn load_all_de_keys(&mut self) -> io::Result<()> {
        let dir = self.user_keys().join("de");
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(user) = name
                .to_str()
                .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|_| entry.file_type().is_ok_and(|t| t.is_dir()))
            else {
                continue;
            };
            let key = match self.storage.retrieve_key(&entry.path(), &EMPTY_AUTH) {
                Ok(key) => key,
                // Probably a partially removed user.
                Err(_) if user != 0 => continue,
                Err(e) => return Err(e),
            };
            let policy = self.install(&key)?;
            if self.de.get(&user).is_some_and(|p| *p != policy) {
                return Err(other(format!("DE policy for user {user} changed")));
            }
            self.de.insert(user, policy);
        }
        Ok(())
    }

    /// The key part of fscrypt_init_user0: user 0's keys, created if
    /// missing, and every user's DE key installed.
    pub fn init_user0_keys(&mut self) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        let keys = self.user_keys();
        for dir in [keys.clone(), keys.join("ce"), keys.join("de")] {
            prepare_root_dir(&dir)?;
        }
        if !self.de_key_path(0).exists() {
            self.create_de_key(0, false)?;
        }
        if !self.ce_key_exists(0) {
            self.create_ce_key(0, false)?;
        }
        self.load_all_de_keys()
    }

    /// fscrypt_create_user_keys.
    pub fn create_user_keys(&mut self, user: u32, ephemeral: bool) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        if self.ce.contains_key(&user) {
            log::error!("Already exists, can't create keys for {user}");
            return Ok(());
        }
        self.create_de_key(user, ephemeral)?;
        self.create_ce_key(user, ephemeral)?;
        if ephemeral {
            self.ephemeral.insert(user);
        }
        Ok(())
    }

    /// fscrypt_destroy_user_keys: evict the user's keys, then destroy the
    /// stored ones; every step is tried.
    pub fn destroy_user_keys(&mut self, user: u32) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        let mut failure = None;
        for policy in [self.ce.remove(&user), self.de.remove(&user)]
            .into_iter()
            .flatten()
        {
            if let Err(e) = evict_key(&self.kernel, &self.data, &policy) {
                failure.get_or_insert(e);
            }
        }
        if !self.ephemeral.remove(&user) {
            if self.new_ce_keys.remove(&user).is_none() {
                for path in self.ce_key_paths(user) {
                    if let Err(e) = self.storage.destroy_key(&path) {
                        failure.get_or_insert(e);
                    }
                }
            }
            match std::fs::remove_dir(self.ce_key_dir(user)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    failure.get_or_insert(e);
                }
                _ => {}
            }
            let de = self.de_key_path(user);
            if de.exists()
                && let Err(e) = self.storage.destroy_key(&de)
            {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// fscrypt_set_ce_key_protection: store the user's CE key with
    /// `secret`, then destroy its other stored copies.
    pub fn set_ce_key_protection(&mut self, user: u32, secret: &[u8]) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        if secret.is_empty() {
            return Err(other(
                "fscrypt_set_ce_key_protection: secret must be nonempty",
            ));
        }
        if self.ephemeral.contains(&user) {
            return Ok(());
        }
        let auth = Auth(secret);
        let key = if let Some(key) = self.new_ce_keys.get(&user) {
            key.clone()
        } else if self.ce_key_exists(user) {
            match self.read_and_fixate_ce_key(user, &EMPTY_AUTH) {
                Ok(key) => key,
                Err(e) => {
                    if self.read_and_fixate_ce_key(user, &auth).is_ok() {
                        log::info!("CE key is already protected by given secret.");
                        return Ok(());
                    }
                    return Err(e);
                }
            }
        } else {
            log::warn!("CE key not found!  Regenerating it");
            self.create_ce_key(user, false)?
        };
        let dir = self.ce_key_dir(user);
        let paths = self.ce_key_paths(user);
        // get_ce_key_new_path: `current`, or a name after the newest key.
        let path = match paths.first() {
            None => dir.join("current"),
            Some(newest) => (0..u32::MAX)
                .map(|i| dir.join(format!("cx{i:010}")))
                .find(|candidate| newest < candidate)
                .ok_or_else(|| other("no name left for a CE key"))?,
        };
        self.storage
            .store_key_atomically(&path, &self.user_key_temp(), &auth, &key)?;
        self.fixate_ce_key(user, &path, &paths)?;
        if self.new_ce_keys.remove(&user).is_some() {
            log::info!("Stored CE key for new user {user}");
        }
        Ok(())
    }

    /// fscrypt_get_unlocked_users.
    pub fn unlocked_users(&self) -> Vec<u32> {
        self.ce.keys().copied().collect()
    }

    /// fscrypt_unlock_ce_storage: install the CE key `secret` opens.
    pub fn unlock_ce_storage(&mut self, user: u32, secret: &[u8]) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        if self.ce.contains_key(&user) {
            log::warn!("CE storage for user {user} is already unlocked");
            return Ok(());
        }
        let key = self.read_and_fixate_ce_key(user, &Auth(secret))?;
        let policy = self.install(&key)?;
        self.ce.insert(user, policy);
        Ok(())
    }

    /// fscrypt_lock_ce_storage: evict the CE key.
    pub fn lock_ce_storage(&mut self, user: u32) -> io::Result<()> {
        if !self.fbe() {
            return Ok(());
        }
        match self.ce.remove(&user) {
            Some(policy) => evict_key(&self.kernel, &self.data, &policy),
            None => Ok(()),
        }
    }

    /// The policy fscrypt_prepare_user_storage gives a user's directories
    /// of one kind: None without FBE, an error when the key is not
    /// installed.
    pub fn user_policy(
        &self,
        uuid: Option<&str>,
        user: u32,
        ce: bool,
    ) -> io::Result<Option<&Policy>> {
        if !self.fbe() {
            return Ok(None);
        }
        if uuid.is_some_and(|u| !u.is_empty()) {
            return Err(other("adoptable storage has no volume keys on this device"));
        }
        let (policies, kind) = if ce {
            (&self.ce, "CE")
        } else {
            (&self.de, "DE")
        };
        policies
            .get(&user)
            .map(Some)
            .ok_or_else(|| other(format!("Cannot find {kind} policy for user {user}")))
    }
}

/// The kernel's fscrypt ioctls, on a directory opened as libfscrypt and
/// KeyUtil open it.
pub struct Kernel;

impl Ioctl for Kernel {
    fn ioctl(&self, dir: &Path, request: u64, arg: &mut [u8]) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(dir)?;
        // SAFETY: `arg` is the request's argument structure, as large as
        // the request says.
        if unsafe { libc::ioctl(dir.as_raw_fd(), request as _, arg.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
