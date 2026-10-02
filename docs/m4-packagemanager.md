# M4: PackageManager, native

Status: design (2026-10-02). Nothing is replaced yet. The design of ADR
0013's PackageManager step: what the native service serves, who it
serves inside system_server while ActivityManager, WindowManager and the
permission front ends stay original (until M5 and M6), who owns which
file at each step, and the slices M4 lands in, each with its CTS bar.
Sources are cited at `android-16.0.0_r1`, the image's tag
(`frameworks/base/services`, the same tree system-services.md cites);
transaction counts are the `TRANSACTION_*` constants of the stubs in the
image's own jars, which is what the `aidl-gen` node checks generated
codes against (system-services.md, "How a service is replaced").

## What PackageManager is on this image

`PackageManagerService.main` (`services/core/.../pm`, 164,000 lines of
Java with the user and shortcut services; the parser is in
`framework.jar`, `com.android.internal.pm.parsing`) is not a
`SystemService`: SystemServer calls it directly in
`startBootstrapServices`, after `AccessCheckingService` and
`DomainVerificationService` and before `UserManagerService.LifeCycle`,
`setSystemProcess` and OverlayManagerService. Its injector builds more
than the package service:

| Built by PMS's injector | Publishes | Notes |
| --- | --- | --- |
| `PackageManagerService` | `package` (IPackageManager), `package_native` (IPackageManagerNative); `PackageManagerInternal`, `PackageManagerLocal` | the state (`Settings`), the scan, installs, intent resolution, package visibility (`AppsFilterImpl`), shared libraries, key sets, preferred activities, suspension, instant apps |
| `PackageInstallerService` | IPackageInstaller through `getPackageInstaller()`; one IPackageInstallerSession per session | holds the concrete `PackageManagerService` |
| `UserManagerService` | `user` (IUserManager, published by SystemServer's `LifeCycle`); `UserManagerInternal` | takes PMS's lock as its `packagesLock`, holds the concrete PMS and calls it 15 times (`createNewUser`, `onNewUserCreated`, `cleanUpUser`, `addCrossProfileIntentFilter`, `snapshotComputer`, `hasSystemFeature`, `isDeviceUpgrading`) |
| `PermissionManagerService` (`PermissionManagerService.create`), `LegacyPermissionManagerService` | `permissionmgr`, `permission_checker`, `legacy_permission`; their internal interfaces | the front ends of `AccessCheckingService`'s state ([permissions.md](permissions.md)) |
| `ArtManagerService`, `DexManager`, `PackageDexOptimizer`, `ApexManager`, `ComponentResolver`, `AppsFilterImpl`, `Settings`, three `PackageParser2`s | | `Settings` writes the files below; `ApexManager` talks to apexd |

Started by SystemServer around it: `DomainVerificationService`
(`domain_verification`, its state persisted inside `packages.xml` through
PMS's `Settings`), `DexUseManagerLocal`, ART Service
(`DexOptHelper.initializeArtManagerLocal(context, pm)`, with the
concrete PMS, at `ArtManagerLocal` in `startOtherServices`), and the
services that are clients of `PackageManagerInternal` only:
OverlayManagerService (`overlay`), ShortcutService, LauncherAppsService,
CrossProfileAppsService, DataLoaderManagerService and
BackgroundInstallControlService. SystemServer itself holds the concrete
PMS (`mPackageManagerService`): nine call sites of `isFirstBoot`,
`isDeviceUpgrading`, `systemReady`, `waitForAppDataPrepared`,
`updatePackagesIfNeeded`, `updateMetricsIfNeeded`,
`performFstrimIfNeeded` and `initializeArtManagerLocal`;
`OtaDexoptService.main`, a tenth, is off on this device
(`config.disable_otadexopt`), and `HsumBootUserInitializer.createInstance`,
an eleventh, reads it only in the headless system user mode. Outside
SystemServer the concrete class is used only for its constants.

## 1. The binder surface

| Interface | Methods | Served by | In M4 |
| --- | --- | --- | --- |
| `android.content.pm.IPackageManager` (`package`) | 224: about 140 queries, 84 changes (install, delete, enable state, preferred activities, suspension, stopped state, dexopt and dex use, storage, verification, mime groups) | PMS | native |
| `android.content.pm.IPackageManagerNative` (`package_native`) | 14: `getNamesForUids`, `getPackageUid`, `getInstallerForPackage`, `getVersionCodeForPackage`, `isAudioPlaybackCaptureAllowed`, `getLocationFlags`, `getTargetSdkVersionForPackage`, `getModuleMetadataPackageName`, `hasSha256SigningCertificate`, `isPackageDebuggable`, `hasSystemFeature`, three staged-APEX calls | PMS, for native daemons (audioserver, cameraserver, media, keystore) | native |
| `IPackageInstaller` | 26 (sessions, uninstall, install of an existing package, archive and unarchive, install constraints) | PackageInstallerService | native |
| `IPackageInstallerSession` | 34 (write, read, commit, abandon, checksums, child sessions, pre-approval) | one object per session | native |
| callbacks: `IPackageInstallerCallback` 5, `IPackageInstallObserver2` 2, `IPackageDeleteObserver2` 2, `IPackageMoveObserver` 2, `IPackageDataObserver` 1, `IPackageStatsObserver` 1, `IOnChecksumsReadyListener` 1, `IDexModuleRegisterCallback` 1, `IRemoteCallback` (package monitor) | 16 | called by PMS | the native service calls them |
| `IDomainVerificationManager` (`domain_verification`) | 9 | DomainVerificationService | native (its state is PMS's file) |
| `android.content.pm.dex.IArtManager` (`getArtManager()`) | 2 | `ArtManagerService` over ART Service | native front, ART Service stays |
| `IUserManager` (`user`) | 105 | UserManagerService | decision D2 |
| `IPermissionManager`, `IPermissionChecker`, `ILegacyPermissionManager` | 33, 3, 9 | the permission front ends | original until the access state moves ([permissions.md](permissions.md), #616) |
| `IOverlayManager` 16, `ILauncherApps` 40, `IShortcutService` 24, `ICrossProfileApps` 10, `IDataLoaderManager` 3, `IBackgroundInstallControlService` 3 | | their own services, clients of `PackageManagerInternal` | original |

What PMS calls below it, all native daemons: installd (`IInstalld`, 52),
artd through ART Service, apexd (`IApexService`, 23), idmap2d through
OverlayManagerService (`IIdmap2`, 10), incremental (`IIncrementalService`,
25), vold.

**What apps use** ("Measured surface" below): 55 of
`IPackageManager`'s 224 methods were called in a first and a repeat
boot with idle time, cold starts, installs and an uninstall, and twenty
of them made 98 % of the calls: package and component queries, intent
queries, enabled state (read and set), install sources, features.
`package_native` saw three methods, from `media.metrics`; the installer
ten session methods, from Play Store. Apps keep `getPackageInfo`,
`getApplicationInfo` and `getPackagesForUid` answers in their own caches
(`PackageManager.sPackageInfoCache`, `sApplicationInfoCache`,
`ApplicationPackageManager.sGetPackagesForUidCache`), dropped when the
`package_info_cache` nonce changes; everything else, intent queries
included, is a binder call.

## Measured surface

**Method** (2026-10-02, main e54a8234, M2 Pro, load
6-7). Window mode, the default image, a fresh disposable data directory
started from the userdata template (so PackageManager's first boot runs
as a repeat boot would, docs/first-boot.md). Both boots ran
`cargo aim boot --windows -- --binder-trace FILE` (docs/system-services.md,
"Inventory"). The first boot was traced to `boot_completed` (6.8 s from
the binder host's start) and then 2 idle minutes. The repeat boot was
traced to `boot_completed` (6.5 s), 2 idle minutes, and then the
following, each followed by 6 s:

- two cold starts of Settings (`am start -W -S`; 236 and 185 ms);
- `pm install -r -g` of Calculator and of Chrome
  (`_build/installed-apps`);
- two cold starts of Chrome (1,059 and 270 ms) and of Calculator (285
  and 271 ms);
- install and uninstall of the probe `orientprobe2.apk`.

A PING to `package` from the shell marked each phase. Callers come from
`ps -A` snapshots: an app is a zygote child or a uid ≥ 10000, the
service host is the native services' process, and native is any other
process. Method names and codes come from the image's AIDL stubs
(`TRANSACTION_*` in framework.jar, via `tools/binder-trace-report.py`).
These are the same stubs that `aim-service-aidl`'s generated codes are
checked against (all 224 `IPackageManager` methods in
`crates/aim-services/sources.lock` agree). The tracked game set is not
included because no document names it (#704).

**What the trace cannot see.** system_server's own calls into
PackageManager are Java calls (`PackageManagerInternal`, `Computer`
snapshots, `PackageManagerLocal`), not binder transactions. The trace
shows 0 for system_server in every row. Measuring those calls is #700.
`pm install` and `pm uninstall` are one `cmd package` shell command
each, which runs the install inside system_server. Their rows show
only the reactions of other processes. Play's own installs go through
`IPackageInstaller`, as the first boot shows (#703). A native
PackageManager also serves `cmd package`.

**Totals.** The table counts transactions to the six interfaces M4
serves. `IPackageManager` has 224 codes at this pin; 55 were called.
`IPackageInstaller` has 26 (6 called), `IPackageInstallerSession` 34
(10), `IPermissionManager` 33 (10), `IPermissionChecker` 3 (2) and
`IPackageManagerNative` 14 (3).

| Phase | Length | All binder | package | permissionmgr | permission_checker | installer + sessions | package_native |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| First boot, to boot_completed | 6.8 s | 10,133 | 874 | 19 | 19 | 0 | 2 |
| First boot, 2 idle minutes | 124 s | 117,042 | 22,301 | 2,774 | 191 | 233 | 6 |
| Repeat boot, to boot_completed | 6.5 s | 13,710 | 1,095 | 25 | 29 | 1 | 5 |
| Repeat boot, 2 idle minutes | 126 s | 18,036 | 3,539 | 1,149 | 27 | 5 | 3 |
| Settings, 1st / 2nd start | 8 s each | 2,281 / 1,464 | 124 / 89 | 102 / 222 | 0 | 0 | 0 |
| Chrome, 1st / 2nd start | 8 s each | 2,938 / 1,365 | 1,197 / 72 | 230 / 2 | 1 / 1 | 0 | 0 |
| Calculator, 1st / 2nd start | 8 s each | 1,289 / 973 | 130 / 38 | 39 / 2 | 0 | 0 | 0 |
| Install Calculator / Chrome / probe | 7-10 s each | 998 / 1,055 / 840 | 262 / 246 / 207 | 3 / 36 / 15 | 0 | 1 / 1 / 1 | 0 |
| Uninstall probe | 7 s | 2,111 | 1,133 | 6 | 0 | 0 | 0 |

The started app's own calls, within its phase:

| Start | Its transactions | To package | package driver time | Other PM/permission calls |
| --- | ---: | ---: | ---: | --- |
| Settings, 1st / 2nd | 417 / 417 | 83 / 83 | 4.2 / 3.9 ms | `IPermissionManager.checkPermission` 1, `IPermissionController.countPermissionApps` 1, `IShortcutService.getShortcuts` 1 |
| Chrome, 1st / 2nd | 465 / 475 | 51 / 41 | 3.0 / 1.9 ms | `IPermissionManager.checkPermission` 1 (1st) |
| Calculator, 1st / 2nd | 157 / 147 | 10 / 10 | 0.3 / 0.3 ms | none |

The calls to `package` are cheap at the driver: p50 46 µs and p99 656 µs
over 23,030 calls in the first boot, and p50 40 µs and p99 746 µs over
8,040 in the repeat boot. They were 2.0 s and 0.67 s of driver time.

**Per method.** The table lists `IPackageManager` methods with at least
10 calls, then every called method of the other five interfaces. The
columns are calls in the first boot (to `boot_completed`, idle), in the
repeat boot (to `boot_completed`, idle), in the six app starts and in
the four install and uninstall phases. In the callers columns, `g.` is
`com.google.android.` and `a.` is `com.android.`.

| interface.method (code) | first:boot | first:idle | repeat:boot | repeat:idle | starts | install | app / native / host | main callers |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| IPackageManager.getPackageInfo (3) | 269 | 7901 | 280 | 1123 | 95 | 463 | 10129 / 2 / 0 | g.gms.persistent 3912, g.gms 2720, a.vending 1727 |
| IPackageManager.queryIntentActivities (30) | 4 | 3323 | 21 | 574 | 642 | 668 | 5232 / 0 / 0 | a.vending 2921, g.apps.wellbeing 1454, a.settings 274 |
| IPackageManager.queryIntentServices (34) | 103 | 2312 | 55 | 202 | 30 | 3 | 2705 / 0 / 0 | g.gms.persistent 1287, g.gms 1065, g.googlequicksearchbox:search 101 |
| IPackageManager.getApplicationInfo (9) | 105 | 1397 | 150 | 437 | 83 | 301 | 2467 / 6 / 0 | g.apps.wellbeing 696, g.gms.persistent 441, g.gms 422 |
| IPackageManager.getInstallSourceInfo (55) |  | 1681 | 3 | 1 | 286 | 31 | 2002 / 0 / 0 | a.vending 1940, a.vending:background 53, g.permissioncontroller 6 |
| IPackageManager.getComponentEnabledSetting (89) | 24 | 1033 | 123 | 266 | 4 | 8 | 1458 / 0 / 0 | g.gms 1223, a.vending 72, g.as 33 |
| IPackageManager.setComponentEnabledSetting (87) | 62 | 1125 | 29 | 76 | 4 | 2 | 1298 / 0 / 0 | g.gms 832, a.vending 76, g.gms.persistent 66 |
| IPackageManager.hasSystemFeature (105) | 123 | 569 | 167 | 204 | 85 | 2 | 1144 / 6 / 0 | g.gms 118, a.settings 98, a.vending 90 |
| IPackageManager.getApplicationEnabledSetting (91) |  | 844 | 3 | 5 | 248 | 29 | 1129 / 0 / 0 | a.vending 1063, a.vending:background 53, g.partnersetup 9 |
| IPackageManager.resolveIntent (27) | 9 | 379 | 12 | 291 | 5 | 244 | 940 / 0 / 0 | g.apps.wellbeing 730, g.gms.ui 94, a.settings 67 |
| IPackageManager.getPackagesForUid (20) | 56 | 300 | 68 | 72 | 17 | 10 | 506 / 2 / 15 | g.gms 130, g.gms.persistent 117, g.permissioncontroller 52 |
| IPackageManager.resolveContentProvider (41) | 4 | 232 | 7 | 45 | 13 | 8 | 309 / 0 / 0 | g.settings.intelligence 83, g.documentsui 79, g.gms 37 |
| IPackageManager.queryIntentReceivers (32) | 13 | 189 | 25 | 43 | 21 | 5 | 296 / 0 / 0 | a.phone 75, g.gms 44, a.settings 40 |
| IPackageManager.getNameForUid (21) | 16 | 175 | 16 | 39 | 3 | 1 | 250 / 0 / 0 | g.gms.persistent 116, g.gms 113, g.gms.ui 5 |
| IPackageManager.notifyDexLoad (111) | 36 | 97 | 48 | 34 | 9 | 1 | 221 / 4 / 0 | g.gms 19, g.gms.unstable 11, g.inputmethod.latin 9 |
| IPackageManager.notifyPackagesReplacedReceived (187) |  | 224 |  |  |  |  | 224 / 0 / 0 | g.bluetooth 4, android.process.acore 4, g.adservices.api 4 |
| IPackageManager.queryIntentContentProviders (35) | 2 | 34 | 3 | 8 | 35 | 43 | 125 / 0 / 0 | a.settings 80, g.documentsui 17, g.bluetooth 14 |
| IPackageManager.getActivityInfo (11) |  | 74 |  | 32 | 10 |  | 116 / 0 / 0 | g.gms.ui 106, org.chromium.chrome 10 |
| IPackageManager.getServiceInfo (14) | 5 | 46 | 11 | 12 | 8 |  | 82 / 0 / 0 | g.inputmethod.latin 10, g.googlequicksearchbox:search 10, a.vending 8 |
| IPackageManager.resolveService (33) | 2 | 40 | 6 | 14 | 2 |  | 64 / 0 / 0 | a.settings 30, g.gms 18, g.apps.safetyhub 8 |
| IPackageManager.getInstalledPackages (36) | 1 | 35 | 4 | 3 | 6 | 14 | 63 / 0 / 0 | g.gms 23, a.vending 17, g.permissioncontroller 9 |
| IPackageManager.getInstallerPackageName (54) |  | 41 | 2 | 12 | 3 |  | 58 / 0 / 0 | g.gms.unstable 18, g.gms 11, a.vending 7 |
| IPackageManager.getProviderInfo (15) | 6 | 23 | 11 | 10 |  | 1 | 50 / 1 / 0 | g.settings.intelligence 3, g.gms 3, g.bluetooth 2 |
| IPackageManager.getSystemAvailableFeatures (104) | 1 | 32 | 2 | 4 | 2 |  | 41 / 0 / 0 | g.gms 20, a.vending 7, g.googlequicksearchbox:search 2 |
| IPackageManager.queryInstrumentationAsUser (46) | 12 | 13 | 12 |  | 2 |  | 0 / 0 / 39 | service host 39 |
| IPackageManager.checkSignatures (17) |  | 11 | 7 | 1 | 12 |  | 31 / 0 / 0 | a.settings 24, g.gms 4, a.vending 3 |
| IPackageManager.notifyPackageUse (110) | 9 | 5 | 11 | 1 |  |  | 24 / 2 / 0 | a.phone 6, android.process.acore 6, a.networkstack.process 4 |
| IPackageManager.isInstantApp (151) | 2 | 19 | 1 | 2 | 2 |  | 26 / 0 / 0 | g.gms.persistent 9, g.gms 7, g.apps.wellbeing 2 |
| IPackageManager.getRotationResolverPackageName (175) |  | 20 |  | 6 |  |  | 26 / 0 / 0 | a.settings 26 |
| IPackageManager.getPackageUid (5) | 1 | 4 | 5 |  | 8 | 6 | 24 / 0 / 0 | a.settings 16, g.providers.media.module 6, g.bluetooth 2 |
| IPackageManager.getInstalledApplications (39) | 1 | 18 | 1 | 3 |  | 1 | 24 / 0 / 0 | g.gms 8, a.settings 8, g.apps.wellbeing 3 |
| IPackageManager.getSystemSharedLibraryNames (102) | 1 | 19 | 1 | 2 |  |  | 23 / 0 / 0 | g.gms 19, a.vending 4 |
| IPackageManager.getPropertyAsUser (209) |  | 19 |  | 3 |  |  | 22 / 0 / 0 | a.settings 11, g.gms 9, g.as 2 |
| IPackageManager.setApplicationEnabledSetting (90) |  | 11 |  | 3 |  |  | 12 / 2 / 0 | g.partnersetup 9, g.gms 3, uid1027 2 |
| IPackageManager.getInstalledModules (183) |  | 8 | 1 | 2 |  | 1 | 12 / 0 / 0 | a.settings 6, a.vending 4, a.vending:background 2 |
| IPackageManager.getInstantAppResolverSettingsComponent (164) |  | 10 |  | 2 |  |  | 12 / 0 / 0 | a.settings 12 |
| IPackageManager.getSdkSandboxPackageName (146) | 2 |  | 1 |  | 8 |  | 11 / 0 / 0 | org.chromium.chrome 4, dev.aim.home 2, a.settings 2 |
| IPermissionManager.getPermissionFlags (7) |  | 2186 |  | 1001 | 525 | 26 | 3738 / 0 / 0 | g.permissioncontroller 3738 |
| IPermissionManager.getPermissionInfo (3) | 11 | 315 | 10 | 117 | 45 | 13 | 511 / 0 / 0 | g.gms 359, g.gms.persistent 95, g.permissioncontroller 57 |
| IPermissionChecker.checkPermission (1) | 17 | 191 | 27 | 27 | 2 |  | 197 / 67 / 0 | android.process.acore 93, g.bluetooth 88, cameraserver 67 |
| IPermissionManager.checkPermission (30) | 3 | 176 | 9 | 21 | 4 |  | 212 / 0 / 1 | g.gms.persistent 89, g.gms 84, a.vending 14 |
| IPackageInstallerSession.getNames (3) |  | 77 |  |  |  |  | 77 / 0 / 0 | a.vending:background 76, a.vending 1 |
| IPackageInstallerSession.isMultiPackage (20) |  | 74 |  |  |  |  | 74 / 0 / 0 | a.vending:background 74 |
| IPermissionManager.getPermissionGroupInfo (2) | 1 | 30 | 1 |  | 11 | 4 | 47 / 0 / 0 | g.permissioncontroller 47 |
| IPermissionManager.addOnPermissionsChangeListener (10) | 3 | 34 | 3 | 5 | 1 |  | 44 / 0 / 2 | g.permissioncontroller 29, g.gms 11, g.gms.persistent 4 |
| IPackageInstallerSession.openWrite (4) |  | 30 |  |  |  |  | 30 / 0 / 0 | a.vending:background 22, a.vending 8 |
| IPermissionManager.removeOnPermissionsChangeListener (11) |  | 25 |  | 3 |  |  | 28 / 0 / 0 | g.permissioncontroller 28 |
| IPackageInstallerSession.setClientProgress (1) |  | 25 |  |  |  |  | 25 / 0 / 0 | a.vending:background 25 |
| IPermissionManager.updatePermissionFlags (8) |  | 4 |  | 2 |  | 13 | 18 / 0 / 1 | g.permissioncontroller 18, service host 1 |
| IPermissionManager.queryPermissionsByGroup (4) |  |  |  |  | 11 | 4 | 15 / 0 / 0 | g.permissioncontroller 15 |
| IPackageInstaller.getStagedSessions (9) |  | 6 |  | 3 |  |  | 9 / 0 / 0 | a.vending:background 6, a.vending 3 |
| IPackageManagerNative.getNamesForUids (1) | 2 | 2 | 3 | 1 |  |  | 0 / 8 / 0 | media.metrics 8 |
| IPackageInstaller.getMySessions (8) |  | 5 |  | 2 |  |  | 7 / 0 / 0 | a.vending 4, a.vending:background 3 |
| IPackageInstaller.getSessionInfo (6) |  | 3 |  |  |  | 3 | 6 / 0 / 0 | g.gms 5, a.vending:background 1 |
| IPermissionManager.registerAttributionSource (27) |  | 4 | 2 |  |  |  | 6 / 0 / 0 | g.gms 6 |
| IPermissionChecker.checkOp (3) | 2 |  | 2 |  |  |  | 4 / 0 / 0 | android.process.acore 4 |
| IPackageManagerNative.getInstallerForPackage (3) |  | 2 | 1 | 1 |  |  | 0 / 4 / 0 | media.metrics 4 |
| IPackageManagerNative.getVersionCodeForPackage (4) |  | 2 | 1 | 1 |  |  | 0 / 4 / 0 | media.metrics 4 |
| IPackageInstaller.registerCallback (10) |  | 2 | 1 |  |  |  | 3 / 0 / 0 | g.gms 3 |
| IPackageInstaller.createSession (1) |  | 2 |  |  |  |  | 2 / 0 / 0 | a.vending 1, a.vending:background 1 |
| IPackageInstaller.openSession (5) |  | 2 |  |  |  |  | 2 / 0 / 0 | a.vending 1, a.vending:background 1 |
| IPackageInstallerSession.commit (12) |  | 2 |  |  |  |  | 2 / 0 / 0 | a.vending 1, a.vending:background 1 |
| IPermissionManager.getSplitPermissions (20) | 1 |  |  |  |  |  | 1 / 0 / 0 | g.gms 1 |
| IPackageInstallerSession.getDataLoaderParams (17) |  | 1 |  |  |  |  | 1 / 0 / 0 | a.vending:background 1 |
| IPackageInstallerSession.setChecksums (8) |  | 1 |  |  |  |  | 1 / 0 / 0 | a.vending:background 1 |
| IPackageInstallerSession.openWriteAppMetadata (31) |  | 1 |  |  |  |  | 1 / 0 / 0 | a.vending:background 1 |
| IPackageInstallerSession.close (11) |  | 1 |  |  |  |  | 1 / 0 / 0 | a.vending:background 1 |
| IPackageInstallerSession.abandon (14) |  | 1 |  |  |  |  | 1 / 0 / 0 | a.vending:background 1 |

Called once or twice: `IPackageManager` getUnsuspendablePackagesForUser,
getChangedPackages, getReceiverInfo, setApplicationCategoryHint,
queryContentProviders, getSharedLibraries, getAppMetadataFd,
getAppMetadataSource, requestPackageChecksums, verifyPendingInstall,
canonicalToCurrentPackageNames, isPackageStateProtected,
getModuleInfo, verifyIntentFilter, getPermissionControllerPackageName,
registerPackageMonitorCallback (the service host), getPackageInstaller,
getSystemCaptionsServicePackageName.

**What PackageManager sends.** These are system_server's binder calls out,
which a native PackageManager makes in its place. In the first boot /
the repeat boot with its phases:

- `installd`: 1,628 / 920 (`android.os.IInstalld`);
- `artd`: 136 / 31;
- `IPackageInstallerCallback` to registered apps: 22 / 105, mostly
  `onSessionProgressChanged`;
- `IOnPermissionsChangeListener`: 15 / 108;
- PermissionController's `IPermissionController`: 64 / 20, mostly
  `updateUserSensitiveForApp`;
- LauncherApps' `IOnAppsChangedListener`: 86 / 15.

The PackageManager-side services next to it are called by apps:
`launcherapps` 389 / 315, `domain_verification` 13 / 13, `shortcut`
14 / 7 and `crossprofileapps` 27 / 4.

**Findings.**

- **Apps call package; system_server cannot, over binder.** Every
  counted call came from an app process, the native service host or a
  native daemon. The service host made 39 `queryInstrumentationAsUser`
  calls, 15 `getPackagesForUid` and 3 `registerPackageMonitorCallback`.
  `media.metrics` was the only native caller of `package_native`, with
  16 calls of 3 methods. `cameraserver` made 67
  `permission_checker.checkPermission` calls. system_server's own reads
  of package state are the larger and unmeasured part (#700).
- **Google's apps dominate, not the started apps.** Of 31,307 `package`
  calls, Play Services (all its processes), the Play Store and Digital
  Wellbeing made about 25,400 (81 %). The started apps' own starts made 83 (Settings),
  41-51 (Chrome) and 10 (Calculator). Twenty methods make up 98 % of the
  calls. In descending order they are getPackageInfo,
  queryIntentActivities, queryIntentServices, getApplicationInfo,
  getInstallSourceInfo, the component and application enabled
  settings, hasSystemFeature, resolveIntent and getPackagesForUid.
- **Package changes fan out.** Each install or uninstall brings
  200-1,100 `package` calls from other apps. The Play Store re-reads
  install sources and enabled states (Chrome's first start phase holds
  the Play Store's 286 getInstallSourceInfo and 248
  getApplicationEnabledSetting calls after Chrome's install). Digital
  Wellbeing re-resolves every launcher activity (an uninstall brought
  494 queryIntentActivities and 244 resolveIntent calls). The first
  boot's 224 `notifyPackagesReplacedReceived` calls come from every
  process that received `MY_PACKAGE_REPLACED`.
- **PermissionController polls flags.** `getPermissionFlags` is 3,738 of
  4,624 `permissionmgr` calls, all from PermissionController, in bursts
  after boot and at Settings' and Chrome's first starts (docs/permissions.md
  saw the same).
- **The first boot's idle minutes are heavy.** The 2 minutes after
  `boot_completed` carried 117,042 transactions, 22,301 of them to
  `package`. That is 6.5 times the repeat boot's idle and is Google's
  apps' first-run work. The Play Store also created, wrote and
  committed two install sessions and showed a window with no user
  action (#703). That is the only use of `IPackageInstallerSession`
  measured.

## 2. Callers inside system_server

PMS's largest surface is not binder. Inside system_server it is reached
through four in-process interfaces, which every subsystem uses:

| Interface | Size | Users outside `pm/` (non-test files) |
| --- | --- | --- |
| `PackageManagerInternal` | 148 abstract methods, about 100 used outside `pm/` | 111 files in 54 subsystems |
| `Computer` (`PackageManagerInternal.snapshot()`, a `PackageDataSnapshot` that `IntentResolver` casts to `Computer`) | 220 methods | `IntentResolver` and its subclasses: ActivityManager's broadcast receiver resolver, `IntentFirewall`, `UriGrantsManagerService` |
| `PackageManagerLocal` (snapshots of `PackageState`, `AndroidPackage`, `SharedUserApi`) | 7 methods, over `PackageState` (78), `PackageStateInternal` (36), `PackageUserState` (41) | ART Service, `AccessCheckingService` and `PermissionService`, `AppOpsService`, `AudioService`, DevicePolicy, `StorageStatsService` |
| `IPackageManager` and `PackageManager` in system_server's own process | 224 | 55 files through `AppGlobals.getPackageManager()`, about 300 through `Context.getPackageManager()`: today calls on the local stub object |

`PackageManagerInternal` by subsystem (files; the methods each uses most):

| Subsystem | Files | Mostly |
| --- | --- | --- |
| ActivityManager (`am/`) | 9 | `getPackageUid`, `getApplicationInfo`, `getPackage`, `isInstantApp`, `removeIsolatedUid`, `filterAppAccess`; broadcast resolution through `snapshot()` |
| WindowManager and ActivityTaskManager (`wm/`) | 9 | `resolveIntent`, `getPackage`, `getNameForUid`, `getApplicationInfo`, `grantImplicitAccess`, `isSameApp` |
| backup | 9 | `isDataRestoreSafe`, `finishPackageInstall`, package added/changed/removed |
| companion | 7 | `getPackageUid`, `getInstalledApplications`, `getPackage` |
| input method | 6 | `queryIntentServices`, `filterAppAccess`, `grantImplicitAccess`, `getDefaultHomeActivity` |
| DevicePolicy | 5 | `setPackagesSuspendedByAdmin`, `getPackageInfo`, `getActivityInfo` |
| power, display, app ops, permission, notification, overlays, content, firewall, rollback, policy, media, people, accessibility, app functions, security | 2-3 each | `getPackageStateInternal`, `getPackageList`, `getAndroidPackage`, `resolveContentProvider`, `forEachPackage`, `getKnownPackageNames`, `onPackage*` |
| 31 more (uri grants, storage, statusbar, wallpaper, accounts, webkit, voice interaction, usage, appwidget, dreams, clipboard, ...) | 1 each | `getPackageUid`, `isSameApp`, `filterAppAccess`, `grantImplicitAccess`, `canQueryPackage` |

By call sites, the most used are `getPackageUid` (83), `getApplicationInfo`
(53), `getPackage` (39), `isSameApp` (33), `getPackageInfo` (29),
`filterAppAccess` (25), `isInstantApp` and `getNameForUid` (19 each),
`grantImplicitAccess` and `resolveIntent` (14 each). Some sit on the
hottest paths of the originals that stay: a process start
(`getApplicationInfo`, the gids), every broadcast (manifest receivers,
`snapshot()` for registered ones), every activity start (`resolveIntent`,
`grantImplicitAccess`), every provider acquisition (`filterAppAccess`).
How many such calls a boot or a start makes is not measured; the binder
trace cannot see them (#700).

Several return live objects, not data: `getPackage` and `getAndroidPackage`
an `AndroidPackage` (`PackageImpl`), `getPackageStateInternal` a
`PackageStateInternal`, `snapshot()` a `Computer`. Some are callbacks into
system_server that PMS calls while installing: `setExternalSourcesPolicy`
(app ops' policy for `REQUEST_INSTALL_PACKAGES`), `PackageListObserver`,
the permission front ends' `onPackageAdded`/`onPackageInstalled`
(permissions.md, "The boundary with PackageManager"), ART Service's
dexopt, OverlayManagerService's `setEnabledOverlayPackages` the other
way.

### How a native PackageManager serves them

While the callers are original, a native owner needs code of ours in
system_server in PMS's place: **a facade** in `aim-services.jar`
(`java/device-services`, the bridge's jar) implementing
`PackageManagerInternal`, `PackageManagerLocal`, `Computer` and the
`PackageState` interfaces, backed by binder to the native service. Two
ways to make it, both weighed:

1. *A forwarder.* Each method is a binder call to the host. Simple and
   always current, but every in-process query becomes a round trip (the
   host answers in 8-15 us at p50, system-services.md) on the paths above,
   many under ActivityManager's or WindowManager's global lock, and live
   objects would be rebuilt at every call (callers keep and compare
   them).
2. *A versioned replica* (chosen). The facade keeps, per package, the
   state the native owner pushes to it, keyed by the owner's state
   version, and answers state lookups in process: an `AndroidPackage` is
   the original `PackageImpl`, read from the parcel the native owner
   keeps for each package (the parser cache's format, read by
   `PackageImpl`'s own parcel constructor, as `PackageCacher` reads it);
   a `PackageState` is a class of ours over the owner's record, kept
   until its package changes, so a caller sees the same object for the
   same version. Computed answers (intent resolution, `filterAppAccess`,
   `canQueryPackage`, `getApplicationInfo` with its flags) are not
   computed twice: they are asked of the native owner and kept by
   arguments and version, as the native services keep permission checks
   by the nonce today ("Mirrored state"). Changes (`grantImplicitAccess`,
   `setPackageStoppedState`, `setEnabledOverlayPackages`,
   `removeIsolatedUid`, ...) are synchronous calls.

   **The version** is a counter in a page the host owns, mapped read-only
   into system_server (an fd handed over the bridge at attach, like
   `ApplicationSharedMemory` the other way). The owner increments it
   before it replies to the call that made a change, so a read after a
   write in any process sees the write without ordering messages; the
   facade checks it with one memory read per lookup. The owner also bumps
   the `package_info_cache` nonce, through the bridge, so apps' caches
   drop as they do today.

**What it costs.** The facade's Java is about 550 methods (148, 7, 220
and some 155 of the `PackageState` family), most of them one line, and
an AIDL of ours between the facade and the host of about 250 methods,
generated for both sides as `IBridge` is. Every in-process query that
misses the replica's caches costs a host round trip; how many that is
per start is #700's measurement, and the bar is no measurable change in a
cold start's TotalTime. The replica is a second copy of the package state
(the template's `packages.xml` is 0.4 MB of ABX; the parsed packages are
the parser cache's 4.9 MB).

**What it needs from SystemServer.** PMS is a static call in
`startBootstrapServices`, not a `startService(Foo.class)` the
native-services edit can turn into `nop`s, and SystemServer uses its
result at eleven more call sites. A facade in its place uses the redirect
edit (decision D1; docs/system-services.md, "Redirected call sites"):
`PackageManagerService.main(...)` becomes a call of the facade's `main`
(same arguments; it returns `null` into `mPackageManagerService`), and
each use that reads it an `invoke-static` of the facade with the same
registers, the receiver (the `null`) first, checked symbolically as the `nop` edit is:
a changed SystemServer fails the build.

**The call sites**, from the image's `services.jar` dex. They go into
`image/system-server-redirects` with slice C, each to the facade's static
method of the same name (`calls` as counted here), not before. In
`com.android.server.SystemServer`:

| Caller | Call (`com.android.server.pm.`) | Calls | At C |
| --- | --- | --- | --- |
| `startBootstrapServices` | `PackageManagerService.main` (static) | 1 | the facade's `main`: attaches to the native owner and publishes the local interfaces |
| `startBootstrapServices` | `PackageManagerService.isFirstBoot` | 1 | redirected |
| `startBootstrapServices` | `OtaDexoptService.main(Context, PackageManagerService)` (static) | 1 | stays: not reached, `config.disable_otadexopt` |
| `isFirstBootOrUpgrade` | `PackageManagerService.isFirstBoot`, `PackageManagerService.isDeviceUpgrading` | 1 each | redirected |
| `lambda$startOtherServices$6` | `PackageManagerService.waitForAppDataPrepared` | 1 | redirected |
| `startOtherServices` | `DexOptHelper.initializeArtManagerLocal(Context, PackageManagerService)` (static) | 1 | redirected: ART Service over the facade's `PackageManagerLocal` |
| `startOtherServices` | `PackageManagerService.updatePackagesIfNeeded`, `updateMetricsIfNeeded`, `performFstrimIfNeeded`, `systemReady` | 1 each | redirected |
| `startOtherServices` | `com.android.server.HsumBootUserInitializer.createInstance(ActivityManagerService, PackageManagerService, ContentResolver, boolean)` (static) | 1 | stays: it returns `null` without the headless system user mode and never reads the PMS |

UserManagerService stays original (D2), and its calls of the concrete
PMS (`mPm`, which the facade passes as `null` when it constructs it) go
to the facade. In `com.android.server.pm.UserManagerService`:

| Caller | Call (`PackageManagerService.`) | Calls |
| --- | --- | --- |
| `createUserInternalUncheckedNoTracing` | `createNewUser` | 1 |
| `createUserInternalUncheckedNoTracing` | `onNewUserCreated` | 1 |
| `lambda$convertPreCreatedUserIfPossible$6` | `onNewUserCreated` | 1 |
| `doesDeviceHardwareSupportPrivateSpace` | `hasSystemFeature` | 4 |
| `removeUserState` | `cleanUpUser` | 1 |
| `setDefaultCrossProfileIntentFilters` | `snapshotComputer` | 2 |
| `setDefaultCrossProfileIntentFilters` | `addCrossProfileIntentFilter` (`invoke-virtual/range`) | 2 |
| `verifyCallingPackage` | `snapshotComputer` | 1 |
| `UserManagerService$LifeCycle.onBootPhase` | `isDeviceUpgrading` | 1 |

That is 15 calls, nine lines of the list; `snapshotComputer` returns the
facade's `Computer`.

**Risks.**

- *Deadlock.* Callers hold ActivityManager's, WindowManager's and
  PackageManager's own client locks while they call. The native owner
  never calls into system_server synchronously while it serves a call
  from system_server; the effects it needs there (broadcasts, killing an
  app, dexopt, the permission front ends' install callbacks) run after
  its reply or on a call it makes while serving someone else (an
  installer app), never under a lock system_server could hold
  (permissions.md's rule, "The bridge").
- *Boot order.* The facade answers from `startBootstrapServices` on,
  before ActivityManager's `systemReady`. The host's owner loads its
  state at the host's start, before zygote, so it is ready first; the
  facade's attach is synchronous in its `main`.
- *system_server restarts.* The original PMS restarts with system_server
  and reads its files again; a native owner outlives it. On a new
  system_server the facade attaches again, and the owner re-runs what the
  original does once per system_server (its `systemReady`, default
  grants' trigger, app data preparation) and drops its callbacks.
- *Live object semantics.* Callers that keep an `AndroidPackage` across
  calls, or compare by reference, see the same object while its version
  holds; a caller holding an old one after a change sees the old state,
  as with the original's immutable snapshots.
- *Divergence.* No answer is computed in Java and in Rust both, so the
  facade cannot disagree with `package`; what it computes itself (a
  `PackageState`'s getters over the record) is checked by the shadow
  comparison (slice A).

**Rejected.** Making the facade a `PackageManagerService` (its test
constructor, `PackageManagerServiceTestParams`) reaches into every
private field; keeping the original PMS running as a follower of the
native owner's state has no such mode and would be two writers of one
in-memory state.

## 3. State

| File | Format | Written by the original | Read by |
| --- | --- | --- | --- |
| `/data/system/packages.xml` (+ `.reservecopy`) | ABX (`persist.sys.binary_xml` unset, the default), `ResilientAtomicFile` | `Settings.writeLPr`: packages, disabled system packages, shared users, key sets, permissions' legacy state, `<version fingerprint>`, domain verification state, renamed packages, restored state | PMS at boot |
| `/data/system/users/<u>/package-restrictions.xml` (+ `.reservecopy`) | ABX | per user: installed, enabled, stopped, hidden, suspended, distracting, data inodes, preferred and persistent preferred activities, cross-profile filters, default browser and apps | PMS |
| `/data/system/packages.list` | text | `writePackageListLPr`: name, uid, debuggable, data dir, seinfo, gids, profileable, version | native daemons (`libpackagelistparser`: run-as, debuggerd, installd's quota, simpleperf, ...) |
| `/data/system/package_cache/<fingerprint>/` | `PackageImpl` parcels with `PackageParserCacheHelper`'s string pool | `PackageCacher` | PMS's parser |
| `access.abx`, `runtime-permissions.xml`, `roles.xml` | ABX, XML | the permission module (`AccessCheckingService`), PermissionController, roles | their owners ([permissions.md](permissions.md), "Persistence") |
| `/data/app/~~*/<pkg>-*/`, `/data/user*/<u>/<pkg>`, `/data/misc/profiles` | files | installd on PMS's orders | apps, ART |

The first-boot template (#565, #647; [first-boot.md](first-boot.md))
ships `packages.xml`, `packages.list`, user 0's restrictions,
the decompressed stubs and the permission module's files, as the
original's build-time first boot wrote them. The native owner reads
them in Rust (`crates/aim-services/src/package`, on the binary and text
XML of `crates/aim-android-xml`), tested on the files of a template first
boot with an installed app, a disabled package and a disabled
component.

On the C branch, `package::owner::Store` commits enabled settings to
the complete restriction document as typed ABX, with the original's
backup and reserve-copy protocol and guest inode ownership (#798).
An interrupted main write preserves the previous state; a reserve-copy
failure reports that the main file committed. It refuses a document
changed outside the owner. In a disposable-data check (2026-10-02),
native code disabled Settings, the original PMS booted and read
`enabled=2`, then native code restored the default; the original booted
again and launched Settings (`am start -W`: status ok, 85 ms). No native
writer runs beside PMS; this store is not yet wired into the C service.

`package::service::PackageQueries` implements the host Binder receiver for
`package` and `package_native` over the owner's published scan snapshot.
Both endpoints capture the same shared source, each call retaining one
immutable version while the owner publishes later versions. They reuse
the shadow-tested query and resolution code with the driver's calling
uid and the pinned generated AIDL readers. Unsupported state dependencies
and writes return an explicit unsupported-operation exception. The
enabled-write decoder is also shared and retains `DONT_KILL_APP` and
`SYNCHRONOUS` flags for the future mutation path. Receiver unit tests cover
visibility by caller uid, interface-token rejection and publication to
existing endpoints; all 123 package tests pass. Guest-init does not
register these receivers; native scanning, mutation side effects and the
SystemServer facade remain prerequisites tracked in #798 and the M4
issues.

For the native scan's built-in shared library input (#707),
`SystemConfig::read` reads `library`/`apex-library` declarations and public
native library lists. It applies partition and SKU permissions, file
existence, SDK limits and bootclasspath transition conditions, retaining
the dependency names. On a disposable original-PMS boot (2026-10-02,
`sys.boot_completed=1`), all 19 built-in library names and paths matched
`pm list libraries -v` and `dumpsys package libraries`.

`package::libraries::Registry` registers built-in and APK-declared
dynamic, static and SDK libraries by name/version, retaining code paths,
internal package names and manifest declaring names. Dynamic declarations
require a system app, cannot replace an existing library, and an updated
system app may expose only names its original declared. A disposable
original-PMS data check (2026-10-02, boot completed) read `packages.xml`
and parsed APKs without the feed: all 23 original library names
and paths matched, including Trichrome's static version 694313732 and
internal package name. With isolated split dependency/asset loading and
runtime display density supplied, the diagnostic parses all 243 installed
APK packages, including GMS's 11 splits. The updated GMS's dynamic library
is restricted to its original disabled system package's declarations.
These checks cover library names/paths and parser success, not the whole
native scan or installed GMS's complete parcel parity (#760).
`Registry::collect` selects direct dependencies in the original order:
required Java, static, optional Java, native when PlatformCompat enforces
them, then SDK. Versioned dependencies check the provider's verified
signatures, including certificate rotation and the pre-27 multi-signer
rule. Native enforcement comes from PlatformCompat; the pinned image's
SDK policy is compiled directly into `SharedLibrariesImpl`.
For native enforcement, `IBridge.areNativeLibraryDependenciesEnforced`
now calls the original `PlatformCompat.isChangeEnabledInternal` install
API with package name and target SDK, without looking up PMS state.
`native_dependencies_enforced` uses generated Binder transaction codes
and preserves transport errors and owner exceptions. The bridge serves
the system UID only; it currently attaches after bootstrap, so C's facade
still must expose the same owner query before the native boot scan (#702).
A disposable original-PMS boot (2026-10-02, boot completed) tested the
Rust-to-Java call: SDK 30 returned false, SDK 31/36 true. Device-service
linkage verification and 156 package unit tests pass. The image has no
`android.content.pm.Flags.sdkLibIndependence` API or loaded aconfig entry
for that flag. Inspecting its original `services.jar` shows that the SDK
dependency collection call passes `required=true` directly. `Policy::pinned`
uses that compiled policy (independence disabled), and `Policy::from_bridge`
combines it with the owner's native-enforcement answer. An explicitly run
integration test reads the original image through `aim_paths`, checks the
SDK call signature/range registers and constant assignment without an
intervening write or branch, and rejects a changed image implementation.
SDK dependency/certificate CTS remains open (#800); this is provenance
verification, not CTS acceptance.
`scan::Inputs` now loads persisted active and disabled-system APK records
without the original parser cache or feed, using physical scan-location
flags and full signature verification. It retains native SPKI keys and
returns contextual errors without mutating persistence. An explicitly run
original-image integration test covers active/disabled GSF, nonstandard
APK paths, v3 lineage, SPKI retention and missing-code errors. On a
disposable original-PMS boot (2026-10-02, boot completed), all 243 active
APK packages and five disabled system packages parsed and verified;
all active certificates, scheme versions and lineages matched original
persisted settings. Disabled originals do not persist their signatures.
Settings launched successfully (warm, 90 ms). These inputs are not a
reconciled query snapshot. APEX state stays separate for apexd verification,
and new/removed image package reconciliation and publication remain
unimplemented (#702).
`scan::Image` now supplies image inputs for a first boot without settings.
It follows the pinned partition capabilities and scan-directory order,
inherits each active APEX's preinstalled partition and factory/change
metadata from the owner, retains duplicate declarations for reconciliation,
and rejects a missing framework package. Parser features not implemented
and signature failures abort the candidate; malformed system-directory
candidates retain their failure reasons. On a disposable original-PMS
boot (2026-10-02, boot completed), all 243 image APK candidates parsed and
verified. Their manifest names match all 243 system packages from
`pm list packages -s -f --match-libraries`; every non-updated system APK
path matches, while five updates run from data. Five directories without
APKs are retained as rejected. Settings started successfully (warm,
126 ms). Explicit integration tests use original APKs to check discovery
without settings, ordering, duplicate names, stage exclusion and
empty/absent framework failure. The records do not allocate UIDs or
publish a reconciled snapshot (#702).
The native UID-slot owner (`owner::app_ids::AppIds`) restores active
package and shared-user identities without modifying persisted settings;
disabled originals do not register again. The sparse map retains the
pinned AppIdSettingMap's array extent and deletion cursor, including its
runtime restriction on reusing deleted IDs and fresh-restoration holes.
An explicitly run original-runtime test matches allocation, deletion,
ownership replacement and exhaustion. On its disposable original-PMS
boot (boot completed), all 243 active packages and 16 shared UID groups
restored with their saved IDs. Java API linkage and 156 package unit tests
pass. `owner::shared_users::Bootstrap` now seeds the nine pinned platform
shared users and the valid OEM declarations with fixed IDs and
system/privileged flags (#803). Rejected OEM names, ranges and slot
conflicts retain their reasons. SystemConfig reads `oem-defined-uid`
regardless of partition allow bits, applies later-name replacement and
preserves ArrayMap's signed hash order and stable collision ties. Malformed
declarations retain their file, raw attributes and rejection reason. An
explicit original-runtime test compares the same disposable XMLs with
original SystemConfig, covering invalid/missing attributes, replacements,
signed integer limits and Arabic/fullwidth digits. Retained saved platform
groups match the seed IDs; the original prunes unused seeds after scanning.
`Bootstrap::restore` merges decoded settings with the initial groups,
preserving seeded flags and saved signing details. Post-reconciliation
pruning retains active and disabled-only members, removes empty groups and
updates the UID owner's deletion cursor. An explicit disposable
original-PMS boot (2026-10-02, boot completed) restores all 243 active
package IDs; after pruning, all 16 group names, IDs and signing records
match the original saved settings. Unit tests cover flags, memberships,
disabled-only members, snapshot/input immutability and cursor behavior;
all 156 package unit tests pass. Conflicting decoded settings fail with
context; ordered recovery from corrupt raw Settings
records remains pending (#803). `scan::UidScan` now prepares UID
ownership from fully parsed and verified system-directory Code inputs.
Saved names retain their IDs and manifest groups; duplicate image names
reuse the same identity. New packages join an existing manifest group or
create it with Settings' zero initial flags, while new leaving packages
receive independent UIDs. Static-library names use the selected version
suffix. `Bootstrap::get_shared_user` preserves existing groups/flags and
creates a new group only after UID allocation succeeds. Lookup without
creation, exhausted allocation, older snapshots and rejected transitions
remain unchanged. Disabled/original-package metadata adoption and changed
saved groups reject explicitly (#804). All 156 package unit tests, six
explicit original-APK scan tests and three original-image DEX policy tests
pass. The DEX policy test pins group creation and package registration
branches, including the original insufficient-storage error. These are UID
candidates; complete new PackageSettings, signing reconciliation for new
members, failed-scan cleanup, persistence and query publication remain
under #702, #803 and #798. Saved signing reconciliation and UID conversion
persistence are verified below.
`scan::Identity` now selects manifest, internal and real names using the
pinned static-library suffix and authorized original-package system rename
rules (#804). Persisted scan inputs compare the selected internal name
with Settings and reject mismatches. Full signature verification alone
does not authorize a saved package identity/UID. An explicit disposable
original-PMS boot (2026-10-02, boot completed) reads the actual display
density and native-parses/verifies all 243 active APKs plus disabled
originals. Every active selected internal name and restored UID matches
the original saved state; static library internal names differ from their
manifest names as the original records. Original-image scan-input tests
cover a different saved name pointing at a valid signed APK and preserve
input settings on failure. After signature verification, Identity::apply
now follows original PackageImpl.setPackageName: the package and all
seven component kinds get the selected internal owning name; manifest,
class and process names retain their parsed values. An explicit original
runtime fixture fills the ComponentName cache before renaming and compares
all seven kinds, their recomputed ComponentNames and unchanged name fields
with native records. Java linkage verification and all explicit
scan/runtime tests pass. All 156 package unit tests pass, and every active
parsed package carries its selected internal name.
`sign::History` now follows pinned SigningDetails certificate relationship
rules: lineage capabilities, strict/existing ancestors and exact
multi-signer matches. Its normal existing-package gate accepts installed
data capability or reverse rollback capability; an owner-authorized
rollback also accepts a previous ancestor. An explicit original-runtime
matrix compares all 486 combinations of nine histories and six capability
masks, including unknown details, revoked lineage rights, signer ordering
and rollback direction. Every result matches original SigningDetails.
The disposable original-PMS boot (2026-10-02, boot completed) native-parses
and fully verifies all 243 active APKs; their verified histories all pass
the normal existing-package gate against saved certificates. All 148
package unit tests and Java linkage pass. The shared UID join gate
distinguishes new installs, updates and system scans, honors revoked
lineage capabilities and checks every existing member for new installs.
An explicit original-runtime matrix of 2,187 candidate/group/member/join
type combinations matches PackageManagerServiceUtils; every active shared
UID APK on that boot passes the update join gate against its saved group.
`has_common_ancestor` rejects histories that diverge before their shared
signer. An explicit original-runtime comparison matches all 144 pairs of
12 histories, including partial lineages, equal current signers with
different ancestors, unknown details and multiple signers. After APK
integrity verification, `scan::Inputs::load` now applies normal saved
package, known disabled-system and saved shared UID membership/divergence
signature gates. All 243 active packages pass that connected path on the
disposable original-PMS boot. Explicit original-image tests reject a
fully signed GSF APK against unrelated package/disabled-system saved
certificates without changing settings. `SigningDetails::merge_lineage_with`
now joins compatible partial histories with self/other/restricted capability
rules, preserving the unchanged-instance signal. The shared UID owner
merges an authorized candidate, then applies restricted capabilities from
other parsed members only if that first merge changed the group. An
explicit original-runtime matrix with real DER certificates matches 507
two-history merges and 2,197 group/candidate/member merges: signer order,
capabilities, scheme version, key count and changed signal all match. All
16 restored groups retain their original saved signatures after merging
the verified active members. Invalid saved certificates fail without
changing the group. The shared UID owner now preserves per-scan
`signaturesChanged`: normal reconciliation sets false only when unset;
commit initializes unknown signers; a physical-system signature failure
may replace the first unchecked group signer, and later failures must pass
the SYSTEM join rule. /data updates cannot use that exception even if
FLAG_SYSTEM is saved. Inconsistent later system members reject at first
API <=29 and raise a fatal system error above 29. An explicit original-
image test pins inspected ReconcilePackageUtils and Settings commit DEX,
including these branches and null-signer initialization. All three explicit
native scan tests pass; real GSF/platform certificates check initialization,
first/later OTA replacements, /data rejection, snapshots and error
atomicity. Scan records retain physical origin for this policy.
`scan::SigningScan` now connects normal authorization, shared lineage
merging, OTA state and initial signer commit for already-saved APKs in
caller-supplied order. The pinned manifest selector retains a declared
shared group for an existing member even while leaving; it ignores a leaving
declaration after the saved package has become independent. Changed or
removed groups are rejected before OTA signature exceptions because this
saved-identity phase cannot allocate replacement UID ownership (#804).
Candidate settings preserve saved UID/metadata;
package/group signing state changes only after all fallible work succeeds,
and later rejection or fatal mismatch leaves earlier candidate commits
intact. `Inputs::load_verified_code` supplies integrity-verified code for
this owner phase; its records do not grant saved signer/UID authorization.
Six explicit native scan tests pass, including initial/OTA sequences and
/data, origin and UID-change rejection, missing/changed manifest groups,
and retained versus already-left shared UID declarations. The OTA sequence
uses an explicitly synthetic signer candidate with an unchanged manifest.
On a disposable original-PMS boot
all 243 active saved APKs pass SigningScan in supplied persisted-record
order. Package metadata and all 16 saved group signing/UID records survive;
verified serialized keys are supplied for package records. This is not
the complete image/data scan order or query-snapshot publication. Candidate shared UID migration now implements the
pinned single-user conversion: an accepted active member must be leaving,
and at most one disabled version may remain, itself parsed and leaving.
With the image's BEST_EFFORT policy selected explicitly, both versions
become independent under the same app ID, the group is removed, and the
UID-slot owner becomes the package without moving the allocation cursor.
NEW_INSTALL_ONLY remains the default and leaves existing groups intact.
Six explicit scan tests cover unparsed/non-leaving members, absent or
non-leaving disabled code, multiple members, policy selection, unchanged
metadata, snapshots and preserved allocation. An original-image DEX test
pins the three inspected eligibility/conversion methods. `Store::commit_shared_uid_migrations` now persists owner-authorized
single-member conversions together with signing state. It switches active
and disabled records from sharedUserId to the same numeric userId, removes
the group, and rebuilds certificate definitions/references after removal.
Unrelated XML nodes and package metadata survive; partial conversions,
remapped IDs, empty/multiple-member group deletion and cleared retained
signers reject before writing. All 156 package unit tests pass. A real
signed APK fixture is installed and updated by original PMS, native-parsed
and integrity-verified, reconciled and migrated under BEST_EFFORT, then
written while that disposable original PMS is stopped. On reboot original
PMS retains the migrated UID (10213 in the recorded run), all 244 package
UID/signature records and the other 16 groups; Settings launches. Main and
reserve bytes match and owned data/process cleanup completes. The signed fixture now requests READ_CONTACTS and READ_CALENDAR. Original
permissionmgr grants contacts with USER_SET and denies calendar with
USER_SET/USER_FIXED; original AppOps sets RUN_IN_BACKGROUND to ignore.
The test waits for those exact flags/mode in AccessPersistence before
stopping its owner. Native package persistence leaves access.abx bytes and
its decoded state unchanged. Original reboot preserves the complete fixture
app ID permission flags and AppOps modes, and original permissionmgr and
AppOps queries confirm the grant, fixed denial and ignore mode. These
Android 16 states remain attached to the same numeric app ID during the
conversion. Live permission-owner notifications/package feed, publication,
complete native boot and CTS remain under #803, #798 and #702.
`Store::commit_signatures` now persists package/group certificate and
lineage capability state in the retained packages.xml document. It emits
ABX with document-wide certificate definitions/references and the existing
backup/reserve/inode protocol, preserving unrelated attributes and nodes.
Serialized public keys remain in the scan snapshot; packages.xml stores
certificates. Metadata changes, cleared retained identities, duplicate/
unmodelled owners and external document changes reject before commit.
All 156 package unit tests pass. The disposable original-PMS runtime test
wrote all 243 active package and 16 group signing records into a separate
native-owned fixture; native re-read matches complete persisted settings
and main/reserve bytes match. Its live original-PMS data was not written.
Original PackageSignatures.readXml now reads that native file; all 259
signature owners match certificate bytes, scheme version, lineage
capability flags and derived key counts. A separate explicit two-boot
test stops original PMS, mounts its own stopped data volume, writes native
signature persistence, detaches and restarts original PMS. Both boots
complete; all 243 package and 16 shared UID signing/UID records survive,
no signature XML read error is logged and Settings launches (`Status: ok`).
This verifies original-reader and reboot compatibility of signature
persistence. Complete package-state persistence/publication remains
pending (#798). New/removed package
reconciliation, boot wiring and an actual OTA boot remain pending
(#702, #803, #805). Legacy certificate compatibility/recovery,
upgrade keysets, owner-authorized rollback, full UID reconciliation and
snapshot publication remain unimplemented (#804). These normal signature
gates and merge primitives do not activate C.
The pinned Settings
DEX compiles out the SDK/no-ID exception in the source: SDK libraries
require a positive app/shared-user ID (#802). An explicit original-image
test pins the inspected read/registration control flow, and native reader
and restoration tests cover negative/zero/positive/shared ownership.
Verified SPKI keys now become the pinned runtime's Serializable public
keys for native query records (#738). An explicitly run disposable-boot
integration test compares six RSA/EC/DSA serialization streams byte for
byte, reads the native streams through the original ObjectInputStream,
and verifies ArraySet's signed hash ordering and key deduplication. The
complete native GSF SigningInfo parcel matches an actual original-PMS
`getPackageInfo(GET_SIGNING_CERTIFICATES)` reply, including the rotation
lineage. The runtime uses Conscrypt/Bouncy Castle class layouts; its
BigInteger serialized cache fields differ from the host JDK's. Java API
linkage verification passes. These checks complete public-key reply
serialization, not native PMS activation or CTS acceptance.
`Selection::files` assembles the provider's base/split paths followed by
its already-resolved transitive files, preserving first occurrence.
Tests cover ordering, missing/optional dependencies, SDK policy,
certificate validation and duplicate files. `Registry::resolve` computes
an acyclic provider graph into a separate candidate, resolving provider
file paths before consumers, copying nested APK dependency records and
applying static-library installation for direct consumers' installed
users. Built-in library paths do not become APK dependency edges, matching
`addSharedLibraryLPr`. Tests check multihop paths, nested records, user
effects and unchanged inputs on failure. Cyclic provider scan/update
order is explicitly unsupported (#799); this is not an Android install
error. Policy wiring, scan/update integration, the native scan and #707's CTS
acceptance still remain; declaration records are not yet served as full
shared-library query results.

The split asset loader prerequisite in `parse::resources` now compares
matching resource configurations across all supplied APK tables instead
of stopping at the first table. It combines all type-spec change flags,
including nonmatching configurations, and retains the first ordinary APK
on a configuration tie, as pinned `AssetManager2` does. This is tested
with base/language-split tables. Nonisolated cluster parsing validates
package/version and split names, orders splits by name and merges their
application manifests. Split type names are validated as `ApkLite` does;
install-time type requirements remain the install owner's responsibility.
An explicitly run integration test compiles base/feature/config APKs with
the pinned aapt2 and checks the merged components, class loader, split
fields and cache serialization, plus rejection of malformed clusters.
On a disposable original-PMS boot (2026-10-02, boot completed), native
output matches all 288 scan cache entries byte for byte, including the
system GMS/Play Store packages. Isolated loading now builds the pinned
`SplitDependencyLoader` tree, rejects missing targets, config targets that
are not features and cycles, and serializes the tree in the original
sparse-array format. Each feature's assets include its ancestors and their
configuration splits, then its own APK and configuration splits; sibling
feature assets are excluded. Config-only split parsing uses its own APK,
as `SplitAssetDependencyLoader` does. The compiled integration test covers
feature dependencies, config targeting, serialization and malformed trees;
a unit test checks the asset scopes. Installed GMS parses, but its complete
parcel comparison against the original is still pending (#760).

**Who writes what, by slice:**

| Slice | Package files | Permission files | App files |
| --- | --- | --- | --- |
| A, B (shadows) | the original, alone; the native model writes nothing | the original | installd, on the original's orders |
| C (the switch) | the native owner, in the original's formats and paths, with the reserve copy and the fingerprint the original writes; the parser cache it keeps is the original's format in the original's directory | the original module, through its front ends, fed by the facade (`PackageManagerLocal` and the install callbacks), until the access state moves (#616) | installd, on the native owner's orders |
| The template | made by the original's build-time first boot until C; from C by the native owner's, so the template's parity check is the native writer's oracle | | |

**Rules.** One writer per file at any time; never a second, partial
writer. The native owner reads an existing data directory as the
original left it (a user's data needs no migration at the switch) and
leaves it as the original would read it: going back to the original is a
CTS-checked step of slice C (boot the original on the native owner's
files; `dumpsys package` the same up to the values first-boot.md lists).
ABX is written as `BinaryXmlSerializer` writes it (interned strings,
typed attributes); a text `packages.xml` is still read, as the original
reads both.

## 4. Dependencies

- **installd.** The native owner is its client (`IInstalld`, generated
  from the pinned AIDL, as every native service's clients are): app data
  (`createAppData`, `fixupAppData`, `restoreconAppData`), code paths
  (`rmPackageDir`, `moveCompleteApp`), sizes and quotas, `linkFile`,
  profiles. The service host runs with system_server's uid and SELinux
  context, which installd's policy admits.
- **Dexopt.** ART Service stays original in system_server: it reads
  packages through `PackageManagerLocal` (the facade's replica) and runs
  artd and dex2oat. The native owner asks it for an install's dexopt
  through the bridge, after the package is committed and outside any
  system_server call; first-boot and boot dexopt stay as the template
  made them. `notifyDexLoad`, `registerDexModule` and
  `performDexOpt*` are forwarded to `DexUseManagerLocal` and
  `ArtManagerLocal` the same way.
- **Overlays.** OverlayManagerService and idmap2d stay original; OMS
  reads packages and writes each user's enabled overlay paths through
  `PackageManagerInternal` (`setEnabledOverlayPackages`), which the native
  owner keeps per user and returns in `ApplicationInfo.overlayPaths`, as
  the original does.
- **APEX.** The owner reads apexd (`getActivePackages`,
  `getAllPackages`, staged sessions) and scans the APKs inside APEXes, as
  `ApexManager` and the APEX scan partitions do; APEX packages are
  answered for `MATCH_APEX`. Staged installs (APEX and rebootless) come
  last within C.
- **Shared libraries.** `SharedLibrariesImpl`: builtin (SystemConfig),
  static, dynamic and SDK libraries, `uses-library` checks at install,
  dependents and the class loader context ART Service compiles with.
  Trichrome is a static library on this image (Chrome and WebView), so
  every app check covers it.
- **Parsing and signing.** The native owner parses APKs in Rust after
  `ParsingPackageUtils` (manifest binary XML, resources only where the
  parser resolves them, aconfig flags) and verifies them after
  `ApkSignatureVerifier` (v1 JAR, v2, v3 and v3.1 with rotation, v4 for
  incremental), producing the `PackageImpl` parcel. The image's parser
  cache is the oracle: each package's parcel must equal the original's
  byte for byte, and the CTS test APKs' must equal what the original
  makes of them (D4). The parser (`crates/aim-services/src/package/parse`,
  `aim-package-parse`) matches a first boot's cache byte for byte for all
  288 packages (285 system packages and the 3 decompressed stubs under
  `/data/app`). The cache depends on the device's locale and display
  density (#722). Signature verification (`.../package/sign`,
  `aim-package-sign`) makes the `SigningDetails` `packages.xml` records
  (signers, scheme, lineage with capabilities, signing key set) for 242
  of its 244 packages, whether it only collects certificates, as the
  scan does, or verifies the contents, as an install does; the other two
  are Play updates the copy of `/data` lacks. The fetched CTS test APKs
  (v1, v3, v4 with `.idsig`) verify too. The paths no oracle checks yet
  (v3.1, PSS, DSA, several signers) are #761.
- **Package visibility.** `AppsFilterImpl` in Rust: `<queries>`,
  implicit grants (`grantImplicitAccess` from ActivityManager and
  WindowManager, synchronous so an app's next query sees it), force
  queryable, instrumentation and the shared-uid rules; every query
  filtered by the caller as the original filters it.
- **Intent resolution.** `ComponentResolver` and `ResolveIntentHelper`:
  the four component kinds and providers by authority, preferred and
  persistent preferred activities, cross-profile filters, domain
  verification for web links (`domain_verification` moves with it),
  instant apps, protected broadcasts, `SaferIntentUtils`' checks.
- **Permissions (#616).** At the switch the permission front ends and
  the access state stay original. The facade gives them what PMS gave
  them: `PackageManagerLocal` snapshots, the install and removal
  callbacks with the `AndroidPackage`, the gids of a uid. The native
  owner creates nothing of theirs; the facade's `main` creates the front
  ends as PMS's injector did (`PermissionManagerService.create`,
  `LegacyPermissionManagerService.create`, both public). The access
  state then moves after the switch, through the seam permissions.md
  describes, with no second package feed: the facade's replica is the
  feed. This amends #616's order: the access state follows M4's switch
  rather than landing in it.
- **Users.** UserManagerService is built by PMS's injector (D2).
- **The original keeps running until parity.** Slices A and B change no
  answer an app or system_server gets. Slice C replaces the original
  only after its bar; until then `image/native-services` does not list
  `package`.

## 5. Slices

Each slice lands on its own, with its bar, before the next starts.

### A: the read model, in shadow

The native model of the package state and every query of `package` and
`package_native`, run beside the original and compared with it, serving
nothing.

- The model is fed by the original through the bridge: at attach a full
  `PackageManagerLocal` snapshot (states, `PackageImpl` parcels, shared
  users, per-user state, preferred activities, the domain verification
  state), then a delta for each change, triggered by the original's
  `package_info_cache` nonce and its package monitor callback. The
  feed's completeness is itself checked: before each comparison the
  model asks for a fresh snapshot digest.

  As built: `PackageFeed` (`aim-services.jar`) hands the host an
  `IPackageFeed` (`IBridge.getPackageFeed`) and sends records on
  `IPackageFeedHost`, synchronous calls from a thread of its own: each
  package and disabled system package (`PackageState` and every user's
  `PackageUserState` getters, signing, the install source of
  `getInstallSourceInfo`, the domain verification state, the installed
  permission definitions, gids and granted permissions), each parsed
  package as `PackageCacher.toCacheEntryStatic` writes it (sent again only
  when the original holds another `AndroidPackage`), each shared user,
  each user's preferred activities (`getPreferredActivityBackup`, in
  full) and AppsFilter's configuration, a large record in chunks. A batch
  sends what differs from what the host holds and ends with the SHA-256
  of a fresh snapshot's records; the host
  (`crates/aim-services/src/package/feed`) publishes the state
  (`package::model::State`) only when its own records give the same
  digest, else asks for every record again. A batch follows each package
  monitor callback; `Feed::fresh`, before a comparison, reads the nonce
  and, when it moved, asks for one (one-way, with a token the batch
  ends with) and waits for it. Nothing reads what only the internal
  interfaces hold: part of the install source (#714), the persistent
  preferred activities and cross-profile filters (#715), component label
  and icon overrides and the loading state (#716).
- **The comparison.** guest-init's binder driver, given `--binder-shadow
  package,package_native` (a diagnostic CLI option, like
  `--binder-trace`), hands the service host a copy of each transaction
  to those nodes and of its reply. The host answers the same call from
  its model and compares the replies after decoding them (a
  `ParceledListSlice` is followed through its binder; binders and fds are
  compared by identity of what they stand for). A difference is logged
  with both replies. Nothing reaches the caller but the original's
  reply.
- **Bar.** No unexplained difference over a run of the device-side
  read modules below and the app checks; each explained one is an issue
  and fixed before C.
- Lands: the Rust state and query model, the bridge's feed (Java, in
  `aim-services.jar`), the driver option.

### B: the write model, in shadow

Installs, updates, removals and state changes computed by the native
owner without side effects, and compared with the original's outcome.

- On each commit the original makes (an install session, `pm install`,
  a `deletePackage`, `setComponentEnabledSetting`, `setPackagesSuspended`,
  a preferred activity, ...), the bridge hands the native owner the same
  input (the session's staged directory and parameters, or the call's
  arguments) before the original runs it. The owner parses, verifies,
  reconciles (shared users, key sets, libraries, the app id it would
  assign, the install-time decisions) and computes the new state,
  calling no daemon. After the original's commit, the owner compares
  its result with the original's delta, up to the per-device values
  first-boot.md lists (app ids, code path names, times).

  As built (`crates/aim-services/src/package/write`): PackageManager has
  no hook before a commit, so a binder write is answered from the latest
  state the model observed before the driver took the call (each shadow
  copy carries that instant), and its change applied to a replica of the
  package's state in that user. `pm install` and `pm uninstall` run
  inside system_server, so installs, updates and removals are found as
  changes between observed states, each computed from the state before
  it. The bridge's `PackageWrites` tells the model of each install
  session's parameters (`IBridge.watchPackageWrites`,
  `IPackageWritesHost`), sent before the installer commits it, and the
  model joins a session to its install. Once a package has settled for
  2 s, the model's result is compared with a fresh state of the
  original; the shadow log carries the comparison as a `check` line, and
  `tools/binder-shadow-report.py` counts the checks per operation.
  Modelled: setComponentEnabledSetting and setApplicationEnabledSetting
  (not yet a caller changing another package, #754), installs and
  updates (app id, shared user, users' state; not yet signing and
  libraries), removals for every user and for some users. The APK signature
  adapter now verifies the parsed base/split paths directly, supporting
  file inputs and cluster base names other than `base.apk` (#801). It
  rejects null/unreadable paths; an original signed APK test checks both
  nonstandard base/split guest paths and its v3 signing lineage without
  altering the original. Not yet: the
  parser's oracle on installs (#760), restoring a system package,
  suspension, preferred activities.
- The parser's oracle runs here too: every APK the original parses
  (boot, install) is parsed natively and the parcels compared.
- **Bar.** No unexplained difference over the device-side install
  modules and the app installs below.

### C: the switch

The native owner and writer of the package state; the original PMS is
not constructed.

- `package`, `package_native`, the installer and its sessions,
  `domain_verification` and `IArtManager`'s front native, registered by
  guest-init under the original names (`image/native-services`); the
  facade in PMS's place (D1); `user` as D2 decides; the permission front
  ends, ART Service, OverlayManagerService and the `PackageManagerInternal`
  clients original, over the facade.
- The native owner makes the side effects: installd, apexd, incremental,
  ART Service through the bridge, broadcasts (`PACKAGE_ADDED` and the
  rest) and app kills through ActivityManager on the bridge.
- **Bar.** Every module below ends each test as with the original; the
  app checks; going back to the original on the native files; the
  template built by the native owner's first boot passes first-boot.md's
  structure and parity checks against the original's template.
- Developed on a branch (AGENTS.md: breaking migrations happen on a
  branch) and landed as one change once the bar holds; A and B make most
  of its code land before, unchanged.

After C, the facade shrinks as its callers move native (M5, M6), and
the access state moves (#616).

### CTS

The CTS release of system-services.md ("Conformance"); the original's
results per module are the baseline each slice is held to. By kind:

| Slice | Device-side (run today with `am instrument`) | Host-side (Tradefed) |
| --- | --- | --- |
| A | CtsContentTestCases (`android.content.pm.cts`), CtsPackageManagerTestCases, CtsAppEnumerationTestCases, CtsDomainVerificationDeviceStandaloneTestCases (#618), CtsSuspendAppsTestCases, CtsShortcutManagerTestCases, CtsInstantAppTests, CtsHibernationTestCases | CtsPackageManagerParsingHostTestCases, CtsPackageManagerPreferredActivityHostTestCases, CtsPackageSettingHostTestCases |
| B | CtsPackageInstallTestCases, CtsPackageInstallSessionTestCases, CtsAtomicInstallTestCases, CtsPackageUninstallTestCases, CtsPackageInstallAppOpDefaultTestCases, CtsPackageInstallAppOpDeniedTestCases, CtsAdminPackageInstallerTestCases, CtsSecureFrpInstallTestCases, CtsPackageInstallerTapjackingTestCases, the eight CtsPackageInstallerCUJ* modules | CtsPackageManagerHostTestCases, CtsAppSecurityHostTestCases, CtsInstallHostTestCases, CtsUsesLibraryHostTestCases, CtsClassloaderSplitsHostTestCases, CtsDexMetadataHostTestCases, CtsAppMetadataHostTestCases, CtsInstantAppsHostTestCases |
| C | all of the above, CtsSuspendAppsPermissionTestCases, CtsDomainVerificationDeviceMultiUserTestCases, CtsRollbackManagerTestCases, CtsPackageWatchdogTestCases, the permission modules of permissions.md, CtsOsTestCases | CtsApexTestCases, CtsStagedInstallHostTestCases, CtsRollbackManagerHostTestCases, CtsOverlayHostTestCases, CtsShortcutHostTestCases, CtsCompilationTestCases, CtsDomainVerificationHostTestCases, CtsIncrementalInstallHostTestCases, CtsInstalledLoadingProgressHostTests, CtsPackageManagerStatsHostTestCases, CtsPackageManagerIncrementalStatsHostTestCases, CtsPackageManagerMultiUserHostTestCases |

Of these 52 modules 22 are host-side, and no host-side module runs
against this device today (#701): most of PackageManager's install,
signing and parsing coverage is there, so C needs them.

**App checks**, each slice: the integration gate; Settings (app info,
storage, default apps, the app list's permissions); Chrome and WebView
(Trichrome's static library); Calculator; the tracked games; Google Play
services' idle traffic; for B and C, an install, update and uninstall
from Play Store and of a local APK with `pm install` and through
`PackageInstaller` sessions.

## 6. Decisions

Tracked on #702.


- **D1. A redirect edit of SystemServer** (with #668). The facade must
  stand where `PackageManagerService.main` is called, in bootstrap, and
  serve its eleven uses. Options: (a) a symbolic redirect of named call
  sites to static methods of `aim-services.jar`, a dex rewrite of
  services.jar with new method ids, verified as the `nop` edit is; (b)
  no edit, and PackageManager stays original until ActivityManager and
  WindowManager are native and the in-process callers are gone (M4 after
  M5 and M6, one much larger switch). Recommended: (a), decided once for
  `power` and M4 alike; needed only for C. Decided (a) (2026-10-02); the
  edit is built (docs/system-services.md, "Redirected call sites"), and
  its list stays empty until C ("The call sites", section 2).
- **D2. The user service at the switch.** UserManagerService is built by
  PMS's injector, shares its lock and calls the concrete PMS 15 times.
  Options: (a) keep it original, constructed by the facade, with those
  15 calls redirected to the facade (D1's edit, inside
  `UserManagerService`); (b) a native `user` in C too (105 methods,
  `UserManagerInternal`'s 58 methods for 76 files, multi-user CTS).
  Recommended: (a): a smaller C, users unchanged, and `user` replaced
  later on its own bar.
- **D3. Shadows instead of a served read slice.** The proposal's slice
  (a), queries served natively while the original writes, would need
  D1's edit to take `package` from PMS, forward every change to the
  original, and a mirror that is never stale for an app that just
  installed, all thrown away at C; the gain is latency. Shadows A and B
  check the same code against every real call without changing an
  answer. Recommended: shadows (compatibility first, 2026-10-01).
- **D4. The parser.** (a) Rust, after `ParsingPackageUtils` and
  `ApkSignatureVerifier`, checked byte for byte against the parser
  cache; (b) the original Java parser run for the native owner (in
  system_server through the bridge, or in a process of its own),
  compatible by construction but leaving installs dependent on a Java
  process and system_server. Recommended: (a), with the cache as its
  oracle; (b) is the fallback if a class of APKs cannot reach parity.
- **#616** (open): the access state moves after M4's switch, not in it
  (section 4).

## Conformance bar

The CTS modules a native PackageManager must end as the original does
(ADR 0013, decision 4), at `android-cts-16_r1`, and the original's
results on this device. The device-side modules and every file their
configs install or push are pinned in `upstream/cts.lock`.

**Modules.** Device-side modules (an instrumentation, run with
`am instrument`):

| Module | Covers | Tests |
| --- | --- | --- |
| CtsPackageManagerTestCases | `android.content.pm.cts` (37 classes; in 16_r1 they are no longer in CtsContentTestCases, which has none): queries, flags, components, properties, install and uninstall through the shell, checksums, archiving, incremental, launcher apps, signatures, resource hardening | 560 methods, more with parameters |
| CtsAppEnumerationTestCases | package visibility (`<queries>`, force-queryable, shared uids, providers, intents, launcher apps, sync adapters) | 336 |
| CtsPackageInstallTestCases | installer sessions through PackageInstaller's UI, install sources, constraints, app metadata, pre-verified domains | 81 |
| CtsPackageInstallSessionTestCases | session parameters, pre-approval, update ownership | 71 |
| CtsPackageUninstallTestCases | uninstall and archive through PackageInstaller | 16 |
| CtsAtomicInstallTestCases | multi-package sessions | 33 |
| CtsDomainVerificationDeviceStandaloneTestCases | domain verification (`domain_verification`) | 76 |
| CtsSuspendAppsTestCases, CtsSuspendAppsPermissionTestCases | suspended packages | 33, 3 |
| CtsPackageInstallAppOpDefaultTestCases, CtsPackageInstallAppOpDeniedTestCases | `REQUEST_INSTALL_PACKAGES` at install | 4, 4 |
| CtsInstantAppTests | instant app resolution | 22 |
| CtsPackageInstallerCUJ{Installation, InstallationViaIntentForResult, InstallationViaSession, Uninstallation, UpdateOwnerShip, UpdateSelf}TestCases, CtsPackageInstallerTapjackingTestCases | PackageInstaller's user journeys | 55, 27, 20, 14, 28, 16, 2 |
| CtsShortcutManagerTestCases | `shortcut` and `launcherapps` | about 76 (JUnit 3) |

Not pinned: CtsSecureFrpInstallTestCases (its config pushes
`TestAppAv1.apk`, which the release does not contain);
CtsPackageInstallerCUJDeviceAdminTestCases and
CtsAdminPackageInstallerTestCases (a device owner);
CtsPackageInstallerCUJMultiUsersTestCases and
CtsDomainVerificationDeviceMultiUserTestCases (a secondary user);
CtsRollbackManagerTestCases, CtsPackageWatchdogTestCases and
CtsHibernationTestCases (services next to PackageManager, for slice C).
The host-side modules of section 5's CTS table (Tradefed `HostTest`
and `JarHostTest`) need an adb transport and a Tradefed host, which the
runner below does not have (#701).

**Run.** On a first boot of a new data directory in window mode, each
module as its Tradefed config prepares it: the config's commands and
settings in order, its pushed files copied to their `/data/local/tmp`
paths, its APKs installed with `pm install -r -g -t` (and
`--force-queryable` unless the config says false, as Tradefed does), then
`am instrument -w -r` with the config's hidden API and annotation
filters (a module with an `instant_app` parameter without its
`@AppModeInstant` tests, as Tradefed's full-app run), and the packages
uninstalled again. CTS expects en-US and the device follows the Mac's
languages with no override (#709), so PackageInstaller,
PermissionController and the modules' packages run in en-US through
per-app locales (`cmd locale set-app-locales`); system_server's own
dialogs stay in the Mac's language.

**The original** (main e54a8234, window mode, first boots; ops checks
m4cts-1 to m4cts-5):

| Module | Pass / fail / skip | Time | Run |
| --- | --- | --- | --- |
| CtsAtomicInstallTestCases | 33 / 0 / 0 | 19 s | whole |
| CtsAppEnumerationTestCases | 334 / 2 / 0 | 232 s | whole; the two `all_canSeeForceQueryable` failed because that run installed without `--force-queryable` (since fixed) |
| CtsPackageUninstallTestCases | 14 / 2 / 0 | 141 s | whole (#730) |
| CtsSuspendAppsTestCases | 30 / 1 / 2 | 95 s | whole (#709) |
| CtsSuspendAppsPermissionTestCases | 3 / 0 / 0 | 1 s | whole |
| CtsPackageInstallAppOpDefaultTestCases | 2 / 0 / 2 | 1 s | whole |
| CtsPackageInstallAppOpDeniedTestCases | 4 / 0 / 0 | 8 s | whole |
| CtsInstantAppTests | 22 / 0 / 0 | 1 s | whole |
| CtsPackageInstallerCUJInstallationViaSessionTestCases | 20 / 0 / 0 | 169 s | whole |
| CtsPackageInstallTestCases | 62 / 2 / 4 of 81 | 840 s (cut) | first 68; `ExternalSourcesInstantAppsTest#externalSourceDeniedTest` is instant-only and ran because that run lacked the filter (since fixed); `InstallAppMetadataTest#readAppMetadataFileShouldFail` (#717) |
| CtsPackageInstallSessionTestCases | 28 / 2 / 0 of 71 | 600 s (cut) | first 30, before the per-app locales; the two `PreapprovalInstallTest#..._userAgree_statusSuccess` could not find `UPDATE ANYWAY` (#709) |
| CtsDomainVerificationDeviceStandaloneTestCases | 17 / 0 / 0 of 76 | 600 s (cut) | hangs (#618) |
| CtsPackageManagerTestCases | 39 / 0 / 1 of 156 (`PackageManagerTest`) | 350 s (cut) | first shard of the module |

Not run yet: the other five CUJ modules, the tapjacking and shortcut
modules, and the rest of CtsPackageManagerTestCases.

**Failure clusters on the original**, each a gap below the guest or in
the runner, to fix at its owner before the bar is final:

- **The language (#709).** UI tests find PackageInstaller's and
  system_server's buttons and titles by their English text. Per-app
  locales fixed PackageInstaller's (CtsPackageUninstallTestCases went
  from 4 to 14 passing and from 408 to 141 s); system_server's
  `SuspendedAppActivity` stays in the Mac's language
  (`DialogTests#testInterceptorActivity_moreDetails`).
- **Domain verification hangs (#618)** after 17 tests on a first boot,
  in `DomainVerificationFilterGroupTests`.
- **No owner and mode check in `open` (#717).** An app reads the app
  metadata file PackageManager keeps beside an APK, which a device denies.
- **Archive dialogs (#730).** Two `ArchiveTest` cases get result code 1
  from PackageInstaller's archive dialogs.
- **`screenrecord` crashes (#710)** in its overlay thread (no EGL display)
  for every test under `ScreenRecordRule`; the tests go on, but each
  start costs time and fills the crash buffer.
- PackageManager's settings writes log `Failed to enable fs-verity ...
  Inappropriate ioctl for device` (ENOTTY, which Linux returns for a file
  system without verity); no test failed on it so far.

**Time.** On the development Mac (M2 Pro, host load 2-20): tests that
drive PackageInstaller's UI take 8-12 s each (UiAutomator waits and a
`screenrecord` start per test), PackageManagerTest's install-heavy tests
about 9 s, AppEnumeration's queries under 1 s. Projected for one full run
of the pinned modules: CtsPackageManagerTestCases about 85 min,
CtsPackageInstallTestCases 17, CtsPackageInstallSessionTestCases 15, the
six CUJ modules 25, AppEnumeration 4, the rest 10 (with domain
verification fixed): about 2 h 40 min of device time, nine to ten
sessions of at most 20 minutes, each a first boot with its installs.
