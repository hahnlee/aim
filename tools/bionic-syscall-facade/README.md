# Bionic syscall facade

This standalone module owns the one `syscall@LIBC` import in the SHA-locked
NDK r28c/API-35 arm64 `libc++_shared.so`, and the Android system-library
`getrandom` calls reached through the same libc symbol. The pinned libc++
surface remains limited to all nine call sites actually present in that ELF:
two `gettid` calls from
`__cxa_guard_acquire`, six private futex calls from libc++ atomic wait/notify,
and libunwind's one `rt_sigprocmask` readability probe. `callsites.tsv` fixes
every instruction address and argument form.

Android and Darwin variadic ABIs are not assumed interchangeable. An AArch64
assembly entry captures x0-x7 and the caller stack before a non-variadic core
dispatches Android syscall numbers. It never invokes Darwin `syscall`, dlsym,
dyld, or a host signal-mask API.

The supported behavior is:

- syscall 178 (`gettid`) returns a positive, stable, provider-owned ID unique
  to each live host thread;
- syscall 278 (`getrandom`) fills a writable guest buffer from Darwin's
  cryptographically secure `arc4random_buf`. Linux `GRND_NONBLOCK`,
  `GRND_RANDOM`, and `GRND_INSECURE` flags are validated; the mutually
  exclusive RANDOM/INSECURE combination and unknown flags fail with Android
  `EINVAL`, while inaccessible output memory fails with Android `EFAULT`;
- syscalls 122/123 (`sched_setaffinity`/`sched_getaffinity`) expose an
  host-online CPU mask, validating provider-owned target thread IDs. Unknown
  targets return ESRCH. An unrestricted set is a no-op; a nonempty restriction
  returns ENOSYS because Darwin cannot enforce Linux CPU-index pinning. Empty
  online intersections return EINVAL. No fictitious binding is stored. The raw
  get syscall writes only its returned, word-rounded mask size;
- syscall 98 accepts `FUTEX_WAIT`/`FUTEX_WAIT_PRIVATE` with a readable aligned
  word and optional relative timeout, and `FUTEX_WAKE`/`FUTEX_WAKE_PRIVATE`
  with any non-negative wake count;
- `FUTEX_WAIT_BITSET[_PRIVATE]` and `FUTEX_WAKE_BITSET[_PRIVATE]` support the
  Android/Chromium match-any bitset, including absolute realtime deadlines;
- futex wait performs the compare and waiter admission under the same provider
  lock used by wake, then uses a fixed 257-address condition side table. It
  returns Android `EAGAIN` on compare mismatch or table exhaustion and
  `ETIMEDOUT` on timeout. A monotonic deadline is recomputed after every
  spurious wake, so wall-clock changes and spurious notifications cannot
  extend the caller's relative duration;
- syscall 135 accepts only libunwind's
  `rt_sigprocmask(-1, address, nullptr, 8)` probe. `mach_vm_region` checks that
  the eight-byte range is wholly readable without dereferencing it. The call
  always returns -1 with Android `EINVAL` for readable memory or `EFAULT` for
  an unreadable, unmapped, or overflowing range, matching the Linux probe's
  observed contract without changing a signal mask.
- syscall 240 (`rt_tgsigqueueinfo`) resolves the Android virtual TID through
  the provider's live thread registry, translates the Android signal number,
  and delivers it to the corresponding Darwin pthread. Darwin cannot consume
  Linux `siginfo_t`, so the payload is intentionally not forwarded; the
  installed host handler constructs its own signal context.

All other syscall numbers fail with Bionic `ENOSYS`; unowned futex operations
and argument forms fail with Bionic `EINVAL`/`EFAULT`. Host errno is preserved.
This is not a general Linux syscall translator, shared-process futex service,
robust-list implementation, PI futex implementation, or complete signal API. The exact
unsupported boundary is recorded in `manifests/unsupported.tsv`; no path fakes
success merely to satisfy eager relocation.

`audit.sh` pins the NDK ELF, Linux UAPI constants, LLVM sources, all nine
disassembly sites, and the exact libc++ import. Its real Android AArch64 ELF
gate exercises stable/unique thread IDs, host-CSPRNG `getrandom` including
flags and inaccessible memory, compare mismatch, wake-one, wake-all,
contention, relative timeout, readable/guard/unmapped/overflow libunwind
probes, unreadable wake addresses, spurious wake deadline behavior, table
exhaustion, wake-token cleanup, targeted signal translation, unknown
operations, Bionic errno, and host errno under ASan/UBSan.
