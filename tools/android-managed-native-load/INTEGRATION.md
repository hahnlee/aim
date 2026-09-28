# Production graphics integration checklist

This checklist is intentionally preparatory. The managed fixture currently
uses the minimal runtime as a negative gate; no shared runtime file is changed
by this module.

## Build and link

1. In `audit_runtime_graphics_link` in `crates/art-bootstrap/src/main.rs`, run
   `tools/android-managed-native-load/audit.sh --build-only` before resolving
   link inputs.
2. Add
   `_build/android-managed-native-load/libopenjdk-runtime-managed-load-darwin.a`
   as the sole complete `java.lang.Runtime` native owner.
3. Keep the compatibility/bootstrap consumer before both complete owner
   archives. Place the Runtime archive in the libopenjdk group before
   `_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a`; keep
   `_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a`
   as the sole complete `sun.nio.fs.UnixNativeDispatcher` owner. The later
   nativehelper-device and liblog archives close `jniRegisterNativeMethods`,
   JNU, and logging dependencies.
4. Extend the graphics link audit to require exactly one Runtime registrar,
   one `Runtime_nativeLoad`, one Unix registrar, all 47 Unix JNI bodies, and
   genuine `JVM_NativeLoad` and `JVM_FindLibraryEntry`. Scan all linked inputs
   for a second complete owner of either Java class and fail on any duplicate.

## Registration

1. In `compat/darwin_libcore_natives.h` and
   `compat/darwin_libcore_natives.cc`, expose one graphics-only fail-fast
   managed-load registration step. Do not add partial method arrays.
2. Move the existing complete Unix registrar call out of
   `RegisterLibcoreNatives`. After the existing Math registration in
   `probes/runtime_entry_probe.cc`, invoke the managed-load step exactly once.
3. The step calls `register_java_lang_Runtime` once, checks for a pending
   exception, then calls `register_java_sun_nio_fs_UnixNativeDispatcher` once
   and checks again. This preserves the relevant Android 16 `OnLoad.cpp`
   order: System, existing java.io/NIO owners, Math, Runtime, Unix dispatcher,
   all before application code.
4. Treat the step as an indivisible startup gate. Each upstream
   `jniRegisterNativeMethods` call installs its complete per-class table and is
   fatal on failure. JNI provides no transactional rollback across the two
   Java classes, so do not claim cross-class rollback; abort the one-shot
   process before application code instead.

## Acceptance

1. Point the final managed gate at
   `_build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib`,
   the reconciled ICU boot JAR, and the same Android ICU runtime environment as
   the graphics probes.
2. Run the unchanged application DEX through its single ART-created
   `PathClassLoader` and its single absolute path. Require `nativeProbe=42` and
   `direct-JavaVMExt=0`.
3. Keep negative audits for a missing Unix owner and a missing Runtime owner;
   their first failures must remain `UnixNativeDispatcher.init` and
   `Runtime.nativeLoad`, respectively.
4. Run the Runtime registrar ASan/UBSan gate, the complete Unix dispatcher
   gate, the graphics link audit, the managed acceptance, and
   `probe-runtime-button`.
