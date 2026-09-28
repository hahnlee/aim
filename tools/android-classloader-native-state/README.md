# Android ClassLoader native-library state

This standalone gate fixes the ownership contract that the AIM adapter
must apply before one Android ELF graph can be shared with ART. It does not call
Darwin `dlopen`, `dlsym`, `dlclose`, dyld, or the production runtime adapter.

The upstream behavior is pinned to Android 16.0.0 r1 ART commit
`ed6c006bd06ae060bd9698fd2cb25c4865512ec3`. Five exact sources cover ART's
process-wide path cache, weak-global ClassLoader lifetime, ClassLoader linker
namespaces, boot loading, and NativeBridge trampoline/unload routing. Run:

```sh
./tools/android-classloader-native-state/audit.sh
```

## Required production contract

- Canonicalize ART's boot ClassLoader to loader identity `0`. Boot libraries use
  the system/APEX route, never a per-app namespace, and remain resident until VM
  shutdown.
- Assign every non-boot ClassLoader a stable, non-reusable identity. Do not use a
  raw `jobject` address. ART compares loader allocator identity for its library
  cache and `IsSameObject` on a namespace weak global; the embedding must make
  those views name the same generation.
- Keep one isolated Android ELF namespace per ClassLoader identity. All graphs
  from one loader reuse it; different loaders never share app-local graphs or
  symbol search. Public/system providers may be explicitly linked, not searched
  through Darwin's process-global namespace.
- Key ART's resident library cache by the exact path string. A same-path,
  same-loader repeat reuses the one resident owner and does not acquire another
  graph reference. The same path from another loader is rejected. Path
  canonicalization/inode comparison is intentionally not claimed because AOSP
  itself leaves that as a TODO.
- Publish a pending cache entry before `JNI_OnLoad`. A recursive load on the
  initializing thread succeeds; other threads wait for the one result. A failed
  `JNI_OnLoad` stays resident and every later attempt reports the stored failure.
- Maintain a handle-to-owner table for every NativeBridge graph. Trampoline
  lookup, JNI method lookup, `JNI_OnUnload`, and close must validate that exact
  handle, loader generation, and namespace. Maintain a second live
  trampoline-address-to-owner index for
  `NativeBridgeIsNativeBridgeFunctionPointer`; remove its entries during the
  owner drain before graph close. Never infer ownership from a pointer tag or a
  single process-global "current graph".
- A cleared non-boot ClassLoader weak global removes its paths from ART lookup.
  `JNI_OnUnload` runs before the library weak global is deleted, and deletion
  happens before graph close/finalizers/unmap. New loads of the same path wait
  until that sequence finishes. Namespace metadata may remain process-resident;
  a cleared weak identity must never be rebound to a new loader.

## Intentional hardening over AOSP r1

Android opens outside `jni_libraries_lock_`. Two first loads can therefore each
open a handle; the race loser destroys its temporary owner and waits on the
winner. In r1, that loser branch does not repeat the ClassLoader allocator check,
so simultaneous first loads from different ClassLoaders can observe the winner's
result instead of the normal cross-loader rejection. The Darwin contract
reserves the path before open and rejects a different loader under the same
lock. This preserves the JNI isolation rule while also avoiding a transient
second ELF graph, which the current provider-owner lifecycle cannot safely host.

The executable probe covers same-path reuse and one-owner refcount, per-loader
namespace reuse/isolation, null boot behavior, recursive and 24-way concurrent
load, failed-OnLoad residence, cross-loader rejection, NativeBridge owner lookup,
exact per-generation function-pointer ownership, deferred collection during
OnLoad, serialized reload during teardown, and
`JNI_OnUnload -> DeleteWeakGlobalRef -> close`. It runs under ASan, UBSan, and
TSan. These tests define the integration boundary; they do not yet integrate the
state machine into `compat/darwin_runtime_adapters.cc`.

## Current integration blockers

- `OpenNativeLibrary` currently allocates a fresh `ElfLibrary` and full provider
  graph for every Android ELF open and ignores `class_loader`. The adapter needs
  the locked path reservation and weak-global identity registry before graph
  discovery begins.
- ART owns the `JNI_OnLoad` pending/result condition outside the adapter. If two
  first opens are coalesced below ART, each returned adapter lease must increment
  a handle refcount: destruction of ART's race-loser `SharedLibrary` may release
  only its lease, never the winner's graph. A successful cache hit at ART level
  still leaves the underlying graph owner count at one.
- Filesystem/provider activation is partly process-global and currently rejects
  a second graph with a different filesystem authority. Per-ClassLoader
  isolation therefore requires either namespace-owned provider instances or an
  explicit, immutable shared authority before two app roots can coexist.
- `NativeBridgeIsNativeBridgeFunctionPointer` uses fixture-global acceptance
  instrumentation rather than a live per-owner trampoline index. It must use
  the generation-safe index described above, and active trampoline calls must
  drain before those entries and the graph are destroyed.
- `CloseNativeLibrary` receives only the opaque handle and bridge bit. The handle
  must resolve to its cache owner and lease generation; a stale, foreign, or
  already-released handle must fail closed without entering graph teardown.
