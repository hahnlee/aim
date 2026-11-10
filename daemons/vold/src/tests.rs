//! vold's FBE keys against KeyStorage.cpp's format, the fscrypt ioctls'
//! structures (the keyring's key identifier is the data filesystem's,
//! `aim_storage::fscrypt::key_identifier`) and FsCrypt.cpp's user
//! lifecycle.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::fscrypt::{self, ADD_KEY, FsCrypt, Ioctl, Options, REMOVE_KEY, SET_POLICY};
use crate::key_storage::{Auth, EMPTY_AUTH, KeyBuffer, KeyStorage, Keystore, Output, random};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};

/// A software Keystore: a blob is a version byte, the AES key and the
/// application id it is bound to.
#[derive(Default)]
struct SoftKeystore {
    deleted: RefCell<Vec<Vec<u8>>>,
    upgrade_next: Cell<bool>,
}

impl SoftKeystore {
    fn open(&self, blob: &[u8], app_id: &[u8]) -> io::Result<(Aes256Gcm, Option<Vec<u8>>)> {
        if self.deleted.borrow().iter().any(|d| d == blob) || blob[33..] != *app_id {
            return Err(io::Error::other("INVALID_KEY_BLOB"));
        }
        let upgraded = self.upgrade_next.take().then(|| {
            let mut upgraded = blob.to_vec();
            upgraded[0] += 1;
            upgraded
        });
        Ok((Aes256Gcm::new_from_slice(&blob[1..33]).unwrap(), upgraded))
    }
}

impl Keystore for SoftKeystore {
    fn generate_key(&self, app_id: &[u8]) -> io::Result<Vec<u8>> {
        Ok([&[1], random(32)?.0.as_slice(), app_id].concat())
    }
    fn encrypt(&self, blob: &[u8], app_id: &[u8], message: &[u8]) -> io::Result<Output> {
        let (cipher, upgraded) = self.open(blob, app_id)?;
        let nonce = random(12)?;
        let body = cipher
            .encrypt(Nonce::from_slice(&nonce.0), message)
            .unwrap();
        let data = KeyBuffer([nonce.0.as_slice(), &body].concat());
        Ok(Output { data, upgraded })
    }
    fn decrypt(&self, blob: &[u8], app_id: &[u8], sealed: &[u8]) -> io::Result<Output> {
        let (cipher, upgraded) = self.open(blob, app_id)?;
        let data = cipher
            .decrypt(Nonce::from_slice(&sealed[..12]), &sealed[12..])
            .map_err(|_| io::Error::other("VERIFICATION_FAILED"))?;
        Ok(Output {
            data: KeyBuffer(data),
            upgraded,
        })
    }
    fn delete_key(&self, blob: &[u8]) -> io::Result<()> {
        self.deleted.borrow_mut().push(blob.to_vec());
        Ok(())
    }
}

/// The data filesystem's fscrypt ioctls as fscrypt.rst specifies them, for
/// root: v2 keys by identifier, policies on empty directories.
#[derive(Default)]
struct Keyring {
    keys: RefCell<BTreeSet<[u8; 16]>>,
    policies: RefCell<BTreeMap<PathBuf, Vec<u8>>>,
}

/// Linux's ENOKEY.
const ENOKEY: i32 = 126;

fn errno(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

impl Ioctl for Keyring {
    fn ioctl(&self, dir: &Path, request: u64, arg: &mut [u8]) -> io::Result<()> {
        let word = |at: usize| u32::from_ne_bytes(arg[at..at + 4].try_into().unwrap());
        match request {
            ADD_KEY => {
                assert_eq!(word(0), 2, "a v2 key by identifier");
                let size = word(40) as usize;
                assert_eq!(arg.len(), 80 + size);
                assert!(
                    arg[44..80].iter().all(|b| *b == 0),
                    "key_id and reserved are zero"
                );
                let id = aim_storage::fscrypt::key_identifier(&arg[80..]);
                arg[8..24].copy_from_slice(&id);
                self.keys.borrow_mut().insert(id);
            }
            REMOVE_KEY => {
                assert_eq!((arg.len(), word(0)), (64, 2));
                let id: [u8; 16] = arg[8..24].try_into().unwrap();
                if !self.keys.borrow_mut().remove(&id) {
                    return Err(errno(ENOKEY));
                }
                arg[40..44].fill(0);
            }
            SET_POLICY => {
                assert_eq!((arg.len(), arg[0]), (24, 2), "struct fscrypt_policy_v2");
                if let Some(existing) = self.policies.borrow().get(dir) {
                    return if *existing == arg {
                        Ok(())
                    } else {
                        Err(errno(libc::EEXIST))
                    };
                }
                if std::fs::read_dir(dir)?.next().is_some() {
                    return Err(errno(libc::ENOTEMPTY));
                }
                if !self.keys.borrow().contains(&arg[8..24]) {
                    return Err(errno(ENOKEY));
                }
                self.policies
                    .borrow_mut()
                    .insert(dir.to_path_buf(), arg.to_vec());
            }
            _ => return Err(errno(libc::ENOTTY)),
        }
        Ok(())
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("vold-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store() -> KeyStorage<SoftKeystore> {
    KeyStorage::new(SoftKeystore::default(), "/usr/bin/true".into())
}

#[test]
fn stored_keys_have_keystorage_format_and_round_trip() {
    let scratch = Scratch::new("format");
    let storage = store();
    let key = random(64).unwrap();
    let (path, tmp) = (scratch.0.join("key"), scratch.0.join("temp"));
    storage
        .store_key_atomically(&path, &tmp, &EMPTY_AUTH, &key)
        .unwrap();
    assert!(!tmp.exists());
    let read = |name: &str| std::fs::read(path.join(name)).unwrap();
    assert_eq!(read("version"), b"1");
    assert_eq!(read("secdiscardable").len(), 1 << 14);
    assert_eq!(read("encrypted_key").len(), 12 + 64 + 16);
    assert_eq!(storage.retrieve_key(&path, &EMPTY_AUTH).unwrap(), key);
    assert!(
        storage
            .store_key_atomically(&path, &tmp, &EMPTY_AUTH, &key)
            .is_err()
    );

    // A Keystore upgrade replaces the blob and deletes the old one.
    let old = read("keymaster_key_blob");
    storage.keystore.upgrade_next.set(true);
    assert_eq!(storage.retrieve_key(&path, &EMPTY_AUTH).unwrap(), key);
    assert_eq!(read("keymaster_key_blob")[0], old[0] + 1);
    assert!(!path.join("keymaster_key_blob_upgraded").exists());
    assert_eq!(*storage.keystore.deleted.borrow(), vec![old]);
    assert_eq!(storage.retrieve_key(&path, &EMPTY_AUTH).unwrap(), key);

    // The secdiscardable hash is the application id: without the file the
    // key is gone.
    std::fs::write(path.join("secdiscardable"), &random(1 << 14).unwrap().0).unwrap();
    assert!(storage.retrieve_key(&path, &EMPTY_AUTH).is_err());

    // A secret seals the key without Keystore.
    let sealed = scratch.0.join("sealed");
    storage
        .store_key_atomically(&sealed, &tmp, &Auth(b"secret"), &key)
        .unwrap();
    assert!(!sealed.join("secdiscardable").exists());
    assert!(!sealed.join("keymaster_key_blob").exists());
    assert_eq!(
        storage.retrieve_key(&sealed, &Auth(b"secret")).unwrap(),
        key
    );
    assert!(storage.retrieve_key(&sealed, &Auth(b"wrong")).is_err());
    assert!(storage.retrieve_key(&sealed, &EMPTY_AUTH).is_err());

    let blob = read("keymaster_key_blob");
    storage.destroy_key(&path).unwrap();
    assert!(!path.exists());
    assert!(storage.keystore.deleted.borrow().contains(&blob));
}

#[test]
fn a_binding_seed_is_set_before_keys_are_used() {
    let scratch = Scratch::new("seed");
    let key = random(64).unwrap();
    let (path, tmp) = (scratch.0.join("key"), scratch.0.join("temp"));
    let bound = store();
    bound.set_binding_seed(b"seed").unwrap();
    assert!(bound.set_binding_seed(b"seed").is_err());
    bound
        .store_key_atomically(&path, &tmp, &Auth(b"secret"), &key)
        .unwrap();
    assert_eq!(bound.retrieve_key(&path, &Auth(b"secret")).unwrap(), key);
    let unbound = store();
    assert!(unbound.retrieve_key(&path, &Auth(b"secret")).is_err());
    assert!(unbound.set_binding_seed(b"seed").is_err());
}

#[test]
fn options_parse_as_libfscrypt() {
    let options = Options::parse(36, "aes-256-xts:aes-256-cts:v2").unwrap();
    assert_eq!(
        (
            options.version,
            options.contents_mode,
            options.filenames_mode,
            options.flags
        ),
        (2, 1, 4, 0x02)
    );
    assert_eq!(options.to_text().unwrap(), "aes-256-xts:aes-256-cts:v2");
    assert_eq!(Options::parse(36, "").unwrap(), options);
    assert_eq!(Options::parse(29, "aes-256-xts").unwrap().version, 1);
    assert_eq!(Options::parse(29, "aes-256-xts").unwrap().flags, 0x00);
    assert_eq!(Options::parse(36, "adiantum").unwrap().flags, 0x02 | 0x04);
    assert!(Options::parse(36, "adiantum:aes-256-cts").is_err());
    assert!(Options::parse(36, "aes-256-xts:aes-256-cts:v2+bogus").is_err());
    assert!(Options::parse(36, "::inlinecrypt_optimized+emmc_optimized").is_err());
    let fstab = "# comment\n\
        /dev/block/vda /system ext4 ro wait\n\
        /data /data none bind latemount,fileencryption=aes-256-xts:aes-256-cts:v2,quota\n";
    assert_eq!(
        fscrypt::data_encryption_options(fstab).unwrap(),
        "aes-256-xts:aes-256-cts:v2"
    );
    assert_eq!(
        fscrypt::data_encryption_options("\n/data /data none bind wait\n").unwrap(),
        ""
    );
    assert!(fscrypt::data_encryption_options("# /data /data none bind\n").is_err());
}

/// A key stored with a secret as the original stores it: its 64 bytes
/// 0..63 sealed by AES-256-GCM (nonce a0..ab) under the first 32 bytes of
/// SHA-512 of the keygen prefix, padded to 128 bytes, and the secret
/// (computed with Python's hashlib and CryptoKit).
#[test]
fn a_golden_secret_protected_key_is_read() {
    let scratch = Scratch::new("golden");
    let dir = scratch.0.join("current");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("version"), b"1").unwrap();
    let sealed = "a0a1a2a3a4a5a6a7a8a9aaab2226744ec8f9fab82ccc7e339d8e356f958b82c1\
                  ae4edf887f2ed353cd5b23547bba4f33612c284dc9456e01b9016b38b1fbe520\
                  4f2162fee037b161d34872e9903b4aa7069d48fcdba4d35bc944c5bd";
    let sealed: Vec<u8> = (0..sealed.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&sealed[i..i + 2], 16).unwrap())
        .collect();
    std::fs::write(dir.join("encrypted_key"), sealed).unwrap();
    let key = store().retrieve_key(&dir, &Auth(b"golden secret")).unwrap();
    assert_eq!(key.0, (0..64).collect::<Vec<u8>>());
    assert!(store().retrieve_key(&dir, &Auth(b"other secret")).is_err());
}

#[test]
fn keys_install_and_evict_through_the_ioctls() {
    let scratch = Scratch::new("ioctl");
    let kernel = Keyring::default();
    let options = Options::parse(36, "aes-256-xts:aes-256-cts:v2").unwrap();
    let key = random(64).unwrap();
    let policy = fscrypt::install_key(&kernel, &scratch.0, &options, &key).unwrap();
    assert_eq!(
        policy.key_raw_ref,
        aim_storage::fscrypt::key_identifier(&key.0)
    );

    let dir = scratch.0.join("dir");
    std::fs::create_dir(&dir).unwrap();
    fscrypt::ensure_policy(&kernel, &policy, &dir).unwrap();
    fscrypt::ensure_policy(&kernel, &policy, &dir).unwrap();
    let full = scratch.0.join("full");
    std::fs::create_dir_all(full.join("x")).unwrap();
    assert!(fscrypt::ensure_policy(&kernel, &policy, &full).is_err());

    fscrypt::evict_key(&kernel, &scratch.0, &policy).unwrap();
    assert!(kernel.keys.borrow().is_empty());
    let error = fscrypt::evict_key(&kernel, &scratch.0, &policy).unwrap_err();
    assert!(error.to_string().contains("Failed to evict"));
}

/// A data directory as init prepares it: `installkey` makes
/// /data/unencrypted, init.rc /data/misc/vold.
fn crypt(data: &Path) -> FsCrypt<SoftKeystore, Keyring> {
    std::fs::create_dir_all(data.join("unencrypted")).unwrap();
    std::fs::create_dir_all(data.join("misc/vold")).unwrap();
    FsCrypt::new(data.to_path_buf(), || true, 36, store(), Keyring::default())
}

#[test]
fn users_keys_are_created_unlocked_locked_and_destroyed() {
    let scratch = Scratch::new("users");
    let data = &scratch.0;
    let options = "aes-256-xts:aes-256-cts:v2";
    let mut fbe = crypt(data);
    fbe.initialize_systemwide_keys(options).unwrap();
    let device = fbe.device_policy.clone().unwrap();
    let unencrypted = data.join("unencrypted");
    assert_eq!(
        std::fs::read(unencrypted.join("ref")).unwrap(),
        device.key_raw_ref
    );
    assert_eq!(
        std::fs::read_to_string(unencrypted.join("mode")).unwrap(),
        options
    );
    assert_eq!(
        std::fs::read(unencrypted.join("per_boot_ref"))
            .unwrap()
            .len(),
        16
    );
    assert!(unencrypted.join("key/encrypted_key").exists());

    fbe.init_user0_keys().unwrap();
    let keys = data.join("misc/vold/user_keys");
    assert!(keys.join("de/0/keymaster_key_blob").exists());
    // A new CE key waits in memory for its secret.
    assert_eq!(std::fs::read_dir(keys.join("ce/0")).unwrap().count(), 0);
    assert_eq!(fbe.unlocked_users(), vec![0]);

    fbe.create_user_keys(10, false).unwrap();
    let (de, ce) = (fbe.de[&10].clone(), fbe.ce[&10].clone());
    assert!(keys.join("de/10/version").exists());
    assert!(fbe.set_ce_key_protection(10, b"").is_err());
    fbe.set_ce_key_protection(10, b"synthetic password secret")
        .unwrap();
    assert!(keys.join("ce/10/current/encrypted_key").exists());
    assert!(!keys.join("ce/10/current/secdiscardable").exists());
    assert_eq!(fbe.user_policy(None, 10, true).unwrap(), Some(&ce));

    fbe.lock_ce_storage(10).unwrap();
    assert!(
        !fbe.kernel
            .keys
            .borrow()
            .iter()
            .any(|k| *k == ce.key_raw_ref[..])
    );
    assert_eq!(fbe.unlocked_users(), vec![0]);
    assert!(fbe.user_policy(None, 10, true).is_err());
    assert!(fbe.unlock_ce_storage(10, b"wrong").is_err());
    fbe.unlock_ce_storage(10, b"synthetic password secret")
        .unwrap();
    assert_eq!(fbe.ce[&10], ce);
    assert_eq!(fbe.unlocked_users(), vec![0, 10]);

    // After a reboot the system DE key and every DE key load again.
    let mut rebooted = crypt(data);
    rebooted.initialize_systemwide_keys(options).unwrap();
    assert_eq!(rebooted.device_policy, Some(device));
    rebooted.init_user0_keys().unwrap();
    assert_eq!(rebooted.de[&10], de);
    // User 0's CE key was never given its secret, so it was not stored and
    // a new one replaces it, as in the original; user 10's stays locked.
    assert_eq!(rebooted.unlocked_users(), vec![0]);
    assert_ne!(rebooted.ce[&0], fbe.ce[&0]);

    fbe.destroy_user_keys(10).unwrap();
    assert!(!keys.join("de/10").exists());
    assert!(!keys.join("ce/10").exists());
    assert!(
        !fbe.kernel
            .keys
            .borrow()
            .iter()
            .any(|k| *k == de.key_raw_ref[..])
    );
    assert!(fbe.user_policy(None, 10, false).is_err());
}

#[test]
fn ephemeral_users_and_devices_without_fbe_store_nothing() {
    let scratch = Scratch::new("ephemeral");
    let mut fbe = crypt(&scratch.0);
    fbe.initialize_systemwide_keys("").unwrap();
    fbe.create_user_keys(11, true).unwrap();
    fbe.set_ce_key_protection(11, b"secret").unwrap();
    assert!(!scratch.0.join("misc/vold/user_keys/de/11").exists());
    fbe.destroy_user_keys(11).unwrap();
    assert!(fbe.kernel.keys.borrow().len() == 2);

    let mut plain = FsCrypt::new(
        scratch.0.join("plain"),
        || false,
        36,
        store(),
        Keyring::default(),
    );
    plain.init_user0_keys().unwrap();
    plain.create_user_keys(10, false).unwrap();
    plain.unlock_ce_storage(10, b"").unwrap();
    assert_eq!(plain.user_policy(None, 10, true).unwrap(), None);
    assert!(plain.kernel.keys.borrow().is_empty());
}
