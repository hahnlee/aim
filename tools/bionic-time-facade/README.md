# Bionic time facade gate

This standalone module implements the Class-B Android arm64 imports
`clock_gettime`, `nanosleep`, and `sysconf` without runtime integration or
dyld fallback. A sorted closed resolver returns only their prefixed C ABI
addresses; `__errno` comes from the standalone Bionic pthread-local provider.

## Clock semantics

Android and Darwin clock numbers are unrelated and are never passed through.
`CLOCK_REALTIME`, `CLOCK_MONOTONIC`, `CLOCK_PROCESS_CPUTIME_ID`, and
`CLOCK_THREAD_CPUTIME_ID` map explicitly to their Darwin counterparts.
`CLOCK_BOOTTIME` uses `mach_continuous_time` and the Mach timebase, preserving
Android's suspend-including semantic; Darwin `CLOCK_MONOTONIC` remains the
suspend-excluding backend for Android monotonic time.

Raw, coarse, alarm, SGI cycle, TAI, unknown, and encoded dynamic clock IDs are
unsupported and return Android `EINVAL`. No nearby Darwin clock is substituted.
The exact supported/unsupported matrix is in `manifests/clocks.tsv`.

Android arm64 and Darwin arm64 both use signed 64-bit seconds/nanoseconds, but
the facade still copies fields rather than exposing a host `timespec`.
BOOTTIME multiplication is performed in 128 bits and fails with Android
`EOVERFLOW` if seconds cannot fit the guest signed field.
The acceptance invokes that production conversion with maximal tick/numerator
inputs and verifies Android `EOVERFLOW`, unchanged host errno, and no capability
failure marker.

## Sleep, sysconf, and errno

`nanosleep` validates negative seconds and the full nanosecond range before the
host call. On `EINTR`, remaining time is copied only when the caller supplied a
buffer. An actual SIGALRM acceptance interrupts a one-second Android ELF sleep
and validates a normalized, positive remainder. Success leaves the remainder
unspecified as POSIX requires.

`sysconf` supports page size (both Android selector spellings), configured and
online processor counts read from the corresponding Darwin `sysconf` selectors,
and the physical/available page counts from the runtime's fixed 8-GiB memory
snapshot. Every other selector returns Android `EINVAL` instead of being sent
to Darwin under a colliding numeric value.

All facade calls save and restore Darwin pthread errno. Expected failures are
published only in Bionic TLS. An unknown Darwin errno or Mach/sysconf invariant
sets a sticky capability failure and deterministic Android `EIO`; the embedding
boundary must stop guest execution when that marker is observed.

## Deterministic acceptance

`./audit.sh` pins NDK r28c API35 time/sysconf headers, Android 16 OsConstants,
the actual libc++ Class-B import manifest, and the Bionic errno translator. It
cross-compiles a real Android AArch64 ELF whose only imports are `__errno` and
the three facade functions. The fixture checks monotonic ordering and duration,
realtime sanity, BOOTTIME ordering, process/thread CPU clocks, unsupported
clock/selector errors, page size and processor counts, invalid sleep, and real
signal-driven `EINTR` remaining time. ABI probes, dependency/export gates,
Clippy, and formatting run with a temporary target directory.
