# Android 16 GraphicsJNI/HWUI strict Darwin closure

This audit composes the existing Android 16 Darwin arm64 archives in their
static-link ownership order. It neither builds a replacement symbol nor
accepts an unresolved symbol merely because the count is below a threshold.

The force-loaded roots are the Layoutlib registrar, all 61 GraphicsJNI host
members, all 88 HWUI core/host members, the five common graphics APEX members,
the unified framework-configured macOS Skia archive with Ganesh GL and Metal
required by HWUI's `whole_static_libs`, and
the two-member Android ICU initialization archive. All other archives are
normal static providers in Android.bp dependency order: skcms, androidfw,
hostgraphics, the six codec modules, Minikin, HarfBuzz, FreeType, ui-types,
nativehelper, utils/cutils/log/base, ziparchive-for-incfs, png/zlib, and ICU.

Run:

```sh
tools/audit-android16-graphics-closure.sh
```

For the real ART class-library ABI, run the independent variant:

```sh
tools/audit-android16-graphics-closure.sh --art-runtime
```

It preserves the same 32-slot provider graph and replaces only the
seven-member host/Layoutlib `libnativehelper_jvm.a` slot with
`_build/nativehelper-device-foundation/libnativehelper-device-darwin.a`. Its
relocatable object and executable are published under
`_build/graphics-runtime-closure-audit` with the
`android16-graphics-runtime-closure` basename. The variant requires the device
archive to define `JniConstants_FileDescriptor_descriptor` and rejects the
host-only `JniConstants_FileDescriptor_fd` definition. If that archive has not
been built, the gate exits with status 2 and points to
`tools/build-android16-nativehelper-device-foundation.sh`; its source and ABI
identity are independently pinned by `android16-nativehelper-device.lock`.

The gate produces a force-loaded relocatable Mach-O object and three strict
classifications:

1. Any unresolved symbol that is defined by an archive already present in the
   composition is a provider ordering/extraction error and fails immediately.
2. Symbols resolved only by the final Darwin executable providers are recorded
   separately. The executable link supplies CoreFoundation, CoreGraphics,
   ImageIO, Foundation, AppKit, libc++, libSystem, lz4, and zlib. The pinned
   module-complete `libandroid-base` archive supplies its Android.bp
   `whole_static_libs` fmt member; host fmt is not an executable provider.
3. Anything still rejected by the executable linker is a missing module. The
   exact symbol set is locked; an unknown or partially changed set is a hard
   regression, not a newly tolerated unresolved list.

## Current closure result

The relocatable composition is internally ordered: none of its 415 imports is
also present in the 42,082-definition provider set. Darwin frameworks,
libc++, libSystem, lz4, and zlib resolve all 415 external imports. The final
arm64 executable links and launches successfully, so the missing-module count
is zero.

The 956-member Skia archive supplies one shared Skia definition set for both
AOSP HWUI's Ganesh GL pipeline and the Darwin Metal compositor. It includes
the Android framework configuration while retaining macOS as the target OS,
so Android framework semantics do not make Metal compile through an iOS
branch. It also retains the two AOSP `android_utils` translation units used by
framework bitmap decoding, HWUI's custom empty font manager, and multi-frame
picture sharing, AOSP FreeType, and the pinned framework image codecs. Its
stable patched-source overlay makes a no-op incremental rebuild complete in
about two seconds instead of invalidating the entire source graph. The
four AOSP AHardwareBuffer Ganesh extension objects provide the zero-copy
AHardwareBuffer/EGLImage path without changing the Metal core's OS target. The
separate 19-member module-complete
libbase archive includes its Android.bp whole-static fmt provider. The final
link therefore does not fall back to a Homebrew fmt dylib.

The executable dependency audit rejects CoreText and host ICU, FreeType, or
png dylib regressions. Any future relocatable import already defined by one of
the 32 supplied archives is a hard provider-order failure; any future final
linker import is emitted as an unclassified missing-module set and fails.

The ART-runtime composition also has 1,529 members across 32 archives. Its
device-nativehelper provider changes the locked global definition identity to
42,078 symbols while retaining the same locked 415-symbol external import
set. Provider-order leaks and missing modules remain zero, and its independent
arm64 executable closure links and launches successfully.
