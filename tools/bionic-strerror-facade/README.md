# Bionic `strerror_r` facade gate

This new-only standalone provider closes the one real libc++ Class-B demand
`strerror_r`. The NDK r28c API 35 arm64 header and an Android compile probe pin
that import to the XSI ABI:

```c
int strerror_r(int android_errno, char* caller_buffer, size_t size);
```

With `_GNU_SOURCE`, the same header exposes the GNU `char*` signature but
renames its dynamic symbol to `__gnu_strerror_r`. That is a different ABI and
not the manifest import owned here. The census also finds no libc++ imports for
`strerror` or `strsignal`, so this resolver deliberately does not claim them.

## Android semantics

The message table is generated from the pinned Android 16 Bionic
`private/bionic_errdefs.h`. It contains 132 distinct numeric entries, including
every numeric value in the existing 80-name Android errno census. The provider
does not pass an Android errno number to Darwin `strerror_r`, and its object has
no host string-function dependency.

Known values copy the exact Bionic English message. Unknown positive and
negative values produce `Unknown error N`. Copy behavior matches Bionic
`strlcpy`: a non-empty destination is always NUL terminated, size zero performs
no write, and any message whose full length is greater than or equal to the
buffer size returns the positive Android `ERANGE` value 34. Success returns
zero. The caller-owned output is the only returned storage; no host global or
thread-local message pointer crosses the boundary.

Bionic declares the XSI buffer non-null. Within that contract, every path is
allocation-free and deterministic. The shim saves and restores Darwin errno;
the function also leaves the separate Bionic errno TLS cell unchanged, exactly
as Bionic's `ErrnoRestorer` does.

## Gate

`./audit.sh` hashes the pinned Bionic `strerror.cpp`, `bionic_errdefs.h`, and
`string.h`, plus the matching NDK headers, libc++ demand manifest, Android OS
constant census, and errno TLS provider. It regenerates the 132-entry table,
locks both XSI and GNU signatures, and proves the provider object imports only
Darwin's private errno accessor—never host `strerror` or `strerror_r`.

The AOSP-derived differential covers errno values -256 through 512 and buffer
sizes 0 through 64, comparing return values, all output bytes, truncation, and
NUL behavior. A real Android AArch64 ELF imports exactly `strerror_r` and
`__errno`; the closed resolver exercises representative equal-number,
Darwin-different, Android-only, unknown, negative, and boundary cases. The
differential and complete ELF boundary repeat under C ASan and UBSan, followed
by all-target Clippy and formatting checks. All build targets are temporary.
