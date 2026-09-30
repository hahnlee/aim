# System services

The living record of ADR 0013's migration: which of Android's system
services run as native implementations on the Mac, how they are replaced,
what apps use, and how each replacement conforms. Keep it current with
every step: replace superseded facts instead of appending a log.

## Status

| Service | Implementation | Conformance (CTS 16_r1) | Since |
| --- | --- | --- | --- |
| `clipboard` (IClipboard) | native, `crates/aim-services`, backed by `NSPasteboard` | 36 of 36 tests pass, as the original; app checks pass | M1, 2026-09-29 |
| `vibrator_manager` (IVibratorManagerService), `external_vibrator_service` | native, `crates/aim-services`: the original without a vibrator, as on a Mac | CtsVibratorTestCases: 268 of 301 pass, 33 skip (no vibrator), each test as the original | M2, 2026-09-30 |
| `location` (ILocationManager) | native, `crates/aim-services`: gps from CoreLocation, network, fused and the geocoder bound from Google Play services through the bridge | CtsLocation{Fine,Coarse,None,Gnss,Privileged}TestCases: each test as the original (234 pass, 10 skip, 3 fail and 2 hang in GNSS tests without a fix) | M2, 2026-09-30 |
| every other service | the original, in SystemServer or its daemon | | |

## How a service is replaced

1. **Its AIDL, generated.** `crates/aim-services/sources.lock` pins the
   framework `.aidl` files at the image's tag (sha256 per file). The
   `aidl-gen` node of `cargo aim` fetches them (gitiles, else git) and
   generates `aim-service-aidl` (`target/aim/gen/service-aidl`,
   `crates/aim-build/src/nodes/service_aidl.rs`): each interface's
   descriptor, every method's code (AIDL's rule: the first call
   transaction plus the method's index or explicit id), and for the
   methods the lock selects the parcel (de)serialization of their
   arguments and replies over `aim_binder_host::parcel`. A Java-only
   parcelable is a type parameter the service implements
   (`ReadParcelable`, `WriteParcelable`) from the pinned Java source.
   The node then reads the `TRANSACTION_*` constants of each interface's
   stub in the image's own `framework.jar` (`aim_android_image::dex`) and
   fails unless every method's code matches: codes cannot drift, and
   nothing is numbered by hand or found by reflection.
2. **A host binder process.** guest-init hosts the binder driver; the
   native services are one more process of it (`aim_binder_host::local`),
   with the system uid and system_server's SELinux context, serving their
   nodes on host threads. A call to them crosses no Mach hop after the
   caller's own. A service sees the caller's pid and uid from the driver,
   and reaches the original services it consults (permissions, app ops,
   users) as a client, through servicemanager. What its decisions read on
   every call it mirrors (below, "Mirrored state").
3. **Registered under the original name.** When servicemanager sets
   `servicemanager.ready` (at boot, and again after a restart), guest-init
   registers each native service with `addService`, as
   `SystemService.publishBinderService` does.
4. **The original not started.** `image/native-services` lists each
   native service and the SystemServer class it replaces. SystemServer has
   no switch to leave a service out, so the derived image's `services.jar`
   is the original with that class's `startService(Foo.class)` call (its
   `const-class` and `invoke-virtual`) turned into `nop`s, in place; the
   dex checksums and the jar's CRC are recomputed and nothing else changes
   (`aim_android_image::system_server`, the `system-server` node, ADR 0013
   "Steps"). The `oat` node compiles the jar's `services.{odex,vdex,art}`
   again with the rebuilt dex2oat64 (`speed-profile` as the original, its
   profile converted to the edited jar, against the regenerated boot
   image): the image's name the jar's entries by CRC, and without matching
   ones system_server verifies all of services.jar at run time (ANRs, and
   a cold Settings start after boot timed out). guest-init reads the same
   list from the image.
5. **Reverse bridge.** system_server code that calls a replaced service's
   `LocalServices` interface needs a bridge to the native service. For
   clipboard there is none to bridge: `ClipboardService` publishes no local
   interface, and nothing else in `services/` reaches into it (searched at
   the tag). Both directions go through one bridge ("The system_server
   bridge").
6. **Conformance.** The CTS module(s) for the service's API, before (the
   original) and after (native), plus the app checks; failing tests keep
   the original.

## Mirrored state

A native service decides on every call by state that other services own
(whether a package is the caller's, its permissions, app-op modes, focus,
the input method, whether the device is locked). Asking the owner each
time costs a system_server round trip per input, so the service host
keeps a mirror of each (`crates/aim-services/src/mirror.rs`), fed by the
owner's standard listener, registered with generated AIDL codes:

| Input | Owner's query (on a miss) | Listener that drops it |
| --- | --- | --- |
| a package is the uid's | `IAppOpsService.checkPackage` | `IPackageManager.registerPackageMonitorCallback` (every package change, all users) |
| an app op's mode | `IAppOpsService.checkOperationForDevice` | `startWatchingModeWithFlags(op, WATCH_FOREGROUND_CHANGES)`, one callback per op, and the package callback |
| an installed instrumentation targets the uid | `IPackageManager.getPackagesForUid`, `queryInstrumentationAsUser` | the package callback |
| focus (the focused root task's uid) | `IActivityTaskManager.getFocusedRootTaskInfo` | `registerTaskStackListener` (every task change) |
| the system user's device is locked | `ITrustManager.isDeviceLocked` | `ITrustManager.registerDeviceLockedStateListener` |

`WATCH_FOREGROUND_CHANGES` also reports a uid's change of foreground state
for an op in `MODE_FOREGROUND`, the one input of a mode that is not a
setting, so no uid observer is needed. Noting an app op
(`noteOperation`), which records an access and decides nothing the check
did not, is sent from a background thread.

**Permissions** are kept by their owner's client-cache nonce, not by a
listener. The owner's listener,
`IPermissionManager.addOnPermissionsChangeListener`, is told of runtime
permissions only. Granting or revoking a development or role permission
(`pm grant`, the role holders') notifies nobody
(`onInstallPermissionGranted`, `PermissionManagerServiceImpl` at the
tag), and shell permission delegation and root's permission overrides
(`UiAutomation.adoptShellPermissionIdentity`, `addOverridePermissionState`,
`AccessCheckDelegate`) change answers without notice
(`testReadInBackgroundRequiresPermission` failed with a permission
mirror). Every change bumps the nonce
(`PackageManager.invalidatePackageInfoCache`) that apps' own
`checkPermission` cache follows. With
`pic_separate_permission_notifications` and `pic_uses_shared_memory` on
in this image, that is the `package_info_cache` nonce in
`ApplicationSharedMemory`, which the system_server bridge hands the host
("The system_server bridge"). The host keeps a check's answer as
`PropertyInvalidatedCache.query` does: keyed by permission and uid
(`checkPermission`; the pid decides nothing) or by permission and
package (`IPermissionManager.checkPermission`), stored with the nonce
read before asking, and dropped once the nonce differs. While the nonce
is unset or reserved, or before the bridge is attached, every check is
asked (`crates/aim-services/src/system.rs`).

One input is asked each time. An instrumentation target's app ops: delegation
decides the ops of the uid it delegates to as shell's and tells no mode
watcher either (#467). It delegates only to the target of an active
instrumentation, a uid one of whose packages an installed
instrumentation targets (an SDK sandbox's uid: its client's), so those
uids' modes are asked each time and every other uid's are mirrored. The
clipboard evaluates the original's disjunction with focus first, so only
a read by an app without focus (the input method, a service) asks for a
permission.

**Settings.** What the original reads from the settings provider
(`Settings.Secure`, `DeviceConfig`) is kept as the framework's own
settings clients keep it (`Settings.NameValueCache`), not by an
observer: `IContentService.registerContentObserver` refuses one from a
process ActivityManager does not know ("Failed to find PID",
`checkContentProviderAccess`), which the service host is (#430). The
service host reads the provider as system_server's code does,
`IContentProvider.call` (`GET_secure`, `PUT_secure`, `GET_config`) on the
provider `IActivityManager.getContentProviderExternal` hands out, and
asks it to track each value's generation: the reply carries the
provider's generation array (a `MemoryIntArray` in ashmem, mapped
read-only in the service host) and the value's index. A kept value is
used while its generation is unchanged, one memory read; the provider
bumps it with every change, so a value is never older than the last
change. A `DeviceConfig` namespace is tracked as a whole (`LIST_config`
of its prefix), since the provider bumps only the namespace for any of
its properties; its values are read after its generation. Everything
kept goes when the provider (system_server) dies
(`crates/aim-services/src/settings.rs`).

**The rule.** A value is kept only while its listener is registered: the
listener is registered before the first query, and a query keeps its
answer only if no notification arrived while it ran (a generation count),
so a kept value is never older than the last notification. A
notification drops what it may have changed, and the next decision asks
the owner once. When the owner dies (system_server restarts), the
registration dies with it: the mirror is dropped and decisions are
synchronous queries until the listener is registered again. The owners
send their notifications one-way when they commit a change, most from a
handler thread, so a call that races a change is decided as just before
it until the notification arrives, as for every other client of these
listeners. Other users' lock state is asked each time; virtual devices
are never locked (`TrustManagerService`).

A clipboard write still asks `IUserManager.getProfileIds`, the one call
on its common path (the restrictions are asked only for a user with
profiles). Nothing that tells of a change in a user's profiles reaches
the service host: `IUserManager`'s one listener reports restrictions,
the user broadcasts need a receiver in a process ActivityManager knows
(`registerReceiverWithFeature` returns null for any other caller), and
`UserManager`'s own cache invalidation, which would be the signal, keeps
its nonce in `ApplicationSharedMemory` in this image
(`application_shared_memory_enabled`, `pic_uses_shared_memory`), not in
a system property (#460, #430).

The binder host's nodes accept file descriptors, as libbinder's do: the
task stack listener's snapshots carry a buffer's. They are closed after
the call, and a service that takes none refuses a call with some, as the
driver refuses it for a node that does not accept them.

## The system_server bridge (#430)

Some inputs of the originals' decisions exist only inside system_server
(`LocalServices`), and some change signals reach only processes
ActivityManager knows. What the native services lack (sources at the
tag):

| Input | The original's source | What it takes |
| --- | --- | --- |
| a setting without a generation, another provider's changes | `IContentService.registerContentObserver` | a known process: `checkContentProviderAccess` looks the caller's pid up in `mPidsSelfLocked` ("Failed to find PID") |
| a user's profiles (#460) | user broadcasts (`registerReceiverWithFeature`), or `UserManager`'s cache nonce | a known process; the nonce is in `ApplicationSharedMemory` |
| every change of a permission check's answer (#497) | the `package_info_cache` nonce | `ApplicationSharedMemory`, whose read-only fd only `IApplicationThread.bindApplication` hands out |
| URI grants to a clip's readers (#429) | `UriGrantsManagerInternal.newUriPermissionOwner`, `revokeUriPermissionFromOwner` | code in system_server: no binder interface hands out an owner; `IActivityManager.grantUriPermission` grants as the caller's uid, and a grant by the system uid is refused (`checkGrantUriPermissionUnlocked`) |
| focus (the notification shade, an overlay) | `WindowManagerInternal.isUidFocused`, `registerWindowFocusChangeListener` | code in system_server |
| content capture, augmented autofill, virtual devices | `ContentCaptureManagerInternal.isContentCaptureServiceForUser`, `AutofillManagerInternal.isAugmentedAutofillServiceForUser`, `VirtualDeviceManagerInternal.getDeviceIdsForUid`, `getDeviceOwnerUid` | code in system_server |
| system_server calling a replaced service (notification) | the service's `LocalServices` interface | code in system_server |

**Who ActivityManager knows.** A pid enters `mPidsSelfLocked` in two
places: `ProcessList.handleProcessStartedLocked`, after zygote forked a
process ActivityManager asked for, and `setSystemProcess`, for
system_server's own pid. `attachApplication` from any other process finds
no pending record and kills the caller ("No pending application record").
Speaking `IApplicationThread` does not make a process known; being
started by ActivityManager does.

**Options.**

1. *A persistent system package whose process is the host.*
   ActivityManager starts it through zygote at `systemReady`, so its
   process is known, receives `ApplicationSharedMemory` and may register
   observers and receivers. That process is a new guest process from
   zygote (running `ActivityThread`, or a wrapper program, `wrap.sh` of
   a debuggable app or a `wrap.<name>` property), never the service host
   inside guest-init, and `attachApplication` kills a caller
   ActivityManager did not start. It would be a Java proxy app registering on
   the host's behalf and passing it the shared memory, with a uid of its
   own: `android.uid.system` needs the platform's signing key. It covers
   the first three rows only, adds a process to every boot and a second
   mechanism beside the one the other rows need, and has no use in the
   core milestones.
2. *Registering as a system process, as system_server registers
   itself.* `setSystemProcess` makes a record for system_server's own
   pid; no binder or internal API makes one for another. Doing it anyway
   is code in system_server writing ActivityManager's private state:
   option 3 with more reach into internals.
3. *A device-specific system service in system_server* (chosen). A
   device vendor adds system services without changing SystemServer:
   `config_deviceSpecificSystemServices` (framework-res, set by the
   vendor's static overlay) names classes that `startOtherServices`
   starts with `SystemServiceManager.startService(className)` from the
   system server class path. Code there reaches every row: system_server
   is a known process, owns `ApplicationSharedMemory`
   (`getReadOnlyFileDescriptor`) and holds the `LocalServices`
   interfaces. #470 needs the same service for its
   `ActivityInterceptorCallback`.

**The bridge.** One service in one jar of ours:

- It publishes nothing to servicemanager. The service host registers one
  binder of its own, and the bridge hands it the bridge's binder when
  system services are ready. Only the
  host holds it, and every call must come from the system uid. A new
  system_server attaches again; when it dies, the host drops the bridge
  and everything fed through it, as for any owner ("Mirrored state").
- Its interface is an AIDL file of ours with codes generated for both
  sides. Each method is the binder form of one internal call, with the
  original's arguments: the read-only `ApplicationSharedMemory` fd (the
  host reads the nonces in the pinned `NonceStore` layout; #497, #460);
  registering the host's `IContentObserver` and `IIntentReceiver` as
  system_server's; `newUriPermissionOwner` and
  `revokeUriPermissionFromOwner`, the grants themselves going through
  `IUriGrantsManager.grantUriPermissionFromOwner` as `ClipboardService`
  makes them (#429); `isUidFocused`, with a focus listener that drops
  the host's focus mirror; the content capture, autofill and virtual
  device queries; later, for a replaced service with a local interface,
  its `LocalServices` implementation forwarding to the native binder.
- The clipboard's settings stay on generations: a generation is one
  memory read per use and cannot race a change, where an observer is a
  callback per change. Observers are for inputs without a generation.

**What it adds to the image** (#520; `image/overlay.toml`, ADR 0013).

- `/system/framework/aim-services.jar`: `dev.aim.server.DeviceServices`,
  a `SystemService`, and the Java of our AIDL (`IBridge`, `IServiceHost`
  in `java/device-services/aidl`). It is built by the `device-services`
  node with a pinned JDK and SDK build tools (docs/build.md, "Java"),
  against stubs of the image's classes. The build checks the stubs and
  every class, field and method the dex names against the image's
  jars, so a changed internal API fails the build instead of the boot.
- `/system/etc/classpaths/systemserverclasspath.pb` (a `replace`): the
  platform's fragment with the jar appended. `derive_classpath` puts
  the platform's fragment before the APEXes' (`/apex/*`, sorted), so on
  `SYSTEMSERVERCLASSPATH` the jar comes right after `services.jar`, not
  last. The class loader context of `services.jar` and of the platform
  jars before it stays valid. That of every APEX service jar (and the
  standalone ones, whose parent is the whole path) names the jar, so
  the `oat` node compiles those again with it, as well as the jar
  itself (`speed`). A classpath fragment of its own would need an APEX,
  and a standalone jar (`STANDALONE_SYSTEMSERVER_JARS`) is not on the
  class loader `startService(className)` uses.
- `/vendor/overlay/aim-framework-overlay.apk`: a static overlay of
  `android` that sets `config_deviceSpecificSystemServices`, signed with
  AOSP's public test key.

`DeviceServices` publishes nothing. The service host registers
`aim.service_host` (`IServiceHost`) with servicemanager, which refuses
app uids. SystemServer starts device-specific services after
`PHASE_SYSTEM_SERVICES_READY`, so at its first phase,
`PHASE_DEVICE_SPECIFIC_SERVICES_READY`, the service hands it the
`IBridge` binder with a one-way call. The host takes the bridge only
from the system uid, and the bridge answers only the system uid.
#470's `ActivityInterceptorCallback`, which answers an app's request
for POST_NOTIFICATIONS with the Mac's prompt, registers from the same
service when the host asks (`IBridge.interceptNotificationPermissionRequests`,
only while the Mac shows notifications); its request reaches the host as
`IServiceHost.requestNotificationPermission`, a temporary exception
until PermissionController is native (docs/notifications.md, "The
permission"; #550).

**Maintenance and CTS.** Each method is one internal call; the internal
APIs it names are checked per image. It runs in system_server, so it
throws only what the original caller would. No permission or public API
changes. The one new servicemanager name, `aim.service_host`, belongs to
the service host, and no CTS module expects the set of names. Each
service's own CTS stays the gate.

**Nonces (#497).** `IBridge.getApplicationSharedMemory` returns a
read-only fd of ActivityManager's `ApplicationSharedMemory`
(`getReadOnlyFileDescriptor`, what `bindApplication` hands an app). The
host maps it (`crates/aim-services/src/nonces.rs`) and finds a nonce's
handle in the store's name block, read when its hash (`Arrays.hashCode`)
matches, as `NonceStore.getHandleForName` does. Permission checks
(`IActivityManager.checkPermission`, `IPermissionManager.checkPermission`)
are then kept as `PermissionManager`'s `sPermissionCache` and
`sPackageNamePermissionCache` keep them ("Mirrored state").

**Measured** (2026-09-30, the second boot of a data directory, load
about 8, a binder trace over the CTS runs below). system_server logs
`StartDeviceSpecificServices dev.aim.server.DeviceServices` and
"bridge attached to the native service host". `cmd overlay list android`
shows the overlay enabled, and `sys.boot_completed` came after 12 s.
During CtsVibratorTestCases, every `vibrate` and `cancelVibrate` checks
VIBRATE:

| | Calls | p50 / p99 |
| --- | --- | --- |
| a kept answer (no call to system_server) | 45 | 10.9 / 84.8 us |
| a check asked (`IActivityManager.checkPermission`, then kept) | 60 | 56.2 / 207.9 us |
| the host's `checkPermission` itself | 74 in the boot | 34.8 / 203.8 us |

So a check costs one mutex and one memory read instead of a round trip
of about 35 us to system_server (p50). The CTS app adopts and drops
shell permissions around its tests, and each change bumps the nonce, so
more than half of its checks are asked. An app without delegation has
its answers kept until a permission or package changes. The
same boot passed the five clipboard classes (36 of 36) and
CtsVibratorTestCases (268 pass, 33 skip, as recorded), and started
Settings cold in 231 ms.

**The core milestones.** Each method is the binder form of an owner
still in system_server and goes when its owner moves native (URI grants,
window focus, ActivityManager's provider access check). A native
ActivityManager knows the service host without a bridge, and the shared
memory is its own. With SystemServer gone, the bridge is gone.

**Order.** Done: the build of the jar and the overlay, shared with #470
(#520), and the permission nonce (#497). Next: the user nonce (#460);
URI grants (#429); focus, content capture, autofill and virtual devices
(#430).

## Inventory: what apps use

**Method.** guest-init `--binder-trace FILE` makes the binder driver
record every transaction: sender pid and euid, target pid, the interface
token at the start of the data, code, one-way or not, and for a
synchronous call three spans: until a target thread's read takes it (the
wake of a free thread, or the wait for one), until the driver takes the
target's `BC_REPLY` (its work), and until the sender's read takes the
reply; plus the target's free threads, the sending thread and the
serving thread. On a booted device, each
app was cold-started with `am force-stop; am start -W -S`, left 6 s, and
its pid taken from ActivityManager's `Start proc` line.
`tools/binder-trace-report.py` names services (from `service list`) and
methods (from the `TRANSACTION_*` constants of every stub in the image's
jars), splits each service's time into wake, work, the serving thread's
own nested calls and return, and lists the slowest calls. `cargo aim
boot -- --binder-trace FILE` records one.

**Cold starts on the original stack** (2026-09-29, first boot of a reused
data image, M2 Pro, load average about 20):

| App | Transactions | One-way | Services | Synchronous p50 | p99 |
| --- | --- | --- | --- | --- | --- |
| Settings | 407 | 64 | 45 | 2.1 ms | 113 ms |
| Calculator | 142 | 49 | 24 | 1.4 ms | 117 ms |
| Chrome | 1,215 | 248 | 51 | 0.53 ms | 15.6 ms |

Services by calls (Settings, Calculator, Chrome); interfaces without a
servicemanager name are objects handed out by a service:

| Service | Settings | Calculator | Chrome |
| --- | --- | --- | --- |
| device_policy | 5 | 0 | 492 |
| IDisplayEventConnection (SurfaceFlinger) | 22 | 25 | 142 |
| package | 83 | 10 | 74 |
| activity | 39 | 8 | 85 |
| SurfaceFlinger, SurfaceFlingerAIDL | 32 | 30 | 88 |
| manager (servicemanager) | 33 | 16 | 33 |
| IContentProvider (mostly the settings provider) | 31 | 7 | 31 |
| connectivity, network_management | 2 | 2 | 67 |
| IActivityClientController, IWindowSession, window | 17 | 13 | 54 |
| user | 20 | 1 | 8 |
| appops | 27 | 0 | 1 |
| safety_center | 23 | 0 | 0 |
| allocator HAL | 7 | 7 | 7 |
| display, accessibility, input_method | 14 | 13 | 13 |
| content, appwidget, notification, activity_task, uimode, voiceinteraction, content capture | 13 | 4 | 50 |
| clipboard | 0 | 2 | 1 |
| 38 others (alarm, role, usb, media.camera, GMS's own interfaces, pings, ...) | 39 | 4 | 69 |

What stands out:

- Chrome calls `IDevicePolicyManager.getProfileOwnerAsUser` and
  `getDeviceOwnerComponent` 246 times each during its start.
- Settings reads package and app-op state heavily (83 `package`, 27
  `appops` calls); every app reads the settings provider.
- servicemanager is slow: `isDeclared` 95-117 ms, `updatableViaApex`
  110 ms, `checkService2` 4 ms at p50 (#435; why: "Where a cold start's
  binder time goes").
- Calculator reads the clipboard at start (`addPrimaryClipChangedListener`,
  `getPrimaryClip`), Chrome registers a listener.

### Binder latency baseline

ADR 0013's target: p50 < 20 us, p99 < 200 us per call.

| What | p50 | p99 |
| --- | --- | --- |
| All synchronous calls of a first boot (59,624, load about 20) | 1.2 ms | 318 ms |
| Calls answered by system_server (44,000) | 0.9 ms | 53 ms |
| Calls answered by servicemanager (3,295) | 5.6 ms | 831 ms |
| `IClipboard.hasPrimaryClip` from the shell, idle, original (41 calls) | 221 us | 997 us |
| The same, native clipboard asking system_server (40 calls) | 541 us | 2,988 us |
| The same, native clipboard with mirrored state (40 calls, 2026-09-30) | 15 us | 95 us |
| `addPrimaryClipChangedListener` during a cold start, original | 377-1,878 us | |
| The same, native clipboard | 15-18 us | |

A binder hop itself costs 6.5 us (docs/binder-driver.md); what the
table shows is the servers' work, under load, and system_server's Java.

### Where a cold start's binder time goes

Measured 2026-09-30 on a settled boot (the second boot of a data
directory, 5 min after `sys.boot_completed`, each app started once
before, host load about 3.6, guest at nice 0; M2 Pro, 16 GB). TotalTime
of `am start -W -S`: Chrome 756 ms, Settings 754 ms, Calculator 1,134 ms.

| Span of a synchronous call (all 796 of the three starts) | p50 | p99 |
| --- | --- | --- |
| Wake of a free server thread | 8 us | 332 us |
| The server's work | 583 us | 24.5 ms |
| Of that, the serving thread's own binder calls | 0 | 6.1 ms |
| Return to the sender | 7 us | 132 us |

- **The transport is not the cost.** No call waited for a busy server
  (every server had a free thread); wake and return are 7-9 us. What is
  slow is the servers' work, and it is rarely nested binder calls: of the
  20 slowest calls (12-53 ms) only the settings provider's `call` spends
  most of its time in calls of its own. Trivial Java methods take
  milliseconds: `IUserManager.getUserPropertiesCopy` 12.7 ms,
  `getProfileParent` 12.5 ms, `INotificationManager.getNotificationChannels`
  47 ms, `IActivityManager.setProcessStateSummary` up to 24 ms.
- **The servers ran in the kernel** (fixed, #444). Fork children mapped
  the shared libraries' code through memory entries mapped executable,
  and Darwin takes a page faulted through such a mapping from every
  other mapping of it, so every app forked from zygote made all other
  guest processes refault libc's, libc++'s and libbase's hot code (a
  `free(malloc(64))` loop: 0.16-0.72 faults per iteration while an app
  started; guest processes 95 % system time). With the code mapped
  without execute and made executable after (docs/fork.md), the same
  loop takes no fault during a start, and a cold start of Settings,
  Calculator and Chrome went from 1.0, 1.45 and 1.1 s to 0.23, 0.31 and
  0.29-0.34 s (2026-09-30, settled boot, host load 2-3). Seven minutes
  after boot the guest has used 78 s of CPU (42 s user), where it used
  1,565 s (60 s user). The system time left, 30 % at idle and 46 % over
  the boot, is syscalls and first-touch faults, not refaults (#446).
- **servicemanager** (#435) answers `isDeclared`, `getService2` and
  `updatableViaApex` for the graphics allocator in 20-53 ms each during a
  start: 163 ms of Calculator's 318 ms of synchronous binder time. Its
  work is libvintf walking the manifests (malloc/free of many small
  strings), 120 us back to back and 1.1-1.7 ms after 300 ms of idle,
  without faults (a native Darwin program's burst of that size is as slow
  after such an idle: the core's clock). Under the fault storm it took
  0.3-1 s right after boot, when com.android.phone restarts after each
  startup ANR (#241) and asks for the radio HALs every 0.4 s. Not VINTF
  re-reads (the cache holds), file access or the driver. Its "no idle
  thread" was the trace counting only threads waiting in a read; an
  epoll looper's idle poll-mode thread now counts as free.
- system_server: 99.9 % of its threads' samples wait in the kernel
  (kevent, ulock, mach_msg). In three of four runs a CLOSE transition's
  task snapshot hung in SurfaceFlinger (RenderEngine waiting on a fence)
  and the watchdog killed system_server (#436); once in two boots with
  #444 fixed.

## Shrinking SystemServer

What SystemServer starts, what each start and each running service
costs, and which services can stop (ADR 0013 decision 5.3, and the
"original not started" step of every replacement). The measurements
below are from before any of it was turned off; what is off since is
under "A1".

**Method.** One disposable data directory booted twice (main 712da8eb,
M2 Pro, host load about 9): a first boot, then Chrome
(`org.chromium.chrome`) installed and Settings and Chrome started once,
4 minutes to settle, then a second boot with `--binder-trace`. At this
image's log level `TimingsTraceAndSlog` logs only where each section
begins (`SystemServerTiming`, `ActivityManagerTiming`; the "took"
line is verbose), so a section's time is until the next line of the
same thread, at millisecond resolution, untraced work included. Phases
come from the events log (`boot_progress_*`), times from guest-init's
spawn. Idle: host CPU and RSS of every guest process 3 minutes after
the second boot, and again 2 minutes later, with the binder trace of
those 2 minutes; then Settings and Chrome started cold
(`am start -W -S`: 596 and 460 ms) with the trace of each.

### Boot timeline

| Phase | First boot | Second boot |
| --- | --- | --- |
| Zygote starts (`boot_progress_start`) | 4.5 s | 7.1 s |
| Preload | 4.9-6.4 s | 7.5-8.7 s |
| `system_run` | 6.8 s | 9.2 s |
| `pms_ready` | 12.0 s | 10.2 s |
| `startOtherServices` | 12.3-15.8 s | 10.4-11.1 s |
| `ams_ready` | 15.5 s | 11.0 s |
| Home and SystemUI started | 15.8 s | 11.1 s |
| `enable_screen` (the launcher drew) | 16.2 s | 11.5 s |
| SurfaceFlinger's boot finished (keyguard and wallpaper drawn) | 16.5 s | 11.8 s |
| `sys.boot_completed` | 16.6 s | 11.8 s |

Both boots start the same services before zygote in the same order; why
zygote came 2.6 s later in the second (the binder trace, host load, the
existing data image) was not established.

**Where system_server's time goes** (second boot, `system_run` to
`ams_ready`, 1.8 s): SystemServer's init and system config 0.3 s,
PackageManagerService 643 ms, OverlayManagerService 194 ms, system
providers 73 ms, network stats 84 ms, Wi-Fi 42 ms, fonts 35 ms,
ActivityManager 32 ms, connectivity 29 ms, notification 22 ms; every
other service takes 0-3 ms to start and its boot phases as little (1,111
sections under 5 ms, 262 ms in all). The first boot adds the package
scan (PackageManagerService 4.9 s) and dexopt (`UpdatePackagesIfNeeded`,
2.3 s).

**What `sys.boot_completed` waits on.** SystemServer starts home and
SystemUI together (11.12 s). ActivityManager enables the screen when
home goes idle: the launcher's process started at 11.2 s, resumed at
11.37 s and drew at 11.455 s, and `enable_screen` followed at 11.458 s
(the keyguard service was not connected yet). WindowManager then waits
for SystemUI's windows, the keyguard (notification shade) and the image
wallpaper, before it tells SurfaceFlinger the boot finished (11.774 s);
ActivityManager's `finishBooting` runs boot phase 1000 (13 ms) and sets
`sys.boot_completed` (11.79 s). No boot animation is waited for. The
shell holds boot completion for 0.67 s after home is started: 0.34 s for
the launcher, 0.32 s for the keyguard and wallpaper.

### Steady state

- **Idle.** Over 2 idle minutes all 114 guest processes used 2.24 s of
  CPU. system_server used 0.24 s; it has 208 threads and 304 MB of RSS
  (shared pages counted) and received 418 binder transactions, nearly
  all from GMS (`activity` 135, `power` 52, `package` 46, `alarm` 17,
  `connectivity` 16), 59 ms of work. The largest idle consumer is
  Bluetooth: its HAL sends the Bluetooth app about 11 HCI events a
  second (1,342), a third of the guest's idle CPU (#514).
- **From boot to idle** (3 minutes): system_server 8.1 s of CPU,
  SystemUI 1.5 s, the launcher 2.5 s, GMS 6.3 s, all processes 60 s.
- **Cold starts.** system_server received 886 transactions during
  Settings' start and 536 during Chrome's; its work was 252 and 84 ms
  (`am`'s own shell command excluded). Settings: `location` 129,
  `package` 110, `permissionmgr` 110, `activity` 80, `device_policy`
  61, `appops` 53, the settings provider 25, `safety_center` 23, `user`
  22. Chrome: `activity` 123, `package` 72, `connectivity` 42,
  `activity_task` 17, `content` 16, the rest under 10 each.
- **Per service** inside system_server, threads and CPU cannot be told
  apart yet: another process's `/proc/<pid>/task` lists only its main
  thread (#379), zygote's children have an empty `comm` to other
  processes (#238), and the host threads are unnamed. What the tables
  below give per service is its start time and the binder calls it
  received.

### Classification

Every service SystemServer starts on this image, in four classes:

- **A**: the Mac has no such hardware or function. **A1**: a device
  without it does not start it, by its configuration: a feature in
  the permissions XML, a `config_` resource (a vendor overlay) or a
  build property. **A2**: every device of this kind starts it; only an
  edit of SystemServer (the `image/native-services` edit) stops it.
- **B**: the Mac owns what it does: a native replacement (M2), or the
  shell (M1).
- **C**: the core (M4-M6).
- **D**: needed as it is for now.

Start times are the service's start plus its boot phases, first / second
boot, in ms; calls are the binder transactions it received in the 2 idle
minutes / Settings' start / Chrome's start.

**Already off** on this device: ConsumerIr, MMS (no telephony feature),
fingerprint, face and iris, HDMI-CEC, TV input and interactive apps,
tuner, broadcast radio, context hub, UWB, persistent data block and OEM
lock (no `ro.frp.pst`), VR, Wear and Auto services, display offload,
isolated compilation, contextual search and system captions (no
`config_` service), the vibrator (native), the clipboard (native).

**A1**

| Service | Start ms | Calls | How a device without it avoids it | CTS and app impact |
| --- | --- | --- | --- | --- |
| Wi-Fi: `wifi`, `wifiscanner`, `wifip2p` (WifiService, WifiScanningService, WifiP2pService) | 133 / 45 | 3 / 1 / 3 | features `android.hardware.wifi`, `.wifi.direct`, `.wifi.passpoint` (`/vendor/etc/permissions`); init's `wificond` has its own script | The guest's network is `eth0`; Android's Wi-Fi has no HAL here and never connects. Without the feature `getSystemService(WIFI_SERVICE)` is null (CTS expects that), Wi-Fi tests skip, Play filters apps that require Wi-Fi. A native `wifi` reporting the Mac's Wi-Fi would be B instead. |
| `usb` (UsbService) | 2 / 0 | 0 / 3 / 0 | feature `android.hardware.usb.host` (no accessory feature is declared) | No USB device reaches the guest. UsbManager has no service behind it; USB tests require the feature. Settings asks it 3 times at start. |
| `network_time_update_service` (NetworkTimeUpdateService) | 0 / 0 | - | build property `config.disable_networktime=true` | The Mac owns the clock. Time detection keeps its other sources. |
| `otadexopt` (OtaDexOptService) | 0 / 0 | - | build property `config.disable_otadexopt=true` | A/B OTA dexopt; the image is updated by `cargo aim`, never by OTA. |
| GestureLauncherService | 0 / 0 | - | `config_cameraDoubleTapPowerGestureEnabled`, `config_emergencyGestureEnabled` and the camera lift trigger off | Power-button gestures; the Mac has no power button events. |
| `wallpaper_effects_generation` | 1 / 0 | 0 | `config_defaultWallpaperEffectsGenerationService` empty | Pixel's generated wallpaper effects. |

Off since 2026-09-30, by the device's configuration: network time and
OTA dexopt (`init.aim.rc` sets the two properties at `early-init`).
Wi-Fi stays declared: presenting the Mac's network as Wi-Fi (#265)
needs the features. USB stays: without the `usb` service GMS persistent
crash-loops (#336), so `android.hardware.usb.host` is declared.
The gesture launcher and wallpaper effects need a framework-res
overlay, which the image build cannot make yet (#526).

Persistent or boot-started system apps without hardware (not
SystemServer services; a device without the hardware does not ship
the APK, a `remove` in `image/overlay.toml`): `com.android.se`
(SecureElement, no eSE or UICC), `com.android.dynsystem` (two processes
at `BOOT_COMPLETED`, dynamic system updates), `com.android.emulator.multidisplay`
(#462). With `com.android.phone` (A2) they are five processes, 0.56 s of
CPU up to idle. The three are removed since 2026-09-30.

**A2**

| Service | Start ms | Calls | CTS and app impact |
| --- | --- | --- | --- |
| TradeInModeService | 2 / 7 | - | Phone trade-in evaluation (a read-only aconfig flag, `enableTradeInMode`). |
| `soundtrigger_middleware`, `soundtrigger` | 4 / 4 | 0 | No hotword DSP (no sound trigger HAL): no modules. VoiceInteraction asks for them. |
| `telephony.registry` (TelephonyRegistry) and `com.android.phone` | 2 / 1 | 0 | Tablets without telephony start both. Apps register telephony callbacks without checking the feature, so the registry stays unless replaced by a native one that never calls back. |
| EmergencyAffordanceService | 0 / 2 | - | Emergency-call affordance of telephony. |
| `reboot_readiness`, `system_update`, `updatelock`, `recovery` (RecoverySystemService), `dynamic_system` | 5 / 5 | 0 | OTA, recovery and dynamic system updates; factory reset (`rebootWipeUserData`) is the one path apps and Settings reach, which on the Mac is a new data image. |
| `serial` (SerialService) | 1 / 1 | 0 | No serial ports. |
| DockObserver, WiredAccessoryManager | 1 / 1 | - | Dock and headset-jack events from `/sys/class/switch` and input; the Mac's audio routes are the host's. |
| `lights` (LightsService) | 0 / 0 | 0 | No lights HAL; serves an empty list. |
| TestHarnessModeService | 1 / 1 | - | Needs the persistent data block, which is off (it logs so at boot). |

All of A together: 152 ms of the first boot's SystemServer and 67 ms of
the second's, Wi-Fi most of it.

**B**, by start cost (the shell last):

| Service | Start ms | Calls | macOS owner |
| --- | --- | --- | --- |
| `netstats`, `connectivity`, `netpolicy`, `ethernet`, `network_management` (NetworkStatsService, ConnectivityService, ...) | 180 / 125 | 26 / 13 / 45 | Network framework (#304 feeds `eth0` today) |
| `notification` | 19 / 22 | 0 / 2 / 2 | UserNotifications (#4; not a leaf, #430) |
| `audio` (AudioService) | 22 / 13 | 0 | Core Audio (HAL today) |
| `power` (PowerManagerService), `batteryproperties`, `battery` | 18 / 15 | 52 / 3 / 5 | IOKit power sources and assertions (health HAL today) |
| `location`, `country_detector`, `location_time_zone_manager` | 17 / 12 | 0 / 129 / 0 | Core Location (GNSS HAL today) |
| `display` (DisplayManagerService) | 9 / 9 | 1 / 6 / 9 | NSScreen |
| `input_method` (InputMethodManagerService) | 18 / 6 | 0 / 4 / 3 | The Mac's input methods (#23) |
| `time_detector`, `time_zone_detector`, `alarm` | 9 / 8 | 17 / 6 / 0 | The Mac's clock and time zone (#283) |
| `uimode` (UiModeManagerService), `color_display`, twilight | 2 / 3 | 0 / 0 / 3 | Appearance and Night Shift (#281) |
| `locale` (LocaleManagerService) | 2 / 1 | 0 / 1 / 1 | The Mac's languages (#282) |
| `sensor_privacy` | 3 / 8 | 0 | Camera and microphone privacy (TCC, #291) |
| `media_session`, `media_router`, `media_projection`, `media_communication` | 10 / 7 | 0 | Now Playing, AirPlay, ScreenCaptureKit |
| `thermalservice`, `hardware_properties` | 3 / 2 | 0 / 0 / 2 | Thermal state (HAL today) |
| `biometric`, `auth` | 7 / 4 | 0 | Touch ID; no biometric HAL today |
| `print`, `midi` | 1 / 2 | 0 | macOS printing, Core MIDI |
| Shell (M1): `statusbar`, `wallpaper` (off by `config_enableWallpaperService`), `appwidget` (feature `android.software.app_widgets`), `dreams`, `search_ui`, `smartspace`, `app_prediction` (the launcher's; off by their `config_` services) | 8 / 8 | 0 / 15 / 0 | The Mac's desktop, Dock and menu bar |

**C**: `package` (PackageManagerService with the installer, domain
verification, dexopt and `overlay`), `activity` and `activity_task`
(with `appops`, `batterystats`, `procstats`, `permission`,
`permissionmgr`, access checking, the permission policy), `user`,
`window`, `input` (InputManagerService), `content` and the settings
provider.

**D**: everything else, as it is. Device and storage: StorageManager,
StorageStats, DeviceStorageMonitor, CameraServiceProxy, SensorService,
SensorNotification, DeviceStateManager, Bluetooth (over our HAL),
Telecom, Adb, Font, WebViewUpdate, Pinner (pins the launcher and
Trichrome in memory), GpuService, HintManager, PowerStats. App model and
policy: Account, DeviceIdle, JobScheduler, UsageStats, AppHibernation,
GameManager, AppCompatOverrides, Backup, BlobStore, Slice, Shortcut,
LauncherApps, CrossProfileApps, People, Restrictions, DevicePolicy,
Role, Supervision, EnhancedConfirmation, AppFunction, IntrusionDetection,
AdvancedProtection, AuthenticationPolicy, Trust, LockSettings,
BackgroundInstallControl, AppBinding, VirtualDevice, CompanionDevice,
SafetyCenter, AppSearch, HealthConnect, AdServices, OnDevicePersonalization,
SdkSandbox, DeviceLock (the devicelock APEX declares its feature),
Ranging (started for Bluetooth LE with the channel-sounding flag). Text, voice and
content: Accessibility, TextServices, TextClassification, Autofill,
Credential, ContentCapture (the clipboard consults it), VoiceInteraction,
SpeechRecognition, TextToSpeech, MusicRecognition, AmbientContext,
WearableSensing, OnDeviceIntelligence, Translation, Search. Network:
NetworkScore, VpnManager, PacProxy, SecurityState, NetworkStack,
Tethering. Infrastructure: Watchdog, PlatformCompat, FileIntegrity,
FeatureFlags, UriGrants, IStats, MemtrackProxy, DataLoaderManager,
Incremental, SystemConfig, CachedDeviceState, BinderCallsStats,
LooperStats, NativeTombstoneManager, BugreportManager, DropBox,
EntropyMixer, SchedulingPolicy, KeyChain, KeyAttestationApplicationIdProvider,
BinaryTransparency, AttestationVerification, SignedConfig, AppIntegrity,
NetworkWatchlist, IpConnectivityMetrics, SelinuxAuditLogs,
DynamicCodeLogging, PruneInstantApps, LogcatManager, Tracing,
DynamicInstrumentation, StatsCompanion, StatsPullAtom, StatsBootstrapAtom,
IncidentCompanion, Profiling, CrashRecovery, Rollback, RemoteProvisioning,
MediaResourceMonitor, MediaMetrics, DiskStats, Runtime, GraphicsStats.

### What stopping them would save

- **A**, ranked: Wi-Fi (45-133 ms, the `wificond` daemon and Wi-Fi's
  threads), the four hardware-less system apps and `com.android.phone`
  (five processes, 0.56 s of CPU after boot), TradeInMode (7 ms), sound
  trigger (4 ms), then 1-2 ms each. With all of A off,
  `sys.boot_completed` would come about 0.07 s earlier on a second boot
  (11.8 to 11.7 s) and 0.15 s on a first: within noise. No A service
  received a binder call at idle or during the cold starts, apart from
  Wi-Fi's 3 and USB's 3 (Settings).
- **B**, ranked by boot cost: network 125-180 ms, notification 19-22 ms,
  audio 13-22 ms, power and battery 15-18 ms, location 12-17 ms (and 129
  calls in Settings' start), display 9 ms, input method 6-18 ms,
  media 7-10 ms, time 8-9 ms. By steady-state traffic: `power` (52 calls per idle 2
  minutes), `alarm` (17), `connectivity` (16 idle, 42 in Chrome's
  start), `location` (Settings).
- **M1**: without SystemUI and the launcher gating it, boot completion
  would follow home's start directly: about 11.15 s instead of 11.8 s on
  the second boot (-0.65 s), and SystemUI (86 threads, 1.5 s of CPU to
  idle), the launcher (50 threads, 2.5 s) and the wallpaper would not
  run.
- **Larger than all of these**: the time before zygote (4.5-7.1 s:
  guest-init, early init, the native services), zygote's preload
  (1.3-1.5 s) and PackageManagerService (0.64 s on a second boot). The
  services SystemServer starts cost about 0.7 s of a second boot's
  11.8 s, and at idle system_server uses 0.2 % of a core; stopping A
  saves little time: its value is fewer threads and binder surfaces,
  and a smaller original to replace.

## Internal dependencies of leaf candidates

From the sources at `android-16.0.0_r1` (`services/core`):

- **clipboard** (`ClipboardService`). Consults ActivityManagerInternal
  (`handleIncomingUser`, `getUidProcessState`), UriGrantsManagerInternal
  (permission owner, grant checks, revocation), WindowManagerInternal
  (`isUidFocused`, `getTopFocusedDisplayId`), VirtualDeviceManagerInternal,
  ContentCaptureManagerInternal, AutofillManagerInternal and
  PackageManagerInternal (`isSameApp`); over binder or managers:
  IUriGrantsManager, IUserManager, AppOpsManager, PackageManager,
  KeyguardManager, TextClassificationManager, Toast, DeviceConfig,
  Settings.Secure, statsd. Publishes no `LocalServices` interface; nothing
  in system_server calls into it. A leaf.
- **vibrator** (`VibratorManagerService`). Consults
  PowerManagerInternal, VirtualDeviceManagerInternal and
  PackageManagerInternal; IBatteryStats, AppOps, PowerManager,
  AudioManager, InputManager. Publishes no local interface; system_server
  uses it through the public Vibrator API (PowerManager's Notifier,
  ActivityManager, notification's VibratorHelper, AudioService,
  PhoneWindowManager, biometrics), which a native binder serves as well.
  A leaf; the device declares no vibrator. Native since M2 ("The
  vibrator").
- **notification** (`NotificationManagerService`). Consults twelve local
  interfaces (UsageStats, PermissionPolicy, JobScheduler, ActivityManager,
  ActivityTaskManager, WindowManager, UserManager, UriGrants,
  PackageManager, DevicePolicy, Lights, StatusBarManager) and SystemUI's
  NotificationDelegate, and publishes `NotificationManagerInternal`, which
  ActivityManager (foreground-service notifications), WindowManager's
  DisplayPolicy, PermissionPolicyService and three job services call. Not
  a leaf: it needs the bridge in both directions (#430).
  Its notifications reach the Mac without replacing it: a native listener
  registered with the original ([notifications.md](notifications.md)).

## The clipboard (M1 pilot)

`crates/aim-services/src/clipboard.rs` follows `ClipboardService.java` at
the tag: a primary clip per user and device, set by any app and read only
by the focused app, the default input method or a holder of
READ_CLIPBOARD_IN_BACKGROUND (INTERNAL_SYSTEM_WINDOW with focus for system
windows); each access checks the caller's package and notes the
READ/WRITE_CLIPBOARD app op (a MODE_ERRORED throws, as `noteOp` does);
listeners are told of each change they may read and forgotten when they
die; a clip is copied to related profiles unless a restriction forbids it,
and cleared after its last use as `DeviceConfig` says
(`clipboard/auto_clear_enabled`, `auto_clear_timeout`, an hour by
default); a setter must be able to grant read access to the clip's
`content:` URIs and intent data (`checkDataOwner`, through
`IUriGrantsManager.checkGrantUriPermission_ignoreNonSystem`, the binder
form of the same check for system callers). The access notification
methods read and write `Settings.Secure.clipboard_show_access_notifications`,
and the default input method is `Settings.Secure.default_input_method`
("Settings" above). `ClipData` is read and written in its Java parcel
form (`clip.rs`): every framework span, `TextLinks`, bundles with file
descriptors (held as files while the clip is, as its binders are); a
dump writes nothing, as the original's.

The Mac takes the emulator's place (`EmulatorClipboardMonitor`): a clip set
on the default device puts its first item's text on the Mac's pasteboard;
text copied on the Mac becomes user 0's clip, labelled "host clipboard"
with SystemUI's `SUPPRESS_CLIPBOARD_OVERLAY`, and set as a mirrored
device's clipboard sync sets it: by the shell uid, from
`com.android.shell`, the one source SystemUI honours the extra for off
the emulator (`ClipboardOverlaySuppressionControllerImpl`), so a copy on
the Mac raises no overlay in the device window. The Mac's text is read
only when an app pastes it (a change is noticed from `changeCount` and
the types, which do not read the content). Clearing an Android clip
clears the Mac's pasteboard only while it still holds that clip.

Where the original uses a system_server-internal API with no binder form,
the native clipboard does without, or stands in:

- focus: the focused root task's `effectiveUid` for
  `WindowManagerInternal.isUidFocused`; content capture, autofill and
  virtual devices not consulted (#430);
- no URI permission grants to the apps that read a clip (#429): the
  grant (`IUriGrantsManager.grantUriPermissionFromOwner`) takes a
  permission owner only `UriGrantsManagerInternal.newUriPermissionOwner`
  makes, and no binder interface hands one out;
- no paste toast, text classification (clips are marked
  `CLASSIFICATION_NOT_PERFORMED`) or statistics (#431). The toast's
  text is the app's label in the device's language and the framework's
  `pasted_from_clipboard`, which system_server resolves with the app's
  and its own `Resources` (no binder interface returns a label), and
  with safety protection on it is a custom-view toast, a window
  system_server adds itself; classification is a session of the
  `textclassification` service with its request, callback and result
  parcels; the statistics are statsd socket writes from system_server's
  identity;
- a clip icon (`Bitmap`) and an item's `ActivityInfo` are refused: no
  API gives an app's clip either, and the original drops the activity
  info when it hands a clip out.

**Cost.** A call that needs no check is fast: `addPrimaryClipChangedListener`
takes 15-18 us against 377-1,878 us for the original. A checked call
reads mirrored state ("Mirrored state"), so a focused app's read makes
no call into system_server unless an installed instrumentation targets
the app (a test, which asks the app op's mode); a read by an app without
focus asks a permission (the input method setting is read again only
after it changed; before that it was asked each time, as in the numbers
below). Measured 2026-09-30 in one boot each,
before and after the mirror, with a binder trace over the CTS run below
(the same data directory; host load 21 before, 6 after, so the shell
loop's numbers are the cleaner comparison):

| Call (sender's total, p50 / p99) | Asking system_server | Mirrored |
| --- | --- | --- |
| `hasPrimaryClip`, shell loop (40) | 293 / 728 us | 15 / 95 us |
| `hasPrimaryClip`, CTS app (69) | 716 / 2,149 us | 61 / 1,162 us |
| `getPrimaryClip`, CTS app (29) | 506 / 1,186 us | 61 / 634 us |
| `getPrimaryClipDescription`, CTS app (30) | 622 / 1,353 us | 52 / 339 us |
| `getPrimaryClip`, input method without focus (48) | 455 / 16,398 us | 254 / 4,771 us |
| `setPrimaryClip`, CTS app (29) | 2,334 / 8,575 us | 2,475 / 11,522 us |

With an instrumentation target's modes asked (2026-09-30, one boot,
the same data directory and run, host load about 12), the CTS app, a
target, pays one `checkOperationForDevice` per read while the shell in
the same boot stays mirrored: `hasPrimaryClip` 227 / 1,685 us (CTS app,
116), 20 / 142 us (shell loop, 40); `getPrimaryClip` 177 / 512 us,
`getPrimaryClipDescription` 165 / 471 us. Whether a uid is a target took
nine `IPackageManager` calls over the run. Results of the CTS classes
below and of CtsVibratorTestCases (268 pass, 33 skip) are unchanged.

The mirror took the service host's calls into system_server over the
run from 2,582 (1,106 app ops, 593 permissions, 369 trust, 285 input
method, 146 focus) to 972 (273 app ops, nearly all background notes; 356
input method and 213 permissions, from reads without focus; 43 focus);
asking a target's modes brings the app ops back to 463 (196 checks). A write still asks `IUserManager` for the profiles
(#460).

## The vibrator

`crates/aim-services/src/vibrator.rs` serves `vibrator_manager` and
`external_vibrator_service`, the binders SystemServer's
`VibratorManagerService` publishes. A Mac has no vibration motor and the
derived image no vibrator HAL, so the original ran without a vibrator:
no vibrator ids, capabilities 0, no `VibratorInfo`, "No vibrator found"
from `cmd vibrator_manager list`. The native service is that original,
`VibratorManagerService.java` at the tag with no `VibratorController`:

- `isVibrating` and the state listener calls check
  ACCESS_VIBRATOR_STATE and answer false: no vibrator has the id;
- `vibrate` and `cancelVibrate` check VIBRATE, and a vibration with a
  token for another uid UPDATE_APP_OPS_STATS, with the original's
  messages, then have nothing to play or cancel; `setAlwaysOnEffect`
  checks VIBRATE_ALWAYS_ON and fails for anything but a removal; haptic
  feedback (one-way) plays nothing;
- a vendor session (`vendor_vibration_effects` is on in the image)
  checks its three permissions and ends unsupported: its callback gets
  `onFinished(STATUS_UNSUPPORTED)` without starting, and cancelling it
  changes nothing;
- `external_vibrator_service` mutes every external (audio-coupled)
  vibration, as the original does without a vibrator with external
  control (`haptics_scale_v2_enabled` is off, so the factor stays
  undefined).

Combined vibrations are read to their end as Java reads them (parcel
tokens, length-prefixed segments, the vendor data's bundle) and refused
where Java's reading throws; their content decides nothing without a
vibrator. The original's third binder, the vibrator control service
(`IVibratorControlService/default`), is published only when declared;
only a vibrator HAL's vendor side calls it, and the derived image drops
its declaration with the HAL.

Not here: the `OP_VIBRATE` access the original records for a vibration
its settings let through, which plays on no vibrator (#479); `dumpsys`
and `cmd vibrator_manager` (#480).

**Cost** (2026-09-30, one boot each, the same data directory, host load
about 7; the driver's latency of each synchronous call, from a binder
trace over the shell loop and the CTS run below):

| Call | Original p50 / p99 | Native p50 / p99 |
| --- | --- | --- |
| `getCapabilities`, shell loop (40) | 33 / 191 us | 9 / 22 us |
| `getVibratorIds`, shell loop (40) | 31 / 165 us | 8 / 15 us |
| `vibrate`, CTS app (40) | 231 / 1,601 us | 41 / 117 us |
| `cancelVibrate`, CTS app (65) | 81 / 1,050 us | 42 / 145 us |

`vibrate` and `cancelVibrate` check VIBRATE; since the system_server
bridge the answer is kept by the permission nonce, and a kept one takes
`vibrate` to about 11 us (p50, "The system_server bridge"). The rest asks
nothing.

## The location service

`crates/aim-services/src/location/` serves `location` (ILocationManager),
the one binder SystemServer's `LocationManagerService` publishes,
following `LocationManagerService.java` and `LocationProviderManager.java`
at the tag:

- **Providers**, in the original's order: `passive`; `network` and
  `fused`, bound from the apps that serve them (Google Play services on
  this image, as the original binds them: `config_enable*Overlay`,
  `ServiceWatcher`); `gps`, the Mac's location (below); and test
  providers. Each manager keeps its registrations (listeners, pending
  intents, current-location requests, the service's own), decides which
  are active (permission and app op, the location setting per user, a
  visible user, the package denylist, the location power save mode),
  merges the active ones into the provider's request (delayed as the
  original delays it), keeps the last locations per user (fine, coarse,
  and with the settings bypass), and delivers what the provider reports
  with the original's checks (the fastest interval with its jitter, the
  smallest displacement, maximum updates, expiration) and app ops
  (`FINE_LOCATION`/`COARSE_LOCATION` noted before each delivery,
  `MONITOR_LOCATION` and `MONITOR_HIGH_POWER_LOCATION` started while a
  registration is active).
- **The Mac's location** (`mac.rs`): CoreLocation's fixes from the host
  module the GNSS HAL uses (`aim_host_location`, in this process), read
  every second while a provider served by it has a request, each new fix
  reported once per provider interval. A fix becomes a `Location` as the
  HAL builds a `GnssLocation`, with `GnssLocationProvider`'s extras. The
  gps provider has `GnssLocationProvider`'s properties and identity
  (`android`, `GnssService`), and the GNSS surface answers as the
  original does with the HAL: capabilities `SCHEDULING`, hardware
  "darwin CoreLocation", no satellites, NMEA, measurements, navigation
  messages or antenna information; status listeners hear the session
  start and stop and the first fix.
- **Coarse locations** (`fudger.rs`, `s2.rs`): `LocationFudger`, with the
  population density provider's S2 cells (`LocationFudgerCache`) for the
  providers present when it binds, as the image's
  `density_based_coarse_locations` has it.
- **Proximity alerts** (`geofence.rs`): `GeofenceManager` over the fused
  provider.
- **Geocoding**: forwarded to the bound geocode provider.
- **Its inputs** (`env.rs`, `watch.rs`): permissions (the #497 nonce
  cache), app op modes (mirrored) and notes (made before the access they
  record), foreground state (a uid observer cut at
  `IMPORTANCE_FOREGROUND_SERVICE`), runtime permission and app op
  changes, settings read by their generation and observed through the
  bridge, platform compat changes (`DELIVER_HISTORICAL_LOCATIONS`,
  `BLOCK_PENDING_INTENT_SYSTEM_API_USAGE`, `LOW_POWER_EXCEPTIONS`).

**The bridge** (`LocationBridge.java`, `ILocationBridge`,
`ILocationHost`): what the original does inside system_server.

| What | Why system_server |
| --- | --- |
| binding `network`, `fused`, the geocoder and the population density provider (`ServiceWatcher`, `CurrentUserServiceSupplier`) | a service is bound by a process ActivityManager knows |
| settings observers (location mode, throttling, package denylist, DeviceConfig `location`) | `registerContentObserver` refuses a process ActivityManager does not know |
| SystemConfig's location allowlists | read by system_server from `/system/etc` |
| user lifecycle, user visibility, location power save mode, screen on/off, package resets | `SystemService` callbacks, `UserManagerInternal`, `PowerManagerInternal`, receivers |
| `LocationManagerInternal` (AppOpsPolicy's location source tags) | a `LocalServices` interface |
| the location packages of the default grants | `LegacyPermissionManagerInternal`, set before `PackageManagerService` grants |
| `LocationManager.invalidateLocalLocationEnabledCaches` | the nonce is in system_server's shared memory |

The default grants run in `PackageManagerService.systemReady`, before the
device's service starts; on a first boot or an upgrade the bridge runs
them again once the location packages are known.

Not here: `cmd location` and the original's full dump (#569); MSL
altitude, delivery wake locks and the emergency bypass (#570); the
hardware activity recognition and geofence proxies (#571); usage
statistics (#572).

**Cost** (2026-09-30, a Settings cold start twice after 3 minutes
settled, the same boot procedure, a binder trace; the driver's latency
of each synchronous call to `location`): PermissionController's burst at
the start is most of it.

| Call | Original: calls, mean | Native: calls, mean |
| --- | --- | --- |
| `isProviderPackage` (PermissionController) | 184, 84.9 us | 184, 52.9 us |
| `getExtraLocationControllerPackage` (PermissionController) | 175, 73.1 us | 175, 45.4 us |
| `isLocationEnabledForUser` (Settings) | 2, 37 us | 2, 50 us |
| all of `location` | 28.5 ms | 17.8 ms |

TotalTime of the two starts: 224 and 201 ms with the original, 233 and
187 ms native.

## Conformance

**Suite.** The official Android 16 CTS, `android-cts-16_r1-linux_x86-arm`
(the image is `BE2A.250530.026.F3`, security patch 2025-07-05), pinned in
`upstream/cts.lock`. The release zip is 18.6 GB; `tools/cts-module.py`
fetches only the pinned entries with HTTP range requests into `_build/cts`
and checks their sha256. The CTS is Apache-2.0 AOSP with its test
libraries under their own licenses; it is a local test input, never
shipped or committed.

**Run.** On a booted device (`cargo aim boot`), install
`CtsContentUriTestApp.apk` then `CtsContentTestCases.apk` (`pm install -r
-g [-t]`) and run the classes with `am instrument -w -r
--no-hidden-api-checks -e class ... android.content.cts/androidx.test.runner.AndroidJUnitRunner`.
`am instrument` runs on `app_process`, which needs init's class path;
the guest's shells (`aimctl shell`, `tools/guest-shell.sh`) inherit it
with the rest of init's environment (boot-status.md, "Debugging").

| Class (CtsContentTestCases) | Tests | Original | Native |
| --- | --- | --- | --- |
| ClipboardManagerTest | 15 | 15 pass | 15 pass |
| ClipboardManagerListenerTest | 1 | pass | pass |
| ClipboardAutoClearTest | 3 | 3 pass | 3 pass (2026-09-30, with DeviceConfig read, #428) |
| ClipDataTest | 11 | 11 pass | 11 pass |
| ClipDescriptionTest | 6 | 6 pass | 6 pass |

The original's numbers are from the same boot procedure with
`image/native-services` empty.

The vibrator: `CtsVibratorTestCases` (all 301 tests, hidden API checks
on, as its config asks), installed with `pm install -r -g -t` and run
with `am instrument -w -r android.os.vibrator.cts/androidx.test.runner.AndroidJUnitRunner`.
Against the original (`image/native-services` with the clipboard only)
and the native vibrator, each test ends the same: 268 pass and 33 skip
on assumptions of a vibrator (`hasVibrator`, `areVendorSessionsSupported`,
a vibrator id), among them the vendor session tests that need one; the
two that need none (`testDeviceWithoutVibrator_returnsUnsupportedStatus`,
`testVendorSessionsNotSupported_returnsUnsupportedStatus`) pass. The
same boots started Settings cold (`am start -W -S`: 315 and 249 ms) and
passed ClipboardManagerTest. The five classes take 3 min 10 s against
the original and 3 min 20 s against the native clipboard. One native run of ClipboardManagerTest was
cut short by a SurfaceFlinger hang in a task snapshot that took
system_server down (#436); the rerun passed.

Location: `CtsLocationFineTestCases`, `CtsLocationCoarseTestCases`,
`CtsLocationNoneTestCases`, `CtsLocationGnssTestCases` and
`CtsLocationPrivilegedTestCases` (hidden API checks on), installed with
`pm install -r -g -t` and run whole with `am instrument -w -r`, 400 s
per module. Against the original and the native service each test ends
the same: Fine 100 pass and 4 skip, Coarse 11 pass, None 112 pass and 2
skip, Gnss and Privileged as far as their hang. Both hang in the same
GNSS test (`testVariedRatesOnOff`, `testGnssMeasurementRegistration_enableFullTracking`)
and fail the same three GNSS assumptions: the host process has no
CoreLocation fix in these boots, so the gps provider reports none. Run alone,
`testGetLastKnownLocation_NoteOp` failed once natively and then passed
three times; it compares app op times in milliseconds with `>=`, and a
fast enough run ends its check in the millisecond of the access before.

**App checks** (native clipboard, device window):

- 2026-09-30 (the settings, URI check, spans and shell-source change):
  text copied on the Mac became the clip without SystemUI's overlay
  (`ClipboardListener: Clipboard overlay suppressed.`), pasted into
  Settings search (Ctrl+V) and, with two characters typed, copied back
  to the Mac; `dumpsys clipboard` exits 0; the access notification
  setting reads 1, is written 0 through the clipboard
  (`settings get secure clipboard_show_access_notifications`: 0) and
  reads 0 back.

- Settings search: text typed in the field and copied (Ctrl+A, Ctrl+C) is
  on the Mac's pasteboard (`pbpaste`); text copied on the Mac pastes into
  the field (Ctrl+V), and copying it back after typing one more character
  puts the combined text on the Mac.
- Chrome: text copied on the Mac pastes into the omnibox, and the
  omnibox's text copies back to the Mac.
- Calculator, Settings and Chrome start (cold, `am start -W -S`); SystemUI
  and Gboard, which listen to the clipboard, run without errors.
