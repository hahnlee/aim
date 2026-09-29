# ART exception: patches for the base-relative heap window

ADR 0012 (decision 4) runs the original Android userspace unmodified, except
ART: macOS arm64 cannot map anything below 4 GiB, while ART stores managed
references as absolute 32-bit addresses. This document records

- which of the aim patches (`patches/art/`, `patches/art-openjdkjvm/`,
  `patches/openjdkjvmti/`, all removed with the old stack; see git history)
  the new world still needs;
- the new series, `patches/art-android/`, applied to the ART sources of the
  image's tag (`patches/art-android/sources.lock`: `android-16.0.0_r1`, ART
  `ed6c006b`);
- how that ART is built as android-arm64 ELF against the original image
  libraries (the `art` node of `cargo aim`, [build.md](build.md));
- how the boot image is regenerated for it.

Tracking: #153 (migration), #161 (syscall-layer window contract), #165 (boot
image), #166 (P2 bring-up), #162 and #163 (build gaps), #164 (orphaned old
patches).

## Summary

| Class | Meaning | Count |
| --- | --- | --- |
| a | Needed for base-relative references / the high heap window | 84 |
| b | Darwin libc, Mach, Mach-O, Apple ABI or Darwin-port scaffolding; not needed on bionic + the Linux syscall layer | 95 |
| c | Host-independent ART fix we still want (kept out of the minimal series) | 6 |
| d | Unclear; the deciding question is in the row | 4 |

189 patches: 184 in `patches/art/`, 1 in `patches/art-openjdkjvm/`, 4 in
`patches/openjdkjvmti/`. Four (a) patches are split: only their reference hunks
are ported (0060, 0066, 0095; 0020 loses its Mach arena). Two more pieces of the
(a) design lived outside `patches/` in the old build and are ported too: the
nterp rewrites in `crates/art-bootstrap/src/runtime_art/nterp.rs` and the
`ImageWriter::IsInBootImage` rewrite in `runtime_art/dex2oat.rs` (removed;
see git history).

The new series is 8 patches, 4,717 lines as files (+1,544 / -348 changed
lines, 320 hunks), against 9,771 lines for the 184 Darwin patches. It contains
no Darwin, Mach or Apple code.

## How the window works

- `ART_HEAP_REFERENCE_BASE` is a build constant, like `ART_BASE_ADDRESS`. The
  build passes `0x0000010000000000` (1 TiB): 4 GiB-aligned, above 4 GiB, and a
  single-bit logical immediate, so `orr xN, xN, #base` decodes in one
  instruction. Zero keeps upstream behaviour; every `#if` in the series is
  `ART_HEAP_REFERENCE_BASE != 0`.
- A heap reference is the object's offset from the base. Because the base is
  4 GiB-aligned, the offset is simply the low 32 bits of the address, so
  encoding is free (a `W` register write) and decoding is `base | reference`.
  Offset 0 is null.
- Managed code (optimizing, baseline, JNI stubs, nterp) keeps references as
  32-bit values everywhere upstream does: HIR values, vregs, stack maps, spills,
  lock words, poisoning. Only a memory access through a reference, or a call
  that hands `mirror::Object*` to C++, decodes first.
- The runtime (C++) keeps native pointers; `PtrCompression`, `ObjPtr`,
  `LockWord` forwarding, stack/vreg visitors and the quick entrypoints convert
  at the boundary.
- Everything ART maps with `low_4gb` lands in `[base, base + 4 GiB)`. Image
  files keep Android's logical addresses (boot image at `ART_BASE_ADDRESS`
  0x70000000, i.e. host 0x10070000000); the image loader converts between
  logical and window addresses.
- Implicit null checks stay on: codegen decodes with `DecodeNullable` where a
  null check is folded into the access, so null still faults below one page and
  the upstream SIGSEGV handler needs no change.

### Syscall-layer contract (#161)

The series reserves nothing itself; the old Darwin patch did that with
`mach_vm_allocate`. The syscall layer owns the window
(`crates/aim-linux-abi/src/sys/window.rs`):

- **Reservation.** Before the program is loaded, `[0x10000000000,
  0x10100000000)` is reserved: an inaccessible host mapping with the Darwin
  VM tag 243. The loader, translated files, thread stacks, host allocations
  and guest mappings without a hint therefore never land in it (Darwin
  places those bottom-up from 4 GiB, Linux top-down far above the window).
  A fork child and an exec'ed program make it again; a fork child then
  maps the parent's window pages into it (`docs/fork.md`).
- **Unmapped to the guest.** Reserved pages look unmapped: they are absent
  from `/proc/self/maps` and from the VM walk behind `mincore`, `madvise` and
  `mremap`; `msync` and `mprotect` there fail with `ENOMEM` (ART's linear
  low-4 GiB allocator probes pages with `msync`).
- **Placement.** A guest `mmap` whose hint (with or without
  `MAP_FIXED_NOREPLACE`) lies in the window and whose range is all reserved
  gets exactly that address. A hint that cannot be honoured there is dropped
  (the mapping goes elsewhere, as Linux does), except with
  `MAP_FIXED_NOREPLACE`, which fails with `EEXIST`. `MAP_FIXED` replaces
  whatever is there, as always. ART checks that the result lies in the
  window and retries.
- **Release.** `munmap` of window pages, and the pages a shrinking or moving
  `mremap` leaves behind, go back to the reservation; `mremap` may grow a
  window mapping in place over reserved pages.
- **Null page.** A fault on the window's first page is delivered as a
  `SEGV_MAPERR` at its offset in that page (`si_addr` and the frame's
  `fault_address`), as a fault on Linux's zero page is: `base | 0` is a null
  reference that some compiled code decodes as non-null before an implicit
  null check (`NotificationChannel.setVibrationPattern`, an inlined
  `long[].length` of a null array, killed every restarted system_server).
  ART's handler then throws the NullPointerException the code expects.
  The layer names each such access (pc, lr) among its messages.
- Nothing is ever mapped below 4 GiB (Darwin's `__PAGEZERO`), so the first
  64 KiB, and the addresses ART's `Context::kBadGprBase` sentinel page
  guards, always fault. ART's "Could not reserve sentinel fault page at the
  right address" warning is expected: it reserves the page at its window
  address instead.

## The new series (`patches/art-android/`)

Applied in `series` order with `patch -p1` to a copy of the fetched ART
subtrees. The files of the patches are disjoint.

| Patch | Lines | Changes | Purpose |
| --- | --- | --- | --- |
| `0001-heap-reference-window.patch` | 121 | +48 -9 | The base constant, conversions, and `MemMap`'s low-4 GiB allocator moved into the window with plain `mmap` |
| `0002-runtime-heap-reference-encoding.patch` | 647 | +198 -59 | Reference encoding in the runtime: `PtrCompression`, `ObjPtr`, lock-word forwarding, heap/card table/bitmaps, vreg and GC-root visitors |
| `0003-image-logical-addresses.patch` | 360 | +104 -39 | Image loader, app-image writer, image writer, OAT code and entry-point addresses: logical <-> window |
| `0004-quick-entrypoint-boundaries.patch` | 591 | +252 -17 | Quick entrypoints (C++ and arm64 assembly) decode arguments from and encode results for managed code |
| `0005-nterp-heap-base.patch` | 353 | +81 -8 | nterp (arm64ng mterp) decodes before dereferences and C++ calls |
| `0006-arm64-codegen-heap-base.patch` | 1,425 | +565 -95 | Optimizing arm64 code generator; the shared `ReferenceCodegenARM64` helper |
| `0007-arm64-intrinsics-heap-base.patch` | 993 | +205 -109 | arm64 intrinsics |
| `0008-arm64-jni-and-baseline-heap-base.patch` | 246 | +94 -12 | JNI compiler and the baseline (fast) compiler |

### Hunks

**0001 — `libartbase/base/globals.h`** (1 hunk): `ART_HEAP_REFERENCE_BASE`
(default 0), `kArtCompressedReferenceBase`, `kArtCompressedReferenceLimit`, a
4 GiB-alignment `static_assert`, and `ArtCompressedReferenceToHostAddress` /
`ArtHostAddressToCompressedReferenceAddress` (logical low-4 GiB address <->
window address; other addresses pass through).

**0001 — `libartbase/base/mem_map.cc`** (6 hunks):
`GenerateNextMemPos` starts the linear allocator at `base + LOW_MEM_START`
(+ the bionic random offset); `CheckMapRequest` accepts a logical expected
address; `TryMemMapLow4GB` accepts only results inside the window;
`MapInternalArtLow4GBAllocator` scans the window instead of `[0, 4 GiB)` and
wraps to `base + LOW_MEM_START`; `MapInternal` places a logical `low_4gb` hint
in the window and bounds-checks against the window. Not ported from the old
0020: the Mach arena reservation, the Apple exec-`MAP_FIXED` pread copy and the
arena-preserving `munmap` (Darwin kernel workarounds; the window is the
syscall layer's job).

**0002 — runtime encoding** (44 hunks):

- `mirror/object_reference.h`: `PtrCompression::Compress/Decompress` store the
  offset and keep 0 as null.
- `obj_ptr.h`, `obj_ptr-inl.h`: `ObjPtr` encode/decode relative to the base.
- `lock_word.h`, `lock_word-inl.h`: the forwarding address in the 32-bit lock
  word is an offset; the payload is shifted as `uint32_t` before the base is
  added back.
- `gc/heap.cc`: preferred alloc-space begin and the card table's covered range
  move into the window; `ObjectIsInBootImageSpace`, `IsInBootImageOatFile`
  compare logical addresses; `HasAppImageSpaceFor` compares spaces directly.
- `gc/space/space.cc`: the large-object bitmap indexes the window.
- `stack.cc`, `thread.cc` (`ReferenceMapVisitor`), `quick_exception_handler.cc`
  (`DeoptimizeStackVisitor`), `monitor.cc`: references read from vregs and
  registers are 32-bit values and go through `CompressedReference::FromVRegValue`.
- `interpreter/interpreter_common.cc`, `method_handles.cc`: a vreg holds a
  reference as its 32-bit value, so identity checks compare that value.
- `interpreter/unstarted_runtime.cc` (13 hunks): reference arguments of the
  unstarted-runtime intrinsics are decoded with `CompressedReference`.
- `mirror/var_handle.cc`: a ByteBuffer view keeps the 64-bit address (a direct
  buffer backed by a managed array lives above 4 GiB).
- `jit/small_pattern_matcher.cc` (6 hunks): the JIT's trivial getter/setter
  stubs decode their receiver and encode object results.

**0003 — image addresses** (20 hunks):

- `oat/image.h`, `oat/image-inl.h`: `ImageHeader` accessors (image begin, oat
  begin/data/end, image roots) return window addresses.
- `gc/space/image_space.cc`: `RelocationRange` converts logical <-> window;
  relocation diffs become `int64_t` (a logical-to-window diff does not fit
  32 bits), for the primary boot image and for a boot image extension
  alike (the extension's diff was still truncated to 32 bits until P2 loaded
  one); boot image loading uses 64-bit begin addresses.
- `class_linker.cc`: boot-image method test uses the logical address.
- `runtime_image.cc` (app-image writer): boot-image membership, content
  offsets, descriptor hashing and pointer serialization in logical addresses.
- `oat/oat_file-inl.h`, `art_method.h`, `instrumentation.cc`: a code address
  or entry point that is still logical is lifted into the window before use,
  comparison or publication.
- `dex2oat/linker/image_writer.h`: `IsInBootImage` compares the logical address
  (from the old `runtime_art/dex2oat.rs` source rewrite).

**0004 — quick entrypoints** (37 hunks):

- `quick_trampoline_entrypoints.cc`: interpreter bridge, proxy handler, generic
  JNI end and the method-exit hook encode reference results
  (`CompressReferenceResultIfNeeded`); the invoke trampolines
  (direct/interface/super/virtual, IMT conflict) and `artInvokePolymorphic*`
  decode `this` received from managed code; the proxy handler returns 0 when an
  exception is pending instead of encoding a stale result.
- `quick_alloc_entrypoints.cc`: allocation entrypoints accept a 32-bit class
  reference (`DecodeAllocationClassArgument`).
- `quick_entrypoints_arm64.S` (21 hunks): `NORMALIZE_ART_METHOD_IN_X0` (a boot
  image `ArtMethod*` loaded as a 32-bit logical address gets the base) in the
  proxy handler, resolution trampoline, generic JNI trampoline and interpreter
  bridge; `INVOKE_STUB_CALL_AND_RETURN` and `art_quick_osr_stub` rebuild the
  native address of `L` results; `art_quick_aput_obj` (10 hunks) decodes array,
  value and classes and zeroes the lock-word half of a fresh object;
  `COMPUTE_ARRAY_SIZE_UNKNOWN` decodes the component type; the
  `art_quick_read_barrier_mark_reg*` family, `BAKER_RB_*` macros and the mark
  introspection entrypoint (fixed +0x100/+0x300 layout kept; the converting
  path lives after the tables; GC-root entry decodes IP0) convert around
  `artReadBarrierMark`.

**0005 — nterp** (`runtime/interpreter/mterp/arm64ng/*.S`): macros
`DECODE_NON_NULL_HEAP_REF`, `DECODE_NULLABLE_HEAP_REF` and
`NORMALIZE_ART_METHOD` in `main.S`; the card-marking write barrier decodes the
holder; `ExecuteNterpImpl`/`ExecuteNterpWithClinitImpl` normalize `ArtMethod*`;
33 decode sites: array length/data (`array.S`, `aput-object` keeps the 32-bit
reference for `art_quick_aput_obj`), `fill-array-data`, `throw`
(`control_flow.S`), monitor enter/exit (`other.S`), receiver and class for
virtual/interface dispatch with an explicit null check instead of the SIGSEGV
path (`invoke.S`), instanceof/checkcast, instance and static field holders
(`object.S`). The count matches the old build's audited inventory in
`nterp.rs`. Three sites the inventory missed came up in the original
system_server (P3): `ExecuteNterpWithClinitImpl` decodes the declaring
class it loads, and `new-instance`/`new-array` decode the class after
`art_quick_read_barrier_mark_reg00`, which returns it compressed while
the thread-local cache holds it decoded.

**0006 — optimizing code generator**:

- `compiler/utils/arm64/heap_reference_arm64.h` (new): `ReferenceCodegenARM64`
  with `Encode`, `DecodeNonNull` (`uxtw` + `orr`), `DecodeNullable` (keeps null);
  it never takes a hidden scratch register.
- `code_generator_arm64.cc` (70 hunks), grouped by what they convert:
  field get/set holders and card marking (`HandleFieldGet/Set`,
  `MarkGCCard`, `CheckGCCardIsValid`); array get/set/length and array-store
  type checks (`VisitArrayGet/Set/Length`); Baker read barriers
  (`GenerateFieldLoadWithBakerReadBarrier`,
  `GenerateArrayLoadWithBakerReadBarrier`, `GenerateReferenceLoad*`,
  `CompileBakerReadBarrierThunk`); dispatch (`GenerateVirtualCall`,
  `VisitInvokeInterface`, `VisitClassTableGet`); type checks
  (`VisitInstanceOf`, `VisitCheckCast`, `GenerateBitstringTypeCheckCompare`);
  class init (`GenerateFrameEntry`, `GenerateClassInitializationCheck`,
  `VisitLoadClass`); runtime-call boundaries (`VisitNewInstance`,
  `VisitNewArray`, `VisitLoadString`, `VisitLoadMethodType`,
  `VisitMonitorOperation`, `VisitThrow`, `VisitLoadException`,
  `VisitClearException`, unresolved field access); implicit null checks
  (`GenerateImplicitNullCheck`, nullable receivers); boot-image and JIT
  literals (`LoadBootImageAddress`, `LoadMethod`,
  `GenerateStaticOrDirectCall`, `LoadTypeForBootImageIntrinsic`,
  `kJitBootImageAddress` as a 32-bit reference); and the slow paths that call
  the runtime with objects.
- `code_generator_arm64.h`, `jit_patches_arm64.{h,cc}`: JIT root-table literals
  are 64-bit, because the JIT data region is a `low_4gb` mapping and now lives
  above 4 GiB.
- `code_generator_vector_arm64_neon.cc`: vector memory operands decode the base.
- `instruction_simplifier_arm64.cc`: no `HIntermediateAddress` for reference
  array gets (a 32-bit reference is not an address).
- `sharpening.cc`: `IsInBootImage(ArtMethod*)` compares the logical address.

**0007 — intrinsics** (`intrinsics_arm64.cc`, 79 hunks): decode before every
heap access in Unsafe get/put/CAS/getAndUpdate, VarHandle accessors and their
type checks, String compareTo/equals/indexOf/getChars/newStringFrom*,
`System.arraycopy` (char and object), CRC32, `Reference.getReferent/refersTo`,
`valueOf` boxing caches (decode class, encode the new object) and
`MethodHandle.invokeExact`; extra temps where a decoded address must survive a
loop; FP acquire/release variants keep a free scratch register.

**0008 — JNI compiler and baseline compiler**:
`JNIMacroAssembler::DecodeHeapReference` (lock object of synchronized natives)
and `NormalizeMethodPointer` (32-bit logical `ArtMethod*` at frame build), the
arm64 `TestMarkBit` decodes before reading the lock word; the baseline
compiler (`fast_compiler_arm64.cc`) decodes receivers, field holders and
checkcast objects and uses 64-bit root literals.

Not ported, although they came with (a) patches: the X0 jobject-return hunks of
0095 (upstream Android already returns 64-bit jobjects in the same register),
the inliner `DarwinJitCanCompile` gate of 0060, the "Darwin graph reject"
loosening of 0066 and the `VLOG(jit) "Darwin ..."` diagnostics.

The six (c) patches are left out of the minimal series; they are ordinary ART
fixes that can be carried separately or upstreamed.

## Building (the `art` node)

```
cargo aim build art
```

- **Toolchain:** Android NDK clang (`~/Library/Android/sdk/ndk/28.2.13676358`,
  clang 19, `--target=aarch64-linux-android35`) with the NDK bionic sysroot
  headers. libc++ headers come from the NDK with `_LIBCPP_ABI_NAMESPACE`
  switched from `__ndk1` to `__1` (generated `__config_site`), because the image's
  `libc++.so`, `libbase.so` and `libunwindstack.so` exchange `std::__1` types.
  `-nostdlib++`; the image's `libc++.so` is linked directly.
- **Link inputs (read-only, from the pinned image):** bionic `libc/libm/libdl`
  and `libdl_android` (runtime APEX), `libc++`, `libbase`, `libartpalette`,
  `liblz4`, `liblzma`, `libnativebridge`, `libnativeloader`, `libsigchain`,
  `libunwindstack` (ART APEX), `liblog`, `libz`, `heapprofd_client_api`
  (system), `libstatspull`, `libstatssocket` (statsd APEX). Every library links
  with `-z defs`, so all symbols resolve against the originals.
- **Sources:** AOSP subtrees fetched at `android-16.0.0_r1`
  and checked against the content hashes of `patches/art-android/sources.lock`,
  into `_build/aosp`: the ART subtrees the build reads (runtime, compiler, dex2oat, the libraries,
  openjdkjvm, adbconnection, ...), libcore's `jvm.h`, libnativehelper,
  libbase/liblog/libcutils/libunwindstack headers, libziparchive, vixl, lzma,
  zlib, tinyxml2, dlmalloc, fmtlib, BoringSSL, bionic platform headers, lz4,
  cpu_features, libcap, perfetto heapprofd header, modules-utils, statsd
  headers, and apexd's `ApexInfoList.xsd`. The series is applied to a
  staged copy of the ART subtrees (`target/aim/art/src/art`), rewriting only
  the files whose content changes. No Soong and no full AOSP
  checkout: `tools/art-android/bp_query.py` evaluates ART's `Android.bp` for an
  arm64 device (defaults chains, `arch/target/codegen` groups; arm64 codegen
  implies arm as in `art/build/codegen.go`) and `tools/art-android/gen_build.py`
  writes a ninja file with Soong's `art.go` global/device flags (CC with Baker
  read barriers forced, because the syscall layer has no userfaultfd), which
  n2 runs ([build.md](build.md), "n2").
- **Generated sources:** `operator_out` (upstream `generate_operator_out.py`),
  nterp (`gen_mterp.py`), `asm_defines.h` (cpp-define-generator, compiled for
  the target), ART aconfig flags (all `is_fixed_read_only`, declaration
  defaults), `com_android_apex`/`com_android_art` XML parsers (xsdc),
  libcap `cap_names.h` (upstream awk + `_makenames`).
- **Soong's device defaults** that change behaviour are passed too:
  `-ftrivial-auto-var-init=zero` (ART relies on it: `AssignVTableIndexes`
  keeps a `BitVector` in an `alloca` buffer it never clears, so without it
  `java.lang.String` links with a wrong vtable and `InitWithoutImage`
  aborts with "Class mismatch"), `-fno-strict-aliasing`, `-funwind-tables`,
  `-fno-short-enums` and `-fno-omit-frame-pointer`.
- **BoringSSL:** `dex2oat64` links `libcrypto_static` built from the fetched
  `external/boringssl/src` (`gen/sources.json`: the bcm and crypto sources and
  their Linux assembly, the flags of `external/boringssl/Android.bp`), as
  upstream does (#163). The platform `libcrypto.so` is not visible in the ART
  linker namespace.
- **Output:** `target/aim/art/` (~0.9 GB with debug info): `lib64/`
  `libartbase.so`, `libdexfile.so`, `libprofile.so`, `libart.so`
  (runtime + JIT compiler, same DT_NEEDED set as the original),
  `libopenjdkjvm.so`, and `bin/dex2oat64`; `stripped/` holds the same files
  without debug info, which `image/overlay.toml` puts into the derived image.
- **Status:** runs on the syscall layer (P2, ADR 0012): the image's
  `dalvikvm64` runs Java with and without a boot image, interpreted and with
  the JIT, and `dex2oat64` regenerates the boot image. A full build takes
  about 3.5 minutes on this machine (NDK clang runs under Rosetta).
- **Deviations:** `metrics/statsd.cc` is replaced by
  `tools/art-android/statsd_unavailable.cc` (upstream's own non-Android stubs),
  so ART reports no metrics to statsd, until `statslog_art` is generated
  (#162: that needs `stats-log-api-gen` built for the host from
  `frameworks/proto_logging`, which is not cheap); odrefresh metrics use
  upstream's host variant; `-Werror` is off (NDK clang 19 vs the platform's
  newer clang).

The rebuilt set must replace, as a unit, every ART APEX library that links
libart's C++ internals: libart, libartbase, libdexfile, libprofile,
libopenjdkjvm (and libopenjdkjvmti, libadbconnection, libperfetto_hprof when
those plugins are used). The `art` node builds libadbconnection
too (zygote loads it); libopenjdkjvmti and libperfetto_hprof are not rebuilt
yet, so their dlopen fails and ART runs without them. Libraries with C APIs (libnativebridge,
libnativeloader, libsigchain, libartpalette, libjavacore, libopenjdk) stay
original.

## Boot image (#165)

The original boot image cannot run on the rebuilt libart: its code and image
data hold absolute 32-bit references, and its compiled code bakes in the
original runtime's Thread and entrypoint offsets. The Darwin dex2oat
(`build-android16-boot-image`) cannot produce it either: its code follows the
Darwin runtime (Apple's JNI stack-argument ABI, the Darwin Thread layout, the
Darwin JIT gates).

Without a boot image, `Runtime::Init` takes `ClassLinker::InitWithoutImage`
and runs the boot class path jars on nterp and the JIT. That is how P2 first
ran Java, and how the boot image is made.

The `boot-image` node of `cargo aim` regenerates it with the rebuilt
`dex2oat64`, which runs on the syscall layer (`linux-run`) with the original
linker64, bionic and ART APEX libraries; the ART exception binaries are
mapped over the image's with `--path-map` file entries, so no derived image is
needed:

```
cargo aim build boot-image
```

- **Primary boot image:** the boot class path recorded in the original
  `boot.oat` (14 jars: the ART module's, the framework's and core-icu4j), as
  odrefresh compiles it: multi-image, `speed-profile` with both boot image
  profiles (`/apex/com.android.art/etc/boot-image.prof`,
  `/system/etc/boot-image.prof`), both `dirty-image-objects` lists,
  `/system/etc/preloaded-classes`, `--base=0x70000000`, lz4 images.
- **Mainline extension:** every `boot-<jar>.art` the original ships beyond
  the primary (here `framework-adservices`), with the jars its original oat
  file holds (here all 31 mainline BCP jars, one image), `verify` without a
  profile as the original, compiled against the new primary image, named
  after the base `boot`. An oat file compiled against the boot image names
  its components' checksums (`i;14/...:i;31/...`), so an extension with
  fewer jars than the original's would leave no app or system_server oat
  file valid.
- **Reproducible:** `--force-determinism` and `--avoid-storing-invocation`;
  two runs produce identical files. The whole run (primary and extension,
  load-time rewriting included) takes about 3 s.
- **Output:** `target/aim/boot-image/framework/arm64/boot*.{art,oat}`
  and `framework/boot*.vdex`. `image/overlay.toml` replaces the 45
  corresponding files under `/system/framework`; the original
  `arm64/boot*.vdex` symlinks stay and resolve to the replaced vdex files.
- **Loading:** the runtime finds it at its default location
  (`/system/framework/boot.art` with the profiles, plus
  `/system/framework/boot-framework-adservices.art`), mapped at
  `0x10070000000` (the window plus `ART_BASE_ADDRESS`); the loader always
  relocates from the logical to the window address (`0003`).

## Other oat files

The image's other oat files were compiled for the original boot image and
runtime. ART checks an oat file's read-barrier state first (the originals are
CMC's, `concurrent-copying = false`; this runtime has no userfaultfd, #442)
and then, for a filter with code, its boot image checksums; either mismatch
rejects the file, and artd reports `kOatCannotOpen ... Read barrier state
mismatch`. For a `verify` odex (139 of 168) that costs nothing: its vdex is
accepted and there was no code. The 29 with code (`speed`, `speed-profile`)
are compiled again by the `oat` node, as each original's header records
(compiler filter, class loader context without its checksums, boot class
path, `<jar>.prof`, app image), against the regenerated boot image. services.jar
is the `system-server` node's edited jar; profman turns its profile into text
and back against the edited jar. The node writes `target/aim/oat/overlay.toml`,
which `image/overlay.toml` includes; the image's dexoptanalyzer then answers
"no dexopt needed" for them (the originals: "dex2oat for filter").

## Classification

Files are abbreviated: `rt/` = `runtime/`, `opt/` = `compiler/optimizing/`,
`artbase/` = `libartbase/base/`.

| Patch | Files | Class | Reason |
| --- | --- | --- | --- |
| `0001-arm64-mach-o-assembly` | `rt/arch/arm64/asm_support_arm64.S` | b | Mach-O symbol spelling (`_` prefix, `.private_extern`, no `.size`) in `asm_support_arm64.S`; ELF assembly is upstream's own. |
| `0002-darwin-dynamic-page-size` | `artbase/globals.h` | b | Adds `__APPLE__` to the `__linux__` sysconf page-size guard; bionic defines `__linux__`. |
| `0003-darwin-allow-pthread-monitors` | `rt/monitor.cc` | b | Drops `static_assert(ART_USE_FUTEXES)` because Darwin has no futex; the syscall layer provides futex. |
| `0004-darwin-uncontended-monitor-lock` | `rt/base/mutex.h`, `rt/base/mutex.cc` | b | pthread fallback for the monitor lock when `!ART_USE_FUTEXES`; dead with futex. |
| `0004b-darwin-cross-thread-monitor-lock` | `rt/base/mutex.cc` | b | Extends 0004's non-futex monitor lock; dead with futex. |
| `0005-darwin-arm64-context-word-type` | `rt/arch/arm64/context_arm64.h`, `rt/arch/arm64/context_arm64.cc` | b | `uint64_t` vs `uintptr_t` are distinct types only in Apple's LP64 ABI; on bionic they are the same. |
| `0006-darwin-standard-signal-set` | `rt/signal_set.h` | b | Aliases `sigset64_t` for Darwin libc; bionic has it. |
| `0007-darwin-thread-cpu-time` | `rt/thread.cc` | b | Thread CPU time via Mach `thread_info`; bionic uses `clock_gettime(cpu_clock)`. |
| `0008-darwin-nonfutex-suspend-barrier` | `rt/thread_list.cc` | b | Compile fix in the `!ART_USE_FUTEXES` suspend barrier; dead with futex. |
| `0009-darwin-locksupport-park` | `rt/thread.cc` | b | Condvar `Park/Unpark` because futex is missing; upstream futex path applies. |
| `0010-darwin-host-gc-release-policy` | `rt/gc/collector/garbage_collector.cc` | b | Hard-codes lazy release because `IsKernelVersionAtLeast` needs Linux `uname`; the syscall layer answers `uname` and the build is `ART_TARGET`. |
| `0011-darwin-disable-userfaultfd-mark-compact` | `rt/gc/collector/mark_compact.cc` | b | Compiles out uffd/`MREMAP_DONTUNMAP`; the new build forces CC (`ART_FORCE_USE_READ_BARRIER`) and upstream falls back when `userfaultfd` returns ENOSYS. |
| `0012-darwin-arm64-quick-symbols` | `rt/arch/arm64/quick_entrypoints_arm64.S` | b | Mach-O symbol spelling for read-barrier introspection labels. |
| `0013-darwin-stat-mtime` | `rt/oat/oat_file.cc` | b | `st_mtimespec` (Darwin) vs `st_mtim` (bionic). |
| `0014-darwin-exec-pidfd-fallback` | `rt/exec_utils.cc` | b | Stubs `pidfd_open`; upstream already falls back to `waitpid` when `pidfd_open` fails (`exec_utils.cc`). |
| `0015-darwin-arm64-feature-fallback` | `rt/arch/arm64/instruction_set_features_arm64.cc` | b | Excludes AOSP `cpu_features` on macOS; it builds for bionic. |
| `0016-darwin-arm64-direct-c-symbols` | `rt/arch/arm64/native_entrypoints_arm64.S`, `rt/arch/arm64/quick_entrypoints_arm64.S` | b | Mach-O `_` prefix on direct `bl` targets. |
| `0020-darwin-low4g-mach-reservation` | `artbase/mem_map.cc`, `artbase/mem_map_unix.cc` | a | Moves ART's low-4 GiB allocator into the window. **Ported without** the Mach `mach_vm_allocate` arena, the exec-`MAP_FIXED` pread workaround and the arena-preserving `munmap` (all Darwin kernel workarounds): new-world 0001 uses plain `mmap` hints inside the window; keeping the window free is the syscall layer's contract (#161). |
| `0021-darwin-compressed-reference-window` | `artbase/globals.h`, `artbase/mem_map.cc` | a | Defines the base, the limit and the logical/host address conversions. Ported as 0001 with the base taken from `ART_HEAP_REFERENCE_BASE` instead of `__APPLE__`. |
| `0022-darwin-base-relative-heap-references` | `rt/mirror/object_reference.h`, `rt/gc/heap.cc` | a | `PtrCompression` stores the offset from the base; heap/card-table begin moved into the window. |
| `0023-darwin-enable-quick-allocation-entrypoints` | `rt/entrypoints/quick/quick_alloc_entrypoints.cc` | b | Turns upstream's own `!__APPLE__` guard into `#if 1`; always true on bionic. |
| `0024-darwin-arm64-ucontext-dump` | `rt/runtime_common.cc` | b | Register dump from Mach `ucontext`; the syscall layer delivers Linux `ucontext`. |
| `0025-darwin-morecore-diagnostics` | `rt/gc/space/malloc_space.cc` | b | Darwin bring-up logging in `MoreCore`. |
| `0026-darwin-base-relative-object-references-only` | `rt/mirror/object_reference.h` | a | Same `object_reference.h` hunk as 0022 for a second staged copy of the runtime; one copy is in 0002. |
| `0027-darwin-string-abi-overlay` | `rt/mirror/string-inl.h` | b | Include-path fix needed only because the old build force-includes `mirror/string-inl.h`; the new build does not. |
| `0029-darwin-arm64-native-stack-pcs` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc`, `rt/arch/arm64/jni_frame_arm64.h` | b | Apple arm64 packed stack-argument ABI in the generic JNI trampoline; bionic natives use AAPCS64. |
| `0030-darwin-large-object-bitmap-window` | `rt/gc/space/space.cc` | a | Large-object bitmap covers the window instead of `[0, 4 GiB)`. |
| `0031-darwin-class-loader-native-path-elements` | `rt/class_linker.cc` | c | `ClassLinker::CreatePathClassLoader` leaves `nativeLibraryPathElements` null (NPE in `findLibrary`); host-independent completeness fix of an upstream helper. |
| `0032-darwin-oat-dlopen-fallback` | `rt/oat/oat_file.cc` | b | Works around upstream's `__APPLE__` `LOG(FATAL)` in `DlOpenOatFile::PreLoad`; bionic takes the `dl_iterate_phdr` path. |
| `0033-darwin-base-relative-lockword-forwarding` | `rt/lock_word.h`, `rt/lock_word-inl.h` | a | GC forwarding address in the 32-bit lock word is an offset from the base. |
| `0034-darwin-fragmented-oom-hspace-retry` | `rt/gc/heap.cc` | c | Retries homogeneous-space compaction on a fragmentation OOM; no platform dependency. |
| `0035-darwin-rosalloc-clear-tail-protection` | `rt/gc/space/rosalloc_space.cc` | b | Needed only when `MADV_DONTNEED` does not zero; Linux semantics (syscall layer) do. |
| `0036-darwin-reader-writer-recursive-read` | `rt/base/mutex.h`, `rt/base/mutex-inl.h`, `rt/base/mutex.cc` | b | Darwin rwlock writer preference; non-futex path. Orphaned: no build applies it (#164). |
| `0037-darwin-shadow-frame-single-initialization` | `rt/interpreter/shadow_frame.h` | b | `__builtin_alloca_uninitialized` under `__APPLE__` only. |
| `0038-darwin-jit-memory` | `rt/jit/jit_memory_region.h`, `rt/jit/jit_scoped_code_cache_write.h`, `rt/jit/jit_memory_region.cc` | b | `MAP_JIT` + `pthread_jit_write_protect_np`; the syscall layer provides the upstream memfd dual mapping (P0 probe). |
| `0039-darwin-jit-primitive-gate` | `rt/jit/jit.cc` | b | Darwin JIT admission allowlist (`DarwinJitCanCompile`). |
| `0039-darwin-memmap-exact-anonymous` | `artbase/mem_map.h`, `artbase/mem_map.cc` | b | Mach exact anonymous mapping. Orphaned (#164). |
| `0040-darwin-jit-compiler-gate` | `compiler/jit/jit_compiler.cc` | b | Admission gate plus a Darwin-only `implicit_null_checks_ = false`; both undone later (0093, 0147-enable). Implicit null checks stay on (upstream) because 0124/0133/0134 decode with `DecodeNullable`. |
| `0040-darwin-jit-exact-data-api` | `rt/jit/jit_memory_region.cc` | b | Refines 0038's `MAP_JIT` reservation order. |
| `0042-darwin-compressed32-jit-gc-boundaries` | `rt/thread.cc`, `rt/stack.cc`, `rt/quick_exception_handler.cc` | a | GC root visiting of vregs/registers decodes 32-bit references (`FromVRegValue`). |
| `0042b-darwin-compressed32-monitor-boundary` | `rt/monitor.cc` | a | Same decode for the monitor's dex-register lock lookup. |
| `0043-darwin-jit-reference-return` | `opt/code_generator_arm64.cc` | a | Includes the codegen helper header; the helper is now `compiler/utils/arm64/heap_reference_arm64.h`. |
| `0044-darwin-arm64-compressed32-field-get` | `opt/code_generator_arm64.cc` | a | Field get decodes the holder before the load. |
| `0045-darwin-jit-forwarding-call-abi` | `opt/optimizing_compiler.cc` | b | "Darwin graph reject" JIT allowlist. |
| `0046-darwin-compressed32-managed-return-boundaries` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc` | a | Interpreter bridge / proxy handler encode reference results and decode the proxy receiver. |
| `0047-darwin-managed-return-arm64` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | Assembly: rebuild the native address of `L` results on the quick return paths. |
| `0048-darwin-compressed32-generic-jni-return` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc` | a | Generic JNI end encodes the reference result for managed code. |
| `0049-darwin-managed-exit-hook` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc`, `rt/instrumentation.cc` | a | Instrumentation method-exit hook encodes/decodes the reference result. |
| `0050-darwin-jit-native-root-slot-literals` | `opt/jit_patches_arm64.h`, `opt/jit_patches_arm64.cc`, `opt/code_generator_arm64.h`, `opt/code_generator_arm64.cc` | a | JIT root-table literals become 64-bit: the JIT data region is a `low_4gb` mapping, which now lives above 4 GiB in the window. |
| `0051-darwin-jit-code-retirement-order` | `rt/jit/jit_code_cache.cc` | c | `RemoveMethod` reinitializes entrypoints before freeing code and erases zombie entries in the non-native branch (asymmetry in upstream). |
| `0052-darwin-jit-boot-object-literals` | `opt/code_generator_arm64.cc` | a | `kJitBootImageAddress` literals use the 32-bit reference value, not the truncated pointer. |
| `0053-darwin-arm64-compressed-field-store-card-address` | `opt/code_generator_arm64.cc` | a | Decode before field stores and card marking. |
| `0054-darwin-arm64-compressed-array-addresses` | `opt/code_generator_arm64.cc` | a | Decode for array get/set/length and array-store type checks. |
| `0055-darwin-arm64-aput-object-runtime` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | `art_quick_aput_obj` / allocation fast paths decode references; zero the lock-word half of a fresh object (64-bit free-list links in the window). |
| `0056-darwin-arm64-allocation-boundary` | `opt/code_generator_arm64.cc` | a | Decode class / encode result around `NewArray`/`NewInstance`. |
| `0057-darwin-large-object-zero-on-release` | `rt/gc/space/large_object_space.cc` | b | `ZeroAndReleaseMemory` because Darwin `MADV_DONTNEED` does not zero; Linux semantics do. |
| `0058-darwin-arm64-array-component-address` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | Decode the component type in the array allocation entrypoint. |
| `0059-darwin-arm64-virtual-dispatch-addresses` | `opt/code_generator_arm64.cc` | a | Decode receiver/class before vtable loads. |
| `0060-darwin-jit-inline-capability` | `opt/inliner.cc`, `opt/code_generator_arm64.cc` | a | Split: `VisitClearException`/`VisitClassTableGet` decode hunks are (a); the inliner `DarwinJitCanCompile` gate is (b) and is not ported. |
| `0061-darwin-arm64-throw-boundary` | `opt/code_generator_arm64.cc` | a | Encode/decode around `LoadException` and `Throw`. |
| `0062-darwin-arm64-monitor-boundary` | `opt/code_generator_arm64.cc` | a | `DecodeNullable` before lock/unlock entrypoints. |
| `0063-darwin-arm64-type-check-boundary` | `opt/code_generator_arm64.cc` | a | Decode before instanceof/checkcast runtime calls and bitstring checks. |
| `0064-darwin-arm64-interface-check-boundary` | `opt/code_generator_arm64.cc` | a | Decode before the iftable walk. |
| `0065-darwin-arm64-array-type-boundary` | `opt/code_generator_arm64.cc` | a | Decode before reading the component primitive type. |
| `0066-darwin-arm64-class-load-boundary` | `opt/code_generator_arm64.cc`, `opt/optimizing_compiler.cc` | a | Split: clinit-check / `kRuntimeCall` load-class encode/decode are (a); the "Darwin graph reject" loosening is (b). Orphaned in the old build (#164); its codegen hunks are ported. |
| `0067-homogeneous-compaction-jit-roots` | `rt/gc/collector/semi_space.cc` | c | SemiSpace used for homogeneous compaction must sweep JIT roots even when the configured collector is non-moving. |
| `0068-darwin-string-resolution-boundary` | `opt/code_generator_arm64.cc` | a | Encode after `ResolveString`. |
| `0069-darwin-interface-dispatch-boundary` | `opt/code_generator_arm64.cc` | a | Decode before interface/IMT class loads. |
| `0070-darwin-imt-conflict-receiver` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc` | a | Quick trampolines decode `this` received from managed code. |
| `0071-darwin-unresolved-static-field-boundary` | `opt/code_generator_arm64.cc` | a | Encode/decode around unresolved field access calls. |
| `0072-darwin-polymorphic-runtime-boundary` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc`, `rt/method_handles.cc` | a | `artInvokePolymorphic*` decode receiver / encode result; method-handle vreg compare uses the 32-bit value. |
| `0073-darwin-polymorphic-runtime-dispatch` | `opt/intrinsics_arm64.cc`, `opt/code_generator_arm64.cc` | a | MethodHandle.invokeExact intrinsic decodes; encode after `ResolveMethodType`. |
| `0074-darwin-interpreter-reference-copy` | `rt/interpreter/interpreter_common.cc` | a | Interpreter shadow-frame copy compares references by their 32-bit value. |
| `0075-darwin-varhandle-addresses` | `opt/intrinsics_arm64.cc` | a | VarHandle intrinsics decode before heap access. |
| `0076-darwin-bytebuffer-native-address` | `rt/mirror/var_handle.cc` | a | ByteBuffer view keeps the full 64-bit address: direct buffers from managed arrays live in the window above 4 GiB. |
| `0077-darwin-varhandle-fp-acquire-scratch` | `opt/intrinsics_arm64.cc` | a | Scratch-register pressure fix required by 0075's extra decode register. |
| `0078-darwin-pthread-empty-checkpoints` | `rt/base/mutex-inl.h`, `rt/base/mutex.cc` | b | Polling pthread waits because Darwin waits cannot be interrupted like futex. |
| `0079-darwin-baker-reference-window` | `opt/code_generator_arm64.cc` | a | Baker read-barrier field loads use the decoded holder. |
| `0080-darwin-baker-array-reference-window` | `opt/code_generator_arm64.cc` | a | Same for array loads. |
| `0081-darwin-baker-mark-introspection-references` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | Mark-introspection entrypoint decodes before reading the lock word (the GC-root hunk is re-applied by hand to the ELF variant). |
| `0082-darwin-fast-jit-native-root-slot-literals` | `opt/fast_compiler_arm64.cc` | a | Baseline compiler side of 0050 (64-bit root literals). |
| `0083-darwin-reference-array-intermediate-address` | `opt/instruction_simplifier_arm64.cc` | a | No `HIntermediateAddress` for reference array gets: a 32-bit reference is not an address. |
| `0084-darwin-baker-unresolved-fields` | `opt/optimizing_compiler.cc` | b | Loosens the Darwin JIT graph gate. |
| `0085-darwin-baker-gc-root-thunk-address` | `opt/code_generator_arm64.cc` | a | Decode before the lock-word load in the GC-root thunk. |
| `0086-darwin-baker-mark-entrypoint-addresses` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | `art_quick_read_barrier_mark_reg*` decode/encode around `artReadBarrierMark`. |
| `0087-darwin-baker-unresolved-invokes` | `opt/optimizing_compiler.cc` | b | Loosens the Darwin JIT graph gate. |
| `0088-darwin-invoke-custom-graph` | `opt/optimizing_compiler.cc` | b | Loosens the Darwin JIT graph gate. |
| `0089-darwin-aosp-arm64-intrinsics` | `opt/optimizing_compiler.cc` | b | Loosens the Darwin JIT graph gate. |
| `0090-darwin-unsafe-get-addresses` | `opt/intrinsics_arm64.cc` | a | Unsafe get intrinsics decode. |
| `0091-darwin-unsafe-write-atomic-addresses` | `opt/intrinsics_arm64.cc` | a | Unsafe put/CAS/getAndUpdate intrinsics decode. |
| `0092-darwin-unrestricted-aosp-invokes` | `opt/optimizing_compiler.cc` | b | Removes the Darwin JIT graph gate. Orphaned (#164). |
| `0093-darwin-aosp-jit-admission` | `compiler/jit/jit_compiler.cc`, `opt/inliner.cc` | b | Removes the `DarwinJitCanCompile` gate added by 0060. |
| `0094-darwin-thread-cpu-nanotime` | `artbase/time_utils.cc` | b | Mach thread CPU time; bionic has `CLOCK_THREAD_CPUTIME_ID`. |
| `0095-darwin-arm64-jni-handle-return` | `compiler/jni/quick/arm64/calling_convention_arm64.cc`, `compiler/jni/quick/jni_compiler.cc`, `compiler/utils/arm64/jni_macro_assembler_arm64.cc` | a | Split: the `TestMarkBit` decode is (a); the X0 jobject-return hunks are (b) (upstream Android already passes 64-bit jobjects through the same register) and are not ported. |
| `0096-darwin-arm64-string-intrinsic-addresses` | `opt/intrinsics_arm64.cc` | a | String/arraycopy intrinsics decode. |
| `0097-darwin-arm64-crc32-array-address` | `opt/intrinsics_arm64.cc` | a | CRC32 intrinsic decodes the array. |
| `0098-darwin-arm64-reference-addresses` | `opt/intrinsics_arm64.cc` | a | `Reference.refersTo` intrinsic decodes. |
| `0099-darwin-arm64-vector-memory-addresses` | `opt/code_generator_arm64.cc`, `opt/code_generator_vector_arm64_neon.cc` | a | NEON/SVE vector memory operands decode the base register. |
| `0100-darwin-small-pattern-reference-abi` | `rt/jit/small_pattern_matcher.cc` | a | JIT small-pattern getter/setter stubs encode/decode (guard widened from `__APPLE__`). |
| `0101-darwin-private-trace-path` | `rt/trace.cc` | b | Darwin private host path resolver for traces. |
| `0102-darwin-logical-pthread-names` | `artbase/utils.cc` | b | macOS `pthread_setname_np` names only the caller. |
| `0103-darwin-app-process-system-class-loader` | `rt/runtime.h`, `rt/runtime.cc` | d | Publishes the system class loader to attached threads for the project's own app_process bootstrap; needed only if the new init still creates the loader after `Runtime::Start()` (original app_process does not). |
| `0104-darwin-attached-thread-class-loader` | `rt/jni/jni_internal.cc` | d | Companion of 0103; same question. |
| `0105-darwin-protection-fault-signal` | `rt/fault_handler.cc` | b | Darwin reports protection faults as SIGBUS; the layer delivers Linux SIGSEGV. |
| `0106-darwin-runtime-virtual-fd-export` | `rt/native/dalvik_system_VMDebug.cc` | b | Guest-fd to host-fd translation. |
| `0107-darwin-embedded-openjdkjvmti-plugin` | `rt/plugin.cc` | b | `dlsym(RTLD_DEFAULT)` fallback for an embedded JVMTI plugin. |
| `0109-darwin-null-cmdline-sigquit` | `rt/signal_catcher.cc` | b | Touches the non-`__linux__` branch only. |
| `0110-darwin-native-allocation-accounting` | `rt/gc/heap.cc` | b | `malloc_zone_statistics`; bionic uses `mallinfo`. |
| `0111-darwin-android-trace-clock` | `rt/trace.cc` | b | Forces the `__linux__` default under `__APPLE__`. |
| `0112-darwin-artbase-private-paths` | `artbase/os_linux.cc`, `artbase/scoped_flock.cc` | b | Darwin private host path resolver in `os_linux.cc`/`scoped_flock.cc`. |
| `0113-darwin-unstarted-reference-arguments` | `rt/interpreter/unstarted_runtime.cc` | a | Unstarted-runtime intrinsics read reference arguments with `CompressedReference`. |
| `0114-darwin-image-logical-address-window` | `rt/gc/space/image_space.cc`, `rt/oat/image.h`, `rt/oat/image-inl.h`, `rt/gc/heap.cc`, `rt/class_linker.cc` | a | Image relocation with 64-bit diffs; image header accessors and boot-image checks convert logical <-> window addresses. |
| `0115-darwin-generic-jni-remote-unwind` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc`, `rt/runtime.cc` | b | Darwin native unwinder registry hooks. |
| `0116-darwin-native-bridge-preinitialize` | `rt/native_bridge_art_interface.cc` | b | Un-guards an `#ifndef __APPLE__` call; no-op on bionic. |
| `0119-darwin-native-bridge-signals` | `rt/native_bridge_art_interface.cc` | b | `NSIG` fallback; bionic has `_NSIG`. |
| `0120-darwin-hprof-private-path` | `rt/native/dalvik_system_VMDebug.cc` | b | Darwin private host path resolver for hprof. |
| `0121-darwin-arm64-jni-monitor-boundary` | `compiler/jni/quick/jni_compiler.cc`, `compiler/utils/jni_macro_assembler.h`, `compiler/utils/arm64/jni_macro_assembler_arm64.h`, `compiler/utils/arm64/jni_macro_assembler_arm64.cc` | a | JNI compiler decodes the lock object for synchronized natives (`DecodeHeapReference`). |
| `0122-darwin-arm64-image-method-addresses` | `opt/code_generator_arm64.cc` | a | Boot/app-image RelRo method loads get the base. |
| `0123-darwin-unrestricted-aosp-loads` | `opt/optimizing_compiler.cc` | b | Removes a Darwin JIT gate. Orphaned (#164). |
| `0124-darwin-arm64-implicit-null-address` | `opt/code_generator_arm64.cc` | a | Implicit null check probes `DecodeNullable(ref)` so null still faults below one page. |
| `0125-darwin-runtime-image-logical-addresses` | `rt/runtime_image.cc` | a | App-image writer converts logical/window addresses (guard widened). |
| `0126-darwin-backtrace-collector` | `rt/backtrace_helper.cc` | b | Custom unwinder because libunwindstack is `__linux__`-only. |
| `0127-darwin-arm64-boxing-allocation-boundary` | `opt/intrinsics_arm64.cc` | a | `valueOf` cache miss decodes the class and encodes the new object. |
| `0128-darwin-arm64-early-fault-pc` | `rt/runtime_common.cc` | b | Mach fault PC diagnostics. |
| `0129-darwin-arm64-baker-intermediate-array-address` | `opt/code_generator_arm64.cc` | a | Baker array load decodes the intermediate address. |
| `0130-darwin-arm64-async-fault-pc` | `rt/runtime_common.cc` | b | Mach async-signal-safe fault dump. |
| `0131-darwin-arm64-boxing-cache-address` | `opt/intrinsics_arm64.cc` | a | Boxing cache array decode. |
| `0132-darwin-sigbus-user-sigsegv-chain` | `rt/fault_handler.cc` | b | Continues 0105's SIGBUS normalization. |
| `0133-darwin-arm64-implicit-invoke-receiver` | `opt/code_generator_arm64.cc` | a | Receiver decode for virtual/interface calls, nullable when the implicit null check is folded in. |
| `0134-darwin-arm64-implicit-field-receiver` | `opt/code_generator_arm64.cc` | a | Same for field get/set. |
| `0135-darwin-arm64-jni-stack-abi` | `compiler/jni/quick/arm64/calling_convention_arm64.h`, `compiler/jni/quick/arm64/calling_convention_arm64.cc`, `compiler/utils/arm64/jni_macro_assembler_arm64.cc` | b | Apple packed JNI stack arguments; AAPCS64 is upstream's `#else`. |
| `0135-darwin-remove-optimizing-allowlist` | `opt/optimizing_compiler.cc` | b | Removes the Darwin JIT gate added by 0093's predecessor. |
| `0136-darwin-arm64-char-arraycopy-addresses` | `opt/intrinsics_arm64.cc` | a | `System.arraycopy(char[])` decodes src/dst. |
| `0136-darwin-remove-optimizing-allowlist-tail` | `opt/optimizing_compiler.cc` | b | Second half of the gate removal. |
| `0137-darwin-arm64-frame-clinit-address` | `opt/code_generator_arm64.cc` | a | Frame-entry class status check decodes the declaring class. |
| `0138-darwin-sharpening-boot-image-address` | `opt/sharpening.cc` | a | `IsInBootImage(ArtMethod*)` compares the logical address. |
| `0139-darwin-arm64-boot-literal-reference` | `opt/code_generator_arm64.cc` | a | After ADRP+ADD of a boot-image object keep only the 32-bit reference (guard widened). |
| `0140-darwin-arm64-reference-intrinsic-class` | `opt/intrinsics_arm64.cc` | a | `Reference.getReferent` intrinsic decodes the class. |
| `0141-darwin-arm64-original-fault-context` | `rt/fault_handler.cc` | b | Mach VM crash-context dump. |
| `0142-darwin-arm64-generic-jni-tag-handoff` | `rt/arch/arm64/quick_entrypoints_arm64.S` | b | Plumbing for the Darwin unwinder registry. |
| `0143-darwin-compiled-jni-frame-publication` | `rt/entrypoints/quick/quick_jni_entrypoints.cc` | b | Darwin unwinder registry. |
| `0145-darwin-arm64-jni-method-pointer` | `compiler/utils/jni_macro_assembler.h`, `compiler/utils/arm64/jni_macro_assembler_arm64.h`, `compiler/utils/arm64/jni_macro_assembler_arm64.cc`, `compiler/jni/quick/jni_compiler.cc`, `opt/fast_compiler_arm64.cc` | a | JNI and baseline frames lift a 32-bit logical `ArtMethod*` into the window (guard widened). |
| `0146-darwin-arm64-managed-method-pointer` | `opt/code_generator_arm64.cc` | a | Frame entry / `LoadMethod` lift a 32-bit logical `ArtMethod*` into the window. |
| `0147-darwin-arm64-runtime-method-pointer-boundaries` | `rt/arch/arm64/quick_entrypoints_arm64.S` | a | `NORMALIZE_ART_METHOD_IN_X0` at quick entrypoints that receive `ArtMethod*` from managed code. |
| `0147-darwin-enable-implicit-null-checks` | `compiler/jit/jit_compiler.cc` | b | Reverts 0040's Darwin-only disable; nothing to port (upstream default). |
| `0150-darwin-oat-quick-method-header-image-identity` | `rt/oat/oat_quick_method_header.cc` | b | Mach-O `__TEXT` lookup for `IsStub()`; bionic uses `dl_iterate_phdr`. |
| `0153-darwin-boot-oat-logical-location` | `rt/gc/space/image_space.cc` | c | Passes the image *location* (not the filename) as the oat location, as upstream already does elsewhere in the same file. |
| `0155-darwin-allocation-entrypoint-class-reference-boundary` | `rt/entrypoints/quick/quick_alloc_entrypoints.cc` | a | Allocation entrypoints accept a 32-bit class reference defensively. |
| `0156-darwin-dex-cookie-identity-trace` | `rt/class_loader_context.cc` | b | Env-gated Darwin tracing. |
| `0157-darwin-define-class-dex-registration-trace` | `rt/native/dalvik_system_DexFile.cc` | b | Env-gated Darwin tracing. |
| `0158-darwin-register-dex-trace` | `rt/class_linker.cc` | b | Env-gated Darwin tracing. |
| `0159-darwin-objptr-base-relative-boundary` | `rt/obj_ptr-inl.h`, `rt/obj_ptr.h` | a | `ObjPtr` encodes/decodes relative to the base. |
| `0164-darwin-clone-dex-for-child-loader` | `rt/native/dalvik_system_DexFile.cc` | d | Clones a DexFile already registered with another loader; needed only if the collision comes from Android semantics rather than the old single-process multi-app model. |
| `0165-darwin-publish-aot-unwind-maps` | `rt/class_linker.cc` | b | Darwin unwinder registry. |
| `0166-darwin-oat-quick-code-host-address` | `rt/oat/oat_file-inl.h` | a | OAT method code address converted from logical to window. |
| `0167-darwin-art-method-entrypoint-window` | `rt/art_method.h` | a | Entry point read converted from logical to window. |
| `0168-darwin-instrumentation-entrypoint-host-address` | `rt/instrumentation.cc` | a | Instrumentation compares/publishes window addresses. |
| `0170-darwin-register-entrypoint-pairs` | `rt/art_method.h` | b | Darwin fault/unwind registry. |
| `0171-darwin-generic-jni-publish-caller-entry` | `rt/entrypoints/quick/quick_trampoline_entrypoints.cc` | b | Side effect for 0170. |
| `0173-darwin-publish-jit-method-ranges` | `rt/art_method.cc`, `rt/art_method.h` | b | Darwin fault/unwind registry. |
| `0174-darwin-publish-oat-method-ranges` | `rt/art_method.cc` | b | Darwin fault/unwind registry. |
| `0175-darwin-publish-native-registration` | `rt/class_linker.cc` | b | Darwin fault/unwind registry. |
| `0176-darwin-publish-jni-internal-registration` | `rt/jni/jni_internal.cc` | b | Darwin fault/unwind registry. |
| `0177-darwin-jit-fault-method-registry` | `rt/jit/jit_code_cache.cc` | b | Darwin fault registry. |
| `0178-darwin-aot-fault-method-registry` | `rt/class_linker.cc` | b | Darwin fault registry. |
| `0179-darwin-arm64-clinit-reference-boundary` | `opt/code_generator_arm64.cc` | a | Decode the class before the clinit status read. |
| `0180-darwin-arm64-fast-invoke-receiver-boundary` | `opt/fast_compiler_arm64.cc` | a | Baseline compiler: decode the receiver for dispatch. |
| `0181-darwin-fast-reference-codegen-include` | `opt/fast_compiler_arm64.cc` | a | Include/operand fix for 0180. |
| `0181-darwin-register-boot-oat-local-visitor` | `rt/class_linker.cc` | b | Darwin fault registry for the boot image. |
| `0182-darwin-arm64-fast-field-reference-boundaries` | `opt/fast_compiler_arm64.cc` | a | Baseline compiler field holders decode. |
| `0183-darwin-arm64-fast-checkcast-reference-boundary` | `opt/fast_compiler_arm64.cc` | a | Baseline compiler checkcast decode. |
| `0184-darwin-arm64-reference-referent-boundary` | `opt/intrinsics_arm64.cc` | a | `getReferent` intrinsic decode. |
| `0184-darwin-atomic-ptr-sized-fields` | `rt/art_method.h` | d | Acquire/release entrypoint accesses on Apple; likely only needed because the Darwin registries read entrypoints outside the mutator lock. Keep out until a race is shown on the new runtime. |
| `0186-darwin-suspend-barrier-diagnostics` | `rt/thread_list.cc` | b | Env-gated diagnostics. |
| `0187-darwin-guard-invalid-signal-context` | `rt/runtime_common.cc` | b | Darwin `uc_mcontext == nullptr`; Linux ucontext from the layer. |
| `0188-darwin-detach-native-thread-exit` | `rt/thread.cc` | b | Threads without bionic's pthread teardown; bionic pthreads in the new world. |
| `0190-darwin-runtime-start-boot-native-provider` | `rt/runtime.cc` | b | Registers boot natives without dlopen; the original libraries are dlopen'able. |
| `0191-darwin-native-client-visibility` | `rt/thread_list.h`, `rt/interpreter/mterp/nterp.h`, `rt/interpreter/interpreter.h`, `rt/parsed_options.h`, `rt/jit/profiling_info.h`, `rt/instrumentation.h`, `rt/common_throws.h`, `rt/ti/agent.h`, `rt/jit/jit_code_cache.h`, `rt/jit/jit.h`, `rt/plugin.h`, `rt/mirror/throwable.h`, `rt/runtime.h`, `rt/gc/heap.h`, `rt/entrypoints/runtime_asm_entrypoints.h`, `rt/entrypoints/quick/quick_default_externs.h` | b | Exports symbols for the Darwin in-process native client. |
| `0192-darwin-native-client-public-entrypoints-arm64` | `rt/arch/arm64/asm_support_arm64.S`, `rt/arch/arm64/quick_entrypoints_arm64.S` | b | Same, for assembly entrypoints (Mach-O). |
| `0193-darwin-shutdown-unregister-readiness` | `rt/thread_list.h` | b | Accessor for the Darwin host embedding. |
| `0195-darwin-fatal-signal-process-exit` | `rt/runtime_linux.cc` | b | Edits the non-`__linux__` branch. |
| `0196-darwin-fd-file-guest-open` | `artbase/unix_file/fd_file.cc` | b | Guest-path open shim. |
| `0197-darwin-os-guest-stat` | `artbase/os_linux.cc` | b | Guest-path stat shim. |
| `0198-darwin-mutex-parking` | `rt/base/mutex-inl.h`, `rt/base/mutex.cc` | b | `os_sync_wait_on_address` futex replacement. |
| `art-openjdkjvm/0001-darwin-jvm-last-error-string` | `openjdkjvm/OpenjdkJvm.cc` | b | Darwin `strerror_r` branch and `.dylib` alias; bionic's branch applies. |
| `openjdkjvmti/0001-darwin-monotonic-jvmti-time` | `ti_timers.cc` | b | Removes an `__APPLE__` gettimeofday fallback; no-op on bionic. |
| `openjdkjvmti/0002-darwin-malloc-size` | `ti_allocator.cc` | b | Darwin `malloc_size`; bionic has `malloc_usable_size`. |
| `openjdkjvmti/0003-darwin-in-memory-dex-file` | `ti_search.cc` | b | `mkstemp` instead of `memfd_create` + `/proc`; the layer provides both. |
| `openjdkjvmti/0004-darwin-search-lazy-system-classes` | `ti_search.cc` | c | `FindSystemClass` + `EnsureInitialized` instead of `LookupClass` for System/Properties in the JVMTI search path; host-independent robustness fix. |
