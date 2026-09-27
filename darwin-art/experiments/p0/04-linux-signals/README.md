# P0-4: Linux signal delivery with Linux arm64 contexts

## Question

A guest handler registered through `rt_sigaction` expects a Linux arm64
`ucontext_t`:

- `uc_mcontext` at +176, holding `regs[31]`, sp, pc, pstate and
  `fault_address`.
- `__reserved`, holding an `fpsimd_context` (magic `0x46508001`, size 528)
  and an `esr_context`.

It returns through `rt_sigreturn`. ART uses SIGSEGV for implicit null checks
and stack overflow, so `si_addr` and `si_code` must be exact, and the handler
must be able to change pc and registers and resume. Can we build that frame
from a Darwin signal, run the handler, and restore from the Linux frame?

## Method (`signals.c`, `guest_sig.S`, the shared svc thunk in `../common`)

Host side:

- Each host thread has a host sigaltstack.
- The host handles SIGSEGV, SIGBUS, SIGILL, SIGTRAP and SIGFPE (synchronous
  faults), plus **SIGUSR2 as the carrier** for every asynchronous guest
  signal.
- Asynchronous guest signals (tgkill here; routed by darwin-artd later) go
  into a per-thread pending set with a full Linux siginfo, and the target is
  poked with SIGUSR2. This gives Linux RT signals 32–64, which Darwin lacks,
  and keeps the guest mask entirely in the layer, so the host mask never
  changes.

Delivery:

1. Build a Linux `rt_sigframe` as `arch/arm64/kernel/signal.c` lays it out:
   `siginfo`, then `ucontext` (sigcontext, fpsimd_context, esr_context,
   terminator), then a frame record above it. Put it on the guest stack, or
   on the guest sigaltstack for SA_ONSTACK.
2. Point the *interrupted Darwin mcontext* at the handler: pc=handler,
   x0=sig, x1=&info, x2=&uc, x29=&record, x30=sa_restorer.
3. Return. Darwin's own sigreturn performs the switch.

`rt_sigreturn` arrives from bionic's `__restore_rt` through the svc thunk:

1. Read the Linux frame.
2. Execute a `udf` "resume trap".
3. The SIGILL handler loads the complete Linux context (x0–x30, sp, pc, NZCV,
   q0–q31, fpsr, fpcr) into the Darwin mcontext.

This is the only register-exact way back to an arbitrary pc; an arm64
indirect branch needs a register.

Races with the syscall path are handled by a per-thread state and pc ranges:

- A signal that arrives in the svc entry, in the dispatcher, or while a
  resume trap is pending, stays pending. A blocking syscall returns EINTR,
  and the exit path delivers the signal with the syscall result in x0.
- A signal that arrives in the exit trampoline is delivered from the saved
  frame, which is final at that point.

Fault translation. Darwin raises **SIGBUS**, not SIGSEGV, for three different
faults, and all three carry the same ESR (data abort, translation fault L3):

- access to a PROT_NONE page;
- a write to a read-only page;
- a read past EOF of a mapped file.

The layer therefore classifies by the VM map:

- no mapping → SIGSEGV / SEGV_MAPERR;
- protection forbids the access (WnR from ESR) → SIGSEGV / SEGV_ACCERR;
- otherwise → SIGBUS / BUS_ADRERR.

`si_addr` comes from FAR. For the null page, Darwin itself reports
`si_code=2` (ACCERR) because `__PAGEZERO` is a mapped no-access region; the
translation reports Linux's MAPERR.

Run with `./run.sh`.

## Results (M2 Pro, macOS 27.0) — PASS, 0 failures

- T1, ART-style null check `ldr w1, [x0, #8]` with x0=0:
  - The guest sees SIGSEGV / SEGV_MAPERR, `si_addr` = `fault_address` = 0x8,
    pc = the faulting `ldr`, the fpsimd record with v8 intact, and
    ESR `0x92000006`.
  - The handler set pc to a landing pad, x1=0xdeadbeef, x19=0x1234 and
    v8=0xf00d (through the fpsimd record). All took effect after
    rt_sigreturn.
- T2, store to a PROT_NONE guard page with SA_ONSTACK:
  - Darwin raised SIGBUS; the guest saw SIGSEGV / SEGV_ACCERR with the exact
    address.
  - The handler ran on the guest sigaltstack and skipped the instruction
    (pc += 4) with x0 set.
- T3, write to a read-only page: SIGSEGV / SEGV_ACCERR with the exact
  address (Darwin: SIGBUS).
- T4, read past EOF of a file mapping: SIGBUS / BUS_ADRERR with the exact
  address.
- T5 and T6, async SIGUSR1 and **RT signal 40** via tgkill into a thread
  spinning in guest asm:
  - `si_code` = SI_TKILL and `si_pid` are correct.
  - The interrupted pc is inside the loop.
  - The handler's write to x10 ended the loop.
  - x19, x20, x28, v8, v9 and v15 were intact.
- T7, SIGUSR1 while the target blocks in FUTEX_WAIT:
  - The futex returns -EINTR.
  - The handler ran at syscall exit, and `sigcontext.regs[0]` = -EINTR.
- T8, storm: 3,000,000 guest syscalls while about 90,000 tgkills hit the
  thread at random points (guest code, thunk, entry, dispatcher, exit).
  About 93,000 handler runs; standard signals coalesce as on Linux.
  **0 iterations with corrupted registers.**
- Cost of a guest SIGSEGV round trip (fault → frame → handler →
  rt_sigreturn → resume): **5.3–5.9 µs**. A native Darwin handler that fixes
  pc is 2.3 µs. The difference is the second host signal, the resume trap.

## Conclusion: **feasible**

Recommended mechanism:

- **Synchronous faults**: a host handler on a host sigaltstack classifies the
  fault through the layer's own mapping table and builds a Linux frame in
  place. It then redirects the Darwin mcontext to the guest handler.
- **Asynchronous signals**: a per-thread pending queue plus one carrier host
  signal. Deliver only at safe points: in guest code, or at syscall exit.
  This covers RT signals and exact siginfo.
- **rt_sigreturn**: a resume trap (SIGILL on a known `udf`) for register
  exactness. It costs about 3 µs extra per signal, which is irrelevant for
  ART's rare NPE, stack-overflow and SIGQUIT paths.

Not prototyped here:

- ART's SignalCatcher waits for SIGQUIT with `sigwait`/`rt_sigtimedwait`,
  which Darwin does not have. It is a natural fit for the pending queue: the
  syscall blocks on the thread's queue.
- Default actions (core, stop), `SA_RESETHAND`, and process-directed signals
  routed by darwin-artd.
- Unwind info for the svc thunks, so debuggerd-style unwinders can walk
  through them.
