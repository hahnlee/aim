# Third-party notices

AIM acknowledges the following upstream work. The root Apache-2.0
license applies only within the scope described in [LICENSING.md](LICENSING.md).
Original copyright holders retain their rights. Full upstream notices must
travel with any redistributed covered sources or binaries.

## Materials present in this source repository

- **Android Open Source Project (AOSP).** The ART exception series
  (`patches/art-android/`) patches ART at `android-16.0.0_r1`
  ([`patches/art-android/sources.lock`](patches/art-android/sources.lock)).
  Patches retain the target file's terms; our patches are AIM modifications,
  not unmodified upstream releases. The vendor HALs and replaced daemons
  (`hal/`, `daemons/`) pin the AOSP sources they build against in their
  `sources.lock` files. [Upstream](https://android.googlesource.com/).
- **AOSP lmkd.** `daemons/lmkd/core` ports lmkd's control protocol
  (`include/lmkd.h`, the `LMK_STAT_KILL_OCCURRED` layout of `statslog.cpp`)
  and its kill order and counters (`lmkd.cpp`) from
  `platform/system/memory/lmkd` at `android-16.0.0_r1` (Apache-2.0;
  Copyright The Android Open Source Project and Google, Inc); the ported
  files name their origin and copyright. [Upstream](https://android.googlesource.com/platform/system/memory/lmkd/).
- **Khronos and ANGLE registries.** `hal/gles/src/thunks.rs` and
  `crates/aim-host-gpu/src/table.rs` are generated from the Khronos XML
  registry (`gl.xml`, `egl.xml`; Apache-2.0) and ANGLE's extension registry
  (BSD-3-Clause) by `tools/gen-gpu-thunks.py`.
- **FreeBSD Linuxulator.** The syscall layer's epoll and `/proc`/`/sys`
  follow the design of FreeBSD's `linux_event.c`, linprocfs and linsysfs
  (BSD-2-Clause); no FreeBSD code is copied.
  [Upstream](https://cgit.freebsd.org/src/tree/sys/compat/linux).

License documents are copied verbatim, with revision and SHA-256 provenance in
[`licensing/upstream-sources.json`](licensing/upstream-sources.json). Individual
file notices remain authoritative when a project contains multiple licenses.

## Dependencies fetched or used by the build

These acknowledgments identify build dependencies; they are not a complete
notice bundle for a binary release. Inspect the actual artifact and its
transitive dependencies before distributing binaries.

| Component family | License handling |
| --- | --- |
| AOSP ART, libnativehelper, libbase, liblog, libziparchive, bionic headers, Rust binder and AIDL interfaces | Predominantly Apache-2.0, with file-specific exceptions |
| OpenJDK (`art/openjdkjvm`, libcore `jvm.h`) | GPL-2.0-only WITH Classpath-exception-2.0; see [licensing/OPENJDK.md](licensing/OPENJDK.md) and [the copied notices](licensing/third-party/) |
| BoringSSL (statically linked into `dex2oat64`) | Preserve its composite license, including applicable OpenSSL/SSLeay and file-specific notices; see the copied [license](licensing/third-party/boringssl-LICENSE). [Upstream](https://boringssl.googlesource.com/boringssl/) |
| VIXL, LZMA SDK, zlib, tinyxml2, dlmalloc, fmtlib, lz4, cpu_features, libcap | Preserve the exact version's license and notices |
| ANGLE (the host GPU library) | BSD-3-Clause; also preserve the notices of its bundled dependencies |
| Rust crates | See the versioned [Rust dependency notices](licensing/rust-dependencies.txt); this inventory includes resolved build/test dependencies |

Refresh Rust notices after changing the lockfile with
`python3 tools/update-rust-license-notices.py`; use `--check` to
verify them without writing. Both commands use Cargo's locked, offline metadata
and require the dependencies to have been fetched into the local Cargo cache.
The `hal/` and `daemons/` workspaces have their own lockfiles; their graphs are
not part of this inventory and must be included when auditing a binary release.

The presence of GPL text in a downloaded tool or test tree does not determine
the license of every library built from that tree. Conversely, static linking
does not remove a component's attribution or source obligations. Preserve all
relevant upstream LICENSE/NOTICE files instead of substituting this summary.
