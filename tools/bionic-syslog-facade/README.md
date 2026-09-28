# Bionic syslog facade

This standalone module owns the exact API-35 ARM64 `libc++_shared.so` imports
`openlog`, `closelog`, and `syslog`. The pinned libc++ call site is
`std::__ndk1::__libcpp_verbose_abort`: it opens tag `libc++`, logs priority
`LOG_CRIT` with format `%s`, closes the log, and aborts. The gate checks that
call sequence and literals directly in the locked NDK ELF.

Android and Darwin arm64 do not share a variadic ABI. The `syslog` entry is
AArch64 assembly that captures Android x0-x7, v0-v7, and the caller stack. Its
non-variadic core constructs Android's 32-byte `va_list` and delegates formatting
to the existing closed Bionic format facade. No Darwin `syslog`, `vsyslog`,
formatted-I/O function, `dlsym`, or dyld fallback is used.

The core follows Android 16 Bionic's state and routing:

- `openlog` retains the caller's tag pointer and options and ignores facility;
- `closelog` clears tag/options but not priority policy;
- facility bits in a message are ignored and priorities map to Android
  ERROR/WARN/INFO/DEBUG;
- output goes through AOSP liblog's `__android_log_write`;
- `LOG_PERROR` writes `tag: message\n` to stderr, without duplicating a newline;
- a null tag uses an immutable guest program tag copied at one-time provider
  activation; without activation it emits nothing and fails closed with Bionic
  `ENOTSUP` rather than exposing Darwin's host process name;
- Darwin errno is preserved across every prefixed boundary.

The source-pinned Bionic implementation uses full `vsnprintf`. This facade's
general formatting surface is intentionally the already-audited format subset:
integers, narrow strings/chars, pointers, and doubles with non-positional width
and precision. Unsupported conversions fail closed through Bionic errno and do
not emit a log. That subset fully covers the only syslog format in the pinned
libc++ owner (`%s`); this module does not claim a process-wide general-purpose
replacement for arbitrary Android applications.

`openlog` deliberately retains the caller's `ident` pointer, exactly like the
upstream global. Its storage lifetime during `syslog` is the POSIX caller's
contract; the provider mutex only serializes state changes and log emission.
`setlogmask` and `vsyslog` exist in the upstream source but are not imported by
the pinned libc++ and remain explicit extension boundaries in
`manifests/extension-boundary.tsv`.

`audit.sh` verifies AOSP/NDK provenance, raw constants, the 32-byte Android vs
8-byte Darwin `va_list` split, the libc++ call site, absence of host syslog
dependencies, and a real Android AArch64 ELF call containing integer, string,
and floating variadic arguments. The ELF gate captures AOSP liblog messages and
checks state reset, priority/facility mapping, `LOG_PERROR`, and host errno.
