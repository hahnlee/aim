//! Storage keys kept as vold's KeyStorage.cpp keeps them (android-16.0.0_r1):
//! a directory holding `version`, `encrypted_key` and, without a secret,
//! `secdiscardable` and `keymaster_key_blob`. Without a secret the key is
//! sealed by a Keystore AES-256-GCM key whose application id is the hash of
//! `secdiscardable`, so discarding that file destroys the key; with one, by
//! AES-256-GCM under a key hashed from the secret (LockSettings derives it
//! from the synthetic password, which Keystore already protects).

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha512};

const AES_KEY_BYTES: usize = 32;
pub const GCM_NONCE_BYTES: usize = 12;
pub const GCM_MAC_BYTES: usize = 16;
const SECDISCARDABLE_BYTES: usize = 1 << 14;
const SHA512_CBLOCK: usize = 128;
const CURRENT_VERSION: &str = "1";
const HASH_PREFIX_SECDISCARDABLE: &str = "Android secdiscardable SHA512";
const HASH_PREFIX_KEYGEN: &str = "Android key wrapping key generation SHA512";
const ENCRYPTED_KEY: &str = "encrypted_key";
const KEY_BLOB: &str = "keymaster_key_blob";
const KEY_BLOB_UPGRADED: &str = "keymaster_key_blob_upgraded";
const SECDISCARDABLE: &str = "secdiscardable";
const VERSION: &str = "version";

/// Key bytes, zeroed when dropped (vold's KeyBuffer).
#[derive(Clone, PartialEq, Eq)]
pub struct KeyBuffer(pub Vec<u8>);

impl Drop for KeyBuffer {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // SAFETY: a valid, aligned byte of our own buffer.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

impl std::fmt::Debug for KeyBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyBuffer({} bytes)", self.0.len())
    }
}

/// A secret file's bytes, read into a buffer that is never reallocated, so
/// no copy is left unzeroed.
fn read_key(path: &Path) -> io::Result<KeyBuffer> {
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len() as usize;
    let mut bytes = KeyBuffer(Vec::with_capacity(size + 1));
    file.read_to_end(&mut bytes.0)?;
    Ok(bytes)
}

/// ReadRandomBytes: `count` bytes of /dev/urandom.
pub fn random(count: usize) -> io::Result<KeyBuffer> {
    let mut bytes = KeyBuffer(vec![0; count]);
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes.0)?;
    Ok(bytes)
}

/// What a Keystore operation produced, and the key blob Keystore upgraded
/// to begin it, if it did.
pub struct Output {
    pub data: KeyBuffer,
    pub upgraded: Option<Vec<u8>>,
}

/// vold's Keystore (Keystore.cpp): keystore2's TRUSTED_ENVIRONMENT security
/// level, with keys of `Domain::BLOB` in vold's namespace.
pub trait Keystore {
    /// generateKeyStorageKey: an AES-256 GCM key bound to `app_id`, without
    /// user authentication; rollback resistant where Keystore can do it.
    fn generate_key(&self, app_id: &[u8]) -> io::Result<Vec<u8>>;
    /// encryptWithKeystoreKey: the nonce Keystore chose, the ciphertext and
    /// the MAC.
    fn encrypt(&self, blob: &[u8], app_id: &[u8], message: &[u8]) -> io::Result<Output>;
    /// decryptWithKeystoreKey of a nonce, ciphertext and MAC.
    fn decrypt(&self, blob: &[u8], app_id: &[u8], sealed: &[u8]) -> io::Result<Output>;
    fn delete_key(&self, blob: &[u8]) -> io::Result<()>;
}

/// KeyAuthentication: the secret a key is stored with; empty for keys
/// only Keystore protects.
pub struct Auth<'a>(pub &'a [u8]);

pub const EMPTY_AUTH: Auth<'static> = Auth(&[]);

impl Auth<'_> {
    fn uses_keystore(&self) -> bool {
        self.0.is_empty()
    }
}

fn hash_with_prefix(prefix: &str, data: &[u8]) -> [u8; 64] {
    let mut padded = prefix.as_bytes().to_vec();
    padded.resize(SHA512_CBLOCK, 0);
    Sha512::new()
        .chain_update(&padded)
        .chain_update(data)
        .finalize()
        .into()
}

fn other(what: impl std::fmt::Display) -> io::Error {
    io::Error::other(what.to_string())
}

pub(crate) fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
    // writeStringToFile: not through a symlink, written, then synced.
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    io::Write::write_all(&mut &file, data)?;
    file.sync_all()
}

fn fsync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// StorageBindingInfo: whether key storage keys are bound to a seed the
/// platform provides, decided by the first of setting it or using a key.
enum Binding {
    Uninitialized,
    InUse(KeyBuffer),
    NotUsed,
}

/// The store of storage keys: KeyStorage.cpp over a Keystore, and the
/// original `secdiscard` program that discards a destroyed key's files.
pub struct KeyStorage<K> {
    pub keystore: K,
    pub secdiscard: PathBuf,
    binding: std::sync::Mutex<Binding>,
}

impl<K: Keystore> KeyStorage<K> {
    pub fn new(keystore: K, secdiscard: PathBuf) -> Self {
        KeyStorage {
            keystore,
            secdiscard,
            binding: std::sync::Mutex::new(Binding::Uninitialized),
        }
    }

    /// setKeyStorageBindingSeed: only before any key is stored or read.
    pub fn set_binding_seed(&self, seed: &[u8]) -> io::Result<()> {
        let mut binding = self.binding.lock().unwrap();
        match *binding {
            Binding::Uninitialized => {
                *binding = Binding::InUse(KeyBuffer(seed.to_vec()));
                Ok(())
            }
            Binding::InUse(_) => Err(other("key storage binding seed already set")),
            Binding::NotUsed => Err(other("key storage already in use without binding")),
        }
    }

    /// createSecdiscardable or readSecdiscardable: the hash of the file, or
    /// nothing when it is absent.
    fn secdiscardable_hash(path: &Path, create: bool) -> io::Result<Vec<u8>> {
        let bytes = if create {
            let bytes = random(SECDISCARDABLE_BYTES)?;
            write_file(path, &bytes.0)?;
            bytes
        } else {
            match read_key(path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(e),
            }
        };
        Ok(hash_with_prefix(HASH_PREFIX_SECDISCARDABLE, &bytes.0).to_vec())
    }

    /// generateAppId: the secdiscardable hash, the secret and the binding
    /// seed.
    fn app_id(&self, auth: &Auth, secdiscardable_hash: &[u8]) -> KeyBuffer {
        let mut app_id = KeyBuffer([secdiscardable_hash, auth.0].concat());
        let mut binding = self.binding.lock().unwrap();
        match &*binding {
            Binding::Uninitialized => *binding = Binding::NotUsed,
            Binding::InUse(seed) => app_id.0.extend_from_slice(&seed.0),
            Binding::NotUsed => {}
        }
        app_id
    }

    /// BeginKeystoreOp's upgrade: the upgraded blob replaces the stored one,
    /// then Keystore forgets the old one (there are no checkpoints to defer
    /// to).
    fn commit_upgrade(&self, dir: &Path, upgraded: Option<Vec<u8>>) -> io::Result<()> {
        let Some(upgraded) = upgraded else {
            return Ok(());
        };
        let (blob_file, upgraded_file) = (dir.join(KEY_BLOB), dir.join(KEY_BLOB_UPGRADED));
        let old = fs::read(&blob_file)?;
        write_file(&upgraded_file, &upgraded)?;
        fs::rename(&upgraded_file, &blob_file)?;
        fsync_dir(dir)?;
        if let Err(e) = self.keystore.delete_key(&old) {
            log::warn!(
                "Failed to delete old key {} from Keystore: {e}",
                blob_file.display()
            );
        }
        log::info!("Key upgraded: {}", blob_file.display());
        Ok(())
    }

    /// DeleteUpgradedKey: an upgraded blob left over by a failed update.
    fn delete_leftover_upgrade(&self, dir: &Path) {
        let path = dir.join(KEY_BLOB_UPGRADED);
        let Ok(blob) = fs::read(&path) else {
            return;
        };
        if let Err(e) = self.keystore.delete_key(&blob) {
            log::warn!(
                "Failed to delete leftover upgraded key {}: {e}",
                path.display()
            );
        }
        if let Err(e) = fs::remove_file(&path) {
            log::warn!(
                "Failed to unlink leftover upgraded key {}: {e}",
                path.display()
            );
        }
    }

    /// storeKey: make `dir` and store `key` in it.
    fn store_key(&self, dir: &Path, auth: &Auth, key: &KeyBuffer) -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(dir)?;
        write_file(&dir.join(VERSION), CURRENT_VERSION.as_bytes())?;
        let hash = if auth.uses_keystore() {
            Self::secdiscardable_hash(&dir.join(SECDISCARDABLE), true)?
        } else {
            Vec::new()
        };
        let app_id = self.app_id(auth, &hash);
        let encrypted = if auth.uses_keystore() {
            let blob = self.keystore.generate_key(&app_id.0)?;
            write_file(&dir.join(KEY_BLOB), &blob)?;
            self.delete_leftover_upgrade(dir);
            let sealed = self.keystore.encrypt(&blob, &app_id.0, &key.0)?;
            self.commit_upgrade(dir, sealed.upgraded)?;
            if sealed.data.0.len() != GCM_NONCE_BYTES + key.0.len() + GCM_MAC_BYTES {
                return Err(other("Keystore GCM output has the wrong size"));
            }
            sealed.data
        } else {
            encrypt_without_keystore(&app_id.0, key)?
        };
        write_file(&dir.join(ENCRYPTED_KEY), &encrypted.0)?;
        fsync_dir(dir)
    }

    /// storeKeyAtomically: store under `tmp`, then rename it to `path`.
    pub fn store_key_atomically(
        &self,
        path: &Path,
        tmp: &Path,
        auth: &Auth,
        key: &KeyBuffer,
    ) -> io::Result<()> {
        if path.exists() {
            return Err(other(format!(
                "Already exists, cannot create key at: {}",
                path.display()
            )));
        }
        if tmp.exists() {
            // It may be partially created, so its errors do not matter.
            let _ = self.destroy_key(tmp);
        }
        self.store_key(tmp, auth, key)?;
        fs::rename(tmp, path)?;
        fsync_dir(path.parent().unwrap_or(Path::new("/")))?;
        log::debug!("Stored key {}", path.display());
        Ok(())
    }

    /// retrieveKey: the key stored in `dir` with `auth`.
    pub fn retrieve_key(&self, dir: &Path, auth: &Auth) -> io::Result<KeyBuffer> {
        let version = fs::read(dir.join(VERSION))?;
        if version != CURRENT_VERSION.as_bytes() {
            return Err(other(format!(
                "Version mismatch, expected {CURRENT_VERSION} got {}",
                String::from_utf8_lossy(&version)
            )));
        }
        let hash = Self::secdiscardable_hash(&dir.join(SECDISCARDABLE), false)?;
        let app_id = self.app_id(auth, &hash);
        let encrypted = read_key(&dir.join(ENCRYPTED_KEY))?;
        if !auth.uses_keystore() {
            return decrypt_without_keystore(&app_id.0, &encrypted.0);
        }
        self.delete_leftover_upgrade(dir);
        let blob = fs::read(dir.join(KEY_BLOB))?;
        let opened = self.keystore.decrypt(&blob, &app_id.0, &encrypted.0)?;
        self.commit_upgrade(dir, opened.upgraded)?;
        Ok(opened.data)
    }

    /// retrieveOrGenerateKey: the key in `path`, or a new one stored there.
    pub fn retrieve_or_generate_key(
        &self,
        path: &Path,
        tmp: &Path,
        auth: &Auth,
        size: usize,
    ) -> io::Result<KeyBuffer> {
        if path.exists() {
            return self.retrieve_key(path, auth);
        }
        log::info!("Creating new key in {}", path.display());
        let key = random(size)?;
        self.store_key_atomically(path, tmp, auth, &key)?;
        Ok(key)
    }

    /// destroyKey: forget Keystore's keys, discard the files that could
    /// recover the key, and remove the directory; every step is tried.
    pub fn destroy_key(&self, dir: &Path) -> io::Result<()> {
        let mut failure = None;
        let mut discard = vec![dir.join(ENCRYPTED_KEY)];
        let secdiscardable = dir.join(SECDISCARDABLE);
        if secdiscardable.exists() {
            discard.push(secdiscardable);
        }
        for name in [KEY_BLOB, KEY_BLOB_UPGRADED] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            if let Err(e) = fs::read(&path).and_then(|blob| self.keystore.delete_key(&blob)) {
                log::error!("Failed to delete {} from Keystore: {e}", path.display());
                failure.get_or_insert(e);
            }
            discard.push(path);
        }
        let status = std::process::Command::new(&self.secdiscard)
            .arg("--")
            .args(&discard)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                failure.get_or_insert(other(format!("secdiscard: {status}")));
            }
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
        if let Err(e) = fs::remove_dir_all(dir) {
            failure.get_or_insert(e);
        }
        failure.map_or(Ok(()), Err)
    }
}

fn keygen_key(app_id: &[u8]) -> KeyBuffer {
    KeyBuffer(hash_with_prefix(HASH_PREFIX_KEYGEN, app_id)[..AES_KEY_BYTES].to_vec())
}

/// encryptWithoutKeystore: nonce, ciphertext and MAC of AES-256-GCM.
fn encrypt_without_keystore(app_id: &[u8], plain: &KeyBuffer) -> io::Result<KeyBuffer> {
    let key = keygen_key(app_id);
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(other)?;
    let nonce = random(GCM_NONCE_BYTES)?;
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce.0), plain.0.as_slice())
        .map_err(|_| other("GCM encryption failed"))?;
    Ok(KeyBuffer([nonce.0.as_slice(), &sealed].concat()))
}

/// decryptWithoutKeystore: fails unless the MAC matches.
fn decrypt_without_keystore(app_id: &[u8], sealed: &[u8]) -> io::Result<KeyBuffer> {
    if sealed.len() < GCM_NONCE_BYTES + GCM_MAC_BYTES {
        return Err(other(format!("GCM ciphertext too small: {}", sealed.len())));
    }
    let key = keygen_key(app_id);
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(other)?;
    let (nonce, body) = sealed.split_at(GCM_NONCE_BYTES);
    cipher
        .decrypt(Nonce::from_slice(nonce), body)
        .map(KeyBuffer)
        .map_err(|_| other("GCM decryption failed: wrong secret or corrupt key"))
}
