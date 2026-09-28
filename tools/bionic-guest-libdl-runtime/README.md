# Bionic guest libdl runtime

This new-only standalone gate executes Android guest `libdl` calls without
Darwin `dlopen`, `dlsym`, `dlclose`, or global-symbol fallback. It composes the
existing closed five-symbol `libdl.so` facade with the existing AArch64 ELF
loader and a test-only ClassLoader-namespace owner.

A real NDK r28c/API-35 root DSO imports exactly `android_dlopen_ext`, `dlopen`,
`dlsym`, `dlclose`, and `dlerror` from `libdl.so@LIBC`. Its `JNI_OnLoad` and a
named JNI native method each open an exact sibling plugin, obtain and call one
export, observe same-handle refcounting, and close it. Plugin constructors and
finalizers report through an explicit virtual provider. No symbol is resolved
through dyld.

The gate also proves per-thread, consume-once `dlerror`; unsupported flags and
non-null `android_dlextinfo` rejection; private failure rollback; and a close
that waits for an admitted concurrent lookup. It runs the host owner under
ASan+UBSan and TSan.

The semantic manifest is pinned to Android 16.0.0 r1 Bionic commit
`09a271af557444c9a6b3f3146d6d474156fd6cdb` and ART commit
`ed6c006bd06ae060bd9698fd2cb25c4865512ec3`. The older local
`_aosp/bionic-dl-iterate-phdr` snapshot is Android 15 and is deliberately not
used as Android 16 provenance. The API-35 `libdl.so` stub exposes twelve
symbols; this gate claims only five. The remaining seven are listed explicitly
in `manifests/full-libdl-exports.tsv`.

Android 16 accepts more modes than this first gate: LAZY is eagerly resolved,
NOLOAD queries residency, GLOBAL/NODELETE affect unloadability, and selected
`android_dlextinfo` fields support reserved mappings, RELRO, fd+offset, force
load, and explicit namespaces. This gate intentionally accepts only
NOW|LOCAL and a null extinfo. Everything else fails before state changes.

## Production blocker

The current ELF loader returns unique raw graph handles and requires the caller
to establish quiescence before unload. It has no API to add a dynamically
opened sibling to an existing ClassLoader namespace, no generation-safe
refcounted guest handle, and no lease that remains valid across use of a
`dlsym` result. The existing `android-dso-namespace` callback facade is bound
once for process lifetime, while production needs a live ClassLoader namespace
owner.

Therefore this gate does not modify or claim integration in
`compat/darwin_runtime_adapters.cc`. `manifests/required-loader-abi.tsv` freezes
the minimum shared loader contract. Production must land that contract,
ClassLoader ownership, provider-namespace routing, and the actual ART probe as
one atomic change.

Run:

```sh
./tools/bionic-guest-libdl-runtime/audit.sh
```
