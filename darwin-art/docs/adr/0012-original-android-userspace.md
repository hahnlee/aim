# ADR 0012: Original Android userspace on a Linux syscall layer

Status: accepted (target architecture; migration tracked in #153)

## Context

The runtime reached Android apps by rebuilding and reimplementing Android for
Darwin:

- ART, libandroid_runtime, HWUI and libbinder are rebuilt as Mach-O and
  patched (for example 184 ART patches and 33 frameworks-base patches).
- The bionic ABI is reimplemented as about 36,000 lines of facades (39 crates).
- System services are hand-written Binder endpoints that mimic AOSP owners,
  some reading AIDL transaction codes through reflection.
- Platform libraries such as `libc.so`, `libEGL.so` and `libmediandk.so` are
  resolved by name inside the native loader. No real file backs them.

Every Android release has to be re-fitted to these patches and
reimplementations. Booting the original ActivityManagerService inside the
current bootstrap (#148) showed the cost of mixing the two: the owners assume
the original SystemServer order, the original internal interfaces
(`LocalServices`) and the original natives.

There is no VM: Android runs as ordinary processes on the Darwin kernel.

## Decision

Run the pinned Android userspace **unmodified**, the way Wine runs Windows
programs, and implement only what lies below it.

1. **Guest world, all arm64 ELF.**
   - The original binaries of the pinned image: `linker64`, bionic, ART
     (libart and the image's own boot image), framework and services,
     SurfaceFlinger, inputflinger, audioserver, servicemanager, logd, and apps.
   - They are loaded by the original Android dynamic linker, with the image's
     linker configuration. The runtime has no by-name special cases.
2. **Boundary 1: Linux syscalls.** Linux syscalls are a stable, documented
   ABI; Windows syscalls are not, which is why Wine stops at ntdll.
   - `svc #0` in guest code is redirected at load time to an in-process
     syscall layer on the host.
   - That layer implements Linux semantics on Darwin: files and the device
     view (`/proc`, `/sys`, `/dev`, path mapping), memory (including ART's
     dual-mapped JIT cache), threads (`clone`, futex over `__ulock`), signals
     with Linux contexts, sockets, and the binder driver.
3. **Boundary 2: host-call.** A narrow, versioned C ABI through which guest
   components reach host implementations.
   - **HALs** follow Wine's `.drv` pattern. A guest ELF service (Rust,
     `aarch64-linux-android`, the original Rust AIDL backend over the original
     `libbinder_ndk`) is started from `/vendor/bin/hw` like any vendor HAL. It
     calls a host-side implementation (Rust) over host-call, which uses
     CoreAudio, IOSurface/Metal, AppKit and IOKit.
   - The composer may become a host-native exception once graphics shows the
     need; that is decided in the graphics phase.
4. **Exception: graphics driver libraries.** `libEGL`, `libGLESv2` and
   `libvulkan` are replaced by our implementations over ANGLE and MoltenVK.
   These are stable Khronos C APIs, the device's GPU driver seam.
5. **Derived system image.** The original archive plus a checked-in overlay
   manifest (additions, and explicitly listed replacements) with its own
   identity.
   - Our HALs, the vintf manifest and the GPU libraries live in `/vendor`,
     as a device vendor's would.
   - Hardware the device does not have is simply not declared, so the
     original services take their no-HAL paths.
   - A command lists what the derived image adds to or replaces in the
     original.
6. **init.** darwin-artd plays init, driven only by the original inputs:
   - it runs the `.rc` services with their sockets, users and restarts;
   - it writes the property areas in bionic's format and serves `setprop` on
     the original socket protocol;
   - APEXes are pre-flattened into the derived image;
   - SELinux is permissive.

   The original init may replace it later.
7. **Cross-process kernel state.** darwin-artd holds it, as wineserver does:
   the binder driver core, the process table and uids.

Performance is handled as explicit exceptions, never by patching the original
userspace. A gap is closed in the syscall layer, the binder driver, a HAL or
the graphics libraries. The goal is running Android apps with the least code
of our own.

## Consequences

- The old stack is retired once the new runtime matches today's app coverage
  (Calculator, Chromium, games):
  - the bionic facades;
  - the rebuilt and patched ART and framework natives;
  - the hand-written system service endpoints and their reflection;
  - the framework-compat class replacements;
  - the hand-written services.jar natives;
  - the loader's by-name providers.

  Until then, the current runtime stays the default. There is no feature
  flag; the new runtime is built alongside it.
- Upgrading Android becomes mostly a new image pin. What must track Linux is
  the syscall layer; what must track the vendor interfaces is the HALs.
- Risks to settle first (phase P0):
  - Does Darwin preserve x18? Android builds its platform with
    shadow-call-stack, which uses it.
  - bionic TLS (`TPIDR_EL0`) coexisting with Darwin's (`TPIDRRO_EL0`).
  - `clone`-created threads and signal delivery with Linux contexts.
  - The cost of a redirected syscall.

## P0 findings (2026-09-27)

- **x18:** Darwin zeroes x18 on every syscall, yield and signal, and on about
  1% of preemptions. The original libc, libart, libbinder, libutils,
  libbinder_ndk and linker64 contain no shadow-call-stack instructions, so
  this only matters for a binary that uses x18. A full-image scan settles it.
- **TPIDR_EL0 (bionic's thread pointer):** not preserved. Darwin writes a
  per-CPU value there on context switch. At load time, alongside `svc #0`,
  `mrs`/`msr tpidr_el0` are rewritten to load and store a per-thread slot:
  0.9 ns against 0.3 ns native.
- **Syscall redirection:** each `svc #0` becomes a `b` to a per-site stub
  within ±128 MiB. x30 is kept, as Linux does, and x18 is never used. A `brk`
  plus SIGTRAP path covers sites out of range. The boundary costs about 18 ns
  per call; a syscall that enters the Darwin kernel costs about 23 ns more
  than natively. Code mapped executable later is rewritten the same way.
- **First run:** the original `linker64` runs on the syscall layer
  (`crates/darwin-linux-abi`, `linux-run`). It loads the image's libraries
  through the original linker configuration and runs the original
  `linkerconfig` binary through libc initialization and `main`.

## Phases

| Phase | Target |
| --- | --- |
| P0 | Load-time `svc` redirection; the original `linker64` runs an original `/system/bin` program; x18, TLS, clone, signals and syscall cost measured |
| P1 | The original servicemanager with the binder driver; transaction latency measured |
| P2 | Original libart with the image's boot image runs Java |
| P3 | The original SystemServer boots (`sys.boot_completed`) with no HALs declared |
| P4 | allocator/mapper and composer HALs plus ANGLE/MoltenVK: SurfaceFlinger in a macOS window, one app draws |
| P5 | Input, audio, power/health, sensors and camera HALs |
| P6 | Parity with the current runtime; switch and delete the old stack |
