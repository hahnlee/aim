# Android managed native loading

This gate closes the distinction between calling ART's
`JavaVMExt::LoadNativeLibrary` from C++ and executing Android application's
managed `System.load` path. It is pinned to Android 16 revisions for libcore
and ART and to the exact prebuilt `core-oj.jar` used by the Darwin runtime.

Run:

```sh
./tools/android-managed-native-load/audit.sh
```

The graphics-runtime build may prepare the pinned sources, Runtime registrar
archive, DEX, and ELF without recursively executing the runtime acceptance:

```sh
./tools/android-managed-native-load/audit.sh --build-only
```

This mode still runs the source, bytecode, exact-symbol, ASan/UBSan, DEX, ELF,
format, shell, and whitespace gates. It skips only the final ART process.

The gate builds three artifacts under `_build/android-managed-native-load`:

- `libopenjdk-runtime-managed-load-darwin.a`, compiled from the unmodified
  Android 16 `Runtime.c`. Its single registrar owns the complete six-method
  `java.lang.Runtime` native table atomically.
- `classes.dex`, containing a real app class whose `loadAbsolute` method invokes
  `System.load` and then a native method, plus a launcher-compatible `Hello`
  class. The DEX is merged with the existing app probe classes and executes
  through exactly one ART-created `PathClassLoader`.
- `libmanaged-native-load.so`, an Android arm64 ELF with no `DT_NEEDED`
  dependencies. It exports `JNI_OnLoad` and the named JNI method that returns
  42.

## Android 16 control flow

The executable Android fork is:

```text
System.load
  -> Runtime.load0
  -> Runtime.nativeLoad
  -> Runtime.c Runtime_nativeLoad
  -> JVM_NativeLoad
  -> JavaVMExt::LoadNativeLibrary
```

`System.loadLibrary` first enters `Runtime.loadLibrary0`, asks a non-boot
ClassLoader for `findLibrary`, applies the exact PathClassLoader fallback to
`System.mapLibraryName`, and then enters the same `Runtime.nativeLoad` seam.

Android 16 explicitly comments out the upstream `ClassLoader.NativeLibraries`
field and load methods. It is therefore neither the owner nor an executable
step on Android's path. The source and prebuilt DEX audits freeze this divergence
so an OpenJDK control-flow description cannot be reported as Android evidence.

For current Android 16 policy, `Runtime.load0` evaluates its read-only dynamic
code check before calling `nativeLoad`. That initializes NIO's default file
system and reaches `sun.nio.fs.UnixNativeDispatcher.init`. The current minimal
runtime does not retain/register that existing module-complete native table, so
the actual DEX gate freezes it as the first missing native. Only after that
owner is installed can `Runtime.nativeLoad` become the next boundary.

`Runtime.c` registers six methods together: `freeMemory`, `totalMemory`,
`maxMemory`, `nativeGc`, `nativeExit`, and `nativeLoad`. Integrating only
`nativeLoad` would split an upstream native table between owners and is rejected
by this design. `JVM_NativeLoad` is already supplied by the genuine
`libopenjdkjvm` archive; it delegates to ART and preserves the calling class,
ClassLoader, error string, and pending-exception behavior.

## Acceptance boundary

The actual ART run is not allowed to call `JavaVMExt` from the host as a
substitute. Java obtains one absolute path from the test environment, invokes
`System.load` once, and calls the loaded DSO's native method. Before registrar
integration, the same real DEX run must stop at the first missing
`UnixNativeDispatcher.init` implementation and the gate reports `BLOCKED` with
the managed `Runtime.load0` stack. The ready Runtime registrar archive is the
next complete owner, not a substitute for that earlier NIO dependency. After
both complete tables are linked and registered, the unchanged gate must reach
`nativeProbe=42` and report `direct-JavaVMExt=0`.

The scope intentionally excludes `System.loadLibrary` execution, multiple
ClassLoaders, multiple paths, recursive managed loads, APK `findLibrary`
resolution, and the removed `NativeLibraries` implementation. The source and
prebuilt bytecode branches for named loading are audited, but only the single
absolute-path vertical slice is claimed at runtime.
