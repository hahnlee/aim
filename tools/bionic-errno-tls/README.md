# Bionic per-pthread errno owner

This standalone module owns the first coherent Bionic `__errno()` provider for
Darwin. A private C `_Thread_local int32_t` is the Android errno cell, so every
Darwin pthread gets distinct storage initialized to zero. The exact Android
import ABI is a no-argument function returning `int*`; on ARM64 it is a
fixed, register-only call and needs no Android AAPCS64/Darwin PCS thunk.

The closed resolver recognizes only `__errno` and returns the prefixed
`aim_bionic___errno` address. It has no `dlsym`, unprefixed fallback, or
host-libc lookup. The private Darwin TLV bootstrap and internal `errno` access
used to save and restore the host value are build-audited dependencies, not
guest-visible addresses. No public API returns Darwin's errno address.

## Translation contract

`manifests/darwin-to-android.tsv` is derived by name from the existing Android
16 OsConstants value manifest and the active Darwin SDK. The gate locks the
same Bionic revision, OsConstants values, registrar translation owner, and
generation script used by the existing runtime gate. Android defines 80 errno
names in that source; Darwin defines 79 of them (`ENONET` is absent), producing
79 source-derived translations. This is a semantic name mapping, not a numeric
cast: for example Darwin `EAGAIN=35` becomes Android `EAGAIN=11`.

The translation APIs preserve host errno on every path:

- `errno_from_darwin` translates without changing Bionic TLS.
- `errno_set_from_darwin` translates and publishes only on success.
- `errno_capture_host` snapshots the current Darwin errno, translates it, and
  restores that exact host value.
- Unknown Darwin numbers return false while leaving both the output argument
  and Bionic TLS unchanged.

Unknown translation is a capability failure. A wrapper must propagate that
failure outside guest execution; it must not reuse the Darwin number, guess an
Android number, clear errno, or return an ordinary failure with stale errno.
Android code may still explicitly write any `int32_t` through `__errno()`, as
normal C permits.

## Allocator and future-wrapper ABI

The allocator facade's result seam already returns
`AimBionicAllocationResult { pointer, bionic_errno }`. Integration is
explicit and is not wired by this module:

```c
AimBionicAllocationResult result =
    aim_bionic_malloc_result(size);
if (result.pointer == NULL) {
  aim_bionic_errno_publish_result(result.bionic_errno);
}
```

`publish_result(0)` leaves the thread's previous errno unchanged, matching the
rule that successful libc operations do not clear errno. A nonzero result is
already an Android errno and is stored verbatim. The gate links the existing
allocator implementation and proves success preserves the old cell while an
`ENOMEM=12` failure publishes 12 without changing host errno.

Future Darwin syscall wrappers should capture the host failure immediately,
before logging or another host call, then use `errno_capture_host`. If it
returns false, the wrapper must stop at the capability boundary. Wrappers that
already saved a Darwin error use `errno_set_from_darwin`; wrappers that produce
an Android-native error use `errno_publish_result`.

## Executable evidence

`probes/fixture.c` is compiled by Android clang into an ELF64 AArch64 shared
object against Bionic `errno.h`. Its dynamic symbol table contains a real
undefined `__errno`, and its exported runner obtains, writes, and rereads the
cell using Bionic's declaration and `errno` macro.

The Darwin E2E launches two simultaneous pthread-backed Rust threads. Each
loads and executes the Android ELF through a closed resolver, writes a different
Android errno, and reports its cell address. The gate proves the addresses and
values differ, neither cell aliases that pthread's Darwin errno, both host errno
sentinels survive, and the main pthread remains zero. A separate native pthread
test covers known/unknown mappings and the result seam under ASan/UBSan.

Run `tools/bionic-errno-tls/audit.sh`. It verifies pinned AOSP Bionic sources,
the existing OsConstants source/hashes, regenerated manifests, exact Mach-O and
ELF namespaces, actual Android `__errno` import, closed resolution, pthread TLS
isolation, allocator interoperability, formatting, and Clippy.

Bionic's `__errno.cpp` is BSD-licensed; the OsConstants/libcore inputs are
Apache-2.0. Only hashes, coordinates, and derived numeric mappings are stored
here.
