# OpenJDK licensing and source provenance

The root Apache-2.0 license does not apply to OpenJDK-derived work. Covered
code is distributed under **GPL-2.0-only WITH Classpath-exception-2.0**.

## What the repository holds

No OpenJDK-derived file and no patch to one. The ART exception build
(`tools/build-art-android.sh`, ADR 0012 decision 4) compiles, unmodified:

- `art/openjdkjvm/OpenjdkJvm.cc` into `libopenjdkjvm.so`, because that
  library uses libart internals and must match the patched runtime;
- libcore's `ojluni/src/main/native/jvm.h` and its companion headers, which
  `OpenjdkJvm.cc` includes.

Both are fetched at `android-16.0.0_r1` (`patches/art-android/sources.lock`)
into the ignored `_build/art-android/src`. The series in
`patches/art-android/` does not touch them.

## License texts and upstream source

The complete upstream license and notice documents are in
[`third-party/`](third-party/), with immutable source URLs and file hashes in
[`upstream-sources.json`](upstream-sources.json):

- `libcore-ojluni-LICENSE` contains GPLv2, the Classpath exception, and the
  designated-file notices. `libcore-ojluni-NOTICE` preserves additional credits.
- `libcore-LICENSE` and `libcore-NOTICE` preserve the root libcore notices;
  their presence does not mean all of libcore has one license.
- `art-openjdkjvm-LICENSE` preserves the ART OpenJDK provider notice. Use it
  together with the complete GPL/exception text in `libcore-ojluni-LICENSE`.

## Corresponding source for any future binary release

A binary release that ships `libopenjdkjvm.so` must provide its complete
corresponding source under GPLv2 using a compliant distribution method: the
exact upstream sources (fetched by the build script at the pinned tag), the
build scripts and the toolchain record. A link to an upstream homepage is not
by itself the corresponding source of a shipped binary.

The Java side of OpenJDK (`core-oj.jar` and friends) comes unmodified from the
original system image and keeps that image's terms.
