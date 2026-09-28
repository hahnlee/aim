# P0-5: ART's dual-mapped JIT code cache on macOS

## Question

ART's `JitMemoryRegion` creates the code cache with `memfd_create` and maps
it twice: a writable view to emit into, and an executable view to run. Which
macOS mechanism provides two simultaneous views, one RW and one RX? Which
code-signing settings does it need? Is icache maintenance
(`sys_icache_invalidate`) enough?

## Method

- `jit.c` runs each candidate mechanism in a forked child. A code-signing
  kill must not end the run. Each child:
  1. creates the views;
  2. writes `mov w0,#N; ret` through the writable view;
  3. invalidates the icache and calls through the executable view;
  4. rewrites the code 1000 times.
- `run.sh` re-signs copies of the same binary ad-hoc in six ways, only our
  own test binaries, and runs the matrix under each.
- `el0_insns.c` checks which EL0 system instructions a Linux guest executes
  itself (its own cache maintenance) are allowed by XNU.

## Results (M2 Pro, macOS 27.0)

| mechanism | linker ad-hoc (no hardened runtime) | hardened | hardened + allow-jit | hardened + allow-unsigned-executable-memory | hardened + jit + unsigned + disable-exec-page-protection | hardened + repo `config/aim-host.entitlements` |
| --- | --- | --- | --- | --- | --- | --- |
| mprotect RW→RX, one view | OK | SIGKILL | SIGKILL | OK | OK | OK |
| anon RWX without MAP_JIT | EACCES | EACCES | EACCES | EACCES | EACCES | EACCES |
| MAP_JIT + `pthread_jit_write_protect_np` (single view) | OK | EINVAL | OK | OK | OK | OK |
| **`mach_vm_remap` RW region → RX alias** | OK | SIGKILL | SIGKILL | **OK** | OK | **OK** |
| MAP_JIT (RX) + remapped RW alias | KERN_PROTECTION_FAILURE | EINVAL | KERN_PROTECTION_FAILURE | KERN_PROTECTION_FAILURE | same | same |
| memory entry created R\|W, mapped RW + RX | KERN_INVALID_RIGHT | same | same | same | same | same |
| **memory entry created R\|W\|X (`mach_make_memory_entry_64`), mapped RW + RX** | OK | SIGKILL | SIGKILL | **OK** | OK | **OK** |
| `shm_open` fd, `mmap` RW + RX | SIGBUS on exec | SIGBUS | SIGBUS | SIGBUS | SIGBUS | SIGBUS |
| unlinked file fd, `mmap` RW + RX | EPERM | EPERM | EPERM | EPERM | EPERM | EPERM |

- The entitlement that matters is **`com.apple.security.cs.allow-unsigned-executable-memory`**
  under the hardened runtime.
- `allow-jit` alone only enables MAP_JIT, which is a single view: Apple
  forbids aliasing a MAP_JIT region.
- `disable-executable-page-protection` is not needed.
- The repo's host entitlements already have allow-unsigned-executable-memory.
- Executable views of *file-backed* shared memory (POSIX shm, a file) are
  refused or killed in every configuration. So a memfd must not be emulated
  with shm or a temp file when it may be mapped PROT_EXEC.

Costs:

| operation | cost |
| --- | --- |
| `pthread_jit_write_protect_np(0)` + `(1)` | 38–50 ns |
| `mprotect` RX + RW of one 16 KiB page | ~690 ns |
| `sys_icache_invalidate` 64 B | 130–215 ns |
| `sys_icache_invalidate` 4 KiB | 0.94–1.4 µs |
| `sys_icache_invalidate` 64 KiB | 18–21 µs |

- Coherence: 1000 rewrites per mechanism with `sys_icache_invalidate` gave
  0 stale executions.
- 100,000 rewrites *without* it also gave 0 stale in this pattern. That does
  not prove it is unnecessary: CTR_EL0 cannot even be read (below), so ARM's
  rules apply and the invalidate stays.

EL0 instructions (`el0_insns.c`):

| instruction | result |
| --- | --- |
| `mrs ctr_el0` | **traps (SIGILL)** |
| `mrs midr_el1`, `mrs id_aa64isar0_el1` | trap. Linux emulates them. |
| `mrs dczid_el0`, `cntvct_el0`, `cntfrq_el0`, `tpidrro_el0` | allowed |
| `dc cvau`, `dc civac`, `dc cvac`, `ic ivau`, `dc zva`, `isb`, `dsb` | allowed |

So guest code can do its own cache maintenance. However, compiler-rt's
`__clear_cache` and ART's `FlushInstructionCache` read CTR_EL0 first to learn
the line sizes, and that read traps.

## Conclusion: **feasible with conditions**

Recommended mechanism:

- **memfd = a Mach named memory entry** created with max protection R|W|X
  (`mach_make_memory_entry_64(MAP_MEM_NAMED_CREATE | VM_PROT_ALL)`).
  - A guest `mmap` of the memfd becomes `mach_vm_map` of that entry with the
    requested protection, so RW and RX views coexist.
  - The entry is a Mach port: it can cross processes (fd passing, zygote to
    app) through aimd, which matches memfd semantics.
  - `mach_vm_remap` is the fallback for anonymous dual views.
- Required signing: hardened runtime plus
  `com.apple.security.cs.allow-unsigned-executable-memory`, as the repo host
  already has.
- `mrs ctr_el0` must be emulated. Trap-and-emulate on SIGILL is enough,
  because compiler-rt caches the value. Report conservative 64-byte lines
  with IDC=DIC=0. Alternatively, rewrite it at load time.
- Do not advertise `HWCAP_CPUID` in AT_HWCAP, so guests do not probe the
  ID registers. If they do anyway, emulate on SIGILL as Linux does.
