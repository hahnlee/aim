# Android 16 ART native-library control flow and Darwin integration boundary

This audit is pinned to `android-16.0.0_r1` ART commit
`ed6c006bd06ae060bd9698fd2cb25c4865512ec3`. The gate materializes only six
files through Gitiles, without Git metadata.

## Upstream ownership and lifecycle

`JavaVMExt::LoadNativeLibrary` resolves the ClassLoader allocator first and
rejects loading the same path into another ClassLoader. It then calls
`OpenNativeLibrary`, constructs and inserts `SharedLibrary`, and only afterward
looks up `JNI_OnLoad`. Inserting first is required for recursive loads.

`needs_native_bridge` is a handle ownership tag, not a performance hint. It is
stored with the opaque handle for its entire lifetime and selects all of:

- native method lookup through `NativeBridgeGetTrampoline2` rather than `dlsym`;
- `JNI_OnLoad` and `JNI_OnUnload` lookup through the bridge;
- `NativeBridgeUnloadLibrary` rather than `dlclose`.

Mixing a handle with the wrong value is an ownership violation. For this Darwin
port, every Android ELF handle must set it to true even though both sides execute
ARM64 instructions. Mach-O handles set it to false.

For bridged lookup, ART filters libraries by the declaring ClassLoader allocator,
tries the JNI short symbol then long symbol, and passes the method shorty only to
the bridge. Regular JNI is call type 1 and includes implicit `JNIEnv*` plus
`jobject/jclass`; CriticalNative is call type 2 and does not. Upstream
`SharedLibrary::FindSymbolWithNativeBridge` currently passes length zero, so the
Darwin adapter must normalize non-null shorty with `strlen`; null plus zero is
reserved for lifecycle functions.

`JNI_OnLoad` is requested with null shorty and regular call type, then called as
`jint(JavaVM*, void*)` while ART's ClassLoader override is installed. No symbol
means success. `JNI_ERR` or a version other than 1.2/1.4/1.6 marks the library
failed. ART deliberately does not close it because registration may be partial;
later loads see the stored failure. During unloading, ART looks up and calls
`void JNI_OnUnload(JavaVM*, void*)` before the `SharedLibrary` destructor closes
the correctly tagged handle.

## Minimal atomic Darwin patch

The patch belongs in the Darwin nativeloader/nativebridge adapter, not in ART's
`JavaVMExt` state machine:

1. Bind one `AimElfLoaderV1` table before native loading. Reject wrong
   ABI version/size or partial callback tables.
2. Classify by file magic before opening. Mach-O remains on dyld. Android ELF
   goes directly to the Rust loader and sets `needs_native_bridge=true`; never
   try dyld first and infer format from its error.
3. Maintain a weak-global ClassLoader identity map in the adapter. Null uses the
   boot namespace. Each live ClassLoader maps to one persistent loader namespace;
   its search/permitted paths are passed once to `create_namespace`.
4. Implement load-ext/open, close, trampoline, error, and ownership queries in
   the same patch. Do not land an ELF open path while trampoline/close still use
   the current null/dyld stubs.
5. `get_trampoline` resolves inside the handle's closed DSO namespace, validates
   the Android GNU version, and returns a Darwin-callable fixed-signature thunk.
   Lifecycle names use their known signatures. Ordinary names use normalized
   shorty and call type. Missing symbols return null; malformed/unsupported
   shorties set a thread-local bridge error and return null.
6. Close decrements the exact Rust handle/reference graph and destroys it only
   at zero. An OnLoad failure is not an immediate close. A race loser closes only
   its own newly opened handle; the winning `SharedLibrary` keeps its handle.

The C ABI intentionally gives namespace and handle ownership to the Rust loader.
The C++ adapter owns Java references, format dispatch, ART-compatible error
strings, and the `needs_native_bridge` tag. No callback may use Darwin global
`dlsym` as fallback.

Before `open` succeeds, the loader must close the complete DT_NEEDED graph,
apply relocations, run ELF initializers, and preflight any defined `JNI_OnLoad`
or `JNI_OnUnload` into their fixed lifecycle trampoline form. ART interprets a
null lifecycle lookup as “symbol absent”; it has no separate channel for
“symbol existed but trampoline generation failed.” Such generation failure must
therefore fail `open` atomically, before ART inserts `SharedLibrary`. Undefined
lifecycle symbols remain valid and later return null. Unsupported finalizers or
other loader capabilities must likewise fail open rather than expose a partial
handle.

The existing dual-PCS proof covers fixed primitive stack repacking. This design
does not yet claim variadic JNI, aggregates/HFAs, arbitrary returns, or the full
proxy `JNIEnv` table. `NativeBridgeGetTrampolineForFunctionPointer` is also a
separate required closure for `RegisterNatives`; it must be added before claiming
general JNI compatibility.

## Gate

```sh
tools/audit-android16-native-library-control-flow.sh
```

The gate locks the upstream branches and ordering, compiles a fake loader through
the proposed C ABI, and verifies namespace isolation, immutable handle dispatch,
OnLoad failure residency, OnUnload-before-close, short/long lookup, shorty length
normalization, and regular/CriticalNative call-type preservation.
