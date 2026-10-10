# Host-side CTS (Tradefed, #701)

CTS modules come in two kinds. A device-side module is an instrumentation
APK; it runs here with `pm install` and `am instrument` in the guest's shell
(system-services.md, "Conformance"). A host-side module (`JarHostTest`,
`HostTest`) is a jar that runs on the Mac inside Tradefed, the release's
harness, and drives the device over adb: installs with flags, `adb shell`
commands, pushes and pulls, reboots. The M4 table explicitly lists 23 host-side modules (m4-packagemanager.md,
"CTS"). All 23 canonical configs in Android 16 r1 use `HostTest` or
`JarHostTest`; the former 52-total/22-host count was inaccurate (#1063).

They run against the guest as against a device: the release's own Tradefed
talks to the image's own adbd. Nothing on either side is replaced or faked.

## What the device provides

| Tradefed needs | The device | Where |
| --- | --- | --- |
| an adb transport | the image's adbd (`com.android.adbd`), started by init as on a device with USB debugging on; with no USB gadget it listens on TCP, on the Mac's loopback only (guest sockets are host sockets, network.md) | `init.aim.rc`, "The debug port" |
| a known port | `--androidboot aim.adb.port=PORT` (bootconfig `ro.boot.aim.adb.port`), else the emulator's 5555; a named port also turns USB debugging on (`persist.sys.usb.config` `adb`, as Developer options would), so init.usb.rc starts adbd on every boot and AdbService reports debugging enabled | the same |
| authorization | adbd checks the keys in `/data/misc/adb/adb_keys` ("Always allow from this computer"); `tools/cts-tradefed.sh` adds the Mac's adb key there with the guest's root shell, since the lightweight shell has no dialog to ask with | `tools/cts-tradefed.sh` |
| `adb shell`, `getprop`, `pm`, `cmd`, `dumpsys`, push and pull, installs | adbd's own services, as the shell user, in the boot's pid namespace with its binder; every service without the shell protocol (ddmlib's shell, `exec-out`, `reboot`, the abb process) runs on a pseudo-terminal (linux-run's, over the Mac's ptys, #745), and a streamed install copies through `splice` (#788) | the image |
| `adb reboot` | adbd sets `sys.powerctl` to `reboot,adb`; guest-init stops the services, detaches the data image and boots again in the same process (same pid and binder name), as the kernel restarts the machine; `shutdown,...` ends the boot | `guest-init.rs` |

A reboot keeps the boot's keeper: `cargo aim boot` and `aimctl` wait on
guest-init's pid, which a reboot keeps. The adb connection drops during the
reboot, and Tradefed reconnects (`adb connect`) once adbd listens again.

## Running a module

Inputs: `python3 tools/cts-module.py --tradefed` fetches the entries of
`upstream/cts-tradefed.lock` (the harness's jars and scripts, and the pinned
host-side modules) from the release into `_build/cts-tradefed`, at their
paths in the archive, where Tradefed looks for them. The JDK and aapt2 are
the repository's (`_build/java`, build.md, "Java") except that host test
jars require JDK 21, supplied by Android Studio's JBR or Homebrew
`openjdk@21`; `adb` is the Mac's Android SDK Platform-Tools, on `PATH`.

```sh
cargo aim build
cargo aim boot --data DATA -- --androidboot aim.adb.port=5611
# once sys.boot_completed is 1:
tools/cts-tradefed.sh DATA 5611 -m CtsPackageSettingHostTestCases
```

`tools/cts-tradefed.sh DATA PORT [option]...` starts an adb server of its
own on PORT + 1 (never the Mac's default one), authorizes the Mac's key,
connects to `127.0.0.1:PORT`, and runs `cts-tradefed run commandAndExit cts
-s 127.0.0.1:PORT --skip-device-info --skip-preconditions` with the
remaining options (`-m MODULE`, `-t CLASS#test`, ...): the classic console
(`USE_ATS=false`), with Tradefed's usage statistics off
(`DISABLE_CLEARCUT=1`). Results and logs are in
`_build/cts-tradefed/android-cts/results` and `logs`. The device-info and
precondition steps are skipped because their APKs are not fetched; neither
changes what a module tests.

The lock now includes all 23 host modules listed in the M4 table: their
complete official module directories, including test APKs, signing sidecars
and the staged-install tools, plus every explicit config file dependency.
There are 375 module files, one shared BackupPreparer helper and eight harness entries (384 pins), occupying
about 646 MiB for the module inputs. These inputs have verified hashes;
fetching them does not establish a module pass. The separate device-side
inputs remain in `upstream/cts.lock`.

To pin another module, list its entries with `tools/cts-module.py --list
testcases/MODULE/`, add each with the sha256 of its inflated content to
`upstream/cts-tradefed.lock`, and fetch.

## Input status (2026-10-08)

All 386 host/harness pins and all 1,065 device-input pins are present and
hash verified. The relevant C device/permission cohort has 43 modules (42
instrumentation modules and one native GTest), with 934 archive paths. The
Secure FRP test APK already embeds its install helper (#1087). The official C parsing host module has completed twice with zero passed and
eleven failed tests, retained in the 2026.10.08_14.49.54 and
2026.10.08_15.09.01 result directories. A C57 diagnostic-option trial failed
configuration and counts as not run; an unchanged-harness single-test replay
fails original permission PREPARE. C67 parsing XML records five passed and six failed tests with the module
complete (2026.10.08_16.45.04); installation/context ownership failures remain.
C69 official parsing XML confirms eleven passed, zero failed and module complete
(2026.10.08_17.02.43). Full native C acceptance remains incomplete.
Runs retain Tradefed results and logs as acceptance evidence.

The separate template-parity gate in [first-boot.md](first-boot.md), #620,
uses the official `CtsBootStatsTestCases` host module. Its config and JAR
are pinned and hash verified in `upstream/cts-tradefed.lock`; the config
requires only that JAR and the existing `JarHostTest` harness, with no
APK or file-push inputs. It is not added to the PM 66-module manifest.
Run it separately on the original first boot and native template first boot
with `tools/cts-tradefed.sh DATA PORT -m CtsBootStatsTestCases`, retaining
both official XML results. Input preparation is complete; this gate has
not been executed. The original `BootStatsHostTest.testBootStats` clears
the events log, reboots the device through Tradefed and reads the boot
atoms with `cmd stats print-stats`; run it on the disposable gate device
after preserving the first-boot measurements.

## Original device transport checks (2026-10-02)

Measured in device-mode boots with a disposable data directory (ops
checks m4tf-1 to m4tf-5): adb connects and is authorized; `adb shell`,
`exec-out`, pty shells, push and pull work; `adb reboot` boots again with
the same guest-init pid and adbd back in 6 s. No module has produced
results yet: an install crashed system_server once (#784), and after a
reboot a module waited on `/sdcard`, which the emulated volume does not
provide without MediaProvider's FUSE (#221).

Official campaign comparison verifies each retained XML's campaign-local path and
recorded SHA before parsing. Test outcomes remain visible even when wrapper or
invocation validation failed; XML completeness, wrapper issues and assumption
failures are reported separately and still prevent acceptance. The comparator
requires complete same outcomes and all native tests passed, so evidence reporting
does not waive a failed or unexecuted gate. Six fixture regressions cover this
separation, tampering and foreign modules.
