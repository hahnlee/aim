# Building: `cargo aim`

Everything in this repository builds through one command, `cargo aim`
(`crates/aim-build`, a cargo alias in `.cargo/config.toml`). It is a task
graph with content-hash caching, in the spirit of Turborepo or Nx: a node
runs only when something it depends on changed, and then everything
downstream of it runs too. There is nothing to remember to rerun.

```sh
cargo aim build                  # what a boot needs (the default set)
cargo aim status                 # which nodes are stale, and why
cargo aim test                   # unit tests
cargo aim test --integration     # builds every node, then all tests
cargo aim boot                   # aim-display + guest-init, docs/boot-status.md
cargo aim bench [--runs N]       # boot and measure, docs/perf-baseline.md
cargo aim clean [NODE...]        # forget nodes and remove their outputs
cargo aim storage [DATA...]      # what the disk images occupy, docs/storage.md
```

What the build made runs as a resident guest, with its apps managed, by
`aimctl` (`target/release/aimctl`, [aimctl.md](aimctl.md)).

`cargo aim build hal/health` builds one node and what it needs; a prefix
selects a group (`cargo aim build hal`). `-v` shows the tools' output,
`-j N` runs up to N nodes at once (default 3). Each node's output goes to
`target/aim-cache/logs/<node>.log`; a failure prints its tail.

## Requirements

- macOS on Apple Silicon; Rust with `rustup target add aarch64-linux-android`.
- The Android SDK (`ANDROID_SDK_ROOT`, else `~/Library/Android/sdk`) with
  NDK `28.2.13676358` and `build-tools/36.0.0` (its `aidl` is pinned by
  sha256 in `hal/sources.lock`).
- JDK 17 (Homebrew `openjdk@17`) for the ART build and xsdc, `python3`,
  `git`, `curl`, `unzip`. The device services' toolchain is fetched
  ("Java").
- The pinned image archive in `_prebuilt` (`image/original.lock`).
- For ANGLE only: an ANGLE checkout (fetched when missing, ~15 GB with its
  toolchain; see "ANGLE") and the system `ninja`.

## Layout

| Path | What |
| --- | --- |
| `_build/android16-image.dmg` | The system image (`image` node): the original and its translation cache as an uncompressed read-only case-sensitive disk image, mounted at `_build/android16-image` (docs/storage.md) |
| `_build/aosp/` | AOSP trees fetched at the image's tag, each checked against its lock's hash |
| `_build/downloads/` | Their archives (and the MoltenVK release), so a refetch needs no network |
| `_build/cts/` | The pinned entries of the CTS release (`upstream/cts.lock`, `tools/cts-module.py`; docs/system-services.md) |
| `_build/java/` | The Java toolchain of `upstream/java-toolchain.lock`, unpacked ("Java") |
| `_build/xsdc`, `_build/angle-source`, `_build/depot_tools` | Pinned checkouts |
| `target/aim/<node>/` | Every build output (`hal/bin`, `art/stripped`, `boot-image`, ...) |
| `target/aim/derived.shadow` | The derived image: the system image's changes by the overlay and its translations, mounted read-only at `target/aim/derived` |
| `target/aim/gen/` | Generated sources: the AIDL crates `hal/aidl/*` and `daemons/aidl/*` point at |
| `target/aim-cache/` | Node stamps and logs |

`crates/aim-paths` is this layout as code. Tests, build scripts and
`cargo aim` all use it, so no test guesses at `CARGO_MANIFEST_DIR/../../_build`.
Nothing under `_build/` or `target/` is in Git.

## Nodes

`cargo aim status` lists them. The graph is derived; only the coarse
non-cargo stages are declared in code:

| Node | Upstream | Inputs (declared) | Outputs |
| --- | --- | --- | --- |
| `host/<bin>` | `aidl-gen` when it compiles generated sources (aim-services) | manifests, build scripts, `Cargo.lock`, cargo config; found: dep-info | cargo's `target/release/<bin>` |
| `image` | `host/android-image-extract`, `host/linux-translate` (order only) | `image/original.lock` | `_build/android16-image.dmg`, attached |
| `aidl-gen` | `image` (order only) | `hal/sources.lock`, `daemons/sources.lock`, `crates/aim-services/sources.lock`, `image/original.lock`, `tools/lib/*.py`, AIDL crate manifests | `target/aim/gen/{hal,daemon,service}-aidl` |
| `hal/<package>`, `daemon/<package>` | `image`; `aidl-gen` when a dependency's sources are generated or fetched | as `host/*` | `target/aim/{hal,daemons}/...` |
| `xsdc` | | `upstream/android16-xsdc.lock` | `target/aim/xsdc` |
| `art` | `xsdc`, `image` | `patches/art-android/*`, `tools/art-android/*`; found: n2's deps log | `target/aim/art/stripped` |
| `boot-image` | `art`, `image`, `host/linux-run` (order only) | | `target/aim/boot-image/framework` |
| `angle` | | `upstream/angle.lock`, `upstream/angle-args.gn` | `_build/angle-source/out/AimRelease` |
| `moltenvk` | | `upstream/moltenvk.lock` | `target/aim/moltenvk` (`libMoltenVK.dylib`, `LICENSE`, `vk.xml`) |
| `device-services` | `image` | `upstream/java-toolchain.lock`, `java/device-services/*`, `java/framework-overlay/*`, `java/notification-permission/*` | `target/aim/device-services`: `aim-services.jar`, the platform's `systemserverclasspath.pb` with it appended, `framework-overlay.apk`, `notification-permission.apk` ("Java") |
| `system-server` | `image` | `image/native-services` | `target/aim/system-server/services.jar`, SystemServer without the start of the natively implemented services (docs/system-services.md) |
| `oat` | `image`, `art`, `boot-image`, `system-server`, `host/linux-run` (order only) | | `target/aim/oat`: the image's oat files with code compiled again, and the device's own APKs compiled (docs/art-exception-patches.md, "Other oat files"), at their guest paths under `root/`, and `overlay.toml`, which `image/overlay.toml` includes |
| `derived-image` | `image` and the producer of every built overlay source | `image/overlay.toml` and its checked-in sources | `target/aim/derived.shadow`, attached at `target/aim/derived` |
| `translation-cache` | `derived-image`, `host/linux-translate` | | the derived image's `translated/` |
| `userdata/empty` | | | `target/aim/userdata/empty.asif`: the empty data image a new data directory starts as a clone of (`guest-init --userdata`, docs/first-boot.md) |
| `userdata/template` | `derived-image`; `userdata/empty`, `translation-cache`, `host/guest-init`, `host/linux-run`, `host/aim-display`, `host/aim-binderd` (order only) | | `target/aim/userdata/userdata-<image>-<sku>.asif` and `.sha256`: what a build-time first boot of the derived image writes that a new data directory starts from (docs/first-boot.md) |

The default `cargo aim build` builds every node except the HALs' test
clients (`[package.metadata.vendor-hal] test = ...`), which `cargo aim test
--integration` adds.

### Where the graph comes from

- **Rust crates:** `cargo metadata`. Every binary of the host crates (the
  workspace's `default-members`) is a `host/<bin>` node. Every guest package
  with a `[package.metadata.vendor-hal]` table (`binary`, `library` or
  `test`) is a `hal/<package>` node, and one with
  `[package.metadata.daemon] program` a `daemon/<package>` node; the table
  names what it is installed as. A guest node depends on `aidl-gen` when one
  of its dependencies has its sources under `target/aim/gen` or
  `_build/aosp`.
- **The derived image:** `image/overlay.toml`. Every `source` under
  `target/` or `_build/` must be an output of some node, and the derived
  image depends on that node; a source that no node produces is an error.
  Every other source is a checked-in input.
- **Inputs:** the tools' own records, plus the declared lock, patch and
  helper files. A cargo node's found inputs are the repository files of
  cargo's dep-info for its artifact; the ART build's are the files n2's deps
  log (`.n2_db`) records from the compilers' depfiles.

So adding a HAL or a daemon takes no graph edit: a package with its metadata
table, and `image/overlay.toml` entries whose sources are its outputs
(`target/aim/hal/bin/<name>`).

### Keys and stamps

A node's key is the sha256 of its recipe version (bumped in code when a
node's recipe changes its output), the versions of the tools it runs
(`rustc -vV`, the NDK's `source.properties`, the pinned `aidl`'s hash, JDK,
Python, n2), the keys of its upstream nodes and the content of its inputs.
An order-only upstream is built first but is not part of the key: the boot
image does not change with the syscall layer that runs dex2oat.

The stamp `target/aim-cache/<node>.stamp` records the key and each input's
hash, size and mtime. A check rehashes only the files whose size or mtime
changed, so a no-op build of the whole graph takes about a second. A node
whose key matches its stamp and whose outputs exist is skipped; otherwise it
runs, and cargo or n2 inside it do their own incremental work. The stamp
also lets `cargo aim status` say what changed:

```
  hal/health        stale  changed hal/health/src/main.rs
  derived-image     stale  after hal/health
  translation-cache stale  after derived-image
```

Downstream nodes see an upstream's output instead of its key where the
output is cheap to name (early cutoff): a cargo node's artifact, and the
derived image's identity with its shadow file's creation time (a shadow
made again has lost what the translation cache wrote into it). A crate
built again to the same binary, or a derived image found unchanged,
leaves what follows fresh. A cargo node that compiles generated AIDL
crates or fetched AOSP sources runs after `aidl-gen` (order only) and
keys on the generated files its dep-info lists, so a change to one
service's AIDL rebuilds what compiles it, not every HAL and daemon.

Nodes run in dependency order, several at once where the graph allows. Ready
cargo nodes of one kind share one cargo invocation (`host`, `hal`, `daemon`:
the daemons build AOSP's binder crate with its `system` feature, the HALs
without it, so they cannot share an invocation).

### Image variants

`cargo aim build --variant NAME` (also `boot` and `status`) builds the
derived image with the entries of `image/variants/NAME.toml` as well: a
manifest of `image/overlay.toml`'s schema and rules, without includes. A
variant is a check-only image of a change not yet made to every boot; it
replaces the derived image (its identity differs, so the shadow is made
again, a few seconds with the translation cache), and the next build
without `--variant` makes the default image again. Use a fresh data
directory for each image: PackageManager drops the packages a variant
removes from a data directory it boots.

- `lightweight-shell`: without SystemUI, the launcher and the wallpaper
  pickers, with the device's own static wallpaper (docs/m1-shell.md,
  #463).

### Rebuild times

A branch that changes one host crate costs its cargo build and nothing
else. Measured in the main checkout before the early cutoff and the
incremental profile (M2 Pro, load 4-7), `cargo aim build` after a change
to one file:

| Change | Time |
| --- | --- |
| `aim-linux-abi` (linux-run, linux-translate) | 7.7 s: cargo 5.0 s, then the translation cache 2.7 s |
| `aim-services` (guest-init) | 3.5 s |
| nothing | 0.5 s |

The host crates build incrementally (`[profile.release]` in
`Cargo.toml`), with release's usual 16 codegen units: in a worktree,
all host binaries after a one-line change to `aim-linux-abi` took 4.1 s
instead of 6.8 s, and after one to `aim-services` 0.9-4.1 s instead of
5.2-9.2 s. The incremental state costs about 600 MB in `target/release`.

## One Cargo workspace

The root workspace holds the host crates and the guest crates (`hal/`,
`daemons/`) with one `Cargo.lock` and shared `[workspace.dependencies]`.
Plain `cargo build` and `cargo test` build the host crates
(`default-members`). The guest crates build with `--target
aarch64-linux-android --profile android` (LTO, one codegen unit, no debug
info); `.cargo/config.toml` names the NDK linker through a wrapper that
`cargo aim` writes into `target/aim/ndk` (the NDK's path is machine-specific),
and the image's `libbinder_ndk.so` and `libnativewindow.so` are linked from
`target/aim/link`.

The guest crates and `aim-services` need `aidl-gen` first: the AIDL
crates' sources and AOSP's binder crate are not in Git. A plain `cargo
build` of the host crates on a fresh checkout therefore starts with
`cargo aim build aidl-gen`. `cargo aim build hal/health` does that;
afterwards `cargo build -p health --target aarch64-linux-android --profile
android` also works.

## n2

The ART build's ninja file (`tools/art-android/gen_build.py`) runs on
[n2](https://github.com/evmar/n2), a ninja-compatible builder, linked into
`cargo aim` as a library at a pinned revision (`crates/aim-build/Cargo.toml`);
`cargo aim` runs itself as n2. No one needs to install ninja for it.

ANGLE is the exception: its node runs the system `ninja`, for two reasons.

- n2 expands a rule's `${rspfile}` inside that rule's own `command` to
  nothing. gn writes actions with response files that way (for example
  perfetto's `gen_buildflags`, `write_buildflag_header.py --rsp
  ${rspfile}`), so those actions fail under n2.
- n2 does not read ninja's `.ninja_log` and `.ninja_deps`. An ANGLE tree
  built by ninja would be rebuilt from scratch, and a fresh ANGLE build
  needs Xcode's Metal Toolchain component (`xcodebuild -downloadComponent
  MetalToolchain`).

## Java

Code that runs in system_server is dex, and a framework overlay is an APK
(docs/system-services.md, "The system_server bridge"). The
`device-services` node builds both with the toolchain of
`upstream/java-toolchain.lock`, downloaded into `_build/downloads`, checked
against its sha256 and unpacked into `_build/java`: a Temurin JDK 17
(`javac`, and the runtime of d8 and apksigner), the SDK Build-Tools'
d8, aapt2, apksigner and aidl, and AOSP's public test key. They are build
tools; nothing of them is linked into the output or put in the image.

- `java/device-services/src` and the Java of its AIDL
  (`java/device-services/aidl`, whose Rust side `aidl-gen` generates,
  `OWN_INTERFACES` of `crates/aim-services/sources.lock`) are compiled
  against `java/device-services/stubs`: stubs of the image's classes with
  only the members used. The stubs are dexed and checked against the
  image's jars on the boot and system server class paths
  (`aim_android_image::linkage`): every class, superclass, member and
  constant a stub declares must be the image's. Then every class, field
  and method the dexed code refers to outside itself must resolve in those
  jars as ART resolves it. A changed internal API fails the build instead
  of the boot. The Java stubs' transaction codes are checked against the
  ones generated for the Rust side.
- The jar holds one stored, aligned `classes.dex`, as the platform's do.
- `java/framework-overlay` is linked by aapt2 against the image's
  `framework-res.apk` and signed by apksigner with the test key, which
  suffices for a preinstalled package; PackageManager keeps its
  certificate, so the key must not change. `java/lightweight-shell-overlay`
  (the image variant below) is built the same way.
- `java/notification-permission` is an app (#470): its code and the Java
  of the AIDL are compiled against the same stubs, checked against the
  boot class path only (an app's), added to the APK stored, aligned and
  signed with the test key.

## ANGLE

The `angle` node checks that `_build/angle-source` is the pin of
`upstream/angle.lock`, writes `out/AimRelease/args.gn` from
`upstream/angle-args.gn` (running `gn gen` only when it differs) and runs
`ninja libEGL libGLESv2` there, a no-op when nothing changed. A missing
checkout is fetched (git at the pin, `scripts/bootstrap.py`, depot_tools'
`gclient sync`); an existing one is never changed, since other checkouts may
share it. Its build stays in the checkout's own `out/` for the same reason:
a second ANGLE build per worktree would cost gigabytes.

## Tests

`cargo aim test` builds `aidl-gen` (aim-services compiles generated
sources), compiles the host crates' tests (`cargo test --release
--no-run`) and runs each test binary on its own, from its package directory,
with a timeout (`--timeout SECS`, default 900) that kills the binary's whole
process group, so a hung guest cannot outlive it. Without `--integration` it
runs the unit tests (lib and bin targets); `--integration` builds every node
first and adds the `tests/` targets.

Tests find their inputs through `aim_paths` (`original_image()`,
`derived_image()`, `angle()`, `hal_test(...)`, `ndk_clang(...)`). A test
whose input is missing calls `aim_paths::skip` (or `aim_paths::input`):
under plain `cargo test` it passes with a note, under `cargo aim test` it is
an error, since every input was built first. Tests use the derived image
`cargo aim` built; plain `cargo test` does not rebuild it, so run `cargo aim
build` first after a change to its inputs.

## Worktrees

A git worktree has no `_build`. Link the large inputs from the main
checkout (they are verified against their pins, and never written once
present):

```sh
mkdir -p _build
ln -s "$MAIN/_build/android16-image.dmg" _build/
ln -s "$MAIN/_build/angle-source" "$MAIN/_build/depot_tools" _build/
```

`_build/aosp` and `_build/downloads` may be linked too, or fetched again.
`cargo aim` and `aim_paths::input` resolve links before they hand a path to
a tool: the Python helpers compute relative paths between real
directories. The system image is mounted beside the real image file, so
every checkout that links it uses the one attachment; each checkout has
its own derived image (a shadow file over it).

A worktree can start from the main checkout's outputs instead of building
them again (the ART node alone takes about 13 minutes): APFS clones share
their blocks until one side writes, and the stamps are keyed by content
and relative paths, so `cargo aim build` then runs only what the branch
changed. Leave out `derived` (a mount point), `derived.shadow` (rebuilt in
seconds), `boot` and `bench`:

```sh
mkdir -p target/aim target/aim-cache
for e in "$MAIN"/target/aim/*; do
  case ${e##*/} in derived|derived.shadow|boot|bench) ;; *) cp -c -R "$e" target/aim/ ;; esac
done
cp -c "$MAIN"/target/aim-cache/*.stamp target/aim-cache/
cargo aim build
```

`cargo aim boot` then runs from the worktree beside a boot of another
checkout: each has its own derived image mount, display socket and data
directory, and each guest-init its own binder service name. They share
the system image's attachment and the user's translation cache
(`~/Library/Caches/aim/translated`, whose entries are published
atomically). Window mode's shims have bundle identifiers scoped to the
data directory (docs/windows.md, "Identity"), so window-mode boots of two
checkouts run side by side too. Run
`target/release/cargo-aim` only through `cargo aim` or with the same
`CARGO_TARGET_DIR`: it builds the host binaries it starts into cargo's
target directory.

`target` may be a link to a directory elsewhere (such as a per-worktree
target directory), but then set `CARGO_TARGET_DIR` to its real path: tests
put guest directories under cargo's `CARGO_TARGET_TMPDIR` and hand them to
linux-run, whose path maps do not match a host path that goes through a
link (fd-relative calls such as `mkdirat` then fail with `EROFS`).

## Scripts it replaced

`tools/build-vendor-hals.sh`, `build-daemons.sh`, `build-art-android.sh`,
`build-art-boot-image.sh` and `build-android16-xsdc.sh` are the `aidl-gen`,
`hal/*`, `daemon/*`, `art`, `boot-image` and `xsdc` nodes. The Python
helpers (`tools/lib/*.py`, `tools/art-android/*.py`) are still run by those
nodes.
