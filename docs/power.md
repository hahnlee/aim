# Power

The design of ADR 0013's `power` step: moving `PowerManagerService` off
the original SystemServer, and mapping Android's power state to the
Mac's. Nothing is replaced, and implementation is paused (compatibility
first, performance later): what a native service needs from
system_server makes it a core-coupled service rather than a leaf, and
the Mac's part does not wait for it ("Order"). Sources are cited at
`android-16.0.0_r1`, the image's tag; counts of callers come from the
image's own `services.jar` dex.

## What apps call

The methods `power` (IPowerManager) was called with over two boots with
the original (the second boot of a data directory each, then a CTS run
and a Settings start; binder traces of ops checks m2next-1 and
m2uimode-2), and who called them:

| Method | Calls | Caller |
| --- | --- | --- |
| `acquireWakeLock`, `releaseWakeLock` | 255 + 253, 282 + 280 | Google Play services' uid, about 95 %; 2-6 each from five other apps |
| `isDeviceIdleMode`, `isLightDeviceIdleMode` | 65 + 61, 70 + 66 | Google Play services |
| `isDisplayInteractive` | 29, 30 | Google Play services 16-18, 1-4 each from other apps |
| `isLowPowerStandbyEnabled` | 25, 24 | Google Play services |
| `isPowerSaveMode`, `getPowerSaveState` | 16, 16 | Google Play services, apps, a system-uid app |
| `updateWakeLockWorkSource` | 6, 8 | Google Play services |
| `isAmbientDisplayAvailable`, `setDozeAfterScreenOff`, `getLastShutdownReason` | 6, 6 | other apps |
| `acquireWakeLockAsync`, `releaseWakeLockAsync`, `updateWakeLockUidsAsync` (one-way) | 3, 3 | audioserver |

Two client caches answer the rest in the app: `PowerManager.isInteractive` and
`isPowerSaveMode` are `PropertyInvalidatedCache`s whose nonces
(`is_interactive`, `is_power_save_mode`) live in system_server's
`ApplicationSharedMemory`.

**system_server is a client too.** 44 classes of system_server hold
`PowerManager.WakeLock`s (AlarmManager's delivery, JobScheduler,
DeviceIdle, Notifier, WindowManager's keep-screen-on lock, ...), 23 ask
`isInteractive`, 8 call `wakeUp`, `goToSleep` or `userActivity`, today
as local calls into the same process. With a native service each is a
binder call from system_server, possibly under a lock of its caller
("Risks").

## PowerManagerInternal

37 classes of system_server call 31 methods of `PowerManagerInternal`
(the outer classes of every `invoke` of it in `services.jar`, the power
package's own included):

| Users | Methods | Kind |
| --- | --- | --- |
| ActivityManagerService, OomAdjuster | `startUidChanges`, `updateUidProcState`, `uidActive`, `uidIdle`, `uidGone`, `finishUidChanges` | pushes, under ActivityManager's lock and `mProcLock`, on every process state change |
| DeviceIdleController | `setDeviceIdleMode`, `setLightDeviceIdleMode`, `setDeviceIdleWhitelist`, `setDeviceIdleTempWhitelist` | pushes |
| BatteryStats, AppStateTracker, DeviceIdle, NetworkPolicy, WindowManager, the location power save helper, vibration settings, ActivityManager, uimode (the bridge) | `registerLowPowerModeObserver`, `getLowPowerState(ServiceType)` | battery saver's per-service policy, read and observed |
| ActivityTaskManager, RootWindowContainer, DisplayPolicy, DisplayRotation, SurfaceAnimationRunner, GameManager, UserController, VoiceInteraction, USB, companion presence, BatterySaverController | `setPowerMode`, `setPowerBoost` | power HAL hints, one-way in effect |
| PhoneWindowManager, RootWindowContainer, DevicePolicy, DreamManager | `interceptPowerKeyDown`, `getLastWakeup`, `setUserInactiveOverrideFromWindowManager`, `setUserActivityTimeoutOverrideFromWindowManager`, `setMaximumScreenOffTimeoutFromDeviceAdmin`, `wasDeviceIdleFor`, `nap`, `setDozeOverrideFromDreamManager`, `setDevicePostured`, `isAmbientDisplaySuppressed`, `updateSettings` | policy inputs and queries |
| LowPowerStandbyController | `setLowPowerStandbyActive`, `setLowPowerStandbyAllowlist` | pushes |
| Notifier, PowerGroup, WakefulnessSessionObserver, vibration settings | `isInteractive(int)`, `wakefulnessToString`, `getLastGoToSleep` | statics and queries |

A forwarding `PowerManagerInternal` in system_server, as
`LocationBridge` publishes `LocationManagerInternal`, can serve them:

- **Pushes** become one-way calls on one binder object, which the driver
  delivers in order. They must not be synchronous: the uid pushes come
  under ActivityManager's locks, and the native service's effects (wake
  locks disabled for a cached uid, battery stats) come back into
  system_server.
- **Queries** are answered from a copy in the forwarder that the native
  service pushes on each change (each `ServiceType`'s `PowerSaveState`,
  interactive per display group, the last wake and sleep, ambient display
  suppression, the device idle times), and `LowPowerModeListener`s are
  called by the forwarder on its handler, as BatterySaverController
  calls them on its own.
- **Hints** (`setPowerMode`, `setPowerBoost`) go to the power HAL
  (`android.hardware.power.IPower/default`, the original's
  `PowerHalController`) from the native service.

**Prerequisite (#668).** The forwarder must exist at bootstrap.
SystemServer starts PowerManagerService in `startBootstrapServices`, and
the next step, `ActivityManagerService.initPowerManagement`, runs
`BatteryStatsService.initPowerManagement`, which calls
`registerLowPowerModeObserver` on it at once; WindowManagerService's
constructor caches it in `startOtherServices`. The device's own service
starts after `PHASE_SYSTEM_SERVICES_READY`, too late, and nothing
config-driven starts before `InitPowerManagement`. The native-services
edit turns a start into `nop`s in place; starting a class of
`aim-services.jar` in PowerManagerService's place needs a new string and
type in services.jar's dex, so a dex rewrite with re-indexed ids, a new
kind of SystemServer exception (the class itself would resolve: the
system server class loader holds the whole `SYSTEMSERVERCLASSPATH`; the
`oat` node would leave that one method's `const-class` unresolved at
compile time). Its stored result (`mPowerManagerService`) is never read
again in SystemServer.

## Display power and wakefulness

PowerManagerService drives the displays; DisplayManager does not power
them on its own.

- `systemReady` calls `DisplayManagerInternal.initPowerManagement(callbacks,
  handler, sensorManager)`; only then does DisplayManager create the
  `DisplayPowerController`s. Each `updatePowerStateLocked` calls
  `requestPowerState(groupId, DisplayPowerRequest, waitForNegativeProximity)`
  per power group and treats a `false` as not ready (suspend blockers
  held, wakefulness change not finished).
- The `DisplayPowerCallbacks` come back from DisplayManager's threads:
  `onStateChanged` (a new update), `onProximityPositive`/`Negative`,
  `onDisplayStateChange(allInactive, allOff)` (the power HAL's
  interactive mode and autosuspend, under DisplayManager's sync root)
  and `acquireSuspendBlocker`/`releaseSuspendBlocker`.
  `DisplayPowerRequest` is not a parcelable; the bridge would carry its
  fields. Screen-on blocking (`screenTurningOn`, the keyguard's first
  draw) is DisplayPowerController's with the window policy and stays
  original.
- `Notifier` tells the rest of system_server about wakefulness and wake
  locks: `ActivityManagerInternal.onWakefulnessChanged` (ActivityTaskManager
  sleeps and stops activities), `WindowManagerPolicy.startedWakingUp`/
  `finishedWakingUp`/`startedGoingToSleep`/`finishedGoingToSleep` (and
  their global forms), `InputManagerInternal.setDisplayInteractivities`,
  `InputMethodManagerInternal.setInteractive`, BatteryStats
  (`noteStartWakelock*`, `noteStopWakelock*`, `noteInteractive`,
  `noteWakeUp`, `noteUserActivity`), the app op `WAKE_LOCK`, the
  `SCREEN_ON`/`SCREEN_OFF` broadcasts, the charging animation and
  `TrustManager.setDeviceLockedForUser`. All of it is code in
  system_server: the bridge would run the original `Notifier`, which is
  public with a public constructor, as it would run
  `BatterySaverController`, `BatterySaverPolicy`,
  `BatterySaverStateMachine`, `LowPowerStandbyController` (which
  publishes `LowPowerStandbyControllerInternal`) and `ShutdownThread`.
- **Input user activity** reaches PowerManagerService through
  libandroid_servers' `android_server_PowerManagerService_userActivity`,
  which InputManager's native code calls and which calls
  `userActivityFromNative` on the object `nativeInit` registered. A
  native service has no such object: input would no longer reset
  Android's screen-off timeout. On the Mac that timeout is the Mac's to
  decide (#670), so a native service would hold Android awake while the
  Mac's display is awake, and keep the original's timeout for an
  explicit `goToSleep`, `wakeUp` and the CTS's key events.

So a native service would keep: wake lock bookkeeping and their disabled
states (uid states, device idle allowlists, low power standby), the
wakefulness state machine per power group, user activity and its
timeouts, the dream and doze decisions, the wake lock log, the dump and
`cmd power`. Every effect of those decisions lands in system_server
(displays, Notifier, battery saver, shutdown). The service's own logic
is about a third of the 21,000 lines of `com.android.server.power`
(thermal excluded); the rest stays Java either way.

## The Mac's part

None of the mappings needs a native PowerManagerService; each has an
owner of its own.

| Android | The Mac | Owner | Issue |
| --- | --- | --- | --- |
| Partial wake locks (PowerManagerService's `PowerManagerService.WakeLocks` suspend blocker, any process's `ISystemSuspend` wake lock) | an `IOPMAssertion` of type `PreventUserIdleSystemSleep`, held while any blocker is held | the system suspend service: a native `ISystemSuspend` in the service host, in `system_suspend`'s place (on a device, the kernel's autosuspend) | #669 |
| Wakefulness: asleep when the Mac's display sleeps or its session locks, awake when it wakes | NSWorkspace `screensDidSleep`/`screensDidWake`, the session lock | the device side: a lid or power key event through the input HAL, or the bridge's `goToSleep`/`wakeUp` with DEVICE_POWER | #670 |
| Screen wake locks, `FLAG_KEEP_SCREEN_ON` | an `IOPMAssertion` of type `PreventUserIdleDisplaySleep` | PowerManagerService's wake lock summary (native only), or the window shim of a visible keep-screen-on window | #671 |
| Battery saver | Low Power Mode (`NSProcessInfo.isLowPowerModeEnabled` and its notification), read only: Android's own choice holds until the Mac's mode changes, as for the appearance and the languages | the bridge, with `setPowerSaveModeEnabled` as Settings calls it | #672 |
| Thermal status | already native (`thermalservice`); its `SHUTDOWN` temperature calls `IPowerManager.shutdown`, which shuts down the guest, never the Mac | | |
| Battery and power source | the health HAL (IOPowerSources); a Mac without a battery is on mains | `vendor.health-aim` | |

Rules for the assertions: the service host holds at most one of each
type, named for the device; it is released when the last blocker goes
and when the host process exits (powerd drops a dead process's
assertions; to be checked with `pmset -g assertions` after a kill). The
Mac's power settings are never changed. The system-sleep assertion is
right only once Android's display sleeps with the Mac's (#670): while it
is on, PowerManagerService holds its display blocker and Android would
keep the Mac awake.

## Risks

What could break compatibility if `power` were native:

- **Boot order.** Without the forwarder at bootstrap (#668)
  system_server stops at `initPowerManagement` (a null
  `PowerManagerInternal`); with it, everything the forwarder answers
  before the native service and the bridge are attached must be what
  the original answers before `systemReady` (awake, no battery saver,
  no device idle).
- **Ordering and deadlocks.** The uid and device idle pushes come under
  ActivityManager's and DeviceIdle's locks, and the effects of the
  native service's decisions come back into system_server (Notifier,
  battery stats, app ops). Pushes must be one-way and in order, and no
  call back into system_server may wait while a caller's lock is held;
  a wake lock disabled or enabled late for a uid that changed state is
  a visible difference (CtsOsTestCases' wake lock tests, job and alarm
  tests).
- **Display power.** A `requestPowerState` answered late or a lost
  `onStateChanged` leaves the screen off, the keyguard's screen-on
  blocking waiting, or wakefulness changing forever; this is the path
  every sleep and wake of the CTS goes through.
- **Client caches.** The `is_interactive` and `is_power_save_mode`
  nonces must be bumped on every change through the bridge, or apps
  keep a stale `isInteractive`/`isPowerSaveMode`.
- **Lost inputs.** Input user activity comes only through
  libandroid_servers' hook on the original's object; dreams, doze and
  the proximity sensor's callbacks only through system_server; the
  power HAL's interactive mode and boosts through the original's
  `PowerHalController`. Each needs its own route.
- **Shutdown and reboot.** `shutdown`, `reboot` and `rebootSafeMode`
  must run `ShutdownThread` in system_server (broadcasts, ActivityManager
  and PackageManager shutdown, `sys.powerctl`); a native service that
  only stopped the guest would skip the apps' `ACTION_SHUTDOWN`.
- **The wake lock surface.** Wake lock tokens and their death, work
  sources, `IWakeLockCallback`, the `WAKE_LOCK` app op notes and battery
  stats' wake lock history are what apps and `dumpsys batterystats`
  see; GMS relies on all of them.
- **The Mac's part.** The system-sleep assertion before Android sleeps
  with the Mac's display would keep the Mac awake whenever Android runs
  (#669, #670); Android's sleep on the Mac's display sleep stops every
  activity, which window mode's shims must survive (#670).

## Conformance

The modules a native `power` must end as the original does:

- CtsOsTestCases: `PowerManagerTest` (4 tests: power save mode, dynamic
  power savings, discharge prediction), `PowerManager_WakeLockTest` (5:
  wake locks, their callbacks and timeout), `LowPowerStandbyTest` (26,
  which need `config_lowPowerStandbySupported`),
  `PowerManager_ThermalTest` (already run for thermal),
  `BatteryChangedBroadcastTest`.
- CtsBatterySavingTestCases (with its two target-API apps): battery
  saver's effects on apps, location and the network.
- CtsOsHostTestCases' power tests (`CtsHostPowerManagerTestApp`) are
  host-side (Tradefed) and are not run here.
- Affected through wakefulness: the keyguard, display and activity
  lifecycle tests of CtsWindowManagerDeviceKeyguard and CtsAppTestCases
  that sleep and wake the device (`input keyevent SLEEP`/`WAKEUP`), and
  CtsJobSchedulerTestCases and CtsAlarmManagerTestCases, which change
  device idle and battery saver.

## Order

1. **Now, with the original PowerManagerService**: the Mac's part
   (#669-#672), the display sleep (#670) before the system-sleep
   assertion (#669).
2. **The native service** needs, before any of its own logic, the
   bootstrap exception (#668), a forwarder for 31 methods called by 37
   classes, a bridge for DisplayManager's power requests and callbacks,
   and the original Notifier, battery saver, low power standby and
   shutdown run by the bridge. It is proposed for the core milestone,
   with DisplayManager and WindowManager, whose power requests and
   policy callbacks it mostly consists of, rather than as an M2 leaf;
   that is a decision on #668.
