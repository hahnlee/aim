# P0-3: Linux futex on `os_sync_wait_on_address` / `__ulock`

## Question

Can we map these Linux futex operations onto `os_sync_wait_on_address` (the
public wrapper of `__ulock_wait2` / `__ulock_wake`)?

- FUTEX_WAIT and FUTEX_WAKE, with and without `_PRIVATE`.
- FUTEX_WAIT_BITSET with absolute CLOCK_MONOTONIC or CLOCK_REALTIME
  timeouts, and FUTEX_WAKE_BITSET.
- The REQUEUE variants that ART's condition variables use.
- Non-private futexes on MAP_SHARED memory across processes, which binder
  and ashmem users need.

We also want the wake latency.

## Method

`futex.c` implements `lx_futex()` with two backends and tests both:

- **direct**: every op goes straight to `os_sync_*`. SHARED flavour for
  shared mappings.
- **table** (private futexes): an in-process hash table of waiters, each
  blocking on its own per-thread word. This gives exact bitset, requeue and
  wake-count semantics. Timed waits park in a per-thread kqueue
  (EVFILT_USER wakes, EVFILT_TIMER with NOTE_LEEWAY 0). Buckets use
  `os_unfair_lock`: `pthread_mutex` made the one-shot wake latency 2× worse.

Guest clocks: guest CLOCK_MONOTONIC := `mach_absolute_time` in ns (Darwin
CLOCK_UPTIME_RAW). Like Linux, it stops during sleep. An absolute MONOTONIC
deadline therefore converts exactly to
`os_sync_wait_on_address_with_deadline(OS_CLOCK_MACH_ABSOLUTE_TIME)`. A
CLOCK_REALTIME deadline is converted to a relative timeout on entry.

`timer_slack.c` measures how late each Darwin timed-wait primitive fires.

Run with `./run.sh`.

## Results (M2 Pro, macOS 27.0)

All checks passed on both backends where the operation is expressible.

- Stale value returns EAGAIN, `{0,0}` returns ETIMEDOUT at once, and a
  deadline in the past returns ETIMEDOUT. `os_sync` itself does not report
  a value mismatch, so the layer pre-checks the value, and a later change
  surfaces as a spurious wakeup.
- WAKE n returns exactly n on the table backend. The direct backend loops
  `wake_any` for small n, and it cannot count waiters for `wake_all`.
- WAKE_BITSET with a real bitset (2 of 4 waiters) and CMP_REQUEUE (wake 1,
  move 3) are exact on the table backend. On `__ulock` neither is
  expressible: bitsets return ENOSYS, and requeue degrades to wake-all,
  which is a legal spurious wakeup but causes a thundering herd.
- `os_sync` keeps SHARED and non-shared waiters in **separate namespaces**.
  A SHARED wait is not woken by a non-shared wake on the same address, or
  the other way round; the wake returns ENOENT. On Linux, FUTEX_PRIVATE on
  private memory does match a shared-flavour op. The layer must therefore
  pick the flavour from the **mapping type** (MAP_SHARED or not), not from
  FUTEX_PRIVATE_FLAG.
- Cross-process: a POSIX shm object mapped at *different* addresses in two
  processes works with SHARED ping-pong. So does inherited
  MAP_SHARED|MAP_ANON. A private-keyed wait on shared memory is not woken
  from another process, which is also Linux behaviour.
- A signal during a wait makes `os_sync` return EINTR.

Latency:

| | |
| --- | --- |
| thread ping-pong round trip, raw `__ulock` | 2.7–2.8 µs |
| thread ping-pong round trip, `lx_futex` direct | 2.5–3.6 µs |
| thread ping-pong round trip, `lx_futex` table | 2.5–4.1 µs |
| cross-process SHARED ping-pong round trip | 1.8–11 µs (scheduling dependent) |
| one-shot wake → sleeping waiter running | p50 2–7 µs, p99 10–17 µs (direct); table within noise of direct with `os_unfair_lock` (p50 2–17 µs) |

Timed waits are where Darwin differs. A 20 ms timeout fires this late:

| primitive | 1 ms wait | 20 ms wait |
| --- | --- | --- |
| `os_sync_*_with_timeout` / `_deadline`, `__ulock_wait2` | +260 µs | **+2.0 ms** (~10% leeway, any QoS) |
| `nanosleep`, `mach_wait_until` | +510 µs | +7–10 ms |
| kqueue EVFILT_TIMER, NOTE_LEEWAY 0 + NOTE_CRITICAL | +17–40 µs | +30–40 µs |

With the kqueue path, the futex timeouts land 20–40 µs past the deadline,
against 2 ms on `os_sync`.

## Conclusion: **feasible with conditions**

Recommended mechanism:

- **Private futexes**: an in-process waiter table with `os_unfair_lock`
  buckets, one wait word per thread on `os_sync`, and timed waits in a
  per-thread kqueue with explicit zero or small leeway. This covers WAIT,
  WAKE, WAIT_BITSET and WAKE_BITSET with any bitset, REQUEUE and
  CMP_REQUEUE (ART's `ConditionVariable::Broadcast`), and exact wake
  counts, at `__ulock` speed.
- **Shared futexes** (the backing mapping is MAP_SHARED: shm, memfd, ashmem,
  binder buffers): `os_sync_*` with the SHARED flags.
  - WAIT, WAKE and MATCH_ANY bitsets work directly.
  - REQUEUE degrades to wake-all.
  - A real bitset on a shared futex needs a aimd-side table. None
    was found in bionic's users.
- Choose the flavour from the syscall layer's own mapping table.
- Guest CLOCK_MONOTONIC = mach absolute time. Every guest timed wait
  (futex, `clock_nanosleep`, `epoll_wait`, `ppoll`) should use kqueue timers
  with explicit leeway. Otherwise Darwin's default ~10% timer leeway makes a
  guest 16 ms frame timer fire 1.6 ms late.
- Still open: FUTEX_LOCK_PI / UNLOCK_PI, used by bionic PI mutexes, are not
  prototyped. They need the table, plus priority boosting through the
  owner's Darwin thread.
