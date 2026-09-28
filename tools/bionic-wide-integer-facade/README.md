# Bionic wide integer conversion facade

This standalone provider closes the four wide integer imports made by the
pinned Android 35 arm64 `libc++_shared.so`: `wcstol`, `wcstoll`, `wcstoul`,
and `wcstoull`. The exact manifest also contains `wcstod`, `wcstof`, and
`wcstold`; those remain explicit capability rejections for the separate wide
floating owner. The pinned demand has no wide integer `_l` variant.

The parser is a host-independent transcription of pinned Bionic
`libc/bionic/strtol.cpp`. It reads Android's unsigned 32-bit `wchar_t`
directly, uses only Bionic's ASCII classification semantics, accepts base 0 or
2 through 36 with guarded `0x` and `0b` prefixes, consumes all valid digits
after overflow, and applies modulo negation for unsigned conversions. Invalid
bases and overflow publish Android `EINVAL`/`ERANGE` only through Bionic TLS.
Darwin `errno` and floating-point environment state are preserved.

No Darwin `wchar.h`, `wctype.h`, `wcsto*`, locale conversion, dynamic ICU, or
global symbol fallback is used. In particular, an Android `wchar_t*` is never
forwarded to Darwin's signed `wchar_t` API. Surrogates, noncharacters, values
above Unicode, and `UINT32_MAX` are ordinary non-ASCII terminators.

Run `./audit.sh`. The gate verifies pinned source and manifest hashes, exact
libc++ demand and API-35 exports, Android unsigned-wchar ABI signatures, a real
Android arm64 ELF with exactly four `@@LIBC` imports plus `__errno`, a 5,084
case differential against the pinned AOSP byte instantiations, explicit
wchar32 edge cases, 8-thread TLS stress, host errno/fenv preservation, C-boundary
ASan/UBSan, closed resolution, Rust clippy/formatting, and target cleanliness.
