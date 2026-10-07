# Host-side CTS (Tradefed, #701)

CTS modules come in two kinds. A device-side module is an instrumentation
APK; it runs here with `pm install` and `am instrument` in the guest's shell
(system-services.md, "Conformance"). A host-side module (`JarHostTest`,
`HostTest`) is a jar that runs on the Mac inside Tradefed, the release's
harness, and drives the device over adb: installs with flags, `adb shell`
commands, pushes and pulls, reboots. For M4, 22 of PackageManager's 52
modules are host-side (m4-packagemanager.md, "CTS").

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

To pin another module, list its entries with `tools/cts-module.py --list
testcases/MODULE/`, add each with the sha256 of its inflated content to
`upstream/cts-tradefed.lock`, and fetch.

## Status (2026-10-02)

Measured in device-mode boots with a disposable data directory (ops
checks m4tf-1 to m4tf-5): adb connects and is authorized; `adb shell`,
`exec-out`, pty shells, push and pull work; `adb reboot` boots again with
the same guest-init pid and adbd back in 6 s. No module has produced
results yet: an install crashed system_server once (#784), and after a
reboot a module waited on `/sdcard`, which the emulated volume does not
provide without MediaProvider's FUSE (#221).
