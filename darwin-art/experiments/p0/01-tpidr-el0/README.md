# P0-1: Can the guest own TPIDR_EL0?

## Question

Bionic keeps its thread pointer in TPIDR_EL0 (`msr tpidr_el0, x0` in
`__set_tls`, and `mrs xN, tpidr_el0` for every `__get_tls()` and every ELF TLS
access). Darwin's libSystem uses TPIDRRO_EL0 for its own TSD. We need to know:

- Does a user-written TPIDR_EL0 survive syscalls, yields, preemption and
  signals, across many threads over time?
- Does Darwin itself read or write TPIDR_EL0?

## Method

- `tpidr.c`: 64 threads (12 CPUs, so they are preempted constantly) each write
  a unique value, then re-check it after `getpid`, `sched_yield`, `write`,
  `malloc`/`free`, `usleep`, `os_unfair_lock`, `pthread_mutex`,
  `clock_gettime`, `snprintf`, `pthread_cpu_number_np` and busy spinning. A
  signal thread sends SIGUSR1 to random workers every 20 us, and the handler
  checks the value too. A separate probe records what TPIDR_EL0 holds in a
  thread that never writes it.
- Whole dyld shared cache scan:
  `xcrun dyld_info -all_dyld_cache -disassemble`, counting `mrs`/`msr` of
  TPIDR_EL0 per image and function. (`otool -tv` only works on the few dylibs
  still on disk, and silently reports nothing for the rest.)
- `tls_rewrite.c`: checks the replacement mechanism (below) across context
  switches, and measures its cost.

Run with `./run.sh [seconds] [threads]`.

## Results (M2 Pro, macOS 27.0 26A428, 30 s, 64 threads)

The kernel owns TPIDR_EL0. A thread that never writes it sees values like
`0x200a` and `0x1005`: the low 12 bits are the CPU number
(`pthread_cpu_number_np` returns exactly `TPIDR_EL0 & 0xfff`), and bits 12 and
up carry the cluster. XNU rewrites it whenever the thread is switched back in.

| after | clobbered / checks |
| --- | --- |
| `usleep(50)` | 2,586,909 / 2,587,295 (100.0%) |
| `sched_yield` | 1,933,779 / 2,587,333 (74.7%) |
| `write(2)` | 150,087 / 2,587,295 (5.8%) |
| spinning (preemption) | 152,770 / 5,174,551 (3.0%) |
| `getpid`, `malloc`, locks, `clock_gettime` | ~0.005%, only when a switch happened in between |

- In total there were 4,825,979 mismatches in 31,047,615 checks, and all 64
  threads were affected. There were 5.2 M involuntary context switches.
- Signal handlers saw a kernel value in 398,831 of 448,709 deliveries.
- The observed values were always the kernel's (`0x0`–`0xb`, `0x1004`,
  `0x2008`, ...), never a partial mix.
- A syscall that does not switch threads leaves the register alone, so the
  kernel restores TPIDR_EL0 on context switch, not on every exception return.

Who reads TPIDR_EL0 in the shared cache (`mrs`):

| image | sites | purpose |
| --- | --- | --- |
| `libsystem_malloc` | 52 | xzone / nano magazine and CPU index |
| `libsystem_kernel` | 5 | `os_custom_x18_abi_enabled` reads bit 48 (per-thread x18 ABI flag) |
| `libdispatch` | 3 | continuation cache CPU index |
| `libsystem_pthread` | 2 | `pthread_cpu_number_np` |
| `libc++abi` | 2 | inlined x18 ABI check |

- No image in the whole shared cache executes `msr tpidr_el0`.
- 21 images read TPIDRRO_EL0 (TSD) inline.

A guest value in TPIDR_EL0 therefore also corrupts host state. Until the next
context switch, host `malloc` computes its CPU index from the guest pointer,
and the x18-ABI flag reads bit 48 of it.

The replacement (`tls_rewrite.c`) keeps the guest thread pointer in a Darwin
TSD slot: pthread key 258, which is `[TPIDRRO_EL0 + 2064]`.

- Correctness: 32 threads × 20,000 reads with `usleep` switches gave
  0 mismatches.
- Cost per read, with a dependent add:

| read | cost |
| --- | --- |
| `mrs x, tpidr_el0` | 0.307 ns |
| `mrs x, tpidrro_el0; ldr x, [x, #slot]` inline | 0.320 ns |
| `b thunk; mrs; ldr; b back` (what a load-time rewriter emits) | 0.654 ns |

## Conclusion: **infeasible as specified; feasible with rewriting**

The guest cannot own TPIDR_EL0 on Darwin, because XNU rewrites it with the CPU
number on every context switch and libSystem depends on that.

The real implementation must rewrite guest instructions at load time, next to
the `svc` redirection:

- `mrs xN, tpidr_el0` becomes `mrs xN, tpidrro_el0; ldr xN, [xN, #slot]`.
  The destination register serves as its own scratch, so no spill is needed.
  It goes into a per-site thunk because the sequence is two instructions.
- `msr tpidr_el0, xN` (only bionic's `__set_tls` and the clone child setup)
  becomes a thunk that stores xN into the slot, spilling one scratch register.
- `CLONE_SETTLS` writes the slot, not the register (see P0-2).

Runtime-generated code is not covered by load-time rewriting, and the loader
design must track this. ART's JIT uses x19 for `Thread*` and does not emit
`mrs tpidr_el0`, but V8/WebView or other JITs still need to be checked.
