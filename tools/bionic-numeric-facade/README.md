# Bionic integer numeric facade

This standalone provider closes the seven integer-parsing imports made by the
pinned Android 35 arm64 `libc++_shared.so`: `strtol`, `strtoll`, `strtoimax`,
`strtoul`, `strtoull`, `strtoll_l`, and `strtoull_l`.

The parser is a host-independent transcription of the pinned Bionic
`libc/bionic/strtol.cpp` behavior. It accepts base 0 or 2 through 36, including
Bionic's guarded `0x` and `0b` prefixes, consumes all valid digits after an
overflow, publishes Android `EINVAL`/`ERANGE` only through Bionic TLS, and
preserves Darwin `errno`. Unsigned leading minus uses modulo negation unless
the magnitude itself overflows. No Darwin `strto*` function or dynamic/global
symbol fallback is used.

The pinned `stdlib_l.cpp` wrappers ignore their locale argument and delegate
to the non-locale parser. The two demanded `_l` functions therefore accept any
opaque Android `locale_t` bit pattern without dereferencing it. The pinned
libc++ does not import `strtol_l` or `strtoul_l`, so they are intentionally not
exported.

Floating parsers (`strtod`, `strtof`, `strtold`), wide parsers, long-double
conversion, and formatted varargs are out of scope and rejected by the closed
resolver. They need separate coherent algorithms and ABI gates.

Run `./audit.sh`. The gate verifies source hashes and semantic anchors, exact
libc++ demand, API-35 `@@LIBC` exports, Android header signatures, a real
Android arm64 ELF with exactly seven numeric imports plus `__errno`, differential
tests against the pinned AOSP implementation, 8x1000 TLS stress, C
ASan/UBSan, Rust clippy/formatting, and target cleanliness.
