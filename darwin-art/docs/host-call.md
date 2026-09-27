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
| Host modules | `crates/darwin-host-<name>` (first: `darwin-host-health`) |
| HAL services and their build | `hal/`, `tools/build-vendor-hals.sh`, `hal/sources.lock` |
| Placement in the derived image | `image/overlay.toml` |

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
2. **AIDL.** For each stable interface, the frozen API directory
   (`aidl_api/<package>/<version>`) is fetched. Its `.hash` is recomputed
   with Soong's freeze recipe (sha1 of the sorted per-file sha1 lines and the
   previous version). The SDK build-tools `aidl --lang=rust --structured
   --stability=vintf --version --hash` then generates the code, and the
   script writes the crate glue Soong's `aidl_rust_glue.py` would. The glue
   has the package module tree and the crate-level `mangled` namespace the
   generated code refers to. Nothing generated is checked in.
3. **Build.** cargo builds with the NDK clang as linker and installs each
   service under the name in its `[package.metadata.vendor-hal] binary`
   into `_build/vendor-hals/bin`. `image/overlay.toml` places it in the
   derived image.

Tools: the Android NDK (clang, sysroot and libclang for bindgen), SDK
build-tools 36 or later (`aidl`), and `rustup target add
aarch64-linux-android`.

To add a HAL:

- add an `AIDL_INTERFACES` line, after the interfaces it imports, listing
  them as `package:version` (the script puts them on the include path and
  re-exports their `mangled` items, as Soong's glue does);
- add a manifest under `hal/aidl/<package>` with `build =
  "../../binder/build.rs"`, since the binder macros expand there;
- add a service crate with its `.rc` and vintf fragment (a driver library
  loaded in-process, like the mapper or the GLES driver, is a `cdylib` whose
  `[package.metadata.vendor-hal] library` names the installed file in
  `_build/vendor-hals/lib`);
- add `[[add]]` entries to the overlay, plus `[[remove]]` for an emulator
  HAL of the same instance.

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

**Verified** (without the binder wiring, #167/#168):

- `tests/hostcall.rs` covers the lean entry, the traced full path and the
  `brk` fallback;
- the HAL, in a derived image assembled with `android-image assemble`, runs
  under `linux-run`, from load-time rewriting and from the translation
  cache. The original linker64 and bionic load it with the image's
  libbinder_ndk. Its host call returns the live battery (`present=true
  level=80% status=NOT_CHARGING ac=true`). libbinder then stops at
  `Opening '/dev/binder' failed`.

Rust std checks the standard fds with `ppoll` at start-up, so every Rust
guest program needs `ppoll` from the syscall layer. The verification run
used a local stand-in.
