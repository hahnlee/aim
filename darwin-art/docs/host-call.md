# Host-call and vendor HALs (ADR 0012, boundary 2)

Guest code reaches host implementations through host-call, the way a Wine
`.drv` reaches its Unix side. The users are our vendor HALs: Rust services
for `aarch64-linux-android` that run from `/vendor/bin/hw` like any vendor
HAL. They speak AIDL over the original `libbinder_ndk.so` and call a host
module (Rust, linked into the syscall layer) for CoreAudio, IOKit, Metal and
the like.

| Piece | Where |
| --- | --- |
| ABI: numbers, argument blocks, guest wrappers, module shape | `crates/darwin-hostcall` (`no_std`) |
| Dispatch and module registry | `crates/darwin-linux-abi/src/hostcall.rs`, `Lhostcall` in `trampoline.S` |
| Host modules | `crates/darwin-host-<name>` (first: `darwin-host-health`; `darwin-host-sensors` serves two modules) |
| HAL services and their build | `hal/`, `tools/build-vendor-hals.sh`, `tools/lib/vendor_hal_aidl.py`, `hal/sources.lock` |
| Placement in the derived image, and the emulator HALs removed | `image/overlay.toml` |

## The call

```text
x8 = 0x4843_0000 ("HC" << 16)    x0 = module id    x1 = function id
x2 = argument block address      x3 = argument block length
svc #0                           -> x0 = result >= 0, or -errno (Linux values)
```

- **Mechanism: a reserved Linux syscall number.** The translator already
  turns every `svc #0` into a branch to a per-site stub (ADR 0012 decision
  8), in translated files, load-time rewrites and JIT code alike. So a host
  call needs no translator change, no new stub kind and no new TSD slot:
  the syscall entry compares x8 once more before its full path. Two
  alternatives were rejected:
  - an `svc` with a distinctive immediate would need a new rewrite kind and
    a translator version bump for no speed gain;
  - a host function pointer handed to the guest and called with `blr` would
    save about 2 ns, but it needs per-process setup and would run host code
    on guest stacks.

  On a real Linux kernel the number is `ENOSYS`, so a guest can probe for
  the host.
- **Register contract: an AAPCS64 call, not a syscall.** Only x0 carries a
  result. x1–x17, v0–v7, v16–v31, the upper halves of v8–v15 and NZCV may be
  clobbered; x19–x29, sp and d8–d15 are kept. `darwin_hostcall::guest::call`
  declares exactly this (`clobber_abi("C")`). The entry therefore saves
  nothing but x19/x20 (on the guest stack), switches to the thread's host
  stack and calls `linux_abi_hostcall` as a C function.
- **Cost (M2 Pro, `examples/boundary_probe`):** 4.6 ns per call against
  19.6 ns for a syscall on the full path. `--trace` runs and `brk` fallback
  sites go through the full path and the ordinary dispatcher, with the same
  result.

## Versioning

- `ABI_VERSION` (module 0, `core`) versions the calling convention itself.
- Module ids are fixed like syscall numbers: never reused or renumbered.
  Function 0 of every module returns the module's version, answered by the
  registry. An unknown module or function returns `-ENOSYS`.
- An argument block is a `#[repr(C)]` struct whose layout never changes once
  shipped. The host requires `len` to be exactly its size (`-EINVAL`
  otherwise), so a stale guest fails cleanly. A module evolves by adding
  functions and bumping its version; a guest checks the version it needs at
  start-up.

## Guest memory rules

Guest and host share one address space, so a guest address is a host
address and nothing is copied.

- The host touches guest memory only through the argument block and the
  pointers it holds, each with an explicit length. It checks the block's
  size, non-null address and alignment (`darwin_hostcall::args_mut`).
- Pointers are valid for the duration of the call only. A module never keeps
  a guest pointer; it copies what it must keep.
- A bad pointer is not turned into `EFAULT`. The fault is delivered as for
  any guest fault. Callers are our own HAL code.
- Calls come concurrently from any guest thread, so modules are `Sync`. A
  call may block like a blocking syscall, but it is not interrupted by
  signals and has no `EINTR`. Long waits belong on an fd the guest polls.
- Host code never calls back into guest code. Asynchronous events reach the
  guest through an fd (eventfd, pipe or socket) that the HAL's looper
  watches.
- The call runs on the thread's host stack (1 MiB), which the syscall full
  path also uses. A guest signal handler must not make a host call while its
  thread is inside one.

## Host modules

A module is a Rust crate exporting a `darwin_hostcall::HostModule` (id, name,
version, `call`). It is linked into the syscall layer and listed in
`MODULES` in `hostcall.rs` at the index of its id; a const assertion checks
the order. Static linking was chosen over dylibs loaded at run time: there is
no loader, no search path and no symbol versioning, and a module is one crate
dependency. Module ids and argument blocks are added to `darwin-hostcall`
first.

| Id | Module | Version | Functions |
| --- | --- | --- | --- |
| 0 | core | `ABI_VERSION` = 1 | (version only) |
| 1 | health | 1 | `FN_BATTERY`: fill `health::Battery` |
| 2 | gpu | 1 | `FN_INIT`, `FN_IMPORT_BUFFER`, `FN_PRESENT`, and the generated EGL/GLES entry points from `FN_TABLE_BASE` ([gles-driver.md](gles-driver.md)) |
| 3 | display | 1 | `FN_CONNECT` (returns the event fd), `FN_IMPORT`, `FN_PRESENT`, `FN_RELEASE`, `FN_SET_VSYNC` ([composer.md](composer.md)) |
| 4 | thermal | 1 | `FN_READ`: fill `thermal::Thermal` ([vendor-hals.md](vendor-hals.md)) |
| 5 | sensors | 1 | `FN_READ`: fill `sensors::Readings` ([vendor-hals.md](vendor-hals.md)) |
| 6 | location | 1 | `FN_START`, `FN_STOP`; `FN_READ`: fill `location::Fix` ([vendor-hals.md](vendor-hals.md)) |
| 7 | audio | 1 | `FN_DEVICES`; `FN_OPEN` on a ring memfd, `FN_START`, `FN_STOP`, `FN_CLOSE` ([audio.md](audio.md)) |
| 8 | bluetooth | 1 | `FN_OPEN` (returns the wake fd), `FN_SEND`, `FN_RECV`, `FN_CLOSE`: HCI packets to and from the virtual controller ([bluetooth.md](bluetooth.md)) |

## Vendor HAL build pipeline

`tools/build-vendor-hals.sh` builds the `hal/` cargo workspace for
`aarch64-linux-android`. All its inputs are pinned in `hal/sources.lock` and
verified:

1. **AOSP Rust binder.** `libbinder_rs` and the `libbinder_ndk` headers are
   fetched at the image's tag (`android-16.0.0_r1`) and checked against a
   content hash. `hal/binder-ndk-sys` and `hal/binder` compile these
   unmodified sources the way Soong builds a vendor module: bindgen with
   `-D__ANDROID_VENDOR__ -D__ANDROID_VNDK__` and the flags of
   `libbinder_ndk_bindgen_flags.txt`, and `--cfg android_vendor --cfg
   android_vndk`. The binary links the image's own `libbinder_ndk.so`; libc,
   libdl, libm and liblog come from the NDK sysroot (the same LL-NDK ABI).
2. **AIDL** (`tools/lib/vendor_hal_aidl.py`). Each stable interface is
   pinned at one frozen version with the imports of that version
   (`versions_with_info` in its `Android.bp`).
   - Its frozen API directory (`aidl_api/<package>/<version>`) is fetched and
     checked as Soong's `aidlVerifyHashRule` does: the sha1 of the sorted
     per-file sha1 lines and the previous version (`latest-version` for V1)
     must equal the last line of `.hash`.
   - The pinned SDK compiler (build-tools 36.0.0, checked by sha256) runs
     `aidl --lang=rust --structured --stability=vintf --version --hash` with
     `-I` the frozen APIs of the imports, transitively, as Soong's
     `rust_aidl` does. Its command line, quoted in every generated file, has
     only relative paths, and files are rewritten only when they change, so
     the output depends on the pinned inputs alone and cargo rebuilds only
     what changed.
   - The crate glue is `aidl_rust_glue.py`'s: the package tree under `aidl`
     (`android_hardware_health::aidl::android::hardware::health`), and a
     `mangled` namespace that re-exports the imported crates' `mangled`,
     through which the generated code names imported types. Crates depend
     on `binder`, `async-trait` and `static_assertions`, as Soong's do.
   - Nothing generated is checked in.
3. **Build.** cargo builds the whole workspace (every interface crate, used
   or not) with the NDK clang as linker. It installs each service under the
   name in its `[package.metadata.vendor-hal] binary` into
   `_build/vendor-hals/bin`. `image/overlay.toml` places it in the derived
   image. A HAL's test client (`test` in the same table) goes to
   `_build/vendor-hals/test`, which crate tests copy into the guest's
   `/data/local/tmp`; it is never placed in the image.

The interfaces P4 and P5 need build: power V6 (imports common.fmq V1),
graphics composer3 V4, allocator V2 and common V6 (with drm.common V1),
audio.core V3 (with audio.common V4, audio.effect V3, audio.core.sounddose
V3, media.audio.common.types V4 and media.audio.eraser.types V1), sensors
V3, bluetooth V1, health V4, thermal V3 and gnss V2, over common V2 and
common.fmq V1.

Tools: the Android NDK (clang, sysroot and libclang for bindgen), SDK
build-tools 36.0.0 (`aidl`), and `rustup target add aarch64-linux-android`.

To add a HAL:

- add an `AIDL_INTERFACES` line for its interface and for each import not
  yet listed;
- add a manifest under `hal/aidl/<package>` (a copy of another) with the
  frozen version and a path dependency on each import's crate; the
  generator checks that these match the lock;
- add a service crate with its `.rc` and vintf fragment (a driver library
  loaded in-process, like the mapper or the GLES driver, is a `cdylib` whose
  `[package.metadata.vendor-hal] library` names the installed file in
  `_build/vendor-hals/lib`);
- add `[[add]]` entries to the overlay, plus `[[remove]]` for an emulator
  HAL of the same instance.

## The vendor partition

The derived image's `/vendor` is the emulator's minus what a Mac cannot
back, plus our HALs. The overlay removes the `.rc` and vintf fragment of
each emulator vendor HAL that talks to QEMU or fakes hardware, so the
instance is undeclared and the original services take their no-HAL paths:

- replaced by ours: the graphics allocator, mapper and GLES driver
  ([graphics-buffers.md](graphics-buffers.md),
  [gles-driver.md](gles-driver.md));
- replaced by ours: thermal (the vendor APEX's example), sensors and GNSS
  ([vendor-hals.md](vendor-hals.md));
- replaced by ours: audio, whose HIDL audio-effect declaration in
  `manifest.xml` also goes ([audio.md](audio.md));
- replaced by ours: Bluetooth ([bluetooth.md](bluetooth.md));
- replaced by ours later: the composer and camera;
- hardware the device does not have: radio, Wi-Fi (with hostapd and the
  supplicant), fingerprint, USB, lights, storage health, the goldfish
  Codec2 store, and the vendor APEXes contexthub, rebootescrow, Thread, UWB
  and vibrator.

Software HALs with no hardware behind them stay: ClearKey DRM, KeyMint
(with secure clock and shared secret), identity credential, and the vendor
APEXes authsecret, cas, dumpstate, gatekeeper, neuralnetworks, power
([vendor-hals.md](vendor-hals.md)) and widevine. `android-image diff` lists
every entry with its reason.

## First HAL: `android.hardware.health` V4

`/vendor/bin/hw/android.hardware.health-service.darwin` registers
`android.hardware.health.IHealth/default` with `AServiceManager_addService`
and joins the binder thread pool.

- **Where the values come from.** The host module `health` reads the Mac's
  battery: level, status, AC and times from `IOPSCopyPowerSourcesInfo`, and
  charge (mAh), cycle count and voltage from the `AppleSmartBattery`
  registry entry. Current macOS no longer publishes battery temperature
  there, so it reads 0.
- **Callbacks.** They get the state on registration, on `update()` and
  every 60 s (AOSP's fast periodic chore), since there are no power-supply
  uevents.
- **Unsupported.** What a Mac cannot report is `UNSUPPORTED_OPERATION`, as
  in AOSP's default implementation without the sysfs node: storage info,
  disk stats, the energy counter, charging policy, battery health data and
  hinges.
- **Replacing the emulator's HAL.** The emulator's
  `android.hardware.health-service.example` reads `/sys/class/power_supply`.
  Its `.rc` and vintf fragment are removed in the overlay, so only ours
  serves the instance.

**Verified** (2026-09-27):

- `tests/hostcall.rs` covers the lean entry, the traced full path and the
  `brk` fallback.
- End to end on the derived image (`android-image assemble`, which takes
  the original's identity from `android16-image-full.identity` beside the
  extracted tree): `guest-init --run --only
  servicemanager,vendor.health-darwin` starts the binder host and both
  services with `linux-run --binder`.
  - The original servicemanager finds `IHealth/default` in the device VINTF
    manifest, and the HAL registers it.
  - The original `service list` shows `android.hardware.health.IHealth/default:
    [android.hardware.health.IHealth]` beside `manager`, and `service
    check` finds it.
  - `service call ... 7` (`getCapacity`) returns 80, the Mac's battery
    level.
  - hwservicemanager is not needed.
- The syscall layer plays binderfs: `/dev/binderfs/<device>`, where init's
  symlinks point `/dev/binder` and the others, is the binder device.
- A service without a `seclabel` is in `u:r:init:s0` (contract section 4).
  guest-init does not yet compute a service's domain from its executable's
  file context, so the HAL runs as `u:r:init:s0`.
