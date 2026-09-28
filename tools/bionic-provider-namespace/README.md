# Closed Bionic provider namespace

This integration seam composes the standalone Bionic providers into one exact
SONAME/symbol/version namespace without modifying their implementations. The
generated ownership table covers all 160 of the pinned NDK r28c API-35 arm64
`libc++_shared.so`'s 160 libc-family `@LIBC` imports, the provider-owned
`pthread_create@LIBC` extension needed by general Android DSOs, and 19
implemented `liblog.so` exports. `generated/unsupported-libc.tsv` is empty;
unknown symbols remain capability errors rather than candidates for a Darwin
symbol with the same name.

The owners are leaf, allocator, Bionic errno TLS, filesystem, time, pthread,
immutable process state, loader program headers, binary stdio, wide stdio,
locale/ICU, integer
numeric parsing, AOSP gdtoa float conversion, wide integer conversion,
ICU/AOSP-gdtoa wide floating conversion, Bionic abort/message state, Bionic
binary128 conversion through Android AAPCS64 q0 entries,
syslog state/Android variadic capture, bounded formatted stdio over
provider-local Android `FILE`, exact libc++ syscall dispatch, liblog, and DSO
lifecycle, plus Linux-offset sendfile over the process filesystem owner.
`generate_manifests.py` derives the
table directly from those provider manifests, the canonical 160-import
classification, and the reviewed extension manifest. It refuses a duplicate
owner or an unreviewed extension. The pinned imports plus reviewed
extensions (`pthread_create`, central `close`, socket, DNS, and the Android
`stdout` object) are
owned by `libc.so`; loader-owned
`dl_iterate_phdr` is owned by `libdl.so`, matching that provider's actual
contract and libc++'s `DT_NEEDED`. Original ELF metadata selects default
versions. `liblog.so` accepts each implemented symbol's exact pinned version
or an unversioned default request, not arbitrary cross-symbol version aliases.

## Embedding contract

The runtime creates a namespace, binds one callback for each provider, and
seals it before loading guest code. A callback is a narrow adapter around that
provider's existing closed resolver. The namespace selects exactly one owner
before calling the adapter, and treats a zero address as provider-manifest
drift (`provider-rejected`). No `dlsym`, dyld, global namespace, or host SONAME
fallback exists here.

Provider contexts and returned addresses must remain valid through teardown.
Resolution holds an in-flight lease while the provider callback runs.
Teardown first stops new lookups, waits for all leases, then calls release
hooks once in the checked dependency order. Guest ELF finalizers and the
Bionic `__cxa_finalize` drain must run *before* this boundary, while normal
resolution is still admitted; release hooks only drop provider-owned host
state. Bionic errno is deliberately released last.

Provider APIs have several resolver signatures, so `builtin_adapters.cc`
supplies typed adapters and intentionally leaves exactly twenty-nine resolver
entrypoints undefined until the embedding binary links every provider. The
runtime provider closure now supplies those definitions, and ART's ELF graph
resolver binds and seals this namespace before mapping guest code. Optional
release hooks carry each provider's real owning context. This does not
introduce a second implementation. It also does not merge the filesystem and
stdio virtual descriptor tables; their existing capability limits still apply
after namespace composition.

Run `tools/bionic-provider-namespace/audit.sh`. It regenerates and diffs every
table, re-derives all 160 libc imports from the hash-pinned real NDK ELF,
checks the complete generated `libc.so` route set and zero unsupported libc
imports, rejects wrong SONAMEs and versions, performs 12-thread lookup stress,
routes all 689 versioned entries through the typed adapters with exact per-provider
counts (including the distinct `libdl.so` contract),
proves teardown waits for a blocked resolver and releases every provider once
in order, scans for host-loader escape hatches, and repeats the C++ boundary
under ASan/UBSan and ThreadSanitizer.
