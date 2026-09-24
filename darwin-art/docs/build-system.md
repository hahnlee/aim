# Build system

AIM uses two build engines behind one developer entry point:

- Cargo owns all Rust code, dependency resolution, orchestration, and process
  lifetime code. Every Rust package is a member of the root workspace and uses
  the root `Cargo.lock` and `target/` cache.
- Ninja owns the large AOSP C, C++, Objective-C++, and assembly graph. Compiler
  depfiles and content fingerprints rebuild only translation units whose inputs
  changed.

Do not invoke an individual tool manifest during ordinary development. The
tool packages are implementation details of the provider closure and share the
workspace cache even when an audit script uses `--manifest-path`.

## Developer commands

```sh
cargo xtask status
cargo xtask check
cargo xtask build
cargo xtask providers
cargo xtask full
```

`cargo xtask check` reads tracked and untracked changes, runs the shared Rust
workspace check when necessary, selects the audit belonging to a changed
provider, and invokes the incremental native graph when a linked provider or
native input changed. Pass paths explicitly to inspect a proposed change:

```sh
cargo xtask explain tools/bionic-vm-facade/src/lib.rs
cargo xtask check compat/darwin_surface_bridge.mm
```

`cargo xtask build` is the native inner loop. It emits the persistent Ninja
graph, rebuilds changed objects, and relinks the runtime. Its final edge runs
the fast dylib symbol checks, but it does not repeat source-pinned foundation
or provider audits. `cargo xtask providers` discovers and runs every Bionic
provider audit in stable name order. `cargo xtask full` runs all workspace
tests, all provider audits, and the strict release graphics link gate. The
official Chrome acceptance remains a product-level gate because it requires
the pinned APK and opens real windows.

## Measured inner loop

Measurements on the Apple Silicon development host after the workspace and
graph migration:

| Operation | Before | After |
| --- | ---: | ---: |
| Warm Rust workspace check (46 packages) | many independent tool targets | 0.84 s |
| Warm native build | 43.9 s | 1.50 s |
| One probe TU + dylib relink/check | full closure audit | 8.72 s |

The first build after promoting old artifacts into the Ninja graph can perform
one dependency scan or a foundation rebuild. That is cache migration, not the
steady-state loop. Repeating the command must report `ninja: no work to do.`
Touching `compat/darwin_android_native_window.cc` without changing its bytes
keeps the content digest stable and schedules only that TU, its archive, and
the final link/check edge.

## Ownership map

| Area | Owner | Rebuild boundary |
| --- | --- | --- |
| Runtime host and lifetime | `crates/darwin-art-host`, `darwin-art-engine*` | Cargo package |
| Android ELF and native APKs | `darwin-art-elf-loader`, `darwin-art-native-artifact` | Cargo package |
| Bionic compatibility | `tools/bionic-*`, composed by the provider closure | Changed provider audit + provider archive |
| Android framework bridge | `compat/`, `probes/runtime_app_*` | Native translation unit |
| Android native window ownership | `compat/darwin_android_native_window.cc` | One native translation unit |
| EGL/GLES/AHB interop | `compat/darwin_angle_egl.cc` | One native translation unit |
| HWUI, Skia, ART, ICU | revision-locked `_aosp/` plus `patches/` | Foundation archive family |
| Final application runtime | host + provider closure + native foundations | Incremental dylib link |

The `tools/bionic-*` directories are deliberately separate ABI ownership and
audit boundaries, not separate products. Their Cargo workspaces and lockfiles
were removed so they no longer duplicate dependency builds.

Within `bionic-fs-facade`, `lib.rs` owns the private filesystem and descriptor
model, while `process_api.rs` owns process-wide activation leases and the C
ABI. The latter is the concurrency/safety boundary: uninstall stops new calls,
waits for Rust-owned in-flight leases, and only then releases broker authority.
Tests live separately in `tests.rs`.

## Cache rules

Generated outputs live under `_build/`; Cargo outputs live only under the root
`target/`. Neither is source control. A cache key must include source content,
the complete compile command, compiler/SDK identity, and referenced headers.
Changing Rust orchestration alone must not invalidate native objects. Changing
an AOSP patch must invalidate its owning foundation family, while a probe-only
change must not rotate the ART bootstrap archive.

Sanitizer audits may set a temporary `CARGO_TARGET_DIR` intentionally. This is
an isolated verification build and must not become the normal development
path.
