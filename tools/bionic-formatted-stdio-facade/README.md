# Closed Bionic formatted stdio facade

This standalone Darwin arm64 provider owns exactly the Android libc symbols
`fprintf` and `vfprintf`. It composes two already accepted modules instead of
redefining them:

- `bionic-format-facade` parses Android AAPCS64 `va_list` and formats into a
  bounded byte buffer without Darwin formatted-I/O calls.
- `bionic-stdio-facade` validates its own 152-byte Android `FILE` tokens and
  commits bytes through `aim_bionic_stdio_fwrite_core`.

The product archive contains only the formatted-stdio provider and the
`fprintf` AAPCS64 capture entry. Allocator, Bionic errno TLS, formatting, and
stdio definitions remain external providers. The audit rejects duplicate
definitions of those ownership roots.

`fprintf` captures x0-x7, v0-v7, and the original Android stack. After its two
fixed arguments, the synthesized Android `va_list` begins at GP offset -48,
FP offset -128, and the original caller stack. `vfprintf` consumes the caller's
32-byte Android `va_list` directly. The ELF fixture deliberately exhausts both
register banks: integer arguments 7/8 and floating arguments 9/10 occupy
Android 8-byte stack slots.

Formatting is intentionally bounded to a NUL-terminated format contained in
the first 4096 bytes (at most 4095 payload bytes) and at most 1,048,576 output
bytes. A complete temporary output is
produced before the stdio provider is mutated. Unsupported grammar propagates
the formatter's Bionic `ENOTSUP`; over-bound output returns `EFBIG`; foreign or
closed `FILE` tokens return `EBADF`. There is no partial stream write on these
pre-commit semantic/provider failures; the gate also fills a stream near its
capacity and verifies exact `EFBIG` plus an unchanged offset. Rust `Vec`
allocation exhaustion in the stdio provider can still abort the process, so
this module does not claim recoverable atomic behavior for host OOM. The
underlying bounded grammar still rejects positional
arguments, `%n`, locale grouping, wide strings, and `long double`.

The gate pins Android 16 Bionic's `stdio.cpp`, `vfprintf.cpp`,
`printf_common.h`, and `libc/Android.bp`, plus NDK r28c's API-35 arm64
`libc++_shared.so`. It proves the actual libc++ artifact imports both functions
as public `libc.so@LIBC`, builds an actual Android arm64 ELF call site, and runs
it through the closed ELF loader under normal, C AddressSanitizer, and C
UndefinedBehaviorSanitizer configurations.

This provider does not claim full Bionic `vfprintf`: it exposes the coherent
bounded conversion subset already documented by `bionic-format-facade`.
