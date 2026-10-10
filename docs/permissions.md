# Permissions and app ops

The design of ADR 0013's permissions milestone: moving PermissionManager,
AppOps and later PermissionController off the original SystemServer.
Nothing is replaced yet. What the native services already do with
permissions and app ops (mirrors, the permission nonce) is in
[system-services.md](system-services.md), "Mirrored state" and "The
system_server bridge". Sources are cited at `android-16.0.0_r1`, the
image's tag.

## Who owns what

On Android 16 permissions and app-op modes are one state with one owner
(`PermissionManager.USE_ACCESS_CHECKING_SERVICE` is `SdkLevel.isAtLeastV()`),
behind two binder front ends:

| Component | Owns | Serves | Created by |
| --- | --- | --- | --- |
| `AccessCheckingService` (`services/permission`, Kotlin, in services.jar) | the access state: permission definitions, trees and groups; per user and app id, every permission's flags (granted is a flag), per virtual device too; per app id and per package, app-op modes; the default grant fingerprint; package versions for upgrades | `LocalServices`: `PermissionManagerServiceInterface`, `AppOpsCheckingServiceInterface`; `PermissionManagerLocal` | SystemServer, `startService(AccessCheckingService.class)` in `startBootstrapServices`, before PackageManager |
| `PermissionManagerService` | callbacks (`IOnPermissionsChangeListener`), the check delegate (`AccessCheckDelegate`: shell identity, overrides), registered attribution sources, one-time permissions, `PermissionControllerManager` | `permissionmgr` (IPermissionManager, 33 methods), `permission_checker` (IPermissionChecker, 3); `LocalServices`: `PermissionManagerServiceInternal` | PackageManagerService's injector (`PermissionManagerService.create`) |
| `AppOpsService` | per attribution: noted, started and paused ops (`AttributedOp`); the uid states it decides foreground modes by (`AppOpsUidStateTracker`); restrictions (user and global); recent accesses (`appops_accesses.xml`); history (`/data/system/appops/history`, discrete ops); async notes waiting for their app; the check delegates | `appops` (IAppOpsService, 60 methods); `LocalServices`: `AppOpsManagerInternal`; `AppOpsManagerLocal` | ActivityManagerService's constructor (`Injector.getAppOpsService`), published by `ActivityManagerService.start` |
| `PermissionPolicyService` | keeping permission-backed app ops in step with grants (`setUidModeFromPermissionPolicy`, `setModeFromPermissionPolicy`), the permission dialog's intercepts | nothing public | SystemServer |
| PermissionController (an app in the `com.android.permission` APEX, uid 10198 here) | the grant dialog, the permission screens Settings opens, default grants, auto-revoke, backup | `IPermissionController`, bound by system_server through ActivityManager | PackageManager requires exactly one privileged handler of `MANAGE_PERMISSIONS` (`getRequiredPermissionControllerLPr`) |

Roles run in system_server (`system_server_role_controller_enabled` is
on in this image), not in PermissionController.

**Who calls them.**

- Apps: `Context.checkPermission` goes to `activity`
  (`IActivityManager.checkPermissionForDevice`), which asks
  `PermissionManagerServiceInternal`; apps cache the answer by the
  `package_info_cache` nonce. `AppOpsManager.checkOp` reads the raw mode
  (`checkOperationRawForDevice`), cached in the app by its own nonce
  (`appop_mode_caching_enabled` is on; both location ops are never
  cached). Notes, starts, finishes, `checkPackage` and watchers always
  call `appops`. PermissionController reads flags and definitions from
  `permissionmgr`.
- Other processes of the system: `permission_checker` (Bluetooth,
  cameraserver, the phone process).
- system_server itself, without binder: ActivityManager and its helpers
  hold the concrete `AppOpsService` and call it directly (25 call sites in
  `am/`: every uid's process state and capabilities
  (`noteUidProcessStateAndCapability` -> `updateUidProcState`),
  foreground services' `startOperation`/`finishOperation`, broadcast and
  provider `checkPackage`, `resetAllModes`/`uidRemoved`, shell
  delegation). `AppOpsManagerInternal` is used by PackageManager,
  PermissionPolicyService, `AppOpsPolicy` (the check delegate SystemServer
  installs for location, voice and camera attribution), SensorPrivacy,
  DevicePolicy, AppWidget and ActivityTaskSupervisor.
  `PermissionManagerServiceInternal` is used from 18 files outside its
  package: PackageManager's, ActivityManager's, PermissionPolicyService,
  AudioService, voice interaction.
  Everything else reaches both through `AppOpsManager` and
  `PermissionManager` in its own process, where the binder is the local
  object: a direct call.

## The boundary with PackageManager

PackageManager stays original until M4, and the access state is fed by
it synchronously:

- `AccessCheckingService.initialize` reads PackageManager's package states
  (`PackageManagerLocal.allPackageStates`), known packages and SystemConfig
  (config permissions, privileged and signature allowlists, split
  permissions), and keeps them as its external state.
- An install, update or removal calls into it before PackageManager
  commits: `onPackageAdded`, `onPackageInstalled` (with the parsed
  `AndroidPackage`: declared and requested permissions, protection levels,
  signing details, shared uid, flags), `onPackageRemoved`,
  `onPackageUninstalled`, `onStorageVolumeMounted`, `onUserCreated`,
  `onUserRemoved`, from `InstallPackageHelper` and `RemovePackageHelper`
  with PackageManager's locks held. Install-time and signature
  permissions are decided there, from the requesting and the defining
  package's certificates.
- A process start reads permission state: ActivityManager's
  `ProcessList` asks `getPackageGids`, which PackageManager answers from
  `getGidsForUid` (`INTERNET` is gid 3003, and so on).
- The default grants: PackageManager grants them in `systemReady`
  (`DefaultPermissionGrantPolicy`, through `LegacyPermissionManagerService`)
  and records the build fingerprint in the state; PermissionPolicyService
  then asks PermissionController for its own
  (`grantOrUpgradeDefaultRuntimePermissions`).

So an access state outside system_server needs, before PackageManager is
native, a feed of the permission-relevant parts of every package state,
answered synchronously on the install path. That feed is PackageManager's
own boundary and is designed with M4.

## Traffic

Measured on a binder trace of a settled boot followed by Settings started
cold twice and Chrome once (2026-09-30, the second boot of a data
directory, `tools/binder-trace-report.py`'s method names; driver latency
of each synchronous call). Whole trace, about 90 s:

| Service | Calls | Most of them |
| --- | --- | --- |
| `permissionmgr` | 1,516 | `getPermissionFlags` 1,279 from PermissionController (p50 19 us), `getPermissionInfo` 165 |
| `activity` `checkPermissionForDevice` | 568 | apps, p50 45 us |
| `appops` | 554 | `checkPackage` 347 (235 from one app, uid 10147), `checkOperationRawForDevice` 65, `setUidMode` 42, `startOperation` 26, `startWatchingModeWithFlags` 22, `noteOperation` 2 |
| `permission_checker` | 81 | Bluetooth 57, apps 13, cameraserver 7 |

A cold start's own calls:

| App | Calls | Driver time |
| --- | --- | --- |
| Settings, first start (216 ms) | `appops` 52: `checkPackage` 23, `checkOperationRawForDevice` 14, `setUidMode` 14; `permissionmgr` 1 | 2.3 ms |
| Settings, second start (210 ms) | `appops` 27 (the 14 `setUidMode`, 13 checks); `permissionmgr` 1 | 0.9 ms |
| Chrome (419 ms) | `activity` `checkPermissionForDevice` 6; no `appops` or `permissionmgr` | 0.3 ms |

In the seconds after each Settings start PermissionController read
permission flags 70 and 210 times (6.0 and 17.6 ms), off the start's path.

What replacing the owners would save: the native services answer in 8-15
us at p50 where the originals take 36-51 us, so about 30 us per call:
1.5 ms of Settings' first start and nothing measurable of Chrome's. The
apps' own caches already keep most checks off binder. What it would add
is not in the trace: system_server's calls into its own permission and
app-op objects are direct calls today and would each become a binder
call. How many a start makes is not measured; the binder trace cannot see
them.

## Staging

### AppOps alone: not separable

IAppOpsService is a clean binder surface, but its owner is not:

- **Its modes are not its own.** They are `AccessCheckingService`'s,
  in the same state, lock and file as permissions (`access.abx`, below).
  A native AppOps owning its modes takes half of that file from its
  writer.
- **ActivityManager holds it as a class.** `mAppOpsService` is a final
  field of type `AppOpsService`, created by ActivityManager's constructor
  and called directly; there is no startService call to leave out.
  `image/native-services` works by turning one `startService` into
  `nop`s; here the class itself would have to become a forwarder, a
  rewrite of a 127-method class inside services.jar, which is not a
  minimal edit.
- **A native front with the Java one still running splits the state.**
  If `appops` were native while ActivityManager kept its own
  `AppOpsService` for its direct calls, started ops (foreground services,
  the camera), recent accesses and history would be kept in two places:
  SystemUI's privacy indicators and PermissionController's dashboard read
  one, ActivityManager writes the other.
- **Every internal call becomes a binder call**, some under
  ActivityManager's locks, and uid state changes are ActivityManager's
  hottest path. A native owner could then never call back into
  system_server synchronously while serving it.

### PermissionManager alone: not separable

`PermissionManagerService` is created by PackageManager's injector, its
state is `AccessCheckingService`'s, PackageManager calls it synchronously
on every install, and ActivityManager asks it for every app's
`checkPermission`. Replacing the `permissionmgr` binder alone would leave
the state, the delegate and the callbacks in system_server.

### The seam that exists: the access state

`AccessCheckingService` is started by SystemServer by class and publishes
its state only through two `LocalServices` interfaces,
`PermissionManagerServiceInterface` and `AppOpsCheckingServiceInterface`,
which the front ends use. That is where a native owner fits without
touching the front ends: the state (and `access.abx`) native, and a class
of ours in its place that implements both interfaces by forwarding to the
native owner. It needs:

- the `startService(AccessCheckingService.class)` call pointed at our
  class: an edit of the same kind as the `nop`s, but a new one;
- the package feed of the previous section, since both interfaces take
  `AndroidPackage` and `PackageState`;
- a cache in the forwarder, kept by the native owner's invalidation, or
  every permission check inside system_server pays a binder call.

It moves state but no traffic: apps still call the Java front ends.

### Order

1. **Now: nothing replaced.** The native services keep reading
   permissions by the nonce and app-op modes by their watchers (done), and
   report what they note as the original does (#615).
2. **With M4 (PackageManager): the access state**, through the seam
   above, sharing the package feed with PackageManager's own replacement.
   Before M4 it would build that feed once for PackageManager's Java and
   again for its native form.
3. **With M4/M5 (ActivityManager): the front ends.** `permissionmgr`,
   `permission_checker` and `appops` native once their internal callers
   are native too: ActivityManager's direct calls, `AppOpsPolicy`,
   `AccessCheckDelegate`, PermissionPolicyService's sync. Then
   ActivityManager's own `checkPermission` answers in process.
4. **PermissionController last** (below).

This moves the permissions milestone from a step of its own into the core
(M4-M5), where system-services.md's classification already puts
`appops`, `permission` and `permissionmgr` (class C). The decision is
#616.

## The bridge

What the access state's native owner needs in each direction, while the
front ends and PackageManager are original:

- **system_server to the host:** the package feed (each package state's
  permission-relevant fields, the disabled system packages, known
  packages, SystemConfig's permission tables, users), synchronously on the
  install path; `readLegacyPermissionStateTEMP`-style migrations at first
  boot; the calls of both interfaces (checks, grants, flags, modes).
- **The host to system_server:** the interfaces' listeners, which the
  front ends turn into their callbacks (`AppOpsModeChangedListener` for
  modes; permission changes for `IOnPermissionsChangeListener` and the
  nonce, `PackageManager.invalidatePackageInfoCache`, which only
  system_server can bump while it owns `ApplicationSharedMemory`).
- **Never** a synchronous call from the host into system_server while it
  serves one of system_server's: the callers hold PackageManager's and
  ActivityManager's locks.

When the front ends move (step 3) the bridge loses the interface calls; the
package feed goes with PackageManager.

## Persistence

A switch back to the original must find its files as it left them, so a
native owner reads and writes the same ones in the same formats:

- **The access state**, ABX (`BinaryXmlSerializer`) with a reserve copy
  (`AtomicFile.writeWithReserveCopy`): `access.abx` in
  `/data/misc/apexdata/com.android.permission` (the system state:
  definitions, trees) and in
  `/data/misc_de/<user>/apexdata/com.android.permission` (per user:
  `<access>` with `package-versions`, `default-permission-grant`, the
  app id permissions and device permissions, `app-id-app-ops`,
  `package-app-ops`). The legacy files (`appops.xml`,
  `runtime-permissions.xml`, `packages.xml`'s permissions) are read only
  to migrate when `access.abx` is missing, so a native owner writes
  `access.abx` for every user it knows, or the original migrates stale
  state after a switch back. `aim_services::package::permissions` reads
  both `access.abx` and `runtime-permissions.xml` as their owners do
  (binary XML in `crates/aim-android-xml`).
- **AppOps' own files**, with the front end (step 3):
  `/data/system/appops_accesses.xml` (recent accesses, text XML),
  `/data/system/appops/history` (the historical registry) and
  `/data/system/appops/discrete` (discrete ops; the sqlite registries'
  flags are off in this image).

For the M4 C single-member shared UID conversion, the app ID stays the
same: the existing per-app-ID grants, flags and UID AppOps modes remain
owned by AccessCheckingService. Native package persistence leaves its
`access.abx` untouched. A disposable original-PMS reboot after a native
conversion (2026-10-02) preserves a READ_CONTACTS grant with USER_SET, a
READ_CALENDAR denial with USER_SET/USER_FIXED and RUN_IN_BACKGROUND ignore;
the original permissionmgr/AppOps queries and the decoded target app ID
state agree. This checks user 0's default-device state. The live package
feed and permission-owner notifications at the native switch remain under
#702; conversion must not report the retained app ID as removed.

## PermissionController

PermissionController is an app, and two originals require it to be one:
PackageManager fails its boot unless exactly one privileged app handles
`MANAGE_PERMISSIONS`, and system_server reaches it by binding its service
through ActivityManager, which binds only services of app processes it
starts. So the app goes when PackageManager and ActivityManager are
native, after the permission state it acts on (step 4); until then it
stays, with #470's interceptor answering `POST_NOTIFICATIONS` from the
Mac (#550).

Its grant dialog is the one part with a Mac owner: which runtime
permissions the Mac's own prompts answer (camera, microphone, location,
contacts, calendars, as `POST_NOTIFICATIONS` does now) is #291. Answering
more of them through the interceptor would widen a temporary exception,
and CtsPermissionUiTestCases drives PermissionController's dialog with
UiAutomator, so every permission the Mac answers fails those tests by
design.

## Conformance

The CTS modules at `android-cts-16_r1` for this milestone, and where
parity is at risk:

| Module | Covers | Risk |
| --- | --- | --- |
| CtsAppOpsTestCases, CtsAppOps2TestCases | modes, watchers, note/start/finish, attribution chains, history, noted-op collection | high: history windows and timing, async notes, `AppOpsLoggingTest`'s callbacks through every service (#615) |
| CtsAttributionSourceTestCases | attribution source registration and chains | medium |
| CtsPermissionTestCases, CtsPermissionPolicyTestCases, CtsPermissionTestCasesSdk28 | checks, definitions, protection levels, the platform's permission policy | high: install-time and signature decisions ride on the package feed |
| CtsPermissionManagerNativeTestCases | the NDK `APermissionManager_checkPermission` | low |
| CtsPermissionMultiDeviceTestCases, CtsPermissionMultiUserTestCases | device-aware permissions, users | medium |
| CtsPermissionUiTestCases | PermissionController's dialogs | unchanged until step 4; fails by design for what the Mac answers |
| CtsRoleTestCases, CtsRoleMultiUserTestCases | roles and their grants | medium: role grants change flags without the runtime listener (system-services.md) |
| CtsPackageInstallAppOpDefaultTestCases, CtsPackageInstallAppOpDeniedTestCases | `REQUEST_INSTALL_PACKAGES` modes at install | medium |
| CtsNoPermissionTestCases, CtsNoPermissionTestCases25 | calls without permissions | low |

Plus the app checks of ADR 0013: Settings' app permission screens, a
runtime grant and revoke, and the privacy indicators.
