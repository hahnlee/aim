# Architecture

AIM follows [ADR 0012](docs/adr/0012-original-android-userspace.md): the
pinned Android 16 arm64 userspace runs **unmodified** as macOS processes on
the Darwin kernel, and AIM implements only what lies below it. There is no VM
and no hypervisor.

```
 guest (original arm64 ELF)      linker64, bionic, ART*, framework, services, apps
                                 our vendor HALs and daemons (hal/, daemons/)
 ─────────── Linux syscalls ───────────────────── host-call ──────────────────
 host (macOS)                    aim-linux-abi         aim-host-* modules
                                 aim-binder-*          aim-display (window)
                                 aim-guest-init        ANGLE (Metal)
```

\* ART is the one rebuilt component (see "Exceptions").

## Boundaries

- **Linux syscalls** (`crates/aim-linux-abi`, `linux-run`). Guest `svc #0`
  is redirected into an in-process layer that implements Linux semantics on
  Darwin: files and the device view (`/proc`, `/sys`, `/dev`, path maps),
  memory (including ART's dual-mapped JIT cache), threads and futexes,
  signals with Linux contexts, sockets, epoll, and fork/exec. The few
  instructions that depend on x18 or TPIDR_EL0 are rewritten once per file
  into a cache (`linux-translate`); the image itself is never modified.
- **Binder** (`crates/aim-binder-driver`, `crates/aim-binder-host`). The Linux
  binder driver's semantics as a library, with the cross-process state in the
  `aim-binderd` daemon ([docs/binder-driver.md](docs/binder-driver.md)).
- **Host-call** (`crates/aim-hostcall`, `crates/aim-host-*`). A narrow,
  versioned C ABI through which guest HALs reach their host implementations,
  as Wine's `.drv` modules do ([docs/host-call.md](docs/host-call.md)). The
  guest HALs (`hal/`) are Rust services over the image's own
  `libbinder_ndk` ([docs/vendor-hals.md](docs/vendor-hals.md)).

## The derived image and init

- `tools/android-image-extract` unpacks the original archive once and records
  its sha256 identity.
- `android-image` (`crates/aim-android-image`) builds the derived image: the
  original plus `image/overlay.toml`, whose every addition, replacement and
  removal carries a reason. Our HALs and the GPU driver live in `/vendor` as a
  device vendor's would.
- `guest-init` (`crates/aim-guest-init`, `crates/aim-android-init`) plays init
  from the image's own `.rc` files and property contexts
  ([docs/guest-init-contract.md](docs/guest-init-contract.md)).

## Exceptions

Where the original cannot run as-is, the smallest maintainable rebuild from
AOSP source replaces it, listed as a `replace` in the overlay. Reflection or
by-name bypasses are never an option.

- **ART:** macOS arm64 cannot map below 4 GiB, so libart and dex2oat are
  rebuilt with base-relative compressed references (`patches/art-android/`,
  [docs/art-exception-patches.md](docs/art-exception-patches.md)) and the boot
  image is regenerated with them.
- **Daemons bound to Linux kernel features** (netd, vold, lmkd, apexd) are
  replaced at their stable AIDL interfaces by Rust programs in `daemons/`.

## Graphics, input and devices

- GLES/EGL: `/vendor/lib64/egl/libGLES_aim.so` forwards to ANGLE on the host
  ([docs/gles-driver.md](docs/gles-driver.md)); buffers are shared as
  described in [docs/graphics-buffers.md](docs/graphics-buffers.md).
- The composer HAL talks to `aim-display`, which owns the macOS window and
  turns AppKit input into evdev devices ([docs/composer.md](docs/composer.md),
  [docs/input.md](docs/input.md)).
- Audio, Bluetooth, camera, sensors, GNSS, health and thermal are vendor HALs
  with host modules ([docs/audio.md](docs/audio.md),
  [docs/bluetooth.md](docs/bluetooth.md), [docs/camera.md](docs/camera.md)).
