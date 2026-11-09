//! vold's file-based encryption keys in the data filesystem
//! (android-16.0.0_r1: KeyUtil.cpp, and libfscrypt's options and
//! EnsurePolicy): keys installed in and evicted from its keyring with the
//! fscrypt ioctls, and the policies they make.

use std::io;
use std::path::Path;

use sha2::{Digest, Sha512};

use crate::key_storage::KeyBuffer;

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

/// The `fileencryption=` options of /data's entry in an fstab.
pub fn data_encryption_options(fstab: &str) -> Option<&str> {
    fstab
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| fields.get(1) == Some(&"/data"))?
        .get(4)?
        .split(',')
        .find_map(|flag| flag.strip_prefix("fileencryption="))
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
