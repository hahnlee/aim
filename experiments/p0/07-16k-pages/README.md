# P0-7: 16 KiB pages, MAP_FIXED placement and large reservations

## Question

- What are the mmap, mprotect and munmap granularities, and how does
  MAP_FIXED behave for placing ELF segments?
- Can we reserve a large guest range (for example 64 GiB) and MAP_FIXED
  inside it later?
- Can the guest get the low 4 GiB, which ART uses on arm64?

## Method

- `pages.c` probes granularity, file offsets, MAP_FIXED over existing
  mappings, a MAP_FIXED_NOREPLACE equivalent, text-segment strategies
  (whether they *execute*, checked in a forked child), and reservations from
  64 GiB to 100 TiB with 1 MiB MAP_FIXED islands touched inside.
- It runs twice: with the linker's ad-hoc signature, and hardened with the
  repo's host entitlements.
- `lowmem.c` / `lowmem.sh` try to obtain memory below 4 GiB with several
  `-pagezero_size` values.

Run with `./run.sh`.

## Results (M2 Pro, macOS 27.0) — PASS under both signings

Granularity:

- `sysconf`, `getpagesize`, `vm_page_size` and `hw.pagesize` are all 16384.
- `mmap(len=4096)` returns a 16 KiB-aligned 16 KiB region, and byte 8192 is
  accessible. A 4 KiB Linux kernel would fault there.
- These return EINVAL:
  - `mprotect(addr+4096, 4096)`;
  - `munmap(addr+4096, 4096)`;
  - `mmap(MAP_FIXED)` at a 4 KiB-but-not-16 KiB address;
  - file `mmap` at offset 4096.
- `mprotect(aligned, 4096)` succeeds and affects the whole 16 KiB page.
- This matches a Linux kernel with 16 KiB pages. The guest sees
  `AT_PAGESZ=16384`, and Android 15+ bionic, linker and ART support 16 KiB
  pages. An ELF with `p_align` 4 KiB cannot be mapped in place. That is the
  guest `linker64`'s problem (its 16 KiB compat mode reads such segments),
  not the syscall layer's.

MAP_FIXED:

- A PROT_NONE image reservation, then MAP_FIXED file-backed RW data and an
  anonymous bss inside it, works.
- MAP_FIXED over an existing mapping replaces it, as on Linux.
- MAP_FIXED_NOREPLACE does not exist, but `mach_vm_allocate(VM_FLAGS_FIXED)`
  returns KERN_NO_SPACE on an occupied range, which is the same semantics.
- A hint without MAP_FIXED is moved elsewhere.

Executable text from an ELF file:

| strategy | result |
| --- | --- |
| `mmap(file, PROT_READ\|PROT_EXEC)` | **EPERM** in both signings. File-backed executable mappings need a code signature, and ELF has none. |
| `mmap(file, PROT_READ, MAP_PRIVATE)`, then `mprotect(+PROT_EXEC)` | works, and the code runs |
| MAP_PRIVATE RW, copy-on-write, `mprotect(RX)` | works, and the code runs |
| anonymous + `pread` + `mprotect(RX)` | works, and the code runs |

Large reservations (PROT_NONE, MAP_NORESERVE, then 3 × 1 MiB MAP_FIXED
islands touched):

| size | reserve time | footprint after |
| --- | --- | --- |
| 64 GiB | 0.1–0.17 ms | +3.1 MiB |
| 1 TiB | 1.7–2.6 ms | +3.1 MiB |
| 16 TiB | 25–27 ms | +3.1 MiB |
| 64 TiB | 60–68 ms | +3.2 MiB |
| 100 TiB | 86–92 ms | +3.2 MiB |

All sizes succeeded. The footprint is only the touched pages.

**Low 4 GiB: unavailable.**

- With the default link, the lowest region is the image at about
  0x1_0000_0000. MAP_FIXED at 64 KiB, 16 MiB, 1 GiB or 3 GiB fails with
  ENOMEM, and a 1 GiB hint lands above 4 GiB.
- `mach_vm_deallocate` of `__PAGEZERO` "succeeds", but a later
  `mach_vm_allocate` / `mach_vm_protect` / `mmap` below 4 GiB still returns
  invalid address or ENOMEM. The map's minimum address is 4 GiB.
- Linking with `-pagezero_size` 0x4000, 0x100000, 0x10000000 or 0x80000000
  gets the process **SIGKILLed at exec**. 0xffffc000 runs, but low
  allocation is still refused.
- Apple states that a custom pagezero is unsupported on arm64
  ([developer forums 655950](https://developer.apple.com/forums/thread/655950),
  [781966](https://developer.apple.com/forums/thread/781966)).
- The current runtime already hit this. `README.md` and
  `patches/art/0020`–`0022` moved ART to base-relative compressed references
  at 1 TiB.

## Conclusion

**Pages, MAP_FIXED and large reservations: feasible.**

- Report `AT_PAGESZ=16384`.
- Implement the guest's MAP_FIXED directly. Implement MAP_FIXED_NOREPLACE
  with `mach_vm_allocate(VM_FLAGS_FIXED)`.
- Turn a guest `mmap(file, …PROT_EXEC)` into `mmap(file, PROT_READ)` plus
  `mprotect`. This keeps it file-backed. Whether the text pages stay shared
  across processes is to be measured in P1.
- A multi-terabyte guest reservation is cheap.

**Low 4 GiB: infeasible — a blocker for "unmodified libart".**

- AOSP's arm64 ART (`USE_ART_LOW_4G_ALLOCATOR`, 32-bit compressed heap
  references) needs the Java heap, and the boot image at its fixed base,
  below 4 GiB.
- A normal macOS arm64 process cannot map there, and there is no supported
  entitlement or link option that changes that.
- The original libart and boot image of the pinned image therefore cannot
  run unmodified in a Darwin process. The ADR needs a decision before P2.
  Options:
  1. Keep a small, versioned ART patch set for base-relative references (the
     current approach), and rebuild libart and the boot image as guest ELF.
     This departs from "unmodified".
  2. Run ART's process under a VM or Hypervisor.framework address space,
     which the ADR rules out.
  3. Keep investigating XNU for an arm64 low-memory mode. None was found.
