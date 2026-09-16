# Bionic virtual-memory facade

This standalone Tier-1 provider owns a deliberately narrow Android arm64
virtual-memory namespace. The pinned NDK r28c/API35 `libc++_shared.so` imports
none of these functions, so this is an explicit general-JNI extension rather
than part of the 160-symbol libc++ closure. A normally linked API35 fixture
imports `mmap`, `mmap64`, `mremap`, `munmap`, `mprotect`, `madvise`, and `__errno`, all at
the public `libc.so` `LIBC` symbol version.

`mmap` and `mmap64` accept a null address, nonzero length no greater than
`PTRDIFF_MAX`, anonymous private/shared or resolver-backed file mappings, and
`PROT_NONE/R/W/X`. Android `MAP_STACK` is treated as Linux's compatibility
hint. A null-address anonymous `MAP_GROWSDOWN` stack is allocated at its full
requested extent because Darwin has no equivalent automatic-growth contract;
non-anonymous or fixed stack-hint combinations remain capability-closed.
The anonymous fd is intentionally ignored and is never treated as a host fd.
Android `MAP_ANONYMOUS=0x20` is rebuilt as Darwin `MAP_ANON=0x1000`; guest flags
are never passed through. Provider-owned `MAP_FIXED`, shared anonymous, and
resolver-backed private/shared file mappings are supported. Ashmem and
unsupported hints fail with explicit Android errno. Offset and range arithmetic
are checked against the Darwin page size exposed by the surrounding process.

Each successful host mapping is registered in a mutex-protected interval table.
`mprotect`, `madvise`, and `munmap` operate on page-aligned owned subranges.
Splits retain their original allocation identity and adjacent segments are
coalesced again only when their allocation identity and effective mapping state
match; distinct adjacent `mmap` allocations therefore retain Linux `munmap` and
`mremap` boundaries. The ordinary VMA metadata capacity is independent from the
fixed-size async-signal-safe JIT snapshot, so non-JIT mappings cannot exhaust
signal-handler storage. Both have explicit fail-closed safety ceilings (65,536
segments and 4,096 JIT-capable ranges) rather than the previous shared 1,024
entry limit. Misaligned or zero ranges fail before Darwin. Calls and retirement
are serialized, so a concurrent operation either sees the live mapping or is
rejected from the table without invoking a host syscall. Provider drop unmaps
any remaining complete mappings.

Direct simultaneous write and execute mappings return Android `EACCES`; RW to
RX and RX to RW transitions are supported. An ownership-checked subset of an
anonymous private `PROT_NONE` reservation may later become RWX for Android V8
CodeRange compatibility. The host never creates an RWX or `MAP_JIT` mapping:
guest RWX starts as host RW, and an instruction/write protection fault changes
the faulting host page between RX and RW. This preserves page-level W^X across
renderer threads without a process-wide transition race and avoids Darwin's
thread-global `pthread_jit_write_protect_np` state. Ordinary guest RW, RX, R,
and `PROT_NONE` always use the exact host page permission. RX transitions and
RWX execution-fault recovery invalidate Darwin's instruction cache. The signal bridge
consumes only a permission fault inside a live guest-RWX range; other faults
remain ordinary Android signals. Truly concurrent write and execute activity
on the same host page can permission-ping-pong and is an explicitly unsupported tier; V8's coordinated
write scopes are covered by the Chromium acceptance gate. The Android ELF
fixture also writes AArch64 instructions into RW memory, changes it to RX, and
executes them.

Advice values are translated explicitly. Android `NORMAL/RANDOM/SEQUENTIAL/
WILLNEED` map to Darwin 0..3. Android `DONTNEED=4` uses an in-place fixed
anonymous remap to preserve Linux zero-fill semantics even
for protected PartitionAlloc ranges, and Android `FREE=8` maps to Darwin
`MADV_FREE=5`. File-backed `DONTNEED` remains file-backed and uses Darwin's
corresponding advice. Other known Android advice is
`EOPNOTSUPP`; unknown advice is `EINVAL`.

All guest failures publish Bionic pthread-local errno while C shims preserve
Darwin host errno. Unknown host errno translation sets a deterministic Android
`EIO` and a provider capability-failure bit. The resolver is closed and accepts
only the six exact versioned imports; it never uses dyld or `dlsym`.

File-backed mappings use the central virtual-FD resolver, which transfers a
duplicate host descriptor into the mapping operation without exposing it to the
guest. EOF/SIGBUS fidelity remains follow-up work. Repeated `munmap` of a retired
range is currently fail-closed `EINVAL`, stricter than Linux's successful no-op.

Run `./audit.sh`. It pins NDK/Bionic/Darwin headers and libc exports, verifies
the zero pinned-libc++ demand, ABI constants/signatures, exact C dependencies,
exact versioned Android ELF imports, normal execution, Clippy/formatting,
C-boundary ASan and UBSan, concurrent owner retirement, and target cleanliness.
The distributed Rust ASan runtime is not claimed.
