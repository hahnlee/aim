# Android/Bionic dl_iterate_phdr provider

This standalone provider is pinned to NDK r28c/API 35 and Bionic revision
`361ba86734fb2821a6adcfdf775db8abd04e0de0`. The public symbol is
`dl_iterate_phdr@LIBC` in `libdl.so`; it must extend the existing loader-owned
`libdl.so` provider rather than create a second SONAME owner.

## Android ABI and upstream behavior

Android arm64 `Elf64_Phdr` is 56 bytes and the API 30+ `dl_phdr_info` is 64
bytes, with `dlpi_adds` at offset 32. Bionic fills load bias, name, program
header pointer/count, process-wide load/unload counters, and current-thread TLS
fields. It invokes callbacks in load-list order, stops on the first nonzero
result, and returns that value unchanged.

Bionic holds its recursive loader mutex across callbacks. This permits same-
thread reentrancy but blocks unload for the whole iteration. The Darwin loader
contract uses immutable leases instead: it releases the graph lock before user
callbacks, allowing concurrent unload while preserving every pointer and mapped
image visible in the acquired snapshot. Nested iteration acquires an independent
lease and observes the then-current graph.

The callback signature is fixed and register-only:

```text
int(AndroidDlPhdrInfo*, size_t, void*)
```

It is directly callable in the currently proven Android/Darwin ARM64 register
subset. This does not generalize to arbitrary callbacks or spilled signatures.

## Loader snapshot C ABI

The provider binds one process-lifetime `AimLoadedImageSourceV1`. Each
`acquire` returns:

- an opaque lease;
- immutable, load-order records for every actually mapped Android ELF image;
- stable SONAME and copied/native `Elf64_Phdr` arrays;
- real load bias and image identity/generation;
- one pair of monotonic load/unload counters.

The record strings, headers, TLS data, and underlying image mappings must remain
valid until the matching release. Unload first removes an image from future
snapshots and increments `subs`, then defers unmapping until outstanding strong
snapshot references drain. Acquire and release must be callable recursively and
must not hold the loader graph mutex while the Android callback runs.

Virtual facade SONAMEs are not fabricated as phdr records. Only real Android ELF
images with real program headers are published. There is no dyld image walk,
Darwin `_dyld_*` fallback, or single-image constant table.

## Minimal loader integration

No existing loader file is changed by this gate. The eventual atomic integration
needs four additions inside the loader owner:

1. maintain monotonic `adds/subs` and load-order image records when a mapped
   `LoadedElf` enters/leaves the namespace;
2. expose a strong-reference snapshot acquire/release C ABI matching the header;
3. bind that source before resolving Android `libdl.so` imports;
4. add this one symbol to the existing `libdl.so@LIBC` allowlist, retaining a
   single resolver/SONAME owner.

Publishing copied phdr metadata without pinning the corresponding mapping is a
contract violation because callbacks commonly compute addresses as
`dlpi_addr + p_vaddr`.

## Gate

```sh
tools/build-android35-dl-iterate-phdr-provider.sh
```

The gate builds two actual Android ELF images, derives their real load biases
from loader-returned symbol addresses, and publishes parsed program headers.
The target ELF callback validates both SONAMEs and PT_LOAD entries, performs a
nested iteration, and verifies early-stop value 37. A concurrent callback holds
a two-image lease while the helper is unpublished; its pointers remain stable,
and the next Android iteration observes one image with `adds=2/subs=1`.
