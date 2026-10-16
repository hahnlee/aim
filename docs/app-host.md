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
| init, servicemanager, SurfaceFlinger, the HALs, zygote | the guest, original | zygote starts without `--start-system-server` |
| `aim-app-host-agent` | the guest, `daemons/app-host-agent` | what the host's services need a guest process of the system uid for: zygote takes commands only from the system uid (`ZygoteConnection`), so it relays the app host's process starts; window input channels are guest sockets, so it makes them, and reads the touchscreen and publishes its motions on the focused window's channel, as InputReader and InputDispatcher do (`crates/aim-input-transport`) |
| `activity` | the host, `crates/aim-services/src/app_host` | starts an app's process as `ProcessList.startProcessLocked` does, takes its `attachApplication`, sends `bindApplication` and `setProcessState`, answers `checkPermission[ForDevice]` and `finishAttachApplication` |
| `package` | the host, same | the native PackageManager queries (`package::query`, `package::resolve`) over the packages the app host joins from `packages.xml`, `packages.list`, the permission state and each package's parse |
| `activity_task`, the activity client controller | the host, same | launches the activity the process starts for (`LaunchActivityItem`, `ResumeActivityItem`, `TopResumedActivityChangeItem`) once its application is created, and takes its reports |
| `display` | the host, same | the built-in display SurfaceFlinger reports, as `DisplayInfo` |
| `window`, its sessions, `IWindowId` | the host, same | adds an app's window as `WindowManagerService.addWindow` does (its input channel from the agent, its insets) and relays it out as a SurfaceFlinger layer of buffers at the display's root, the whole display, with no system bars |
| `permission` | the host, same | `IPermissionController`, which SurfaceFlinger's permission checks wait for |
| `input`, `audio` | the host, same | what a touch and a click ask: the velocity tracker's strategy; whether navigation repeats sound, and the click's sound effect while sound effects are off (#1433) |
| `user`, `appops`, `trust`, `input_method` | the host, same | what an app's start asks of them: the one user's state and restrictions, the clipboard's ops, an unlocked device, the input method's client (no input method yet, #1432) |
| the `settings` provider | the host, same | `GET_*` and `LIST_config` from the settings the original persisted |
| `aim.app_host` | the host, same | `startActivity(String component)` (code 1, `package/.Class`) for the shell |

The start arguments, the configuration and the compat changes follow the
pinned sources (`android-16.0.0_r1`); their tests cite them. They were
checked against what the original system_server sent Calculator, captured
with `guest-init --binder-capture` (below): the global configuration,
`CompatibilityInfo`, the core settings and both compat change lists
match, and the app host's `DisplayInfo` is the original's byte for byte.

## Running it

```
cargo aim build
tools/app-host-image.sh                 # target/aim/app-host/root
guest-init --image target/aim/app-host/root --data <data> --run \
    --only apexd-bootstrap,apexd,keystore2,odsign,vendor.keymint-default,prng_seeder,logd,servicemanager,hwservicemanager,derive_classpath,derive_sdk,art_boot,zygote,surfaceflinger,vendor.graphics.allocator,vendor.hwcomposer-3,gpu,system_aconfigd_platform_init,mainline_aconfigd_init,system_aconfigd_mainline_init,system_aconfigd_socket_service,mainline_aconfigd_socket_service,vendor.power-default,aim-app-host-agent \
    [--gpu ... --vulkan ... --display ...]
tools/guest-shell.sh <data> 'service call aim.app_host 1 s16 com.android.calculator2/.Calculator'
```

`<data>` must be one Android booted once with its original image (the
app host reads the package and settings state it persisted), with the app
installed. `--only` leaves out what an app's start does not need; apexd,
keystore2 and odsign stay, as properties init waits for are theirs;
the power HAL stays, as SurfaceFlinger waits for it at its first frame.

`guest-init --binder-trace FILE --binder-capture DESCRIPTOR#CODE,...`
keeps the whole data of the named calls, and of their replies, in
`FILE.data/`.

## Measured (2026-10-16, M2 Pro, second boot of a data image)

| Step | Time |
| --- | --- |
| guest start to zygote accepting commands, the services above | 4.8 s (the original image's boot without system_server: 7.2-8.0 s) |
| of it: zygote's preload | 1.8 s |
| first `startProcess`: the app host loads its packages (244 parsed) | 4.1 s |
| `startProcess` to zygote's pid | 119 ms |
| to `attachApplication` | 162 ms |
| to `finishAttachApplication` (the application created) | 414-428 ms |
| to `activityResumed` | 438-537 ms |
| to `activityIdle` (its first frame drawn) | 484-582 ms |

`aim-display --capture` shows Calculator's whole window on the display;
taps on 7, +, 8 and = on aim-display's touchscreen show 15.

## Not yet

More than one activity and window, focus changes and accessibility
(#1422); keys, the mouse and the rest of input dispatch (#1434); sound
effects (#1433); a finished draw's transaction (#1431); the input
method (#1432); settings generation tracking (#1430); external
storage (#1423),
providers and the font map in `bindApplication` (#1424), PackageManager's
writes (#1425), the parse at the first start (#1426) and system apps'
hidden API allowlist (#1427).
