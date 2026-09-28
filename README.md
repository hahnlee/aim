# AIM — Android on macOS

AIM runs the original Android 16 arm64 userspace on Apple Silicon as ordinary
macOS processes, the way Wine runs Windows programs: no VM, no hypervisor.
The pinned image's own `linker64`, bionic, framework, SurfaceFlinger and apps
run unmodified on an in-process Linux syscall layer; HALs reach macOS
(CoreAudio, Metal through ANGLE, AppKit, IOKit) over a narrow host-call ABI.

The architecture and its decisions are in
[ADR 0012](docs/adr/0012-original-android-userspace.md); a short map of the
tree is in [ARCHITECTURE.md](ARCHITECTURE.md). How far the image boots is
recorded in [docs/boot-status.md](docs/boot-status.md).

## Requirements

- macOS on Apple Silicon, Rust (with `rustup target add aarch64-linux-android`)
- the Android SDK with an NDK and `build-tools/36.0.0` (for `aidl`)
- Homebrew `openjdk@17` and `ninja`
- an ANGLE build (Metal backend) in `_build/angle-source`, see
  [docs/gles-driver.md](docs/gles-driver.md)
- the pinned original image archive (see [docs/gsi-base.md](docs/gsi-base.md))

## Build and boot

```sh
cargo build --release
target/release/android-image-extract ARCHIVE.zip _build/android16-image-full

tools/build-vendor-hals.sh        # our vendor HALs (hal/)
tools/build-daemons.sh            # replaced netd/vold/lmkd/apexd (daemons/)
tools/build-art-android.sh        # the ART exception (patches/art-android/)
tools/build-art-boot-image.sh     # its boot image

target/release/android-image assemble --original _build/android16-image-full \
    --manifest image/overlay.toml --out DERIVED
target/release/linux-translate DERIVED

target/release/aim-display --socket DISPLAY --size 1080x1920 &
target/release/guest-init --image DERIVED --data DATA --run \
    --gpu _build/angle-source/out/AimRelease --display DISPLAY
```

`tools/guest-logcat.sh DATA/run` reads the running guest's logd.

## Layout

| Path | What |
| --- | --- |
| `crates/aim-linux-abi` | The Linux syscall layer and loader (`linux-run`), the translation cache (`linux-translate`) |
| `crates/aim-binder-driver`, `crates/aim-binder-host` | The binder driver and its cross-process host (`aim-binderd`) |
| `crates/aim-guest-init`, `crates/aim-android-init` | init for the guest: `.rc` services, properties, sockets (`guest-init`) |
| `crates/aim-android-image` | The derived image: original plus `image/overlay.toml` (`android-image`) |
| `crates/aim-hostcall`, `crates/aim-host-*` | The host-call ABI and the host side of each HAL; `aim-display` owns the window |
| `hal/`, `daemons/` | Guest-side vendor HALs and replaced daemons (Rust, `aarch64-linux-android`) |
| `image/` | The overlay manifest and our `/vendor` files |
| `patches/art-android/` | The ART exception series |
| `tools/` | Build scripts, `android-image-extract`, the GPU thunk generator |
| `docs/` | Design documents; `docs/adr/` holds the decisions |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Licensing
is described in [LICENSING.md](LICENSING.md).
