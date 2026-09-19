# System services: current compatibility layer and AOSP migration

This inventory is the `SystemServiceFactory` service table as of 2026-09-19.
`DarwinSystemServer` publishes it through the partial `ServiceDirectory`
`IServiceManager` endpoint. Each listed endpoint is a project implementation
of a guest-facing Binder contract, **not** an upstream AOSP system service.
The framework client JAR is present, but the full AOSP `system_server` and
`PackageManagerService` are not running. AOSP Java source/JAR existence alone
does not make the service's boot, state, permission and Binder dependencies work.

| Published name | Current project endpoint | Intended Android owner / migration |
| --- | --- | --- |
| `package` | `pm.PackageManagerEndpoint` | Version-pinned PMS, package parsing/scanning, install state, permission and component queries. Retire `Installed*Info` legacy hint projection. |
| `activity` | `am.ActivityManagerEndpoint` | AOSP ActivityManagerService, process/service/broadcast state. |
| `activity_task` | `wm.ActivityTaskManagerEndpoint` | AOSP ActivityTaskManagerService, activity/task lifecycle and orientation policy. |
| `display` | `display.DisplayManagerEndpoint` | AOSP DisplayManagerService, logical display/device state and configuration. |
| `window` | `wm.WindowManagerEndpoint` | AOSP WindowManagerService, sessions, relayout, focus and surface transactions. |
| `user` | `user.UserManagerEndpoint` | AOSP UserManagerService and persistent user state. |
| `content` | `content.ContentServiceEndpoint` | AOSP ContentService; provider acquisition/lifecycle still belongs to AMS/ActivityThread. |
| `clipboard` | `content.ClipboardServiceEndpoint` | AOSP ClipboardService with a narrow macOS clipboard provider. |
| `notification` | `notification.NotificationManagerEndpoint` | AOSP NotificationManagerService with a host notification provider. |
| `input_method` | `inputmethod.InputMethodManagerEndpoint` | AOSP InputMethodManagerService with AppKit keyboard/IME events at the boundary. |
| `input` | `input.InputManagerEndpoint` | AOSP InputManagerService/InputDispatcher; AppKit supplies physical events. |
| `audio` | `audio.AudioServiceEndpoint` | AOSP AudioService policy with CoreAudio output/input provider. |
| `media.camera` | `camera.CameraServiceEndpoint` | Android camera service/API contract with AVFoundation capture provider. |
| `alarm` | `alarm.AlarmManagerEndpoint` | AOSP AlarmManagerService scheduling and wakeup semantics. |
| `appops` | `appops.AppOpsServiceEndpoint` | AOSP AppOpsService policy and persistence. |
| `shortcut` | `shortcut.ShortcutManagerEndpoint` | AOSP ShortcutService. |
| `mount` | `storage.StorageManagerEndpoint` | AOSP StorageManagerService/vold guest contract with filesystem provider. |
| `device_policy` | `admin.DevicePolicyManagerEndpoint` | AOSP DevicePolicyManagerService. |
| `power` | `power.PowerManagerEndpoint` | AOSP PowerManagerService; retain `DarwinPowerStateProvider` as host facts. |
| `thermalservice` | `power.ThermalServiceEndpoint` | AOSP ThermalManagerService; host thermal observations remain provider data. |
| `restrictions` | `restrictions.RestrictionsManagerEndpoint` | AOSP RestrictionsManagerService. |
| `trust` | `trust.TrustManagerEndpoint` | AOSP TrustManagerService and auth/trust policy. |
| `uimode` | `uimode.UiModeManagerEndpoint` | AOSP UiModeManagerService, configuration propagation. |
| `locale` | `locale.LocaleManagerEndpoint` | AOSP LocaleManagerService, per-app locale persistence/configuration. |
| `usagestats` | `usage.UsageStatsManagerEndpoint` | AOSP UsageStatsService and permission checks. |
| `jobscheduler` | `job.JobSchedulerEndpoint` | AOSP JobSchedulerService and system constraints. |
| `connectivity` | `connectivity.ConnectivityManagerEndpoint` | AOSP ConnectivityService policy; keep `NetworkPathProvider` for macOS path facts. |

The table also publishes three **Darwin-private** transport/control services:
`darwin.package_registry` (profile install ledger), `darwin.window_metadata`
(desktop metadata), and `darwin.desktop_root` (native root authority). They
should stay narrow and private; do not turn them into application-facing Android
policy. Nested endpoints include `WindowSessionEndpoint`,
`ActivityClientControllerEndpoint`, and `SettingsProviderEndpoint` (the latter
is a content provider, not a ServiceManager entry). `ServiceDirectory` itself
is a partial hand-coded `IServiceManager` publication/lookup implementation.
The list is a source inventory, not a claim of complete AOSP API semantics.

## Why the current package service exists

`ApplicationPackageManager` and its AOSP Java caller code already run from the
framework image. Their generated Binder calls need a matching system-owned
`IPackageManager` implementation and installed-package database. The current
runtime has neither a booted upstream PMS nor all of its dependencies, so
`PackageManagerEndpoint` answers a limited set of transactions from the sealed
profile registry and APK-inspector hints. On Blue Archive, `ActivityThread`
installed its declared `androidx.startup.InitializationProvider`, but
`ApplicationPackageManager.getProviderInfo()` initially failed because the
endpoint omitted that transaction. Adding the declared-provider query at the
PM owner lets the existing AOSP client and `ActivityThread` continue; it does
not mean PMS provider policy, parsing, permissions or process semantics are
complete. The same boundary now carries actual ABI split names so
`LoadedApk.registerAppInfoToArt()` can pair split code paths with profile names.

## Migration sequence and acceptance

1. Pin the AOSP release for each Binder interface and service owner. Generate
   AIDL transaction/parcel contracts rather than extending numeric dispatch
   ad hoc. Keep existing endpoints only as scoped, tested compatibility
   adapters while their owners are brought up.
2. Boot upstream service owners with real process, filesystem, permission and
   package state. Start with PMS/package parsing and ActivityManager/ActivityTask,
   then DisplayManager/WindowManager/Input; migrate dependent services in
   dependency order. Compare responses and lifecycle with unchanged APKs.
3. Move host facts behind small Darwin provider interfaces: filesystem/IPC,
   AppKit input and window backing, network path, power/thermal, clipboard,
   camera, audio and notifications. The Android service chooses policy and
   publishes configuration; macOS supplies or presents resources.
4. Remove each project Binder endpoint and its legacy launcher metadata only
   after production callers use the upstream owner, focused contract tests pass,
   and physical APK behavior is verified. Do not count DTO tests as lifecycle
   or UI acceptance.

The urgent display example is shared: Blue Archive declares
`userLandscape` (`screenOrientation=11`), which reaches `ActivityInfo`, but the
current host and Android display are fixed to portrait `360×640dp`. AppKit
resizing reallocates the IOSurface while Android ViewRoot/Unity and input keep
the old display geometry. AOSP ActivityTask/Display/Window owners must publish
one orientation/resize configuration and relayout transaction; the host applies
the resulting backing pixels and Retina scale. Reviving the retired fixture
`DarwinServiceBridge` orientation path would reintroduce production policy in
a probe and would not solve the lifecycle contract.
