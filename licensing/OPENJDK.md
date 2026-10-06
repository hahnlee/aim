# OpenJDK licensing and source provenance

The root Apache-2.0 license does not apply to OpenJDK-derived work. Covered
code is distributed under **GPL-2.0-only WITH Classpath-exception-2.0**.

## What the repository holds

The repository holds a Rust port of libcore's TimSort at
[`crates/aim-services/src/package/timsort.rs`](../crates/aim-services/src/package/timsort.rs).
It follows `platform/libcore` revision
`fff4fcc0cf7f080cf2511cbb57561482b13b218f` (`android-16.0.0_r1`),
[`ojluni/src/main/java/java/util/TimSort.java`](https://android.googlesource.com/platform/libcore/+/fff4fcc0cf7f080cf2511cbb57561482b13b218f/ojluni/src/main/java/java/util/TimSort.java).
The local changes use indices, Rust vectors and Result diagnostics while
preserving the comparator sequence needed by SELinuxMMAC's duplicate detection
and domain-owner ranking.
The port retains upstream copyright/license notices and the Classpath exception;
its containing Cargo package records both that exception and the Apache-2.0
terms of the package's independent files. The root Apache license does not
relicense the port.

The ART exception build
(the `art` node of `cargo aim`, ADR 0012 decision 4) compiles, unmodified:

- `art/openjdkjvm/OpenjdkJvm.cc` into `libopenjdkjvm.so`, because that
  library uses libart internals and must match the patched runtime;
- libcore's `ojluni/src/main/native/jvm.h` and its companion headers, which
  `OpenjdkJvm.cc` includes.

Both are fetched at `android-16.0.0_r1` (`patches/art-android/sources.lock`)
into the ignored `_build/aosp`. The series in
`patches/art-android/` does not touch them.

The device-specific system services (`java/device-services`) are compiled
with a pinned Eclipse Temurin JDK (`upstream/java-toolchain.lock`). It is a
build tool: its `javac` compiles our sources and its runtime runs d8 and
apksigner; no JDK class or library is linked into the output (the code runs
on the image's own libcore) or shipped.

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

A binary release that ships `libopenjdkjvm.so` or includes the native TimSort
port must provide the covered components' complete
corresponding source under GPLv2 using a compliant distribution method: the
exact upstream sources (fetched by the build script at the pinned tag), the
build scripts and the toolchain record. A link to an upstream homepage is not
by itself the corresponding source of a shipped binary.

The Java side of OpenJDK (`core-oj.jar` and friends) comes unmodified from the
original system image and keeps that image's terms.
