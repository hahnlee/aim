# The app host

ADR 0013's end state is an app started without booting Android: native
services on the host answer what an app's process asks of system_server.
The app host is its first slice (L1): Calculator's start, with no
system_server process at all. It runs on its own image; the default image
and boot are unchanged.

## What runs

| Piece | Where | What it does |
| --- | --- | --- |
| init, servicemanager, SurfaceFlinger, the HALs, zygote | the guest, original | zygote starts without `--start-system-server` |
| `aim-zygote-relay` | the guest, `daemons/zygote-relay` | zygote takes commands only from the system uid (`ZygoteConnection`); this system daemon sends the app host's to it |
| `activity` | the host, `crates/aim-services/src/app_host` | starts an app's process as `ProcessList.startProcessLocked` does, takes its `attachApplication`, sends `bindApplication` and `setProcessState`, answers `checkPermission[ForDevice]` and `finishAttachApplication` |
| `package` | the host, same | the native PackageManager queries (`package::query`, `package::resolve`) over the packages the app host joins from `packages.xml`, `packages.list`, the permission state and each package's parse |
| `aim.app_host` | the host, same | `startProcess(String package)` (code 1) for the shell |

The start arguments, the configuration and the compat changes follow the
pinned sources (`android-16.0.0_r1`); their tests cite them. They were
checked against what the original system_server sent Calculator, captured
with `guest-init --binder-capture` (below): the global configuration,
`CompatibilityInfo`, the core settings and both compat change lists match.

## Running it

```
cargo aim build
tools/app-host-image.sh                 # target/aim/app-host/root
guest-init --image target/aim/app-host/root --data <data> --run \
    --only apexd-bootstrap,apexd,keystore2,odsign,vendor.keymint-default,prng_seeder,logd,servicemanager,hwservicemanager,derive_classpath,derive_sdk,art_boot,zygote,surfaceflinger,vendor.graphics.allocator,vendor.hwcomposer-3,gpu,system_aconfigd_platform_init,mainline_aconfigd_init,system_aconfigd_mainline_init,system_aconfigd_socket_service,mainline_aconfigd_socket_service,aim-zygote-relay \
    [--gpu ... --vulkan ... --display ...]
tools/guest-shell.sh <data> 'service call aim.app_host 1 s16 com.android.calculator2'
```

`<data>` must be one Android booted once with its original image (the
app host reads the package and settings state it persisted), with the app
installed. `--only` leaves out what an app's start does not need; apexd,
keystore2 and odsign stay, as properties init waits for are theirs.

`guest-init --binder-trace FILE --binder-capture DESCRIPTOR#CODE,...`
keeps the whole data of the named calls in `FILE.data/`.

## Measured (2026-10-16, M2 Pro, second boot of a data image)

| Step | Time |
| --- | --- |
| guest start to zygote accepting commands, the services above | 4.8 s (the original image's boot without system_server: 7.2-8.0 s) |
| of it: zygote's preload | 1.8 s |
| first `startProcess`: the app host loads its packages (244 parsed) | 4.1 s |
| `startProcess` to zygote's pid | 119 ms |
| to `attachApplication` | 162 ms |
| to `finishAttachApplication` (the application created) | 414 ms |

## Not yet

Activities and their windows (#1422), external storage (#1423),
providers and the font map in `bindApplication` (#1424), PackageManager's
writes (#1425), the parse at the first start (#1426) and system apps'
hidden API allowlist (#1427).
