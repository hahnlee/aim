# AOSP NativeLoader policy import

`bash tools/build-android16-native-loader-policy.sh` verifies pinned source
hashes and compiles original Android 16 `LibraryNamespaces` and
`NativeLoaderNamespace`, `public_libraries.cpp` and generated VNDK property
accessors into Mach-O objects. `ART_TARGET_ANDROID` is intentional:
the host branch is not a namespace implementation. The script checks that the
device policy exports exist and linker/NativeBridge backend calls remain.
It also compiles the hash-checked original `native_loader.cpp` lifecycle owner
from the existing Android 16 classloader source import. Its namespace creation,
initialization/reset and library-open exports are checked, with unresolved
imports recorded in `loader-required.txt`. This does not install its exports
alongside the old production loader: that replacement still needs the backend.
The same archive compiles hash-pinned original ClassLoaderFactory JNI, verifying
its registration export and CreateClassLoaderNamespace import. No replacement
JNI body or successful-null namespace stub is used. Runtime registration is
still pending the backend and removal of conflicting loader exports.

The hash-locked guest-config patch changes only file/directory opening to the
guest filesystem boundary; parsing and library selection remain original.
The type include supplies Darwin's missing `off64_t` spelling and verifies the
original `android_dlextinfo` layout. A separate NativeLoader-only libdl include
uses Android arm64 flags and redirects dlopen/dlclose/dlerror away from macOS
libdyld to the Android linker owner. Its unresolved close implementation remains
a required backend, not a success stub or host fallback. The symbol gate rejects
unprefixed host libdl imports. Original SDK-level headers and VNDK sysprop input are hash-locked;
generated source/header output is also hash-checked. Requires the existing
AOSP `sysprop_cpp` generator built by the HWUI dependency preparation.
This archive is **not yet a production link input** and is not a runtime or
Chrome acceptance test. The standalone test executes the original public/APEX
config parsers only. Generated VNDK getters are tested in the guest test against
the real process property owner, not by writing to a host map. No namespace/backend implementations are mocked in
that test: unused backend-dependent functions are dead-stripped. A symbol gate
rejects libbase's host property store in the guest executable. A second
guest-config test installs the real filesystem facade and runs original default
public-library policy against a temporary guest system/APEX config. Original
extension-policy execution also runs against the guest filesystem. Its public
SDK function aliases the existing original bionic getter; direct property-get
imports are renamed to the bionic process-state ABI to avoid libbase's host
symbol. Tests compare SDK to the actual process property, without a new SDK
constant. This archive also compiles hash-checked original libbase properties.cpp
in device mode. The guest test links its GetProperty/GetIntProperty to real
bionic find/read callbacks, not libbase's host map. Serial/area-serial now share
the process property owner, and original CachedProperty read-only queries are
tested. Read-only callback value storage survives the callback as required by
that cache. Wait now uses process-owner notification, monotonic deadlines and
shutdown cancellation; original libbase matching/timeout paths are tested.
The setter remains an unresolved backend requirement; no success stub is
supplied. Serial declarations supplement the pinned public header.
The archive is not production-linked, and Java's separate property owner has
not been removed, so this is not complete property-authority unification. The build now
requires existing native/Rust provider closure archives for this test.

Remaining integration boundaries:

`crates/darwin-art-runtime/src/linker_namespace.rs` now owns typed namespace
IDs, explicit direct links and image leases, including shared creation-time
snapshots. Its lower-level path access rules follow bionic
`linker/linker_namespaces.cpp` at `361ba86734fb2821a6adcfdf775db8abd04e0de0`.
Shared creation also appends parent paths and copies direct links, following
Android 16 `linker.cpp` at `09a271af557444c9a6b3f3146d6d474156fd6cdb`;
the public `dlext_namespaces.h` comment claiming paths are not inherited is
inconsistent with that implementation. Parent images/links are snapshots,
while copied link targets remain live. Source hash is already pinned in
`tools/bionic-guest-libdl-runtime/upstream-sources.tsv`.
Local candidate generation keeps LD_LIBRARY_PATH and default paths separate,
with resolved DT_RUNPATH candidates between them; permitted paths are access
grants, not search paths. Candidate generation performs no I/O and grants no
access: the eventual guest filesystem loader must canonicalize/check the file.
`NamespaceRegistry::search_plan` snapshots only the caller and explicitly
permitted direct links. `linker_search.rs` performs file admission outside the
registry lock and returns the actual target namespace, canonical path and file
lease together. The opener supplies the coherent guest path/file pair; this is
not authorization to use host-global path lookup. Resident image lookup must
precede file search. Plans must be consumed while their runtime owner is live
and drained before teardown. The private C `darwin_art_linker_namespace_open`
composes this with the existing guest-image opener and Android errno getter,
returning an owned host fd plus target namespace. The integration test calls
the real filesystem facade through an actual denied symlink and public link;
fixture payloads are ordinary files, not loaded ELF images. This API is not
yet called by NativeLoader. Error detail propagation and full bionic search
error compatibility still require integration rather than treating this as
the public Android linker API.

`linker_runpath.rs` implements byte-preserving token expansion from the
canonical guest image path: ORIGIN and LIB (arm64 lib64), braces supported,
replacement text not recursively expanded. Android16 `set_dt_runpath` and
`format_string` are the reference, including prefix matches such as
`$ORIGIN_SUFFIX`. PLATFORM stays literal. The namespace-open ABI composes this
with guest directory resolution; existing directories are still not access
grants. APK entry resolution remains outstanding. `linker_discovery.rs` retains
canonical path and target namespace per admitted image ID, so descendants
search from their actual owner. It matches the ELF graph dependency callback;
it does not supply ClassLoader policy or multi-namespace symbol resolution.
This module is not yet called by NativeLoader or guest libdl. Its private C
boundary is `include/darwin_art_linker_namespace.h`, implemented separately in
`linker_namespace_ffi.rs`. Registry state is mutex-owned; lookup leases can
outlive registry destruction. Resource retain/release callbacks run outside
that mutex and must honor the documented thread/lifetime contract.
`cargo test -p darwin-art-runtime linker_namespace --lib` covers owner rules
and a real libSystem dlopen/dlsym lease across registry destruction. This proves
callback lifetime, not actual unmapping of the shared-cache system library or
ELF graph integration. It is not a guest Android namespace API or load test.
Canonicalization through the guest filesystem must precede access checks;
lexical checks alone are not symlink containment. Non-shared global-group
inheritance and symbol lookup of primary images' immediate dependencies still
need explicit integration with actual loaded-image metadata.

Non-shared parent-group inheritance now has a separate private C entry point
`darwin_art_linker_namespace_inherit`, implemented in
`linker_namespace_inherit.rs`. It accepts borrowed image leases, verifies actual
parent membership, and snapshots them without copying parent paths or links.
Selection from DF_1_GLOBAL / RTLD_GLOBAL metadata remains the linker's job;
the ABI itself does not implement soinfo flag selection or public namespace
creation. The real Mach-O lease test exercises this boundary and resource
lifetime, not complete Android dependency loading.

`linker_group_snapshot.rs` provides owned ordered snapshots of resident
DF_1_GLOBAL or RTLD_GLOBAL images. Native consumers borrow payload/SONAME pairs
from that snapshot, never from an unlocked registry iterator. Registry teardown
does not invalidate a live snapshot; destroying it releases its image leases.
`compat/loader/namespace_elf_group` now converts selected ELF snapshots to the
ELF load ABI, preserving Rust ownership. The real NDK execution fixture covers
registry/source destruction before load and snapshot destruction before guest
execution. Opaque/non-ELF members are rejected, not silently skipped. This is
not yet called by the production NativeLoader namespace backend.

Startup must follow `init_default_namespaces` in the pinned bionic `linker.cpp`,
not fabricate one permissive default namespace. That function consumes the
binary linker configuration: isolated/search/permitted/allowed libraries,
visible exports, and explicit named or allow-all direct links. `linker_links.rs`
now distinguishes named sets from explicit all-library grants, exposed through
the private `darwin_art_linker_namespace_link_all` ABI. Resident lookup and file
search both stop at direct targets; shared namespace creation copies the grant
but sees subsequent loads in that target. Empty named sets remain errors.
`darwin_art_linker_namespace_create_configured` now accepts configured allowed
library basenames alongside search/default/permitted paths. The original
private create ABI delegates with no name restriction. Invalid names fail
before publication; empty lists retain bionic's unrestricted-name semantics,
without disabling isolated path checks. Loading installed configuration into
these owner APIs remains integration work.
The current APK launcher stages fonts/resources and selected DSOs, not the
complete `/system/etc` or generated `/linkerconfig` tree. The locked API36
image contains `public.libraries.txt` and `linker.config.pb`; use
`bash tools/extract-android16-linker-config.sh` for hash-verified extraction to
`_build/android16-linker-config/system/etc` without copying the system partition.
`upstream/android16-linker-config.lock` pins image and payload identities.
The protobuf is an input to AOSP linkerconfig generation, NOT the generated
`ld.config.txt` consumed by bionic. Generation needs actual installed APEX and
partition metadata; these extracted files alone do not enable NativeLoader.

Generator source: `bash tools/sync-android16-linkerconfig.sh` materializes the
pristine Android16 `platform/system/linkerconfig` commit pinned in
`upstream/android16-linkerconfig-source.lock`. Existing modified/mismatched
checkouts are rejected, never reset. The original `Android.bp` supports host
builds, and original `main.cc` exposes `--root` outside Android. Use that mode
for an actual installed-root snapshot; do not use recovery mode or inject an
empty APEX list as normal-app policy. The host build still needs protobuf-lite
and Soong's linker_config.proto generated code, libapexutil, tinyxml2-generated
apex-info-list bindings, libbase/log and host ICU. Source sync alone is not a
successful generator build or startup integration.

`bash tools/build-android16-linkerconfig-proto.sh` now generates the pinned Soong
schema with installed host protoc, builds original AOSP configparser plus the
generated lite implementation into `liblinkerconfig-proto-host.a`, and parses
the extracted real system input (38 provided/22 required libraries observed).
It also checks missing-file and malformed protobuf errors. Host protobuf/Abseil
come from matching Homebrew includes/libraries, with protoc version reported;
they are tooling dependencies, not guest runtime link inputs. This verifies
one generator dependency, not the full linkerconfig binary or APEX scanner.

`bash tools/build-android16-linkerconfig-apex.sh` builds original libapexutil and
generated lite ApexManifest into `libapexutil-host.a`. Its test uses the actual
hash-verified i18n APEX manifest, including a versioned directory to verify
original active-directory filtering (one com.android.i18n, version1, five
provided libraries). Source/schema hashes are in
`upstream/android16-linkerconfig-apex.sources`. This partial dependency
test is not a ScanActiveApexes/full-generator or installed-root acceptance.

`bash tools/build-android16-apex-info-xml.sh` builds pinned original xsdc with
OpenJDK17 and hash-pinned commons-cli1.2, generates the AOSP schema's tinyxml
binding, and archives it with verified existing tinyxml2 sources. The generated
parser/writer roundtrip and malformed-XML test pass using an explicitly
test-only inventory. `libapex-info-xml-host.a` is ready for the full generator;
no synthetic inventory is installed into the runtime.

Installed-root audit: the launcher currently copies selected resources/DSOs
and exports host i18n/tzdata data directories; it does not activate complete APEX
directories. The available original artifact manifests identify com.android.art
and com.android.conscrypt version360499999, com.android.i18n version1, and
com.android.tzdata version360499999. Archive availability is not proof of
installed-module completeness. Preserve manifests and actual payload layout in
the runtime installation owner before publishing apex-info-list.xml; do not
reuse the golden fixture inventory. Config extraction now defaults to the
bootclasspath's ps16k image; both locked image variants independently yield
identical public.libraries.txt/linker.config.pb hashes.

`bash tools/build-android16-linkerconfig.sh` now compiles original modules,
contents, generator and main into an arm64 macOS `linkerconfig` executable.
Uses GNU C++20 (upstream uses typeof), explicit unistd declaration inclusion,
and the host dependency archives above. Build and --help pass with the upstream
source tree pristine. `bash tools/test-android16-linkerconfig-golden.sh` prepares
upstream fixtures using generated protobuf JSON conversion and a test-only
portable equivalent of rundiff/prepare_root. It compares file sets and content
against untouched upstream golden_output: stage1, stage2, vendor_with_vndk,
gen-only-a-single-apex and guest all PASS. Fixtures live only in an owned
temporary directory; the generator/policy are unchanged. Actual installed-root
generation and runtime loading are not validated yet.

`tools/build-android16-apex-inventory.sh` builds a placement adapter around the
original ApexManifest reader and generated ApexInfoList writer. It requires
materialized named APEX directories and actual preinstalled SYSTEM archive files,
and rejects identity/count/duplicate/missing-source mismatches before output.
`tools/prepare-android16-linker-root.sh NEW_ABSOLUTE_BUILD_DIRECTORY` constructs
an offline snapshot from the locked payload preparation, uses APFS clones and
archive hardlinks, checks manifest equality against the original archives, and
runs the original generator in Android init mode (no `--strict`). Add
`--strict-audit` for the additional complete-dependency diagnostic. It does not
modify a live profile.
The first actual five-module snapshot FAILED strict generation: the full source
image system protobuf requires `libadb_pairing_auth.so` (and other omitted APEX
providers). This is a real installation/configuration mismatch, not permission
to fabricate providers. Subsequent source audit corrected a mistaken assumption:
normal Android init does NOT pass `--strict` (revision/hash in
`upstream/android16-linkerconfig-init.lock`). Original normal-mode generation
succeeds for the selected materialized APEXes without changing source policy or
declaring omitted modules present. Strict audit remains useful, but is not an
Android boot prerequisite. The build snapshot currently uses the host tool's
non-treble topology; actual runtime topology/config installation still needs
integration. The generated config is not evidence that any advertised system
library can load, and no snapshot has been activated in a running profile.

Build-owner audit (Soong revision/source hashes in
`upstream/android16-native-install-graph.sources`): `getLibsForLinkerConfig`
selects installed modules by packaging specs/partition, then external dependency
edges of those modules. `BuildLinkerConfig` emits declared stub/LLNDK interfaces,
not all ELF exports or all files named `.so`; host dependencies are excluded.
The system-image builder also retains an older Make-compatible path through
`systemprovide`. These are build graph policies, not runtime namespace searches.
`darwin-art-build-contract::native_install` now represents explicit selected
module placement, declared provided/imported interfaces and dependency edges.
It derives sorted partition lists and rejects missing graph nodes, duplicate
identities and conflicting interfaces. It does not discover API visibility from
binary symbols, serialize protobuf, or claim existing artifacts are installed.
Canonical packaging still needs to populate this contract from verified runtime
install inputs. The current native-loader builtin SONAME array is not suitable
input; it describes legacy resolver behavior, not Android API declarations.

`compat/loader/configured_namespaces` now transports original bionic
`NamespaceConfig` objects into a privately owned Rust namespace registry.
Creation/export precede explicit named/all-library links; any failure destroys
the staging registry without modifying an existing owner. It preserves default
LD_LIBRARY_PATH separately from configured default search paths, and bionic's
default-vs-additional-namespace allowed-library distinction. The test imports a
hash-pinned original header and exercises real registry links, exports, path
denial and image leases, but does not load a DSO. It still needs the original
file parser, guest filesystem boundary and production startup connection.
Publishing actual initial linker/libdl resident images and target SDK state is
separate outstanding work; this adapter does not invent those resources.

`tools/build-android16-bionic-linker-config.sh` imports hash-pinned original
bionic `linker_config.cpp` and `linker_utils.cpp` with their headers, compiling
both unchanged as Mach-O. Darwin adjustments only provide missing type/attribute
spellings; NDK fallback headers supply ELF declarations. Required symbols are
recorded under `_build/bionic-linker-config/required-symbols.txt`. This is not
yet a production archive input: ReadFileToString/access/realpath/stat must use
the guest filesystem, GetProperty the existing Android property authority,
and linker diagnostics actual logging. Do not link the current host filesystem
or host property functions as a shortcut. No parser behavior has been replaced.

The hash-checked `bionic-config-filesystem.patch` now routes pathname operations
through the guest filesystem adapter and uses Android stat layout. The imported
units use Android PATH_MAX=4096; the adapter still bounds copies into arbitrary
host-sized callers. Symbol gates reject host access/stat/realpath and pathname
ReadFileToString imports. Compilation passes, but the new real guest filesystem
integration test exposed stat of an absolute guest symlink using the old strict
no-link broker. This is now fixed in the separate filesystem `metadata.rs`
owner: following stat on the immutable guest root uses the guest-root descriptor
walk and metadata from the opened node, preserving symlink-before-parent order.
The original failing C++ test now passes against the rebuilt real providers;
all 25 filesystem unit tests pass. Existing lstat/private-overlay behavior was
not broadened by this fix. Property/logging/startup linking and full parser
execution remain pending.

`tools/test-bionic-linker-config.sh` now executes the actual generated root
configuration through the guest VFS, original bionic parser and Rust namespace
adapter. Five namespaces and i18n visibility pass; missing-file behavior retains
bionic's intentional empty ENOENT diagnostic, directory-read errors carry text,
and the copied registry survives parser reset. Original device GetProperty is
linked to the bionic property owner; the test rejects libbase's host map symbol.
Original async-safe formatter/linker debug source is imported. Only native log
transport is changed: on Darwin the upstream allocation-free stderr writev sink
is used, not Linux logd socket/syscalls. Fatal message storage calls the existing
bionic abort provider. This test still does not initialize the APK process's
NativeLoader owner or publish its real resident images.
`create_namespace` also resolves an absent parent from the caller's primary
namespace or the anonymous namespace; `set_anonymous_namespace` permits only
one successful assignment. NativeLoader's null system namespace fallback does
not authorize ignoring those caller/default distinctions. Read configuration
through the installed guest filesystem, and keep this linker state separate
from ClassLoader policy and macOS dyld state.

- Link the compiled original public-library policy into production. Its file
  and directory reads now use installed guest system/APEX configuration;
  its SDK query is connected in this archive, while libbase property queries
  still require production backend integration.
- Finish property authority integration: this policy archive's libbase getter
  uses bionic state, but the production Java map and host libbase build remain.
  Supply the authorized setter before full linkage; do
  not let missing device symbols resolve to an independent host property map.
- Supply real namespace create/link/load state, shared-library snapshots and
  explicit permitted/search paths for both ELF and Mach-O. Keep native resource
  ownership outside the original policy; do not stub success at these calls.
- Connect ClassLoaderFactory's original JNI and NativeLoader initialization to
  that policy. Preserve one serialized namespace owner, weak ClassLoader
  identity, parent lookup and real errors. Reconcile the current ELF identity
  registry with this owner instead of retaining two independent authorities.
- Replace current SONAME cache/selected-dylib policy branches with namespace
  lookups. A namespace link is an explicit grant, not process-global visibility.

The SDK-level header is in AOSP
`platform/frameworks/libs/modules-utils/build/include/android-modules-utils/sdk_level.h`
(not a `sdk_level/` subdirectory). It uses `android_get_device_api_level` and
`__system_property_get`; these must reflect runtime Android properties.
