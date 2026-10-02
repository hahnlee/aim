# Boot status

A living record of how far the original Android 16 userspace boots under
aim-guest-init on the syscall layer (ADR 0012, tracking #153). Update it
with every boot-relevant change: the table, the date and the run it came
from.

## How to reproduce

```
cargo aim build        # <derived> is target/aim/derived/root
guest-init --image <derived> --data <data> --run \
    --exclude zygote,surfaceflinger,vold,bpfloader --timeout 45
```

- `--exclude` keeps services (and `exec` programs) from starting. A
  `wait_for_prop` or `exec` that only an excluded service would end is
  satisfied after two seconds, as with `--only`.
- Shutdown cleanup is incomplete on SIGINT: guest processes and the
  mounted data volume can remain after guest-init exits (#796).
- `<data>` is a case-sensitive disk image, `<data>.asif`, that guest-init
  attaches at `<data>` for the boot and detaches when it stops
  (docs/storage.md); `cargo aim storage` shows what it occupies.
- The device has no boot animation (`debug.sf.nobootanimation`);
  `--exclude bootanim` still works but is no longer needed.
- The device is `aim` (`androidboot.hardware=aim`):
  `/vendor/etc/init/hw/init.aim.rc` and `/vendor/etc/fstab.aim` from
  `image/overlay.toml`. The emulator's `init.ranchu.rc` and helpers are
  removed; `android-image diff` lists them with reasons. The vendor
  `build.prop` still describes the emulator (#206).

## Native system services (2026-09-29, ADR 0013)

`image/native-services` lists the system services implemented natively
(docs/system-services.md): today `clipboard`, `vibrator_manager`
(with `external_vibrator_service`), `location`, `thermalservice` and
`uimode`. The
derived image's `services.jar` does not start `ClipboardService`,
`VibratorManagerService$Lifecycle`, `LocationManagerService$Lifecycle`,
`ThermalManagerService` or `UiModeManagerService`
(their `SystemServerTiming` trace lines stay), and guest-init registers
the native services with servicemanager when `servicemanager.ready` is
set; `service check clipboard` finds it. The vibrator control service
(`IVibratorControlService/default`) is no longer declared or published.
Boots with it reach `sys.boot_completed` as before (four boots of a
reused data image, 20-25 s; fresh data images 50-52 s, then a cold
Settings start in 11.9 and 13.8 s); SystemUI and Gboard, which listen to the
clipboard, run without errors, and CTS's 36 clipboard tests pass. An empty list gives the original clipboard back.

PackageManager still runs original. On the M4 C branch, native
`package::owner::Store` writes enabled settings to the original ABX
restriction file with backup, reserve copy and system ownership (#798).
Two original-PMS boots on disposable native-written data (2026-10-02)
reached `sys.boot_completed=1`: Settings read as disabled (`enabled=2`),
then after a native reset to default, Settings launched successfully
(`am start -W`, 85 ms). This checks file compatibility; the native
PackageManager service and SystemServer facade are not activated. The
C branch also has Binder query receivers for `package` and
`package_native`, tested for caller visibility, interface tokens and
shared snapshot publication (192 package unit tests pass). These are not
registered in guest-init; their write path still returns an explicit
unsupported-operation exception.
The native scan's SystemConfig library reader also matches all 19
built-in library names and paths from `pm list libraries -v` and
`dumpsys package libraries` on a disposable original-PMS boot
(2026-10-02, `sys.boot_completed=1`). The scan-owned declaration registry matches all 23 original library
names/paths after signer reconciliation of all 243 active APK packages
and their three verified disabled originals (`scan_runtime`, 2026-10-02).
The same saved-record diagnostic compares actual code timestamps and native
policy-generated public/private flags with all 243 original saved values,
including GMS splits (2026-10-03, 146.49s). Flag inputs include the reported
active APEX inventory, the pre-framework overlay stage, refreshed factory flags
for data updates and the shared-UID privilege exception. All 16 saved shared UID
groups and package metadata retain their state;
verified public keys are rebuilt in the candidate. The diagnostic captures
the original library dump, stops original PMS, then scans only its disposable
reattached data image: persisted data is unchanged, and owned processes and
mounts are cleaned. It uses persisted record order, not the complete boot
image/data version selector. With isolated split dependencies and asset
scopes implemented and runtime density supplied, the diagnostic parses
all 243 installed APK packages, including GMS's 11 splits (#720). The
native parser's output also
matches all 288 original-PMS scan cache entries byte for byte on a
disposable boot (2026-10-02, `sys.boot_completed=1`); these include the
system GMS/Play Store packages, not the installed GMS cluster. A compiled
split APK integration test passes for manifest merging, split ordering,
dependency serialization, missing/cyclic dependencies and malformed
clusters; a unit test checks ancestor/config asset scopes without sibling
assets. Installed GMS's full parser parcel has not been compared against
the original (#760). This remains an incomplete native scan check.
Shared-library dependency resolution now has ordered direct selection,
static/SDK version and signer checks,
and file-path assembly from resolved provider snapshots. Tests cover
missing/optional/native dependencies, SDK policy, signer rotation,
multiple signers and deduplication. The candidate graph builder now
resolves acyclic APK provider graphs, fills nested dependency records and
package file paths, and marks static libraries installed for the direct
consumer's installed users. Tests check multihop ordering, input
immutability and user effects. Cyclic provider updates fail explicitly
(#799). The original PlatformCompat install-time native-library policy
is now queried through the system-server bridge (package name and target
SDK, without PMS lookup). On a disposable original-PMS boot (2026-10-02,
boot completed), a temporary Rust diagnostic received false for SDK 30
and true for 31/36; Settings started successfully (warm, 90 ms). The
diagnostic was removed and the boot/data cleaned. The original image's
`SharedLibrariesImpl` dex passes `required=true` for SDK dependencies;
`Policy::pinned` therefore disables SDK independence. An explicitly run
image integration test verifies that call argument and its straight-line
assignment, rejecting changed policy code (#800). `Policy::from_bridge`
combines it with the original PlatformCompat response. SDK dependency CTS
is still pending. Bootstrap facade wiring, native scan
integration and native PackageManager
activation remain pending (#707);
no native PackageManager CTS result is claimed.
The write/scan signature adapter now reads the parser's declared base and
split APK paths rather than discovering files by a `base.apk` name. An
original signed GSF APK verifies through nonstandard base/split guest
paths, retaining v3 and its two-certificate lineage; null and unreadable
split paths fail explicitly (#801). `scan::Inputs` now reads persisted
active and disabled-system APK locations with native parsing and full
signature verification, retaining SPKI keys. The signature adapter now
also serializes those keys for query parcels: RSA, EC and DSA streams
match the original runtime byte for byte and deserialize successfully
there (#738). The explicit runtime integration test checks six generated
keys, ArraySet hash ordering and duplicate keys, then compares the entire
native GSF SigningInfo parcel against an actual original-PMS
`getPackageInfo(GET_SIGNING_CERTIFICATES)` reply, including its rotation
lineage. Java API linkage verification also passes. Parse flags follow
the physical partition, rather
than an updated system package's saved system flag. On a disposable
original-PMS boot (2026-10-02, boot completed), all 243 active APK packages
and five disabled system packages parsed and verified. Every active
package's verified certificate, scheme version and lineage matched the
original persisted settings; disabled originals do not persist their
signatures. Settings launched successfully (warm, 90 ms). An explicitly
run original-image integration
test checks active/disabled GSF records, nonstandard APK paths, v3 lineage,
SPKI retention, input immutability and code disappearing between parsing
and verification. All 181 package
unit tests run with their inputs present and pass. These are scan inputs,
not reconciled package snapshots; APEX verification belongs to apexd,
and new/removed image package reconciliation and publication remain
unimplemented (#702). This does not activate the native scan.
`scan::Image` supplies the first-boot image inputs without settings:
overlay directories in reverse partition order, the framework, then each
partition's priv-app/app directories and active APEX directories in the
owner's reported order. Records retain the partition, privilege and
factory/changed APEX origin; duplicate package names are preserved for
reconciliation. Unsupported parser behavior and signature failures abort
the candidate; invalid directory candidates retain their rejection reason,
and a missing framework package fails. On a disposable original-PMS boot
(2026-10-02, boot completed), all 243 image APK candidates parsed and
verified. Their distinct manifest names matched all 243 system packages
from `pm list packages -s -f --match-libraries`, and every non-updated
system APK path matched; five updated packages run from data. Five
directories without APKs were reported as rejected. Settings launched
successfully (warm, 126 ms). Explicit image-input integration tests check
ordering, duplicate names, verified original APKs, stage filtering,
rejection retention and absent/empty framework failure.
`owner::app_ids::AppIds` restores active package and shared-user UID
ownership without changing persisted IDs, registering disabled originals
again or mutating settings. Its sparse slots preserve the pinned
AppIdSettingMap's array extent, allocation range and deletion cursor.
An explicitly run original-runtime integration test matches imported-hole
allocation, deletion, ownership replacement, fresh restoration and range
exhaustion. The disposable original-PMS boot completed and all 243 active
packages and 16 shared UID groups restored with their saved IDs. Java
API linkage verification and 181 package unit tests pass. The allocator
is not connected to scan reconciliation, persistence or snapshot
publication (#702). `owner::shared_users::Bootstrap` now constructs the
nine pinned platform shared users and valid OEM declarations, preserving
fixed IDs, system/privileged flags and contextual name/range/slot
rejections (#803). SystemConfig reads OEM UID tags from every scanned
partition, replaces repeated names and keeps ArrayMap's signed hash
ordering with stable collision ties. Malformed declarations retain their
image-relative file, raw attributes and rejection reason. An explicitly
run original-runtime
test compares the same disposable XMLs against original SystemConfig,
including replacement, missing/empty/invalid values, signed limits and
Arabic/fullwidth digits. Saved groups retained after the original scan
match the seeded IDs; unused initial groups are pruned by the original.
`Bootstrap::restore` now merges decoded settings with initial groups,
keeps constructor UID flags, aggregates active package public/private flags and
loads saved signatures. Disabled package flags do not contribute to the active
group. `SharedUser::add_package` and `remove_package` port SharedUserSetting's
membership rules: duplicate additions refresh the canonical setting's current
flags without OR-ing again, removals recompute only when the removed member's
current bits overlap the aggregate, and constructor flags survive the last
removal. Signing reconciliation stages membership/flags with the group and commits
them only after signature and library checks succeed. A disposable original-PMS
runtime compares 24 add/update/remove sequences, including duplicate/absent
removals and stale aggregate bits, with every state/result identical (2026-10-03,
49.73s). Constructor/update/library/manifest/time checks, Settings launch and
owned cleanup also pass; Java original API linkage passes (13.6s). All eight
explicit original-APK scan tests pass (5.68s), including accepted new-member flags
and rejected-candidate group rollback. Membership removal is implemented at the
owner; complete removal/replacement pipeline wiring and SELinux/process state
remain separate work under #803/#702. Its post-reconciliation `prune_unused` keeps groups referenced by active or
disabled packages and removes empty groups through the UID owner, retaining
the runtime allocation cursor. An explicitly run disposable original-PMS
boot (2026-10-02, boot completed) restores all 243 active package IDs and,
after pruning, matches all 16 original saved group names, IDs and signing
records. Unit tests cover seed flags, shared membership, disabled-only
members, pruning, snapshot/input immutability and deletion cursor behavior;
all 181 package unit tests pass. Conflicting decoded settings reject the
candidate with context; the original reader's ordered recovery from corrupt
raw records remains pending (#803). `scan::UidScan` now prepares UID
ownership from fully parsed and verified system-directory Code inputs.
Saved names retain their IDs and manifest groups; duplicate image names
reuse the same identity. New packages join an existing manifest group or
create it with Settings' zero initial flags, while new leaving packages
receive independent UIDs. Static-library names use the selected version
suffix. `Bootstrap::get_shared_user` preserves existing groups/flags and
creates a new group only after UID allocation succeeds. Lookup without
creation, exhausted allocation, older snapshots and rejected transitions
remain unchanged. The UID-only preparation still rejects disabled/original
adoption and changed saved groups; original-setting adoption now has a
separate signer-reconciled path below (#804). All 181 package unit tests, eight
explicit original-APK scan tests and eight original-image DEX policy tests
pass. The DEX policy test pins group creation and package registration
branches, including the original insufficient-storage error and cleanup
and empty-group removal guards. UID preparations now track pending members.
After reconciliation, accept_uid retains their ownership; reject_pending
removes only a rejected independently allocated package slot and advances
the original allocation cursor. A rejected shared member loses its candidate
membership while its group slot survives until final pruning. Restored and
accepted identities remain intact; a changed slot owner rejects cleanup.
Final pruning requires all preparations resolved and retains accepted active
or saved disabled members. Six explicit native scan tests include verified
GSF input rejected, pruned and retried under the next UID. All 181 package
unit tests and six original-image DEX policy tests pass.
`UidScan::new_setting` now constructs new PackageSetting candidates only
for pending preparations with matching canonical slot ownership. The pinned
constructor's code/ABI/library/MIME/domain metadata, unassigned keyset (-1),
category (-1), unknown signing details and initial loading state are retained.
User policy distinguishes a null target, USER_ALL and an explicit user;
pre-created and debugging-restricted users are excluded only for USER_ALL.
Ordinary apps get explicit stopped/not-launched states when installation is
allowed and UserManager has initialized; system apps only get explicit stopped
state under that scan policy. A disposable original-PMS boot (2026-10-03,
boot completed) compares 64 constructor combinations with original
Settings.createNewSetting, including independent/shared UIDs, system flags,
stopped policy and null/explicit/all install targets with uninitialized
UserManager. All results match; Settings launches and owned processes/data
are cleaned. The same test compares 16 Settings.updatePackageSetting
combinations: old/new system flags, old required-for-system-user bit and
changed/unchanged paths, with UserManager uninitialized. UID/shared state,
public/private flags, retained/replaced legacy native paths, ABI, version,
installation/uninstall reason and MIME names match. Explicit-user-list
promotion, unchanged stopped state and retained MIME values are covered by
unit tests. Java API linkage verification and all 181 package unit tests pass.
`SigningScan::apply_existing` now copies a saved setting, validates unchanged
canonical UID/shared ownership and native parsed identity, applies the typed
initial-scan setting update, then reconciles signatures and library declarations
before accepting metadata/user-state candidates. Only the system public flag
is changed in this Settings phase; the original required-for-system-user bit
is retained. Non-system packages promoted onto a new system path with no
disabled original become installed for enumerated users with unknown uninstall
reason. System legacy native paths stay unchanged; changed non-system paths
prefer the incoming legacy path. A verified-signer scan test rejects an
unrelated data update without any candidate mutation and accepts the matching
update while retaining UID, user state and existing MIME values. All eight
explicit scan tests and eight original-image DEX policy tests pass; the latter
pin Settings.updatePackageSetting and PackageSetting.updateMimeGroups.
Native `ScanPolicy` now applies the pinned scan's manifest/component restrictions:
system direct-boot propagation, non-system broadcast/core/persistent/storage and
permission-group restrictions, single-user export policy, partition/APEX flags,
and current platform signer comparison. Physical locations supply initial flags;
updated data packages inherit the factory setting after its system scan, not
the incomplete flags restored from disabled XML. Shared-UID privilege adjustment
uses the group owner's flags and honors platform-key, leaving-member and pre-P
vendor exceptions. The selected platform package is optional: initial overlay
directories scan before framework-res. Compressed stub detection reads the real
`-Stub` sibling directory and rejects inaccessible inputs before mutation.
`application_flags` reuses native ApplicationInfo generation with the owner's
updated-system bit; it does not copy saved public/private flags. The GSF accepted
candidate now receives these calculated flags. A disposable original-runtime
oracle compares four GSF policy combinations through original applyPolicy
(2026-10-03, 48.08s), covering booleans, component main state, groups, broadcasts
and original/adopt declarations. Unit tests cover policy boundaries, current
multi-signer equality versus lineage, shared-UID exceptions and compressed
inventory; unreadable-input rollback and Java API linkage (13.9s) pass.
The full saved-record diagnostic compares calculated public/private flags
against all 243 original saved values (146.49s), including APEX packages,
pre-framework overlays and updates inheriting refreshed factory partition flags.
It also preserves the earlier timestamp/signature/group/library and stopped-data
checks; these flag calculations are diagnostic, not native service activation.
`LibraryCompatibility` now ports the original ordered library updaters (#808):
wear/IKE/maps removal, pre-P HTTP, legacy system HIDL, runner/mock/base dependency
ordering and the final SystemConfig/APEX rules. Required/optional presence is
retained without promotion. SystemConfig records insertion order, including
replacement without reinsertion, so stable Java-hash sorting preserves ArrayMap
collision order. Checked UnboundedSdkLevel rejects invalid image policy; this
scan path does not swallow the errors as SystemConfig's XML filter does.
The owner supplies the selected test.base boot-classpath policy and, for
non-system apps when test.base is separate, resolved PlatformCompat change
133396946. Missing decisions reject unchanged. `ScanPolicy::apply` stages both
manifest and library policy atomically before reconciliation; the accepted GSF
candidate now uses that combined path. A disposable original-runtime oracle
compares 48 SDK/system/update/required/optional combinations against original
modifySharedLibraries using actual SystemConfig, reported boot-classpath policy
and original PlatformCompat decisions (48.08s). Lists and order match, test.base
is separate on this image, and the unresolved-decision rollback check passes.
The existing constructor/update/time/manifest checks, Settings launch and owned
cleanup also pass. Unit coverage includes SDK thresholds, optional dependencies,
hash-collision order and invalid input rollback. The generated system-UID-only
IBridge exposes the original PackageBackwardCompatibility build policy.
`ScanPolicy::apply_from_platform_compat` calls the original `platform_compat`
Binder directly with generated pinned `IPlatformCompat.isChangeEnabled` and a
native-generated typed ApplicationInfo. SystemServer registers that owner before
PMS bootstrap; no additional Java compatibility-query relay is needed.
Native `from_bridge`/`apply_from_platform_compat` preserve owner exceptions and
transport errors and stage manifest/library changes atomically. An in-process
Binder test checks false/true, owner rejection and transport failure. The original
runtime processes four complete native-generated AIDL requests through its real
compatibility Binder; native reply decoding agrees with its own PackageImpl-based
SDK-dependent decisions. The derived image builds (45.5s, including post-restart
original attachment), Java API linkage passes (14.6s), and the disposable
original-PMS runtime checks pass (52.19s, 2026-10-03), including 48 library cases,
four manifest cases, constructor/update/time checks, Settings launch and owned
cleanup. All 181 package unit tests pass (11.34s). The runtime uses a guest sender
for the native-generated request bytes; native-host Strong transport is covered
by the in-process Binder test. The test.base build-policy bridge still attaches
at DEVICE_SPECIFIC_SERVICES_READY, after the original PMS initial scan: early
build-policy delivery and native boot-scan wiring remain #808. ABI derivation,
group membership lifecycle wiring and the complete boot scan selector/publication
remain; no native-PMS CTS activation is claimed.

`NativeLibraryPaths` derives paths after ABI selection from the code/base path,
actual code-directory status, preferred image ABI and installation library root
(#810). It ports monolithic/cluster, system/updated-system, partition/APEX,
primary/secondary ISA and legacy app-lib rules; non-bundled monolithic paths do
not consult the ABI. An unrecognized bundled root requires a canonical guest
source from the filesystem owner; absent/invalid inputs reject before package
mutation. `apply` writes the validated paths to the native parsed package.
A disposable original-PMS runtime compares 72 path/system/update/selected-ABI
combinations with original PackageAbiHelperImpl, then checks the actual GSF paths
and propagates the calculated root into its accepted setting candidate. Existing
24 shared-UID sequences, 48 library cases, manifest/constructor/update/time checks,
Settings launch and owned cleanup pass too (2026-10-03, 74.03s). Java original API
linkage passes (13.7s), all 183 package unit tests pass (11.55s), and focused path
coverage checks partition roots, unknown-root/invalid-ABI rejection and secondary
paths. `BundledAbis` now selects primary/secondary ABIs from the selected image's
unpacked library inventory and ordered 32/64-bit ABI lists, preserving the
preferred ISA and reporting both-ABI/non-multiarch packages without discarding
either ABI. Missing/unreadable inventory inputs reject before mutation. The
same runtime compares four image apps with real arm64 libraries and an original
app removed by the overlay against original PackageAbiHelperImpl, then derives
GSF ABIs and paths before accepting its setting metadata. Native inventory reads
the actual derived image used by that boot, not the original image whose removed
libraries differ. Unit coverage also checks monolithic/cluster, 32/64/both/neither,
preference, multiarch and error branches. Extraction/alignment,
shared-UID lifecycle integration and complete boot-scan wiring remain #810/#702;
native PMS is not activated.

`SharedUserAbi` ports original primary-ABI selection and application separately:
the scanned package takes precedence, updates skip their retained setting,
otherwise the first ABI-bearing member in owner-supplied ArraySet order selects
the ABI. ISA conflicts retain package/required-owner diagnostics rather than
discarding an ABI. Application updates only ABI-less settings and the scanned
package, preserves secondary ABIs and existing parsed members, and returns changed
code paths for subsequent dex/installation work. A disposable original-PMS boot
compares selection, settings, scanned ABI, retained parsed ABIs and changed paths
with original PackageAbiHelperImpl and ScanPackageUtils in 60 cases, including
updates, ISA aliases/conflicts, absent/loaded parsed members and no required ABI
(2026-10-03, 72.67s). All prior runtime checks, Settings launch and owned cleanup
pass; 185 package unit tests pass (2.99s), focused ABI tests pass, and Java image
API linkage passes (13.2s). These helpers do not establish the full boot owner's
membership order, run dex/installation side effects or activate native PMS;
boot and install lifecycle integration remains #810/#702.

`ZipNativeLibraries` reads base and split APK inventories through the native
filesystem owner and ports original NativeLibraryHelper.findSupportedAbi:
the best supported-list index across APKs wins, with distinct no-native-code and
no-matching-ABI results. Native candidates follow ApkParsing's path length, safe
filename and no-subdirectory rules, including non-.so files; the JNI's pinned
RenderScript filename rule is preserved. Malformed ZIPs, NUL entry names and
unreadable/unmapped code reject explicitly. A disposable original runtime
compares 17 newly generated ZIP archive groups (four ABI lists, split unions,
safe/unsafe/nested/long names, RenderScript and NUL rejection), then compares
actual unchanged APK inventories of four image apps with original
NativeLibraryHelper. All prior runtime cases, Settings launch and owned cleanup
pass (2026-10-03, 78.16s); 187 package unit tests pass (3.04s) and Java image API
linkage passes (16.0s). This owns inventory matching; extraction/alignment and
boot/install side effects remain #810/#702. Original PMS still runs.

`AbiPolicy` reads ordered supported ABIs, the image's multiarch-match aconfig
flag and reported native-bridge ISA properties; absent image policy rejects.
`PackageAbis` ports multiarch 32/64 selection and preference, SDK-35 native-only
matching, override semantics, RenderScript's 32-bit requirement and native
shared-library rejection. Rejections preserve original install codes/messages.
`Apks::native_library_scan` stages this selection, uses bundled inventory only
for a non-updated system package with no primary ABI, and calculates final
native paths without mutating an erroneous input. It reports required extraction;
`apply_metadata` commits metadata after the side-effect owner has completed
required work. A disposable original runtime matches ABI decisions and full
error codes/messages in 240 combinations of ZIP inputs, multiarch, preference,
SDK, overrides and shared libraries. The combined factory derivation agrees with
original helper calls on four unchanged image apps; GSF's ZIP decision and
combined metadata feed the accepted setting. Disappearing APKs reject unchanged.
All prior runtime checks, Settings launch and owned cleanup pass (2026-10-03,
80.20s); 190 package units pass (3.45s), including native-bridge filtering,
and Java image API linkage passes (16.3s). Extraction/alignment,
full boot selection/publication and install side effects remain
#810/#702; native PMS is not activated.

`AbiScanContext` now selects ABI lifecycle inputs: normal rescans reuse retained
ABIs, first boot/upgrades, prior stubs and missing settings rederive; installs
retain compilation ABIs and moves reuse settings. APEX libraries stay with
apexd. The validated platform package uses the VM owner's primary ABI after path
calculation, retaining secondary ABI and path ordering. Setting enrichment writes
primary/secondary ABI, normalized override (`-` clears it) and legacy root.
`SigningScan::finish_native_library_metadata` validates accepted identity/UID/
signer/setting state, stages the lifecycle result and commits parsed/setting ABI
metadata together. Required extraction rejects until its owner has completed it;
typed selection/input errors and multiarch diagnostics reach the caller. The
disposable original runtime compares 96 lifecycle combinations with original
helper/setter routes (2026-10-03, 80.44s), including system/update origins, clear
override, saved/stub/missing/move/compiled/APEX and platform-ABI ordering. Native
reuse/APEX cases use an unreadable inventory mapper, proving they do not reopen
APKs. Accepted GSF metadata receives the requested arm64 ABI and override through
the signing owner; unreadable code and stale candidates preserve owner state.
All earlier runtime checks, Settings launch and owned cleanup pass; 192 package
units pass (3.00s), and Java image API linkage passes (13.5s). The runtime uses
original ABI/path/setter APIs, not the complete original ScanPackageUtils entry
point. Full boot/install selection, extraction/alignment, membership effects and
snapshot publication remain #810/#702; native PMS is not activated.

`NativeLibraryInstallPolicy` now inspects selected-ABI ZIP entries using the
guest page size and original NativeLibraryHelper admission rules (2026-10-03).
Direct APK mapping requires stored, page-aligned entries; 16 KiB compatibility
can extract 4 KiB-aligned entries unless disabled by the device or manifest.
Debuggable `wrap.sh` always requires extraction. Local-header corruption,
encrypted entries, inconsistent stored lengths and NUL names reject.
`Apks::native_library_install_plan` checks all base/split APKs before returning
their extraction decisions. `NativeLibraryInstallPolicy::copy_to` now extracts
selected entries into the caller's checked native directory: size/CRC/mtime
matches reuse the existing regular file; changed files are decoded and CRC
checked into an exclusive temporary file, synced, assigned ZIP modification
time, 0755 mode and guest ownership, then renamed. Failures preserve the old
file and remove temporary output; installation error codes remain typed.
DOS time conversion requires the guest clock owner, avoiding host timezone
assumptions. APKs remain read-only. Package units pass (201, 2.98s), including
corrupt-payload rollback, metadata and reuse. The disposable original runtime
matches admission errors, extraction content/mtime and repeated-copy reuse in
48 combinations at 16 KiB guest pages (2026-10-03, 83.74s); all earlier checks,
Settings launch and owned cleanup pass. Java image API linkage passes (14.2s).
`Apks::native_library_alignment` selects the original supported 64-bit ABI over
the base/split union, applies the root's ISA rule, and combines pinned ZIP/ELF
page-compatibility flags. It inspects APK offsets when extraction is disabled
and extracted files otherwise; compressed direct entries, unreadable/truncated
headers and offset overflow return errors. Non-64-bit ELF classes and non-LOAD
segments follow the original diagnostic's rules; this is not loader validation.
The same runtime matches 60 alignment cases, including split aggregation and
error precedence, missing files, malformed program headers and ISA paths.
`PageSizeCompatPolicy` reads the required image aconfig input, applies the
16 KiB gate, gives positive manifest flags precedence and excludes
system/APEX/platform or 32-bit-only scans from alignment inspection. Diagnostic
errors preserve saved flags and reach the caller. `Package::set_page_size_compat`
matches the original OR and settings-override toggle semantics with checked
0..127 values; settings XML reads use the same check. The original setter
matches 910 seed/mode cases, including invalid values and full error messages.
`SigningScan::finish_page_size_metadata` stages this policy after accepted
identity/UID/signer validation and commits setting flags; invalid values and
stale candidates reject. The runtime exercises accepted GSF metadata with the
actual image policy; unit tests cover exclusions, failure preservation and
invalid persisted settings. This does not exercise the complete original
ScanPackageUtils entry point. Guest-clock delivery, complete base/split and
multiarch copy orchestration, boot/install policy delivery and publication remain
#810/#702; native PMS is not activated.

`SigningScan::finish_metadata` now completes timestamp/version/volume metadata
for a current accepted setting candidate (2026-10-03). It checks the retained
setting, identity, UID, signer and leaving status before mutation; stale or
unreconciled candidates reject unchanged. The scan clock and verified-code
owner supply timestamps. `Apks::scan_file_time` now reads the monolithic
code path's Unix modification time or the maximum base/split APK time for a
cluster, in milliseconds; it does not use the cluster directory's timestamp.
`finish_code_metadata` requires matching parsed/accepted code paths, reads that
input and finishes metadata with ScanClock policy. Unreadable/missing code
returns an explicit error without changing the candidate; full reconciliation
still owns code removal. A disposable original-PMS boot (2026-10-03, 48.08s)
loads the native GSF parser entry through original PackageCacher and invokes
original getLastModifiedTime: its actual file time matches native output and
becomes the accepted first-install/last-update/last-modified metadata. The test
also checks unreadable-input rollback, Settings Status: ok and owned cleanup.
A filesystem unit test checks monolithic/pre-epoch fractional times, cluster
maxima, ignored directory timestamps and missing/null inputs. Java API linkage
passes (13.9s), and all 181 package unit tests pass. These original timestamp
comparisons cover available code, not removed-code lifecycle behavior.
Earliest first-install time excludes zero and follows
the original Long.MAX_VALUE sentinel. A zero current clock uses file time for
new installs, or updates last-update time only for changed system-directory
code; a nonzero clock honors the original explicit update-time policy.
USER_ALL modifies only existing explicit user states. Parsed long version,
volume UUID (including clearing it), debuggable/base revision and system
install-source orphan state are applied without changing UID or libraries.
Two unit tests cover clock/system/update branches, signed timestamp and
version boundaries, empty/all-user targets and unrelated user-state retention.
The verified-signer scan test rejects stale metadata without mutation and
finishes an accepted data update; all eight explicit scan tests pass (5.73s).
Eight original-image DEX policy tests pass (0.08s), now pinning inspected
ScanPackageUtils.scanPackageOnly and PackageStateUtils earliest-time code.
This is candidate metadata evidence, not an executed original scan/OTA or
native activation. Complete scan-policy/ABI integration, code/version selection, scan-clock
and boot-pipeline wiring, persistence, side effects and query publication remain
#804/#707/#798. Live DONT_KILL installs need retained
old-code-path lifecycle ownership (#807); no live-install support is claimed.
`SigningScan::apply_new_system` now connects new-system UID preparation,
setting construction, INSTALL shared-UID authorization, ordered lineage merge
and signer commit. Successful candidates return their signed settings, parsed
identity and initial user states. Failure leaves package/user state unchanged;
rejected independent slots are released with the original allocation cursor,
and a rejected new shared group remains allocated until final pruning. Eight
explicit original-APK scan integration tests pass (2026-10-02): real GSF and
framework code initialize their original declared groups, and labeled synthetic
policy/signer candidates check unrelated-member rejection, prior-record
preservation, first OTA replacement, API <=29 rejection versus newer fatal
mismatch, group pruning and retry IDs. Physical data paths reject before
allocation. New static libraries also pass the pinned declaration constraints:
target SDK, instant status, original names, dynamic libraries, shared UID,
components, permission declarations, attributions, protected broadcasts and
overlay target. `Registry::latest_static_setting` selects the greatest
nonnegative declaration version strictly below the incoming version and looks
up that declaration's package setting, without falling back to an older
setting when the selected one is absent. `SigningScan` now owns the declaration registry, initialized from SystemConfig
and extended only by accepted records. Its initial-scan `apply` uses prior
accepted declarations for this signer selection, retains the target's
UID/settings and selects the request's disabled original. `apply_with_disabled`
requires a matching verified disabled record for an updated-system dynamic
provider and admits only library names declared by that original. Existing
built-in/dynamic entries retain their declarations. Signatures, group markers
and the registry commit together after all fallible declaration work succeeds;
a failed new declaration publishes neither settings nor library state and
retains the original UID cleanup cursor.
A labeled synthetic static-library policy test uses verified original GSF and
framework signer material: data-origin mismatch rejects without mutation,
system-origin mismatch follows the explicit OTA diagnostic, rotation accepts
the real lineage's installed-data capability, revocation rejects, and unrelated
versions/disabled originals remain unchanged. The same explicit scan suite
now also builds the owned registry from accepted versions in order, checks
registration failure/retry UID cleanup, missing/mismatched disabled-record
rejection, updated-system declaration filtering and built-in preservation.
All eight explicit original-APK scan tests pass (2026-10-02), alongside six
original-image DEX policy tests and 181 package unit tests. These are scan
candidates; complete scan enrichment, owner inputs and registry dependency
resolution, removed-package reconciliation, full pipeline
failure/side-effect cleanup, persistence and query publication remain under
#702, #707, #803, #806 and #798.
`SigningScan::apply_original_system` now adopts an eligible unscanned
original system package (2026-10-03, #804): declarations are considered in
reverse order; non-system, already scanned and incompatible shared-UID
originals are skipped, and an existing authorized rename uses the saved
identity path instead. The constructor clones the original setting and
explicit supplied user states, preserves UID, MIME groups, install source
and key sets, and updates code/ABI/version/flags/domain metadata with unknown
initial package signatures as pinned Settings.createNewSetting does.
Signer/group and library reconciliation precede committing the copied
setting and renamed map. Missing user-state inputs and declaration failures
leave the entire candidate unchanged; a normally scanned shared member
establishes the original -104 rejection for an unrelated adoption signer.
The eight explicit scan tests pass with verified original GSF/framework
signers and labeled synthetic adoption declarations/metadata; seven
original-image DEX policy tests pin selection and construction, and 164
package unit tests pass. APKs are unchanged. This does not demonstrate a
real original-package OTA or activate native PMS: complete image/data scan
selection, ownership replacement, disabled-setting construction, transfer
side effects, user-state publication and persistence remain #804/#707/#798.
Saved signing reconciliation and UID conversion persistence are verified below.
`scan::Identity` selects manifest/internal/real names with the pinned
static-library version suffix and declared system-package rename rules
(#804). Persisted scan inputs reject a saved name different from the
selected identity; full APK signature validity alone does not authorize
that identity or its UID. An explicit disposable original-PMS boot
(2026-10-02, boot completed) supplies the actual display density and scans
all 243 active APKs plus disabled originals through native parsing and
full signature verification. Every active selected internal name and UID
matches the original saved state, including static library names distinct
from their manifest names. Original-image tests cover the mismatched-name
failure without mutating settings. After signature verification,
`Identity::apply` now changes the parsed package's internal name and the
owning package of all seven top-level component kinds as original
PackageImpl.setPackageName does. An explicit original-runtime fixture
populates the original ComponentName cache before renaming and compares
all resulting package/component identities, unchanged manifest/class names
and main-component process names with native records. Java linkage and
all explicit scan/runtime tests pass; all 181 package unit tests
pass. Every active parsed package now carries its selected internal name.
`sign::History` now compares verified and persisted certificate histories
with the pinned SigningDetails capability, ancestor and exact multi-signer
rules. Its existing-package gate accepts installed-data capability,
reverse rollback capability or an owner-authorized ancestor rollback.
An explicit original-runtime matrix compares 486 combinations of unknown,
single/multiple signers, ordering, granted/revoked lineage capabilities and
rollback direction; every result matches original SigningDetails.
On the disposable original-PMS boot (2026-10-02, boot completed), all 243
fully verified active APKs pass the normal existing-package signing gate
against their saved certificates. All 181 package unit tests and Java
linkage verification pass. The shared UID join gate distinguishes new
installs, updates and system scans, including lineage capability revocation
and every existing member. An explicit original-runtime matrix of 2,187
candidate/group/member/join-type combinations matches
PackageManagerServiceUtils; every active shared-UID APK on the same boot
passes the update join gate against its saved group. `has_common_ancestor`
rejects overlapping histories that diverge before a shared signer; all 144
pairs of 12 histories match original SigningDetails, including shortened
histories, equal signers with different ancestors and unknown/multiple
signers. `scan::Inputs::load` now applies the normal saved-package gate,
known disabled-system signature gate and saved shared-UID membership/
divergence gates after APK integrity verification. All 243 active APKs on
the disposable original-PMS boot pass this connected path. Explicit
original-image tests reject a fully signed GSF APK against unrelated saved
package and disabled-system certificates without mutating settings. These
are normal signature gates. `SigningDetails::merge_lineage_with` now
merges partial histories with self/other/restricted capability rules and
preserves the original unchanged-instance signal. The shared UID owner
merges an authorized candidate with other parsed members using restricted
capabilities only when the first merge changed the group. An explicit
original-runtime matrix with real DER certificates matches 507 two-history
merges and 2,197 group/candidate/member merges, including capabilities,
scheme versions, key counts and the changed signal. All 16 restored groups
retain their saved signing records when the verified active members are
merged. Invalid saved certificates reject the merge without changing the
group. The shared UID owner now carries the original per-scan
`signaturesChanged` state. Normal reconciliation initializes it to false;
commit initializes unknown group signatures without overwriting existing
signers. A physical-system signature failure can establish a new group
signer only before the first check, or after passing the SYSTEM join rule;
/data code is rejected. Inconsistent later system members produce a
reconcile rejection at first API <=29 and a fatal system error above 29.
An explicit original-image test pins the inspected ReconcilePackageUtils
and Settings commit DEX branches, including that API boundary and null
signer initialization. Three explicit native scan tests pass; real GSF
and platform certificates exercise initialization, first/later OTA cases,
/data rejection, snapshots and error atomicity. Scan records retain their
physical origin independently of saved FLAG_SYSTEM. `scan::SigningScan`
now wires normal authorization, shared lineage merging, OTA state and
initial signer commit for already-saved APK identities in caller-supplied
order. Before signature reconciliation it applies the pinned manifest shared
UID selector: an existing member retains its declared group while leaving,
and an already independent package ignores a leaving declaration. A changed
or removed group requires replacement UID allocation and is rejected by this
saved-identity phase (#804), including system-directory records. Its
candidate settings retain UID ownership and metadata; each
record finishes all fallible work before updating package/group signatures,
and later rejection or fatal mismatch leaves prior candidate commits
intact. `Inputs::load_verified_code` provides integrity-verified records
for this phase without granting saved signer/UID authorization; `load`
retains the normal-gate diagnostic. Six explicit native scan tests pass,
including sequential initial/OTA cases and rejection of /data replacement,
tampered origin and UID changes, changed/missing manifest groups, and
retained versus already-left shared UID declarations. The signer-mismatch
sequence uses an explicit synthetic signer candidate with an unchanged
manifest. On a disposable original-PMS boot, all
243 active saved APKs pass SigningScan in supplied persisted-record order;
all package metadata and 16 saved group signatures/UIDs remain unchanged,
with verified serialized keys supplied for package records. This is a
candidate signing phase, not the complete image/data scan order or query
snapshot publication. Candidate shared UID migration now implements the
pinned single-user conversion: an accepted active member must be leaving,
and at most one disabled version may remain, itself parsed and leaving.
With the image's BEST_EFFORT policy selected explicitly, both versions
become independent under the same app ID, the group is removed, and the
UID-slot owner becomes the package without moving the allocation cursor.
NEW_INSTALL_ONLY remains the default and leaves existing groups intact.
Six explicit scan tests cover unparsed/non-leaving members, absent or
non-leaving disabled code, multiple members, policy selection, unchanged
metadata, snapshots and preserved allocation. An original-image DEX test
pins the three inspected eligibility/conversion methods. `Store::commit_shared_uid_migrations` now persists owner-authorized
single-member conversions together with signing state. It switches active
and disabled records from sharedUserId to the same numeric userId, removes
the group, and rebuilds certificate definitions/references after removal.
Unrelated XML nodes and package metadata survive; partial conversions,
remapped IDs, empty/multiple-member group deletion and cleared retained
signers reject before writing. All 181 package unit tests pass. A real
signed APK fixture is installed and updated by original PMS, native-parsed
and integrity-verified, reconciled and migrated under BEST_EFFORT, then
written while that disposable original PMS is stopped. On reboot original
PMS retains the migrated UID (10213 in the recorded run), all 244 package
UID/signature records and the other 16 groups; Settings launches. Main and
reserve bytes match and owned data/process cleanup completes. The signed fixture now requests READ_CONTACTS and READ_CALENDAR. Original
permissionmgr grants contacts with USER_SET and denies calendar with
USER_SET/USER_FIXED; original AppOps sets RUN_IN_BACKGROUND to ignore.
The test waits for those exact flags/mode in AccessPersistence before
stopping its owner. Native package persistence leaves access.abx bytes and
its decoded state unchanged. Original reboot preserves the complete fixture
app ID permission flags and AppOps modes, and original permissionmgr and
AppOps queries confirm the grant, fixed denial and ignore mode. These
Android 16 states remain attached to the same numeric app ID during the
conversion. Live permission-owner notifications/package feed, publication,
complete native boot and CTS remain under #803, #798 and #702. `Store::commit_signatures` now writes the candidate
package/group certificates and capabilities to the retained packages.xml
document as ABX with the existing backup/reserve and system inode protocol.
Its document-wide certificate table defines each encoding once and
references subsequent occurrences by index, including lineage entries.
Unrelated package attributes/children and other root nodes survive;
serialized public keys remain in the scan snapshot because packages.xml
persists certificates. The writer rejects unrelated metadata changes,
cleared retained signing identities, duplicate/unmodelled owners and
external document changes. All 181 package unit tests pass. The disposable
original-PMS runtime test wrote all 243 active package and 16 group signing
records into its separate native-owned fixture and re-read them with
complete persisted settings parity; main/reserve bytes match. Original
PMS still owns its live mounted data in that test. The same native file
is now parsed by original PackageSignatures.readXml: all 259 signature
owners match in certificate bytes, scheme versions, lineage capabilities
and derived key counts. An explicit two-boot test on a second disposable
data image stops original PMS, mounts only its owned stopped volume,
writes native signature persistence, detaches and restarts original PMS.
Both boots complete; all 243 package and 16 group signing/UID records
survive, no signature XML read error is logged, and Settings launches
with `Status: ok`. This verifies original-reader and reboot compatibility
for signature persistence. Full package-state persistence/publication
remains pending (#798). New/removed package reconciliation, full boot
wiring and an actual OTA boot remain pending (#702, #803, #805). Legacy
certificate compatibility/
recovery, upgrade keysets, owner-authorized rollback, full UID
reconciliation and native snapshot publication remain unimplemented
(#804). This does not activate native PMS.
The pinned Settings DEX compiles out `disallowSdkLibsToBeApps`: SDK
libraries require a positive app/shared-user ID, and native restoration
rejects missing identities (#802). An explicit original-image test checks
the complete inspected reader/registration control flow; a settings test
covers SDK/non-SDK negative IDs, zero IDs and positive/shared ownership.

The device's own system service (`dev.aim.server.DeviceServices`,
docs/system-services.md, "The system_server bridge") is on the system
server class path (`/system/framework/aim-services.jar`, compiled by the
`oat` node) and named by the static overlay
`/vendor/overlay/aim-framework-overlay.apk`. SystemServer starts it in
`StartDeviceSpecificServices`, and at
`PHASE_DEVICE_SPECIFIC_SERVICES_READY` it hands guest-init's service
host (`aim.service_host`) the bridge: guest-init logs "system_server's
bridge attached". Its second boot of a data directory reached
`sys.boot_completed` in 12 s (load 8), and Settings started cold in 231 ms.

## Compiled oat files (2026-09-30)

The boot image extension holds all 31 mainline BCP jars, as the original's,
and the 29 oat files with code (system_server's class path,
`org.apache.http.legacy`, six `speed` apps) are compiled for it by the `oat`
node (docs/art-exception-patches.md, "Other oat files"); the image's
dexoptanalyzer finds them usable, the originals not. The 139 `verify` odex
files are still rejected ("Read barrier state mismatch": no CMC without
userfaultfd, #442) and their vdex used. On four fresh-data smoke boots at
load 4-17 rising to 40-60 (two before, two after), `sys.boot_completed`
came at 50-51 s either way and `boot_progress_enable_screen` at 36.5-40.7 s
before, 36.8-37.3 s after; a cold Settings start took 10.5 s and 33.5 s
before, 45 s and 19.3 s after, dominated by ANRs of com.android.phone and
GMS in all four (gone since; see "First boot").

## First boot (2026-09-30, #241, #375)

Two fresh first boots of main (49eed57f, host load 9.5) and of the device
without telephony (load 3.9), measured from guest-init's start to
`sys.boot_completed` and the 5 minutes after, CPU per guest process from
the host every 2 s:

| Check | Before | Without telephony |
| --- | --- | --- |
| `sys.boot_completed` (1 s polling, within noise) | 14 s | 16 s |
| `am_anr`, `am_crash` | 0, 0 | 0, 0 |
| RILJ log lines (phone process) | 180 | 0 |
| lmkd kills (host memory pressure, #300) | 25 | 0 |
| Guest CPU: boot, 0-2 min, 2-5 min | 17.5, 191, 10.4 s | 20.4, 167, 12.3 s |
| Settings cold start | 278 ms | 253 ms |

The ANRs of com.android.phone and GMS persistent ("failed to complete
startup") no longer happen. The device declares no telephony (the
emulator's `handheld_core_hardware.xml` without it, `image/overlay.toml`),
so the phone process no longer builds a GSM phone and RIL for a modem it
does not have. The first two minutes are Android's and Google's own work:
Play Store and GMS update themselves from the network 40-70 s after boot
(`installPackageLI` stops them and their clients), dex2oat compiles the
updates (about 28 s of CPU), GMS and the Google apps start (about 90 s),
system_server takes 18-21 s. guest-init takes 14-19 s: it hosts the binder
driver, whose transport cost is #451. After two minutes the guest is
nearly idle. The kills of cached processes in the first run came from
lmkd reacting to the Mac's memory pressure, not from the guest.

## Other processes in /proc (2026-09-30, #238, #379)

Each process keeps a record beside its by-pid entry
(`docs/guest-init-contract.md` section 4): its threads, and the stack
pages of its argument strings, mapped from the record. Another
process's `cmdline`, `comm`, `task/` and per-thread `stat` read it.
One boot each of main (aeb32a54) and the branch in the main tree, a
fresh data image (main's boot was its first, the branch's its second),
host load 3-25:

| Check | main | branch |
| --- | --- | --- |
| `ps -A -o NAME` of zygote's children | 66 of 121 lines empty, `pidof system_server` empty | none empty; system_server, SystemUI, phone, Settings named |
| `/proc/<system_server>/task` | 1 entry | 224 threads; `top -H -p` lists them with TIME+ per thread |
| "Render thread does not belong to process" | 1 | 0 |
| logd host syscalls per log line, 30 s after `sys.boot_completed` | 9.0 (452,613 for 50,095 lines) | 1.2 (33,445 for 28,016) |
| the same over the next 30 s | 5.0 (29,896 for 5,980) | 1.0 (77 for 78) |
| Settings scroll, 10 swipes: frames, janky, p50/p90 | 419, 0.72 %, 8/9 ms | 417, 0.24 %, 8/9 ms |

## Guest kernel time (2026-09-30, #446)

On a settled boot of a reused data image, main (80965c99) against the
fixes of #446, measured 7 minutes after `sys.boot_completed`, then over
the next 60 s of idle:

| Check | Before | After |
| --- | --- | --- |
| Guest CPU 7 min after boot | 77 s, 48 % system (GNSS HAL 20 s) | 53 s, 51 % system |
| Faults in that time | 1.22 M | 0.93 M |
| Guest CPU per idle minute | 1.4-6.8 s, 33-49 % system | 1.6 s, 46 % system |
| Cold start: Settings, Calculator, Chrome | 0.23, 0.30-0.33, 0.31 s | 0.21, 0.30-0.32, 0.30 s |

The GNSS HAL's CoreLocation thread no longer spins while updates are on.
Content files of `/proc`, `/sys` and selinuxfs are reused, ART's JIT
memfd is no longer copied, and ashmem and `faccessat` make fewer host
calls. A credential change rewrites the process's by-pid entry in place
(`PR_CAPBSET_DROP` 175-357 us to 4.5-4.9 us, #449), and a private
mapping of a plain file maps the file copy-on-write instead of copying
it (4 MiB: 309-344 us to 12 us, #450).

Where the remaining kernel time went before the fixes below, from a
per-syscall profile of host user and kernel time (a temporary build; M2
Pro, host load 7-16, a reused data image, boot to 4 minutes after
`sys.boot_completed`), largest first:

| Item | Kernel time | Per call | Owner |
| --- | --- | --- | --- |
| page faults and other time outside syscalls | 5.4 s of 23.5 s | | Chrome alone 1.75 s in its start (#500) |
| openat | 3.8 s | 81 µs | host open under the security agent (#418); logd reading `/proc/<pid>/cmdline` of zygote's children, empty to it, on every log line (#238, fixed since: see "Other processes in /proc") |
| BINDER_WRITE_READ | 2.5 s | 14 µs, 7 µs of it the Mach round trip | #451 |
| mmap of files | 1.9 s | 40 µs small, 1.5-5 ms at 16 MiB and more | ART's JIT memfd and private memfd mappings, fixed below (#501) |
| faccessat, madvise(DONTNEED) | 0.9 s each | 25 µs, 5.5 µs | each app re-reading owner attributes zygote had read, fixed below (#505); #502 |
| membarrier, ashmem PIN | 0.5 s, 0.4 s | 140 µs, 16 µs | #503, #504 |
| fork (zygote's clone) | 0.5 s | 10 ms | #421 |

A binder read reserves placeholder fds for the files a transaction may
carry. They were opens of `/dev/null`, and installing a file over one
closed a vnode: about 40 µs of kernel time per file received. They are
sockets now (docs/binder-driver.md, "Fd transport"), and a
BINDER_WRITE_READ went from 14.8 to 8.7 µs of kernel time in the caller
up to `sys.boot_completed`, and from 25.8 to 10.0 µs while Settings
started twice and Chrome once (0.98 to 0.33 s in all; one boot each). A process no longer sweeps the memfd
directory at its first memfd_create (2.2 ms with 48 memfds alive; 49
calls in a boot averaged 2.9 ms).

A binder read that waits for work no longer blocks a thread of the
binder daemon: it parks, and the daemon thread that brings the work
answers it (docs/binder-driver.md, "Driver–process path"). A call
crosses four Mach hops and no wake inside the daemon, and the driver's
maps no longer hash with SipHash. On a settled boot (one each, host load
10 and 6), the synchronous calls of a Settings and a Chrome cold start
went from p50 61 to 51 µs, and guest-init's CPU over the starts from
0.74 to 0.60 s. What is left in the caller is one Mach round trip per
ioctl, about 7 µs of kernel time in a boot (#451). Since no daemon thread
blocks, one per CPU serves every guest binder thread (#553): guest-init
has 33 threads after a boot and a Settings and a Chrome start, where it
had one per guest binder thread.

membarrier now interrupts only the guest threads that are running, as
Linux does, instead of every thread of the task (host unit benchmark, 60
parked and 2 spinning threads: 115 to 58 µs of kernel time per call,
#503). ashmem PIN and UNPIN skip reading the region's attribute while
the file's ctime shows nobody changed it (an UNPIN+PIN pair: 32-39 to
19-21 µs, #504). Neither is measured in a boot yet.

The large file mappings were ART's JIT cache: its memfd's first
executable view looked for the memfd's other views by walking the whole
VM map, 3.5-4.8 ms of kernel time in a process with 3,000 mappings. The
process now records where it maps a memfd (48-50 µs). A private mapping
of a memfd maps it copy-on-write instead of copying it (16 MiB: 1.6 to
0.16 ms). A fork child starts with its parent's owner attributes (an
app with zygote's; still checked against each inode's ctime), so its
first stat of a file zygote knew costs 2.6 µs instead of 20 µs (#478,
#505). On a reused data image, one boot each (host load about 12),
before and after:

| Check | Before | After |
| --- | --- | --- |
| All guest processes at `sys.boot_completed` + 60 s: CPU, system | 48.2 s, 23.3 s | 36.9 s, 16.3 s |
| The same after the cold starts below | 55.6 s, 26.0 s | 42.8 s, 18.6 s |
| Settings cold start (3): time, the app's system CPU | 206-234 ms, 0.11-0.13 s | 177-199 ms, 0.09-0.10 s |
| Calculator cold start: time, system CPU | 331 ms, 0.17 s | 296 ms, 0.13 s |

## Vsync off at idle (2026-09-30, #452)

SurfaceFlinger kept hardware vsync on for good, because it ignored
present fences (docs/composer.md, "Vsync"): at 120 Hz, the composer and
SurfaceFlinger handled every vsync while nothing changed. Present fences
now signal at the vsync that shows the frame, SurfaceFlinger predicts
vsync from them, and aim-display runs its display link only while vsync
is enabled. On a settled boot of a reused data image (host load 5-7), one
idle minute, before (c0a2b9b7) and after:

| Check | Before | After |
| --- | --- | --- |
| Composer HAL, SurfaceFlinger CPU | 0.40 s, 0.28 s | 0.01 s, 0.02 s |
| aim-display (host) CPU | 0.22 s | 0.01 s |
| All guest processes | 2.6 s | 1.4 s |
| Settings cold start (3) | 255, 214, 224 ms | 239, 215, 213 ms |
| Settings scroll (10 swipes) | 0 % janky, p50/p90/p99 7/9/12 ms | 0 % janky, 7/9/13 ms |

During the scroll the display link did not run: SurfaceFlinger scheduled
all 253 frames from its model.

## Shared code stays mapped (2026-09-30, #444)

Fork children map code without execute and make it executable after
(docs/fork.md), so they no longer make every guest process refault the
shared libraries' code. On a settled boot of a reused data image (host
load 2-3), before and after:

| Check | Before | After |
| --- | --- | --- |
| `free(malloc(64))` loop, faults per iteration during a cold start | 0.16-0.72 | 0 |
| Guest CPU 7 min after boot | 1,565 s, 96 % system | 78 s, 46 % system (#446) |
| `isDeclared` after 300 ms idle | 1.7-2.3 ms | 1.1-1.7 ms, no faults |
| Cold start (`am start -W -S`): Settings, Calculator, Chrome | 0.97-1.01, 1.45-1.46, 1.06-1.14 s | 0.23, 0.31, 0.29-0.34 s |

One of the two boots with the fix hit SurfaceFlinger's hung task
snapshot (#436): the display server deadlocked adding a drawable's
presented handler while Core Animation ran an earlier one, so a present
fence never signaled and RenderEngine waited on it for ever. Fixed
(docs/composer.md, "Buffers and presents"); 20 force-stops of a visible
Settings in one boot then passed.

## Boot timeline (2026-09-30)

One disposable data directory, first and second boot, host load about 9
on the M2 Pro (docs/system-services.md, "Shrinking SystemServer", has
the SystemServer detail): `boot_progress_start` 4.5 and 7.1 s,
`system_run` 6.8 and 9.2 s, `pms_ready` 12.0 and 10.2 s, `ams_ready`
15.5 and 11.0 s, `enable_screen` 16.2 and 11.5 s, `sys.boot_completed`
16.6 and 11.8 s. SystemServer's services take about 0.7 s of a second
boot; the shell (the launcher's first draw, then SystemUI's keyguard
and wallpaper) holds boot completion for 0.67 s after home starts.

guest-init's report breaks the time before zygote down: each command
carries the time it ran, and its `timeline:` lines give the preparation
steps (runtime layout, property areas, scripts, binder host), the data
image's attach and mount, every wait of init's queue with its length,
and every property a service sets, since guest-init started (also
`ro.boottime.*`'s epoch). The data image attaches on a thread while the
boot prepares and runs early-init and init; the `fs` stage's `mount_all`
waits for it (docs/storage.md). On a second boot (host load 26-31) the
preparation takes 0.16 s, the attach ends at 0.50-0.53 s and the mount
waits 0-0.07 s for it; `start zygote` runs at 1.58-1.60 s (a first
and a second boot, host load 10) and `boot_progress_start` about 0.75 s
later. linkerconfig runs once, at `perform_apex_config --bootstrap`:
post-fs-data's run would see the same APEXes (#564). A first boot
clones the image from the template of the `userdata/template` node
(docs/first-boot.md): its attach ends at 0.41-0.53 s and the mount waits
0-0.07 s (host load 12-14; creating the image held the mount up by
0.6 s before, #563). The template build boot puts its display socket in
a short temporary directory so a long worktree path fits Darwin's Unix
socket limit; the directory is removed when the boot stops. From the
template's PackageManager and permission state (docs/first-boot.md),
PackageManager takes 0.57 s from
`pms_start` to `pms_ready` (3.90 s on an original first boot: the scan,
the stubs' decompression) and 0.47 s to `ams_ready` (2.64 s: boot
dexopt), and `sys.boot_completed` comes at 4.9 s (10.6 s without the
template; host load 9-18, 2026-10-01). Those times had the parser cache
in the template; it depends on the device's locale and is no longer
shipped (#722), so the scan parses every package: 0.63-1.03 s against
0.42-0.48 s on a repeat boot, `pms_start` to `pms_ready` 0.85-1.35 s
(host load 8-14, 2026-10-02). Between the mount and zygote-start, init runs its exec
programs one after another (about 25-120 ms each, mostly starting
`linux-run`); the longest are bpfloader (0.3-0.6 s) and
`aconfigd-mainline init` (0.12-0.36 s) (#529). bpfloader's 0.32 s
(run alone: 0.31 s) is three process images (netbpfload execs
uprobestatsbpfload, then the platform bpfloader, about 0.03 s each),
netbpfload's own 300,000 `lseek`/`read` calls of 0.3-0.6 us each
(bionic stdio re-reading its ELF objects; a Linux kernel costs about the
same) and the 142 objects' file creations, pins (hard links), renames and
owner records at 0.1-0.5 ms each on the host (#418, #561).

## Debugging

The original logd runs, and every service logs to it. Read it with the
image's own logcat from another `linux-run` process:

```
tools/guest-logcat.sh [--linux-run PATH] <data>.run            # logcat -d -b all -v threadtime
tools/guest-logcat.sh <data>.run -d -s keystore2               # any logcat arguments
```

- A service's stdout and stderr (the layer's own messages: unimplemented
  syscalls, fatal signals with the faulting module) are in
  `<data>.run/logs/<service>.log`.
- Fatal signals in host code are symbolized there, for example
  `_platform_memmove+0x1bc (libsystem_platform.dylib)`.
- `tools/guest-shell.sh <data> [COMMAND]` is the guest's shell as adbd
  runs it: root, in the boot's pid namespace, with its binder and init's
  global environment (`PATH`, `BOOTCLASSPATH`, `ANDROID_*` and the rest
  of `<data>.run/environ`), so `app_process` tools (`uiautomator`,
  `monkey`, `am instrument`) start. `aimctl shell` is the same for an
  aimctl guest. `tools/guest-shell.sh <data> 'service list'` lists the
  registered binder services.
- A UI dump (the accessibility view tree with each view's text, id and
  bounds) of what the screen shows, for checks of Settings pages or of
  the page Chrome shows:

  ```
  tools/guest-shell.sh <data> 'uiautomator dump /data/local/tmp/ui.xml >/dev/null && cat /data/local/tmp/ui.xml'
  ```

  The XML is one line; `grep -o 'text="[^"]\+"'` lists the visible
  texts, in the guest's language (the Mac's).

## Early boot (2026-09-28)

Everything but the Java world and the services noted below, on an M2 Pro.

- **Result:** 40 services running after 30 s, 48 binder services
  registered with the original servicemanager, and 8,800 lines in logd.
  With selinuxfs's classes (#203) logd holds about 1,070 lines after
  44 s instead of 9,360 after 32 s: 8,390 of those were "Unknown class".
- **Boot time:** init's queue first went idle after 12.0 s, and the last
  early service started at 13.5 s. About 8 s of that are four two-second
  grace periods for waits that only excluded or absent services end:
  - vdc `checkpoint markBootAttempt` and `prepareCheckpoint` and
    `keymaster earlyBootEnded` wait for vold;
  - `keystore.module_hash.sent` waits for keystore2, which waits for
    apexd's `apexservice`.

  Without them the early boot takes about 5.5 s.

### Services

States are init's `init.svc.*` after 30 s. "Restarting" means the service
keeps exiting and init restarts it every 5 s.

| Service | State | Why |
| --- | --- | --- |
| servicemanager, hwservicemanager | running | |
| logd | running | its kernel-log and audit listeners do not start (no `/proc/kmsg`, `syslog(2)` or `NETLINK_AUDIT`: #207, #201) |
| vndservicemanager | not declared | the image has no `vndservicemanager` service |
| prng_seeder | running | |
| keystore2 | running | waits for `apexservice` (`IApexService.getActivePackages`) for the module hash (#200) |
| vendor.keymint-default | running | software KeyMint |
| vendor.gatekeeper_nonsecure | running | registers `IGatekeeper/default` at its first start: `/bootstrap-apex` declares it before `apex.all.ready` |
| gatekeeperd | running | |
| credstore, vendor.identity-default | running | |
| vendor.health-aim | running | our health HAL |
| vendor.graphics.allocator | running | our allocator HAL |
| vendor.authsecret_default, vendor.cas-default, vendor.drm-widevine-hal, vendor.power-default, vendor.power.stats-default | running | vendor APEX HALs |
| neuralnetworks_hal_service_* (3) | running | CPU sample drivers |
| system_suspend | running | no `/sys/power/suspend_stats` |
| tombstoned | running | |
| statsd | running | |
| traced, traced_probes | running | |
| hidl_memory | running | |
| media, mediametrics, mediadrm, drm | running | |
| media.swcodec, mediaextractor | running | their minijail seccomp filters are accepted but not enforced |
| cameraserver | running | no camera HAL declared |
| installd, idmap2d, incidentd, storaged | running | |
| wificond | running | no nl80211 (`AF_NETLINK`, #201) |
| gpu | running | |
| boringssl_self_test64, boringssl_self_test64_vendor, boringssl_self_test_apex64 | exit 0 | |
| system_aconfigd_platform_init, system_aconfigd_socket_service, mainline_aconfigd_init, mainline_aconfigd_socket_service | exit 0 | |
| system_aconfigd_mainline_init | exit 1 | "aconfigd_mainline is enabled, skipping mainline init" |
| derive_sdk, derive_classpath, art_boot, logd-reinit, update_verifier | exit 0 | |
| usbd | exit 0 | no USB HAL |
| odsign | stops itself | keystore refuses the boot-level key (`Boot stage key absent`, `LOCKED`; #200) |
| misctrl | exit 1 | no `/misc` partition |
| exec `recovery-refresh` | exit 254 | no pstore |
| exec `chattr +F /data/media` | exit 1 | `FS_IOC_GETFLAGS` is ENOTTY |
| exec `otapreopt_slot` | exit 1 | not an A/B device ("Slot property empty") |
| exec `kcmdlinectrl update-props` | exit 1 | no `/misc` partition |
| lmkd | restarting (exit 0) | no PSI or memcg (P3 replaced it; see "Memory pressure") |
| netd | restarting (SIGABRT) | `libnetd_updatable_init`: no cgroup v2 directory; then eBPF and netlink (#202, #201; P3 replaces it) |
| audioserver | restarting (SIGSEGV) | null dereference after "Found no HAL version": needs the audio HAL (P5) |
| vold | excluded | the original drives block devices, dm-crypt and fscrypt (P3 replaced it: `daemons/vold`) |
| bpfloader (netbpfload) | excluded: exit 1, then init reboots (`reboot_on_failure`) | no bpffs at `/sys/fs/bpf` (#202) |
| surfaceflinger | excluded: SIGABRT | guest-init passes no `--gpu` (#204), so RenderEngine gets `EGL_BAD_DISPLAY`; with it, RenderEngine runs on ANGLE and SurfaceFlinger aborts with "failed to get hwcomposer service" (the composer is P4) |
| zygote | excluded | ART (P2) |

A crashing native process gets its symbolized backtrace in `logcat -b crash`,
and `debuggerd -b <pid>` prints a process's stacks: debuggerd's `crash_dump64`
runs on the layer's ptrace and cross-process `/proc` (#557). tombstoned writes
the tombstone to `/data/tombstones/tombstone_NN` (an `O_TMPFILE` it names with
`linkat`, #642); its memory map names only the guest's files (#597).

### Fixed on the way

- The binder host no longer reads the argument of `BINDER_THREAD_EXIT`
  (libbinder passes 0). keystore2 crashed in the layer when a hwbinder
  thread exited.
- `/proc/self/fd` (and any synthesized `/proc` directory) has a link and
  `fstat` that match its path, so bionic's `realpath` works. prng_seeder's
  inherited-fd scan aborted.
- `/proc/mounts`, `/proc/<pid>/mounts` and `mountinfo` describe the path
  map.
- `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)` is accepted.
- A `#!` script as `linux-run`'s program runs its interpreter.
- guest-init plays apexd more closely: `apex.all.ready` is set at
  activation (libvintf reads the vendor APEXes' VINTF fragments from
  `/apex` only then, and before it from `/bootstrap-apex`, which lists the
  `vendorBootstrap` APEXes), a lazy `aidl/apexservice` start no longer
  resets `apexd.status`, and `perform_apex_config --bootstrap` loads the
  scripts of `vendorBootstrap` APEXes (the gatekeeper HAL, class
  `early_hal`).
- guest-init resolves symlinks in the image relative to the guest root, as
  a GSI's `/system_ext -> /system/system_ext` needs.
- guest-init's shutdown stops services without running `onrestart`.
- selinuxfs has the classes and permissions of the image's policy and an
  `access` that allows everything, so `selinux_check_access` answers
  quietly (#203).
- `/dev/ashmem` exists (#195): audioserver's `MemoryHeapBase` no longer
  fails with "Unable to open ashmem device".
- POSIX timers, a writable `trace_marker` (#192), per-thread `comm` files
  (#198), and `/proc/<tid>`, the scheduler calls and `tgkill` for every tid.
- `/proc` lists thread-group leaders only, and a value written under
  `/proc/<pid or tid>` goes with its process or thread (#378).
  `/proc/<pid>` belongs to the process's effective ids and its `status`
  has its credentials and capabilities from the process table (#377).
  Signals from another guest process carry its namespace pid and guest
  real uid; a child stays in the table as a zombie until a parent in the
  namespace reaps it, so SIGCHLD and `waitid` report its uid (#383).
- The process table is the guest's pid namespace for every call that
  names a process, not only `/proc` (#341): `kill` (also `-1` and process
  groups), pidfds, the scheduler and priority calls and the rest reach
  guest processes only, and a Mac process is ESRCH. `getppid` and
  `/proc/<pid>/stat` give 0 for a parent outside, such as guest-init
  (#364), and `sysinfo` counts the namespace's processes (#366). A `linux-run` started without a table (tests, a debugging
  shell) is alone in a private namespace with its descendants (#361);
  `--by-pid` joins a boot's. Signals, renicing and rescheduling another
  guest process follow the kernel's uid and capability rules (#362): an
  app gets EPERM for a system uid process, system_server's CAP_KILL and
  CAP_SYS_NICE reach apps. `prlimit` reads another guest process's limits
  under the same kind of rule (#363); setting them is EPERM.
- guest-init's wait for linkerconfig ends at `--timeout` and on SIGINT or
  SIGTERM (#196).

## Java world (P3, 2026-09-28)

The original zygote and system_server on the ART exception, on an M2 Pro.
`guest-init --run` with the derived image of `image/overlay.toml`: our Rust
apexd, vold, netd and lmkd (`daemons/`, the `daemon/*` nodes) replace
the originals (lmkd since the second part below; it kills on the Mac's
memory pressure since #277, see "Memory pressure").

- **zygote** preloads 18,367 classes in 0.31 s and forks system_server
  0.07 s after its preload ends, listening on init's `zygote` and
  `usap_pool_primary` sockets;
  its seccomp filter is accepted and not enforced.
- **system_server** runs its bootstrap and core services, then
  `startOtherServices`: PackageManager scans 245 packages and first-boot
  dexopt runs through artd and dex2oat64 (137 results), about 8 s from
  `StartPackageManagerService` to `startOtherServices`. 197 binder services
  are registered. ActivityManager is ready 15 s after the fork and starts
  SystemUI, the network stack, phone, Bluetooth, the WebView relro creator
  and the setup wizard (`top-activity`) through zygote; SystemUI's shell
  runs.
- **Memory** (host RSS, shared pages counted in each process): system_server
  about 220 MB, zygote 33 MB, surfaceflinger 32 MB, artd 39 MB; 52 guest
  processes take about 730 MB together.
- **Not reached in this first part: `sys.boot_completed`.** On the last runs the host's
  coreaudiod stopped answering (its log repeats `BeginWriteOperation:
  still waiting`; a plain host program's `AudioObjectGetPropertyData`
  hangs too), the audio HAL never registers `IConfig/default`, audioserver
  waits for it, and system_server blocks in `AudioService.<init>` until its
  watchdog kills it (#217).

### Fixed on the way

- ART (`0005`): nterp's `new-instance`/`new-array` decode the class after
  the read-barrier mark entrypoint, and `ExecuteNterpWithClinitImpl` decodes
  the declaring class. libadbconnection is rebuilt (zygote loads it).
- Mount namespaces are per-process path-map entries (`unshare`, bind,
  tmpfs, move, `umount2`, carried over `execve`); init's own binds between
  writable areas (the data mirrors for app data isolation) become path-map
  entries for processes started later.
- `--stdio-null`: services get `/dev/null` on fds 0–2 as init gives them,
  and the layer logs to a hidden descriptor (zygote refused its
  non-allowlisted stdout).
- cgroup v2 and bpffs are areas of the path map; `bpf()` creates maps in
  shared memory and pins them, and loads programs and BTF without running
  them (#224), so NetBpfLoad and uprobestats load everything.
- A forked child gets the stub islands' executable views back as views of
  writable memory (SIGILL in system_server's first JNI call).
- xattrs, with `security.selinux` from the original image (re-extract with
  the current android-image-extract, #229), from `genfscon` for bpffs and
  cgroup2, or `unlabeled`; `stat` reports an image file's original owner
  and mode. installd's restorecon and the tethering module's
  `verifyClatPerms` need them.
- `init_user0` runs `vdc cryptfs init_user0`; vold prepares `/data/data`
  and user 0's storage. `/data/user/0` was a symlink to `/data/data` (#221);
  it is now a path-map bind (see #234 under "Open").
- Fork is a freshly spawned linux-run that takes over a copy-on-write
  snapshot of the guest memory and the layer's state (`docs/fork.md`): a
  Darwin `fork()` child cannot reach XPC services, so apps could not
  compile shaders (#233), and it needed the layer's locks held across the
  fork (artd's dex2oat child hung on fs-attrs) and `__objc_fork_ok` (zygote
  children died of SIGKILL when two threads met in an Objective-C
  `+initialize`). A thread with its own file table runs as a process
  (#228; the debuggerd pseudothread closed every fd of system_server).
- Binder: a zero-length user copy succeeds (a restarted
  `BINDER_WRITE_READ` failed with EFAULT, and libbinder aborted).
- An empty `SCM_RIGHTS` passes nothing; PF_KEY sockets open (NetworkStats'
  `synchronizeKernelRCU`); `setpriority` honors `RLIMIT_NICE`; fwmarkd
  listens; the GNSS HAL answers UNSUPPORTED for its nullable extensions.

## sys.boot_completed (P3, 2026-09-28, second part)

The same boot, on an M2 Pro (16 GB), reaches `sys.boot_completed=1`. First
boot (empty data), `--exclude bootanim` (see "Host security agent"),
`aim-display --size 1080x1920` for the window, the audio HAL on its
null sink (the host's coreaudiod hung, #217):

```
aim-display --socket DISPLAY --size 1080x1920 &
guest-init --image DERIVED --data DATA --run --exclude bootanim \
    --gpu _build/angle-source/out/AimRelease --display DISPLAY
```

`cargo aim boot` runs these two with these flags, and gives aim-display
`--capture target/aim/boot/capture.bmp`: SIGUSR1 to it writes the last
presented buffer there.

| Since guest-init started | s |
| --- | --- |
| zygote (`boot_progress_start`) | 5.8 |
| system_server runs | 7.6 |
| PackageManager ready (245 packages) | 14.3 |
| ActivityManager ready | 23.3 |
| `boot_progress_enable_screen` | 25.2 |
| `sys.boot_completed=1` (keystore2 sees it) | 25.3 |

- **Services:** 277 binder services registered; `init.svc.lmkd`,
  `init.svc.zygote`, `init.svc.surfaceflinger`, `init.svc.audioserver` and
  `init.svc.vendor.audio-hal-aidl` running.
- **Memory** (host RSS, shared pages counted in each process): 74 guest
  processes with about 2.8 GB together at `sys.boot_completed`, system_server
  369 MB, zygote 144 MB; about 4.4 GB for 92 processes once Settings and the
  first-boot apps (GMS, launcher, SystemUI) run. `dumpsys meminfo` has no
  PSS (no `smaps_rollup`).
- **Settings:** `am start -W -n com.android.settings/.Settings` 1 s after
  boot completion: `Status: ok`, `LaunchState: WARM` (its process was up for
  a broadcast), `TotalTime: 243` ms (`wm_activity_launch_time` 243).
  `dumpsys activity activities`: `topResumedActivity=ActivityRecord{... u0
  com.android.settings/.Settings t7}`. A cold launch (after `am force-stop`)
  could not be measured: the host's security agent had ended the Java world
  by then.
- **Input:** a tap injected into aim-display's touchscreen through its
  injection path (a client of `DISPLAY.input/event0` writing records, as the
  tests do; no host HID events) reaches the guest as an evdev packet
  (`getevent -lt`: `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X/Y`,
  `BTN_TOUCH`, `SYN_REPORT`), InputDispatcher's `RecentQueue` holds the
  down/up MotionEvents, and `input_interaction: Interaction with: ...
  Android System` names the top window it went to: an app-error dialog
  that the crash loops of #234 keep on top of Settings. The tap did not
  visibly act on it.
- **Screen:** aim-display presents at 60 fps, but every app window is
  black: zygote children cannot reach Metal's shader compiler service
  (#233). SurfaceFlinger's own output (bootanimation, P4) is not affected.

### Fixed on the way

- **Audio HAL (#217):** registers at once; CoreAudio runs in
  `linux-run --audio-io` with timeouts and a null sink
  ([audio.md](audio.md)).
- **lmkd:** ActivityManager waits for the lmkd socket under its lock on
  every process event. Without lmkd, app starts took 10–25 s, ANRs piled
  up and the watchdog killed system_server a few minutes after boot. Our
  lmkd answers the protocol (and, since #277, kills on the Mac's memory
  pressure); the first boot went from 75–95 s to 25 s.
- **`/proc/config.gz`:** system_server's `Debug.isVmapStack` CHECKs that
  libvintf can read the kernel configuration; the first ANR aborted it.
- **Null page of the heap window:** a fault there is a null-check fault
  (#236); every restarted system_server had died in
  `NotificationChannel.setVibrationPattern`.
- **vold:** the storage views replace init.rc's empty placeholder
  directories (IVold.mount failed with EEXIST), and user 0's exist from
  `initUser0` on; zygote had aborted every app with an installer mount
  mode (Settings: "Failed to mount /mnt/installer/0 to /storage").

### Host security agent

The development Mac runs Exosphere, whose anti-ransomware module flags the
boot's `linux-run` (when bootanimation reads `bootanimation.zip`, and
when GMS churns files after boot) and from then on denies its write opens
(EPERM) and kills every exec of it (SIGKILL, exit 137) (#232). A boot
here therefore excludes bootanim, uses a freshly built `linux-run`, and
runs its `am`/`dumpsys` commands with another build; the Java world lasts
about a minute after `sys.boot_completed`.
On 2026-09-29 it flagged a boot at 04:45:52, four minutes after boot
completion: every process that a release binary of that target
directory spawned (fork children, `sh -c`, zygote and service restarts)
was then SIGKILLed at start, with no kernel or crash report, and netd's
read-write opens of its BPF maps failed with EPERM (so netd aborted, and
its `onrestart` restarted zygote every 5 s). The same binaries run from
a shell, and other target directories' binaries, were not affected; by
05:27 the kills had stopped.

### Open

- #233 app rendering (Metal compiler service after fork);
- #234 app data isolation (SQLite `CANTOPEN` in `/data/user/0/<pkg>`),
  crash loops of acore, launcher and GMS. Cause, from the code paths:
  zygote's `isolateAppData` mounts a tmpfs over `/data/data` and
  `/data/user`, then looks for the app's CE directory at
  `/data_mirror/data_ce/null/0/<pkg>`. That mirror is init's bind of
  `/data/user`, whose `0` was vold's symlink to `/data/data`; in the app's
  view the symlink leads into the new, empty tmpfs, so `getAppDataDirName`
  finds nothing, zygote logs "Ignoring missing CE app data dir" and binds
  no CE directory. `/data/user/0/<pkg>` then does not exist in the app,
  and SQLite cannot create its database. The DE directory was bound.
  Fixed on `agent/appdata`, **not yet verified at runtime** (#232):
  - `/data/user/0` is a path-map bind of `/data/data`, as vold's
    `prepare_special_dirs` bind is on Android; init's `bind rec` of
    `/data/user` copies it to `/data_mirror/data_ce/null/0`; vold checks
    the bind instead of making the symlink;
  - in the layer, a process's own mount hides the older mounts at and
    below its mount point (zygote's `symlink("/data/data", "/data/user/0")`
    in its tmpfs would fail with EEXIST otherwise);
  - fs-attrs are keyed by a file's path in its area, so the stub zygote
    gives `root:root 0700` in its tmpfs no longer overwrote the owner
    installd gave `/data/data/<pkg>`, and `/data/user/0/<pkg>` shows it.
  To confirm: a boot, then `am start` of Contacts (acore) or the launcher
  with no `CANTOPEN` / "Ignoring missing CE app data dir" in logcat, and
  `ls -ln /data/user/0/<pkg>` from `run-as` (or the app) showing its uid;
- #236 the ART codegen behind the null-page
  faults; #237 audio retries after the null sink; #232 the security agent.


## P3–P5 acceptance (2026-09-28, third part)

The same boot with the spawned fork (docs/fork.md), ANGLE displays made
on first use (#233) and `/data/user/0` as a bind (#234), on an M2 Pro
(16 GB), release build. First boot (empty data), `--exclude bootanim`,
`aim-display --size 1080x1920 --capture FILE` (SIGUSR1 writes the last
presented buffer), the audio HAL on CoreAudio. Before any input, the
guest's `sound_effects_enabled` and `charging_sounds_enabled` are set to 0;
the only test signal is the -90 dBFS tone below.

| Since guest-init started | s |
| --- | --- |
| zygote (`boot_progress_start`) | 6.8 |
| system_server runs | 8.9 |
| PackageManager ready | 16.5 |
| ActivityManager ready | 34.6 |
| `boot_progress_enable_screen` | 44.0 |
| `sys.boot_completed=1` (keystore2 sees it) | 50.5 |

The first boot is twice as long as with the Darwin fork (25.3 s), and app
frames are slow (#239).

| | Result | Evidence |
| --- | --- | --- |
| **P3** | passed | `sys.boot_completed=1` at 50.5 s (48.2 s on another run); 68 guest processes with 2.9 GB RSS at that point, 118 with 3.5 GB once Settings ran (system_server 205 MB, Settings 93 MB). |
| **P4** | passed | `am start -W -n com.android.settings/.Settings`: `Status: ok`, `LaunchState: WARM`, `TotalTime: 5076`; `topResumedActivity` and `mCurrentFocus` are Settings. The captured buffer shows the Settings home page, not black. Logcat has no MSL or MTLCompilerService message and no EGL call error; HWUI's config complaint "Device claims wide gamut support" remains (#240). |
| **App health** | passed | No crash of acore (ContactsProvider2), the launcher or GMS, no `SQLITE_CANTOPEN`, no "Ignoring missing CE app data dir", and no dialog over Settings. com.android.phone and GMS persistent each have one startup ANR and come back (#241). |
| **Input** | passed | A tap written into aim-display's touchscreen through its injection path (a client of `DISPLAY.input/event0` writing `ABS_MT_*`, `BTN_TOUCH` and `SYN_REPORT` records; no host HID events) on "Connected devices" opens it: `topResumedActivity` goes from `.Settings` to `.SubSettings`, and the second capture shows the "Connected devices" page. |
| **Audio** | passed | `service check media.audio_flinger`: found. `audio_tone 2000 0` (AAudio, -90 dBFS): 96,000 frames written, `output_xruns 0`, `ok done`. AudioFlinger's primary output (`AUDIO_DEVICE_OUT_SPEAKER`) wrote 240,768 frames; the HAL logged "output stream in standby: 240768 frames, 469 device callbacks, 0 xrun frames, peak -90.0 dBFS": the stream reached CoreAudio, not the null sink. |
| **Vulkan** | partial | `ro.hardware.vulkan=aim` loads `vulkan.aim.so` over MoltenVK (docs/vulkan-driver.md); `pm list features`: `android.hardware.vulkan.level` 0, `.version` 1.3, `.compute`; `dumpsys gpu`: `vulkanVersion = 4206592`. The NDK checks run in `tests/vulkan.rs` (two queues of one family, AHardwareBuffer and YUV images, sync-fd semaphores on the GPU). In a boot, the swapchain mode draws 100 frames, and with `debug.hwui.renderer=skiavk` (a test setting; the default stays GLES) Settings, Chrome and Clock draw with `Pipeline=Skia (Vulkan)`. |
| **Sensors** | passed | `dumpsys sensorservice`: Ambient Light Sensor (`android.sensor.light`) and Lid Angle Sensor (`android.sensor.hinge_angle`), vendor "Apple (darwin host)". `pm list features`: `android.hardware.sensor.light` and `.hinge_angle`, from the SKU guest-init reports (`ro.boot.product.vendor.sku` = `light_hinge`); no "cannot find light sensor" from DisplayPowerController (2026-09-30, #496). |
| **Thermal** | passed | `dumpsys thermalservice` (the native service, 2026-10-01): status 0, cpu 44.2 °C, battery 30.6 °C, skin NaN; no thermal HAL (#624). |
| **Health** | passed | `dumpsys battery`: level 80, AC powered, as `pmset -g batt` (80 %; AC attached). Temperature reads 0 (#242). |
| **Bluetooth** | passed | `dumpsys bluetooth_manager`: `enabled: true`, `state: ON`, crashed 0 times, over our HAL's virtual controller. No scan was run (TCC). |
| **GNSS** | passed | `dumpsys location` (the native service, 2026-09-30): `gps provider` enabled and allowed, identity `1000/android[GnssService]`; network and fused bound from Google Play services. No fix in boots whose host process has no CoreLocation authorization. |

The host's security agent did not flag or block any of the four boots
(#232), each from a freshly built target directory.

### Fixed on the way

- **Fork children aborted** ("fdsan: double-close of file descriptor 55")
  and the boot never completed: the layer's timer kqueue stayed at a low
  fd when RLIMIT_NOFILE exceeded the descriptor table, and a fork child
  kept it hidden although it did not inherit it, so the guest's next fd
  there could not be closed. Hidden fds go above 3/4 of the table, and a
  child hides only what it inherited.
- **"There's an internal problem with your device"** over Settings (#227):
  libvintf's runtime check read the kernel's SELinux policy version as 15;
  selinuxfs has `policyvers` (33).
- **Lost logs:** logd's `logdw` had Darwin's 4 KiB datagram buffer, and
  liblog dropped about 6,300 messages in a boot. guest-init gives init's
  datagram sockets Linux's 208 KiB.
- **Bluetooth crash loop:** the stack asserts Secure Simple Pairing
  (`btm_sec_dev_reset`); the virtual controller claims it.
- **"Failed to wait for the fence 0x3006"** in the launcher: ANGLE's Metal
  backend refuses `eglClientWaitSync` without a current context, which
  HWUI's bitmap uploader does; the driver polls the sync's status then.
- **Chrome exited at first run** (#251): its shared memory check
  (`SharedMemoryRegionGetProtectionFlags`) found no `/dev/ashmem` node and
  failed. The node stats as the device, on the region files' `st_dev`.
  memfds now keep their name and seals with the file, so an fd received
  over binder or `SCM_RIGHTS`, or reopened through `/proc/self/fd`, is
  the same memfd. Chrome shows its first-run page.
- **Chrome aborted "Timed out waiting for GPU channel"** (#256) about 35 s
  after first run: `IChildProcessService.setupConnection` carries more than
  eight fds. Through the binder daemon a reader had only eight
  pre-reserved fd numbers, so the oneway call was dropped with `EMFILE`,
  and neither the GPU process nor the renderers ever started. Now the read
  stops before such a transaction and the shim reads again with enough
  (docs/binder-driver.md). Chrome shows its new tab page, and its GPU
  process keeps running.
- **Chrome's renderers died "V8 process OOM (Failed to reserve virtual
  memory for CodeRange)"** (#260) on every web page: V8 reserves its code
  range PROT_NONE and makes it RWX with `mprotect`, which Darwin refuses
  (EACCES) for anything but `MAP_JIT` memory. RWX memory is now `MAP_JIT`
  (ADR 0012, "Application JITs"). Chrome draws `https://example.com`, a
  `data:` page's script runs (`fib(30)` in 9–17 ms, optimized code), and
  the renderers stay up.
- **GMS persistent crash loop** (#336): Nearby's USB medium throws
  "UsbManagerCompat is unavailable" without the `usb` service, about once
  a second two minutes after boot (104 in a 2-minute window, 49 process
  starts). The device declares `android.hardware.usb.host` (the Mac's
  ports), so UsbService runs; none in three boots since. Declaring it
  alone killed system_server 48 times in 9.5 minutes ("Unable to open
  socket for UEventObserver"): init.usb.rc's writes had made
  `/sys/class/android_usb` appear, which sends UsbService down the USB
  gadget path. sysfs's device trees now hold only the modeled devices,
  and NETLINK_KOBJECT_UEVENT sockets work.

### Open

- #239 boot time and frame times after the spawned fork;
- #240 wide-gamut EGL configs; #241 phone and GMS startup ANRs; #242
  battery temperature; #230 traced aborts (traced_probes' were its memory
  watchdog reading four times its rss from `/proc`, now counted in the
  guest's 16 KiB pages).
- #258 remaining memfd seal gaps; #261 app data lost on a second boot of
  the same data directory.

## Network (2026-09-28)

Details in [network.md](network.md). The original EthernetService,
NetworkStack and DnsResolver bring up `eth0`, which stands for the Mac's
network: the layer's netlink and ioctls carry the configuration, and
`eth0`'s virtual router leases the Mac's address, gateway and DNS servers
by DHCP. First boot, `cargo aim boot`, on a Mac on Wi-Fi:

- `dumpsys connectivity`: `Active default network: 100`, Ethernet,
  `IS_VALIDATED`, LinkProperties 172.30.1.46/24 with the Mac's gateway and
  DNS servers. NetworkMonitor's HTTP and HTTPS `generate_204` probes
  answer 204. The DHCP exchange takes 72 ms; the network is validated
  1.7 s after the lease, 9.4 s after EthernetService starts and 6 s
  before `sys.boot_completed`.
- Shell: `ping -c 3 www.google.com` answers; an NDK program resolves
  `example.com` through DnsResolver and reads `HTTP/1.1 200 OK`.
- Chrome loads and draws `https://example.com` (#260 fixed the renderer).
  Its "No such process (3)" warnings (#257), DnsResolver's ESRCH for a
  network with no nameservers, are gone.

### Fixed on the way

- Java's `bind`/`connect` of AF_INET sockets failed with EINVAL: libcore
  tries a v4-mapped IPv6 address first and falls back on EAFNOSUPPORT,
  which Darwin does not return.
- UDP `connect` to port 0 (the "have IPv4/IPv6" probes of bionic and
  DnsResolver) failed, so every AI_ADDRCONFIG lookup found nothing.
- `SO_PROTOCOL` read 0, so libcore did not exempt UDP `connect` from
  StrictMode, and NetworkStack died of NetworkOnMainThreadException in
  DnsResolver's address sorting.
- `SO_MARK` failed, and with it every DnsResolver query; `SO_RCVBUF` 0
  failed (DhcpClient).
- The emulator's vendor overlay made `eth0` a restricted network; it goes
  from the derived image.
- DhcpClient's UDP socket took the Mac's port 68, so a second guest (or
  the NDK network tests) failed to bind it; the port is `eth0`'s (#334).

## Memory pressure (2026-09-29, #277)

lmkd (`daemons/lmkd`) keeps the original's protocol and kill order and
kills on the Mac's memory pressure, which the host-call module `memory`
reports (ADR 0012's lmkd row, [host-call.md](host-call.md)). A thread's
Linux scheduling sets its host QoS: a nice value of 10 or more or
SCHED_BATCH runs at utility, 19 or SCHED_IDLE at background.

Measured on a loaded host (load average 90, other agents' boots running),
`cargo aim boot` with a reused data directory, seven apps opened with
`am start -W` and then HOME:

- the Mac's level went to warn (`memory_pressure -l warn`) for 30 s; lmkd
  logged one kill a second, 48 in all, every one at oom_score_adj 900 to
  999, none below. `dumpsys activity lmk` counted the same 48, and
  `dumpsys activity exit-info` gave the killed deskclock `reason=3
  (LOW_MEMORY)`. When the level fell back, lmkd logged `memory pressure
  Warn -> Normal` and stopped.
- A dispatch memory-pressure source is not used: the kernel notifies only
  a few, large processes, and neither lmkd nor a plain host process got
  an event while the level read warn for a minute. The module polls the
  level every 250 ms instead.

What is not covered: ActivityManager's process groups
(`setProcessGroup`) change nothing on the host. Without cgroup
controllers libprocessgroup's background profile keeps only its timer
slack action, which has no process form, so the call fails in the guest
(#297). Kills name their process: another process's
`/proc/<pid>/cmdline` and `comm` come from its record (#238).

**Morning check** (one boot slot, after the throttle is lifted):

1. `cargo aim boot`, wait for `sys.boot_completed`, open several apps with
   `am start -W`, then `am start -a android.intent.action.MAIN -c
   android.intent.category.HOME`.
2. `dumpsys activity oom`: several `cch` processes (oom_score_adj >= 900).
3. `memory_pressure -l warn -s 5` on the Mac (real pressure; `-S` needs
   root).
4. `logcat -d -s lowmemorykiller`: `Kill ... oom_score_adj` lines only at
   900 and above (the highest registered first), one a second, then `memory pressure Warn
   -> Normal`; `dumpsys activity lmk` counts them.

## The Mac's settings (2026-09-29, #283, #282, #281)

Details in [mac-settings.md](mac-settings.md). The device takes the Mac's
time zone, languages with their region, and light/dark appearance:
aim-guest-init sets `vendor.aim.mac.*` before init's first action and
when the Mac changes them, and `init.aim.rc` applies them with
`persist.sys.timezone` and `persist.sys.locale` (the first language) in
`post-fs-data` and `cmd alarm set-timezone`; the native uimode service
follows the appearance itself (#655). The service host applies the
whole language list through the system_server bridge at boot and when
the Mac's list changes (#344). A first boot on a Mac on Asia/Seoul,
`ko-KR` and Light shows KST (the Mac's clock), `ko-rKR` and `notnight`,
and `system_locales` `ko-KR`; Settings draws in Korean.

## Storage images (2026-09-29)

On a host with no attached images, IOKit can return success with a null
matching iterator. The storage reader now treats that as an empty collection
rather than resetting the invalid handle forever (#809). A real no-match IOKit
regression test passes; the original read-only image attaches in 1.1s after a
host restart, and the derived image rebuild completes (45.5s, 2026-10-03).

The boot on the case-sensitive images of [storage.md](storage.md): the
system image (compressed, with its translation cache), the derived image
as its shadow, and the data directory as a data image. No boot
animation (`debug.sf.nobootanimation`), no `--exclude`. M2 Pro, with other
agents' builds and boots loading the host (load average 65–150), so the
times are not comparable with the sections above.

| Check | Result |
| --- | --- |
| First boot (empty data image) | `sys.boot_completed` after 48.8 s |
| Names that differ only in case in `/data` | `Foo` and `foo` coexist |
| `pm install -r -g` Chrome | Success; `/data/data/org.chromium.chrome` is 10212:10212 0700 |
| `am start -W -S` Settings (`.homepage.SettingsHomepageActivity`) | `Status: ok`, COLD, 2232 ms |
| `am start -W -S` Chrome | `Status: ok`, COLD, 2688 ms (first-run activity) |
| 1 GiB written to `/data/local/tmp`, deleted, stop | the image file went from 2.86 GB to 1.63 GB (1.44 GB used in its volume) |
| Second boot of the same data image | `sys.boot_completed` after 33–42 s; Chrome's data directory still 10212:10212 0700; Settings (6.7 s) and Chrome (5.5 s) start; no "Failed to prepare", `CANTOPEN` or "Ignoring missing CE app data dir" (#261) |

One second-boot run's Settings launch 1 s after `sys.boot_completed`
timed out (`am start -W`, 14 s); 20 s later it started, as did every
launch of the other runs.

### How to check it

```
cargo aim build                       # attaches _build/android16-image and target/aim/derived
cargo aim boot --data target/aim/boot/data
# in another shell, with the boot's binder (guest-init's pid):
linux-run --root target/aim/derived/root --path-map target/aim/boot/data.run/path-map \
    --binder dev.aim.guest-init.<pid>.binder /system/bin/sh -c \
    'echo a > /data/local/tmp/Foo; echo b > /data/local/tmp/foo; ls /data/local/tmp'
# a shell that should see and signal the boot's processes (ps, kill) joins
# its pid namespace; without --by-pid it is alone in a private one:
linux-run --root target/aim/derived/root --path-map target/aim/boot/data.run/path-map \
    --binder dev.aim.guest-init.<pid>.binder \
    --by-pid target/aim/boot/data.run/identity/by-pid /system/bin/ps -A
cargo aim storage                     # the images and what they occupy
```

`aimctl` ([aimctl.md](aimctl.md)) runs the same boot in the background
(`aimctl --data DIR start`), with `aimctl shell` in place of the linux-run
lines above.

After a stop, `target/aim/boot/data` is empty (detached); a second
`cargo aim boot` attaches the same image, and installed apps start with
their data. Look for a data image left attached by a crash with
`hdiutil info`; guest-init detaches it at the next start.

## Window mode (2026-09-29, #294)

`cargo aim boot --windows` ([windows.md](windows.md)): the display is the
Mac's main screen (3840×2160 at 2×, plus the bar margin: 3840×2352), the
default display runs freeform windowing, and the task bridge reports its
tasks. On an M2 Pro with other agents' boots loading the host (load average
37 to 125):

| Check | Result |
| --- | --- |
| Settings, Calculator (installed with `pm install`) and Chrome from `am start` | three native windows, each showing its task, no Android caption; a window the server shows itself is titled as the launcher names its task (Settings: "Settings", 2026-09-30), not with its package |
| A click in a window | one touch at the display pixel under it (point × 2); Settings opened the page clicked |
| Raising a window | `mCurrentFocus` becomes its task |
| Resize (Accessibility, 412×756 to 640×820 points) | the task's bounds 1280×1592 pixels; Chrome lays out for the width |
| Move | the task moves with the window |
| Cmd+[, mouse button 4, two-finger swipe right | Back (`KEY_BACK`; button 4 is now the mouse's `BTN_SIDE`); SubSettings back to Settings each time |
| Esc | `KEY_ESC` reaches the app; no Back (Android 16 closes system dialogs instead) |
| A task restored from recents at boot | no window |

Under that load SystemUI hit ANRs, and freeform positions Android chose
itself (Chrome's launch, Settings shifted away from Chrome when a page
opened) did not reach the task surfaces, so those windows showed other
parts of the display; the bridge now commits such bounds by moving the
task a pixel and back with `resizeTask` (a resize to the same bounds is a
no-op and did not help).

**Per-task composition** (2026-10-01, #692, [layers.md](layers.md)):
window mode now composes each Mac window from its own task's layers
(device composition in the composer HAL) instead of cropping one display
buffer. Fresh window-mode boots (ops checks layers-1, -2, -5): every app
layer `DEVICE` in `dumpsys SurfaceFlinger` (6 of 6, none `CLIENT`); Clock
overlapping Settings (about 80 %) and Chrome (its left part), in either
stacking order, each window showed only its own app; Chrome's first-run
page filled its window (no dark strip, #691), and a force-stopped Chrome
relaunched into a new window within 1 s; a Clock task moved by `am task
resize` took its window and content along with no panel left behind;
`input tap` in the Clock window switched its tab. No composition errors,
SurfaceFlinger aborts or ANRs in logcat. Device mode stays client
composition (`CLIENT`). On a first boot PackageManager installs GMS about
30-40 s after boot completes; a Chrome running then kills itself
(`DynamiteLoaderV2Impl: Module config changed, forcing restart`) and its
window closes with its task.

**App shims** (same boots): `aim-apps shims --watch` wrote 19 shims into
`target/aim/boot/apps`, the packages `cmd package query-activities -a MAIN
-c LAUNCHER` lists (Gboard's launcher activity, disabled at run time, and
GMS's, disabled by a resource, left out). Opening Calculator.app,
Settings.app and Chrome.app started each app in its own process with its
name and icon in the Dock; a click in the Calculator shim's window, behind
Chrome's, focused its task and typed 7, and keys typed 5 5.

Window-mode boots of 2026-09-30 (#352, #354, #356, #357):

| Check | Result |
| --- | --- |
| Shims per launcher activity | 22 shims; the Google app has two, "Google" (primary, `dev.aim.app.com.google.android.googlequicksearchbox`) and "Voice Search" (its own bundle identifier); each connects as the host of its activity |
| Server window | `am start` of Settings with no shim running: a window of the server titled "Settings"; opening Settings.app moved the task into the shim's window |
| Server in the Dock | none: `lsappinfo` type `UIElement`; the Dock shows the shims' icons (Google, Voice Search, Clock with its hands at 10:10) |
| Uninstall | `pm uninstall` of Calculator (its shim open): the bundle removed within 4 s, its host exited, and Launch Services no longer lists it; at the end of the boot `aim-apps clean` left no bundle or registration of `target/aim/boot/apps` |
| system_server restart | none in five boots on one disposable data image (2026-09-30); before, every boot after the first restarted zygote (#490): system_server reached BiometricService before gatekeeperd, whose HAL's first start had aborted, and died ("Gatekeeper service not available"), and zygote killed itself with it. The task bridge still restarts with zygote (`init.svc.zygote=restarting`) |

VoiceSearchActivity opened no window of its own (no freeform task with
bounds was reported for it).

Window-mode boots of 2026-10-01 (#463):

| Check | Result |
| --- | --- |
| Splash (D4) | opening YouTube.app cold: its window with the YouTube icon on YouTube's splash color (white; Clock's black), YouTube's UI in it about a second later. The host shows the splash before it sets up its renderer, notification center and connection (#605): 0.22-0.26 s from `open` to the window on screen on an idle Mac, against 0.28-0.31 s before (a probe shim); the first recording's 1.1 s was at load 7. The color is the launcher activity's `windowSplashScreenBackground` or `windowBackground`, light and dark (#606) |
| HOME (D6) | `am start -c HOME` over Settings: the empty home in front, Settings' window gone from the screen (a window of the server: minimized; a shim's app: hidden) |
| The lightweight shell, the image in both modes since 2026-10-01 ([m1-shell.md](m1-shell.md)); measured before as a check-only image against the one with SystemUI | boots to `sys.boot_completed` (fresh data 5.7 s, repeat 4.6-5.8 s, against 5.9 s and 4.7-4.9 s with SystemUI); HOME is the device's own empty `dev.aim.home/.Home`, with no Settings `FallbackHome` restarts (the platform's `SystemUserHomeActivity` is never resolved as home, #643); no SystemUI, launcher or wallpaper picker process; `googlequicksearchbox:search` runs, bound by the default assistant's `:interactor` (#604); the overlay's services absent; standard Mac title bars (caption 0, #545); its own static wallpaper (`java/image-wallpaper`) in ImageWallpaper's place: CtsWallpaperTestCases, whole module in two shards, 115 pass / 6 fail against the default image's 117 / 4, the same four plus the two `_onLockScreen` visibility tests, which need a keyguard the shell has none of (#650); CtsNotificationTestCases' NotificationManagerTest 111/0/4 and NotificationManagerZenTest 71/0/1 as with SystemUI, in 2.5x the time (#680), and its bubble tests fail or hang without bubbles (#679); screen pinning (`am task lock`) shows a pin in the menu bar of the task's shim, or of the display server for a task without one, and `am task lock stop` (the call its Unpin makes) removes it; AimHome and AimImageWallpaper run from the image's odex (`pm art dump`: verify, prebuilt) |

**Notifications** (2026-09-30, #4, [notifications.md](notifications.md)):
guest-init's notification bridge registers with NotificationManagerService
once it is published; the first boot's notifications ("Android is
starting", Play Store's) appeared in Notification Center from the "Android
System" and Play Store shims, which the display server opened in the
background, and `cmd notification post` from the shell uid from "Android
System". CtsNotificationTestCases' NotificationManagerTest passes as
without the bridge (114 of 114). Custom views are read past (Clock's
timer keeps its actions), resource and `file:` icons are drawn on the Mac,
and full-screen intents launch while the Mac is locked (not exercised on a
locked Mac).

**Media** (2026-10-01, #463, [media.md](media.md)): guest-init's media
bridge follows the media session Android's media keys go to and publishes
it as the Mac's Now Playing from the app's shim (VLC: `now playing
org.videolan.vlc (Playing)`, then `nothing` after a force-stop). Screen
capture requests start the device's consent activity
(`/system/app/AimMediaProjection`, named by the framework overlay in place
of SystemUI's), which asks with a sheet on the app's window and creates
the projection through MediaProjectionManagerService. The Mac's Now
Playing UI and an answer to the sheet were not exercised (no clicks);
CtsMediaProjection* wait for SystemUI's dialog (#632).

## Pointer, scrolling and shortcuts (2026-09-29, #214, #288)

A window-mode boot of the derived image with the mouse device
([input.md](input.md)); the input went through the server's path for a
window host's records (`hosts::apply`, the calls `aim-display`'s AppKit
handlers make), not through posted AppKit events.

| Check | Result |
| --- | --- |
| `dumpsys input` | `aim-mouse`: classes `TOUCH`, Touch Input Mapper in `POINTER` mode, sources `MOUSE`, X 0–3839 and Y 0–2351, `VSCROLL` and `HSCROLL`; `disable_touch_input_mapper_pointer_usage` unset (the pointer-usage path) |
| Hover over Settings | the row under the pointer highlights |
| 15 trackpad deltas of 40 pixels up | Settings scrolls; `getevent -lt`: `REL_WHEEL_HI_RES` −37/−38 each, a whole `REL_WHEEL` −1 every third |
| A diagonal gesture up and left (20 and 40 pixels per event) | both axes: `REL_WHEEL_HI_RES` −18/−19 and `REL_HWHEEL_HI_RES` +37/+38 per event, its first 8 points' horizontal part sent once decided |
| Right click in the Settings search field | the text field's context menu (undo, select all, autofill) |
| Typing, Cmd+A, Cmd+C, Right, Cmd+V | `KEY_LEFTCTRL` around `KEY_A`, `KEY_C`, `KEY_V`; the field reads "ifiifi" |
| Pinch out with rotation (12 steps of +5 % and 3°) | Pointer location shows two pointers spreading along arcs |
| A two-finger swipe right, then momentum | `KEY_BACK` down/up only, no `REL_HWHEEL`; SearchActivity back to Settings |
| The pointer sprite | `CURSOR` in SurfaceFlinger's HWC layers; absent from the presented frame (the display server's capture), present in `screencap` |

**The Mac cursor, smart zoom and text shortcuts** (2026-09-30, #387,
#391, #394): a window-mode boot, a window host of the server's protocol
sending the records `aim-display`'s handlers send and reading the cursor
records every host gets; Settings' search field (SettingsIntelligence,
Gboard as the input method).

| Check | Result |
| --- | --- |
| The mouse over the window, then over the field, resting at each | the host gets the arrow (48x48, hot spot (9, 7): its tip) and over the field the I-beam (48x48, hot spot (24, 22): its middle), each time the pointer crosses; typing hides the pointer, and the host gets the default cursor |
| Typing "hello world", then a double tap (smart zoom's four touches) on "hello" and z | "z world": the double tap selected the word |
| Cmd+Right, Option+Delete | "z ": the word before deleted |
| abc, Option+Left, x | "z xabc" (keys a third of a second apart; with no gap Gboard applied its composed letters after the move) |
| Cmd+Delete | "abc": deleted to the line's start |
| Cmd+Left, Cmd+Shift+Right, q | "q": the selection replaced |

A device-mode boot then, keys written into the keyboard as the display
server sends them:

| Check | Result |
| --- | --- |
| The Mac on 2-Set Korean (`2SetHangul`) | `vendor.aim.mac.keyboard_layout` `keyboard_layout_english_us`; `aim-keyboard` set it (logcat) |
| G K S R M F in the Settings search field, Gboard on Korean | `KEY_G` … `KEY_F` in `getevent -lt`; the field reads 한글 |
| Ctrl+Space, then the layout set to `keyboard_layout_english_us_dvorak` (as the Mac's Dvorak would) | Gboard on English; Q W E R T Y and H J K L type `',.pyf` and `dhtn` |
