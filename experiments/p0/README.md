# ADR 0012 phase P0: platform experiments

These are small, self-contained programs that prove or disprove the Darwin
platform assumptions behind running the original Android arm64 userspace on
an in-process Linux syscall layer
([ADR 0012](../../docs/adr/0012-original-android-userspace.md)).

- Each directory has a `run.sh` and a README with the question, method,
  measured results and conclusion.
- Binaries go to `$P0_OUT` (default `$TMPDIR/aim-p0`), never into the
  tree.
- Signing variants are ad-hoc signatures of those test binaries only.
- Measured on an M2 Pro running macOS 27.0 (26A428).

`common/` is a stand-in syscall layer shared by experiments 2, 4 and 6. It
provides the svc thunk (`LX_SVC`), the full-save entry and exit trampoline,
and per-thread state in Darwin TSD slots. The real loader and layer replace
it.

| # | experiment | verdict | key numbers | mechanism to use |
| --- | --- | --- | --- | --- |
| 1 | [TPIDR_EL0](01-tpidr-el0/) | **infeasible as specified; feasible with rewriting** | XNU keeps the CPU number in TPIDR_EL0 and rewrites it on every context switch: 100% clobbered after `usleep`, 75% after `sched_yield`, 3% under preemption. libmalloc, libdispatch and the x18-ABI flag read it. The TSD-slot replacement costs 0.65 ns per read through a thunk. | Rewrite `mrs/msr tpidr_el0` at load time to a TSD slot at TPIDRRO_EL0 + key×8 |
| 2 | [clone](02-clone-threads/) | **feasible** | clone → child running: p50 12 µs (native `pthread_create` is 13 µs). 512 concurrent threads OK. CLEARTID + wake OK. | `pthread_create` with a small host stack, then resume the copied frame on the guest stack |
| 3 | [futex](03-futex/) | **feasible with conditions** | ping-pong 2.5 µs per round trip. `os_sync` timeouts are 2 ms late on 20 ms; kqueue timers are 30 µs late. SHARED and private `os_sync` namespaces do not match. | Private: waiter table with per-thread `os_sync` words and kqueue timeouts. Shared: `os_sync` SHARED, with the flavour chosen from the mapping type. |
| 4 | [signals](04-linux-signals/) | **feasible** | Exact si_addr and si_code (Darwin's SIGBUS translated to SEGV_ACCERR). A handler can modify pc, x and v registers. RT signals work. 3 M syscalls under a signal storm: 0 corruptions. 5.3 µs per fault round trip. | Linux rt_sigframe built from the Darwin mcontext; a carrier signal plus a pending queue for async signals; a resume trap for `rt_sigreturn` |
| 5 | [JIT dual mapping](05-jit-dual-mapping/) | **feasible with conditions** | A memory entry mapped RW + RX works under hardened runtime + allow-unsigned-executable-memory. shm and file RX views are refused. `mrs ctr_el0` traps. | memfd = Mach named memory entry (VM_PROT_ALL), mapped per view. Emulate CTR_EL0. |
| 6 | [syscall cost](06-syscall-cost/) | **feasible** | full save 14.8 ns, integer-only 4.1 ns, AAPCS-minimal 3.1 ns; Darwin getpid 88 ns; SIGSYS trap 1.8 µs | Lean integer path with a `-mgeneral-regs-only` dispatcher; full path per syscall |
| 7 | [16 KiB pages](07-16k-pages/) | pages: **feasible**; low 4 GiB: **infeasible** | 16 KiB granularity is Linux-16K compatible. A 100 TiB reservation takes 92 ms. File PROT_EXEC mmap gets EPERM; `mprotect` after the mapping works. Nothing can be mapped below 4 GiB. | `AT_PAGESZ=16384`; `mmap` + `mprotect` for text. **ART's low-4 GiB heap needs an ADR decision.** |

Findings that change the ADR's plan:

1. The guest cannot own TPIDR_EL0 (1). Load-time rewriting must cover
   `mrs`/`msr tpidr_el0` as well as `svc`.
2. Unmodified arm64 libart assumes a heap and boot image below 4 GiB, which
   macOS arm64 never maps (7).
3. The svc thunk must not assume the guest stack is mapped, because of
   bionic's `_exit_with_stack_teardown` (2).
4. `mrs ctr_el0` traps (5).
5. Darwin's default timer leeway makes a naive futex or sleep mapping 10%
   late (3).
