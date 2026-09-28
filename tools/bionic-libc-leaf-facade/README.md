# First Bionic libc leaf facade

This isolated module classifies every `libc.so` dynamic import in the pinned NDK
r28c ARM64 `libc++_shared.so`. The ELF contains **159 function imports** plus the
Bionic `FILE` object `__sF`, so the complete manifest has 160 rows.

- **A**: ABI- and semantics-safe state-free leaves.
- **B**: a prefixed wrapper must translate errno, constants, layouts, paths,
  varargs, or kernel-visible structures.
- **C**: Bionic must own the state, including allocator, pthread/TLS, locale,
  multibyte conversion, environment, and `FILE` globals.
- **D**: loader, Android service, process-lifecycle, auxv, or Linux-kernel
  coupling.

Only the coherent A subset is implemented: five byte-memory functions, three
byte-string functions, and the three state-free 32-bit `wchar_t` memory/string
functions used by this libc++. Every export is `aim_bionic_*`. The
sorted resolver table returns only these explicit prefixed addresses; it never
uses `dlsym`, exports a host spelling, or falls back to a similarly named Darwin
symbol.

Android ARM64 defines `wchar_t` as unsigned 32-bit while Darwin uses signed
32-bit. The calling-width and object representation are usable for these three
leaf operations, but `wmemcmp` explicitly compares `uint32_t` values to retain
Bionic ordering; it is not forwarded to Darwin.

`long double`, errno storage and values, `FILE`/`DIR`/`stat` layouts, locale
handles and behavior, allocator ownership, pthread objects/TLS, Linux flags and
syscall numbers remain unsupported. In particular, the locale-taking wide
ctype functions are category C rather than being declared safe based on a few
ASCII examples.

## Provenance and gate

The import set is derived directly from the SHA-locked NDK ELF. API behavior and
differential vectors are grounded in pinned AOSP Bionic
[`string.h`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/libc/include/string.h),
[`wchar.h`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/libc/include/wchar.h),
[`string_test.cpp`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/tests/string_test.cpp), and
[`wchar_test.cpp`](https://android.googlesource.com/platform/bionic/+/361ba86734fb2821a6adcfdf775db8abd04e0de0/tests/wchar_test.cpp).
Their decoded source hashes are recorded in `sources.lock`.

Run `tools/bionic-libc-leaf-facade/audit.sh`. It checks ELF/manifest equality,
classification counts, absence of host undefined references and unprefixed
definitions, the explicit resolver allowlist, overlap behavior, unsigned-byte
ordering, high-byte strings, 32-bit wide values, and 4,096 deterministic
differential cases against Darwin.
