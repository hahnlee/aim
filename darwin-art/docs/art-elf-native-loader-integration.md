# ART Android ELF JNI integration boundary

This is the current contract for loading Android AArch64 ELF JNI graphs through
ART on macOS. It is not a claim of general Android `.so` compatibility.
Historical experiment and stage logs live in Git.

## Ownership and admission

The host selects one trusted package directory. The filesystem broker opens the
root and recursively named siblings with `O_NOFOLLOW`, regular-file checks and
fixed file/count/size caps. The same descriptor supplies metadata and bytes.
SONAME providers are never reopened from disk; RPATH/RUNPATH, dyld search and
alternate paths are excluded. The host must prevent concurrent writes to
authorized inodes.

The closed namespace contains the discovered graph plus explicit reviewed
Bionic/host providers. Unknown libraries, symbols, relocations and malformed
metadata fail before publication. Android system libraries are providers, not
disk siblings. Non-UTF-8 path components are preserved, while non-UTF-8
SONAME/DT_NEEDED remains unsupported by the current string-keyed namespace.

## Implemented contract

- Recursive root → child → grandchild discovery and eager relocation.
- Immutable image ranges, GNU RELRO sealing and reverse dependency teardown.
- Local AArch64 TLSDESC with per-thread aligned blocks and live-thread unload
  rejection. Imported/static TLS and TLS destructors remain unsupported.
- Exact pinned NDK `libc++_shared.so` import census (160/160) and execution,
  including collection and cross-frame exception fixtures.
- APK extraction for bounded arm64 stored/deflated entries with ZIP agreement,
  CRC verification and atomic read-only publication.
- `extractNativeLibs=false` read-only 16 KiB-aligned APK slices without copying.
- Closed Bionic filesystem, descriptor, numeric-loopback network and DNS
  providers with process-scoped leases.
- Guest `libdl` standalone `dlopen`/`dlsym`/`dlclose`/`dlerror` and
  `android_dlopen_ext` ownership.

## ART and JNI lifecycle

`JavaVMExt::LoadNativeLibrary` keeps Mach-O on `dlopen` and gives an admitted ELF
graph a private Rust handle. Discovery, relocation, constructors and JNI proxy
preflight complete before publication. ART receives
`needs_native_bridge=true`; close dispatch distinguishes ELF and dyld handles.

`JNI_OnLoad` runs through a closed proxy `JavaVM/JNIEnv`; guest code never gets
ART's raw function tables. The bounded proxy covers class lookup, one table of
up to 32 regular-JNI registrations, scalar/reference descriptors, modified
UTF-8, references, byte arrays and exception observation. Named JNI,
CriticalNative, aggregates/HFA and unreviewed varargs remain rejected.

Generated Darwin-entry thunks repack Android ARM64 regular-JNI arguments. Their
page transitions RW → RX, belongs to the `ElfLibrary`, and is unpublished before
graph finalization. Raw ELF pointers never enter ART. Registration failure
unregisters the class and destroys the unpublished generation. Shutdown requires
external quiescence and returns executable-page and graph counts to zero.

Per-image `__cxa_finalize(dso_handle)` callbacks drain before ELF finalizers;
dependents finish before dependencies and no global finalize shortcut is used.
The signal-chain seam restores ART's dispatcher at the front while retaining a
displaced handler as the next action.

## Evidence and open work

The actual-ART gates prove recursive constructors/finalizers, generic and fixed
RegisterNatives tables, JNI ABI repacking, W^X thunks, pinned libc++, local TLS,
APK extraction/slices and loopback network quiescence. Run:

```sh
cargo run -q -p art-bootstrap -- probe-runtime-elf-jni
cargo run -q -p art-bootstrap -- probe-runtime-network
bash tools/audit-android16-register-natives-bridge.sh
bash tools/android-apk-native-extract/audit.sh
```

Remaining production work includes ClassLoader-scoped dynamic sibling insertion,
generation handles and use leases for returned `dlsym` pointers, broader JNI
coverage, external DNS/Internet policy and complete NativeLoader cutover. The
anonymous RW→RX page passes development Apple Silicon gates; hardened runtime
still needs explicit `MAP_JIT`/entitlement acceptance.
