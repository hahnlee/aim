# ADR 0016: File-based encryption on a Mac data volume

Status: accepted 2026-11-10 (#2761). The owner chose the recommended path:
option B, CE protected by a Mac Keychain item, FileVault for confidentiality
at rest (D not now), and existing data directories recreated by a breaking
migration on a branch. Slices 1 to 4 are implemented (#2845, #2846, #2847,
#2848, #2849): the guest is a device with file-based encryption (FBE),
`ro.crypto.state=encrypted` and `ro.crypto.type=file`, vold manages the
keys as `FsCrypt.cpp` does and the app host protects and unlocks CE with
the keychain's secret.

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

Before slice 3 every owner said "no FBE": the fstab (`image/vendor/etc/fstab.aim`),
guest-init's `mount_all` (`ro.crypto.state=unencrypted`), `installkey` and
`mkdir encryption=` (no-ops), the replaced vold (directories without keys;
`unlockCeStorage` only recorded the user), and the syscall layer, which had
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
under the ~900-line review budget except the first, split in two. Slice 3
turns FBE on and depends on slice 4's CE protection: it lands after it.

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
   Implemented (#2847). As in the original, a key stored without a secret
   is sealed by a keystore2 key bound to its `secdiscardable`, one stored
   with a secret by AES-GCM under the secret's hash, and a new user's CE
   key is stored only when `setCeStorageProtection` gives its secret.
   Until something calls it, user 0's CE key is never stored, so each FBE
   boot would generate a different one (see Risks). The
   device has no adoptable storage, so no volume keys, and no inline
   encryption hardware, so hardware-wrapped keys are refused.
3. **guest-init and the image**: `fileencryption=aes-256-xts:aes-256-cts:v2`
   in `fstab.aim`, `mount_all` reporting FILE_ENCRYPTED
   (`ro.crypto.state=encrypted`, `ro.crypto.type=file`), `installkey`, and
   `mkdir encryption=`/`key=` as `FscryptSetDirectoryPolicy`; the data
   migration of decision 4. Implemented (#2848). guest-init's `mount_all`
   reads the named fstab's `/data` entry and, for `fileencryption=`, does
   what `queue_fs_event` does for FILE_ENCRYPTED; `installkey` makes
   `/data/unencrypted` and runs `vdc cryptfs enablefilecrypto`, and
   `init_user0` its `vdc`, each failure rebooting into recovery as
   `ExecVdcRebootOnFailure` does; `mkdir`'s options are parsed as
   `ParseMkdir` does and the policy of `/data/unencrypted/ref` (or
   `per_boot_ref`) with the options of `mode` is set through the fscrypt
   owner (`Owner::set_policy`, as root with CAP_FOWNER, EnsurePolicy's
   checks), `Attempt` and `DeleteIfNecessary` as the original treats
   them. What guest-init itself creates under `/data` (`mkdir`, `write`,
   `symlink`, the persistent properties) inherits the directory's policy.
   A reboot into recovery (a failed policy) has no recovery to go to, so
   that boot ends. vold reads `IsFbeEnabled` at each call, as the
   original does: it starts at `early-fs`, before `mount_all` sets it.
   The device's `on fs` no longer makes `/data/misc/keystore` (#1253): it
   can only be made with the system DE key, after `installkey`, as on a
   device. The app host image lets `rm` run (`--only`): boot_completed's
   `rm -rf /data/per_boot` removes the previous boot's directory, whose
   per-boot key is gone, before it is made again.
   The migration (decision 4): `mount_all` refuses a `/data` with entries
   that no earlier `mount_all` marked encrypted (`DataMount::predates_fbe`;
   the mark, `Owner::encrypt`, comes before the first directory is made, so
   a boot that ends between a directory and its policy is not taken for one
   from before), so a data directory from before is recreated, never
   mixed. A policy set again on a directory that has it records its root,
   which a boot that ended after storing it did not. A data
   template's files cannot be in `/data`, whose directories get their
   policies from the new directory's own keys; the template keeps them
   beside it (`template/`), and guest-init moves them in after
   `init_user0`, each taking its new directory's policy before it moves,
   as if their owners had written them, so a boot that stops part way
   leaves the rest staged for the next and none in `/data` without one;
   copies (`aim_storage::copy`) never carry a
   context, which names a key of its own volume.
4. **App host**: CE protection per decision 2 in `lock_settings` and the
   user's unlock. Implemented (#2849), in the original's order for an
   unsecured user: StorageManagerService's `restoreCeUnlockedUsers` asks
   vold for the users it unlocked (user 0 on the first boot, its new key
   installed); LockSettingsService's one-time migration
   (`MIGRATED_SP_FULL`) makes user 0's secret if it has no protector
   (`initializeSyntheticPassword`: a random 32-byte secret, a random
   protector id as `sp-handle` in `locksettings.db`) and calls
   `setCeStorageProtection` with it before the user starts; at the
   unlock, `unlockUserKeyIfUnsecured` reads it back for `unlockCeStorage`
   unless vold had unlocked the user. The secret is a generic password of
   the Mac user's keychain (`aim-host-lock`): service `dev.aim.ce-secret`,
   account the user and its protector, so each data directory has its
   own. `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` belongs to the
   data-protection keychain, which an ad-hoc signed process cannot use;
   its equivalent is the file-based login keychain, unlocked by the Mac
   user's login and locked at logout, never synchronized, read without UI
   so a locked keychain fails the read (`errAuthorizationInternal`, tested
   on a throwaway keychain) instead of prompting. The item's access lets
   any of the user's applications decrypt it and carries no partition
   list: an item made with the default access is bound to its maker's
   ad-hoc cdhash, and a second, separately built ad-hoc binary reading it
   blocked on a prompt, where with this access it read the item without
   one (checked with two throwaway binaries and a throwaway keychain). A
   rebuilt app host is therefore never prompted, and setting a partition
   list, which needs the keychain's password, is not needed. It loses
   nothing, as every process of the user can read the plaintext data
   image. As in the original, a failed unlock (the keychain locked, the
   item missing) is logged and the user stays locked (Direct Boot): the
   app host's boot stops after `LOCKED_BOOT_COMPLETED` instead of failing.
   Every user is unsecured to LockSettings here: the Mac's session is the
   credential. A throwaway data directory (a test's, the build's: any but
   aimctl's default, or one booted with `guest-init --ce-keychain`) keeps
   its secrets in a keychain file of its own beside it
   (`aim_storage::data::keychain_of`, `<data>.keychain`, out of the
   user's search list, taken out again at each open, the list's read and
   write serialized among aim's processes by a lock file of the user's),
   which `data::remove` removes with it, so nothing of it reaches the
   user's login keychain (#2882).
   A data directory has one CE protector. The derived image runs the
   original LockSettingsService, which protects user 0's CE key with its
   synthetic password (a blob under `system_de/0/spblob`, sealed by
   Keystore), and the app host image the keychain's secret; neither can
   open the other's, and both record the migration done in
   `locksettings.db`, so a data directory booted by one and then the other
   would leave user 0 in Direct Boot. guest-init's `mount_all` records,
   with the fscrypt owner's state (`fscrypt/ce-protector`), the protector
   of the image that first mounts a data directory (the app host's in an
   image with `/system/etc/aim/app-host-services`), and refuses the other
   image's with both named. Unwrapping the original's protector in the app
   host (as `SyntheticPasswordManager` does for an unsecured user) would
   let the original's data directories move to the app host, but not back;
   it is not done. With slice 3 the FBE
   branches of the ported `fixAppsDataOnBoot` and `UserDataPreparer` run
   as on a device. Remaining: CTS `EncryptionTest` and
   `FileBasedEncryptionPolicyTest`.

## Risks

- The layer keeps paths, not inodes. A working directory or an open
  directory inside a locked directory is known by its no-key names; once
  the key is added (or removed, for one opened with it) a lookup relative
  to it fails with ENOENT, where Linux keeps the inode. Removing a key
  locks its inodes at once: open files keep working, as busy files do on
  Linux, but the key is reported absent rather than `FILES_BUSY`.

- Host-side writers bypass the syscall layer: the app host and aim-system
  write `/data/system*` and prepare app data directly or through installd.
  They give what they make the policy Linux would through the fscrypt
  owner's `aim_storage::fscrypt::adopt` (#2901): `guest_inode::record`,
  which every such writer calls on the inodes it makes, adopts, and the few
  writers that record no guest owner (the instant app store, the native
  writer's lease, compressed and moved package stages) call it directly;
  a host copy takes its directory's policy (`inherit`), not its source's.
  guest-init's builtins give a policy when they create, so their chown and
  chmod records (`guest_inode::change`) do not adopt. An inode written past
  both, or by an older build, has none, a state Linux never allows:
  `fscrypt::policy_less` lists such inodes below an encrypted directory
  (diagnostics and tests), and `mount_all` gives each its directory's
  policy (`fscrypt::repair`, `inherit` as template placement does) once
  per data directory and writer fix: a filesystem a build encrypts, or one
  it repaired, is marked whole at that build's `WHOLE_VERSION`
  (`fscrypt/whole`), a number bumped with each fix of a host-side writer
  that left inodes without a policy, and is walked again only by a build
  whose version is greater (or when the mark is missing or unreadable).
  A walk that fails ends the boot naming the inode and its errno, and
  leaves the mark as it was. An older build that boots a marked data
  directory writes past the mark: what it leaves without a policy is
  repaired by the next current build only if that build's `WHOLE_VERSION`
  is greater than the one stored, that is only if a writer fix landed
  between the build that stored the mark and it; otherwise it stays
  until `policy_less` reports it. Only a data directory first encrypted
  by a build before #2906 pays the walk unasked (#2906):
  0.29-0.59 s for the 4,486 inodes of a fresh app host data directory just
  attached (its metadata not yet cached), too much for every boot. A
  writer that bypasses both after that is a bug `policy_less` finds, not
  a state the mount repairs: it found the package parser's cache
  (`/data/system/package_cache`, 494 inodes) and the move claims'
  directory, which now adopt. The fresh app host boot of
  `aim-services/tests/device_lock.rs` checks that a boot leaves none.
  The app host's writers of `/data/system_ce/<user>` wait for the user's unlock, as their originals do (usage stats,
  shortcuts, widgets, launch params, accounts' CE database, hibernation),
  so vold prepares a fresh directory's CE storage at the first try.
- The hot path: inheritance costs an xattr write per inode created under
  `/data`, and a host-side adoption an xattr read and an `lstat` per
  directory it climbs, up to the first with a policy or the data volume's
  root; lookups pay only while a key is absent (until user 0's unlock
  and the previous boot's `/data/per_boot` is gone), an xattr read per
  directory of a writable area they walk, up to the first encrypted one
  whose key is present: everything below it has its policy. Slice 1 measures
  `startup_bench` and a boot before and after.
- A partial FBE (properties before slices 1-3) is worse than none: the
  properties flip only in slice 3, after the ABI and vold are in.
- CE data depends on the keychain item. vold stores a new user's CE key
  only when LockSettings calls `setCeStorageProtection` (slice 4 does), and
  opens it only with the item's secret: a data directory moved to another
  Mac user or Mac, or whose item was deleted, keeps its CE data locked
  (the unlock fails), as a device whose synthetic password is lost does.
  Deleting the user's own data directory by hand leaves its item in the
  keychain; a throwaway one's keychain file goes with it.
- `secdiscard` cannot securely erase on a `/data` backed by APFS: its F2FS
  and block-discard ioctls do not exist there, so it falls back to
  overwriting, which a copy-on-write filesystem and SSD remapping do not
  guarantee reaches the old blocks. Destroyed keys' wrapped material may
  remain on the Mac's disk; FileVault bounds the exposure to someone with
  the Mac user's credentials.
