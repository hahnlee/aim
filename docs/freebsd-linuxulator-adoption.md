# FreeBSD Linuxulator reference gate

This repository carries a small, immutable FreeBSD Linuxulator reference slice as an independent oracle for the AIM compatibility layer. It is not a plan to compile Linuxulator on macOS, and none of the vendored kernel code is linked into the runtime.

## Pin and gate

- FreeBSD source revision: `087d722624c7293c2cbc5ab2bc1e20b6a68aae96` (`main` observed 2026-08-16)
- Source origin: `https://github.com/freebsd/freebsd-src`
- Checked slice: 24 files, 377,278 bytes, with a SHA-256 and byte count for every file
- Android comparison: Android 16 Bionic revision `09a271af557444c9a6b3f3146d6d474156fd6cdb` through the existing checked `android16-os-constants-values.tsv`

Run the offline-capable verification gate with:

```sh
tools/materialize-freebsd-linuxulator-reference.sh
```

The command downloads only a missing pinned file, rejects a checksum or byte-count mismatch, rejects extra files in the slice, regenerates every manifest in a temporary directory, and diffs it against the checked output. `--update-manifests` is an explicit maintainer operation after reviewing a generator or pin change; it never advances the FreeBSD revision.

The generated manifests are:

- `arm64-syscalls.tsv`: all 308 arm64 records in `syscalls.master`, including implementation status, plus a consistency check against `linux_syscall.h`
- `constants.tsv`: 384 evaluable Linux ABI constants across errno, signal, open, mmap, and socket domains
- `errno-translations.tsv`: both symbolic directions of the FreeBSD/Linux errno tables
- `signal-translations.tsv`: both symbolic directions of the non-realtime signal tables
- `translation-ownership.tsv`: 239 concrete conversion, helper, and syscall-handler symbols, their source owner, and the explicit kernel-coupling classification

The Bionic differential currently compares 200 shared constants. Two differences are required and documented rather than hidden: Linuxulator's common `O_SYNC` value is `4096` while Linux arm64/Bionic exposes `1052672`, and the raw Linux realtime signal floor is 32 while Bionic exposes 42 after reserving ten signals. A missing, new, changed, or unexpectedly resolved difference fails the gate.

## Adoption boundary

| Linuxulator material | AIM use | Constraint |
| --- | --- | --- |
| arm64 syscall number/name/status table | Generate dispatch coverage and unsupported-syscall reports | Bionic/Linux UAPI remains authoritative when FreeBSD support is absent or stale |
| Linux errno numbers and symbolic conversion tables | Test Linux-facing errno values and design symbolic Darwin-to-Linux mappings | FreeBSD host errno numbers and lossy choices must not be copied as Darwin mappings |
| signal numbers, masks, and table intent | Validate raw kernel signal ABI and conversion coverage | Bionic reserves realtime signals; Darwin signal numbering/frame layout is different |
| open/fcntl flag definitions and `linux_common_openflags` behavior | Build flag matrices, unknown-bit policy, and path-operation semantics | Numeric reuse is unsafe; `O_SYNC` is a demonstrated divergence and `O_PATH` needs an emulation policy |
| mmap/madvise constants and validation order | Define acceptance/error tests and translation ownership | FreeBSD VM calls, locks, map entries, and page policy are not portable to Mach VM |
| socket/message/options constants and conversion routines | Validate sockaddr, cmsghdr, option, and message-flag conversions | Darwin option availability, control-message layout, and errno behavior require a native backend |
| emulation-root path lookup | Specify Android prefix lookup, fallback, and escape tests | `namei`, `vnode`, `pwd`, and `copyin/copyout` are FreeBSD kernel facilities |
| epoll/eventfd and futex state machines | Semantic reference for edge/oneshot, wake, requeue, and timeout tests | Implementation must use Darwin primitives and preserve Linux ordering/error behavior |
| arm64 `linux_sysvec` and ELF personality | Reference register, return-value, auxv, signal-frame, and vDSO responsibilities | `sysentvec`, brand registration, trap entry, `struct thread`, and kernel exec hooks have no supported macOS equivalent |

The reusable unit is therefore a semantic rule, ABI datum, or test vector. The C implementation remains source evidence. Code that depends on `struct thread`, `copyin/copyout`, `kern_*`, `namei`/`vnode`, FreeBSD VM objects, kernel locks, Capsicum, RACCT, or `sysentvec` is classified as `semantics-only-freebsd-kernel-coupled` in the ownership manifest.

## Differential test program

The checked constant comparison is the first layer. Compatibility work should extend it in this order:

1. Compare the generated arm64 syscall table with the pinned Bionic/Linux arm64 UAPI table, separating kernel syscall availability from libc wrapper availability.
2. For every Bionic errno, exercise Darwin failure cases and assert the Linux-facing result. Add explicit tests for lossy aliases, cancellation, restart, and unknown host errno values.
3. Run signal tests at both boundaries: raw Linux 1–64 semantics and Bionic's application-visible realtime range. Cover masks, `SA_*`, alternate stacks, interruption/restart, and synchronous fault delivery without copying a FreeBSD signal frame.
4. Exhaust the meaningful open-flag product space, especially access mode, `O_CLOEXEC`, `O_DIRECTORY`, `O_NOFOLLOW`, `O_PATH`, `O_DIRECT`, `O_DSYNC`, and `O_SYNC`. Compare return value, errno, descriptor flags, and observable I/O semantics against an Android helper binary.
5. Differentially test mmap protection/flag validation, fixed mappings, anonymous/file-backed mappings, alignment, partial unmap, advice, and failure ordering against Android on the same page-size fixtures.
6. Compare socket families/types, option get/set round trips, sockaddr lengths, ancillary-data alignment, credentials/rights passing, nonblocking behavior, and message truncation. Unsupported Darwin features need stable Linux errno contracts.
7. Treat epoll and futex as trace comparisons rather than constant comparisons: feed identical operation schedules to Android and the Darwin backend and compare readiness/wake sets, ordering invariants, timeouts, and errors.

FreeBSD and Bionic agreement is strong evidence, not automatic truth. AOSP Bionic and Linux UAPI decide the guest-visible ABI; Linuxulator is an independent implementation that exposes ambiguity and provides mature semantic test ideas.

## Licensing and provenance

The source slice is an unmodified subset of FreeBSD and retains every original copyright, SPDX identifier, and license header. The pinned top-level `COPYRIGHT` file is included. Files in the selected slice use permissive FreeBSD licenses, including BSD-2-Clause and BSD-3-Clause terms; attribution and disclaimer requirements remain file-specific.

Do not remove those headers or copy implementation text into a new runtime file without preserving the applicable notice. Generated manifests record facts derived from named source lines, but they retain explicit FreeBSD provenance and should travel with this document and the source/checksum lock. Revision updates require reviewing upstream license-header changes as well as ABI diffs.
