# NativeLoader production cutover audit

Evidence inspected 2026-09-12. This is an integration inventory, not a claim
that APK startup works. The complete acceptance goal remains in
architecture-migration.md.

## Latest dependency integration (checkpoint1611)

The new namespace backend now admits a concrete typed `ld-android.so` native
image, backed by strong references to actual loader owners, and loads the
unchanged original `libdl_android.so` through its preserved DT_NEEDED edge.
Original wrapper execution, SDK/page policy, namespace creation/link/isolation,
resident reuse and balanced close passed. Baseline and original payload hashes
both remain `739c5716653bcff6700501d98f576957fc299e4f2483c3ad2bd1c0f641e96422`.

Admission, relocation lookup, native dependency retention and handle lifecycle
recognize the private image type. No empty ELF, host dlsym or suppressed edge is
used. The immutable dispatch marker owns no dynamic resources or registry
shared_ptr; actual image leases still have Rust registry identity/lifetime.
Global visibility/SDK metadata and versioned loader exports remain incomplete;
do not infer a complete original libdl/ld-android ABI from these nine imports.

Preload still18/28, but binder_ndk now passes ld-android and reaches libcutils:
`android_fdsan_get_owner_tag@LIBC_Q` was the next missing strong symbol.
Log `/tmp/android-linker-native-image-final.log`. Production bootstrap still
uses the old NativeLoader; this is tested backend integration, not APK acceptance.

Checkpoint1612 supplies that query from the shared Rust descriptor tag store,
registers its exact sealed provider route, and verifies query/dup/close behavior.
Final binder_ndk load passes it and reaches missing
`__system_property_foreach@LIBC` in libcutils. Preload remains18/28.
Log `/tmp/android-linker-fdsan-query-final.log`; no actual APK acceptance yet.

Checkpoint1614 connects `__system_property_foreach@LIBC` to the existing Rust
property owner with unlocked callbacks and retained lifetime. Real sealed-ABI
iteration and original ELF relocation pass; binder_ndk now reaches missing
`strtoimax@LIBC` in libcutils. Preload remains18/28, not production acceptance.
Log `/tmp/android-property-iteration-preload.log`. Default parallel property
tests14PASS after test-only global-owner isolation; standalone process-state
audit remains failing on a pre-existing errno dependency-lock digest mismatch.

Checkpoint1615 adds signed `strtoimax@LIBC` through the existing numeric owner.
Original libcutils now advances to `mkdirat@LIBC`; preload remains18/28.
`/tmp/android-strtoimax-preload.log`. Sealed execution and6425 original-AOSP
comparisons pass. Reviewed process-state errno pins/imports are reconciled;
both numeric and process-state full audits now stop at five ELF-loader Clippy
errors, not the old digest mismatch. Production cutover remains incomplete.

Checkpoint1616 adds retained-directory `mkdirat@LIBC`. Original libcutils now
reaches `chown@LIBC`; preload remains18/28. `/tmp/android-mkdirat-preload.log`.
Directory/mount/cwd rename and error tests pass (FS58PASS/1existingignored).
Internal loader request grouping preserves C ABI and resolves the five Clippy
errors; numeric and process-state full audits now pass. Standalone FS audit
still stops on a pre-existing guest_path.rs dependency digest mismatch.

Checkpoints1617-1618 identify why `chown` cannot be treated as another alias:
native process credentials/stat still expose host IDs, while overlay ownership
is unstored. The old overlay fchown success-without-change is removed; it now
fails explicitly, not a claim of ownership support. Real implementation needs
trusted Android credentials and persisted guest inode ownership shared by stat.
`chown` is not newly exported. Full FS audit now passes after reviewed pins,
exact demand/import checks, legacy readonly mkdirat correction, test isolation
and static/format cleanup: `/tmp/android-fs-owner-audit-final.log`.

## Latest boundary correction (checkpoint 1596)

The imported NativeLoader device policy's two `/system/lib64` stat calls now
use `filesystem::GuestLinkerStat` and the Android stat layout through a pinned,
hash-verified patch. The upstream device workaround predicate is unchanged.
The policy build rejects host stat imports and requires the guest owner symbol;
guest-only file, absent file and host `/etc/passwd` exclusion tests pass against
the installed test VFS. This removes a host-filesystem escape from the imported
policy object, not the old production loader. Process registry installation,
original Initialize/Reset and managed symbol lookup cutover remain pending.

## Production owner still to replace

Checkpoint1597 reran the actual original public preload list with a freshly
linked namespace test: 18 loaded, 10 failed. Eight root files were absent from
the baseline materialization inventory. Their original PS16K bytes are now
hash-pinned in android16-system-native-files.lock; producer verification passes.
An isolated APFS-cloned baseline plus these files still loads18/fails10, now at
dependency admission rather than missing root lookup. Missing first edges:
libandroid_runtime (amidi), ld-android (binder_ndk via libdl_android), virtual
device AIDL (camera2ndk), libhwui (jnigraphics), graphics bufferqueue HIDL
(mediandk), libwilhelm (OpenMAXAL/OpenSLES), libhidlbase (RS), configstore HIDL
(vulkan), libandroidfw (webview platform support). Installing raw roots does
not connect existing Darwin port owners or constitute preload support. Do not
enable original Initialize against this incomplete set or waive failures.
Logs: /tmp/android-native-loader-{current,expanded}-preloads.log. No live
profile/image was changed by this check.

- `compat/darwin_native_bridge_stubs.cc` defines empty
  `InitializeNativeLoader` and `ResetNativeLoader` exports.
- `compat/darwin_runtime_native_loader.cc` retains the old library loader.
- `crates/darwin-art-build-contract/src/lib.rs` includes both in runtime
  source lists; `crates/art-bootstrap/src/probe/apk_direct.rs` also selects
  the old loader archive member explicitly. Updating only one build list is
  insufficient to remove the old owner.
- `tools/build-android16-native-loader-policy.sh` compiles the hash-verified
  `_aosp/android16-classloader-native-state/libnativeloader/native_loader.cpp`
  and original ClassLoaderFactory JNI into a separate archive. It does not
  install them in production. Do not confuse this source with the other
  native-library-control-flow import when reviewing patches or imports.

## What the successful ICU component test establishes

`tools/test-bionic-linker-config.sh ROOT --load` exercises original generated
configuration through guest filesystem access, configured namespaces, admitted
dependencies and an actual ELF graph load. The successful run is recorded in
checkpoint 1430. It does not call the production NativeLoader lifecycle.

The current implementation has these boundaries:

| Boundary | Existing owner | Remaining integration |
| --- | --- | --- |
| Initial registry | `loader/namespace_startup` | Install one process-owned registry before original NativeLoader initialization; drain users before teardown. |
| Opaque namespace handles | `loader/namespace_handles`, `namespace_creation` | Public linker ABI flags, caller/default selection and exempt-list behavior; do not silently discard unsupported flags. |
| File admission | `loader/namespace_files`, `namespace_discovery` | Invoke from actual library-open operations, preserving admitted descriptors and namespace identity. |
| ELF loading | `loader/namespace_load` | Accepts admitted Bionic and selected-ELF residents with original dependency identities. Still needs the production open/close caller. |
| ELF publication | `loader/namespace_publication` | NamespaceHandles publishes complete loaded graphs atomically at admitted placements. Production open orchestration/repeat-root handling remains. |
| Symbol resolution | `loader/admitted_provider_symbols` | Typed Bionic and selected-ELF resolution is connected; no host dlsym fallback. |
| ClassLoader policy | Original `LibraryNamespaces` and JNI archive | Replace old exports and register original JNI only after the real linker backend is connected. |

## Cutover order and required evidence

1. Close repeated-load ownership before exposing `android_dlopen_ext`:
   publish loaded ELF identities, resolve resident ELF dependencies, and prove
   repeated/shared dependencies retain the same image with balanced leases.
   A first isolated graph load cannot establish this requirement.
2. Connect namespace ABI calls to the process registry. Original
   `NativeLoaderNamespace::Create` uses ISOLATED, SHARED, EXEMPT_LIST_ENABLED
   and ALSO_USED_AS_ANONYMOUS. Original Link permits an implicit default
   target. Keep these semantics in the linker owner, not Java fixtures.
3. Route load/close/error and system-library entry points to that same owner.
   The compiled original loader still imports `android_dlopen_ext`, `dlclose`,
   `dlerror`, `OpenSystemLibrary` and `stat`. Audit the exact pinned source
   call sites before linking: accidental dyld or host-filesystem resolution
   is not an Android backend.
4. Replace the conflicting production exports and archive selection together;
   use original Initialize/Reset and ClassLoaderFactory registration. Verify
   source/link identity, then launch an unchanged APK through ActivityThread.
5. Run the full physical Chromium, Calculator and DeskClock acceptance suite.

No production bypass was removed by this audit. Property-service successful
writes, shared publication and Java property-authority unification also remain
unfinished; passing the ICU import chain does not waive those contracts.

## Selected-image lookup verification

Checkpoint 1433 adds an actual NDK-built ARM64 ELF test of the new selected
image lookup ABI. A retained clone continues to execute the root function
after the original graph handle and selected handle are released; its address
is unchanged. Dependency-only exports and nonexistent versions are absent.
The six local/global/protected-symbol fixture variants pass.

Historical gap at checkpoint 1433 (resolved by checkpoints 1440–1453):
This did not yet permit treating resident ELF as native-provider leaves:
The legacy resident C ABI still supplies no dependency metadata, and
`namespace_lookup_layout.rs` assigns dependency edges only to file entries.
Reusing that representation without extending the graph contract would drop
the resident ELF's DT_NEEDED edges from the local breadth-first lookup group.
Preserve those edges/identities explicitly before enabling resident ELF in
NamespaceAdmission. Do not append them to the global group as a shortcut:
that would change symbol precedence and visibility.

The internal discovery engine now accepts ordered resident edges, visits their
dependencies, records them on the result and revalidates repeated IDs plus
edge metadata. Cycles terminate after revalidation. The legacy C ABI explicitly
uses an empty list until an extended metadata handoff is connected; this is
not permission to enable ELF residents at the namespace adapter yet.

## Current repeated-dependency evidence (checkpoint 1453)

The actual ICU test now publishes the first mapped graph, deliberately stages
another libicu.so root from its admitted descriptor, then discovers and loads
its already-resident dependencies. Only one byte source is staged; the loaded
root's retained libicuuc dependency is the exact registry image, verified by
image identity comparison. Ordered resident edges and original bindings remain
enforced. This is dependency reuse, not yet root dlopen reuse, guest handle
close semantics or a production NativeLoader call.

Remaining first integration step: connect resident-first root open with loading
and publication, respecting path/caller/namespace semantics and concurrent or
reentrant opens. Do not expose the component Publish method as a complete
dlopen implementation or repeatedly publish fresh image identities.

## Constructor ordering blocker (checkpoint 1454)

Initial comparison used `_aosp/bionic-dl-iterate-phdr/linker/linker.cpp`, an
Android 15 snapshot, not Android 16 provenance. Checkpoint 1461 revalidated
the ordering against Android 16 revision below. Historical local anchors:

- Lines 1389–1418 skip SONAME lookup for names containing `/`.
- Lines 1109–1139 compare device/inode/file offset, with namespace-link access
  checks. Canonical path strings alone are not file identity.
- Lines 1823–1843 mark load tasks linked and establish cross-group references.
- Lines 2230–2243 obtain the linked image handle before calling constructors.

At checkpoint 1454, ELF graph loading instead called `run_initializers_for_graph` in
`namespace.rs` before constructing its shared GraphInner; `ffi_graph_load.rs`
then exposes a handle only after constructors. NamespaceHandles::Publish is
necessarily later. DsoLifecycle::publish_image registers an mmap reservation
for __cxa_atexit; it is not namespace publication and does not close this gap.

The required replacement was to split relocated graph ownership from initialization:
make the linked image group available to its linker owner before constructors,
establish explicit initialization/reentry state, and avoid holding a Rust
mutable reference or mutex across foreign initializer calls that can reenter.
GraphInner currently assumes post-initialization immutability for Send/Sync;
do not simply move constructor calls below Arc construction or mutate through
shared references. Preserve finalizer/lifetime contracts and add a constructor
that reopens itself/dependency as an execution test. Only then expose the root
open/reuse operation. A plain nonrecursive load mutex is not a solution.

## Android 16 synchronization provenance (checkpoint 1461)

Fetched original `platform/bionic` revision
`09a271af557444c9a6b3f3146d6d474156fd6cdb` and matched every SHA-256 against
`tools/bionic-guest-libdl-runtime/upstream-sources.tsv`:

- `linker/dlfcn.cpp`: `ca86981c9a4d8893e21845cd54c5f884f9f0cac2db5413d713fb12afe75acd7c`.
  Line101 defines recursive g_dl_mutex; lines137–149 hold it across do_dlopen.
- `linker/linker.cpp`: `fcdc080bf5666d396a2928390cf35f067cff25a3ba04485032a085f90316562f`.
  Lines1107–1115 match inode/device/offset;1391 skips path SONAME lookup;
  1827 marks tasks linked;2242 calls constructors after finding linked image.
- `linker/linker_soinfo.cpp`: `cab308b5cf1de18df5d23cfa84fd23b2600495df7eb58ef0f03e45f9b857c20d`.
  Lines463–478 skip previously-started initialization and mark it started
  before descending into children (485–487). This is not an error returned
  from guest dlopen. DT_INIT precedes DT_INIT_ARRAY (493–495).

Therefore the next root-open owner should preserve one recursive operation
lock, including constructors, while graph/registry data locks stay short-lived.
Do not add independently running per-image constructor workers. The low-level
initialize API currently rejects duplicate initialization; the future guest
open owner must reuse a started group's handle on same-thread recursion rather
than surfacing that low-level error. Competing-thread opens wait at the loader
operation boundary. Existing standalone libdl gate is test-only and must not
be promoted to production: it explicitly lacks the full caller/flag contract.

## Root-open integration status (checkpoint 1467)

The constructor-ordering component blocker above was addressed in checkpoints
1455–1463: link produces an owned graph before initialization; namespace
publication precedes foreign constructors, with rollback on initialization
failure. One recursive registry operation spans lookup, linking and constructors.
`NamespaceHandles::OpenLocalSoname` returns a resident lease on reentry rather
than calling low-level initialization again. The low-level initializer still
rejects repeat calls intentionally. Full guest dlopen/close is not exposed.

Explicit selected-namespace paths now admit an FD through the guest filesystem
and compare its inode/device/offset to visible resident images before checking
new-file canonical-path permission. This order matches pinned Android16
`linker.cpp` lines1191–1224 (hash above). Direct namespace-link SONAME permission
still applies to reuse; an unrelated resident inode is not a permission bypass.
File discovery uses the same descriptor if a new load is permitted. The strict
file-only admission API remains available without resident lookup.

Checkpoint 1468 connects new-path direct-link selection through OpenPath:
caller first, then ordered permitted direct targets, with target-local inode
lookup (no re-export through target links). Original basename link policy is
preserved. This remains the ordinary private operation, not guest flags or
legacy exempt-list policy. Actual fresh ICU placement is tested separately
from the concurrent first-SONAME-load test.

Still required: complete
caller/flags/FD-offset semantics, guest handle and close ownership, process
registry lifecycle, original NativeLoader production cutover, and physical APK
acceptance. Component progress must not be mistaken for a completed cutover.

## Unload boundary (checkpoint 1469)

Original NativeLoader `CloseNativeLibrary` calls dlclose. Pinned Android16
linker.cpp 1883–2000 decrements the local group's references, honors unload
eligibility/NODELETE, calls all local destructors before freeing any local
soinfo, then releases external group references. A registry lease release alone
does not implement that sequence.

The ELF graph now exposes an explicit destructor phase while retaining all
mapping owners. Group and image once guards prevent reentrant/repeated callback
execution; graph handle locks are released before callbacks. Automatic graph
Drop also performs a complete destructor pass before the unmapping pass.
The native fixture verifies a real ARM64 destructor runs once before graph
release and is not repeated by release while a selected image retains storage.

This is not guest dlclose yet. The Android linker owner must still maintain
group open/dependency references, NODELETE and destructor reentry policy, retain
namespace visibility during callbacks, unpublish afterwards and release groups
in the correct order. Do not wire NativeLoader Close to a raw lease drop or
call the destructor phase without establishing group unload eligibility.

## Local-group boundary blocker (checkpoint 1470)

Do not add dlclose reference counts around the current entire LoadedGraph.
The following source evidence changes the next implementation step:

- Pinned Android16 linker.cpp 1737–1827 creates local-group roots whenever
  a dependency crosses primary namespaces, in load-task BFS order. Each root
  gets its namespace's accessible dependency walk and global lookup group.
  Newly linked images retain their local-group root.
- Lines1840–1848 increment references for edges between different local
  groups. linker_soinfo.cpp 714–727 stores reference counts on that root,
  not independently on every namespace image lease.
- linker_soinfo.cpp 675–681 also prevents unload for RTLD_GLOBAL, not only
  RTLD_NODELETE. set_dt_flags_1 at577–588 promotes the matching ELF flags.
  The original file hash is cab308b5cf1de18df5d23cfa84fd23b2600495df7eb58ef0f03e45f9b857c20d.

Current mismatch in this tree:

- namespace_load.cc passes one global span into the ELF linker, then moves
  DiscoveredPlacement records into the result only after linking. Their
  namespace_id is not part of the current ELF linking inputs.
- ffi_types.rs DarwinArtElfDiscoveredGraph retains admission IDs and files,
  but does not carry original namespace/local-group ownership into relocation.
- namespace_lookup_layout.rs returns vec![group; files.len()], assigning
  every newly staged file the same root lookup scope. namespace.rs likewise
  builds one global_catalog and one GraphInner for the full discovered graph.
- Selected images retain that whole GraphInner. Its current explicit finalize
  is a whole-graph phase, NOT an Android cross-namespace group-close operation.

Therefore the next implementation must carry primary namespace and original
resident group identities into linking, derive namespace-specific local/global
lookup scopes, and split group lifetime ownership without losing dependency
mapping retention. Preserve actual image IDs and DT_NEEDED edges, rather than
grouping by SONAME or simply counting Arc references. Only then attach the
Android root open/dependency counts and NODELETE/GLOBAL unload policy.

Required regression: newly loaded root and dependency in different namespaces,
with distinct global symbol candidates; verify each relocation selects its own
namespace's group, closing root retains an independently opened dependency,
and destruction/unpublication occurs exactly once at its actual last reference.
The current ICU test's successful publication placement and inode reuse do not
establish this cross-namespace relocation/unload contract. No production
NativeLoader cutover is justified by those component results alone.

Checkpoint 1471 adds a namespace-group scope planner using image-slot identity,
primary namespace IDs and explicit accessibility. It gathers boundary roots in
BFS order, prunes inaccessible subtrees, keeps resident entries without taking
their ownership, and preserves the first linked group on namespace reentry.
The existing closed-single-namespace lookup adapter uses it as a degenerate
one-owner case. This does not yet pass actual discovery namespace IDs through
the linking ABI, supply per-namespace global catalogs, or split GraphInner
lifetime. Those are still required before root reference counting/cutover.

Checkpoint 1472 adds explicit NamespaceScopes to the Rust linking entry:
complete primary/accessibility/global-index inputs drive actual per-requester
local scopes and namespace-specific global catalogs. New DF_1_GLOBAL images
are included only in their primary namespace, in dependency BFS order.
Missing explicit metadata fails instead of falling back to unrestricted scope.
The legacy entry remains a declared single-namespace operation.

Real NDK root/child fixtures now relocate in namespaces10/20: child's variable
binds to its own definition (result16), an app-only global cannot interpose it,
and placing the global in the child's scope yields result78. This is execution
evidence for the Rust API, not C++ discovery integration. The C ABI still needs
to carry actual admitted identities/visibility/global snapshots; GraphInner
mapping/group lifetime separation and original NativeLoader cutover remain.

Checkpoint 1473 adds the discovery scope C ABI. Complete image-ID placements
and namespace visibility/global-index records are copied transactionally into
the discovered graph; unknown IDs, duplicate/missing data and invalid indices
are rejected. Subsequent linking consumes that scope set and checks the global
snapshot count. Native ARM64 fixture proves a resident excluded by the C scope
input fails relocation, then succeeds/executes42 after restoring visibility.
This closes the ABI transport gap, not policy collection: NamespaceHandles must
still derive these records from its live discovery/registry and capture every
participating namespace's actual global group. Do not fill them with all-visible
fixture scopes in production. Group lifetime separation remains pending.

Checkpoint 1474 connects actual scope collection: discovery retains successful
parent/image edges and queries original primary/secondary membership plus direct
primary-parent visibility, matching pinned linker_namespaces.cpp. Symbol access
is not inferred from namespace link allowlists or file path permission. Tests
distinguish a direct dependency from private grandchildren and dependencies of
secondary-only parents.

NamespaceScopeSnapshot captures every participating namespace's retained global
group, constructs the actual image-ID scope records, and attaches them before
the discovery context is destroyed. LinkNamespaceGraph now requires a ready
snapshot and uses those globals; the old caller-supplied single root global
span is removed. Actual ICU discovery/link/reuse and retained consumer execution
pass through this connection. This is the new loader component's real caller,
not original NativeLoader APK cutover; group lifetime/close remains unfinished.

Checkpoint 1475 preserves the planner's actual per-image group-root assignment
on the loaded graph, rather than discarding it after relocation. Selected-image
C ABI returns a retained original group-root image without namespace re-search.
Two independently mapped same-SONAME graphs remain different group identities;
the root can still be retrieved after the original graph handle is released.
The multi-namespace ARM64 example asserts separate root/child group roots.
This identity is a prerequisite for reference ownership, not proof of independent
group lifetime: selected roots still retain whole GraphInner mappings, so raw
release/finalize must not be exposed as Android group-close semantics yet.

Checkpoint 1476 replaces the flat mapping vector with mapping-group owners,
partitioned by the preserved planner assignment. Original image indexing stays
stable; each group owns its mapping/destructor order, while the graph performs
the full destructor pass before releasing group mappings and external owners.
Image identity now compares physical group ownership plus member position,
not the containing GraphInner pointer. The ARM64 cross-namespace example checks
that root/child have different physical groups and retained clones share one.
Graph views still retain every group. Next separate a selected group's dependency
retention from unrelated groups (including external/native owners) before using
these owners for independent close. No Android close eligibility is implied by
an Arc count or by this storage partition alone.

Checkpoint 1478 records relocation winners in the actual GraphResolver and
retains their original local/provider slots or retained-global snapshot indices
on GraphInner. Namespace-filtered and newly DF_1_GLOBAL catalogs carry explicit
source identities; matching export addresses or names are not identity evidence.
Both defined-symbol preemption and undefined-symbol resolution record winners,
deduplicating repeated bindings without retaining failed lookup candidates.
`relocation_elf_dependencies` exposes exact mapped/global-catalog ELF identities
for verification. External resolver providers are still recorded as admitted
slots, not exposed as selected ELF handles. These records must be combined with
DT_NEEDED and exact provider ownership when implementing group leases; they do
not themselves narrow current whole-graph retention or implement Android close.

Checkpoint 1479 connects those relocation edges and original DT_NEEDED edges
to physical mapping retention. Original local groups share a lifetime component
only for dependency cycles; a source-first DAG owns dependency components with
no strong-reference cycles. The component performs all local finalizers before
unmapping local groups, then releases dependency and external owners. GraphInner
destruction now releases its view instead of explicitly finalizing mappings
that may still have a dependent lease. `retain_mapping` exposes this low-level
physical lease; independent retained child execution after parent finalization
is exercised by the real ARM64 namespace example. Concurrent child release
during a blocked parent finalizer is covered by a mapping lifecycle test.

External globals/native resources have one shared resource owner, avoiding an
extra retained graph reference. Their retention is still conservative, not yet
per-image. Existing GlobalElfImage and selected-image C ABI still own graph
views: migrating those callers, exact external ownership and Android open/close
policy remain required before original NativeLoader production cutover. These
physical Arc leases do not implement RTLD_GLOBAL/NODELETE or guest dlclose.

Checkpoint 1480 removes whole-GraphInner ownership from GlobalElfImage and the
selected-image C ABI. Selection now owns a mapping lease, shared external
resources and immutable metadata with weak image identities. Group-root and
original dependency comparisons use those original identities after the graph
view is released. Borrowed global sources distinguish read-only selected images
from owning graph handles; global-input loading accepts either, but a selected
source cannot initialize/finalize a graph or select unrelated members.

The actual NamespaceHandles publication/resolver path consumes this ABI without
an extra full-graph reference. A real cross-namespace ARM64 C ABI test releases
the owning graph, observes only parent finalization, executes retained child16,
resolves its original group root and observes child finalization on last release.
The original global reference-count test now expects one graph-view reference,
because selected mappings intentionally no longer increment that count; callable
resident lifetime remains separately verified after the original view is gone.
Exact per-image external resource retention and Android logical open/close
eligibility remain separate unfinished work. Original NativeLoader/production
APK cutover has not been performed by this selected-owner migration.

Checkpoint 1481 scopes retained global images to physical components. Actual
resident binding indices and DT_NEEDED names form the inputs; a component unions
the requirements of all its members, while dependency components retain their
own resources. Selected images use that component's external owner instead of
the complete graph snapshot. The real ARM64 example proves an unused global's
finalizer runs after graph-view release, while a bound global remains through
child execution and finalizes only after the last child release.

For a declared dependency with multiple different same-name global identities,
resource retention is deliberately conservative (all candidates remain), but
identity comparison now reports unavailable rather than choosing the first.
Exact admitted dependency identity is still required to remove that ambiguity.
Opaque native owners and source descriptors remain unscoped until their per-image
ownership keys are transported. This change does not add Android logical close
eligibility or perform the original NativeLoader production cutover.

Checkpoint 1482 carries admitted image keys with native provider owners and
source-file descriptors. Named resident retention wraps the original callback
result (no extra native retain); descriptor retention wraps the same admitted
Arc<File>, preserving inode identity. Component resource selection keeps its
own source descriptors and providers named by DT_NEEDED or actual binding slots.
Selected native-dependency queries unwrap the original retained callback result.
Legacy unkeyed resource callers remain conservatively retained; no ownership is
inferred from raw addresses. Tests verify unused keyed resources release, actual
bound providers survive, and real descriptors outlive discovery but not their
last resource owner. Android logical open/close and NativeLoader cutover remain.

Checkpoint 1483 separates effective per-image GLOBAL/NODELETE state from
namespace visibility. Original Android16 soinfo.cpp577–584 promotes both ELF
bits;675–680 checks these effective flags only after linked state. Existing
visibility now consumes the shared atomic flag owner; NODELETE promotion does
not grant global/shared namespace visibility. The private image ABI exposes
effective booleans and one-way NODELETE promotion, not unload permission.

The new -z nodelete execution experiment was rejected by the existing ELF
DT_FLAGS_1 capability guard (parser.rs), before publication. That guard remains:
recording flags is not yet enforcing Android group close. The native test now
explicitly verifies rejection of an actual NOW|GLOBAL|NODELETE fixture, while
normal fixtures verify effective flags and shared-lease promotion. Group-root
identity in the runtime registry, logical open/dependency counts and eligible
unpublication/finalization must be connected before allowing this flag through
guest execution. No claim of NODELETE APK execution support is made.
