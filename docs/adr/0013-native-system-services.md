# ADR 0013: Android's system services as native macOS implementations

Status: accepted 2026-09-29 (migration tracked in #153)

## Context

ADR 0012 runs the pinned Android userspace unmodified on a Linux syscall
layer. It works: the original SystemServer, SurfaceFlinger and apps run, and
compatibility comes from the original code. It also means every start boots
a whole Android system before the first app window:

- a boot runs ~70 services and ~25 one-shot programs, then SystemServer
  starts over a hundred Java services, mostly in order, then SystemUI, the
  launcher, the keyguard and the wallpaper; first boot completes after
  about 40 s on an M2 Pro;
- most of that serves an Android device shell (status bar, home screen,
  lock screen, recents) that a Mac already provides, and that window mode
  (#294) hides;
- snapshots or a resident guest can hide the cost but not remove it.

Wine is fast because it does not run Windows: it implements the Windows API
on the host and runs each program as one host process. The same is possible
for Android: an app's own process (ART, framework.jar, its native
libraries) talks to the system almost only through binder services
(ActivityManager, PackageManager, WindowManager, notification, clipboard,
input method, display, power, location, ...). If those services are native
implementations backed by macOS, no Android system has to boot.

The first stack (ADR 0012's context) tried a version of this and failed:
hand-written endpoints that mimicked AOSP owners by reflection, rebuilt and
patched ART and frameworks, by-name special cases in the loader. What
failed was how it was done: unversioned reflection, no conformance bar,
and everything replaced at once. Replacing services is still the right idea.

## Decision

Move Android's system services, one at a time, from the original
SystemServer to native implementations backed by macOS, until no Android
system has to boot. Keep an original component only where compatibility
truly requires it. The runtime stays working at every step: this is a
migration from the ADR 0012 stack, not a rewrite.

1. **What stays original.** The app's own process is untouched: ART and the
   boot image, framework.jar and the other boot classpath jars, bionic, the
   linker, the app's native libraries, all on the ADR 0012 syscall layer
   with the binder driver and the HALs. So is anything whose exact
   semantics apps depend on and that has no macOS owner, until a
   replacement passes its conformance bar (below).
2. **The replacement unit is a binder service.** A replacement implements
   the service's AIDL interface at the pinned image's version, with
   transaction codes generated from the pinned AIDL (never reflection), and
   is registered with servicemanager under the original name. The original
   service is then not started. Apps and the original framework code in
   their processes cannot tell the difference.
3. **Where a replacement lives.** It is Rust and runs on the host (in the
   binder host or its own process), using macOS for what macOS owns:
   NSWindow for windows and focus, the notification center, NSPasteboard,
   the Mac's input methods, displays, power and battery, location,
   networking, locale and appearance. It keeps whatever state Android
   persists (settings, package state) in Android's formats while the
   original still reads them.
4. **Conformance bar.** A service is replaced only when its replacement
   passes the CTS module(s) for that service's API, and the app checks
   (the integration gate, Settings, Chrome, Calculator and the tracked
   game set) still pass. Failing tests keep the original. The test results
   are recorded with the change.
5. **Order.** Largest cost and clearest macOS owner first:
   1. **The shell**: launcher, SystemUI (status and navigation bars,
      notification shade, quick settings, keyguard, recents, volume UI,
      toasts, and the window-management shell WMShell that window mode
      uses), wallpaper, boot completion without a home activity. macOS
      provides the shell; the task bridge (`aim-windows`) takes over task
      organization.
   2. **Services with a macOS owner**: notification (display and actions,
      #4), clipboard, input method (#23), display, power and battery,
      location, connectivity state, locale, time zone and appearance
      (#281-#283), audio policy, vibrator.
   3. **Services that start on demand**: SystemServer services that no
      replacement covers yet start when first requested instead of at boot
      (a SystemServer exception under ADR 0012's rules), until they are
      replaced.
   4. **Core**: PackageManager, ActivityManager/ActivityTaskManager,
      WindowManager. Last, and each only after its conformance bar.
   When the core is replaced, SystemServer no longer boots.
6. **What an app start costs at the end.** A resident host process (the
   binder host with the native services, like wineserver) and, per app,
   one process from a preloaded zygote snapshot: the target is a Wine-like
   start, about 1-2 s from a click to a window.

## Consequences

- ADR 0012's "unmodified, only below the guest" principle now applies to
  the app's process and to originals not yet replaced. System services move
  above the line: we own them.
- Android upgrades cost more than a new image pin: every replaced service
  tracks its AIDL and CTS at the new version. The generated transaction
  codes and the CTS bar make that work mechanical rather than guesswork.
- Google Play services and apps that inspect the system (device policy,
  accessibility, launchers) depend on many services at once. They are the
  compatibility risk and are tested at every step.
- The launcher and SystemUI are not in the image (since 2026-10-01, in
  both modes: device mode is a debugging view of the display, #463 D1).
- AGENTS.md changes with this ADR: replacing a system service at its binder
  interface, gated by CTS, is no longer an exception. Reflection, name
  interception and app-specific branches stay forbidden.

## Measurements

Each migration step records targeted measurements of what it changes (the
binder trace of the calls it serves, their latency before and after, a
smoke boot with a cold app start) and its CTS results, in
[system-services.md](../system-services.md) and the step's PR; no full
benchmark runs.

## Steps

The migration's state and conformance results are in
[system-services.md](../system-services.md).

### M1: the replacement pipeline, on clipboard (2026-09-29)

- **Generated AIDL.** The framework interfaces a native service serves or
  calls are generated from the pinned `.aidl` files
  (`crates/aim-services/sources.lock`) into `aim-service-aidl`; every
  transaction code is checked against the stubs in the image's
  `framework.jar`.
- **The service host.** Native services are a binder process of the
  driver inside guest-init (`aim_binder_host::local`), registered with
  servicemanager when it is ready.
- **The SystemServer exception.** SystemServer has no configuration to
  leave a service out, and rebuilding `services.jar` from source would
  take the platform build. The derived image's `services.jar` is the
  original with the one `startService(Foo.class)` call of each service in
  `image/native-services` turned into `nop`s in place: a verified,
  symbolic edit (the class, the call and its unused result are checked,
  so a changed SystemServer fails the build instead of being patched
  wrongly), recorded as a `replace` in `image/overlay.toml`. The jar's
  oat files name its entries by CRC; the `oat` node compiles them for the
  edited jar, as it compiles the image's other oat files with code (ADR
  0012, decision 4), with the original's `speed-profile` and its profile
  converted to the edited jar's checksums. Without them system_server
  verified services.jar at run time and a cold Settings start after boot
  timed out. It is the
  ADR 0012 exception for SystemServer that decision 5.3 anticipated.
- **The pilot.** `clipboard` on `NSPasteboard` passes 35 of CTS's 36
  clipboard tests (the original: 36); the missing one needs DeviceConfig
  (#428). What system_server-internal state native services need, and how
  their permission checks are made fast, are #430 and #432.

### Permissions, part 1: mirrored access state (2026-09-30)

- A native service's access decisions read state other services own.
  The service host mirrors what a focused app's access reads (package
  ownership, app-op modes, focus, the device lock), each fed by its
  owner's standard listener and dropped when the owner dies; permissions
  and the input method are still asked each time, because their changes
  are not all notified to a process outside system_server (#467, #430).
  The rule and the measurements are in
  [system-services.md](../system-services.md), "Mirrored state".
- Shell permission delegation decides an instrumentation target's app
  ops without notice, so a uid that an installed instrumentation targets
  has its modes asked each time (#467). Permissions stay asked: their
  listener misses development and role grants and the delegation, and
  the owner's complete signal, the client-cache nonce, lives in
  `ApplicationSharedMemory`, which only app processes receive (#497).

### M2: the vibrator (2026-09-30)

- `vibrator_manager` and `external_vibrator_service` are native: the
  original without a vibrator, which the Mac and the derived image do
  not have. CtsVibratorTestCases ends each test as the original does;
  what is not served yet is #479 and #480. Clipboard writes still ask
  for the user's profiles: no change notification reaches the service
  host (#460, #430).

### M2: location (2026-09-30)

- `location` is native: the original `LocationManagerService` and its
  provider managers, with gps from the Mac's CoreLocation in the service
  host and network, fused, the geocoder and the population density
  provider bound from the apps that serve them, as the original binds
  them, through the bridge (`LocationBridge`: provider binding, settings
  observers, SystemConfig's allowlists, user, power save, screen and
  package events, `LocationManagerInternal` for AppOpsPolicy, the default
  grants' location packages). Its five CTS modules end each test as the
  original. What is not served yet is #569-#572.

### M2: thermal (2026-10-01)

- `thermalservice` is native: the original `ThermalManagerService` with
  the Mac in the thermal HAL's place, read in the service host from the
  host module the HAL used (macOS's thermal state as the `SKIN` status,
  the CPU and battery temperatures, no thresholds and so no headroom).
  It is a leaf and needs nothing from the bridge. Its CTS
  (CtsThermalTestCases, `PowerManager_ThermalTest`) ends each test as
  the original, and `cmd thermalservice` answers as the original, served
  by a shell command reader every native service can use. The thermal
  HAL, left with no client, is no longer in the image (#624). Not served:
  the statsd atoms and the event log entry (#617). Chosen over `power`,
  which apps call most (GMS's wake locks) but whose local interface 37
  source files of system_server use, and `uimode`, whose configuration
  changes go through ActivityTaskManager's internals.

### M2: uimode (2026-10-01)

- `uimode` is native: the original `UiModeManagerService` with the
  Mac's appearance as the night mode (read, never changed) and night mode
  managed by the system (`isNightModeLocked`, requests answered as the
  original answers them under `config_lockDayNightMode`). Its
  configuration changes, an app's night mode, broadcasts, notification,
  dock apps, dreams and wake lock go through the bridge
  (`UiModeBridge`, `IUiModeBridge`, `IUiModeHost`; image additions only
  in the existing `aim-services.jar`). `UiModeManagerTest`
  (CtsAppTestCases) ends each test as the original
  (docs/system-services.md, "The uimode service").

### The system_server bridge (design, 2026-09-30)

- What native services need from system_server internals (URI grant
  owners, focus, content capture, virtual devices, `LocalServices`
  callers) and from being a process ActivityManager knows (content
  observers, receivers, `ApplicationSharedMemory`) comes through one
  device-specific system service of ours (`config_deviceSpecificSystemServices`),
  which hands the service host a binder of internal queries. A persistent
  proxy app, or a process record made for the host, was weighed and not
  chosen ([system-services.md](../system-services.md), "The
  system_server bridge"). Its image additions are exceptions to record
  when built (#520); each query goes when its owner moves native.

### The system_server bridge, part 1 (2026-09-30)

- **The exception.** The device's own system service
  (`dev.aim.server.DeviceServices`, `java/device-services`) enters the
  image as a device vendor's does, through three `image/overlay.toml`
  entries. `aim-services.jar` is added. The platform's
  `systemserverclasspath.pb` is replaced by one with that jar appended:
  only the system server class path holds classes SystemServer starts by
  name, and no other fragment is the device's. A static framework-res
  overlay in `/vendor/overlay` sets `config_deviceSpecificSystemServices`.
  On `SYSTEMSERVERCLASSPATH` the jar follows `services.jar` and precedes
  the APEX jars, so the `oat` node compiles it and compiles again the
  oat files whose class loader context names `services.jar`.
  SystemServer itself is unchanged.
- **The build.** Java, dex and APKs come from a pinned JDK and SDK build
  tools (`upstream/java-toolchain.lock`, licence records in
  THIRD_PARTY_NOTICES). The overlay is signed with AOSP's public test key
  (a user decision on #520): a preinstalled overlay needs a stable key,
  not the platform's. The code is compiled against stubs that are checked
  with every reference against the image's jars, so a changed internal
  API fails the build.
- **First slice (#497).** The service hands the native service host a
  bridge (`IBridge`, answering the system uid only). Its first method
  gives the read-only `ApplicationSharedMemory`, whose
  `package_info_cache` nonce keys the host's permission checks as it
  keys apps' own caches. Measurements and CTS are in
  [system-services.md](../system-services.md), "The system_server
  bridge".

### The Mac's languages (2026-10-01)

- The device's language list follows the Mac's preferred languages
  (#344): the service host hands it to the bridge
  (`IBridge.updateLocales`) at each attach and on each change of the
  Mac's list, and the bridge applies it with the platform's own
  `LocalePicker.updateLocales`, as Settings does. A choice made in
  Android holds until the Mac's list changes. Image additions only in
  the existing `aim-services.jar`; `LocaleManagerService` stays the
  original ([mac-settings.md](../mac-settings.md), "The language list").

### The notification permission, a temporary exception (2026-09-30)

- **The exception (#470).** An app's request for `POST_NOTIFICATIONS`
  shows only the Mac's prompt (a user decision on #470). Until
  PermissionController is native (the permissions milestone; #550), the
  device's system service registers a `PRODUCT_ORDERED_ID`
  `ActivityInterceptorCallback`, the vendor's hook for activity starts,
  that redirects a request for that permission alone, and
  PermissionPolicyService's request for a pre-33 app, to an invisible
  activity in a preinstalled APK (`java/notification-permission`, an
  `add` in `image/overlay.toml`). The activity asks the native service
  host through a request object only the redirected intent carries; the
  host has the app's shim ask the Mac, sets the permission as a device's
  settings do, and the activity returns PermissionController's result.
  Every other permission goes to PermissionController, and SystemServer
  and PermissionController are unchanged.
- **The one product interceptor (2026-10-01).** WindowManager takes one
  callback per id, and a product has one id, so the device registers one
  `PRODUCT_ORDERED_ID` callback (`ProductInterceptor`) that hands each
  start to its callbacks in order: the notification-permission redirect
  above, and in the lightweight shell the window shell's `LaunchModes`,
  which only notes the windowing mode a start's `ActivityOptions` ask for
  and never intercepts. On the shell's fullscreen display area
  WindowManager resolves an explicit fullscreen launch and one without a
  mode alike; the window shell makes only the latter freeform, as a
  freeform display area does ([task-organizer.md](../task-organizer.md),
  "Explicit windowing modes").
- **The mirror.** The Mac's per-app setting is mirrored into the
  permission when a shim starts and when its app becomes active: denied
  revokes, allowed (also provisional) grants. Shims no longer ask for
  authorization at start; an app that holds the permission without
  having asked gets provisional authorization at its first post. Details
  are in
  [notifications.md](../notifications.md), "The permission".

### Permissions, part 2: the design (2026-10-01)

- Permissions and app-op modes are one state on Android 16
  (`AccessCheckingService`, `access.abx`), and its two front ends are held
  by ActivityManager (`AppOpsService`) and created by PackageManager
  (`PermissionManagerService`); PermissionController must stay an app
  while those two are original. So neither AppOps nor PermissionManager
  is replaced alone: the access state is proposed to move with
  PackageManager (M4), the front ends with ActivityManager, and
  PermissionController after both (#616, a decision). The design, the
  measured traffic and the CTS modules are in
  [permissions.md](../permissions.md).

### M2: power, the design (2026-10-01)

- `power` is not replaced in M2. Its local interface must exist before
  ActivityManager's `initPowerManagement` at bootstrap, which the
  in-place SystemServer edit cannot provide (#668, a decision); its
  service drives DisplayManager's power controllers and runs Notifier,
  battery saver and shutdown inside system_server, under its callers'
  locks. Implementation is paused (compatibility first); it is proposed
  for the core milestone with DisplayManager and WindowManager. The Mac's
  part does not wait for it: Android's wake locks as an idle-sleep
  assertion at the system suspend service (#669), the Mac's display
  sleep as Android's sleep (#670), keep-screen-on as a display-sleep
  assertion (#671), battery saver from Low Power Mode (#672). The
  design, its compatibility risks and the CTS modules are in
  [power.md](../power.md).

### The shell: the lightweight shell is the image (2026-10-01)

- Decision 5.1 is done ([m1-shell.md](../m1-shell.md), #463): the derived
  image has no SystemUI, launcher, wallpaper pickers or wallet plugin, in
  either mode (image/overlay.toml, "The lightweight shell", `remove`
  entries with their reasons). In their place: an empty home of the
  device's own, its static wallpaper, a static overlay of framework-res,
  the native `IStatusBar` (toasts, the credential and pinning sheets,
  per-app menu bar items) and the window shell, a task organizer and
  transition player in the device service
  ([task-organizer.md](../task-organizer.md)).
- Checked before the switch, the shell against the image with SystemUI
  (window mode): the same results in CtsToastTestCases,
  CtsToastLegacyTestCases, CtsWidgetTestCases' ToastTest,
  CtsMediaProjection*, CtsWallpaperTestCases (but #650) and
  CtsNotificationTestCases' manager classes; CtsBiometricsTestCases
  fails the five tests that drive SystemUI's credential views (the Mac
  sheet answers); the notification module's bubble tests fail or hang
  without bubbles (#679) and its classes ran 2.5x slower (#680).
- Existing data directories upgrade at their first boot: PackageManager
  drops the removed packages, and the device service sets the default
  display area fullscreen again and drops the launch params recorded
  while it was freeform, before WindowManager reads them (#636).

### M4: PackageManager, the design (2026-10-02)

- PackageManager's largest surface is not binder: inside system_server,
  `PackageManagerInternal` (148 methods) is used by 111 files in 54
  subsystems, with `Computer` snapshots and `PackageManagerLocal` (ART
  Service, the permission state); SystemServer calls
  `PackageManagerService.main` in bootstrap and uses the result at eleven
  call sites, and PMS's injector builds UserManagerService and the
  permission front ends. Apps used 55 of `IPackageManager`'s 224 methods
  in a measured boot, idle, starts and installs, ten of them for 92 % of
  the calls.
- While ActivityManager, WindowManager and the permission front ends are
  original, a native owner serves them through a facade in
  system_server, a replica of its state kept by a version counter in
  shared memory; it needs a symbolic redirect of SystemServer's call
  sites, the edit #668 asks for `power`.
- M4 lands in three slices: A, the read model in shadow (compared with
  the original on every real call by a driver option, serving nothing);
  B, the write model in shadow (installs and changes computed without
  side effects and compared); C, the switch, on a branch, with the
  facade. The access state moves after C (#616). The design, the CTS
  modules (22 of 52 host-side, #701) and the decisions (#702) are in
  [m4-packagemanager.md](../m4-packagemanager.md).

### The redirect edit (2026-10-02)

- Decided by the user (#702 D1, for #668 too): a second kind of
  SystemServer exception, next to the `nop` edit. Named call sites of
  services.jar (`image/system-server-redirects`, with a reason each) call
  a static method of `aim-services.jar` instead, with the same registers
  and the receiver first; the dex is written again with the targets'
  method ids, re-indexed, every other byte's meaning unchanged. It is
  checked as the `nop` edit is (each site found as often as listed, the
  target a public static method of that signature) and verified as ART
  opens it, recorded on the `replace` of services.jar in
  `image/overlay.toml`. The list is empty until a user lands: M4's
  slice C (PackageManagerService's `main` and its eleven uses in
  SystemServer, UserManagerService's 15 calls) and a native `power`
  (#668, core milestone). Details in
  [system-services.md](../system-services.md), "Redirected call sites".
