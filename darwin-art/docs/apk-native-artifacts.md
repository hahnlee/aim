# APK installation and native artifacts

## Identity and execution domains

An installed APK retains its Android identity. Java observes
`System.mapLibraryName("foo") == "libfoo.so"`, class-loader namespaces contain
Android logical SONAMEs, and the untouched APK remains the source of truth.

Native execution has two distinct domains:

1. **Complete Darwin** — a converter-produced arm64 `MH_DYLIB` graph using
   dyld, Darwin PCS, libSystem, Darwin pthread/TLS/unwind, and macOS graphics or
   other platform providers. No Android ELF calling convention, Bionic runtime
   state, ELF relocation, raw Linux syscall, or `.so` dependency may remain.
2. **Android ELF compatibility** — the original APK `lib/arm64-v8a/*.so` graph,
   mapped by the Rust ELF loader and serviced by the explicit Bionic/Android ABI
   compatibility layer.

A Mach-O container that still executes Android PCS or imports Android `.so`
files belongs to neither accepted form. It must not be labeled or selected as a
Darwin accelerator.

## Install layout

The immutable APK installation and separately replaceable derived cache are:

```text
apps/<package>/<version-code>/<apk-sha256>/
├── base.apk
└── android-elf/arm64-v8a/libfoo.so

native-artifact-cache/<apk-sha256>/<runtime-abi>/
├── libfoo.dylib
└── libfoo.dylib.contract

native-artifact-cache/<apk-sha256>/<runtime-abi>.elf-fallback
```

The Darwin directory is a cache derived from the APK, never a mutation of it.
Its contract binds the logical Android SONAME, APK SHA-256, source ELF SHA-256,
output dylib SHA-256, and runtime ABI identity. Reinstalling a changed APK or
upgrading an incompatible runtime creates a different directory rather than
silently reusing translated code.

`darwin-art-native-artifact` owns content-based admission. It verifies thin
arm64 `MH_DYLIB`, `LC_BUILD_VERSION` for macOS, `LC_ID_DYLIB`, Darwin-only load
dependencies, and the exact contract hashes. A `.dylib` extension alone has no
meaning; an ELF renamed to `.dylib` is rejected.

The publisher validates the exact file set and complete graph, flushes and
seals files read-only, and publishes the sibling staging directory with Darwin
`renamex_np(RENAME_EXCL)`. A racing winner is accepted only after its complete
graph and hashes are independently revalidated. The resolver consumes only a
sealed cache directory after publication. Code-signature policy remains part
of conversion admission before untrusted converted artifacts are accepted.

## Conversion transaction

PackageManagerService installs the APK and extracts its Android ELF graph into
the package's `nativeLibraryDir` (ADR 0009). `darwin-art-native-resolve` owns
the conversion attempt, made once per APK identity before the first launch
that names a converter. An optional converter is identified by the SHA-256 of
an absolute, regular, non-symlink executable and is invoked once for the
entire installed ELF directory. It receives the APK
hash, runtime ABI, immutable ELF directory, and a fresh private output
directory. It may emit only the exact `lib*.dylib` set corresponding to the
installed `lib*.so` set; it does not get to author admission contracts.

Rust then validates every output as a thin arm64 macOS `MH_DYLIB`, requires a
direct `@loader_path/lib*.dylib` install name, requires a terminal embedded code
signature, and admits only Apple system dependencies or `@loader_path`
dependencies contained in the same converted graph. Rust hashes each source
ELF and output dylib, creates the contracts, revalidates the exact graph, seals
it, and publishes it atomically.

Converter rejection, process failure, a missing member, an extra member, or any
invalid dylib discards the whole private staging directory. The resolver then
selects the unchanged Android ELF graph and writes an advisory negative cache
bound to the APK graph hash, runtime ABI, and converter hash. An unchanged
failed graph is not converted again on every launch; changing any of those
identities retries conversion. A present but invalid *published* Darwin cache
remains an integrity error and is never treated as a normal fallback.

## Graph-atomic selection

Selection is performed once for a closed native graph:

```text
all libraries have valid complete-Darwin artifacts
                    │
             yes ───┴─── no
              │           │
          dyld graph   Android ELF graph
```

Per-library mixing is forbidden. It would reintroduce ambiguous ABI, exception,
TLS, allocator, and teardown ownership at every dependency edge. Boundary calls
between ART and a complete Darwin JNI library are ordinary Darwin JNI calls;
boundary calls into the ELF domain continue to use the explicit Android PCS/JNI
proxy machinery.

An installed but invalid Darwin candidate is an integrity failure, not a quiet
fallback. An absent Darwin candidate is normal and selects the original ELF
graph. This distinction prevents a corrupted or partially updated accelerator
from being silently ignored.

## Conversion acceptance

An ELF-to-Darwin converter is complete only when it rewrites or regenerates all
of the following for the whole graph:

- object format, relocations, symbol binding, constructors, and visibility;
- Android AArch64 PCS calls to Darwin PCS where the conventions differ;
- Bionic libc/libdl/pthread/TLS APIs and allocator ownership;
- C++ ABI, exception, unwind, and thread-local destructor behavior;
- JNI exports, `JNI_OnLoad`/`JNI_OnUnload`, and teardown ordering;
- Linux syscall assumptions and Android platform-library imports;
- Mach-O install names, macOS platform metadata, and code signature.

Until that proof exists for a graph, the correct execution artifact is the
original Android ELF, not a partially converted dylib.

`tools/audit-complete-darwin-apk-native.sh` exercises both decisions using a
real two-library dependency graph. Its complete converter produces a signed
Darwin root dylib with an `@loader_path` child and proves the call through dyld,
`JNI_OnLoad`, and the child library. Its intentionally incomplete converter
produces no admissible graph and proves that the unchanged two-member Android
ELF graph instead runs through the compatibility loader. No per-library mixed
state is accepted in either run.
