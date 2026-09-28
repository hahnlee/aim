# Darwin AndroidRuntime JavaVM ownership

The Android 16 resource JNI code calls
`android::AndroidRuntime::getJNIEnv()` from `LoaderAssetsProvider` callbacks.
This document records why the upstream host translation unit cannot be used as
a narrow provider and defines the Darwin process ownership boundary that closes
the resource module without a per-symbol stub.

## Pinned upstream experiment

`tools/materialize-android16-android-runtime-host.sh` downloads the exact
`core/jni/platform/host/HostRuntime.cpp` from `platform/frameworks/base` at
`99b01a65cc4c104933788b3143285ab6bae65827`. The source and the shared
`AndroidRuntime.cpp`/`AndroidRuntime.h` inputs are checksum locked in
`upstream/android16-android-runtime-host.lock`.

The exact upstream `HostRuntime.cpp` compiles as Darwin arm64 and genuinely
defines both `AndroidRuntime::getJavaVM()` and `AndroidRuntime::getJNIEnv()`.
It cannot, however, be dead-stripped into a resource-only provider. Its
namespace-scope `std::unordered_map` has a live global constructor containing
all host registrar function pointers. Extracting the same archive member for
`getJNIEnv()` retains that constructor and leaves exactly 17 unrelated
registrars unresolved. The gate locks both the count and the source-derived
linker manifest. Linking those modules would turn a four-TU resource boundary
into the full layoutlib runtime closure.

The device `AndroidRuntime.cpp` is not a better host boundary: it is the broad
Android process runtime implementation, includes Bionic/Binder/property/runtime
startup machinery, owns the full device `gRegJNI`, and creates/destroys the VM.
Our ART process is already created by the Darwin runtime bootstrap, so a second
AndroidRuntime VM owner would be incorrect.

## Darwin platform ownership module

`platform/darwin/android_runtime_host.cc` is a process-level port of the small
host ownership contract, not a function that returns a fabricated `JNIEnv`.
It provides one coherent boundary:

- `aim_android_runtime_install(JNIEnv*)` obtains the actual process
  `JavaVM` through `JNIEnv::GetJavaVM`, then verifies that `JavaVM::GetEnv` on
  the current attached thread returns the identical `JNIEnv`.
- `AndroidRuntime::getJavaVM()` returns that installed process VM.
- `AndroidRuntime::getJNIEnv()` uses the VM's `GetEnv(JNI_VERSION_1_6)` on every
  call. It therefore returns ART's thread-local JNI environment and returns
  null for an unattached native thread, matching upstream host behavior. It
  never invents an environment and never attaches a thread implicitly.
- `aim_android_runtime_uninstall(JNIEnv*)` validates the same VM and
  attached thread, clears ownership exactly once, and prevents process-VM
  reuse. Duplicate and wrong-VM operations have explicit statuses.

The mutex protects state transitions and readers. As with upstream
`AndroidRuntime`, VM destruction remains a process lifecycle operation:
callbacks must be quiescent before uninstall, and `DestroyJavaVM` must not race
with `getJNIEnv()`.

This is a platform port because it owns the real process JavaVM lifecycle and
implements the AndroidRuntime thread lookup semantics. A per-symbol stub would
instead return a fixed/null/fake environment without installation, VM identity,
thread attachment, or teardown invariants.

## Integration order

The runtime integration must be atomic:

1. Create ART and obtain the owner thread's real `JNIEnv`.
2. Finish the minimal runtime initialization required for managed class use.
3. Call `aim_android_runtime_install(env)`.
4. Remove the fake AssetManager registration and register the four real
   resource JNI modules in pinned `AndroidRuntime.cpp` order:
   AssetManager, StringBlock, XmlBlock, ApkAssets.
5. Register real graphics JNI and run the managed activity.
6. Quiesce callbacks and delete resource/global references.
7. Call `aim_android_runtime_uninstall(env)` on the owner attached
   thread, then call `DestroyJavaVM`.

The present gate intentionally does not modify the runtime bootstrap. Its
JNI-table ownership probe checks install, VM identity, attached/detached thread
behavior, wrong-VM rejection, one-shot uninstall, and non-reentrancy. The final
acceptance still requires the runtime to call the two lifecycle hooks around
the real resource registrar and exercise `LoaderAssetsProvider` on ART.

Run:

```sh
tools/materialize-android16-android-runtime-host.sh
tools/build-android16-android-runtime-host.sh
```

The build emits:

- `_build/android-runtime-host/libandroid-runtime-darwin-host.a`
- `_build/android-runtime-host/android-runtime-darwin-host.o`
- `_build/android-runtime-host/resource-jni-closure.bundle`
- `_build/android-runtime-host/upstream-host-unresolved.txt`
- `_build/android-runtime-host/ownership-smoke.log`

No registrar, AndroidRuntime method, or JNI environment is stubbed.
