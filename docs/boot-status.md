# Boot status

A living record of how far the original Android 16 userspace boots under
aim-guest-init on the syscall layer (ADR 0012, tracking #153). Update it
with every boot-relevant change: the table, the date and the run it came
from.

## How to reproduce

```
cargo aim build        # <derived> is target/aim/derived/root
guest-init --image <derived> --data <data> --run \
    --exclude zygote,surfaceflinger,vold,bpfloader --timeout 45
```

- `--exclude` keeps services (and `exec` programs) from starting. A
  `wait_for_prop` or `exec` that only an excluded service would end is
  satisfied after two seconds, as with `--only`.
- Shutdown cleanup is incomplete on SIGINT: guest processes and the
  mounted data volume can remain after guest-init exits (#796).
- `<data>` is a case-sensitive disk image, `<data>.asif`, that guest-init
  attaches at `<data>` for the boot and detaches when it stops
  (docs/storage.md); `cargo aim storage` shows what it occupies.
- The device has no boot animation (`debug.sf.nobootanimation`);
  `--exclude bootanim` still works but is no longer needed.
- The device is `aim` (`androidboot.hardware=aim`):
  `/vendor/etc/init/hw/init.aim.rc` and `/vendor/etc/fstab.aim` from
  `image/overlay.toml`. The emulator's `init.ranchu.rc` and helpers are
  removed; `android-image diff` lists them with reasons. The vendor
  `build.prop` still describes the emulator (#206).

## Native system services (2026-09-29, ADR 0013)

`image/native-services` lists the system services implemented natively
(docs/system-services.md): today `clipboard`, `vibrator_manager`
(with `external_vibrator_service`), `location`, `thermalservice` and
`uimode`. The
derived image's `services.jar` does not start `ClipboardService`,
`VibratorManagerService$Lifecycle`, `LocationManagerService$Lifecycle`,
`ThermalManagerService` or `UiModeManagerService`
(their `SystemServerTiming` trace lines stay), and guest-init registers
the native services with servicemanager when `servicemanager.ready` is
set; `service check clipboard` finds it. The vibrator control service
(`IVibratorControlService/default`) is no longer declared or published.
Boots with it reach `sys.boot_completed` as before (four boots of a
reused data image, 20-25 s; fresh data images 50-52 s, then a cold
Settings start in 11.9 and 13.8 s); SystemUI and Gboard, which listen to the
clipboard, run without errors, and CTS's 36 clipboard tests pass. An empty list gives the original clipboard back.

PackageManager still runs original. On the M4 C branch, native
`package::owner::Store` writes enabled settings to the original ABX
restriction file with backup, reserve copy and system ownership (#798).
Two original-PMS boots on disposable native-written data (2026-10-02)
reached `sys.boot_completed=1`: Settings read as disabled (`enabled=2`),
then after a native reset to default, Settings launched successfully
(`am start -W`, 85 ms). This checks file compatibility; the native
PackageManager service and SystemServer facade are not activated. The
C branch also has Binder query receivers for `package` and
`package_native`, tested for caller visibility, interface tokens and
shared snapshot publication (130 package unit tests pass). These are not
registered in guest-init; their write path still returns an explicit
unsupported-operation exception.
The native scan's SystemConfig library reader also matches all 19
built-in library names and paths from `pm list libraries -v` and
`dumpsys package libraries` on a disposable original-PMS boot
(2026-10-02, `sys.boot_completed=1`). The declaration registry assembled
from persisted package settings and native-parsed APKs matches all 23
original library names/paths. With isolated split dependencies and asset
scopes implemented and runtime density supplied, the diagnostic parses
all 243 installed APK packages, including GMS's 11 splits (#720). The
native parser's output also
matches all 288 original-PMS scan cache entries byte for byte on a
disposable boot (2026-10-02, `sys.boot_completed=1`); these include the
system GMS/Play Store packages, not the installed GMS cluster. A compiled
split APK integration test passes for manifest merging, split ordering,
dependency serialization, missing/cyclic dependencies and malformed
clusters; a unit test checks ancestor/config asset scopes without sibling
assets. Installed GMS's full parser parcel has not been compared against
the original (#760). This remains an incomplete native scan check.
Shared-library dependency resolution now has ordered direct selection,
static/SDK version and signer checks,
and file-path assembly from resolved provider snapshots. Tests cover
missing/optional/native dependencies, SDK policy, signer rotation,
multiple signers and deduplication. The candidate graph builder now
resolves acyclic APK provider graphs, fills nested dependency records and
package file paths, and marks static libraries installed for the direct
consumer's installed users. Tests check multihop ordering, input
immutability and user effects. Cyclic provider updates fail explicitly
(#799). The original PlatformCompat install-time native-library policy
is now queried through the system-server bridge (package name and target
SDK, without PMS lookup). On a disposable original-PMS boot (2026-10-02,
boot completed), a temporary Rust diagnostic received false for SDK 30
and true for 31/36; Settings started successfully (warm, 90 ms). The
diagnostic was removed and the boot/data cleaned. SDK library independence
has no matching Flags API or aconfig entry in this image and still needs
an authoritative policy (#800). Bootstrap facade wiring, native scan
integration and native PackageManager
activation remain pending (#707);
no native PackageManager CTS result is claimed.

The device's own system service (`dev.aim.server.DeviceServices`,
docs/system-services.md, "The system_server bridge") is on the system
server class path (`/system/framework/aim-services.jar`, compiled by the
`oat` node) and named by the static overlay
`/vendor/overlay/aim-framework-overlay.apk`. SystemServer starts it in
`StartDeviceSpecificServices`, and at
`PHASE_DEVICE_SPECIFIC_SERVICES_READY` it hands guest-init's service
host (`aim.service_host`) the bridge: guest-init logs "system_server's
bridge attached". Its second boot of a data directory reached
`sys.boot_completed` in 12 s (load 8), and Settings started cold in 231 ms.

## Compiled oat files (2026-09-30)

The boot image extension holds all 31 mainline BCP jars, as the original's,
and the 29 oat files with code (system_server's class path,
`org.apache.http.legacy`, six `speed` apps) are compiled for it by the `oat`
node (docs/art-exception-patches.md, "Other oat files"); the image's
dexoptanalyzer finds them usable, the originals not. The 139 `verify` odex
files are still rejected ("Read barrier state mismatch": no CMC without
userfaultfd, #442) and their vdex used. On four fresh-data smoke boots at
load 4-17 rising to 40-60 (two before, two after), `sys.boot_completed`
came at 50-51 s either way and `boot_progress_enable_screen` at 36.5-40.7 s
before, 36.8-37.3 s after; a cold Settings start took 10.5 s and 33.5 s
before, 45 s and 19.3 s after, dominated by ANRs of com.android.phone and
GMS in all four (gone since; see "First boot").

## First boot (2026-09-30, #241, #375)

Two fresh first boots of main (49eed57f, host load 9.5) and of the device
without telephony (load 3.9), measured from guest-init's start to
`sys.boot_completed` and the 5 minutes after, CPU per guest process from
the host every 2 s:

| Check | Before | Without telephony |
| --- | --- | --- |
| `sys.boot_completed` (1 s polling, within noise) | 14 s | 16 s |
| `am_anr`, `am_crash` | 0, 0 | 0, 0 |
| RILJ log lines (phone process) | 180 | 0 |
| lmkd kills (host memory pressure, #300) | 25 | 0 |
| Guest CPU: boot, 0-2 min, 2-5 min | 17.5, 191, 10.4 s | 20.4, 167, 12.3 s |
| Settings cold start | 278 ms | 253 ms |

The ANRs of com.android.phone and GMS persistent ("failed to complete
startup") no longer happen. The device declares no telephony (the
emulator's `handheld_core_hardware.xml` without it, `image/overlay.toml`),
so the phone process no longer builds a GSM phone and RIL for a modem it
does not have. The first two minutes are Android's and Google's own work:
Play Store and GMS update themselves from the network 40-70 s after boot
(`installPackageLI` stops them and their clients), dex2oat compiles the
updates (about 28 s of CPU), GMS and the Google apps start (about 90 s),
system_server takes 18-21 s. guest-init takes 14-19 s: it hosts the binder
driver, whose transport cost is #451. After two minutes the guest is
nearly idle. The kills of cached processes in the first run came from
lmkd reacting to the Mac's memory pressure, not from the guest.

## Other processes in /proc (2026-09-30, #238, #379)

Each process keeps a record beside its by-pid entry
(`docs/guest-init-contract.md` section 4): its threads, and the stack
pages of its argument strings, mapped from the record. Another
process's `cmdline`, `comm`, `task/` and per-thread `stat` read it.
One boot each of main (aeb32a54) and the branch in the main tree, a
fresh data image (main's boot was its first, the branch's its second),
host load 3-25:

| Check | main | branch |
| --- | --- | --- |
| `ps -A -o NAME` of zygote's children | 66 of 121 lines empty, `pidof system_server` empty | none empty; system_server, SystemUI, phone, Settings named |
| `/proc/<system_server>/task` | 1 entry | 224 threads; `top -H -p` lists them with TIME+ per thread |
| "Render thread does not belong to process" | 1 | 0 |
| logd host syscalls per log line, 30 s after `sys.boot_completed` | 9.0 (452,613 for 50,095 lines) | 1.2 (33,445 for 28,016) |
| the same over the next 30 s | 5.0 (29,896 for 5,980) | 1.0 (77 for 78) |
| Settings scroll, 10 swipes: frames, janky, p50/p90 | 419, 0.72 %, 8/9 ms | 417, 0.24 %, 8/9 ms |

## Guest kernel time (2026-09-30, #446)

On a settled boot of a reused data image, main (80965c99) against the
fixes of #446, measured 7 minutes after `sys.boot_completed`, then over
the next 60 s of idle:

| Check | Before | After |
| --- | --- | --- |
| Guest CPU 7 min after boot | 77 s, 48 % system (GNSS HAL 20 s) | 53 s, 51 % system |
| Faults in that time | 1.22 M | 0.93 M |
| Guest CPU per idle minute | 1.4-6.8 s, 33-49 % system | 1.6 s, 46 % system |
| Cold start: Settings, Calculator, Chrome | 0.23, 0.30-0.33, 0.31 s | 0.21, 0.30-0.32, 0.30 s |

The GNSS HAL's CoreLocation thread no longer spins while updates are on.
Content files of `/proc`, `/sys` and selinuxfs are reused, ART's JIT
memfd is no longer copied, and ashmem and `faccessat` make fewer host
calls. A credential change rewrites the process's by-pid entry in place
(`PR_CAPBSET_DROP` 175-357 us to 4.5-4.9 us, #449), and a private
mapping of a plain file maps the file copy-on-write instead of copying
it (4 MiB: 309-344 us to 12 us, #450).

Where the remaining kernel time went before the fixes below, from a
per-syscall profile of host user and kernel time (a temporary build; M2
Pro, host load 7-16, a reused data image, boot to 4 minutes after
`sys.boot_completed`), largest first:

| Item | Kernel time | Per call | Owner |
| --- | --- | --- | --- |
| page faults and other time outside syscalls | 5.4 s of 23.5 s | | Chrome alone 1.75 s in its start (#500) |
| openat | 3.8 s | 81 µs | host open under the security agent (#418); logd reading `/proc/<pid>/cmdline` of zygote's children, empty to it, on every log line (#238, fixed since: see "Other processes in /proc") |
| BINDER_WRITE_READ | 2.5 s | 14 µs, 7 µs of it the Mach round trip | #451 |
| mmap of files | 1.9 s | 40 µs small, 1.5-5 ms at 16 MiB and more | ART's JIT memfd and private memfd mappings, fixed below (#501) |
| faccessat, madvise(DONTNEED) | 0.9 s each | 25 µs, 5.5 µs | each app re-reading owner attributes zygote had read, fixed below (#505); #502 |
| membarrier, ashmem PIN | 0.5 s, 0.4 s | 140 µs, 16 µs | #503, #504 |
| fork (zygote's clone) | 0.5 s | 10 ms | #421 |

A binder read reserves placeholder fds for the files a transaction may
carry. They were opens of `/dev/null`, and installing a file over one
closed a vnode: about 40 µs of kernel time per file received. They are
sockets now (docs/binder-driver.md, "Fd transport"), and a
BINDER_WRITE_READ went from 14.8 to 8.7 µs of kernel time in the caller
up to `sys.boot_completed`, and from 25.8 to 10.0 µs while Settings
started twice and Chrome once (0.98 to 0.33 s in all; one boot each). A process no longer sweeps the memfd
directory at its first memfd_create (2.2 ms with 48 memfds alive; 49
calls in a boot averaged 2.9 ms).

A binder read that waits for work no longer blocks a thread of the
binder daemon: it parks, and the daemon thread that brings the work
answers it (docs/binder-driver.md, "Driver–process path"). A call
crosses four Mach hops and no wake inside the daemon, and the driver's
maps no longer hash with SipHash. On a settled boot (one each, host load
10 and 6), the synchronous calls of a Settings and a Chrome cold start
went from p50 61 to 51 µs, and guest-init's CPU over the starts from
0.74 to 0.60 s. What is left in the caller is one Mach round trip per
ioctl, about 7 µs of kernel time in a boot (#451). Since no daemon thread
blocks, one per CPU serves every guest binder thread (#553): guest-init
has 33 threads after a boot and a Settings and a Chrome start, where it
had one per guest binder thread.

membarrier now interrupts only the guest threads that are running, as
Linux does, instead of every thread of the task (host unit benchmark, 60
parked and 2 spinning threads: 115 to 58 µs of kernel time per call,
#503). ashmem PIN and UNPIN skip reading the region's attribute while
the file's ctime shows nobody changed it (an UNPIN+PIN pair: 32-39 to
19-21 µs, #504). Neither is measured in a boot yet.

The large file mappings were ART's JIT cache: its memfd's first
executable view looked for the memfd's other views by walking the whole
VM map, 3.5-4.8 ms of kernel time in a process with 3,000 mappings. The
process now records where it maps a memfd (48-50 µs). A private mapping
of a memfd maps it copy-on-write instead of copying it (16 MiB: 1.6 to
0.16 ms). A fork child starts with its parent's owner attributes (an
app with zygote's; still checked against each inode's ctime), so its
first stat of a file zygote knew costs 2.6 µs instead of 20 µs (#478,
#505). On a reused data image, one boot each (host load about 12),
before and after:

| Check | Before | After |
| --- | --- | --- |
| All guest processes at `sys.boot_completed` + 60 s: CPU, system | 48.2 s, 23.3 s | 36.9 s, 16.3 s |
| The same after the cold starts below | 55.6 s, 26.0 s | 42.8 s, 18.6 s |
| Settings cold start (3): time, the app's system CPU | 206-234 ms, 0.11-0.13 s | 177-199 ms, 0.09-0.10 s |
| Calculator cold start: time, system CPU | 331 ms, 0.17 s | 296 ms, 0.13 s |

## Vsync off at idle (2026-09-30, #452)

SurfaceFlinger kept hardware vsync on for good, because it ignored
present fences (docs/composer.md, "Vsync"): at 120 Hz, the composer and
SurfaceFlinger handled every vsync while nothing changed. Present fences
now signal at the vsync that shows the frame, SurfaceFlinger predicts
vsync from them, and aim-display runs its display link only while vsync
is enabled. On a settled boot of a reused data image (host load 5-7), one
idle minute, before (c0a2b9b7) and after:

| Check | Before | After |
| --- | --- | --- |
| Composer HAL, SurfaceFlinger CPU | 0.40 s, 0.28 s | 0.01 s, 0.02 s |
| aim-display (host) CPU | 0.22 s | 0.01 s |
| All guest processes | 2.6 s | 1.4 s |
| Settings cold start (3) | 255, 214, 224 ms | 239, 215, 213 ms |
| Settings scroll (10 swipes) | 0 % janky, p50/p90/p99 7/9/12 ms | 0 % janky, 7/9/13 ms |

During the scroll the display link did not run: SurfaceFlinger scheduled
all 253 frames from its model.

## Shared code stays mapped (2026-09-30, #444)

Fork children map code without execute and make it executable after
(docs/fork.md), so they no longer make every guest process refault the
shared libraries' code. On a settled boot of a reused data image (host
load 2-3), before and after:

| Check | Before | After |
| --- | --- | --- |
| `free(malloc(64))` loop, faults per iteration during a cold start | 0.16-0.72 | 0 |
| Guest CPU 7 min after boot | 1,565 s, 96 % system | 78 s, 46 % system (#446) |
| `isDeclared` after 300 ms idle | 1.7-2.3 ms | 1.1-1.7 ms, no faults |
| Cold start (`am start -W -S`): Settings, Calculator, Chrome | 0.97-1.01, 1.45-1.46, 1.06-1.14 s | 0.23, 0.31, 0.29-0.34 s |

One of the two boots with the fix hit SurfaceFlinger's hung task
snapshot (#436): the display server deadlocked adding a drawable's
presented handler while Core Animation ran an earlier one, so a present
fence never signaled and RenderEngine waited on it for ever. Fixed
(docs/composer.md, "Buffers and presents"); 20 force-stops of a visible
Settings in one boot then passed.

## Boot timeline (2026-09-30)

One disposable data directory, first and second boot, host load about 9
on the M2 Pro (docs/system-services.md, "Shrinking SystemServer", has
the SystemServer detail): `boot_progress_start` 4.5 and 7.1 s,
`system_run` 6.8 and 9.2 s, `pms_ready` 12.0 and 10.2 s, `ams_ready`
15.5 and 11.0 s, `enable_screen` 16.2 and 11.5 s, `sys.boot_completed`
16.6 and 11.8 s. SystemServer's services take about 0.7 s of a second
boot; the shell (the launcher's first draw, then SystemUI's keyguard
and wallpaper) holds boot completion for 0.67 s after home starts.

guest-init's report breaks the time before zygote down: each command
carries the time it ran, and its `timeline:` lines give the preparation
steps (runtime layout, property areas, scripts, binder host), the data
image's attach and mount, every wait of init's queue with its length,
and every property a service sets, since guest-init started (also
`ro.boottime.*`'s epoch). The data image attaches on a thread while the
boot prepares and runs early-init and init; the `fs` stage's `mount_all`
waits for it (docs/storage.md). On a second boot (host load 26-31) the
preparation takes 0.16 s, the attach ends at 0.50-0.53 s and the mount
waits 0-0.07 s for it; `start zygote` runs at 1.58-1.60 s (a first
and a second boot, host load 10) and `boot_progress_start` about 0.75 s
later. linkerconfig runs once, at `perform_apex_config --bootstrap`:
post-fs-data's run would see the same APEXes (#564). A first boot
clones the image from the template of the `userdata/template` node
(docs/first-boot.md): its attach ends at 0.41-0.53 s and the mount waits
0-0.07 s (host load 12-14; creating the image held the mount up by
0.6 s before, #563). The template build boot puts its display socket in
a short temporary directory so a long worktree path fits Darwin's Unix
socket limit; the directory is removed when the boot stops. From the
template's PackageManager and permission state (docs/first-boot.md),
PackageManager takes 0.57 s from
`pms_start` to `pms_ready` (3.90 s on an original first boot: the scan,
the stubs' decompression) and 0.47 s to `ams_ready` (2.64 s: boot
dexopt), and `sys.boot_completed` comes at 4.9 s (10.6 s without the
template; host load 9-18, 2026-10-01). Those times had the parser cache
in the template; it depends on the device's locale and is no longer
shipped (#722), so the scan parses every package: 0.63-1.03 s against
0.42-0.48 s on a repeat boot, `pms_start` to `pms_ready` 0.85-1.35 s
(host load 8-14, 2026-10-02). Between the mount and zygote-start, init runs its exec
programs one after another (about 25-120 ms each, mostly starting
`linux-run`); the longest are bpfloader (0.3-0.6 s) and
`aconfigd-mainline init` (0.12-0.36 s) (#529). bpfloader's 0.32 s
(run alone: 0.31 s) is three process images (netbpfload execs
uprobestatsbpfload, then the platform bpfloader, about 0.03 s each),
netbpfload's own 300,000 `lseek`/`read` calls of 0.3-0.6 us each
(bionic stdio re-reading its ELF objects; a Linux kernel costs about the
same) and the 142 objects' file creations, pins (hard links), renames and
owner records at 0.1-0.5 ms each on the host (#418, #561).

## Debugging

The original logd runs, and every service logs to it. Read it with the
image's own logcat from another `linux-run` process:

```
tools/guest-logcat.sh [--linux-run PATH] <data>.run            # logcat -d -b all -v threadtime
tools/guest-logcat.sh <data>.run -d -s keystore2               # any logcat arguments
```

- A service's stdout and stderr (the layer's own messages: unimplemented
  syscalls, fatal signals with the faulting module) are in
  `<data>.run/logs/<service>.log`.
- Fatal signals in host code are symbolized there, for example
  `_platform_memmove+0x1bc (libsystem_platform.dylib)`.
- `tools/guest-shell.sh <data> [COMMAND]` is the guest's shell as adbd
  runs it: root, in the boot's pid namespace, with its binder and init's
  global environment (`PATH`, `BOOTCLASSPATH`, `ANDROID_*` and the rest
  of `<data>.run/environ`), so `app_process` tools (`uiautomator`,
  `monkey`, `am instrument`) start. `aimctl shell` is the same for an
  aimctl guest. `tools/guest-shell.sh <data> 'service list'` lists the
  registered binder services.
- A UI dump (the accessibility view tree with each view's text, id and
  bounds) of what the screen shows, for checks of Settings pages or of
  the page Chrome shows:

  ```
  tools/guest-shell.sh <data> 'uiautomator dump /data/local/tmp/ui.xml >/dev/null && cat /data/local/tmp/ui.xml'
  ```

  The XML is one line; `grep -o 'text="[^"]\+"'` lists the visible
  texts, in the guest's language (the Mac's).

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
| vendor.gatekeeper_nonsecure | running | registers `IGatekeeper/default` at its first start: `/bootstrap-apex` declares it before `apex.all.ready` |
| gatekeeperd | running | |
| credstore, vendor.identity-default | running | |
| vendor.health-aim | running | our health HAL |
| vendor.graphics.allocator | running | our allocator HAL |
| vendor.authsecret_default, vendor.cas-default, vendor.drm-widevine-hal, vendor.power-default, vendor.power.stats-default | running | vendor APEX HALs |
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
| lmkd | restarting (exit 0) | no PSI or memcg (P3 replaced it; see "Memory pressure") |
| netd | restarting (SIGABRT) | `libnetd_updatable_init`: no cgroup v2 directory; then eBPF and netlink (#202, #201; P3 replaces it) |
| audioserver | restarting (SIGSEGV) | null dereference after "Found no HAL version": needs the audio HAL (P5) |
| vold | excluded | the original drives block devices, dm-crypt and fscrypt (P3 replaced it: `daemons/vold`) |
| bpfloader (netbpfload) | excluded: exit 1, then init reboots (`reboot_on_failure`) | no bpffs at `/sys/fs/bpf` (#202) |
| surfaceflinger | excluded: SIGABRT | guest-init passes no `--gpu` (#204), so RenderEngine gets `EGL_BAD_DISPLAY`; with it, RenderEngine runs on ANGLE and SurfaceFlinger aborts with "failed to get hwcomposer service" (the composer is P4) |
| zygote | excluded | ART (P2) |

A crashing native process gets its symbolized backtrace in `logcat -b crash`,
and `debuggerd -b <pid>` prints a process's stacks: debuggerd's `crash_dump64`
runs on the layer's ptrace and cross-process `/proc` (#557). tombstoned writes
the tombstone to `/data/tombstones/tombstone_NN` (an `O_TMPFILE` it names with
`linkat`, #642); its memory map names only the guest's files (#597).

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
  activation (libvintf reads the vendor APEXes' VINTF fragments from
  `/apex` only then, and before it from `/bootstrap-apex`, which lists the
  `vendorBootstrap` APEXes), a lazy `aidl/apexservice` start no longer
  resets `apexd.status`, and `perform_apex_config --bootstrap` loads the
  scripts of `vendorBootstrap` APEXes (the gatekeeper HAL, class
  `early_hal`).
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
- `/proc` lists thread-group leaders only, and a value written under
  `/proc/<pid or tid>` goes with its process or thread (#378).
  `/proc/<pid>` belongs to the process's effective ids and its `status`
  has its credentials and capabilities from the process table (#377).
  Signals from another guest process carry its namespace pid and guest
  real uid; a child stays in the table as a zombie until a parent in the
  namespace reaps it, so SIGCHLD and `waitid` report its uid (#383).
- The process table is the guest's pid namespace for every call that
  names a process, not only `/proc` (#341): `kill` (also `-1` and process
  groups), pidfds, the scheduler and priority calls and the rest reach
  guest processes only, and a Mac process is ESRCH. `getppid` and
  `/proc/<pid>/stat` give 0 for a parent outside, such as guest-init
  (#364), and `sysinfo` counts the namespace's processes (#366). A `linux-run` started without a table (tests, a debugging
  shell) is alone in a private namespace with its descendants (#361);
  `--by-pid` joins a boot's. Signals, renicing and rescheduling another
  guest process follow the kernel's uid and capability rules (#362): an
  app gets EPERM for a system uid process, system_server's CAP_KILL and
  CAP_SYS_NICE reach apps. `prlimit` reads another guest process's limits
  under the same kind of rule (#363); setting them is EPERM.
- guest-init's wait for linkerconfig ends at `--timeout` and on SIGINT or
  SIGTERM (#196).

## Java world (P3, 2026-09-28)

The original zygote and system_server on the ART exception, on an M2 Pro.
`guest-init --run` with the derived image of `image/overlay.toml`: our Rust
apexd, vold, netd and lmkd (`daemons/`, the `daemon/*` nodes) replace
the originals (lmkd since the second part below; it kills on the Mac's
memory pressure since #277, see "Memory pressure").

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
  lmkd answers the protocol (and, since #277, kills on the Mac's memory
  pressure); the first boot went from 75–95 s to 25 s.
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
On 2026-09-29 it flagged a boot at 04:45:52, four minutes after boot
completion: every process that a release binary of that target
directory spawned (fork children, `sh -c`, zygote and service restarts)
was then SIGKILLed at start, with no kernel or crash report, and netd's
read-write opens of its BPF maps failed with EPERM (so netd aborted, and
its `onrestart` restarted zygote every 5 s). The same binaries run from
a shell, and other target directories' binaries, were not affected; by
05:27 the kills had stopped.

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
- #236 the ART codegen behind the null-page
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
| **Vulkan** | partial | `ro.hardware.vulkan=aim` loads `vulkan.aim.so` over MoltenVK (docs/vulkan-driver.md); `pm list features`: `android.hardware.vulkan.level` 0, `.version` 1.3, `.compute`; `dumpsys gpu`: `vulkanVersion = 4206592`. The NDK checks run in `tests/vulkan.rs` (two queues of one family, AHardwareBuffer and YUV images, sync-fd semaphores on the GPU). In a boot, the swapchain mode draws 100 frames, and with `debug.hwui.renderer=skiavk` (a test setting; the default stays GLES) Settings, Chrome and Clock draw with `Pipeline=Skia (Vulkan)`. |
| **Sensors** | passed | `dumpsys sensorservice`: Ambient Light Sensor (`android.sensor.light`) and Lid Angle Sensor (`android.sensor.hinge_angle`), vendor "Apple (darwin host)". `pm list features`: `android.hardware.sensor.light` and `.hinge_angle`, from the SKU guest-init reports (`ro.boot.product.vendor.sku` = `light_hinge`); no "cannot find light sensor" from DisplayPowerController (2026-09-30, #496). |
| **Thermal** | passed | `dumpsys thermalservice` (the native service, 2026-10-01): status 0, cpu 44.2 °C, battery 30.6 °C, skin NaN; no thermal HAL (#624). |
| **Health** | passed | `dumpsys battery`: level 80, AC powered, as `pmset -g batt` (80 %; AC attached). Temperature reads 0 (#242). |
| **Bluetooth** | passed | `dumpsys bluetooth_manager`: `enabled: true`, `state: ON`, crashed 0 times, over our HAL's virtual controller. No scan was run (TCC). |
| **GNSS** | passed | `dumpsys location` (the native service, 2026-09-30): `gps provider` enabled and allowed, identity `1000/android[GnssService]`; network and fused bound from Google Play services. No fix in boots whose host process has no CoreLocation authorization. |

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
  the same memfd. Chrome shows its first-run page.
- **Chrome aborted "Timed out waiting for GPU channel"** (#256) about 35 s
  after first run: `IChildProcessService.setupConnection` carries more than
  eight fds. Through the binder daemon a reader had only eight
  pre-reserved fd numbers, so the oneway call was dropped with `EMFILE`,
  and neither the GPU process nor the renderers ever started. Now the read
  stops before such a transaction and the shim reads again with enough
  (docs/binder-driver.md). Chrome shows its new tab page, and its GPU
  process keeps running.
- **Chrome's renderers died "V8 process OOM (Failed to reserve virtual
  memory for CodeRange)"** (#260) on every web page: V8 reserves its code
  range PROT_NONE and makes it RWX with `mprotect`, which Darwin refuses
  (EACCES) for anything but `MAP_JIT` memory. RWX memory is now `MAP_JIT`
  (ADR 0012, "Application JITs"). Chrome draws `https://example.com`, a
  `data:` page's script runs (`fib(30)` in 9–17 ms, optimized code), and
  the renderers stay up.
- **GMS persistent crash loop** (#336): Nearby's USB medium throws
  "UsbManagerCompat is unavailable" without the `usb` service, about once
  a second two minutes after boot (104 in a 2-minute window, 49 process
  starts). The device declares `android.hardware.usb.host` (the Mac's
  ports), so UsbService runs; none in three boots since. Declaring it
  alone killed system_server 48 times in 9.5 minutes ("Unable to open
  socket for UEventObserver"): init.usb.rc's writes had made
  `/sys/class/android_usb` appear, which sends UsbService down the USB
  gadget path. sysfs's device trees now hold only the modeled devices,
  and NETLINK_KOBJECT_UEVENT sockets work.

### Open

- #239 boot time and frame times after the spawned fork;
- #240 wide-gamut EGL configs; #241 phone and GMS startup ANRs; #242
  battery temperature; #230 traced aborts (traced_probes' were its memory
  watchdog reading four times its rss from `/proc`, now counted in the
  guest's 16 KiB pages).
- #258 remaining memfd seal gaps; #261 app data lost on a second boot of
  the same data directory.

## Network (2026-09-28)

Details in [network.md](network.md). The original EthernetService,
NetworkStack and DnsResolver bring up `eth0`, which stands for the Mac's
network: the layer's netlink and ioctls carry the configuration, and
`eth0`'s virtual router leases the Mac's address, gateway and DNS servers
by DHCP. First boot, `cargo aim boot`, on a Mac on Wi-Fi:

- `dumpsys connectivity`: `Active default network: 100`, Ethernet,
  `IS_VALIDATED`, LinkProperties 172.30.1.46/24 with the Mac's gateway and
  DNS servers. NetworkMonitor's HTTP and HTTPS `generate_204` probes
  answer 204. The DHCP exchange takes 72 ms; the network is validated
  1.7 s after the lease, 9.4 s after EthernetService starts and 6 s
  before `sys.boot_completed`.
- Shell: `ping -c 3 www.google.com` answers; an NDK program resolves
  `example.com` through DnsResolver and reads `HTTP/1.1 200 OK`.
- Chrome loads and draws `https://example.com` (#260 fixed the renderer).
  Its "No such process (3)" warnings (#257), DnsResolver's ESRCH for a
  network with no nameservers, are gone.

### Fixed on the way

- Java's `bind`/`connect` of AF_INET sockets failed with EINVAL: libcore
  tries a v4-mapped IPv6 address first and falls back on EAFNOSUPPORT,
  which Darwin does not return.
- UDP `connect` to port 0 (the "have IPv4/IPv6" probes of bionic and
  DnsResolver) failed, so every AI_ADDRCONFIG lookup found nothing.
- `SO_PROTOCOL` read 0, so libcore did not exempt UDP `connect` from
  StrictMode, and NetworkStack died of NetworkOnMainThreadException in
  DnsResolver's address sorting.
- `SO_MARK` failed, and with it every DnsResolver query; `SO_RCVBUF` 0
  failed (DhcpClient).
- The emulator's vendor overlay made `eth0` a restricted network; it goes
  from the derived image.
- DhcpClient's UDP socket took the Mac's port 68, so a second guest (or
  the NDK network tests) failed to bind it; the port is `eth0`'s (#334).

## Memory pressure (2026-09-29, #277)

lmkd (`daemons/lmkd`) keeps the original's protocol and kill order and
kills on the Mac's memory pressure, which the host-call module `memory`
reports (ADR 0012's lmkd row, [host-call.md](host-call.md)). A thread's
Linux scheduling sets its host QoS: a nice value of 10 or more or
SCHED_BATCH runs at utility, 19 or SCHED_IDLE at background.

Measured on a loaded host (load average 90, other agents' boots running),
`cargo aim boot` with a reused data directory, seven apps opened with
`am start -W` and then HOME:

- the Mac's level went to warn (`memory_pressure -l warn`) for 30 s; lmkd
  logged one kill a second, 48 in all, every one at oom_score_adj 900 to
  999, none below. `dumpsys activity lmk` counted the same 48, and
  `dumpsys activity exit-info` gave the killed deskclock `reason=3
  (LOW_MEMORY)`. When the level fell back, lmkd logged `memory pressure
  Warn -> Normal` and stopped.
- A dispatch memory-pressure source is not used: the kernel notifies only
  a few, large processes, and neither lmkd nor a plain host process got
  an event while the level read warn for a minute. The module polls the
  level every 250 ms instead.

What is not covered: ActivityManager's process groups
(`setProcessGroup`) change nothing on the host. Without cgroup
controllers libprocessgroup's background profile keeps only its timer
slack action, which has no process form, so the call fails in the guest
(#297). Kills name their process: another process's
`/proc/<pid>/cmdline` and `comm` come from its record (#238).

**Morning check** (one boot slot, after the throttle is lifted):

1. `cargo aim boot`, wait for `sys.boot_completed`, open several apps with
   `am start -W`, then `am start -a android.intent.action.MAIN -c
   android.intent.category.HOME`.
2. `dumpsys activity oom`: several `cch` processes (oom_score_adj >= 900).
3. `memory_pressure -l warn -s 5` on the Mac (real pressure; `-S` needs
   root).
4. `logcat -d -s lowmemorykiller`: `Kill ... oom_score_adj` lines only at
   900 and above (the highest registered first), one a second, then `memory pressure Warn
   -> Normal`; `dumpsys activity lmk` counts them.

## The Mac's settings (2026-09-29, #283, #282, #281)

Details in [mac-settings.md](mac-settings.md). The device takes the Mac's
time zone, languages with their region, and light/dark appearance:
aim-guest-init sets `vendor.aim.mac.*` before init's first action and
when the Mac changes them, and `init.aim.rc` applies them with
`persist.sys.timezone` and `persist.sys.locale` (the first language) in
`post-fs-data` and `cmd alarm set-timezone`; the native uimode service
follows the appearance itself (#655). The service host applies the
whole language list through the system_server bridge at boot and when
the Mac's list changes (#344). A first boot on a Mac on Asia/Seoul,
`ko-KR` and Light shows KST (the Mac's clock), `ko-rKR` and `notnight`,
and `system_locales` `ko-KR`; Settings draws in Korean.

## Storage images (2026-09-29)

The boot on the case-sensitive images of [storage.md](storage.md): the
system image (compressed, with its translation cache), the derived image
as its shadow, and the data directory as a data image. No boot
animation (`debug.sf.nobootanimation`), no `--exclude`. M2 Pro, with other
agents' builds and boots loading the host (load average 65–150), so the
times are not comparable with the sections above.

| Check | Result |
| --- | --- |
| First boot (empty data image) | `sys.boot_completed` after 48.8 s |
| Names that differ only in case in `/data` | `Foo` and `foo` coexist |
| `pm install -r -g` Chrome | Success; `/data/data/org.chromium.chrome` is 10212:10212 0700 |
| `am start -W -S` Settings (`.homepage.SettingsHomepageActivity`) | `Status: ok`, COLD, 2232 ms |
| `am start -W -S` Chrome | `Status: ok`, COLD, 2688 ms (first-run activity) |
| 1 GiB written to `/data/local/tmp`, deleted, stop | the image file went from 2.86 GB to 1.63 GB (1.44 GB used in its volume) |
| Second boot of the same data image | `sys.boot_completed` after 33–42 s; Chrome's data directory still 10212:10212 0700; Settings (6.7 s) and Chrome (5.5 s) start; no "Failed to prepare", `CANTOPEN` or "Ignoring missing CE app data dir" (#261) |

One second-boot run's Settings launch 1 s after `sys.boot_completed`
timed out (`am start -W`, 14 s); 20 s later it started, as did every
launch of the other runs.

### How to check it

```
cargo aim build                       # attaches _build/android16-image and target/aim/derived
cargo aim boot --data target/aim/boot/data
# in another shell, with the boot's binder (guest-init's pid):
linux-run --root target/aim/derived/root --path-map target/aim/boot/data.run/path-map \
    --binder dev.aim.guest-init.<pid>.binder /system/bin/sh -c \
    'echo a > /data/local/tmp/Foo; echo b > /data/local/tmp/foo; ls /data/local/tmp'
# a shell that should see and signal the boot's processes (ps, kill) joins
# its pid namespace; without --by-pid it is alone in a private one:
linux-run --root target/aim/derived/root --path-map target/aim/boot/data.run/path-map \
    --binder dev.aim.guest-init.<pid>.binder \
    --by-pid target/aim/boot/data.run/identity/by-pid /system/bin/ps -A
cargo aim storage                     # the images and what they occupy
```

`aimctl` ([aimctl.md](aimctl.md)) runs the same boot in the background
(`aimctl --data DIR start`), with `aimctl shell` in place of the linux-run
lines above.

After a stop, `target/aim/boot/data` is empty (detached); a second
`cargo aim boot` attaches the same image, and installed apps start with
their data. Look for a data image left attached by a crash with
`hdiutil info`; guest-init detaches it at the next start.

## Window mode (2026-09-29, #294)

`cargo aim boot --windows` ([windows.md](windows.md)): the display is the
Mac's main screen (3840×2160 at 2×, plus the bar margin: 3840×2352), the
default display runs freeform windowing, and the task bridge reports its
tasks. On an M2 Pro with other agents' boots loading the host (load average
37 to 125):

| Check | Result |
| --- | --- |
| Settings, Calculator (installed with `pm install`) and Chrome from `am start` | three native windows, each showing its task, no Android caption; a window the server shows itself is titled as the launcher names its task (Settings: "Settings", 2026-09-30), not with its package |
| A click in a window | one touch at the display pixel under it (point × 2); Settings opened the page clicked |
| Raising a window | `mCurrentFocus` becomes its task |
| Resize (Accessibility, 412×756 to 640×820 points) | the task's bounds 1280×1592 pixels; Chrome lays out for the width |
| Move | the task moves with the window |
| Cmd+[, mouse button 4, two-finger swipe right | Back (`KEY_BACK`; button 4 is now the mouse's `BTN_SIDE`); SubSettings back to Settings each time |
| Esc | `KEY_ESC` reaches the app; no Back (Android 16 closes system dialogs instead) |
| A task restored from recents at boot | no window |

Under that load SystemUI hit ANRs, and freeform positions Android chose
itself (Chrome's launch, Settings shifted away from Chrome when a page
opened) did not reach the task surfaces, so those windows showed other
parts of the display; the bridge now commits such bounds by moving the
task a pixel and back with `resizeTask` (a resize to the same bounds is a
no-op and did not help).

**Per-task composition** (2026-10-01, #692, [layers.md](layers.md)):
window mode now composes each Mac window from its own task's layers
(device composition in the composer HAL) instead of cropping one display
buffer. Fresh window-mode boots (ops checks layers-1, -2, -5): every app
layer `DEVICE` in `dumpsys SurfaceFlinger` (6 of 6, none `CLIENT`); Clock
overlapping Settings (about 80 %) and Chrome (its left part), in either
stacking order, each window showed only its own app; Chrome's first-run
page filled its window (no dark strip, #691), and a force-stopped Chrome
relaunched into a new window within 1 s; a Clock task moved by `am task
resize` took its window and content along with no panel left behind;
`input tap` in the Clock window switched its tab. No composition errors,
SurfaceFlinger aborts or ANRs in logcat. Device mode stays client
composition (`CLIENT`). On a first boot PackageManager installs GMS about
30-40 s after boot completes; a Chrome running then kills itself
(`DynamiteLoaderV2Impl: Module config changed, forcing restart`) and its
window closes with its task.

**App shims** (same boots): `aim-apps shims --watch` wrote 19 shims into
`target/aim/boot/apps`, the packages `cmd package query-activities -a MAIN
-c LAUNCHER` lists (Gboard's launcher activity, disabled at run time, and
GMS's, disabled by a resource, left out). Opening Calculator.app,
Settings.app and Chrome.app started each app in its own process with its
name and icon in the Dock; a click in the Calculator shim's window, behind
Chrome's, focused its task and typed 7, and keys typed 5 5.

Window-mode boots of 2026-09-30 (#352, #354, #356, #357):

| Check | Result |
| --- | --- |
| Shims per launcher activity | 22 shims; the Google app has two, "Google" (primary, `dev.aim.app.com.google.android.googlequicksearchbox`) and "Voice Search" (its own bundle identifier); each connects as the host of its activity |
| Server window | `am start` of Settings with no shim running: a window of the server titled "Settings"; opening Settings.app moved the task into the shim's window |
| Server in the Dock | none: `lsappinfo` type `UIElement`; the Dock shows the shims' icons (Google, Voice Search, Clock with its hands at 10:10) |
| Uninstall | `pm uninstall` of Calculator (its shim open): the bundle removed within 4 s, its host exited, and Launch Services no longer lists it; at the end of the boot `aim-apps clean` left no bundle or registration of `target/aim/boot/apps` |
| system_server restart | none in five boots on one disposable data image (2026-09-30); before, every boot after the first restarted zygote (#490): system_server reached BiometricService before gatekeeperd, whose HAL's first start had aborted, and died ("Gatekeeper service not available"), and zygote killed itself with it. The task bridge still restarts with zygote (`init.svc.zygote=restarting`) |

VoiceSearchActivity opened no window of its own (no freeform task with
bounds was reported for it).

Window-mode boots of 2026-10-01 (#463):

| Check | Result |
| --- | --- |
| Splash (D4) | opening YouTube.app cold: its window with the YouTube icon on YouTube's splash color (white; Clock's black), YouTube's UI in it about a second later. The host shows the splash before it sets up its renderer, notification center and connection (#605): 0.22-0.26 s from `open` to the window on screen on an idle Mac, against 0.28-0.31 s before (a probe shim); the first recording's 1.1 s was at load 7. The color is the launcher activity's `windowSplashScreenBackground` or `windowBackground`, light and dark (#606) |
| HOME (D6) | `am start -c HOME` over Settings: the empty home in front, Settings' window gone from the screen (a window of the server: minimized; a shim's app: hidden) |
| The lightweight shell, the image in both modes since 2026-10-01 ([m1-shell.md](m1-shell.md)); measured before as a check-only image against the one with SystemUI | boots to `sys.boot_completed` (fresh data 5.7 s, repeat 4.6-5.8 s, against 5.9 s and 4.7-4.9 s with SystemUI); HOME is the device's own empty `dev.aim.home/.Home`, with no Settings `FallbackHome` restarts (the platform's `SystemUserHomeActivity` is never resolved as home, #643); no SystemUI, launcher or wallpaper picker process; `googlequicksearchbox:search` runs, bound by the default assistant's `:interactor` (#604); the overlay's services absent; standard Mac title bars (caption 0, #545); its own static wallpaper (`java/image-wallpaper`) in ImageWallpaper's place: CtsWallpaperTestCases, whole module in two shards, 115 pass / 6 fail against the default image's 117 / 4, the same four plus the two `_onLockScreen` visibility tests, which need a keyguard the shell has none of (#650); CtsNotificationTestCases' NotificationManagerTest 111/0/4 and NotificationManagerZenTest 71/0/1 as with SystemUI, in 2.5x the time (#680), and its bubble tests fail or hang without bubbles (#679); screen pinning (`am task lock`) shows a pin in the menu bar of the task's shim, or of the display server for a task without one, and `am task lock stop` (the call its Unpin makes) removes it; AimHome and AimImageWallpaper run from the image's odex (`pm art dump`: verify, prebuilt) |

**Notifications** (2026-09-30, #4, [notifications.md](notifications.md)):
guest-init's notification bridge registers with NotificationManagerService
once it is published; the first boot's notifications ("Android is
starting", Play Store's) appeared in Notification Center from the "Android
System" and Play Store shims, which the display server opened in the
background, and `cmd notification post` from the shell uid from "Android
System". CtsNotificationTestCases' NotificationManagerTest passes as
without the bridge (114 of 114). Custom views are read past (Clock's
timer keeps its actions), resource and `file:` icons are drawn on the Mac,
and full-screen intents launch while the Mac is locked (not exercised on a
locked Mac).

**Media** (2026-10-01, #463, [media.md](media.md)): guest-init's media
bridge follows the media session Android's media keys go to and publishes
it as the Mac's Now Playing from the app's shim (VLC: `now playing
org.videolan.vlc (Playing)`, then `nothing` after a force-stop). Screen
capture requests start the device's consent activity
(`/system/app/AimMediaProjection`, named by the framework overlay in place
of SystemUI's), which asks with a sheet on the app's window and creates
the projection through MediaProjectionManagerService. The Mac's Now
Playing UI and an answer to the sheet were not exercised (no clicks);
CtsMediaProjection* wait for SystemUI's dialog (#632).

## Pointer, scrolling and shortcuts (2026-09-29, #214, #288)

A window-mode boot of the derived image with the mouse device
([input.md](input.md)); the input went through the server's path for a
window host's records (`hosts::apply`, the calls `aim-display`'s AppKit
handlers make), not through posted AppKit events.

| Check | Result |
| --- | --- |
| `dumpsys input` | `aim-mouse`: classes `TOUCH`, Touch Input Mapper in `POINTER` mode, sources `MOUSE`, X 0–3839 and Y 0–2351, `VSCROLL` and `HSCROLL`; `disable_touch_input_mapper_pointer_usage` unset (the pointer-usage path) |
| Hover over Settings | the row under the pointer highlights |
| 15 trackpad deltas of 40 pixels up | Settings scrolls; `getevent -lt`: `REL_WHEEL_HI_RES` −37/−38 each, a whole `REL_WHEEL` −1 every third |
| A diagonal gesture up and left (20 and 40 pixels per event) | both axes: `REL_WHEEL_HI_RES` −18/−19 and `REL_HWHEEL_HI_RES` +37/+38 per event, its first 8 points' horizontal part sent once decided |
| Right click in the Settings search field | the text field's context menu (undo, select all, autofill) |
| Typing, Cmd+A, Cmd+C, Right, Cmd+V | `KEY_LEFTCTRL` around `KEY_A`, `KEY_C`, `KEY_V`; the field reads "ifiifi" |
| Pinch out with rotation (12 steps of +5 % and 3°) | Pointer location shows two pointers spreading along arcs |
| A two-finger swipe right, then momentum | `KEY_BACK` down/up only, no `REL_HWHEEL`; SearchActivity back to Settings |
| The pointer sprite | `CURSOR` in SurfaceFlinger's HWC layers; absent from the presented frame (the display server's capture), present in `screencap` |

**The Mac cursor, smart zoom and text shortcuts** (2026-09-30, #387,
#391, #394): a window-mode boot, a window host of the server's protocol
sending the records `aim-display`'s handlers send and reading the cursor
records every host gets; Settings' search field (SettingsIntelligence,
Gboard as the input method).

| Check | Result |
| --- | --- |
| The mouse over the window, then over the field, resting at each | the host gets the arrow (48x48, hot spot (9, 7): its tip) and over the field the I-beam (48x48, hot spot (24, 22): its middle), each time the pointer crosses; typing hides the pointer, and the host gets the default cursor |
| Typing "hello world", then a double tap (smart zoom's four touches) on "hello" and z | "z world": the double tap selected the word |
| Cmd+Right, Option+Delete | "z ": the word before deleted |
| abc, Option+Left, x | "z xabc" (keys a third of a second apart; with no gap Gboard applied its composed letters after the move) |
| Cmd+Delete | "abc": deleted to the line's start |
| Cmd+Left, Cmd+Shift+Right, q | "q": the selection replaced |

A device-mode boot then, keys written into the keyboard as the display
server sends them:

| Check | Result |
| --- | --- |
| The Mac on 2-Set Korean (`2SetHangul`) | `vendor.aim.mac.keyboard_layout` `keyboard_layout_english_us`; `aim-keyboard` set it (logcat) |
| G K S R M F in the Settings search field, Gboard on Korean | `KEY_G` … `KEY_F` in `getevent -lt`; the field reads 한글 |
| Ctrl+Space, then the layout set to `keyboard_layout_english_us_dvorak` (as the Mac's Dvorak would) | Gboard on English; Q W E R T Y and H J K L type `',.pyf` and `dhtn` |
