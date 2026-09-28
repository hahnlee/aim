# Android arm64 binary128 conversion facade

This standalone gate owns exactly the pinned libc++ demand `strtold`,
`strtold_l`, and `wcstold` for `libc.so@LIBC`. Darwin arm64 `long double` is
binary64, so no C or C++ declaration returns `long double`. Three assembly
entries call raw-output cores and load the 16-byte IEEE binary128 result into
`q0`, the Android AAPCS64 return location.

The parser is the Android 16 Bionic/OpenBSD `strtorQ.c`, linked with the
existing renamed 13-TU gdtoa archive. `strtold_l` follows pinned Bionic and
ignores its locale handle without dereferencing it. `wcstold` uses Android
unsigned wchar32, ICU 76 White_Space classification, Bionic allocator results,
and the same restricted ASCII conversion span as the wide-float facade. It
never passes Android wchar32 to Darwin wchar APIs.

The product archive owns only the three entries, their raw cores, resolver,
capability query, and `aim_aosp_strtorQ`. The allocator, errno TLS,
gdtoa common parser, and ICU archives remain normal external providers and
must occur after this archive at link time. This prevents duplicate owners.

This capability is conversion-only. It does not implement Android binary128
arithmetic on Darwin, nor does it make Darwin `long double` ABI-compatible.
Callers must be Android AArch64 code or an equally exact q0 ABI bridge.

Run `./audit.sh`. The gate checks source/revision and demand hashes, AArch64
entry disassembly, raw known bits and rounding, a real NDK Android ELF that
observes q0 results, wchar32 and locale behavior, closed versioned resolution,
threading, and C/C++ ASan/UBSan builds.
