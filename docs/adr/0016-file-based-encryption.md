# ADR 0016: File-based encryption on a Mac data volume

Status: accepted 2026-11-10 (#2761). The owner chose the recommended path:
option B, CE protected by a Mac Keychain item, FileVault for confidentiality
at rest (D not now), and existing data directories recreated by a breaking
migration on a branch. Slice 1 is implemented (#2845, #2846); the guest is
a device without file-based encryption (FBE) until slices 2-3 land.

## Context

The pinned Android 16 decides FBE once, from two properties that init sets
in `queue_fs_event` when fs_mgr mounts a `/data` whose fstab entry has
`fileencryption=`: `ro.crypto.state=encrypted` and `ro.crypto.type=file`.
`StorageManager.isFileEncrypted()` is `RoSystemProperties.CRYPTO_ENCRYPTED
&& CRYPTO_FILE_ENCRYPTED` (both from `android.sysprop.CryptoProperties`;
checked in the image's `framework.jar`). Everything else follows from them:

- init's `installkey /data` runs `vdc cryptfs enablefilecrypto`, and vold's
  `fscrypt_initialize_systemwide_keys` installs the system DE key and writes
  its policy reference under `/data/unencrypted`; the ~40
  `mkdir ... encryption=Require` of `init.rc` then set that policy on each
  top-level `/data` directory (`None` for the per-user parents,
  `DeleteIfNecessary` for staging and rollback, `key=per_boot_ref` for
  `/data/per_boot`).
- vold's `init_user0`, `createUserStorageKeys`, `prepareUserStorage`,
  `setCeStorageProtection` and `unlockCeStorage` create each user's DE and
  CE keys (wrapped by Keystore2/KeyMint, CE also by the user's synthetic
  password), add them with `FS_IOC_ADD_ENCRYPTION_KEY` and set v2 policies
  on `/data/user_de/<u>`, `/data/user/<u>`, `/data/{system,misc,vendor}_{de,ce}/<u>`
  and `/data/media/<u>`.
- The package manager's `fixAppsDataOnBoot` prepares only DE data at boot
  and leaves CE to the user's unlock; without FBE, `maybeMigrateAppDataLIF`
  migrates every system app to the storage it defaults to and creates its
  data again, on every boot. In the app host (`system_package_internal_storage.rs`,
  a port of the original) that ends package construction 0.44-0.98 s after
  it would on an FBE device (#2761). Batching installd's calls did not help
  (#2749).
- CTS checks the claim, not just the property: `EncryptionTest` requires
  `encrypted`/`file` for this API level, and `FileBasedEncryptionPolicyTest`
  reads an app file's policy with `FS_IOC_GET_ENCRYPTION_POLICY_EX` and
  validates its modes and flags.

Today every owner says "no FBE": the fstab (`image/vendor/etc/fstab.aim`),
guest-init's `mount_all` (`ro.crypto.state=unencrypted`), `installkey` and
`mkdir encryption=` (no-ops), the replaced vold (directories without keys;
`unlockCeStorage` only records the user), and the syscall layer, which has
no fscrypt ioctl. Setting the two properties alone would be the
success-returning stub AGENTS.md forbids: the ioctls answer ENOTTY, CE
storage is never locked, and CTS fails. So the work is to make the guest
an FBE device at its owners, or to keep the cost.

## What Linux semantics an honest FBE needs

From `Documentation/filesystems/fscrypt.rst` (v6.12) and ext4/f2fs:

1. **Policies per inode, inherited.** `FS_IOC_SET_ENCRYPTION_POLICY` on an
   empty directory owned by the caller (`EACCES`, `ENOTDIR`, `ENOTEMPTY`,
   `EEXIST` for a different policy, `EINVAL` for bad modes or flags; a v2
   policy needs its key added by the caller or CAP_FOWNER, else `ENOKEY`).
   Every inode created below inherits it, with its own nonce
   (`FS_IOC_GET_ENCRYPTION_NONCE`). `link` and `rename` into a directory of
   another policy fail with `EXDEV`.
2. **Queries.** `FS_IOC_GET_ENCRYPTION_POLICY` (`EINVAL` for v2,
   `ENODATA` when unencrypted), `..._POLICY_EX`, `FS_ENCRYPT_FL` in
   `FS_IOC_GETFLAGS`, `STATX_ATTR_ENCRYPTED`.
3. **A filesystem keyring.** `FS_IOC_ADD_ENCRYPTION_KEY` returns the v2
   key identifier (HKDF-SHA512 of the raw key), tracks which users added
   it; `FS_IOC_REMOVE_ENCRYPTION_KEY[_ALL_USERS]` drops a user's claim and
   evicts the key with `FILES_BUSY`/`OTHER_USERS` status;
   `FS_IOC_GET_ENCRYPTION_KEY_STATUS`. The keyring is the filesystem's, so
   one state shared by every guest process.
4. **Locked directories behave as locked.** Without the key, `readdir`
   returns no-key names (stable, opaque, usable by `unlink`/`rmdir`),
   `open` of a file, `readlink` and every create fail with `ENOKEY`,
   `stat` still works. After the key is added the plaintext names return.

## Options

**A. Properties only.** Rejected: a stub (above).

**B. fscrypt in the syscall layer over plaintext APFS ("policy-faithful").**
The ABI implements 1-4: policies kept on the host inode (an xattr, as owners
are, `guest-init-contract.md` section 6) and in a small policy-root table
held by a storage owner in guest-init (the keyring's owner too, reached as
the fs-verity owner is), with a generation word in shared memory so the hot
path pays nothing while every key in use is present, which is the steady
state after user 0's unlock. Names in a locked directory are encoded with a
per-volume secret of the owner, so they are stable and can be decoded for
`unlink`. The bytes stay plaintext in the data image; confidentiality at
rest is the Mac's (FileVault) or option D's. The policy reports the modes
vold asked for, as Linux reports them under `inlinecrypt` without saying who
encrypts.

**C. Real per-file encryption in the syscall layer.** Every read, write and
`mmap` of `/data` (dex, `.so`, databases) through AES-XTS in user space,
names through AES-CTS. Rejected: the cost is on every app's I/O and page
faults, and it duplicates what APFS already does in the kernel.

**D. B, plus the data image encrypted by APFS** (an encrypted ASIF or APFS
volume, its key in the Mac user's login Keychain), so the "encrypted" claim
is also true of the bytes on the Mac's disk without FileVault. Optional and
independent of B; one key for DE and CE, so CE locking stays B's.

## Decisions

1. **Is FBE worth it?** The gain measured in #2761 is package construction
   (0.44-0.98 s). CE data is still prepared at user 0's unlock, which is on
   the path to app admission, so admission gains less; slice 4 measures it.
   The other gains are fidelity: CTS `EncryptionTest` and the policy test,
   `DevicePolicyManager.getStorageEncryptionStatus` = ACTIVE_PER_USER, and
   Direct Boot behaving as on every device since Android 10. Decided: yes.
2. **What protects CE.** LockSettings sets no credential; its type follows
   the Mac's (`lock_settings`, #1887). Either CE is protected like a
   credential-less user (vold's empty secret, unlocked at user start as
   `unlockUserKeyIfUnsecured` does), or by a Keychain item available while
   the Mac user's session is unlocked, which matches "the device is the
   Mac's lock". Recommended: the Keychain, so CE locks when the Mac user
   logs out and needs no Android credential.
   Decided: the Keychain.
3. **Real encryption at rest (D) or FileVault only.** Decided: FileVault
   only for now; D stays optional.
4. **Existing data directories.** An unencrypted non-empty directory cannot
   get a policy (`ENOTEMPTY`), and `encryption=Require` then fails the boot
   as it does on Linux. Existing data directories and userdata templates
   must be recreated: a breaking migration, done on a branch with the
   templates regenerated. Decided: recreate, on a branch.

## Recommended path and slices

B, with D left to decision 3. Each slice is independently correct and
under the ~900-line review budget except the first, split in two:

1. **fscrypt ABI** (`aim-linux-abi`, a keyring/policy owner in guest-init):
   (a) keyring ioctls, set/get policy, nonce, flags and statx, inheritance
   and `EXDEV`; (b) locked semantics: `ENOKEY` and no-key names. Tests
   ported from xfstests' encryption group (generic/395-399, 419, 429, 580,
   581, 593) and fscrypt.rst's error table.
2. **vold**: `fbeEnable`, `initUser0`, `createUserStorageKeys`,
   `setCeStorageProtection`, `unlockCeStorage`, `lockCeStorage`,
   `prepareUserStorage` and key destruction as `FsCrypt.cpp`, keys stored
   where the original stores them (`/data/misc/vold/user_keys`,
   `/data/unencrypted`), wrapped by the software KeyMint (ADR 0012).
3. **guest-init and the image**: `fileencryption=aes-256-xts:aes-256-cts:v2`
   in `fstab.aim`, `mount_all` reporting FILE_ENCRYPTED
   (`ro.crypto.state=encrypted`, `ro.crypto.type=file`), `installkey`, and
   `mkdir encryption=`/`key=` as `FscryptSetDirectoryPolicy`; the data
   migration of decision 4.
4. **App host**: CE protection per decision 2 in `lock_settings` and the
   user's unlock; the FBE branches of the ported `fixAppsDataOnBoot` and
   `UserDataPreparer` then run as on a device. Measure 3 fresh and 3 warm
   boots against main (boot-measurement discipline) and run CTS
   `EncryptionTest` and `FileBasedEncryptionPolicyTest`.

## Risks

- The layer keeps paths, not inodes. A working directory or an open
  directory inside a locked directory is known by its no-key names; once
  the key is added (or removed, for one opened with it) a lookup relative
  to it fails with ENOENT, where Linux keeps the inode. Removing a key
  locks its inodes at once: open files keep working, as busy files do on
  Linux, but the key is reported absent rather than `FILES_BUSY`.

- Host-side writers bypass the syscall layer: the app host and aim-system
  write `/data/system*` and prepare app data directly or through installd.
  They must create files through the ABI's inheritance (or the owner's
  API), else files appear unencrypted in an encrypted tree; slice 1 adds a
  check that reports such inodes.
- The hot path: inheritance costs an xattr write per inode created under
  `/data`; lookups pay only while a key is absent. Slice 1 measures
  `startup_bench` and a boot before and after.
- A partial FBE (properties before slices 1-3) is worse than none: the
  properties flip only in slice 3, after the ABI and vold are in.
