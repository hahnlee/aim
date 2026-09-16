# Bionic allocator state facade

Current runtime note (2026-09-12): the provider closure consumes this allocator.
`allocator_options.c` owns allocator control requests separately from allocation
operations. Android `M_PURGE` and `M_PURGE_ALL` invoke Darwin zone pressure relief;
the value is ignored and zero reclaimed bytes is not failure. Unknown `mallopt`
requests return zero, replacing the former unconditional success.

The private `android_mallopt` boundary does not provide heapprofd instrumentation
for the host allocator. Profiling initialization returns false/Android ENOTSUP
(malformed arguments return EINVAL). The original ActivityThread JNI ignores this
optional result; no successful profiler initialization is claimed. Profiling and
other unsupported allocator controls remain explicit compatibility gaps.

This isolated module implements exactly the four allocator imports used by the
pinned NDK r28c/API 35 ARM64 `libc++_shared.so`: `malloc`, `free`, `realloc`,
and `posix_memalign`. It does not integrate with the runtime. Every exported
symbol is prefixed `darwin_art_bionic_*`, and the explicit sorted resolver has
no `dlsym`, host-symbol fallback, or interposition path.

## Ownership and semantics

Darwin's allocator is the sole owner of every block returned by this module.
Allocation, resizing, aligned allocation, and release all remain in that one
family. A block must never cross to a future Bionic allocator backend, and a
foreign block must never be passed into these functions.

The wrapper supplies Bionic's observable edge behavior:

- `malloc(0)` and `realloc(NULL, 0)` return a non-null freeable allocation.
- `realloc(pointer, 0)` frees and returns null.
- failed non-zero `realloc` returns null without releasing or changing the old
  allocation.
- `posix_memalign` accepts only power-of-two alignments at least
  `sizeof(void*)`, leaves the caller's output unchanged on failure, and returns
  Android `EINVAL` (22) or `ENOMEM` (12) directly.
- zero-size aligned allocation is non-null, aligned, and freeable.

Darwin errno TLS is saved and restored around every backend call and its
address is never exposed. Until a Bionic TLS errno provider exists, direct
`malloc` and `realloc` addresses can report failure only as null. Their
`*_result` seam additionally returns the Android errno number without touching
host errno. The capability table marks this limitation; callers must not claim
full Bionic errno compatibility from the direct addresses. `posix_memalign`
needs no TLS seam because its error number is its return value.

## Direct-address ABI audit

These four APIs contain only pointers, 64-bit `size_t`, and 32-bit `int` and
have no aggregates, varargs, `long double`, or layout-bearing objects. On
Android ARM64 and Darwin ARM64 their arguments and results occupy the same
integer registers, recorded in `abi-manifest.json`. The gate compiles the exact
prototypes against both SDKs and checks pointer/size/int widths. This audit is
specific to arm64 and does not authorize any additional allocator API such as
`malloc_size` or `malloc_usable_size`.

## Provenance and gate

The import set is read from and SHA-locked to the real NDK ELF. Behavioral
vectors follow pinned AOSP Bionic
[`malloc_test.cpp`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/tests/malloc_test.cpp),
[`malloc_common.cpp`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/libc/bionic/malloc_common.cpp),
[`malloc.h`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/libc/include/malloc.h), and
[`stdlib.h`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/libc/include/stdlib.h).
Decoded hashes are recorded in `sources.lock`.

Run `tools/bionic-libc-allocator-facade/audit.sh`. It checks pinned ELF import
provenance, exact prefixed exports and Darwin backend dependencies, ABI
signatures under both compilers, resolver allowlisting, alignment and size-zero
behavior, Android error values, host errno isolation, failed-realloc ownership,
and ASan/UBSan.
