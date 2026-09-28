# Closed Bionic formatting facade

This standalone provider owns `snprintf`, `vsnprintf`, and `vasprintf` without
calling Darwin formatted I/O. `snprintf` enters through an arm64 assembly shim
that captures Android AAPCS64 x0-x7, v0-v7, and the caller stack. The two `v*`
functions consume Android's 32-byte `{stack, gr_top, vr_top, gr_offs, vr_offs}`
`va_list` directly; Darwin arm64 uses an incompatible 8-byte stack pointer.

Supported conversions are integer `diuoxX`, narrow `c/s`, pointer `p`, and
double `f/e/E/g/G`, with non-positional width and precision. `%n`, positional
arguments, `long double`, locale grouping, wide strings, and unknown conversions
fail closed with Bionic `ENOTSUP`. `vasprintf` allocates exclusively through the
Bionic allocator facade and every error is written to Bionic errno TLS.

`fprintf`/`vfprintf` remain rejected because this module does not own Bionic's
`FILE` state. `sscanf`/`vsscanf` remain rejected because their pointer-write
grammar is not part of the coherent output-format subset.

The differential gate pins 32 supported cases from AOSP Bionic's
`tests/stdio_test.cpp` on `android16-release`, including Bionic's null `%p`
spelling (`0x0`), integer limits, negative zero, width, and truncation.

This is a bounded subset, not full Bionic `printf`. The C++ core uses
`std::string` and `std::to_chars`; allocation failure is not yet a noexcept
guest boundary.
