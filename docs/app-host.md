# The app host

ADR 0013's end state is an app started without booting Android: native
services on the host answer what an app's process asks of system_server.
The app host is its first slice (L1): Calculator's start, its first
frame on the display and its touch input, with no system_server process
at all. It runs on its own image; the default image
and boot are unchanged.

## What runs

| Piece | Where | What it does |
| --- | --- | --- |
| init, servicemanager, SurfaceFlinger, audioserver, the HALs, zygote | the guest, original | zygote starts without `--start-system-server` |
| `aim-app-host-agent` | the guest, `daemons/app-host-agent` | what the host's services need a guest process of the system uid for: it mounts user 0's emulated storage through vold and serves its FUSE connection, as MediaProvider's FuseDaemon does, by a process as MediaProvider's uid and FUSE-daemon groups (`crates/aim-emulated-storage`: the operations on `/data/media`, each user to its own directory); zygote takes commands only from the system uid (`ZygoteConnection`), so it relays the app host's process starts; window input channels are guest sockets, so it makes them, and reads the touchscreen and publishes its motions on the channel of the front window under a touch, as InputReader and InputDispatcher do (`crates/aim-input-transport`); it kills the process of a removed task; it plays the sound effects, each file decoded once through the media services (NdkMediaExtractor, NdkMediaCodec) and played as a sonification stream (AAudio), at most four at once, as SoundPool plays them |
| `activity` | the host, `crates/aim-services/src/app_host` | starts an app's process as `ProcessList.startProcessLocked` does, takes its `attachApplication`, sends `bindApplication` (with the app's own content providers, `generateApplicationProvidersLocked`) and `setProcessState`, answers `checkPermission[ForDevice]` and `finishAttachApplication`, takes `publishContentProviders` and serves a published provider to its own uid (another app's: #1450); keeps the receivers apps register and sticky broadcasts, and delivers a broadcast to the registered receivers it matches as BroadcastController and BroadcastSkipPolicy do (`scheduleRegisteredReceiver`; an ordered one receiver by receiver through `finishReceiver`; manifest receivers: #1462; broadcast app ops: #1463; the system's broadcasts: #1464); answers `getRunningAppProcesses` and `getMyMemoryState` for its processes, each top |
| `package` | the host, same | the native PackageManager queries (`package::query`, `package::resolve`) over the packages the app host joins from `packages.xml`, `packages.list`, the permission state and each package's parse |
| `activity_task`, the activity client controller | the host, same | launches the activity the process starts for (`LaunchActivityItem`, `ResumeActivityItem`, `TopResumedActivityChangeItem`) once its application is created, and takes its reports; an app's own `startActivity` of its activities (`app_host/activity_start.rs`: resolved as `ActivityStarter` resolves them, with their launch modes and flags, results and new intents) and `finishActivity`, in its task's stack of activities as `TaskFragment.resumeTopActivity` moves them (`app_host/activity_stack.rs`: the top paused before the next is launched or resumed, the rest stopped, the finished destroyed); activities in tasks of their own, launch options and the resolver: #1485; a task to the back: #1486 |
| `display` | the host, same | the built-in display SurfaceFlinger reports, as `DisplayInfo` |
| `window`, its sessions, `IWindowId` | the host, same | adds an app's window as `WindowManagerService.addWindow` does (its input channel from the agent, its insets) and relays it out as `WindowLayout.computeFrames` places it on a display with no system bars (`app_host/window_layout.rs`): an activity's window the whole display, or in window mode its task ([windows.md](windows.md), "The app host"); a popup, a menu or a dialog at its position and size, a sub-window in its parent's frame. Each window is a SurfaceFlinger container layer at its frame (less its surface insets), its buffers' layer a child of it, as `WindowState` and `WindowSurfaceController` make them; the agent takes its touches in the region `WindowState` gives it (a modal window's its task's), and tells windows that watch outside touches of them |
| `permission` | the host, same | `IPermissionController`, which SurfaceFlinger's permission checks wait for |
| `permissionmgr` | the host, same | PermissionManagerService's `IPermissionManager` for apps (`app_host/permission_manager.rs`), as the permission service (`PermissionService`) answers from the state it persisted (`access.abx`): `checkPermission` and `checkUidPermission` (the app id's flags through shared uids, the fuller permissions, instant apps holding only `instant` ones, SystemConfig's grants to uids of no package), `getPermissionInfo` (the declaring package's parse, the protection before O), `getPermissionGroupInfo`, `getAllPermissionGroups` (the groups as `AppIdPermissionPolicy` adds them) and `queryPermissionsByGroup`, each as the caller may see the declaring package; `getPermissionFlags`, `isPermissionRevokedByPolicy`, `shouldShowRequestPermissionRationale` and `getAllowlistedRestrictedPermissions` with their checks, `getSplitPermissions`, and listeners (kept: nothing changes the state); every device id the default device's, as without a VirtualDeviceManager. Grants, revocations, flag and allowlist changes, another device's permissions, one-time sessions, auto-revoke, attribution sources, the permission states and `permission_checker`: #1508 |
| `mount` | the host, same | StorageManagerService's side of external storage: it connects vold (listener, reset, user 0's start), has the agent mount the emulated volume, and answers `getVolumeList` and `mkdirs` (vold's `setupAppDir`); processes start with the default external mount and app data isolation (`--pkg-data-info-map`, `--bind-mount-data-dirs`) as `ProcessList` starts them |
| `input`, `audio` | the host, same | what a touch and a click ask: the velocity tracker's strategy, and the input devices (InputReader's virtual keyboard, whose key character map apps load: `Virtual.kcm` as `KeyCharacterMap` parses and writes it, `app_host/keychars.rs`; the touchscreen, keyboard and mouse: #1434); AudioService's stream volumes (`app_host/volume.rs`): ranges, defaults and aliases as AudioService decides them for the image, the indexes it persisted, mute, `getStreamVolume` and its kin, `setStreamVolume` and `adjustStreamVolume` with its checks and app ops, `VOLUME_CHANGED_ACTION`; audioserver initialized with them (`initStreamVolume`, `setStreamVolumeIndex`, `systemReady`, `app_host/audio_policy.rs`); the speaker as the device of absolute volume guest-init's volume bridge makes it, its dispatcher told of each change; not persisted (#1474), the ringer and zen modes read only (#1475), the players and recorders apps make (the playback monitor: ids from AudioFlinger, their events and audioserver's port events, the playback callbacks, anonymized for the public; recording configurations: #1480); the sound effects (`SoundEffectsHelper`): each effect's file as `audio_assets.xml` names it, played by the agent at `config_soundEffectVolumeDb` while sound effects are on and the system stream is not muted, with whether navigation repeat and home sound |
| `sensor_privacy` | the host, same | what audioserver's policy and apps read: whether the microphone and camera toggles are supported, and their states as SensorPrivacyService persisted them; nothing changes them (#1476) |
| `user`, `appops`, `trust`, `input_method` | the host, same | what an app's start asks of them: the one user's state (unlocked, a full user, no profiles) and restrictions, the clipboard's and stream volumes' ops (with `no_adjust_volume`), an unlocked device, and the input method: the Mac's, whose text goes to the app's input connection ([input.md](input.md), "The app host: the Mac's input method") |
| the `settings` provider | the host, same | `GET_*` and `LIST_config` from the settings the original persisted |
| `content` | the host, same | `ContentService`'s observers (`ObserverNode`'s tree: a change reaches its URI's observers and the observers of its ancestors that take descendants), `notifyChange` after `checkContentProviderAccess` as the provider's permissions allow, and the content cache; sync adapters: #1460 |
| `notification` | the host, same | NotificationManagerService for apps: channels and groups as `PreferencesHelper` keeps them, read from and written to `notification_policy.xml` (`app_host/notification_channels.rs`); `enqueueNotificationWithTag` with `fixNotification`'s changes made in the notification's parcel (flags, the allowlist token, the extras only the system sets, null actions, a full-screen intent the app may not use), the channel's and the POST_NOTIFICATIONS checks, the per-app count and rate limits; cancels, group summaries with their children, `getAppActiveNotifications`, `areNotificationsEnabled`; the system's listeners (`registerListener`, ranking updates in shared memory), so the Mac's notification bridge shows apps' notifications as it showed the original's; vibration effects without a pattern: #1487; foreground services, CallStyle, zen beyond the manual rule, ranking, toasts, app listeners, delegates and clicks: #1488 |
| `power` | the host, same | PowerManagerService for apps: wake locks with their permissions, levels, death and callbacks, and the suspend blockers the original holds in `system_suspend` (`PowerManagerService.WakeLocks` while one keeps the CPU on, `.Display` while the display is on); the device awake and interactive (the app host's display is always on), never idle; battery saver from the persisted `low_power`, its policies not run (#1489) |
| services (`activity`'s) | the host, same | apps' services as `ActiveServices` runs them (`app_host/services.rs`): `startService`, `stopService` and `stopSelf`, `bindService` and `unbindService` with their checks (exported, permission), the service created in its app's process (started for it when it does not run, created once it attaches) with `scheduleCreateService`, its starts with `scheduleServiceArgs`, its bindings with `scheduleBindService` and the binder it publishes passed to its connections, the start results (sticky, not sticky, redelivered), the service destroyed once neither started nor auto-created by a binding, its clients told when its process dies, and a service still needed started again a second later; services in processes of their own, isolated, external and foreground services: #1492 |
| PendingIntents (`activity`'s) | the host, same | `getIntentSender(WithFeature)` as `PendingIntentController` keeps them (`app_host/pending_intents.rs`): a record by its key, made, updated, cancelled or returned as `FLAG_NO_CREATE`, `FLAG_CANCEL_CURRENT` and `FLAG_UPDATE_CURRENT` ask, with U+'s refusal of mutable implicit ones; `sendIntentSender` and `IIntentSender.send` send it as its creator (an activity started in its package, a broadcast, a service), the sender's intent filled in (`Intent.fillIn` over the parcels, `app_host/kept.rs`) unless it is immutable, once if one-shot, its finished receiver told; `cancelIntentSender` with its cancel listeners, `getInfoForIntentSender`, `getIntentForIntentSender`; activity results and the creator's activity options: #1493 |
| `alarm` | the host, same | AlarmManagerService for apps (`app_host/alarm_service.rs`): `set` with its checks (the caller's package, exact alarms' `SCHEDULE_EXACT_ALARM`/`USE_EXACT_ALARM` and their compat changes, prioritized alarms, no repeating listener, 500 per uid), the minimum futurity and clamped intervals; each alarm delivered at the start of its window on the Mac's clocks, through its PendingIntent (with `EXTRA_ALARM_COUNT`) or its `IAlarmListener`, repeating ones rescheduled; `remove`, `removeAll`, alarm clocks with `getNextAlarmClock` and `ACTION_NEXT_ALARM_CLOCK_CHANGED`, `canScheduleExactAlarms`, `hasScheduleExactAlarm`; `ACTION_TIME_TICK` each minute and `ACTION_DATE_CHANGED` at local midnight; the Mac's clock changes, `setTime`, batching and standby: #1501 |
| `batteryproperties` | the host, same | BatteryService for apps (`app_host/battery_service.rs`): the health HAL's client (`vendor.health-aim`, over the Mac's battery) as `HealthServiceWrapperAidl` is, its `IHealthInfoCallback` registered and an update asked, again when a restarted HAL registers; each health info processed as `processValuesLocked` processes it: the plug type, low battery at `config_lowBatteryWarningLevel` (or the `low_power_trigger_level` setting) until `config_lowBatteryCloseWarningBump` above it, the sticky `ACTION_BATTERY_CHANGED` with every extra (rate limited when the original's flag is on), `ACTION_POWER_CONNECTED`/`DISCONNECTED` and `ACTION_BATTERY_LOW`/`OKAY` to every user; `getProperty` from the HAL with its permissions, and `scheduleUpdate`. Battery stats (and with them `BatteryManager.isCharging`), `ACTION_BATTERY_LEVEL_CHANGED`, the shutdown on a critical or hot battery, the LED and the `battery` service (a dump and shell commands) are not run |
| `account` | the host, same | AccountManagerService for apps (`app_host/account_service.rs`): each user's authenticators as `AccountAuthenticatorCache` finds them (`app_host/account_authenticators.rs`: the services for `android.accounts.AccountAuthenticator` and the `<account-authenticator>` their meta-data names, a type kept by the uid that first registered it, as `registered_services` persists it), `getAuthenticatorTypes`; the accounts in the original's databases (`accounts_de.db`, with `accounts_ce.db` attached while the user is unlocked), read and written through SQLite (`app_host/accounts_db.rs`), opened as `getUserAccounts` opens them (old grants purged, the authenticators' uids kept in `meta`); `getAccountsAsUser`, `getAccountsForPackage`, `getAccountsByTypeForPackage`, `getAccountsAndVisibilityForPackage`, `getAccountVisibility` and `getPackagesAndVisibilityForAccount` with the original's visibility (the authenticator's signers, stored and legacy values, GET_ACCOUNTS before O, READ_CONTACTS, GET_ACCOUNTS_PRIVILEGED), `getPassword`, `getUserData`, `peekAuthToken` and `getPreviousName` for the authenticator's apps, `someUserHasAccount`, `hasAccountAccess`, `onAccountAccessed`, `registerAccountListener` and `unregisterAccountListener`; adding, removing and changing accounts, the authenticator sessions (`getAuthToken`, `addAccount`, ...), the broadcasts of a change, access grants and a device or profile owner: #1504 |
| `connectivity` | the host, same | ConnectivityService for apps (`app_host/connectivity_service.rs`, its parcelables in `app_host/network.rs`): one network, the Mac's, which guest sockets really use (they are host sockets), described as the original describes the validated Ethernet network of a boot ([network.md](network.md)): TRANSPORT_ETHERNET with INTERNET, NOT_RESTRICTED, TRUSTED, NOT_VPN, VALIDATED, NOT_ROAMING, FOREGROUND, NOT_CONGESTED, NOT_SUSPENDED, NOT_VCN_MANAGED, NOT_METERED and NOT_BANDWIDTH_CONSTRAINED, `eth0`'s specifier, the Ethernet agent's `NetworkInfo`, and the link properties IpClient makes of the virtual router's lease (the Mac's address, routes, DNS servers and MTU, `crates/aim-uplink`); netId 100 and up, none while the Mac has no network, a new one when the Mac moves to another, new link properties when only its DNS servers change; `getActiveNetwork[ForUid]`, `getActiveNetworkInfo[ForUid]`, `getNetworkInfo[ForUid]`, `getAllNetworks`, `getNetworkCapabilities` (redacted for the caller), `getLinkProperties`, `isActiveNetworkMetered`, `getGlobalProxy` and `getProxyForNetwork` (none), `getMultipathPreference`, `getRestrictBackgroundStatusByCaller` (from the persisted `netpolicy.xml`), `getEnabledConnectivityManagerFeatures`; `requestNetwork` (requests, TRACK_DEFAULT, LISTEN_FOR_BEST), `listenForNetwork` and `releaseNetworkRequest` with the original's checks and per-uid limits, their callbacks through the request's `Messenger` (AVAILABLE with capabilities and link properties, LOST, IP_CHANGED, UNAVAIL on a timeout) to the methods the callback declares, released when their binder dies. Apps are all top, so no firewall chain or data saver blocks them; an app without INTERNET is blocked. PendingIntent requests, reservations, agents and offers, VPNs, keepalives, the boot's test hook, and netd and DnsResolver behind the netId: #1509 |
| `statusbar` | the host, same | StatusBarManagerService's `registerStatusBar` (guest-init's lightweight shell bar registers and hears the display's default state) and `onNotificationClick`, which cancels a clicked auto-cancel notification |
| `aim.app_host` | the host, same | `startActivity(String)` (code 1: a component, `package/.Class`, or a package, whose launch activity it resolves) for the shell and `aimctl open` |

The start arguments, the configuration and the compat changes follow the
pinned sources (`android-16.0.0_r1`); their tests cite them. They were
checked against what the original system_server sent Calculator, captured
with `guest-init --binder-capture` (below): the global configuration,
`CompatibilityInfo`, the core settings and both compat change lists
match, and the app host's `DisplayInfo` is the original's byte for byte.

## Running it

`aimctl --data DIR start [--windows] --image target/aim/app-host/root`
keeps it resident and `aimctl open PACKAGE` starts an app
([aimctl.md](aimctl.md), "The app host"); with `--windows` each app is a
Mac window of its own. By hand:

```
cargo aim build
tools/app-host-image.sh                 # target/aim/app-host/root
guest-init --image target/aim/app-host/root --data <data> --run \
    --only "$(grep -v '^#' image/app-host/services | paste -sd, -)" \
    [--gpu ... --vulkan ... --display ...]
tools/guest-shell.sh <data> 'service call aim.app_host 1 s16 com.android.calculator2/.Calculator'
```

`startActivity` takes a component (`package/.Class`) or a package, whose
launch activity it starts. `<data>` must be one Android booted once with its original image (the
app host reads the package and settings state it persisted), with the app
installed. `image/app-host/services` (in the image as
`/system/etc/aim/app-host-services`) leaves out what an app's start does
not need, and says why the rest stays. The agent waits for zygote to
listen, as `ZygoteProcess.waitForConnectionToZygote` does.

`guest-init --binder-trace FILE --binder-capture DESCRIPTOR#CODE,...`
keeps the whole data of the named calls, and of their replies, in
`FILE.data/`.

## Measured (2026-10-16, M2 Pro, second boot of a data image)

| Step | Time |
| --- | --- |
| guest start to zygote accepting commands, the services above | 4.2-4.3 s (the original image's boot without system_server: 7.2-8.0 s) |
| of it: zygote's preload | 1.0-1.05 s |
| the app host loads its packages (244 parsed in parallel), while the boot goes on, from the agent's attach | 1.1-1.2 s |
| `startProcess` to zygote's pid | 119-143 ms |
| to `attachApplication` | 162 ms |
| to `finishAttachApplication` (the application created) | 414-428 ms |
| to `activityResumed` | 438-537 ms |
| to `activityIdle` (its first frame drawn) | 355-370 ms |

`aim-display --capture` shows Calculator's whole window on the display;
taps on 7, +, 8 and = on aim-display's touchscreen show 15. In window
mode Calculator opens in its own window, 824×1464 pixels (412×732 dp),
from its shim; it takes taps, moves and resizes with its window (made
landscape it relaunches into its landscape layout, with the advanced
panel), and closing the window ends its process; opened again, it is
idle 0.33 s after its process's start.

## Not yet

Focus changes within an app and accessibility (#1422); keys, the mouse
and the rest of input dispatch (#1434); a finished draw's transaction (#1431); the input
method's candidates at the caret (#1458); settings generation tracking (#1430); scoped storage's
per-file ownership and the media database (#1455); the font map in
`bindApplication`, to come from the Mac's fonts (#1449),
PackageManager's writes (#1425), PermissionManager's writes (#1508), sync adapters (#1460), and other apps' content providers, which Clock's timer needs next (#1450).
