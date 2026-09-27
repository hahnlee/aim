# P0-6: Cost of a redirected syscall

## Question

What does a guest `svc #0` redirected into an in-process syscall layer cost?
We compare it with a native Darwin syscall (`getpid` / `getppid` via raw
`svc #0x80`). We also want to know what a leaner save (integer registers
only) would cost.

## Method

`cost.S` has one loop per path. Every loop issues Linux `gettid` (x8=178),
which the dispatchers answer without entering the kernel, so the numbers are
pure redirection overhead. Each figure is the best of 5 runs of 20 M calls.

| path | what it saves |
| --- | --- |
| **thunk only** | The per-site thunk: `stp x16,x17 / str x30 / bl entry / ldr / ldp`. The entry returns at once. |
| **full** | `../common/lx_trampoline.S`, the path experiments 2 and 4 use. x0–x30, sp, pc, NZCV, q0–q31, fpsr and fpcr go into the thread's frame (found through TSD). It switches to the host stack, calls a C dispatcher, and restores everything. |
| **lean** | x0–x30, sp, pc and NZCV. No SIMD. The dispatcher is built with `-mgeneral-regs-only`, so host code never touches v0–v31. |
| **minimal** | Only what AAPCS64 lets the callee clobber: x0–x15, x18 and NZCV. x16, x17 and x30 are already stacked by the thunk; x19–x29 are preserved by the C dispatcher. Same `-mgeneral-regs-only` dispatcher. x18 is saved because Darwin reserves it and shadow-call-stack guests keep live state in it. |
| **SIGSYS trap** | A raw `svc #0` with an invalid Darwin number in x16. Darwin raises SIGSYS and the handler emulates. |

## Results (M2 Pro, macOS 27.0, ns per call)

| path | ns |
| --- | --- |
| `bl`/`ret` to an empty function | 0.91 |
| Darwin `getpid`, raw `svc #0x80` | 88.8 |
| Darwin `getppid`, raw `svc #0x80` | 88.4 |
| svc thunk only | 1.60 |
| **full** (all integer + all SIMD + host stack switch) | **14.8** |
| **lean** (all integer, no SIMD) | **4.1** |
| **minimal** (AAPCS caller-saved integer) | **3.1** |
| SIGSYS trap | 1824 |

- A redirected syscall that passes through to a Darwin syscall costs about
  14.8 + 88 ≈ 103 ns, **1.2× a native getppid**.
- An emulated syscall that stays in the layer (gettid, most futex fast paths,
  clock reads if not vDSO'd, TLS or signal-mask bookkeeping) costs 3–15 ns,
  **6–30× cheaper than any native syscall**.
- The SIMD save and restore is about 10.7 ns of the full path's 14.8 ns.

## Conclusion: **feasible; cost is not a concern**

- Use the lean path by default: all integer registers, and a dispatcher
  compiled `-mgeneral-regs-only`. This is 4 ns.
- Take the full path only for syscalls that need the complete guest context:
  - `clone` (copies the frame);
  - `rt_sigreturn` and signal delivery at syscall exit (build or restore
    the Linux sigcontext);
  - execve-like operations;
  - any handler that calls into general host code (libSystem may use SIMD
    registers).

  The dispatcher can switch per syscall number before saving SIMD.
- The SIGSYS route is roughly 120× slower than the full redirect. It is also
  unsafe: the guest's x16 at an `svc` is arbitrary and can name a real Darwin
  syscall. It is not a fallback.
- The TLS rewrite from P0-1 adds about 0.35 ns per `mrs tpidr_el0`.
