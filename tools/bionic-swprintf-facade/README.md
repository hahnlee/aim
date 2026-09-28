# Bionic `swprintf` facade

This bounded provider closes the exact `swprintf@LIBC` demand in pinned NDK
r28c `libc++_shared.so`: `to_wstring(float|double)` calls `%f`, while
`to_wstring(long double)` calls `%Lf`. Android arm64 uses unsigned 32-bit
`wchar_t` and IEEE binary128 `long double`; neither value is forwarded to a
Darwin wide or long-double ABI.

The AArch64 entry captures v0-v7 and the caller stack. `%f` reuses the accepted
Android `va_list` formatting owner, then widens its ASCII result to Android
wchar32. `%Lf` decodes q0 as IEEE binary128 and calls the pinned AOSP/OpenBSD
`gdtoa.c` with a 113-bit FPI descriptor and Bionic's mode-3/default-six-digit
contract. The provider preserves signed zero, infinity/NaN spelling, host
errno, and the active floating-point rounding mode. It does not use Darwin
`swprintf`, `vswprintf`, `wchar_t`, `long double`, or host formatted I/O.

Only the two exact formats `%f` and `%Lf` are accepted. Width, precision,
additional arguments, other conversions, and arbitrary general `swprintf`
grammar fail closed with Bionic `ENOTSUP`. A zero-sized buffer fails with
`EINVAL`; truncation NUL-terminates the final element and fails with Android
`EOVERFLOW`. This is sufficient for the pinned libc++ owner but is not a claim
of a complete Bionic wide-formatting subsystem.

Run `./audit.sh`. It pins Android 16 Bionic `stdio.cpp`, `vswprintf.c`, and
`gdtoa.c`, the exact NDK artifact, actual Android AArch64 imports and q0 call
sites, the closed resolver, normal/ASan/UBSan execution, and the absence of a
module-local Cargo target. The pinned gdtoa normalization expression performs
a deliberate signed shift, so only UBSan's `shift` checker is disabled for that
one upstream TU; the provider and assembly boundary retain fail-fast UBSan.
