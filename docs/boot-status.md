# Boot status

A living record of how far the original Android 16 userspace boots under
aim-guest-init on the syscall layer (ADR 0012, tracking #153). Update it
with every boot-relevant change: the table, the date and the run it came
from.

## How to reproduce

```
cargo aim build        # <derived> is target/aim/derived-image
guest-init --image <derived> --data <data> --run \
    --exclude zygote,surfaceflinger,vold,bpfloader --timeout 45
```

- `--exclude` keeps services (and `exec` programs) from starting. A
  `wait_for_prop` or `exec` that only an excluded service would end is
  satisfied after two seconds, as with `--only`.
- guest-init stops every service on its timeout, on SIGINT and on SIGTERM.
- The device is `aim` (`androidboot.hardware=aim`):
  `/vendor/etc/init/hw/init.aim.rc` and `/vendor/etc/fstab.aim` from
  `image/overlay.toml`. The emulator's `init.ranchu.rc` and helpers are
  removed; `android-image diff` lists them with reasons. The vendor
  `build.prop` still describes the emulator (#206).

## Debugging

The original logd runs, and every service logs to it. Read it with the
image's own logcat from another `linux-run` process:

```
tools/guest-logcat.sh [--linux-run PATH] <data>/run            # logcat -d -b all -v threadtime
tools/guest-logcat.sh <data>/run -d -s keystore2               # any logcat arguments
```

- A service's stdout and stderr (the layer's own messages: unimplemented
  syscalls, fatal signals with the faulting module) are in
  `<data>/run/logs/<service>.log`.
- Fatal signals in host code are symbolized there, for example
  `_platform_memmove+0x1bc (libsystem_platform.dylib)`.
- `linux-run --path-map <data>/run/path-map --binder
  dev.aim.guest-init.<pid>.binder /system/bin/service list` lists the
  registered binder services.

## Early boot (2026-09-28)

Everything but the Java world and the services noted below, on an M2 Pro.

- **Result:** 40 services running after 30 s, 48 binder services
  registered with the original servicemanager, and 8,800 lines in logd.
  With selinuxfs's classes (#203) logd holds about 1,070 lines after
  44 s instead of 9,360 after 32 s: 8,390 of those were "Unknown class".
- **Boot time:** init's queue first went idle after 12.0 s, and the last
  early service started at 13.5 s. About 8 s of that are four two-second
  grace periods for waits that only excluded or absent services end:
  - vdc `checkpoint markBootAttempt` and `prepareCheckpoint` and
    `keymaster earlyBootEnded` wait for vold;
  - `keystore.module_hash.sent` waits for keystore2, which waits for
    apexd's `apexservice`.

  Without them the early boot takes about 5.5 s.

### Services

States are init's `init.svc.*` after 30 s. "Restarting" means the service
keeps exiting and init restarts it every 5 s.

| Service | State | Why |
| --- | --- | --- |
| servicemanager, hwservicemanager | running | |
| logd | running | its kernel-log and audit listeners do not start (no `/proc/kmsg`, `syslog(2)` or `NETLINK_AUDIT`: #207, #201) |
| vndservicemanager | not declared | the image has no `vndservicemanager` service |
| prng_seeder | running | |
| keystore2 | running | waits for `apexservice` (`IApexService.getActivePackages`) for the module hash (#200) |
| vendor.keymint-default | running | software KeyMint |
| vendor.gatekeeper_nonsecure | running | the first start ends with SIGABRT and no log message; the restart registers `IGatekeeper/default` (#205) |
| gatekeeperd | running | |
| credstore, vendor.identity-default | running | |
| vendor.health-aim | running | our health HAL |
| vendor.graphics.allocator | running | our allocator HAL |
| vendor.authsecret_default, vendor.cas-default, vendor.drm-widevine-hal, vendor.power-default, vendor.power.stats-default, vendor.thermal-example | running | vendor APEX HALs |
| neuralnetworks_hal_service_* (3) | running | CPU sample drivers |
| system_suspend | running | no `/sys/power/suspend_stats` |
| tombstoned | running | |
| statsd | running | |
| traced, traced_probes | running | |
| hidl_memory | running | |
| media, mediametrics, mediadrm, drm | running | |
| media.swcodec, mediaextractor | running | their minijail seccomp filters are accepted but not enforced |
| cameraserver | running | no camera HAL declared |
| installd, idmap2d, incidentd, storaged | running | |
| wificond | running | no nl80211 (`AF_NETLINK`, #201) |
| gpu | running | |
| boringssl_self_test64, boringssl_self_test64_vendor, boringssl_self_test_apex64 | exit 0 | |
| system_aconfigd_platform_init, system_aconfigd_socket_service, mainline_aconfigd_init, mainline_aconfigd_socket_service | exit 0 | |
| system_aconfigd_mainline_init | exit 1 | "aconfigd_mainline is enabled, skipping mainline init" |
| derive_sdk, derive_classpath, art_boot, logd-reinit, update_verifier | exit 0 | |
| usbd | exit 0 | no USB HAL |
| odsign | stops itself | keystore refuses the boot-level key (`Boot stage key absent`, `LOCKED`; #200) |
| misctrl | exit 1 | no `/misc` partition |
| exec `recovery-refresh` | exit 254 | no pstore |
| exec `chattr +F /data/media` | exit 1 | `FS_IOC_GETFLAGS` is ENOTTY |
| exec `otapreopt_slot` | exit 1 | not an A/B device ("Slot property empty") |
| exec `kcmdlinectrl update-props` | exit 1 | no `/misc` partition |
| lmkd | restarting (exit 0) | no PSI or memcg (P3 replaces it) |
| netd | restarting (SIGABRT) | `libnetd_updatable_init`: no cgroup v2 directory; then eBPF and netlink (#202, #201; P3 replaces it) |
| audioserver | restarting (SIGSEGV) | null dereference after "Found no HAL version": needs the audio HAL (P5) |
| vold | excluded: exit 1, then init reboots (`reboot_on_failure`) | no `NETLINK_KOBJECT_UEVENT` socket (#201; P3 replaces it) |
| bpfloader (netbpfload) | excluded: exit 1, then init reboots (`reboot_on_failure`) | no bpffs at `/sys/fs/bpf` (#202) |
| surfaceflinger | excluded: SIGABRT | guest-init passes no `--gpu` (#204), so RenderEngine gets `EGL_BAD_DISPLAY`; with it, RenderEngine runs on ANGLE and SurfaceFlinger aborts with "failed to get hwcomposer service" (the composer is P4) |
| zygote | excluded | ART (P2) |

Every crashing process also logs "crash_dump helper failed to exec, or was
killed": debuggerd's `crash_dump64` does not run yet (#191).

### Fixed on the way

- The binder host no longer reads the argument of `BINDER_THREAD_EXIT`
  (libbinder passes 0). keystore2 crashed in the layer when a hwbinder
  thread exited.
- `/proc/self/fd` (and any synthesized `/proc` directory) has a link and
  `fstat` that match its path, so bionic's `realpath` works. prng_seeder's
  inherited-fd scan aborted.
- `/proc/mounts`, `/proc/<pid>/mounts` and `mountinfo` describe the path
  map.
- `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)` is accepted.
- A `#!` script as `linux-run`'s program runs its interpreter.
- guest-init plays apexd more closely: `apex.all.ready` is set at
  activation (libvintf reads the vendor APEXes' VINTF fragments only
  then), a lazy `aidl/apexservice` start no longer resets `apexd.status`,
  and `perform_apex_config --bootstrap` loads the scripts of
  `vendorBootstrap` APEXes (the gatekeeper HAL, class `early_hal`).
- guest-init resolves symlinks in the image relative to the guest root, as
  a GSI's `/system_ext -> /system/system_ext` needs.
- guest-init's shutdown stops services without running `onrestart`.
- selinuxfs has the classes and permissions of the image's policy and an
  `access` that allows everything, so `selinux_check_access` answers
  quietly (#203).
- `/dev/ashmem` exists (#195): audioserver's `MemoryHeapBase` no longer
  fails with "Unable to open ashmem device".
- POSIX timers, a writable `trace_marker` (#192), per-thread `comm` files
  (#198), and `/proc/<tid>`, the scheduler calls and `tgkill` for every tid.
- guest-init's wait for linkerconfig ends at `--timeout` and on SIGINT or
  SIGTERM (#196).

## Java world (P3, 2026-09-28)

The original zygote and system_server on the ART exception, on an M2 Pro.
`guest-init --run` with the derived image of `image/overlay.toml`: our Rust
apexd, vold, netd and lmkd (`daemons/`, the `daemon/*` nodes) replace
the originals (lmkd since the second part below; it kills nothing, #222).

- **zygote** preloads 18,367 classes in 0.31 s and forks system_server
  0.07 s after its preload ends, listening on init's `zygote` and
  `usap_pool_primary` sockets;
  its seccomp filter is accepted and not enforced.
- **system_server** runs its bootstrap and core services, then
  `startOtherServices`: PackageManager scans 245 packages and first-boot
  dexopt runs through artd and dex2oat64 (137 results), about 8 s from
  `StartPackageManagerService` to `startOtherServices`. 197 binder services
  are registered. ActivityManager is ready 15 s after the fork and starts
  SystemUI, the network stack, phone, Bluetooth, the WebView relro creator
  and the setup wizard (`top-activity`) through zygote; SystemUI's shell
  runs.
- **Memory** (host RSS, shared pages counted in each process): system_server
  about 220 MB, zygote 33 MB, surfaceflinger 32 MB, artd 39 MB; 52 guest
  processes take about 730 MB together.
- **Not reached in this first part: `sys.boot_completed`.** On the last runs the host's
  coreaudiod stopped answering (its log repeats `BeginWriteOperation:
  still waiting`; a plain host program's `AudioObjectGetPropertyData`
  hangs too), the audio HAL never registers `IConfig/default`, audioserver
  waits for it, and system_server blocks in `AudioService.<init>` until its
  watchdog kills it (#217).

### Fixed on the way

- ART (`0005`): nterp's `new-instance`/`new-array` decode the class after
  the read-barrier mark entrypoint, and `ExecuteNterpWithClinitImpl` decodes
  the declaring class. libadbconnection is rebuilt (zygote loads it).
- Mount namespaces are per-process path-map entries (`unshare`, bind,
  tmpfs, move, `umount2`, carried over `execve`); init's own binds between
  writable areas (the data mirrors for app data isolation) become path-map
  entries for processes started later.
- `--stdio-null`: services get `/dev/null` on fds 0–2 as init gives them,
  and the layer logs to a hidden descriptor (zygote refused its
  non-allowlisted stdout).
- cgroup v2 and bpffs are areas of the path map; `bpf()` creates maps in
  shared memory and pins them, and loads programs and BTF without running
  them (#224), so NetBpfLoad and uprobestats load everything.
- A forked child gets the stub islands' executable views back as views of
  writable memory (SIGILL in system_server's first JNI call).
- xattrs, with `security.selinux` from the original image (re-extract with
  the current android-image-extract, #229), from `genfscon` for bpffs and
  cgroup2, or `unlabeled`; `stat` reports an image file's original owner
  and mode. installd's restorecon and the tethering module's
  `verifyClatPerms` need them.
- `init_user0` runs `vdc cryptfs init_user0`; vold prepares `/data/data`
  and user 0's storage. `/data/user/0` was a symlink to `/data/data` (#221);
  it is now a path-map bind (see #234 under "Open").
- Fork is a freshly spawned linux-run that takes over a copy-on-write
  snapshot of the guest memory and the layer's state (`docs/fork.md`): a
  Darwin `fork()` child cannot reach XPC services, so apps could not
  compile shaders (#233), and it needed the layer's locks held across the
  fork (artd's dex2oat child hung on fs-attrs) and `__objc_fork_ok` (zygote
  children died of SIGKILL when two threads met in an Objective-C
  `+initialize`). A thread with its own file table runs as a process
  (#228; the debuggerd pseudothread closed every fd of system_server).
- Binder: a zero-length user copy succeeds (a restarted
  `BINDER_WRITE_READ` failed with EFAULT, and libbinder aborted).
- An empty `SCM_RIGHTS` passes nothing; PF_KEY sockets open (NetworkStats'
  `synchronizeKernelRCU`); `setpriority` honors `RLIMIT_NICE`; fwmarkd
  listens; the GNSS HAL answers UNSUPPORTED for its nullable extensions.

## sys.boot_completed (P3, 2026-09-28, second part)

The same boot, on an M2 Pro (16 GB), reaches `sys.boot_completed=1`. First
boot (empty data), `--exclude bootanim` (see "Host security agent"),
`aim-display --size 1080x1920` for the window, the audio HAL on its
null sink (the host's coreaudiod hung, #217):

```
aim-display --socket DISPLAY --size 1080x1920 &
guest-init --image DERIVED --data DATA --run --exclude bootanim \
    --gpu _build/angle-source/out/AimRelease --display DISPLAY
```

`cargo aim boot` runs these two with these flags, and gives aim-display
`--capture target/aim/boot/capture.bmp`: SIGUSR1 to it writes the last
presented buffer there.

| Since guest-init started | s |
| --- | --- |
| zygote (`boot_progress_start`) | 5.8 |
| system_server runs | 7.6 |
| PackageManager ready (245 packages) | 14.3 |
| ActivityManager ready | 23.3 |
| `boot_progress_enable_screen` | 25.2 |
| `sys.boot_completed=1` (keystore2 sees it) | 25.3 |

- **Services:** 277 binder services registered; `init.svc.lmkd`,
  `init.svc.zygote`, `init.svc.surfaceflinger`, `init.svc.audioserver` and
  `init.svc.vendor.audio-hal-aidl` running.
- **Memory** (host RSS, shared pages counted in each process): 74 guest
  processes with about 2.8 GB together at `sys.boot_completed`, system_server
  369 MB, zygote 144 MB; about 4.4 GB for 92 processes once Settings and the
  first-boot apps (GMS, launcher, SystemUI) run. `dumpsys meminfo` has no
  PSS (no `smaps_rollup`).
- **Settings:** `am start -W -n com.android.settings/.Settings` 1 s after
  boot completion: `Status: ok`, `LaunchState: WARM` (its process was up for
  a broadcast), `TotalTime: 243` ms (`wm_activity_launch_time` 243).
  `dumpsys activity activities`: `topResumedActivity=ActivityRecord{... u0
  com.android.settings/.Settings t7}`. A cold launch (after `am force-stop`)
  could not be measured: the host's security agent had ended the Java world
  by then.
- **Input:** a tap injected into aim-display's touchscreen through its
  injection path (a client of `DISPLAY.input/event0` writing records, as the
  tests do; no host HID events) reaches the guest as an evdev packet
  (`getevent -lt`: `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X/Y`,
  `BTN_TOUCH`, `SYN_REPORT`), InputDispatcher's `RecentQueue` holds the
  down/up MotionEvents, and `input_interaction: Interaction with: ...
  Android System` names the top window it went to: an app-error dialog
  that the crash loops of #234 keep on top of Settings. The tap did not
  visibly act on it.
- **Screen:** aim-display presents at 60 fps, but every app window is
  black: zygote children cannot reach Metal's shader compiler service
  (#233). SurfaceFlinger's own output (bootanimation, P4) is not affected.

### Fixed on the way

- **Audio HAL (#217):** registers at once; CoreAudio runs in
  `linux-run --audio-io` with timeouts and a null sink
  ([audio.md](audio.md)).
- **lmkd:** ActivityManager waits for the lmkd socket under its lock on
  every process event. Without lmkd, app starts took 10–25 s, ANRs piled
  up and the watchdog killed system_server a few minutes after boot. Our
  lmkd answers the protocol and kills nothing; the first boot went from
  75–95 s to 25 s.
- **`/proc/config.gz`:** system_server's `Debug.isVmapStack` CHECKs that
  libvintf can read the kernel configuration; the first ANR aborted it.
- **Null page of the heap window:** a fault there is a null-check fault
  (#236); every restarted system_server had died in
  `NotificationChannel.setVibrationPattern`.
- **vold:** the storage views replace init.rc's empty placeholder
  directories (IVold.mount failed with EEXIST), and user 0's exist from
  `initUser0` on; zygote had aborted every app with an installer mount
  mode (Settings: "Failed to mount /mnt/installer/0 to /storage").

### Host security agent

The development Mac runs Exosphere, whose anti-ransomware module flags the
boot's `linux-run` (when bootanimation reads `bootanimation.zip`, and
when GMS churns files after boot) and from then on denies its write opens
(EPERM) and kills every exec of it (SIGKILL, exit 137) (#232). A boot
here therefore excludes bootanim, uses a freshly built `linux-run`, and
runs its `am`/`dumpsys` commands with another build; the Java world lasts
about a minute after `sys.boot_completed`.

### Open

- #233 app rendering (Metal compiler service after fork);
- #234 app data isolation (SQLite `CANTOPEN` in `/data/user/0/<pkg>`),
  crash loops of acore, launcher and GMS. Cause, from the code paths:
  zygote's `isolateAppData` mounts a tmpfs over `/data/data` and
  `/data/user`, then looks for the app's CE directory at
  `/data_mirror/data_ce/null/0/<pkg>`. That mirror is init's bind of
  `/data/user`, whose `0` was vold's symlink to `/data/data`; in the app's
  view the symlink leads into the new, empty tmpfs, so `getAppDataDirName`
  finds nothing, zygote logs "Ignoring missing CE app data dir" and binds
  no CE directory. `/data/user/0/<pkg>` then does not exist in the app,
  and SQLite cannot create its database. The DE directory was bound.
  Fixed on `agent/appdata`, **not yet verified at runtime** (#232):
  - `/data/user/0` is a path-map bind of `/data/data`, as vold's
    `prepare_special_dirs` bind is on Android; init's `bind rec` of
    `/data/user` copies it to `/data_mirror/data_ce/null/0`; vold checks
    the bind instead of making the symlink;
  - in the layer, a process's own mount hides the older mounts at and
    below its mount point (zygote's `symlink("/data/data", "/data/user/0")`
    in its tmpfs would fail with EEXIST otherwise);
  - fs-attrs are keyed by a file's path in its area, so the stub zygote
    gives `root:root 0700` in its tmpfs no longer overwrote the owner
    installd gave `/data/data/<pkg>`, and `/data/user/0/<pkg>` shows it.
  To confirm: a boot, then `am start` of Contacts (acore) or the launcher
  with no `CANTOPEN` / "Ignoring missing CE app data dir" in logcat, and
  `ls -ln /data/user/0/<pkg>` from `run-as` (or the app) showing its uid;
- #235 evdev fds across exec; #236 the ART codegen behind the null-page
  faults; #237 audio retries after the null sink; #232 the security agent.


## P3–P5 acceptance (2026-09-28, third part)

The same boot with the spawned fork (docs/fork.md), ANGLE displays made
on first use (#233) and `/data/user/0` as a bind (#234), on an M2 Pro
(16 GB), release build. First boot (empty data), `--exclude bootanim`,
`aim-display --size 1080x1920 --capture FILE` (SIGUSR1 writes the last
presented buffer), the audio HAL on CoreAudio. Before any input, the
guest's `sound_effects_enabled` and `charging_sounds_enabled` are set to 0;
the only test signal is the -90 dBFS tone below.

| Since guest-init started | s |
| --- | --- |
| zygote (`boot_progress_start`) | 6.8 |
| system_server runs | 8.9 |
| PackageManager ready | 16.5 |
| ActivityManager ready | 34.6 |
| `boot_progress_enable_screen` | 44.0 |
| `sys.boot_completed=1` (keystore2 sees it) | 50.5 |

The first boot is twice as long as with the Darwin fork (25.3 s), and app
frames are slow (#239).

| | Result | Evidence |
| --- | --- | --- |
| **P3** | passed | `sys.boot_completed=1` at 50.5 s (48.2 s on another run); 68 guest processes with 2.9 GB RSS at that point, 118 with 3.5 GB once Settings ran (system_server 205 MB, Settings 93 MB). |
| **P4** | passed | `am start -W -n com.android.settings/.Settings`: `Status: ok`, `LaunchState: WARM`, `TotalTime: 5076`; `topResumedActivity` and `mCurrentFocus` are Settings. The captured buffer shows the Settings home page, not black. Logcat has no MSL or MTLCompilerService message and no EGL call error; HWUI's config complaint "Device claims wide gamut support" remains (#240). |
| **App health** | passed | No crash of acore (ContactsProvider2), the launcher or GMS, no `SQLITE_CANTOPEN`, no "Ignoring missing CE app data dir", and no dialog over Settings. com.android.phone and GMS persistent each have one startup ANR and come back (#241). |
| **Input** | passed | A tap written into aim-display's touchscreen through its injection path (a client of `DISPLAY.input/event0` writing `ABS_MT_*`, `BTN_TOUCH` and `SYN_REPORT` records; no host HID events) on "Connected devices" opens it: `topResumedActivity` goes from `.Settings` to `.SubSettings`, and the second capture shows the "Connected devices" page. |
| **Audio** | passed | `service check media.audio_flinger`: found. `audio_tone 2000 0` (AAudio, -90 dBFS): 96,000 frames written, `output_xruns 0`, `ok done`. AudioFlinger's primary output (`AUDIO_DEVICE_OUT_SPEAKER`) wrote 240,768 frames; the HAL logged "output stream in standby: 240768 frames, 469 device callbacks, 0 xrun frames, peak -90.0 dBFS": the stream reached CoreAudio, not the null sink. |
| **Sensors** | passed | `dumpsys sensorservice`: Ambient Light Sensor (`android.sensor.light`) and Lid Angle Sensor (`android.sensor.hinge_angle`), vendor "Apple (darwin host)". |
| **Thermal** | passed | `dumpsys thermalservice`: HAL AIDL 3 connected; cpu 52.9 °C, battery 33.8 °C, skin NaN. |
| **Health** | passed | `dumpsys battery`: level 80, AC powered, as `pmset -g batt` (80 %; AC attached). Temperature reads 0 (#242). |
| **Bluetooth** | passed | `dumpsys bluetooth_manager`: `enabled: true`, `state: ON`, crashed 0 times, over our HAL's virtual controller. No scan was run (TCC). |
| **GNSS** | passed | `dumpsys location`: `gps provider` enabled and allowed; GNSS hardware model "darwin CoreLocation". |

The host's security agent did not flag or block any of the four boots
(#232), each from a freshly built target directory.

### Fixed on the way

- **Fork children aborted** ("fdsan: double-close of file descriptor 55")
  and the boot never completed: the layer's timer kqueue stayed at a low
  fd when RLIMIT_NOFILE exceeded the descriptor table, and a fork child
  kept it hidden although it did not inherit it, so the guest's next fd
  there could not be closed. Hidden fds go above 3/4 of the table, and a
  child hides only what it inherited.
- **"There's an internal problem with your device"** over Settings (#227):
  libvintf's runtime check read the kernel's SELinux policy version as 15;
  selinuxfs has `policyvers` (33).
- **Lost logs:** logd's `logdw` had Darwin's 4 KiB datagram buffer, and
  liblog dropped about 6,300 messages in a boot. guest-init gives init's
  datagram sockets Linux's 208 KiB.
- **Bluetooth crash loop:** the stack asserts Secure Simple Pairing
  (`btm_sec_dev_reset`); the virtual controller claims it.
- **"Failed to wait for the fence 0x3006"** in the launcher: ANGLE's Metal
  backend refuses `eglClientWaitSync` without a current context, which
  HWUI's bitmap uploader does; the driver polls the sync's status then.
- **Chrome exited at first run** (#251): its shared memory check
  (`SharedMemoryRegionGetProtectionFlags`) found no `/dev/ashmem` node and
  failed. The node stats as the device, on the region files' `st_dev`.
  memfds now keep their name and seals with the file, so an fd received
  over binder or `SCM_RIGHTS`, or reopened through `/proc/self/fd`, is
  the same memfd. Chrome shows its first-run page; about 35 s later the
  browser aborts waiting for its GPU process (#256).

### Open

- #239 boot time and frame times after the spawned fork;
- #238 app processes' names in `/proc/<pid>/cmdline` for other processes;
- #240 wide-gamut EGL configs; #241 phone and GMS startup ANRs; #242
  battery temperature; #230 traced and traced_probes aborts.
- #256 Chrome's GPU child never gets `setupConnection`; #257 ESRCH from a
  socket call in Chrome's network thread; #258 remaining memfd seal gaps.
