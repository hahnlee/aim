# Android 35 Bionic floating conversion facade

This standalone Darwin arm64 provider closes the coherent binary32/binary64
subset of the four floating conversion imports made by the pinned NDK r28c API
35 arm64 `libc++_shared.so`:

- supported: `strtod`, `strtof`;
- rejected: `strtold`, `strtold_l`.

Android system `libc++.so` additionally imports `strtod_l@LIBC_O` and
`strtof_l@LIBC_O`. Bionic implements both as locale-ignoring calls to the base
parsers, so the same AOSP gdtoa implementations provide those two extensions.

The supported functions are the pinned Bionic/OpenBSD gdtoa implementation,
compiled under private names rather than forwarded to Darwin `strtod` or
`strtof`. The thirteen-file parser closure preserves Bionic's C-locale grammar,
hexadecimal floats, infinity and NaN parsing (including payload bits), exact
binary rounding, end-pointer behavior, signed zero, and `ERANGE` overflow and
underflow behavior. Parser errno is translated only to the existing Bionic TLS
owner; the caller's Darwin errno and complete floating-point environment are
restored after every call.

## Lock and ABI boundaries

Bionic's gdtoa sources use two internal mutex identities. Android's compact
`pthread_mutex_t` storage cannot be cast to Darwin's opaque mutex type, so the
host build replaces only the upstream lock macros with a locked identity side
table of provider-owned Darwin `std::mutex` objects. The pinned arithmetic,
tokenization, bigint, hexadecimal, NaN, and rounding code is unchanged. No
Android pthread object crosses this boundary.

Android arm64 `float` and `double` match Darwin arm64 at 4 and 8 bytes, including
their register ABI. Android arm64 `long double`, however, is 16-byte IEEE
binary128 with 16-byte alignment; Apple arm64 defines `long double` as the
8-byte double format. `strtold` and `strtold_l` therefore have incompatible
return ABIs and representations. They are explicitly rejected rather than
silently narrowed, and `strtold_l`'s otherwise locale-ignoring wrapper cannot
make that ABI safe.

## Provenance and gate

Run:

```sh
tools/bionic-float-conversion-facade/audit.sh
```

The gate derives the exact four-symbol demand from the canonical 160-import
manifest, verifies NDK and Bionic source hashes, and sparsely materializes 23
files without Git metadata. Those files include the Android.bp gdtoa closure,
Bionic long-double and locale wrappers, and the AOSP stdlib test corpus.

Acceptance includes Android and Darwin ABI assertions, a real Android arm64 ELF
with exactly `strtod`, `strtof`, `strtod_l`, `strtof_l`, and `__errno`,
including the `LIBC_O` version boundary for the locale wrappers, bit/end/errno differential
checks against the directly compiled AOSP implementation over 47 strings and
all four rounding modes, NaN/hex/subnormal boundary cases, 8x1000 concurrent
calls, C/C++ ASan plus UBSan, host errno/fenv preservation, Rust loader
execution, and closed-resolver rejection of both binary128 functions. The
single UBSan shift checker is explicitly disabled because pinned `gethex.c`
shifts a signed hexadecimal nibble into bit 31 before consuming the resulting
32-bit pattern; every other enabled UBSan check is fail-fast. Build artifacts are
written to `_build/bionic-float-conversion-facade`; Cargo targets remain in the
gate's temporary directory.
