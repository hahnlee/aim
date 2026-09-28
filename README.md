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
- the Android SDK with NDK `28.2.13676358` and `build-tools/36.0.0` (for `aidl`)
- Homebrew `openjdk@17`, `python3`
- the pinned original image archive in `_prebuilt` (`image/original.lock`,
  [docs/gsi-base.md](docs/gsi-base.md))
- for ANGLE (Metal), its checkout in `_build/angle-source` and `ninja`, see
  [docs/build.md](docs/build.md) and [docs/gles-driver.md](docs/gles-driver.md)

## Build and boot

```sh
cargo aim build     # everything a boot needs; reruns only what changed
cargo aim status    # what is stale, and why
cargo aim test      # unit tests; --integration builds everything and runs all
cargo aim boot      # aim-display + guest-init on the derived image
```

`cargo aim` is the build graph of [docs/build.md](docs/build.md): the host
tools, our vendor HALs and daemons, the ART exception and its boot image, the
extracted original and the derived image, each rebuilt when its inputs
change. `tools/guest-logcat.sh DATA/run` reads the running guest's logd
(`cargo aim boot` puts DATA in `target/aim/boot/data`).

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
| `crates/aim-build`, `crates/aim-paths` | `cargo aim` and its output layout |
| `tools/` | `android-image-extract`, the build's Python helpers, the GPU thunk generator |
| `docs/` | Design documents; `docs/adr/` holds the decisions |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Licensing
is described in [LICENSING.md](LICENSING.md).
