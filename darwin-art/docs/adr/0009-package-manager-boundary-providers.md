# ADR 0009: AOSP PackageManagerService with Darwin boundary providers

Status: accepted for the current Android 16 compatibility slice (#30)

## Context

Package state used to come from the profile's install ledger: a Rust manifest
schema and Java projections (`InstalledPackageInfos`, `InstalledPackageParser`,
`PackageManagerEndpoint`) answered PackageManager queries. They could not
compute what PackageManagerService computes: permissions, signatures,
privileged system apps, features, component resolution or install sessions.

The system server now runs AOSP `SystemServer.startBootstrapServices` for the
package stack, in AOSP order, with the original owners: `SystemServiceManager`,
`LocalServices`, `PlatformCompat`, `Installer`, `AccessCheckingService`,
`AppOpsService`, `DomainVerificationService`, `PackageManagerService.main`,
`DexUseManagerLocal`, `UserManagerService`, `SensorPrivacyService`,
`SystemConfigService`, `AppHibernationService`, `ArtManagerLocal`,
`StatsCompanion`, the boot phases, `PackageManagerService.systemReady` and the
system user's start and unlock, after `RuntimeInit.commonInit` as the zygote
runs it.
Those owners expect native daemons, kernel features and a device image that do
not exist on macOS. This ADR records where the Darwin boundary lies.

## Decision

Everything PackageManagerService owns stays AOSP: Settings (`packages.xml` in
the system server's `/data/system`), ComponentResolver, AppsFilter, shared
libraries, domain verification, UserManagerService and PermissionManagerService.
Darwin code exists only at these boundaries:

- **installd** is a Binder in the system server (`DarwinInstalld`) whose app-data
  operations the profile daemon performs (`OP_INSTALLD`), authenticated as the
  system server's child. `rmPackageDir` moves the directory to
  `<mount>/system/package-trash` rather than deleting it. `createUserData`
  prepares `/data/misc/user/<id>` as `ensure_config_user_dirs` does. Operations
  with no host meaning (quota, fs-verity, SELinux relabelling) have fixed
  answers; everything else throws `ServiceSpecificException(EOPNOTSUPP)`.
  `destroyUserData` stays unsupported (#42).
- **artd** is a Binder (`DarwinArtd`) implementing ART Service's `IArtd` for
  in-process callers: profile and artifact files (paths as `path_utils.cc`
  builds them), their visibility and sizes, and the dexopt status this
  process's ART reports. This runtime's ART executes on the host with its own
  boot image, so dex2oat output for the image's boot image would be rejected
  (as dexpreopt output is); `dalvik.vm.disable-art-service-dexopt` is set and
  dexopt, profile merging and cleanup fail with EOPNOTSUPP.
- **apexd** is a Binder (`DarwinApexService`) that reports the image's
  `apex-info-list.xml`. There are no staged sessions, so `markBootCompleted`
  has nothing to mark and updates are unsupported.
- **vold** preparation of per-user storage is `UserStorage` behind the storage
  endpoint. It creates vold's directories and modes, and CE storage state is
  tracked per user. The host has no per-user keys. The system server's `/data`
  root directories come from init.rc's `post-fs-data` list
  (`prepare_system_private_data`).
- **init's property service** is the profile daemon (`OP_PROPERTY_SET`), with
  init's validation, write-once `ro.*`, no control messages and persistent
  `persist.*`. Requests travel over the profile protocol, not
  `/dev/socket/property_service`, because the socket facade has no guest
  `AF_UNIX` namespace. Cross-process propagation is #40.
- **derive_classpath** runs at image assembly. `system/etc/classpath` exports
  `SYSTEMSERVERCLASSPATH` and `STANDALONE_SYSTEMSERVER_JARS` from the image's
  classpath fragments. The system process prefetches the standalone class
  loaders as the zygote does.
- **idmap2** runs at image assembly. The product partition's static overlays
  that target the framework are part of the system root, and the pinned AOSP
  `idmap2 create-multiple` (a Darwin host build) writes their idmaps into
  `/system/etc/resource-cache` with the policies OverlayConfig requests. The
  `OverlayConfig.createIdmap` native returns a shipped idmap only when its header
  matches the request. An overlay idmap2 leaves out stays out, as on a device.
- **libartservice** (the ART module's JNI for `service-art.jar`) is compiled
  from the pinned ART sources into this runtime's Darwin ART. The loader
  resolves `libartservice.so` for ART-module callers to the runtime image.
  `EnsureNoProcessInDir` uses libproc and kqueue.
- **Filesystem.** ART opens and stats guest device paths through the process's
  Android filesystem namespace (FdFile, `OS::FileExists`/`DirectoryExists`), as
  libziparchive and androidfw do. The private data mount stores Linux `user.*`
  extended attributes, which `UserDataPreparer` needs for its serial checks.
- **System partition.** PMS scans the pinned image's `/system/app`,
  `/system/priv-app` and APK-in-APEX packages (including Health Connect, whose
  APK defines permissions AppOps maps), plus the permission and sysconfig XML
  and the system and system_ext aconfig flag protos that decide featureFlag
  manifest elements (`/system_ext` links to `/system/system_ext` as on the
  device). Dexpreopt output is left out because it was compiled against the
  image's own boot image.
- **Install and the host shell.** `darwin-art install` stages APKs in the
  system server's `/data/local/tmp`, as adb does, and runs `cmd package
  install`, a PackageInstaller session. The profile daemon relays `cmd`,
  `dumpsys`, and the launcher's `launcher-info` and `archive-info`
  (PackageManager's launch activity, code paths, uid, label and icon) to the
  system server. App uids for process identity come from `packages.list`, as
  run-as and the zygote read it.

The runtime's own activity, task, storage and usage endpoints publish the
local interfaces AOSP owners look up (`ActivityManagerInternal`,
`ActivityTaskManagerInternal`, `UsageStatsManagerInternal`,
`StorageManagerInternal`); `IStorageManager.getVolumes` reports the internal
private volume at `/data`. Each answers from its
own state; a method the runtime does not implement throws
`UnsupportedOperationException` naming it, never a fabricated default.

## Consequences

AOSP owners take destructive clean-up actions on state they do not recognize
(unknown `/data/app` directories, users without serial numbers). Profiles are
therefore migrated before PMS first boots on them: the daemon moves ledger
installs into the `/data/app` layout and writes their `PackageSetting`s with the
ledger's app ids into `packages.xml`, so uids and app data carry over; a
profile that was copied or renamed resolves its records in its own store. The
ledger is not read again once PMS owns the settings; providers that delete
host data move it aside instead, and PMS experiments run in a disposable
profile. APEX and system app updates are out of scope, because the image is
immutable. The path back to on-device behaviour is to replace a provider with
the AOSP daemon (installd, apexd, vold) behind the same Binder interface; no
PackageManagerService code depends on the Darwin implementations.
