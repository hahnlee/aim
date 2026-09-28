# Android 35 libc++ provider coverage

This gate measures coherent standalone provider ownership against the exact
160 `libc.so` imports of the SHA-locked NDK r28c/API 35 arm64
`libc++_shared.so`. It is deliberately stricter than counting functions that
happen to exist in Darwin libc: a symbol is covered only when a project facade
claims its Android ABI and semantic state.

The current composed namespace owns all 160 libc-family imports plus 18 liblog
routes through twenty-nine providers:

| Provider | Imports | Boundary |
|---|---:|---|
| leaf | 11 | state-free byte and 32-bit wide-character operations |
| allocator | 4 | Darwin allocation owner with explicit Android failure seam |
| errno | 1 | pthread-local Bionic errno cell |
| filesystem | 29 | virtual FDs, byte paths, translated stat/dirent |
| time | 3 | Android clocks, nanosleep, and sysconf selectors |
| pthread | 24 | TLS, once, mutex/attributes, condition variables, rwlocks, join/detach |
| process-state | 3 | immutable environment, properties, and auxv snapshot |
| phdr | 1 | loader-owned Android ELF program-header snapshots |
| stdio | 13 | Android FILE tokens, `__sF`, and fixed-register binary I/O |
| wide-stdio | 3 | central FILE leases with Android wchar32 stream state |
| scanf | 2 | Android AAPCS64 varargs and 32-byte `va_list` scanning |
| swprintf | 1 | Android wchar32 output and AAPCS64 variadic floating input |
| ioctl | 1 | Linux request decoding over the virtual FD owner |
| strftime | 1 | Bionic C-locale formatting with fixed-offset timezone state |
| sendfile | 1 | virtual-FD transfer with Linux offset and errno semantics |
| locale | 31 | ICU 76 locale, multibyte, and wide-character semantics |
| numeric | 6 | narrow integer conversion |
| float-conversion | 2 | renamed AOSP gdtoa `strtod`/`strtof` |
| format | 3 | Android variadic formatting ABI |
| formatted-stdio | 2 | bounded formatter output committed to provider-local Android `FILE` tokens |
| strerror | 1 | Bionic errno message ownership |
| wide-integer | 4 | unsigned Android wchar32 integer conversion |
| wide-float | 2 | ICU whitespace plus AOSP gdtoa `wcstod`/`wcstof` |
| binary128-conversion | 3 | Android AAPCS64 q0 `strtold`/`strtold_l`/`wcstold` |
| abort | 2 | Bionic abort/message state |
| liblog | 18 | AOSP Android logging entrypoints without host fallback |
| syslog | 3 | Android variadic capture, Bionic state, and AOSP liblog routing |
| syscall | 1 | exact libc++ gettid/futex/libunwind-probe dispatch without raw host syscalls |
| lifecycle | 2 | Bionic DSO finalizer ownership |

Coverage by the original ABI classification is A 11/11, B 76/76, C 65/65,
and D 8/8. No pinned libc++ libc-family import remains unsupported.
Binary128 conversions return raw IEEE bits through Android's q0 ABI and never
reinterpret Darwin's binary64 `long double`.

These manifests feed the generated closed provider namespace used by ART's ELF
graph resolver. The runtime closure still requires exactly one allocator,
errno/gdtoa, and ICU owner; the wide-float archive contains none of those
dependencies itself. The formatted-stdio archive likewise owns only
`fprintf`/`vfprintf` and reuses the one format, stdio, allocator, and errno
owners.
The syscall archive owns only the exact Android `syscall@LIBC` entry and
reuses the namespace's Bionic errno semantics; unknown numbers and forms stay
fail-closed rather than forwarding to Darwin's syscall ABI.
The binary128 archive is ordered before allocator, existing gdtoa/errno, and
ICU providers and introduces no duplicate owner for those dependencies.

Run `tools/audit-android35-libcxx-provider-coverage.sh`. It hash-locks every
input manifest, rejects providers outside the actual libc++ import set,
rejects duplicate owners, and pins provider and capability-class counts.
