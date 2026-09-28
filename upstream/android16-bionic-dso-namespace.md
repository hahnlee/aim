# Android 16 Bionic DSO namespace: first coherent providers

This gate defines the first closed symbol namespace for Android ELF DSOs on
Darwin. It intentionally does **not** call Darwin `dlsym(RTLD_DEFAULT, ...)` and
does not claim that Darwin libc is Bionic. Unknown SONAMEs, symbols, and GNU
versions fail resolution.

## Locked inputs and observed ABI

The acceptance fixture is compiled for ARM64 Android API 35 with NDK r28c
(`28.2.13676358`). The NDK headers, public stub DSOs, fixture source, real
`libc++_shared.so`, and their source-derived symbol manifests are pinned in
`android16-bionic-dso-namespace.lock`.

The real 9.2 MB `libc++_shared.so` has `DT_NEEDED` entries for `libc.so`,
`libm.so`, and `libdl.so`. It has 161 unique undefined symbols: 160 require the
Android GNU version `LIBC` (159 attributed to `libc.so`, one to `libdl.so`);
the sole unversioned import is
`__cxa_thread_atexit_impl`. It also needs `dl_iterate_phdr@LIBC`, which is not in
the first five-method loader owner. Therefore this gate must not be interpreted
as libc++ execution acceptance. A Bionic-compatible `libc.so` facade and a
loader-owned program-header iterator remain hard blockers.

## Provider policy

The first virtual namespace contains only:

- `libdl.so`: loader-owned `dlopen`, `dlsym`, `dlclose`, `dlerror`, and
  `android_dlopen_ext`, all exported as GNU version `LIBC`. The Rust bridge uses
  exact C layouts and a once-bound loader callback table. It preserves
  thread-local consume-on-read `dlerror` behavior. The loader, not Darwin dyld,
  owns handles, dependency traversal, relocation, reference counts, namespace
  scope, and `android_dlextinfo` behavior.
- `liblog.so`: the exact 18-symbol unversioned public surface of the NDK r28 API
  35 stub. Every symbol must be defined by the pinned, nine-member AOSP Android
  16 Darwin `liblog-darwin.a`. Private globals present in that archive remain
  invisible because the resolver allowlist comes from the NDK public stub.

`android_dlopen_ext` is forwarded without pretending to implement unsupported
extension flags. The eventual ELF loader must validate and implement
`ANDROID_DLEXT_*` semantics atomically. The facade only owns the public ABI and
error propagation.

Resolution takes `(requesting namespace, DT_NEEDED SONAME, symbol, optional GNU
version)` conceptually. This first API exposes SONAME/symbol/version resolution;
the loader must add requester-local/global group ordering when multiple loaded
DSOs become providers. A versioned request matches only the exact Android
version. An unversioned request may select the provider's default definition.
There is no fallback to a similarly named Mach-O symbol.

## Gate

Run:

```sh
tools/audit-android16-bionic-dso-namespace.sh
```

It verifies the locked NDK artifacts, audits the real libc++ imports, builds and
inspects an ARM64 API 35 fixture that imports all five libdl methods plus
`__android_log_print`, tests the Rust C ABI and closed namespace, and checks the
public liblog manifest against actual definitions in the Darwin archive.

Next provider order is: complete loader `libdl.so` additions such as
`dl_iterate_phdr`, then an allowlisted Android-value/ABI `libc.so` facade, then
`libm.so`; only after those close should `libc++_shared.so` enter the namespace.
