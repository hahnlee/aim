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
| `UserManagerService` | `user` (IUserManager, published by SystemServer's `LifeCycle`); `UserManagerInternal` | takes PMS's lock as its `packagesLock`, holds the concrete PMS and calls it 12 times (`createNewUser`, `onNewUserCreated`, `cleanUpUser`, `addCrossProfileIntentFilter`, `snapshotComputer`, `hasSystemFeature`, `isDeviceUpgrading`) |
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
(`config.disable_otadexopt`). Outside SystemServer the
concrete class is used only for its constants.

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

**What apps use** ("Measured" below): 55 of
`IPackageManager`'s 224 methods were called in a first and a repeat
boot with idle time, cold starts, installs and an uninstall, and ten of
them made 92 % of the calls: package and component queries, intent
queries, enabled state (read and set), install sources, features.
`package_native` saw three methods, from `media.metrics`; the installer
ten session methods, from Play Store. Apps keep `getPackageInfo`,
`getApplicationInfo` and `getPackagesForUid` answers in their own caches
(`PackageManager.sPackageInfoCache`, `sApplicationInfoCache`,
`ApplicationPackageManager.sGetPackagesForUidCache`), dropped when the
`package_info_cache` nonce changes; everything else, intent queries
included, is a binder call.

### Measured

A census (2026-10-02, main e54a8234, window mode): a binder trace of a template first boot of a new data
directory with 2 idle minutes, then of its repeat boot with 2 idle
minutes, Settings started cold twice, Calculator and Chrome installed
with `pm install` and started cold twice each, and a probe APK installed
and uninstalled. Calls received, by interface:

| Interface | First boot and idle | Repeat boot and the session |
| --- | --- | --- |
| `IPackageManager` | 23,175 (apps 23,116, the service host 37, natives 22) | 8,132 |
| `IPermissionManager` | 2,793 | 1,831 |
| `IPackageInstallerSession` | 213 (Play Store) | 0 |
| `IPackageInstaller` | 20 | 9 |
| `IPackageManagerNative` | 8 (`media.metrics`) | 8 |
| `IInstalld` (from system_server) | 1,628 | 920 |
| `IArtd` (from system_server) | 136 | 31 |
| `ILauncherApps`, `IShortcutService`, `ICrossProfileApps`, `IDomainVerificationManager` | 389, 14, 27, 13 | 315, 7, 4, 13 |

`IPackageManager` on the first boot, by method: `getPackageInfo` 8,170
(Google Play services and Play Store, despite the apps' cache),
`queryIntentActivities` 3,327, `queryIntentServices` 2,415,
`getInstallSourceInfo` 1,681, `getApplicationInfo` 1,502,
`setComponentEnabledSetting` 1,187, `getComponentEnabledSetting` 1,057,
`getApplicationEnabledSetting` 844, `hasSystemFeature` 692,
`resolveIntent` 388; 44 more methods share the last 8 %. Most of it is
the idle minutes after the first boot (Play Store updating itself,
Google Play services setting itself up). The changes apps make are
frequent, not rare: `setComponentEnabledSetting` is the sixth most
called method. Per phase of the repeat session, all callers counted:
Settings' two cold starts 124 and 89 calls, Calculator's 130 and 38,
Chrome's 1,197 (Play Store reacting to the install just before) and 72;
each install 207-262, the uninstall 1,133 (Digital Wellbeing resolving
its intents again).
system_server's own calls do not appear: they are in-process (#700).

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
native-services edit can turn into `nop`s, and SystemServer uses its result at
nine more call sites. A facade in its place needs a new kind of
SystemServer exception, the one #668 asks for `power`: a symbolic
redirect of a call site to a static method of `aim-services.jar`. For
M4 that is `PackageManagerService.main(...)` to the facade's `main`
(same arguments; it returns `null` into `mPackageManagerService`) and
each of the nine uses (`invoke-virtual`, or the argument of
`initializeArtManagerLocal`) to an `invoke-static` of the facade taking
the same registers (the receiver arrives as the `null`),
checked symbolically as the `nop` edit is: a changed SystemServer fails
the build. A dex rewrite with new method ids (decision D1).

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
ships `packages.xml`, `packages.list`, user 0's restrictions, the parser
cache, the decompressed stubs and the permission module's files, as the
original's build-time first boot wrote them. The native owner reads
them in Rust (`crates/aim-services`, after the ABX reader of
`crates/aim-build/src/abx.rs`), tested on the files of a template first
boot with an installed app, a disabled package and a disabled
component.

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
  cache is the oracle: the parcel of each of the 286 system packages must
  equal the original's byte for byte, and the CTS test APKs' must equal
  what the original makes of them (D4).
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
  serve its nine uses. Options: (a) a symbolic redirect of named call
  sites to static methods of `aim-services.jar`, a dex rewrite of
  services.jar with new method ids, verified as the `nop` edit is; (b)
  no edit, and PackageManager stays original until ActivityManager and
  WindowManager are native and the in-process callers are gone (M4 after
  M5 and M6, one much larger switch). Recommended: (a), decided once for
  `power` and M4 alike; needed only for C.
- **D2. The user service at the switch.** UserManagerService is built by
  PMS's injector, shares its lock and calls the concrete PMS 12 times.
  Options: (a) keep it original, constructed by the facade, with those
  12 calls redirected to the facade (D1's edit, inside
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
