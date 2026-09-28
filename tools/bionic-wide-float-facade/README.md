# Bionic wide floating facade

This gate owns the pinned Android libc++ `libc.so@LIBC` demand for `wcstod`
and `wcstof`. It deliberately does not own `wcstold`: Android arm64 returns an
IEEE binary128 `long double`, while Darwin arm64 uses binary64. Returning a
Darwin `long double` through the Android ABI would silently corrupt the result.

The implementation follows Android 16 Bionic's `libc/bionic/wcstod.cpp`
boundary without passing Android strings to Darwin wide-character APIs.
Android `wchar_t` is represented explicitly as unsigned 32-bit code units.
Leading whitespace is classified by the pinned static ICU 76.1
`UCHAR_WHITE_SPACE` property. The exact Bionic ASCII candidate set
`-+0123456789.xXeEpP()nNaAiIfFtTyY` is copied through the existing Bionic
allocator facade and parsed by the existing renamed AOSP gdtoa `strtod` and
`strtof` providers. The returned byte offset is mapped back to the original
wide input, including Bionic's rule that a failed conversion reports the
unadvanced original pointer.

The facade preserves Darwin pthread errno and floating-point environment.
Overflow and underflow are published only to the Bionic TLS errno cell by the
existing gdtoa provider. Allocation failure publishes Android `ENOMEM`.

`audit.sh` locks the Android 16 Bionic source and the NDK r28c API-35 libc++
demand, verifies the binary128 rejection, builds an actual AArch64 Android ELF
with exactly `wcstod`, `wcstof`, and `__errno@LIBC`, and executes it with the
closed Rust ELF resolver. Unicode whitespace, non-whitespace, NaN payload,
hexadecimal, overflow/underflow, exact end pointers, four rounding modes,
thread isolation, ASan, and UBSan are acceptance requirements.

The published archive contains only `provider.o`. It deliberately does not
embed allocator, gdtoa, errno, or ICU objects, so a runtime closure must use
its existing owners exactly once. The standalone Cargo and sanitizer gates
compile the pinned allocator source as a separate test-only dependency.

This directory is standalone. It does not modify the runtime namespace,
loader, abort, or lifecycle implementation. Integration order is:

1. `libaim-bionic-wide-float.a` as the consumer/root;
2. the existing allocator owner supplying prefixed `malloc_result` and `free`;
3. `libaim-bionic-float-conversion.a` for renamed AOSP gdtoa and Bionic
   errno, without re-adding a separate errno owner;
4. the already force-loaded `libandroidicuinit-darwin.a` if the enclosing
   closure has it, otherwise force-load it once;
5. the existing `libicuuc-common-darwin.a` and
   `libicuuc-stubdata-darwin.a` providers, once each;
6. Darwin libc++ and system frameworks.
