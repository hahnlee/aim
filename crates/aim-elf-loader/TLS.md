# Bounded AArch64 ELF TLS

The loader supports the AArch64 TLS-descriptor form emitted for a local TLS
definition by the pinned NDK r28c/API 35 `-fPIC` toolchain. This is a bounded
dynamic-TLS slice, not a Linux thread-pointer emulation layer.

The contract follows Arm's
[System V ABI for AArch64](https://github.com/ARM-software/abi-aa/blob/main/sysvabi64/sysvabi64.rst):
an `R_AARCH64_TLSDESC` target is two consecutive GOT words containing a
resolver and its argument. The resolver receives the descriptor in `x0`,
returns the variable's offset from `TPIDR_EL0` in `x0`, and preserves every
general-purpose and SIMD/FP register except `x0`, `x1`, `x30`, and flags.
The relocation number and `TLSDESC(S+A)` operation come from Arm's
[AAELF64 specification](https://github.com/ARM-software/abi-aa/blob/main/aaelf64/aaelf64.rst).

## Ownership model

- One well-formed `PT_TLS` is accepted per image. The file template must be
  mapped consistently inside one `PT_LOAD`; `.tbss` is zero-filled. Size is
  capped at 64 MiB and alignment at 1 MiB.
- Only `R_AARCH64_TLSDESC` for a definition owned by the same image is
  accepted. A symbol-zero relocation uses its RELA addend as the offset;
  defined `STT_TLS` symbols use `st_value + addend`.
- Each Darwin thread lazily owns one aligned allocation per ELF module. The
  initialized bytes are copied from the relocated template and the remainder
  is zero-filled. A host `thread_local` destructor frees it on pthread exit.
- The descriptor resolver returns
  `guest_variable_address - current_TPIDR_EL0`. Guest code can consequently
  execute its standard `TPIDR_EL0 + returned_offset` sequence without the
  loader replacing or exposing Darwin's thread-pointer layout.
- A descriptor's second word is an opaque monotonic token, never a Rust
  pointer. Assembly loads the token and passes integer values to Rust, which
  requires both a process-registry hit and the exact registered descriptor
  address. A forged pointer or token fails stop before Rust accesses module
  state.
- Image teardown first releases the unloading thread's block, then closes the
  module to new allocations. Unloading while another thread still owns a
  block aborts before `munmap`; continuing would leave executable code and
  descriptor arguments dangling.

## Deliberate exclusions

Imported `STT_TLS`, graph-wide TLS module indexes, `R_AARCH64_TLS_DTPMOD`,
`R_AARCH64_TLS_DTPREL`, `R_AARCH64_TLS_TPREL` (initial/local exec), and
language-level TLS destructor registration are not implemented. These forms
fail closed. Initial/local exec require a process-wide static TLS layout
relative to Darwin's thread pointer, which this loader intentionally does not
fabricate. C++ `thread_local` destructors remain a separate
`__cxa_thread_atexit_impl` lifecycle problem.

`run-gate.sh` builds a real AArch64 Android DSO containing initialized,
zero-filled, and 64-byte-aligned TLS. Four pthread-backed Rust workers prove
distinct initial values and retained mutations, then exit to prove allocation
teardown. Mutated segment bounds/alignment/relocations are rejected, and a
child process proves that unload with a live TLS-owning thread aborts.
A second child copies a valid token into a forged descriptor and proves the
registered-address check aborts.
