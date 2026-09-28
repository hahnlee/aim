# Android 16 RegisterNatives function-pointer bridge

This gate is pinned to AOSP `android-16.0.0_r1` ART commit
`ed6c006bd06ae060bd9698fd2cb25c4865512ec3`. It covers the path that is
different from normal JNI symbol lookup: an Android ELF library obtains its own
function address and passes it to `JNIEnv::RegisterNatives`.

## Exact upstream flow

`JNIImpl::RegisterNatives` validates the table, strips the obsolete `!` fast-JNI
descriptor prefix, resolves the declared native `ArtMethod`, and then evaluates:

```text
class-loader namespace is bridged
    OR NativeBridgeIsNativeBridgeFunctionPointer(fnPtr) [bridge v8]
                         |
                         v
method.GetShorty() + exact nonzero length
method.IsCriticalNative() -> call type 2, otherwise type 1
                         |
                         v
NativeBridgeGetTrampolineForFunctionPointer [bridge v7]
                         |
                         v
ClassLinker::RegisterNative
                         |
                         v
RuntimeCallbacks::RegisterNativeMethod may replace the pointer
                         |
                         v
ArtMethod JNI entrypoint (or delayed CriticalNative entrypoint)
```

`RegisterNatives` supplies the real shorty length, unlike the separate native
symbol-lookup flow audited by `android16-native-library-control-flow.md`.
Regular and FastNative both use call type 1 and have implicit `JNIEnv*` plus
`jobject`/`jclass`. CriticalNative uses type 2 and has neither implicit argument.
Static versus instance is not a PCS cache-key dimension because its second
regular-JNI argument is pointer-sized in both cases.

`ClassLinker::UnregisterNative` only restores the JNI dlsym lookup stub. There is
no native-bridge callback to release one registered-function trampoline. The
bridge therefore caches until its loader closes the exact image generation,
after `JNI_OnUnload` and class-loader teardown. The key is:

```text
(loader image id, load generation, Android function address,
 exact shorty bytes, JNI call type)
```

Both image identity and generation are necessary: a later `mmap` can reuse the
same virtual address. The loader's executable `PT_LOAD` interval is the sole
ownership authority; Darwin global `dlsym` and guessed address provenance are
forbidden. A successful ownership lookup also acquires a temporary image lease;
the loader removes the image from lookup and waits for leases to drain before
unmapping and retiring the cache generation. This closes the lookup-to-thunk
creation race.

## Required narrow aim patch

The pinned source exposes an important current blocker. Both
`IsClassLoaderNamespaceNativelyBridged` and `GenerateNativeBridgeTrampoline` in
`runtime/jni/jni_internal.cc` are guarded by `ART_TARGET_ANDROID`. A aim
build currently reports every namespace as unbridged and returns `fn_ptr`
unchanged. An Android ELF `RegisterNatives` therefore installs an
Android-PCS address directly as a Darwin-callable entrypoint.

Do not globally define `ART_TARGET_ANDROID`; that also selects unrelated Bionic,
nativeloader, and platform behavior. The atomic integration patch is limited to
these boundaries:

1. Add a Darwin branch to `IsClassLoaderNamespaceNativelyBridged` which asks the
   existing Darwin loader's weak-global ClassLoader-to-namespace registry. Every
   namespace that can load Android ELF is marked bridged; Mach-O-only namespaces
   remain false.
2. Add a Darwin branch to `GenerateNativeBridgeTrampoline`. Keep ART's
   `GetShorty` and `IsCriticalNative` derivation, then call
   `aim_get_registered_native_trampoline` instead of returning `fn_ptr`.
3. Replace the Android-only ownership predicate in the OR expression with a
   small platform dispatch: Android uses
   `NativeBridgeIsNativeBridgeFunctionPointer`; Darwin uses
   `aim_is_android_function_pointer` backed by live ELF executable
   ranges.
4. Before `ClassLinker::RegisterNative`, handle a null Darwin trampoline as a
   registration error with a pending `UnsatisfiedLinkError`. Upstream assumes a
   compatible bridge never returns null and would otherwise hit
   `CHECK(native_method != nullptr)`.
5. Wire image-close to
   `aim_registered_native_cache_retire_image(image_id, generation)`.
   Never retire on `UnregisterNatives`; no such notification reaches the bridge.

These changes and the function-pointer bridge must land atomically. Enabling
only the namespace predicate converts a silent raw-pointer install into a null
trampoline crash; enabling only the trampoline function leaves unmarked
classloader pointers dependent on the v8 fallback.

## Fixture acceptance

The existing dependency-free NDK r28c/API 35 AArch64 fixture contains hidden
local implementation symbols inside one executable `PT_LOAD`:

| Method | Descriptor | ART shorty | ELF value |
|---|---|---|---:|
| `nativeAdd` | `(IJI)J` | `JIJI` | `0x5748` |
| `nativeSpill` | `(ZBCSIJLjava/lang/Object;FDFDFDFDFFD)J` | `JZBCSIJLFDFDFDFDFFD` | `0x5788` |
| `nativeUsesEnv` | `()I` | `I` | `0x5880` |
| `nativeNarrowStack` | `(IIIIIIZBCSIJLjava/lang/Object;)I` | `IIIIIIIZBCSIJL` | `0x58e4` |
| `nativeEcho` | `(Ljava/lang/Object;)Ljava/lang/Object;` | `LL` | `0x5970` |
| `nativeFloat` | `(F)F` | `FF` | `0x5978` |
| `nativeDouble` | `(D)D` | `DD` | `0x5984` |
| `nativeVoid` | `()V` | `V` | `0x5990` |

The mixed method exhausts both integer and floating register classes, then
places two 32-bit floats before a 64-bit double on the stack. Darwin compacts
the floats while Android assigns each an eight-byte slot, so the layouts
genuinely diverge. `nativeNarrowStack` separately forces byte, halfword, word,
and doubleword values into Darwin's naturally packed tail. The standalone
registered-native cache smoke still uses fake callable functions to isolate
ownership and retirement, while the ART runtime gate executes the real
regular-JNI scalar/reference shorty generator. A separate generic graph calls
proxy `GetEnv`, `FindClass`, and `RegisterNatives` for one `(IJI)J` method and
executes it through ART before the exact table is installed. The current
production boundary allows one static-method table of at most 32 entries per
graph; repeated tables, instance methods, and CriticalNative remain explicit
fail-closed capabilities.

## Gate

```sh
tools/audit-android16-register-natives-bridge.sh
```

The gate sparsely materializes only five pinned ART files without Git metadata,
rebuilds and audits the existing Android ELF fixture, and emits a Darwin arm64
archive plus standalone smoke under `_build/register-natives-bridge`.
