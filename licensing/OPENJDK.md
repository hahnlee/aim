# OpenJDK licensing and source provenance

The root Apache-2.0 license does not apply to the OpenJDK-derived work below.
Covered original code and our modifications are distributed under
**GPL-2.0-only WITH Classpath-exception-2.0**. Darwin ART explicitly retains
and extends the upstream Classpath exception to its modifications of files
already covered by that exception. This does not add an exception to unrelated
GPL code whose copyright holders have not granted one.

## License texts and upstream source

The complete upstream license and notice documents are included in
[`third-party/`](third-party/), with immutable source URLs and file hashes in
[`upstream-sources.json`](upstream-sources.json):

- `libcore-ojluni-LICENSE` contains GPLv2, the Classpath exception, and the
  designated-file notices. `libcore-ojluni-NOTICE` preserves additional credits.
- `libcore-LICENSE` and `libcore-NOTICE` preserve the root libcore notices;
  their presence does not mean all of libcore has one license.
- `art-openjdkjvm-LICENSE` and `art-openjdkjvmti-LICENSE` preserve the ART
  OpenJDK provider notices. Use them together with the complete GPL/exception
  text in `libcore-ojluni-LICENSE`.

The active source inputs are:

| Owner | Revision | Lock |
| --- | --- | --- |
| AOSP `platform/libcore` | `080fac8bb8670bc7fbc895050caf4b13c4d6cd12` | `darwin-art/sources.lock` and the libcore/native-owner locks under `darwin-art/upstream/` |
| AOSP `platform/art`, `openjdkjvm` | `ed6c006bd06ae060bd9698fd2cb25c4865512ec3` | `darwin-art/upstream/android16-openjdkjvm-darwin.lock` |
| AOSP `platform/art`, `openjdkjvmti` | `9fac16b7c1e3599509612e9f9567283a00b16ac1` | `darwin-art/upstream/android16-openjdkjvmti.lock` |

## Repository modifications covered by these terms

Paths in this table are relative to `darwin-art/`. These are modifications by
Darwin ART contributors, documented on 2026-09-24. Git history records the
individual change dates; the patch bytes remain unchanged so their existing
source/build checksums remain valid.

| Repository path | Upstream target / modification |
| --- | --- |
| `patches/art-openjdkjvm/0001-darwin-jvm-last-error-string.patch` | `openjdkjvm/OpenjdkJvm.cc`; Darwin error-string handling |
| `patches/libcore-openjdk/0001-darwin-nativehelper-file-descriptor.patch` | `ojluni/src/main/native/io_util_md.c`; nativehelper descriptors and open bridge |
| `patches/libcore-openjdk/0002-darwin-file-descriptor-syscalls.patch` | `FileDescriptor_md.c`; Darwin descriptor operations |
| `patches/libcore-openjdk/0002-darwin-file-input-stream-virtual-descriptors.patch` | `io_util_md.c`, `FileInputStream.c`; Android descriptor routing |
| `patches/libcore-openjdk/0003-darwin-native-thread-signal-lifecycle.patch` | `NativeThread.c`; signal setup and restoration |
| `patches/libcore-openjdk/0003-darwin-unix-native-dispatcher-times.patch` | `UnixNativeDispatcher.c`; Darwin timestamp fields |
| `patches/libcore-openjdk/0004-darwin-system-boringssl-version-header.patch` | `System.c`; crypto version header and library-path handling |
| `patches/libcore-openjdk/0004-darwin-unix-process.patch` | `UNIXProcess_md.c`; Darwin wait header |
| `patches/libcore-openjdk/0005-android-guest-jni-library-suffix.patch` | `jvm_md.h`; Android guest library names |
| `patches/libcore-openjdk/0005-darwin-strict-math-fdlibm-include.patch` | `StrictMath.c`; fdlibm include path |
| `patches/openjdkjvmti/0001-darwin-monotonic-jvmti-time.patch` | `ti_timers.cc`; Darwin monotonic clock |
| `patches/openjdkjvmti/0002-darwin-malloc-size.patch` | `ti_allocator.cc`; Darwin allocation size |
| `patches/openjdkjvmti/0003-darwin-in-memory-dex-file.patch` | `ti_search.cc`; in-memory DEX loading |
| `patches/openjdkjvmti/0004-darwin-search-lazy-system-classes.patch` | `ti_search.cc`; lazy system class lookup |
| `compat/darwin_openjdk_nio_copy.c` | Adaptation of `ojluni/src/main/native/UnixCopyFile.c`; virtual descriptors and copy-loop changes; original notice restored |

All `.patch` files in those three patch directories follow the covered target's
GPLv2 + Classpath terms, including future patches to the same covered sources.
Directory `README.license` files make this scope visible beside the patches.
New targets must be checked for their own terms before being added.

`tools/build-android16-framework-compat.sh` also modifies
`java/security/security.properties` inside the image-derived `core-oj.jar` to
select security providers. The resulting `core-oj-compat.jar` retains the
original component's licenses; a configuration edit does not relicense the
archive. Native build scripts materialize and patch other OpenJDK sources under
ignored `_aosp/` and `_build/` directories. Those generated source trees retain
upstream licenses even though the orchestration scripts are independent code.

## Classpath boundary

The exception permits linking covered libraries with independent modules under
their own terms. It does not turn the covered library or its derivatives into
Apache-2.0. Renaming, translating, or moving a derived implementation into
`compat/` or a Rust crate does not change that distinction. An independent host
API provider is not assigned GPL merely because it is called by OpenJDK.

## Corresponding source for any future binary release

For a binary containing covered work, provide its complete corresponding source
under GPLv2 using a compliant distribution method. Our intended method is a
source download alongside the binary, not an unfulfilled written offer.
Include the exact upstream source, original notices, all local modifications,
interface definition files, and scripts needed to control compilation and
installation. Record the actual build inputs and toolchain. A patch-only
archive, this source index, or a link to an upstream homepage is not by itself
the complete corresponding source of a shipped binary.

Build owners include `tools/build-android16-openjdkjvm-darwin.sh`,
`tools/build-android16-openjdkjvmti-darwin.sh`, the file-descriptor,
file-input-stream, unix-filesystem, unix-native-dispatcher, openjdk-nio-mapping,
system-natives and openjdk-named-jni-owner scripts, and the framework-compat
script. Preserve their transitive source/header/patch/build inputs as well.

The image-derived `core-oj.jar` currently has a binary hash, but this repository
does not establish a complete corresponding-source mapping for that image's
exact build. The native libcore revision above must not be presented as proof
of that mapping. Resolve it or replace the JAR with a traceable source build
before a binary release. This source-only licensing change does not package or
approve a customer application.
