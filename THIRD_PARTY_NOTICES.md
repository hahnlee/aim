# Third-party notices

Darwin ART acknowledges the following upstream work. The root Apache-2.0
license applies only within the scope described in [LICENSING.md](LICENSING.md).
Original copyright holders retain their rights. Full upstream notices must
travel with any redistributed covered sources or binaries.

## Materials present in this source repository

- **Android Open Source Project (AOSP).** Framework, ART, Bionic, system-library
  and related ports use source revisions recorded in
  [`darwin-art/sources.lock`](darwin-art/sources.lock), `darwin-art/upstream/`,
  and provider `sources.lock` files. Patches retain the target file's terms;
  AOSP is not uniformly Apache-2.0. Our patches are Darwin ART modifications,
  not unmodified upstream releases. [Upstream](https://android.googlesource.com/).
- **OpenJDK / Android libcore and ART OpenJDK providers.** Copyright notices
  include Oracle and/or its affiliates, the Android Open Source Project,
  and other authors named in the original files. GPLv2 with the Classpath
  exception applies to the designated files and their modifications. See
  [the exact scope and modification inventory](licensing/OPENJDK.md) and
  [copied upstream notices](licensing/third-party/).
- **FreeBSD Linuxulator reference slice.** The pinned 24-file slice retains its
  original per-file BSD notices and
  [`COPYRIGHT`](darwin-art/upstream/freebsd-linuxulator/source/COPYRIGHT).
  It is used for semantic/ABI reference; the reference kernel implementation
  is not linked into the runtime. Generated manifests retain FreeBSD provenance.
  See [the adoption document](darwin-art/docs/freebsd-linuxulator-adoption.md).
- **Bionic device API header.** Copyright (C) 2018 The Android Open Source
  Project. The vendored
  [`get_device_api_level_inlines.h`](darwin-art/tools/bionic-process-state-facade/upstream/get_device_api_level_inlines.h)
  retains its full BSD-2-Clause notice. Its revision and hash are in that
  provider's `sources.lock`.
- **Skia.** Darwin patches adapt Skia's BSD-licensed implementation. The copied
  [Skia license](licensing/third-party/skia-LICENSE) retains the upstream notice.
  [Upstream](https://skia.googlesource.com/skia/).
- **BoringSSL.** Darwin patches adapt BoringSSL. Preserve its composite license,
  including applicable OpenSSL/SSLeay and other file-specific notices; see the
  copied [BoringSSL license](licensing/third-party/boringssl-LICENSE).
  [Upstream](https://boringssl.googlesource.com/boringssl/).

License documents are copied verbatim, with revision and SHA-256 provenance in
[`licensing/upstream-sources.json`](licensing/upstream-sources.json). Individual
file notices remain authoritative when a project contains multiple licenses.

## Dependencies fetched or used by the build

These acknowledgments identify build dependencies; they are not a complete
notice bundle for a future compiled application. Inspect the actual artifact
and its transitive dependencies before distributing binaries.

| Component family | License handling |
| --- | --- |
| AOSP ART, Framework, HWUI, Minikin, system libraries, Perfetto | Predominantly Apache-2.0, with file-specific exceptions including the OpenJDK components above |
| Bionic and imported BSD libc routines | Preserve each file's BSD/other notice; kernel UAPI headers retain any applicable syscall exception |
| Skia, ANGLE, Dawn and their dependencies | Preserve the BSD/other terms of the actual source and linked third-party code |
| MoltenVK | Apache-2.0; its pinned version and license hash are in `darwin-art/sources.lock`; also preserve notices for bundled dependencies |
| ICU / ICU4J, HarfBuzz, FreeType and font files | Preserve code and data licenses separately; use the FreeType License (FTL) option where available and satisfy its credit requirement |
| BoringSSL / Conscrypt, libc++ / libunwind, compression and image codecs | Preserve the exact version's composite licenses, exceptions, and third-party notices |
| Rust crates | See the versioned [Rust dependency notices](licensing/rust-dependencies.txt); this inventory includes resolved build/test dependencies |

Refresh Rust notices after changing the lockfile with
`python3 darwin-art/tools/update-rust-license-notices.py`; use `--check` to
verify them without writing. Both commands use Cargo's locked, offline metadata
and require the dependencies to have been fetched into the local Cargo cache.
The staged AOSP DebugStore crate has a separate template manifest/lock under
`darwin-art/tools/debugstore/`; its generated build graph is not part of this
workspace inventory and must be included when auditing a binary release.

This software uses the FreeType project (https://freetype.org/). Font files
retain their own licenses; this acknowledgment does not cover font rights.

The presence of GPL text in a downloaded tool or test tree does not determine
the license of every library built from that tree. Conversely, static linking
does not remove a component's attribution or source obligations. Preserve all
relevant upstream LICENSE/NOTICE files instead of substituting this summary.
