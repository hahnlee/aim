# P0-2: bionic `clone()` on Darwin threads

## Question

Bionic's `pthread_create` calls
`clone(CLONE_VM|FS|FILES|SIGHAND|THREAD|SYSVSEM|SETTLS|PARENT_SETTID|CHILD_CLEARTID, stack, &ptid, tls, &ctid)`.
The child starts on the given stack, at the instruction after the `svc`,
with x0=0 and every other register equal to the parent's. Can a Darwin thread
start that way, with the guest TLS set? And can we implement CHILD_CLEARTID,
which writes 0 to ctid and futex-wakes it when the thread exits?

## Method

The guest side is `guest_clone.S`, which copies bionic's `__bionic_clone.S`
byte for byte except that `svc #0` becomes the `LX_SVC` thunk from
`../common/lx.h`. A C mock of `pthread_internal_t`, where `tid` doubles as
ptid and ctid, and bionic-style create/join drive it.

The host side is the prototype syscall layer: `../common/lx_trampoline.S`
(full register save, host stack switch) and `clone.c` (`lx_dispatch`).

- **clone**:
  1. Allocate a Linux tid.
  2. Write `*ptid` (PARENT_SETTID) before the child can run.
  3. Copy the caller's saved frame, with x0=0 and sp=child_stack.
  4. `pthread_create` a detached Darwin thread with a 128 KiB host stack.
  5. The new thread binds itself: the TSD slot holds its `lx_thread`, and
     the guest-TP slot holds `tls` (CLONE_SETTLS; see P0-1 for why this is
     not TPIDR_EL0).
  6. The thread calls `lx_resume()` on the copied frame, and the child
     continues right after the `svc` on the guest stack.
- **exit**:
  1. Store 0 to ctid (CHILD_CLEARTID) and `os_sync_wake_by_address_any` it.
  2. Call `pthread_exit` from the host stack. The guest stack is never
     touched after the store.
- **futex WAIT/WAKE**: implemented with `os_sync_*` (P0-3 covers the full
  mapping).

`bsdthread_create` was rejected: it bypasses libpthread, so the host code the
thread runs in the syscall layer would have no TSD, no `errno` and no
pthread_self.

Run with `./run.sh [repeat-count]`.

## Results (M2 Pro, macOS 27.0)

- Register hand-off: the child sees x0=0, x19/x20/x28, d8 and v9 equal to
  the parent's values, and sp == child_stack. OK.
- Sequential bionic-style create/join ×2000: 0 mismatches in TLS (guest TP
  == `&tls[0]`), `gettid` and `ptid`.

| | p50 | p99 |
| --- | --- | --- |
| clone → child running guest code | 11.6–12.3 µs | 17–34 µs |
| clone → join returns (CLEARTID wake) | 18.5–22.8 µs | 25–52 µs |
| native `pthread_create`+`join` | 12.5–14.1 µs | 20–40 µs |

- 512 concurrent guest threads were created, then joined, in 4.3–7.1 ms,
  with 0 mismatches.
- Repeat runs: 0 of 29 extra runs failed.

A race found while building this is worth keeping in the real
implementation. The child can run, exit and free its host `lx_thread` before
`pthread_create` returns to the parent. Reading `c->tid` after
`pthread_create` intermittently returned 0 to the parent, which then took the
child path and crashed. The syscall layer must capture everything it returns
before starting the thread.

## Conclusion: **feasible**

Recommended mechanism:

- Back each guest thread with a `pthread_create`d Darwin thread that has a
  small host stack.
- Resume the copied register frame on the guest stack.
- Keep the guest thread pointer and the host thread record in Darwin TSD
  slots.
- Implement CLEARTID as store-0 plus a futex wake from the host stack at
  `exit`.
- Allocate Linux tids process-wide. darwin-artd owns the pid/tid space
  across processes (ADR 0012 §7).

Constraints for the loader and svc-thunk design:

- The thunk used here stores 32 bytes below the guest sp.
  `_exit_with_stack_teardown` in bionic munmaps the thread's own stack and
  then issues `exit`. The syscall layer must therefore defer an `munmap` that
  covers the caller's current sp until that thread exits, or use a thunk that
  does not touch the guest stack.
- The syscall return path resumes at a thunk tail, which restores
  x16/x17/x30. The guest-visible pc of a syscall is therefore
  `thunk tail + 8`. P0-4 relies on this when it builds signal frames.
