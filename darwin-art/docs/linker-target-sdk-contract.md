# Android linker target SDK contract

Audited against Bionic Android16 revision
`09a271af557444c9a6b3f3146d6d474156fd6cdb`. This is implementation guidance,
not a completed runtime contract or an APK acceptance result.

## Source identity

- `linker/linker_sdk_versions.cpp`: SHA256
  `ab700fc4222d7010d50aa4338ed023b53cf716554bd936c9c1c18182a6a26829`.
- `libc/bionic/libc_init_common.cpp`: SHA256
  `6838e79d1f9fcecce6d8d03f40b4bc9ba1c76ef6b986666df29a12c1ac1d33eb`.
- `linker/dlfcn.cpp`: pinned by
  `tools/bionic-guest-libdl-runtime/upstream-sources.tsv`. The similarly named
  local `_aosp/bionic-dl-iterate-phdr/linker/dlfcn.cpp` differs; use the pinned
  Android16 revision, not that local file as source identity evidence.

Original files are available under
https://android.googlesource.com/platform/bionic/+/09a271af557444c9a6b3f3146d6d474156fd6cdb/.

## Required owner and effects

1. Linker owns process target SDK, initially platform API. This is not device
   `ro.build.version.sdk`, per-classloader target SDK or a new property override.
2. Setter takes the same recursive linker operation lock as dlopen. A load
   cannot observe policy changing in its middle. Getter reads atomic state.
3. Input zero means platform API; original setter otherwise stores the input.
4. For target below30, call fdsan's property-aware setter with WARN_ONCE default.
   Existing `darwin_art_bionic_android_fdsan_set_error_level_from_property`
   is that owner; directly assigning WARN_ONCE would discard debug.fdsan policy.
5. Invoke the installed libc target-SDK hook, if any. Original libc's hook
   adjusts large-allocation slack only under USE_SCUDO without HWASan.
   Current allocator facade owns Darwin malloc blocks, not Scudo blocks; do not
   call an unrelated guest Scudo allocator or fabricate a successful Scudo hook.
6. Consumers must share this SDK owner: linker namespace exemption/legacy
   policies and load-time soinfo SDK snapshots. Writing a separate scalar
   without connecting those consumers is not implementation of this contract.

## Current tree and next cutover

ConfiguredNamespaces copies the parsed target SDK. Checkpoint1600 adds private
NamespaceHandles SDK state initialized from that configuration (zero maps to
the build platform API). Its internal setter holds NamespaceOperation and
invokes the existing property-aware fdsan owner below30. Staging a registry
does not alter process-wide fdsan. Actual process activation and the public
Bionic setter are not connected yet.
CreateForAddress explicitly rejects EXEMPT_LIST_ENABLED rather than silently
granting access. Checkpoint1601 connects new ELF publication to immutable SDK
metadata on its Rust image owner, under the same linker operation. Resident
reuse preserves that original SDK; old/native publications return unknown
rather than inventing one. Actual libsync ELF load/reuse/retained-lease tests
pass. Legacy namespace/symbol-policy consumers still need this metadata;
preserve explicit unsupported behavior until those owners are joined.

The component test verifies config/zero normalization, fdsan29/30 behavior,
same-thread recursive update and a competing update blocked by a live loader
operation. This does not establish old-SDK namespace or image policy support.

Connect activation and real image-policy consumers, then expose the loader
setter/getter in a typed linker image. Test zero normalization, property-aware
fdsan behavior, reentry/competing load serialization and SDK-boundary namespace
decisions. Do not register an empty ld-android image simply to satisfy the
libdl_android DT_NEEDED edge. 16KB appcompat has a separate contract and remains
pending. Physical Chrome/Calculator/DeskClock acceptance remains mandatory.

## Linear symbol lookup versus ordinary handles

Pinned Android16 `linker.cpp` dlsym_linear_lookup applies SDK23 to each candidate:
effective RTLD_GLOBAL includes it regardless of SDK; otherwise its linked SDK
must be below23. Current process SDK and local-group root flags do not replace
that image's metadata. NODELETE grants no additional lookup visibility.

Checkpoint1602 adds this eligibility predicate to the actual Rust image flag /
SDK owner and its private query ABI. It does not change ordinary handle lookup
or the shared/relocation global groups. Unknown non-GLOBAL metadata returns
unknown, not an implicit old SDK or a silently skipped image.

The complete caller is still required: walk the selected namespace's ordered
soinfo list, begin after the actual caller for NEXT, then on a miss walk the
caller's original local-group dependency graph (also skipping through the
caller for NEXT). A union of all registry images is not namespace list order.
Do not expose the predicate alone as completed RTLD_DEFAULT/NEXT support.

Checkpoint1603 connects a private LinearSymbol phase to a namespace-ordered
Rust image snapshot and the eligibility owner. NEXT uses exact image identity,
not SONAME or address equality. Original libsync.so is found/executed at SDK22
and remains eligible after process SDK36; starting after it misses its symbol.
Full public DEFAULT/NEXT routing and caller-local-group fallback remain pending.

Checkpoint1604 adds skip-through-caller support to the existing dependency BFS,
preserving the ordinary-handle wrapper. Before reaching the caller, traversal
continues into children even for inaccessible nodes, as in original AOSP.
Afterwards normal accessibility pruning resumes. Real four-image ELF fixtures
verify sibling-before-grandchild, NEXT to later grandchild and hidden subtree
exclusion. A private Rust query now retains the original local-group root using
registered member identity and shared group reference-owner identity, not names.
These pieces still need composition with LinearSymbol at the public caller ABI.

Checkpoint1605 composes them in the new backend's __loader_dlsym entry.
Ordinary handles keep LibrarySymbol; DEFAULT/NEXT retain the explicit ELF caller,
select its namespace, run LinearSymbol, and use its original local group only on
a real miss. Unknown metadata errors do not trigger a broader search. NEXT with
no registered caller rejects. The private darwin_art_linker_dlsym wrapper now
forwards its return address instead of discarding it.

Real ELF-address ABI tests prove modern-local DEFAULT fallback, NEXT to malloc
in a dependency, skipping sync_wait in the caller, ordinary-handle equivalence
and execution. This does not install ld-android or replace the old production
NativeLoader. Native/Mach-O caller identity, versioned symbols, complete provider
SDK/global metadata and full guest libdl linkage still need their own gates.

Checkpoint1606 transports an explicit version through __loader_dlvsym, ordinary
handles, linear lookup and local-group fallback to the existing image-specific
resolver. malloc@LIBC resolves through the original ELF caller's dependency for
all three modes; a nonexistent LIBC version is rejected. Raw Mach-O with no
Android version contract rejects a version request instead of silently dropping
it. Unversioned entry points still pass null.

This is NOT full Android version matching yet. Pinned linker_soinfo.cpp states
that an ELF DSO with no version table matches any version, and an undefined
requested version can match VER_NDX_GLOBAL definitions. The low-level selected
ELF export API currently promises exact version strings and lacks this frontend
distinction. Preserve that low-level contract for its existing users; introduce
the Android matching policy using real versym/verdef metadata, including global
versus local indexes, then exercise actual versioned ELF definitions. Also
distinguish a provider's unknown-version miss from malformed-provider failure
when traversing later images. Do not treat transport tests as those semantics.

Checkpoint1608 implements that additive Android frontend. Export metadata keeps
raw versym indexes; selected-image Android lookup consults actual verdef entries,
handles absent versym and unknown-version GLOBAL fallback, and masks hidden bits
for explicit requests. Strict selected-image lookup is unchanged. Namespace ELF
resolution now selects this frontend; provider UNKNOWN_VERSION is a search miss.
Rust library tests: 88 passed, 1 existing ABI-specific ignored test. Actual NDK
ARM64 ELF fixture executes TEST_1 hidden (11), TEST_2 default (22), and unversioned
GLOBAL (33), with negative strict/defined-version checks. Original libsync caller
tests accept arbitrary versions on its unversioned sync_wait. Native resident
suite passed; full preload audit still 18/28, not a production-cutover pass.

Checkpoint1609 connects original __loader_android_{set,get}_application_target_sdk_version
to the installed NamespaceHandles owner. Setter retains the owner's recursive
operation/fdsan policy; getter reads the same atomic state. Missing installation
is a fatal lifecycle violation, not a second default SDK store. Actual namespace
tests prove public/internal state identity and zero normalization. This exports
the backend ABI; original libdl_android's ld-android dependency still needs an
admitted real loader owner, including its remaining 16KB appcompat contract.
