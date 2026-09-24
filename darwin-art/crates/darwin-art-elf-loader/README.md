# AIM ELF loader vertical slice

This standalone crate proves a bounded Android `.so` execution path on Apple
Silicon. It reserves one Darwin virtual-address range for an AArch64 `ET_DYN`,
copies and zero-fills checked `PT_LOAD` segments, applies
`R_AARCH64_RELATIVE`, `R_AARCH64_ABS64`, `R_AARCH64_GLOB_DAT`,
`R_AARCH64_JUMP_SLOT`, and deferred `R_AARCH64_IRELATIVE` RELA entries, installs final page protections, executes
`DT_INIT_ARRAY` once in table order, calls a defined no-argument export, and
executes `DT_FINI_ARRAY` in reverse table order followed by `DT_FINI` at unload.

Run `./run-gate.sh` on Apple Silicon with Android NDK r28 or newer. The fixtures
are linked without libc or startup objects. A versioned virtual provider proves
real function, data, and function-pointer imports. Its addresses come only from
the caller's `SymbolResolver`; the loader never calls `dlopen`, `dlsym`, or dyld.
`DT_NEEDED` order and strings are preserved, and GNU VERSYM/VERNEED symbol,
version, flags, hidden bit, and provider SONAME are passed to the resolver.
Missing weak undefined symbols resolve to zero; missing strong symbols fail.

The pinned NDK r28c/API 35 `libc++_shared.so` is also a structural acceptance
image. With initializers disabled, the loader applies all 4,267 supported RELA
entries through a closed resolver, observes all 160 strong `@LIBC` imports,
seals GNU RELRO, and finds a real export. Its zero-valued
`DT_AARCH64_BTI_PLT` metadata is recognized. A nonzero BTI requirement is a
capability error until the Darwin execution contract for BTI landing pads is
proved; a duplicate tag is malformed ELF.

A bounded AArch64 TLS slice accepts one validated `PT_TLS` and local-definition
`R_AARCH64_TLSDESC` relocations. It uses opaque descriptor tokens and aligned
per-thread guest blocks while leaving Darwin `TPIDR_EL0` intact. Imported and
static TLS models remain explicit capabilities. The ownership and unload
contract is documented in [TLS.md](TLS.md).

The TLS registry, TLSDESC resolver, and per-thread allocation/drop path live in
`src/tls.rs`; the main loader remains responsible for parsing, relocation, and
mapping policy. This is an implementation split only: the descriptor token ABI
and unload behavior are unchanged.

`ClosedElfNamespace` adds a recursive graph layer without introducing path
search. Each ELF byte source is registered under its exact embedded
`DT_SONAME`; an optional caller-owned `SymbolResolver` is the only non-ELF
provider source. All objects are staged before relocation, so dependency cycles
can resolve against already mapped peers. Lookup is limited to the requesting
object and its breadth-first `DT_NEEDED` closure. Versioned requests are pinned
to their VERNEED provider SONAME and matched against GNU VERDEF/VERSYM exports.
Constructors run dependency-first (deterministic DFS postorder for a cycle).
Clones of one loaded graph share an owner, and the final clone close unmaps
objects in reverse constructor order, running each object's finalizers before
its mapping is released. Separate `load` calls do not reuse a namespace-global
`dlopen` cache. Non-final clones cannot trigger teardown.

The NDK r28c graph fixture loads a parent and two real AArch64 dependencies. It
also covers missing weak imports, a GNU version mismatch, an accepted two-node
cycle, unknown dependency rollback, and two independent namespaces containing
different bytes under the same SONAME. Source insertion order is deliberately
different from dependency traversal order.

This is intentionally not a general dynamic linker. PLT relocation is eager and
requires `DT_BIND_NOW`, `DF_BIND_NOW`, or `DF_1_NOW`; lazy binding is rejected.
One well-formed `PT_GNU_RELRO` range is relocated while writable and then
page-rounded and protected read-only before constructors or handle publication.
The range must stay inside one readable, non-executable `PT_LOAD`; malformed,
duplicate, or out-of-image RELRO segments fail closed. Imported/static TLS
models, `DT_REL`, `DT_RELR`, other relocation kinds, `DT_INIT`, preinit arrays,
text relocations, symbolic lookup, and RPATH/RUNPATH return explicit capability
errors.
`LoadedElf` owns the complete `mmap` reservation and unmaps it on every error
path or `Drop`. Resolver-provided addresses must remain valid until the image is
dropped.

Finalizer metadata and relocated targets are validated before constructors can
run. An image that was never initialized never runs finalizers; relocation or
preflight failure therefore tears mappings down silently. `LoadedElf::close`
and `Drop` share the same exactly-once path. Graph construction completes all
relocation/finalizer preflight before its dependency-first constructor pass, so
there is no structurally partially initialized graph. A constructor terminating
the process or unwinding across its C ABI is outside this loader's recovery
contract.

The additive per-image lifecycle seam composes Bionic C++
`__cxa_atexit`/`__cxa_finalize(dso_handle)` with ELF finalization. The graph
publishes every mmap reservation after full relocation and before constructors.
Dependent-first teardown drains and quiesces that image's callbacks before its
`DT_FINI_ARRAY` and `DT_FINI`; callback failure is fail-stop before unmap. No
global `finalize(NULL)` is used. Local `TLSDESC` remains available while guest
finalizers run; descriptors are unpublished only afterward. Language-level TLS
destructors and cross-image TLS remain rejected.

Graph exports with default or protected visibility participate in the closed
dependency scope. Nonzero `SHN_ABS` definitions are rejected explicitly rather
than being mistaken for mapping-relative addresses.

The fixtures use only the no-argument AArch64 procedure-call subset. Android JNI
functions with spilled arguments still need a per-shorty Darwin-to-Android PCS
repacking trampoline; this gate does not claim general JNI `.so` compatibility.

## C ABI seam

`include/darwin_art_elf_loader.h` exposes the crate as a static library with
bytes/path loading, a closed synchronous resolver callback, initializer control,
export lookup, and explicit unload. The additive graph ABI accepts an exact root
SONAME, an in-memory SONAME-to-bytes table, and an explicit virtual-provider
SONAME set. `graph_load` performs the complete recursive relocation and
dependency-first constructor transaction before publishing its opaque handle;
root lookup and dependent-first graph unload use that same owner. There is no C
ABI path search. Every entrypoint returns a stable structured status and
optionally fills `DarwinArtElfErrorBuffer`; `required` includes the terminating
NUL even when the message was truncated. Rust panics are caught at every
fallible C entrypoint.

Handles are unique and opaque. Init/lookup are internally synchronized and may
move across ART-attached threads. Unload may also run on another thread, but the
adapter must first establish quiescence: no concurrent ABI operation and no code
executing through a borrowed lookup/import address. Resolver request storage is
valid only during the callback, while every FOUND address must remain valid until
unload. Because lazy binding is rejected, the callback/context is not retained.

A NativeBridge open adapter must keep the handle private until initializer and
preflight success, and unload it privately on failure. Export addresses are
invalid after unload; callers own their exact ABI. The Objective-C++ smoke
validates header layout, both single-image load forms, an atomic three-object
graph load, versioned imports, cross-thread lookup/unload, lifecycle, structured
errors, and idempotent unload.
