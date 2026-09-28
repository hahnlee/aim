# Android 16 GraphicsJNI Darwin host gate

This gate owns the complete common `android_graphics_jni.srcs` selection from
Android 16 `frameworks/base/libs/hwui/Android.bp`, its Darwin host
`platform/darwin/utils/SharedLib.cpp`, and the host `LayoutlibLoader.cpp`
registrar. It does not select a Canvas/Paint subset or manufacture unresolved
JNI implementations.

## Pinned source and ABI

The lock pins `platform/frameworks/base` revision
`99b01a65cc4c104933788b3143285ab6bae65827`, all 60 common JNI translation
units in Blueprint order, the Darwin host source, Layoutlib's complete 51-entry
class-to-function map, and `apex/jni_runtime.cpp`. The generated order is the
upstream `gRegJNI` order filtered through Layoutlib's supported key set. The
`unordered_map` initializer order is explicitly not used: `jni_runtime.cpp`
requires ColorSpace before Graphics and contains the authoritative dependency
order, while every possible function address is retained by Layoutlib's map.

After verifying that source map, the gate generates
`_build/android-graphics-jni/generated/darwin_android_graphics_registration.h`
and its adjacent CSV. Runtime code can set the header's
`kNativeClassesPropertyName` to `kNativeClassesCsv` through `System.setProperty`
before invoking the registrar. This prevents the empty default property from
silently producing a successful zero-class registration. The header is always
derived from verified upstream source; there is no committed duplicate table.

The immutable source is copied into `_build/android-graphics-jni/patched-hwui`.
Only the locked `0001-darwin-android-critical-jni-abi.patch` is applied there,
and every TU is compiled with `AIM_ANDROID_CRITICAL_JNI_ABI`. The gate
rejects Canvas/Paint critical-native symbols carrying host `JNIEnv*`/`jclass`
arguments. Defining `__ANDROID__` globally is not an acceptable substitute.
PathIterator has one host wrapper that directly calls its critical native; the
disposable copy receives the same macro condition at that call site, guarded
by locked pre/post hashes. Without it the patched declaration correctly drops
the two JNI arguments while the host wrapper incorrectly continues passing
them.

The compile gate retains the upstream `-Wall -Werror -Wunused
-Wunreachable-code` policy and its host `-Wno-unused-variable` exception.
Apple Clang diagnoses upstream VectorDrawable's two deliberate stack VLAs as
`-Wvla-cxx-extension`, so that single Darwin-only diagnostic is also narrowly
suppressed; warnings are not globally demoted.

## Build and force-load audit

Run the local registrar proof with:

```sh
tools/build-android16-android-graphics-jni.sh --registrar-only
```

The full object audit automatically materializes revision-locked, history-free
Gitiles inputs under ignored `_aosp` paths. It fetches six individual
libjpeg-turbo files (99,396 bytes), UltraHDR's two root files plus its
`lib/include` archive (14 files, 164,673 bytes), `frameworks/native` GUI
includes (78 files, 542,585 bytes), `frameworks/av` media NDK includes (13
files, 200,055 bytes), and libhardware `include_all` (41 files, 981,160 bytes).
This is about 1.9 MiB unpacked and contains no Git history or full checkout.
Temporary history-free extracts can instead be supplied using
`AIM_LIBJPEG_TURBO_ROOT` and `AIM_LIBULTRAHDR_ROOT`. The
separately sparse-materialized
`frameworks/native/libs/gui/include` tree can be supplied with
`AIM_FRAMEWORKS_NATIVE_GUI_INCLUDE`; the host common HardwareRenderer
source also needs `frameworks/av/media/ndk/include`, overrideable with
`AIM_FRAMEWORKS_AV_MEDIA_NDK_INCLUDE`. YUV/JPEG's PixelFormat include
closure needs the real `hardware/libhardware/include_all` facade (not a local
copy of `hardware.h`), supplied with `AIM_LIBHARDWARE_INCLUDE`.

The exact libjpeg-turbo file set is `Android.bp`, `jpeglib.h`,
`jpeglibmangler.h`, `jconfig.h`, `jmorecfg.h`, and `jerror.h`. UltraHDR is
`Android.bp`, `ultrahdr_api.h`, and the immutable `lib/include` subtree. The
other three archive paths are exactly `libs/gui/include`, `media/ndk/include`,
and `include_all`, respectively. Re-running the sync gate verifies hashes and
does no network work when these sparse trees are already present.

```sh
tools/build-android16-android-graphics-jni.sh --object-audit
```

A successful object audit creates a 61-member common-host archive and a
separate registrar archive. Apple `ld -r` force-loads both. The resulting
Mach-O object must define every function named by all 51 registration entries;
this proves static archive extraction/dead-strip behavior without pretending
that the transitive executable closure is complete. Remaining undefined
symbols are recorded as a module acquisition input, never filled one-by-one.
The verified Darwin arm64 closure currently records 739 unique transitive
undefined symbols with manifest SHA-256
`29f3c634d6c314acbb6475035c2fce5ce1f6ccf4901ccf5ed12faa15880fea0d`.

The Android.bp GraphicsJNI dependency closure is `libbase`, `libcutils`,
`libharfbuzz_ng`, `libimage_io`, `libjpeg`, `libultrahdr`, `liblog`, `libminikin`,
`libz`, `libziparchive_for_incfs`, host `libandroidfw`, and
`libnativehelper_jvm`. The containing `libhwui` additionally supplies the HWUI
Canvas/RenderNode/renderer implementation and framework-configured Skia.
Executable acceptance therefore requires module-complete archives for these
groups; per-symbol stubs and a reduced registration map are forbidden.

## Paint and RenderNode ownership invariant

The current bootstrap runtime still registers fake
`android/graphics/Paint` and `android/graphics/RenderNode` natives from
`compat/darwin_framework_natives.cc`. They cannot coexist with Layoutlib's
upstream tables: duplicate registration order is unsafe, and their native
handles do not have the same concrete C++ type or lifetime.

Default full integration consequently exits 2 while both fake registrations
remain. They may be removed from runtime composition only in the same change
that force-load-links the complete upstream GraphicsJNI/HWUI module closure.
The object audit is deliberately separate so source porting can progress
without temporarily mixing the two implementations.
