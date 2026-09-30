# System services

The living record of ADR 0013's migration: which of Android's system
services run as native implementations on the Mac, how they are replaced,
what apps use, and how each replacement conforms. Keep it current with
every step: replace superseded facts instead of appending a log.

## Status

| Service | Implementation | Conformance (CTS 16_r1) | Since |
| --- | --- | --- | --- |
| `clipboard` (IClipboard) | native, `crates/aim-services`, backed by `NSPasteboard` | 35 of 36 tests pass (original: 36 of 36); app checks pass | M1, 2026-09-29 |
| `vibrator_manager` (IVibratorManagerService), `external_vibrator_service` | native, `crates/aim-services`: the original without a vibrator, as on a Mac | CtsVibratorTestCases: 268 of 301 pass, 33 skip (no vibrator), each test as the original | M2, 2026-09-30 |
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
   the tag). What a native service needs from system_server internals is
   #430.
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

Two inputs are asked each time. Permissions: their owner's listener,
`IPermissionManager.addOnPermissionsChangeListener`, is told of runtime
permissions only. Granting or revoking a development or role permission
(`pm grant`, the role holders') notifies nobody
(`onInstallPermissionGranted`, `PermissionManagerServiceImpl` at the
tag), and shell permission delegation and root's permission overrides
(`UiAutomation.adoptShellPermissionIdentity`, `addOverridePermissionState`,
`AccessCheckDelegate`) change answers without notice
(`testReadInBackgroundRequiresPermission` failed with a permission
mirror). What every change bumps is the owner's client-cache nonce
(`PackageManager.invalidatePackageInfoCache`), which apps' own
`checkPermission` cache follows; with
`pic_separate_permission_notifications` and `pic_uses_shared_memory` on
in this image it is the `package_info_cache` nonce in
`ApplicationSharedMemory`, which system_server hands only to app
processes (#497, #430). An instrumentation target's app ops: delegation
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

`vibrate` and `cancelVibrate` ask ActivityManager for VIBRATE each time
(permissions are not mirrored, "Mirrored state"); the rest asks nothing.

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
`am instrument` runs on `app_process`, which needs init's class path:
a debugging shell sources it first (`while read -r e n v; do [ "$e" =
export ] && export "$n=$v"; done < /data/system/environ/classpath`).

| Class (CtsContentTestCases) | Tests | Original | Native |
| --- | --- | --- | --- |
| ClipboardManagerTest | 15 | 15 pass | 15 pass |
| ClipboardManagerListenerTest | 1 | pass | pass |
| ClipboardAutoClearTest | 3 | 3 pass | 2 pass; `testAutoClearJob` fails (#428) |
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

**App checks** (native clipboard, device window):

- Settings search: text typed in the field and copied (Ctrl+A, Ctrl+C) is
  on the Mac's pasteboard (`pbpaste`); text copied on the Mac pastes into
  the field (Ctrl+V), and copying it back after typing one more character
  puts the combined text on the Mac.
- Chrome: text copied on the Mac pastes into the omnibox, and the
  omnibox's text copies back to the Mac.
- Calculator, Settings and Chrome start (cold, `am start -W -S`); SystemUI
  and Gboard, which listen to the clipboard, run without errors.
