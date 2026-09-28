# Android 16 resource JNI Darwin slice

This gate pins `platform/frameworks/base` at
`99b01a65cc4c104933788b3143285ab6bae65827` and extracts the four complete
resource JNI translation units selected from `libandroid_runtime`'s
checksum-verified `core/jni/Android.bp`:

- `android_util_AssetManager.cpp` — 57 native methods
- `android_content_res_ApkAssets.cpp` — 12 native methods
- `android_util_StringBlock.cpp` — 5 native methods
- `android_util_XmlBlock.cpp` — 22 native methods

`tools/materialize-android16-resource-jni.sh` downloads eleven individual,
hash-locked files without Git metadata. `tools/build-android16-resource-jni.sh`
compiles all four TUs for Darwin arm64, verifies the 96-method source closure,
checks all four registrars, and emits:

- `_build/resource-jni-foundation/libandroid-resource-jni-darwin.a`
- `_build/resource-jni-foundation/android-resource-jni-force-loaded.o`
- `_build/resource-jni-foundation/undefined-symbols.txt`
- `_build/resource-jni-foundation/resource-registrar-order.txt`

The `nativeIsUpToDate(long)` CriticalNative entry uses Android's no-`JNIEnv` ABI
under a narrow Darwin build define. No individual native method is replaced or
stubbed.

## Provider closure

After linking the existing module-complete androidfw, ART/device-layout
nativehelper (`java.io.FileDescriptor.descriptor:I`), utils,
cutils, log, libbase, ZIP/incfs, ICU, PNG, and zlib providers, the only
non-system unresolved symbol is:

```text
android::AndroidRuntime::getJNIEnv()
```

This is used by `LoaderAssetsProvider` callbacks. Upstream's provider lives in
`platform/host/HostRuntime.cpp`, but that TU also owns the complete host runtime
registrar and therefore imports resource, graphics, media, input, system, and
other JNI modules. Pulling one method out as a local stub would violate module
ownership. Runtime integration must either link the complete upstream host
runtime closure or add a reviewed Darwin AndroidRuntime ownership boundary that
uses the process JavaVM.

## Runtime registration order

The checksum-locked Android 16 `AndroidRuntime.cpp` `gRegJNI` order is:

```text
register_android_content_AssetManager
register_android_content_StringBlock
register_android_content_XmlBlock
register_android_content_res_ApkAssets
```

For this port, real resource registration must be atomic with removal of the
current fake AssetManager table. It should run after `FinishMinimalForDarwinProbe`
and before the real graphics registrar, so managed framework classes can finish
initialization before the resource and graphics registration code performs
class/field lookups.

The local `framework-res.apk` currently has SHA-256
`eb79e2971aea57e7be5ce5e60699b26b4f94341b968a6f7c95b3c787ab03e245`
and size 36,581,050 bytes, but it has no authoritative `sources.lock`
provenance yet. It must be revision/source pinned before the Button resource
pipeline is integrated.
