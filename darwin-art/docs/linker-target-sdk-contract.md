# Android linker target SDK contract

Audited against Bionic Android 16 revision
`09a271af557444c9a6b3f3146d6d474156fd6cdb`. This is implementation guidance,
not complete production acceptance. Historical checkpoint notes live in Git.

## Upstream identity

- `linker/linker_sdk_versions.cpp` SHA-256:
  `ab700fc4222d7010d50aa4338ed023b53cf716554bd936c9c1c18182a6a26829`
- `libc/bionic/libc_init_common.cpp` SHA-256:
  `6838e79d1f9fcecce6d8d03f40b4bc9ba1c76ef6b986666df29a12c1ac1d33eb`
- `linker/dlfcn.cpp` is pinned by
  `tools/bionic-guest-libdl-runtime/upstream-sources.tsv`.

Use the pinned source at
`https://android.googlesource.com/platform/bionic/+/09a271af557444c9a6b3f3146d6d474156fd6cdb/`,
not similarly named local snapshots.

## Required contract

1. The linker owns the process target SDK. Zero normalizes to platform API.
2. The setter takes the recursive linker-operation lock; no load may observe a
   mid-operation policy change. The getter reads the same atomic state.
3. Targets below 30 invoke the property-aware fdsan owner with WARN_ONCE as the
   default. Do not bypass `debug.fdsan` policy.
4. Invoke the installed libc target-SDK hook when present. The Darwin allocator
   is not Scudo, so do not fabricate Scudo success.
5. Namespace policy, each image's immutable SDK snapshot and public Bionic
   setter/getter must consume this same owner.

## Current implementation

- `ConfiguredNamespaces` feeds `NamespaceHandles`; target zero normalizes to the
  platform API and updates serialize with loader operations.
- New ELF publication stores immutable image SDK metadata. Resident reuse keeps
  the original value; unknown legacy/native publications stay unknown.
- Original `__loader_android_{set,get}_application_target_sdk_version` uses the
  installed owner. Missing installation is a lifecycle failure.
- Linear symbol lookup implements Android SDK23 eligibility: RTLD_GLOBAL is
  always eligible; non-global candidates require linked SDK below 23.
- DEFAULT/NEXT select the caller namespace in list order, preserve exact caller
  identity, and fall back to the caller's original local dependency group only
  after a real miss. Ordinary handles keep their separate path.
- `__loader_dlvsym` transports explicit versions. Android lookup uses actual
  versym/verdef data, handles unversioned images and GLOBAL fallback, masks
  hidden bits for explicit requests, and treats provider UNKNOWN_VERSION as a
  search miss. Strict selected-image lookup remains unchanged.

Focused namespace and real ELF fixtures cover SDK normalization, fdsan 29/30,
recursive update, load serialization, SDK22 visibility, NEXT ordering, local
group fallback, ordinary-handle equivalence and versioned definitions.

## Remaining cutover

- Connect remaining namespace exemption and legacy image-policy consumers.
- Admit a real loader owner for original `libdl_android`'s `ld-android`
  dependency; do not register an empty image to satisfy the edge.
- Complete the separate 16 KiB appcompat contract.
- Cover native/Mach-O caller identity and complete provider SDK/global metadata.
- Replace the old production NativeLoader path and pass the complete preload
  audit; focused tests alone do not establish cutover.
- Re-run unchanged Chromium, Calculator and DeskClock physical acceptance.

Unknown SDK, metadata or version state must fail explicitly. Do not widen a
search, assume an old SDK or silently drop a requested version.
