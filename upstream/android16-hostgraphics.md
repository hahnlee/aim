# Android 16 libhostgraphics Darwin gate

This gate builds the complete Android 16 `libhostgraphics` host-static module
selection from `frameworks/base/libs/hostgraphics/Android.bp`. It compiles the
five upstream translation units in Android.bp order and emits a five-member
Mach-O arm64 archive. It does not prune objects or provide replacement symbols.

## Sparse source materialization

Run `tools/sync-android16-hostgraphics.sh`. It downloads only Android.bp, the
five module sources, the nine exported module headers, and two direct header
dependencies. There is no Git checkout or full AOSP subtree:

- frameworks/base commit `99b01a65cc4c104933788b3143285ab6bae65827`,
  `libs/hostgraphics`;
- frameworks/native commit `2827a4a16b0340ecd07c2d5a6c89991799b362bb`,
  `libs/nativedisplay/include/apex/display.h`;
- hardware/libhardware commit
  `cd2b68d71a1dd9a45668ddd9a507000ccd7ff114`,
  `include_all/hardware/hardware.h`.

The source, exported-header, Android.bp, and dependency-header identities are
checked against `upstream/android16-hostgraphics.lock`. Existing common Android
16 frameworks/native and system/core include materializations remain required;
the sync command intentionally does not duplicate them.

## Build and acceptance

Run `tools/build-android16-hostgraphics.sh`. Optional
`AIM_ANDROID16_HOSTGRAPHICS_ROOT`,
`AIM_ANDROID16_NATIVEDISPLAY_INCLUDE`, and
`AIM_ANDROID16_LIBHARDWARE_INCLUDE` variables can point the gate at
equivalent checksum-locked sparse trees.

Apple clang requires `<type_traits>` before `ADisplay.cpp` evaluates its
`std::is_trivially_destructible` assertions. The Android source relies on its
build environment to expose that declaration, so the Darwin module gate uses
`-include type_traits`; the pinned upstream source remains unmodified.

Acceptance requires:

- exactly five archive members and only the arm64 architecture;
- representative `ANativeWindow_dequeueBuffer` and
  `ANativeWindow_tryAllocateBuffers` C definitions;
- representative `android::ADisplay_acquirePhysicalDisplays` and
  `android::ADisplay_getCurrentConfig` definitions.

The result is `_build/hostgraphics/libhostgraphics-darwin.a`. Its unresolved
manifest is diagnostic: Android.bp declares module-level dependencies on
libbase, libmath, libui-types, and libutils, which are linked by the later HWUI
force-load closure rather than replaced with per-symbol stubs here.
