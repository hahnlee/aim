# Android 16 HWUI Canvas host closure

This gate deliberately imports the Android 16 upstream implementation instead
of replacing Android graphics natives with Darwin stubs. Source identity is in
`android16-hwui.lock`.

## Required upstream translation units

The public gate is the complete registration table in
`jni/android_graphics_Canvas.cpp`. Because that table takes the address of every
Canvas and BaseCanvas native, dead stripping cannot reduce it to only the
methods exercised by the first probe.

Direct gate files:

- `jni/android_graphics_Canvas.cpp`
- `jni/Paint.cpp`
- `SkiaCanvas.cpp`
- `hwui/Canvas.cpp`

The JNI files must be built using the `android_graphics_jni` defaults from the
same `Android.bp`. The native Canvas implementation must come from
`libhwui_static`/`libhwui_defaults`, not a local compatibility class.

Important source groups pulled in by that module-level contract include:

- graphics JNI support: `Graphics.cpp`, `Utils.cpp`, `Bitmap.cpp`,
  `FontUtils.cpp`, Matrix, Path, Region, Shader, ColorFilter, Typeface, Mesh,
  NinePatch, Picture, and their JNI registration translation units;
- HWUI software implementation: `SkiaCanvas.cpp`, `hwui/Canvas.cpp`,
  `hwui/PaintImpl.cpp`, `hwui/Bitmap.cpp`, `hwui/Typeface.cpp`,
  `hwui/MinikinSkia.cpp`, `hwui/MinikinUtils.cpp`, vector drawable and effect
  helpers referenced by `SkiaCanvas.cpp`;
- the remaining `libhwui_defaults` source list because the upstream module
  couples RenderNode, recording Canvas, animation, and pipeline symbols even
  when the first surface is CPU rasterized.

## Project/module dependencies

The Android 16 host definitions name these non-system modules:

- `libskia`
- `libandroidfw`
- `libminikin`
- `libharfbuzz_ng`
- `libft2`
- `libicuuc` and `libicui18n`
- `libbase`, `liblog`, `libutils`, `libcutils`, and `libui-types`
- `libnativehelper_jvm` headers/helpers
- image/codec modules named by `android_graphics_jni` when its full table is
  linked; these can only be omitted by an upstream build option, not by a
  locally rewritten registration table.

The AOSP Darwin host variant of `libhwui_defaults` defines `HWUI_NULL_GPU`.
That is suitable for compiling the complete software Canvas/RenderNode closure,
but it is not the final Metal renderer. Metal presentation remains owned by the
persistent IOSurface bridge until a Darwin HWUI pipeline exists.

## Darwin compile findings

The four direct Android 16 translation units compile unmodified as arm64 Mach-O
objects with Apple's Clang. This is a source/host compile proof, not yet a safe
ART registration build. The reproducible command is
`tools/compile-android16-hwui-canvas-gate.sh`; it intentionally performs an
object compile rather than pretending that the full registration table is
already link-closed.

At this direct-object gate there are 284 unique undefined symbols after object
headings are excluded (the generated authoritative list is
`_build/hwui-canvas-gate/undefined-symbols.txt`). That list includes platform
C/C++ runtime symbols as well as the expected Skia, GraphicsJNI, libhwui,
Minikin, liblog, and JNI helper closure. The next gate must resolve it by adding
the upstream modules above; selectively implementing unresolved names locally
would recreate the stubs this port is intended to remove.

Both Skia and HWUI must be compiled with `SK_BUILD_FOR_ANDROID_FRAMEWORK`.
Without the same definition on both sides, Android-private Skia API is absent:

- `Sk3DView::setCameraLocation()` and `getCameraLocationZ()` are hidden;
- `SkAndroidFrameworkUtils` is not declared or compiled;
- Android's extra Canvas virtuals differ, leaving `RecordingCanvas` abstract.

`_build/skia/libskia.a` now uses that definition and its executable gate calls
the implementations from `src/android/SkAndroidFrameworkUtils.cpp`. This is an
ABI/build-configuration requirement, not a symbol to replace with a Darwin
stub.

There is a second, independent ABI choice in `jni/graphics_jni_helpers.h`.
Because this target is not `__ANDROID__`, the unmodified host branch expands
`CRITICAL_JNI_PARAMS` to `JNIEnv*, jclass`. Android framework DEX retains its
`@CriticalNative` annotations, so ART calls those functions without the two JNI
parameters. Defining `__ANDROID__` globally would also select unrelated Android
platform/GPU behavior. The compile gate now applies the locked
`0001-darwin-android-critical-jni-abi.patch` only to a disposable build copy and
defines `AIM_ANDROID_CRITICAL_JNI_ABI`; representative Canvas/Paint
symbols are rejected if their demangled signatures contain the host JNI
parameters. It also uses `-fno-rtti` to match the GN Skia archive rather than
inventing a `SkDrawable` typeinfo symbol.

With those ABI fixes, the four direct objects have 278 unique undefined symbols.
The framework Skia archive defines 149, Apple runtime libraries own 56, and the
remaining Android-owned groups are HWUI core (48), graphics JNI (17), Minikin
(11), and liblog (2). These are progress counts only: every archive adds a new
transitive module closure, so a decreasing number is not a link-complete gate.

The compile gate also avoids adding `compat/` as a normal angle-bracket include
directory. Doing so intercepts `<unistd.h>` with the Darwin compatibility
overlay and conflicts with AOSP `utils/Compat.h`'s `lseek64`. It uses
`-Icompat/include` for the narrow public compatibility headers and
`-iquote compat` only for `skia_darwin_config.h`.

## Ordered compile gates

1. Materialize and checksum the exact `libs/hwui` subtree.
2. Compile the four direct translation units without modifying their JNI tables.
3. Preserve the checked Android critical-native/no-RTTI ABI contract across all
   subsequent HWUI and graphics JNI translation units.
4. Add upstream dependency projects at the locked revisions as missing headers
   and symbols are encountered.
5. Link every native referenced by `register_android_graphics_Canvas()` and
   `register_android_graphics_Paint()`.
6. Register the complete upstream graphics map in ART and delete `ProbeCanvas`
   and the
   Darwin Paint implementation only after the complete registration/link audit
   passes.
