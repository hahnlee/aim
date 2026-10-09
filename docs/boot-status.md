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

PackageManager remains original in the default image. Controlled slice C images
redirect native construction, UserManager/ART adapters and lifecycle calls;
`package` and `package_native` identify the native guest-init owner. All 298
planned methods are implemented; this is not conformance acceptance.

The frozen b04c9e67 original/native pair uses the same pinned 66-module CTS
inputs. The original campaign is terminal and all owned processes are reaped:
26 pass / 41 fail / five assumptions in verified associated XML, plus ten
DexMetadata passes retained separately after an association error. Seven rows
are recorded and 59 remain not-run-complete; this is not acceptance. Later
original commands cannot load libdl after pre-reboot dexopt namespace changes
(#1236); Compilation APK/DM writes also fail fstat with ENODATA (#1235).

The native b04c9e67 campaign is terminal: all 66 modules were attempted, with
three all-pass recorded modules, five recorded failing modules and 58 not-run
or incomplete modules. Owned processes and leases were released; this is not
acceptance. Parsing reports 9 pass / 2 fail against
original 11 / 0 (#1110); Setting 2 / 0, UsesLibrary 3 / 0, DexMetadata 1 / 9
(#1225, #1226), Compilation 3 / 26 (#1232, #1233, #1234, #1061). Host classpath
failure (#1222), concurrent XML association failure (#1223) and actual device
preparation timeout (#1227) leave affected rows NOT RUN. Original zygote
SIGABRT (#1224) and native adbd fdsan abort (#1221) remain separate failures.
Neither campaign proves the acceptance gate.
The inverse VFS view now compares the implicit root with bind aliases (#1236),
so binding `/` under the pre-reboot dexopt path cannot rename ordinary library
paths. Actual syscall readlink through an open file, nested mount precedence,
alias retargeting and user/storage mirror checks pass (four tests). The terminal
original campaign cannot be counted as corrected; a new replay remains needed.
The retained b04 native logs also contain 45 adbd and one zygote
`fork child: damaged state` failures (#1241). The next runtime now reports the
first invalid fork-state module and byte offset, expected/received version, and
distinguishes state restoration from unread or truncated final descriptor tails.
Encoding and restore order are unchanged; one focused truncation, invalid UTF-8,
version and tail regression passes. This adds diagnostics, not a verified fix;
actual failing original-process replay with these diagnostics remains NOT RUN.
Binder socket transfer now preserves the genuine allocation receipt and actual
backing through native fileports (#1235), with strict class-5 metadata validation
and no receiver-side allocation guess. The original install-write ENODATA was
reproduced on an incoming socket that had lost this receipt. File-carrying reply
references now release after send, before waiting for the next request, so they
do not anchor peer EOF. Two owned-process Binder identity/payload/dup/EOF and
multicast regressions pass; host library gate is 56 pass, zero fail, one ignored
controlled helper. Binder wire version 5 requires a coherent runtime rebuild.
The coherent aa0c8884 original-PMS replay failed before installation because
initial path-map parsing rejected the bootstrap read-only `ro` entries (#1242).
The parser now retains them as image areas; a fresh-process initialization test
passes actual read access and EROFS on writes without changing the backing.
The coherent 7f837348 replay passed initial map parsing but failed boot after
keystore2 exited with status 1 four times. Its actual namespace receipts exist;
external readiness instead failed with missing self admission (exit 127, #1243),
a separate failure. APK installation remains NOT RUN. The original full-default
linkerconfig was also not generated after the restricted bootstrap run (#1245); its
causal relation to keystore2 requires a corrected replay.
External managed-runtime commands now deliberately enter the live authenticated
init namespace through `--mount-namespace-from-init` (#1243). The native POSIX
owner separately admits the audited actor and creates its real lock holder;
stale/foreign/missing records and absent capabilities fail. Actual process IPC
and final lock-release checks pass, as do selected all-target compilation and
shell syntax checks. Corrected boot readiness and CTS replay remain NOT RUN.

The f242504e combined ABI gate passes 249 with zero failures and 21 ignored;
controlled child entries run through their parents, and input-dependent/manual
gates remain separate. Native create-user now forwards original pinned parser
options to the retained original UserManager, preserving actual caller and PFDs
(#1238). Four focused dispatch/transport/caller tests and complete Java/AIDL,
D8 and original API linkage pass; real user creation/removal and the relevant
AppOps CTS setup remain NOT RUN.
Shell install now passes the retained incoming APK file owner directly after
actual sizing (#1239), preserving allocation metadata and writer lifetime
through the generated session WRITE transaction. One actual Binder leaf
regression passes; the frozen Attribution setup failure has no stderr/session
receipt proving this cause, and actual corrected installation remains NOT RUN.
Instant caller query resolution now applies the original post-filter rather than
throwing for every instant result (#725): retain its own package or visible
non-instant activities, after existing web/split handling, with actual UID/user
and captured identity preserved. Four focused public-query/policy regressions
pass; resolve-for-start behavior is unchanged. Actual AppOps instant replay is
still NOT RUN.

The isolated namespace owner now retains stable mount-object IDs, parent and
shared/slave relations, clones real mount inventories, and separates native
init bootstrap/default views (#1228). The pinned original apexd enables early
virt bootstrap; its original ELF initializer and bootstrap use were checked.
Fork publishes an authenticated child namespace before resuming it, including
CLONE_NEWNS and parent exit; private APEX/linkerconfig changes stay isolated.
The default namespace now records each actually activated APEX module as its
own read-only mount (#1246). Actual module open/proc-fd readlink stays under
`/apex`, writable opens return EROFS, and native bootstrap/default inventory
and stable mount-ID proofs pass (two focused tests). The coherent 79cf9cec replay confirms the actual bootstrap/default helper
namespace receipts and a 238499-byte full default linkerconfig. The original
BPF loader exits zero and sets bpf.progs_loaded=1. Original SystemServer then
dies in HintManagerService after Power HAL discovery returns no SupportInfo
(#1247); actual HAL addService failures and init namespace selection remain
under investigation. The original-init prerequisite mismatch is verified:
ServiceManager was admitted to bootstrap before default readiness, hiding
active vendor VINTF inputs. Native init now consumes the pinned original
service policy from explicit image configuration, preserving per-service
restart choices, typed transient helpers and updatable-service deferral. An
actual selected-default child reads the unchanged original Power VINTF bytes;
five focused tests and the full guest-init library pass (33 pass, two controlled
helpers ignored and exercised by their parents). Corrected HAL registration,
The 0b4b2631 replay confirms ServiceManager/HW-manager default placement
before readiness and successful original security HAL registration. It stops
when bootstrap keystore cannot link libandroidicu from libsqlite (#1251). The
bootstrap APEX view used symlink directory entries, which original libapexutil
skips when scanning DT_DIR mountpoints. Native init now supplies real directory
mountpoints with unchanged read-only module backing; actual enumeration and
bootstrap/default namespace regression pass. Corrected linkerconfig scanning,
boot completion and APK installation require a new image/runtime replay.
Kernfs area encoding is retained instead of being imported as writable. Actual
original-bionic fork/NEWNS, two-process view, mountinfo and parent-exit proofs
pass. Combined guest-init: 27 pass, zero fail; storage: 60 pass, zero fail. The
combined ABI gate is not green: 243 pass and two Binder/socket failures remain
under separate owner review. New image CTS and #1236 replay remain NOT RUN.

Two native empty-data templates publish 70 guest files each and reach boot
completion at 196.2 and 196.8 seconds under concurrent load. Parity is not
accepted: preferred state differs (#1218), permission/role completion is missing
(#1219), loading state is incomplete (#1220), and shared-UID ABI inheritance is
missing (#810). The isolated native constructor now publishes an immutable settings bundle before
public service registration, retaining the actual unsigned 64-bit query epoch and
PermissionController version. The builder consumes its bytes, file and ancestor
metadata and xattrs; the original image retains its existing capture path without
claiming constructor provenance. Capture and permission-completion tests pass
(one storage, two builder), and related all-target compilation passes. Actual new
image/template replay and consumer checks remain necessary. The nine pinned ART shell commands
now delegate to the original ArtManagerLocal shell owner with actual transferred
PFDs and authenticated inbound caller identity (#1226). Three transport tests
pass, including real driver forwarding of the shell caller, descriptor lifetime,
and original exception/tail handling; complete Java/AIDL, D8 and original-image
linkage pass. One source-derived dispatch guard test and the three transport tests pass;
Rust/Java guard drift is checked against the pinned original command set. Actual
profile creation and official CTS replay remain NOT RUN.
Installer authorization now retains an uncached reader of the original Context
permission owner, not granted permission values (#1230). Public create-session
retains actual PID/UID; commit and emergency decisions re-read current installer
and target UID permissions. The existing typed Maintenance leaf is available
before ActivityManager publication and preserves original Context semantics.
Seven focused tests pass, including grant/revoke/re-adopt and real driver reply,
epoch and descriptor checks. Actual Java delegation and official replay remain
NOT RUN.
SDM admission now reads the original cloudCompilationVerification flag through
its generated Bootstrap getter (94), performs full v3 crypto verification, and
matches current signer certificates to the already verified APK (#1233). Four
focused tests cover actual crypto/admission and live flag owner errors; complete
Java/AIDL, D8 and original-image linkage pass. Official SDM replay remains NOT RUN.
The original ART install completion result now preserves actual per-dex external
profile errors into per-child ADB warnings and PackageInstaller.EXTRA_WARNINGS
(#1234). Atomic package success remains separate from dexopt warning/error
status. Original improveInstallFreeze selects the exception policy: asynchronous
failures log the actual Throwable and continue without a fabricated DexoptResult;
synchronous failures propagate the same Throwable. Six DTO/ledger/real Binder
regressions and a production Java policy oracle pass; Java/AIDL, D8 and original
method/field linkage pass. Actual ART execution and official warning replay
remain NOT RUN.
The separate original-PMS svc reproduction confirms actual Svc execution with
no output or shell-v2 exit frame before the 140-second bound (#1227). The isolated
901eea52 carrier-owner runtime still times out untraced. A later diagnostic
SIGQUIT to a freshly verified owned Svc PID captures original ART main in
DestroyJavaVM/Runtime destruction waiting on SignalCatcher pthread_join; the
client then exits zero. This is post-signal evidence, not an untraced pass or
proof of the pre-signal wait location. All diagnostic processes and mounts are
reaped. Standalone original-bionic regressions now pass thread-directed blocked
SIGQUIT before/after wait readiness, competing-waiter isolation, SI_TKILL and
bounded join, plus the separate external SIGQUIT case. These do not reproduce
the managed ART hang; no production signal defect is demonstrated.

Isolated acceptance fixes retain the running pair unchanged. The complete PM
library gate passes 867 tests with zero failures and eleven ignored tests not
run. Visibility recapture passes five focused tests, loading completion three,
shared-UID ABI inheritance three, and dex-metadata sidecar handling six, including
preflight of all temporary names and restoration after rename failure.
Complete private Java/AIDL compilation, D8 conversion and strict original-image
linkage pass. These source checks do not establish deployed Binder, template or
CTS success. The corrected official-XML collector passes three actual paired-XML
regressions and six comparison tests; it binds exact command, device and wrapper
lifetime and retains ambiguity as an error. Current frozen campaign association
failures remain NOT RUN until a fresh campaign uses the correction. The carrier retirement owner now retains failed retirement until a real ACK,
without replacing a completed syscall result (#1229). A new actual release
triggers synchronous retirement; socket observations cross its owner barrier,
while pipe-only readiness and getpid retain their results. Actual lease/discard/
reconnect/EOF and dead-service noninterference regressions pass. The complete ABI
library gate passes 242 with zero failures and 16 ignored; controlled helper
entries execute through their parents, and the two standalone performance benches
remain not run. Its FUSE CLI helper input is the hash-pinned frozen b04 executable,
not proof of a newly deployed runtime. The poll FD fixture is process-isolated
from other tests' global FD state. Actual Perfetto/adbd crash replay remains
unverified (#1207, #1221). The isolated native carrier now uses an opaque PIPE
writer token with a retained reader and one process-owned kqueue reactor (#1231).
Actual final kernel reference loss drives EOF, including queued rights and a
SIGKILL recipient that never authenticated. Reciprocal pipe identity protects
registration, and ordinary pipes remain unclassified. Host library gate passes
54 with zero failures; its one ignored child helper executes through the parents.
Actual ABI import/re-export/discard/reconnect/ACK/EOF regression passes. No known
pre-auth failure is waived. The live b04 cohort still uses the older carrier;
all common runtime binaries must be rebuilt coherently before runtime replay.

The inactive UM redirect gate verifies all 14 original PMS
call sites. An isolated experimental C worktree has built redirected services.jar and its
matching oat/vdex and derived image; the default image remains original. Experimental C boots now reach the redirected native PM main.
The first attempts exposed startup ordering defects: native image mapping
must open the actual mounted guest `/data` only at PM entry, and the headless
mode Binder query must follow original UM registration. C now reaches the
raw-scan entry after UM creation. Initial APK scanning follows the original
`InitAppsHelper` `/data/app` scope and does not require the later mount service;
adopted volumes stay on the post-mount listener path. Early scan now uses original parser/AppsFilter constructor
state; provider-backed policies bind at the later original systemReady phase.
First-boot Settings/restriction publication precedes protected preferred/browser
reads. These controlled C startup fixes are not yet CTS/app acceptance. Adding an adb bootconfig
key now preserves the default `hardware=aim`; without it the wrong init/fstab
prevented zygote startup. These fixes are under controlled boot verification;
C now parses original system APKs with actual display density. The current
C44 passes RoleController/window startup, but GMS provider startup still hits
another prepared-mutation race. All common prepare callers now serialize native
capture/prepare under the same gate where owner side effects permit it; installation
codes use fieldwise latest rebase after their actual lower-owner work (#1055).
Application-data plans retain their true source capture. Latest selection, conflict
checks and publication share the coordinator lock, and callbacks run after release.
Two exact interleaving regressions pass. Caller-preserving native CLI read/install/
mutation cohorts compile with original helper APIs (#1052). C45 full Java/linkage/
Rust/oat/image build passes; native full gate 726/0, eight ignored not run.
C46 reaches `sys.boot_completed=1` with zygote running and native PM active.
Native read CLI lists the actual packages and resolves paths/features; HOME resolves
to `dev.aim.home/.Home`. Original Settings cold start completes with `Status: ok`
(13.501 s under the concurrent host load). Local Calculator installation reaches
the installer but fails its device-policy generation check; this is not an install
pass. Original
Googlesdksetup provider startup fails when its own getProviderInfo returns absent;
this remains under owner-level correction. C47 Java/DEX linkage, Rust/oat and derived-image build pass with saved factory
runtime capture for persisted disabled-system packages. C49 repeats the C46 data and reaches `sys.boot_completed=1`, after fixing
completed-scan persistence on restored boots and bounding installed-application
Binder replies. `dumpsys --pid package` and `package_native` both return the
actual native guest-init owner PID, distinct from original SystemServer.
Repeat-data boot completion is verified. Afterward Settings launch times out and
SystemServer watchdog reports blocked foreground/main/UI/AM handlers; its traces
are preserved. A local Calculator installation rejects a host path at the original
installer context boundary. The host-to-guest installer context mapper and snapshot lock scopes are corrected
in C50; C50 still hits a main-thread startup Watchdog while original connectivity
systemReady waits, so it has no app/CTS pass. Actual snapshot traces and the native
usage coordinator path are under investigation. C51 integrates the raw-FD/vold/
FUSE owner cohort: eleven focused syscall/backend/cache tests pass, including a
SIGKILL writer and actual write-before-release; its helper is exercised by the
parent test. C52 reaches boot completion and Settings launches with `Status: ok` (21.631 s
under host load), HOME resolves to AimHome and both public service PID queries
identify guest-init. The actual FUSE mount rejects the original trailing-comma
options (#1081); its parser correction passes a focused regression. Calculator
installation reaches permission-candidate runtime validation but fails setting/code
identity. Contacts provider enumeration null-metadata rejection is corrected at the query
owner with a passing focused regression (#1082); actual replay is pending. Post-boot app ANRs and a subsequent foreground/main Watchdog remain. A native
sample confirms each component/usage publication repeatedly serializes the whole
parsed APK inventory under the publication path; owner-local immutable code reuse
and usage-only projection now pass focused regressions (#1083, #1085). C53
includes install-candidate runtime rebinding (#1084) and provider metadata lookup
(#1082), reaches boot completion with native PM ownership and starts the original
MediaProvider ExternalStorageService/FuseDaemon through its original JNI and real
raw-FD handoff. The subsequent per-user bind layout lacks `/mnt/installer/0` and
fails a FUSE bind (#1086 scope); it is not a mounted storage pass. Calculator
installation advances to a missing candidate library dependency inventory (#1084).
A later original scheduler request for isolated UID packages raises unsupported
and kills SystemServer; exact UID-query semantics are under correction.
C54 reaches boot completion and Settings starts cold successfully (9.008 s),
with no SystemServer fatal/Watchdog observed during that check. The selected
isolated/compute/SDK UID regression passes (#1089). Installer and Android-writable
view roots exist (#1090), but a real FUSE target lookup for Android/data still
returns ENOENT, so the volume is unmountable. Local install now advances past the
library graph and fails the legacy permission package inventory. These producer
gaps are under correction; original MediaProvider mount, CTS and full app
acceptance remain unverified. The first actual host parsing-module run reaches
UsesSdkTest over authorized adbd/Tradefed and fails on the legacy candidate
inventory and `<extension-sdk>` support. The run is interrupted after an image
rebuild force-detaches the live derived mount; its partial results are not a
module pass. The experimental image now includes main's adb init configuration,
Active-image rebuild refusal now has a live guest lease check: exclusive image
mutation is refused while guest-init retains its reader. C55 reaches boot completion
and original MediaProvider reports emulated;0 mounted, with real installer/FUSE
Android data paths. Settings starts cold (26.448 s under load). adb/Tradefed now
runs the parsing module; extension-sdk parsing reaches commit and two official
manifest-fixture regressions pass (#1097). Install-context/candidate generation
and usage-runtime rebasing still fail, and adb shell storage remains an empty
caller view pending namespace alias correction. The completed official parsing module reports zero passed and eleven failed tests;
its XML is retained. Install snapshot/usage rebasing, init bind-source propagation,
setgid creation inheritance and blocked default SIGCHLD handling have focused
regression passes (#1100, #1098, #1099, #1101). C56 includes these fixes and
C56 reaches boot completion and mounts through original MediaProvider again;
Settings starts warm successfully (4.714 s). Calculator install now fails the
final scan publication with Stale, and the caller storage view remains an empty
anchor because init bind provenance is absent from the generated path-map. The
official parsing module replay exercises all eleven tests and all fail at original
permission PREPARE (exception -4, null message); official XML confirms zero passed, eleven failed and one completed module.
C56 shutdown completed before the next image rebuild. Atomic install publication
and failed-session reopening regressions each pass (#1100, #1103); C57 includes those fixes and the init path-map preservation correction; its
controlled repeat-data replay reaches boot completion and original FUSE mounting.
Both guest-shell and real adb shell UID 2000 see the actual media_rw directory
inventory rather than an empty anchor. Calculator install advances past final
publication Stale but fails post-install permission lifecycle/live state matching
for an unrelated existing package. Zygote later aborts in GetOpenFdsIgnoring/Restat
and restarts the system; this is under kernel investigation. C57 CTS diagnostic
option trial is rejected during configuration, so its tests are not run. A later
unmodified-harness single UsesSdk test executes and fails; exact PREPARE logging
identifies original AppIdPermissionPolicy.trimPermissionStates line 1585 throwing
NullPointerException from onPackageAdded. Receipt UID scope and original permission
owner lifecycle are under correction. Final single-test XML confirms zero passed, one failed and one completed module.
C57 is stopped before rebuild. An interrupted internal mount-journal lock
regression passes (#1104); whether it caused this zygote abort needs replay.
C58 starts with fresh disposable data and reaches boot completion. The first
Calculator PREPARE completes but install-base validation rejects a changed
publication; a subsequent unchanged official CTS-generated APK immediately
reproduces trimPermissionStates NullPointerException. This establishes a failed
PREPARE cleanup gap in one fresh boot; neither installation passes. C58 Settings
starts warm successfully (4.601 s) and storage shows actual FUSE content. C58
is stopped. C59 includes committed-aware original permission rollback, UID receipt
scope, exact install identity diagnostics and the typed original DVS XML/settings
bridge; Java/image linkage and native tests pass before controlled replay. C59
fails construction because existing-install completion reaches domain reconciliation
before domain initialization. Its runtime initializer now follows persistence
installation and precedes installer construction. C60 reaches boot completion
without that failure or the previous null DVS connection. Calculator installation
advances to PACKAGE_ADDED, but original OverlayManager then throws native
`internal mutation base changed` from overlay publication and kills SystemServer.
Native durable commit and query publication precede this PACKAGE_ADDED event;
committed reservation release retires permission scopes without rollback. The
CLI completion is interrupted and is not counted as a pass. C60 is
stopped. Internal mutation publication now retries pure verified operation
preparation if usage/context publication wins the final prewrite CAS; bootstrap
and persistence identity and true field conflicts remain checked. Rust compile
passes; C61 reaches boot completion and retains Calculator at its actual
/data/app path. Explicit Calculator cold start passes (2.197 s). The implicit
MAIN/LAUNCHER difference is original AMS MATCH_DEFAULT_ONLY behavior, confirmed
with the same explicit query flag returning no activity. Repeated official CTS
generated APK install fails final query publication scan-owner validation but no
longer raises PREPARE NullPointerException or rollback errors. SystemServer remains
alive during these checks through the final pre-shutdown query. Canonical
scan/usage exact-base CAS and latest install query recapture are implemented;
six usage-owner regressions and the full native cohort pass. C62 still detects
postcommit target-code replacement by a nonusage publisher. Freezer-specific
canonical/query-lag preservation passes seven actual owner regressions; C63 still
reproduces target-code replacement by another publisher. Caller/location and exact
identity diagnostics at C64 identify internal_grant_implicit_access as the actual
stale publisher (system_package_internal_implicit.rs:56), replacing the new
committed path with the previous path at the same version. C64 remains alive
through replay and is stopped. Typed visibility/query-lag publication is being
corrected with eight passing owner regressions. C65 reaches boot completion,
installs and publishes the actual new CTS-generated APK path, but postpublication
monitor validation rejects a newer coherent query snapshot. SystemServer remains
alive through the final path query and C65 is stopped. Receipt-based monitor
validation uses actual target identity/receipt and passes its regression. C66
returns Success for the unchanged official generated APK. Its full parsing module
then reaches SDK assertions and finds missing minExtensionVersions dump output;
an asynchronous original DVS save races new native package attachment and kills
SystemServer with an attached-universe validation error. These two gaps are under
correction; the module is not a pass. Dump field and DVS baseline merge
regressions pass. C67 completes the official module with five passed and six
failed tests; the remaining failures are native install/context owner changes.
No SystemServer fatal is observed during the completed run. Typed unrelated user
state rebase and isolated context publication pass regressions and the full
native cohort. C68 progresses through seven tests with one blank install failure
before an asynchronous DVS export of a retired UUID kills SystemServer during
test eight. The run is stopped after stalled recovery and remains partial, not
a completed module. Previous-baseline/no-delta UUID validation now passes three
regressions; C69 adds that fix and owner-specific install exception provenance,
reaches boot completion, and official CtsPackageManagerParsingHostTestCases XML
confirms all eleven tests passed, zero failed and the module completed
(2026.10.08_17.02.43). SystemServer remains alive after the run. Full C CTS/app
acceptance remains incomplete. C70 verifies Calculator update and Chrome install
both return Success, and original libchrome loads directly from its unchanged
APK after ABI preparation. Chrome first-run later hits an input-focus ANR;
launch acknowledgement is not an app pass. Timestamp formatting and
instrumentation CLI are implemented. C71 actual instrumentation list succeeds,
and PreferredActivity now executes device tests but fails preferred-add permission
semantics (zero host tests passed, two failed). PackageSetting actual device
checks of first install time and code path pass, including code path after real
reboot; both host tests fail cleanup because SDK-library removal policy has no
real flag producer. A typed original hidden Flags leaf fixes that gap. Preferred
Context permissions now use authenticated live original AMS checks, including
shell permission delegation. C71 is stopped before the next rebuild.
The original DVS verifier proxy and immutable signer owner fixes pass Java/image
linkage before their next runtime replay. C72 preferred add/delegated permission
checks pass before reboot, but duplicate preferred restoration and removal
cleanup fail. The exact original preferred XML admission rule and library/runtime
removal recompletion now pass native regressions. C73 finds a nonshared UID
incorrectly cast as SharedUserApi; C74 corrects that and identifies strict
settings removal mismatch. C75 shows native query Settings have 42 UID-less APEX
metadata records and transient/shared-UID runtime fields absent from parsed
packages.xml. Shared UID and removal Binder lifetime are fixed; persistent
settings projection now reuses the actual writer/parser with strict persistent
field equality. Its projection/removal regression and actual System DVS disk
fixture pass. C76 reaches boot completion and actual preferred-test APK uninstall
returns Success, with no DVS fatal observed. PreferredActivity full CTS replay
completes with one passed and one failed host test: reboot duplicate state passes,
but uninstall leaves one live preferred entry. PackageSetting device/date/code
checks pass, while cleanup detects canonical removal CAS and postcommit query
publication races. C77 adds actual preferred removal callbacks (including factory
KEEP_DATA), atomic single-file metadata/setting removal, latest canonical query
publication and consistent persistent owner checks. All native tests pass before
replay. C77 reaches reboot preferred verification but an old DVS export after
actual package deletion is rejected as foreign and terminates SystemServer.
Exact unchanged previous-baseline deleted attachments are now handled without
reinserting deleted records; foreign UUID/signature and altered retired fields
remain rejected (six regressions pass). C78 stops before module execution because
installer session recovery checks a missing prepared stage at XML read boundary.
Original readFromXml does not impose that existence check, and successful
nonstaged sessions must be removed from active persisted inventory; both owner
lifetimes are corrected: read-boundary regressions pass six tests, ordinary
root/child success retirement preserves history/allocated IDs and passes its
regression. C79 boots the same failed-history disposable data successfully, with
all native tests passing. PreferredActivity device checks pass before reboot,
but the replay cannot finish: original MediaProvider hits a startup ANR and dies,
while vold retains the FUSE callback device fd during bind setup. Actual sdcard
stat blocks in FUSE read_reply and Tradefed reports external storage unavailable
after 239 seconds. This incomplete invocation is not a module pass. The vold
callback now transfers its original fd and releases its local owner before bind
setup (#1127). The actual broker/duplicated-fd/pending-LOOKUP daemon-close
regression passes (one test, no skips), and the derived image rebuild succeeds.
C80 reaches reboot completion with the actual FUSE storage accessible (17
media_rw-owned directories), and its second preferred host test passes live
addition, state checks and reboot verification. Its first test starts with one
preferred entry retained from the aborted C79 run: original CTS setup installs
without clearing preferences, while teardown deletes the package. The following
initial-zero check passes after that actual deletion. A clean full module replay
completes in C80: official PreferredActivityHost XML records two passed tests,
zero failures and done=true (2026.10.08_19.39.52). Both host tests perform actual
reboot verification and cleanup. Broader 66-module/app acceptance remains
incomplete. Removal persistence lock order is also corrected (#1128): disk is
locked before canonical snapshot publication and released before query callbacks;
the actual four-publication regression passes without skips.
C81 integrates that lock-order correction. PackageSetting device first-install
time and code-path checks pass across both reboots; official XML records two
passes, zero failures and done=true (2026.10.08_19.49.49). Original bugreport
collection finishes after 380 seconds; this was not a proven deadlock. Its duplicate-uninstall warning is the original
host cleanUp followed by automatic APK teardown; the first removal and absent
package assertion pass. The separate WifiUtil postcheck install reports a blank
original exception during committed package-added effects; this remains an open
validation failure. Explicit retained Settings policy, native boot owner and
signing transport gates each pass with zero skips after test owner initialization
and keyed URI-group assertions are brought current. The full original-ART parcel
oracle fixture compilation gaps are corrected (#1130): complete own AIDL and
original compile-only API closure, immutable-cache ownership and duplicate methods
are reconciled. The current compile gate passes; actual ART execution is pending.
C82 adds package-added callback provenance and passes the Java/image build.
An unchanged official WifiUtil install succeeds on the fresh runtime epoch, but
current ParsingHost replay fails its first install: packageAdded cannot find the
committed generated package in the query snapshot (actual NPE at the typed bridge
state lookup). Original DVS subsequently rejects that same package as foreign and
terminates SystemServer. The incomplete invocation is not a current module pass;
the Java cache owner concurrency is corrected (#1129): only same-thread
recursion retains an older graph, independent captures publish monotonically, and
version/capture RPCs run outside the Store monitor. Domain lifecycle admissions
carry exact captured generations and native receipts; unchanged queued removed
records are accepted only until acknowledged absence, with foreign/altered states
still rejected (eight owner regressions pass). The current native service suite
passes 787 tests, zero failures, eight ignored; coherent original image and
updated oracle compilation pass. The full original-ART parcel integration passes
one test, zero failures, zero ignored in 231.46 seconds after genuine reader header
owners are connected in its fixtures. It exercises original modern domain events,
Settings defaults, incremental signatures/permissions, keysets, nullable URI rules,
UUID modes, TimSort and user selections. C83 ParsingHost reaches eight successful
test completions without the earlier snapshot/DVS fatal; official current C83
ParsingHost XML then records 11 passes, zero failures and done=true
(2026.10.08_20.20.45). The explicit original libcore TimSort gate also passes,
one test with zero failures/ignored. A separate Bluetooth component mutation
exposes keyset allocation-counter regression during concurrent installs; that
owner race is corrected with exact unchanged pool/role validation and complete
durable KeySets retention (#1132); its real allocation/retirement regression
passes. Internal graph preparation remains outside publication/disk locks.
Chrome C83 renders FirstRun but still reaches FocusEvent ANR and exits. The
resolveService owner now uses its exact captured native computer endpoint without
eager whole-Java-graph construction (#1131), preserving caller/filter identity
and install candidate scope. Coherent original/native builds pass; actual C84
Chrome and keyset runtime verification are pending. The original domain transport
fixture also seeds persistence from its actual current capture before commits;
its prior stale-capture failure is awaiting a full actual replay.
C85 replaces whole-graph usage revalidation with a sealed validated usage delta
(#1133); ten targeted and 789 aggregate native tests pass. Chrome still reaches
FocusEvent ANR. Its actual SystemServer trace identifies queryIntentReceivers
materializing the whole Java package graph while AMS holds its lock; the next
typed query-only owner correction passes compile. C86 then identifies
filterAppAccess/canQueryPackage/isSameApp/getApplicationInfo full-graph capture
under AMS; those pure DTO/bool paths are converted together to exact native
query captures (#1131), preserving caller identity, aliases and candidate scope.
Actual C87 replay remains pending. The domain fixture is repaired to use its
actual persisted domain state and retain captured uninstall-block ownership;
the exact host-only lifecycle/persistence reproduction passes in 6.64 seconds.
The actual original ART domain transport replay passes 1/0/0 in 213.27s,
including the 4,000-entry original Binder oracle, and keyset transport passes
1/0/0 in 125.27s. The root-only crash child helper is not an independent gate.
All seven meaningful originally excluded library gates have now been explicitly
executed successfully. Actual ART pure-query and mapped-page usage-isolation gates now pass; the
package-add concurrency fixture uses a real external APK installation. C96
exposes the mixed scan/query lease (#1140); the corrected C97 replay passes.
C87 native ParsingHost campaign records 11/0 with module complete; the current
original campaign records Parsing 11/0, Preferred 2/0 and PackageSetting 2/0.
Both stop normally between modules before the next image revision. Chrome
C87 still reaches ANR in full Java package-owner reads. The owner now exposes
a metadata revision inherited only by sealed usage-only snapshots, plus an exact
versioned bulk usage capture. Java reuses immutable metadata while rewrapping
changed active usage and rebinding shared/UID members; detached original settings
and previous snapshots retain isolated usage values. Eleven targeted Rust cases,
full device-services compilation and 790 aggregate native tests pass (nine
ignored in that aggregate, including the separate host reproduction and crash
child helper). Actual original ART query/usage gate passes 1/0/0 in 68.56s: the real mapped
version page and internal usage producer preserve latest/previous usage, detached
settings and transient-state isolation. Latest complete oracle ABI compilation
also passes. C88 quiet Chrome cold launch reaches 2612ms and web launch returns
1915ms, but another ANR occurs, so the app gate remains incomplete. Isolated UID
query-only capture now retains unchanged validated metadata while updating the
query version (eleven owner regressions pass). Existing binder-trace can now
record non-destructive pending/returning nested transactions every 250ms
(#1134, actual nested byte-protocol regression passes). C89 host build and actual live Binder trace capture complete; Chrome remains
unverified because the captured runtime reaches ANR. Package-add concurrency now has a genuine external-install publication seam;
C96 exposes a mixed scan/query lease (#1140), corrected and verified on C97.
C89 trace captures 4,086,703 completed calls over 259.63s; 96.638% are
ScanSnapshot/Computer metadata/lookup calls. The Chrome bindServiceInstance
request is delivered in 1.792us yet waits up to 14.487s inside the service,
without evidence of driver delivery starvation. Base-aware query publication now
compares the complete native owner, normalizing only active usage, and retains
metadata revision only on exact equivalence. Real metadata changes still receive
a new revision and full validation. A full-suite empty-graph regression exposed
usage normalization on unchanged input; that boundary is corrected. Latest
aggregate native tests pass 792/0, ten ignored. C90 still reaches ANR.
C92 bounded creator diagnostics prove both duplicate app-data state publications
and actual per-package component user-state changes trigger whole metadata graph
reconstruction. Duplicate unchanged receipts now retain metadata revision while
persisting/publishing normally (13 regressions pass). Generic active scoped-user
metadata delta compares complete native owners, normalizing only usage/user maps,
and uses unique lease-owned comparison capabilities with strict lineage/lifetime
checks; foreign, expired, factory or nonuser changes use full capture. Current
whole native suite passes 794/0, eleven ignored and full Java builds pass. C93
completed Binder calls fall to about 282,898 over 173.9s versus roughly four
million before; Chrome still ANRs. Its actual main thread waits 3.5335s in
registerReceiverWithFeature during the five-second FocusEvent window. C94 real native runtime scoped-user ART oracle passes at UID1000 using
original public IPM on disposable Calculator: stopped/component changes, old/new
state and code isolation, and expired comparison fallback complete with exit0.
C97 also passes the real shared-UID branch using original Downloads UI
(shared android.media, UID10103): changed members rebind to the current record,
old views stay isolated, comparison expiry holds and original stopped/component
settings are restored. The process exits0 with member_rebound_verified. C94 quiet cold launches take 1.34–1.56s but still reach FocusEvent ANR. C95
uses the same native image with pending/completed Binder tracing: its 8.69s
registerReceiver call begins after the first timeout and therefore does not
establish the cause of that timeout. A no-SIGQUIT pre-timeout host sample of
original Chrome finds 715/1934 main-thread samples in procfs maps, including
682 repeated mount-root canonicalizations. Each procfs maps read now captures
one inverse namespace view and resolves each visible host alias once (#1135),
retaining direct/canonical prefix precedence and observing mount/symlink changes
on new reads; all six VFS regressions pass and the C linux-run build succeeds.
C96 boots the same native image with this lower-layer change. Its new pre-timeout
sample shows procfs maps below 2% of main samples versus about 37% before.
Two early cold attempts still ANR; a later quiet cold launch completes in 1810ms,
stays alive without observed ANR, opens the web URL in 449ms and actually renders
the page (capture checked after dismissing the original Chrome notification prompt).
This establishes the quiet app flow, not early-boot or complete app acceptance;
remaining FUSE/path work and the full CTS/app/template gates are still pending.
The actual package-add concurrency oracle now runs at UID1000 with an unchanged
official CTS APK and catches new canonical metadata paired with an older Computer
(#1140). Query-bound capture now retains the published query owner's own scan;
raw boot metadata capture remains separate. A real System/ServiceHost Binder
regression passes across canonical-before-query publication. Installation publishes
the APK but its subsequent bound-session close fails after registry retirement;
live Binder session capabilities now retain the actual terminal owner/count
through registry retirement with weak indexing and last-capability cleanup;
two focused regressions pass. C97 actual original APK installation returns
Success with exit0 and no post-commit cleanup exception; the original ART
package-added concurrency oracle exits0, proving retained old reentry, independent
new installed capture and monotonic publication. The shared-UID oracle also
exits0 with member_rebound_verified and restores original user settings. A separate actual FUSE path-walk
LOOKUP reference leak is corrected (#1139); ten real protocol regressions pass,
with C97 runtime integration complete. A fresh frozen native 66-module campaign
starts in separate disposable data alongside the original campaign. The first
fresh native Parsing attempt records 7/4, including an early invocation before
boot completion and real subsequent failures after SystemServer watchdog/keyset
divergence. It stops at the module boundary with XML/logs/dumps preserved; it
is not acceptance evidence. Fresh-template native CPU samples show repeated
grantImplicitAccess AppsFilter construction inside the bootstrap mutex while
AMS holds its monitor. Typed visibility-only capture now retains validated
metadata and its static resolver relations. Resolution/preparation run outside
the bootstrap mutex, with epoch/base checks before publication; 17 visibility
regressions and two strict fast/full parity cases pass. Fresh runtime replay is
pending. The keyset mismatch comes from native installation
persisting canonical metadata before publishing queries while the metadata
actor uses the previous query owner. A typed installation permit now serializes
durable/canonical commit through query publication with metadata mutations,
including compressed activation without recursively locking the same gate;
original callbacks run after release (#1142). Real SPKI/durable publication,
already-held permit, foreign permit rejection and callback release regression
passes; strict pool/role checks remain. Latest full native suite passes 799/0,
eleven ignored, and the C host build passes. A new fresh native cohort verifies
boot_completed and both package services before invoking CTS; Parsing completes with official XML 11/0; PreferredActivity is running,
including a real reboot, without the previous watchdog/keyset crash observed
so far. The full 66-module result is still incomplete. C98 separately passes a newly authored ordinary WebView consumer:
the original com.google.android.webview 133.0.6943.137/694313732 provider completes
relro 1/1, starts its renderer, returns the exact JavaScript DOM marker and renders
local HTML/SVG in an inspected capture. Reproducible probe source is in
tools/webview-probe; originals remain unchanged. Settings storage/default-app and
Chromium app-info screens render. C107 with the pinned instant-view
permissions/HOME/AppPrediction policy (#987) and the original SUSPEND_APPS plus
other-user-only FULL cross-user rule (#1145) renders the original app-permission
screen (Calculator notifications listed, 2650ms cold launch) and Wellbeing
dashboard (704ms warm launch); foreground activity receipts and inspected
screenshots confirm both. Later isolated Chromium process-group creation kills
SystemServer (#1153), so this is bounded UI evidence, not full app acceptance.
The frozen original CTS baseline also finds
actual cross-app file access (#1143) and missing procfs network tables (#1144);
those lower owners are being corrected separately with original results preserved.
The frozen native campaign completes Parsing 11/0 and PreferredActivity 2/0,
then a real Setting reboot repeatedly fails interpreter resolution (#1146).
Failed/incomplete and device-offline module records are preserved; the runner
stops at a module boundary. Bounded diagnostics locate Darwin EPERM on the mount
journal's existing-file write-open, not ELF absence or flock. Subsequent controls
contradict a global write-denial interpretation: native C/Rust helpers can open
the same journal, and linux-run's entry/initialization probes can create,
write-reopen and delete control files, while its journal open fails before ABI
initialization. Flags, variadic arguments and a sandbox denial are not proven
causes. The exact process/file-dependent condition remains unresolved. An independent real-signal errno-preservation defect is fixed
with a two-delivered-signal regression (1/0/0); EPERM still reproduces after it,
so this is not claimed as the reboot fix. Diagnostics leave no owned processes
or temporary files behind.
Current Source lower-owner cohort passes 174 ABI tests (four ignored), 805 PM
library tests (eleven ignored) and 65 complete Binder tests. Guest DAC now
checks ancestor search, final access, owner/groups/capabilities and checked
metadata persistence; metadata mutations use hidden real inode anchors and a
cross-process inode lock (#1143). Anonymous memfd metadata is identified by its
actual inode/seal owner rather than a diagnostic host pathname. C105 exposed
the old classification as EROFS during gralloc ftruncate before SurfaceFlinger
could start. The regression verifies actual memfd resize, shared mmap and
write/readback, ordinary unmapped FD resize, and retained image EROFS. Current
C107 reaches boot_completed=1 with original SystemServer and both native package
service names. The syscall owner binds current-process procfs FD magic links to
a retained descriptor, matching original bionic's O_PATH chmod fallback (#1150).
The focused regression verifies UID ownership, rename retention, hidden/closed
FDs and symlink errors. Later original SystemServer fails lchown of
/sys/fs/cgroup/apps/uid_90000/. with EPERM while creating an isolated Chromium
process, then dies with ProcessList's createProcessGroup assertion (#1153).
Original processgroup reads the controller root /sys/fs/cgroup, not /apps;
our SetupCgroups had applied cgroups.json ownership only to its children. The
mounted root therefore retained0:0/0755, causing a1000:1000 SystemServer without
CAP_CHOWN to request0:0 on its1000:1000 child. C108 preserves that exact denial
and actual SystemServer credentials. SetupCgroups now applies the configured
mode/UID/GID to the same mounted root before children, as original
MountV2CgroupController does. A real inode regression verifies root identity,
root-before-child ordering, the configured 1000:1000/0775 and an alternate tuple.
No stat, DAC or capability guard is relaxed. C109 actually confirms root 1000:1000/0775, keeps the original WebView isolated
process alive, completes relro1/1 and DOM verification, and renders inspected
HTML/SVG. The unchanged immutable runtime pair then completes two real guest
reboots (SystemServer 42633→43583→45430); both return boot_completed=1 and the
original native service names, with no interpreter EPERM or process-group fatal.
WebView DOM verification passes again after the first reboot. After the second,
Calculator cold launch 1663ms renders its actual UI; Chrome cold launch 2624ms
reaches first-run, continues without an account, and renders example.com with its
sandboxed process alive and no observed ANR during this bounded replay. The
browser reports offline network validation, so no general network pass is claimed.
This does not establish the fresh-first-boot Chrome gate. Debug boots and helpers
are reaped; diagnostic logging is removed. The C109 fresh native full 66 campaign
used this exact immutable host pair, explicit --linux-run through Tradefed,
pre-spawn hashes and actual readiness before invocation. Full CTS remains
incomplete; the old cohorts' failures and missing tests are retained. The C109 fresh
campaign's first invocation cannot allocate a device because original adbd
exec-out fails opening /dev/pts/3 (#1154). It records NOT RUN with no XML;
its runner, wrapper, guest and display are normally stopped. Runtime preparation
now creates the actual devpts namespace directory, preserving DAC traversal and
real terminal identity. Two real PTY/runtime-directory regressions pass. Fresh C110 reaches genuine
boot readiness, but five exec-out getprop probes return an error payload with
Permission denied opening /dev/pts/3, despite exit0. There are no property rows,
so the content gate fails and no CTS invocation/XML is created. The missing
ancestor is fixed, while actual slave allocation ownership/mode remains under
#1154 investigation. Its guest/display/ADB are normally stopped, with data and
provenance preserved. PTMX allocation now records the actual allocator filesystem
UID/GID and Linux default0600 mode against the real devfs inode/generation/birth
identity. Guest chmod/chown and repeated unlock/grantpt preserve that binding;
a reused slave allocation receives its new owner's state. The genuine shellUID2000
open/foreign-denial/reuse/durable-error regression passes. This does not count as
full CTS acceptance. C111 now completes actual adbd exec-out getprop on its first
probe: exit0,27949 bytes,660 parsed property rows,SDK36 and boot_completed1,
with no PTY error. Its fresh full66 campaign allocates the device and starts the
first official Parsing module completes with actual XML:11 passed,0 failed,
done=true, SHA86d5e8020002161d5fa7d50b84582f7337acaa0e0a78fc0cb4f76d401aba8cdb.
PreferredActivity also completes with official XML2/0, done=true, after two
real Tradefed reboots; SHA0f6827de97f9eb7c4c51ce432046ca0a52ad79af3d64ba9b2c1f3f712e80e18b.
Setting also completes with official XML2/0, done=true,
SHA5c278472e8b4437ca2273afb0b55f041199b0b564ca894d7f43c92089f90cfd1.
Its second body has a device-unavailable detection warning, so only observed
reboots are claimed. C111 PackageManagerHost now retains complete official XML:
134 bodies end99 pass/33 fail/2 assumptions, SHA
92c48ae689967dfaabdea2603fea5746b5d65ce312a0ca1a7e92d85645f7d65e.
All33 failed cases pass in the original exact-case baseline; both assumptions
match original outcomes. Source wrapper edits during its Bash wait invalidate
the invocation lifetime and produce exit127 (#1178), so this is retained body
evidence, not clean acceptance. The cohort stops normally after this module;
owned guest/display/runner and image/data leases are absent. Its restricted permission tests reveal missing native runtime permission dump
rows before mutation assertions (#1061). Native dump now reads the existing
original permission owner for live granted/denied/flags, revalidates native
UID/code/shared-user/user-inventory/install identity, and emits the pinned
runtime permission format once per shared group. Eight owned regressions pass;
actual affected CTS replay remains pending. No permission state is synthesized
or changed to make the diagnostic pass. PackageManagerHost observes unknown get-max-users in cross-user test bodies
(#1163), so those fast-ended bodies are not counted as full coverage before XML.
Native shell now calls the original UserManager.getMaxSupportedUsers through
its authenticated typed leaf and prints the exact original prefix. Actual Binder
CLI output/exception/tail tests and complete private Java original linkage/stub
code checks pass; matched image deployment remains pending and C111 is unchanged.
The same frozen PMHost cohort exposes activity filter priority and public
restrict-update error mismatches (#1169/#1170). Native activity registration now
applies the pinned privilege/protected-action/setup-wizard and factory-filter
priority rules; native install stages retain original public failure text and
status while logging their own stage diagnostics separately. The complete PM
library suite passes 815/0 with eleven ignored; matched-image CTS replay remains
pending.
C111 live feature resource and replacement code-cache cases expose missing
post-install replacement lifecycle (#1177/#1179). Native installation now
captures prior visibility before replacement, classifies final installed users
against original installed-or-data state, clears actual code cache/profiles for
replacement, completes ART, then delivers original replacement/new broadcasts
and code cleanup with dont-kill retention. Captured code/signers/UID/user owners
are revalidated; failed ART completion retires the plan without broadcasts.
The focused lifecycle/user regression and private javac/D8/original linkage
plus generated transaction24/25 checks pass. Actual affected CTS replay is pending.
Five C111 UseProcess cases accept packages whose application/component process
is absent from a nonempty declared process map (#1180); all exact original cases
pass. Native scan now validates application and activity/service/receiver/provider
process keys in the pinned order before live UID mutation, and during image/data
scan with their existing rejection cleanup. Null/empty process maps retain their
original allowance. The pinned DEX status -122 and public failure text are kept;
two focused policy/atomic-batch checks pass without skips. CTS replay is pending.
The same official base-only split test exposes an unsupported query branch
for activities declared in absent dynamic splits (#1176). Native post-resolution
filtering now follows the original captured installer/drop policy, constructing
typed auxiliary failure/package/version/split metadata when an installer exists
and applying original label/icon selection before visibility filtering. Three
focused cases and the complete PM library suite pass826/0 with eleven ignored;
actual affected CTS replay remains pending.
C111 required split-type removal and required-base tests accept invalid final
inventories (#1174), while malformed type syntax loses its original parse status
(#1175). Native install now validates actual submitted/retained split requirements
and providers before inheritance copies; inherited bases contribute requirements
but not provided types. Missing splits retain legacy -28, malformed type syntax
retains MANIFEST_MALFORMED -108 through typed storage/native failure mapping.
Thirteen focused split regressions pass, including unchanged official APKs and
real inherited-copy admission. Actual affected CTS replay remains pending.
C111 first-valid shared UID install also fails before the intended foreign-cert
negative phase because candidate process-group assignments retain the old group
inventory (#1173); the original exact test passes. Candidate runtime completion
now rebuilds ordered process groups from actual finalized native member owners
transactionally before Store validation. Three focused checks pass without skips:
first/second null process maps, real-DER shared signer admission and foreign-cert
atomic rejection, and candidate validation/failure atomicity. Actual CTS replay
remains pending.
C111 also exposes retained updated-system-app state after factory restore rejects
an APK inside APEX (#1171). Native restore now resolves the actual original APEX
inventory before parsing, retaining its partition, application privilege and
PARSE_APK_IN_APEX policy. Location/parser and sequential update-to-factory owner
regressions both pass; actual affected CTS replay remains pending. The
old failed C109/C110 cohorts remain NOT RUN evidence. A separate rollback on
a clone of native-written data constructs original PMS (package owner PID equals
SystemServer 50573), preserving five selected packages' UID/codepath/version,
persisted signatures and runtime permissions; Calculator, WebView and Chrome
render. Strict component parity is incomplete because native internal Version
metadata is 0/0 with missing fingerprints, making original PMS apply its pre-Q
AppDetails migration (#1157). Native boot now retains the actual current
VersionInfo and commits SDK/database versions and original fingerprint updates
at boot completion; pre-Q component migration follows the retained original
lifecycle predicate for non-system user0 only. Actual persistence/reload and
build-only-change regressions pass, and the combined PM library suite passes
803/0 with eleven ignored. C110 actual rollback on a clone constructs original PMS and preserves both
Version rows plus the selected five packages' UID/codepath/version/signatures,
permission maps and component/restriction state. The prior AppDetails delta is
gone; representative Calculator/WebView/Chrome rendering passes. Its owned
processes and attachments are cleaned. This proves selected rollback parity,
not all-package parity; fresh empty-data template fingerprints expose a separate first-scan handoff:
readLPw's forceCurrent Version owners are discarded when a new APK scan owner
replaces prepared settings. The first-scan owner now retains those authoritative internal/external Version
rows. A genuine absent-file recovery→first scan→initial restriction claim→real
commit/reopen regression passes, with fingerprints and the next lifecycle flags
preserved. Fresh template runtime replay remains pending; the earlier
existing-data rollback does not cover this path. A standalone isolated template command now
uses explicit existing image/immutable-host/display/empty inputs and an exclusive
new output; it holds the image read lease, records hashes before boot, refuses
protected-path overlap and checks drift before publication. Focused CLI isolation
passes; the two native template builds and structural/runtime/BootStats parity
remain incomplete. The first genuine empty-data C110 template boot reaches
boot_completed at131.94s and captures original permission state at127.67s,
but times out at300s waiting for the settings shipped set because packages.list
is absent (#1160). The failed output/raw boot and input provenance are retained; no
published template is counted. A second identical build waits for the constructor
output correction, then two completed builds still must satisfy the original
structure and runtime parity gates. Donor/clone cleanup is verified and the clone is
retained. GMS idle traffic is not accepted: the fresh native boot also records a
CrisisAlerts persistent-process fatal (#1155), under independent investigation. The frozen original full66 campaign stops after module16 with all sixteen
retained XML hashes verified:428 pass,84 fail,51 assumption and3 ignored; fifty
modules are uninvoked. Ten campaign rows are recorded, not accepted/pass counts.
All fifteen test-executing invocations retain SELinux/thread checker failures;
clean invocation acceptance is zero. Shortcut completes9 pass/3 ignored but its
wrapper exits127 after an in-place source script edit during Bash wait (#1178).
Official body evidence remains intact; the original guest/display/logcat/runner
exit normally and its image lease is released. Future campaigns now capture detached immutable wrapper/helper/harness script
generations, validate hashes, modes and external JAR targets before each launch,
and preserve exact bundle/command provenance. Resume rejects changed generations;
fresh output directories remain exclusive. Three lifetime/resume/output checks,
six comparison checks and four host selection checks pass. Actual next-cohort
invocation acceptance remains pending.
Current Source has no aggregate
cargo aim test --integration result. That command builds the complete graph,
then runs normal test binaries; 49 ignored integration tests (48 service tests
and one property ABI test) and ten explicit original ART/PM library gates
require separate invocation. The two HAL client inputs in #1156 are now built
in Source through their graph owners and verified as AArch64 ELF; their actual
runtime gates remain not run. Historical Main logs and old skip markers do not
prove this cohort ran. Full
integration waits for frozen build/image leases; no missing-input skip is counted
as executed. The standalone service-AIDL generation command is verified in
the isolated C worktree: existing getUserIds code9 is unchanged and the appended
active-user leaf is generated as code10 with typed request/reply helpers. Source
shared generated SHA remains unchanged. This is contract preparation only; the
new Java leaf still needs a coherent image build before runtime deployment. The private complete Java service DEX and
stub DEX now pass the original boot/systemserver linkage checker and generated
own-AIDL transaction-code verifier through the read-only check-service-java CLI.
No image or graph build occurred; this is contract proof, not runtime deployment. The final constructor contract is now registered in
Source: generated codes9/10/11 agree with the complete private Java DEX and
original linkage. Actual package rows/GIDs are prepared before taking the disk
lock, original active/runtime user inventories are retained and revalidated,
then packages.list is committed and every actual runtime user is queued before
the worker starts. Real list ownership/persistence and permission main/reserve
write regressions pass; coherent Source compilation also passes. The Java leaf
and host caller require a matched image rebuild, so the current C111 cohort
continues using its frozen prior contract and is not deployment evidence. The registered-constructor PM library rerun now
passes805/0 with eleven ignored. A fixture's one-shot publication gate try_lock
had falsely rejected a concurrent legitimate writer; its bounded actual lock
acquisition retains keyset/query ordering and catches a retained installer permit
(#1164). No production publication behavior is relaxed.
Inherited capability refresh now pins each open description and revalidates its actual kernel endpoint before managed
FD publication; disappearing descriptors cannot abort binder initialization or
grant a class to a reused number (#1152). The concurrent replacement regression
and full ABI run pass: 174/0, four ignored. The frozen original
Install CTS preparation exposes a FUSE notification backpressure deadlock
(#1151): broker send holds its connection mutex while daemon reads wait on it.
The owner now coalesces level notifications, sends outside that mutex, and
uses an explicitly nonblocking notification socket with a poll-based lifetime
wait. Draining precedes packet acknowledgement and queued requests re-arm
readiness. Eleven ACK/clone/death/abort regressions and the actual full-buffer
backpressure test pass. C107 actually returns mounted FUSE statistics from
df -k /sdcard; this is not a complete Install CTS rerun. Path-only descriptor capabilities preserve
actual backing identity and ARM64 flags through SCM/Binder; guessed hidden
kernel descriptors fail as nonexistent guest FDs. Typed import failure rejects
the actual delivered transaction, closes descriptors and releases its buffer,
including nested and oneway work (#1147, matching host/runtime wire version 3).
Exec preserves projected-mount identity for future parent events (#1148).
The actual translation-cache test uses a fresh process so unrelated tests cannot
preinitialize its process-wide runtime (#1149); its original execution assertions
remain. Procnet exposes actual registered guest socket identities/endpoints,
but unavailable TCP timer epoch/probe metrics return a real unsupported error;
#1144 remains incomplete and no full network-table CTS pass is claimed. A real isolated
public-ABI integration test passes for IPv4/IPv6 TCP listening and connected
traffic with supported inactive timers, and UDP bind/connected traffic. It also
verifies that an unsupported TCP keepalive state returns EOPNOTSUPP without
hiding the unrelated UDP inventory. The official seven baseline failures occur
at the host file-pull assertion; this integration result is not their CTS replay.
The regular descriptor Binder transport now retains backing and existing writer
lease fileports in the same driver file owner, with wire version4 and explicit
Path/Regular classes. Native extracted descriptors retain that original owner
through their lifetime and forwarding. Thirty-five host regressions pass, including
actual Binder reply-buffer and immediate final-reference writer release; the PM
library suite passes829/0 with eleven ignored. Matching ABI installation/close,
publication and a new coherent runtime remain required; C111 used wire3.
A current-source guest fixture now runs with the actual authenticated native
coordinator and strict fork Ready ACK. Its owned protected ELF includes explicit
TLS instructions, is asserted to translate, and produces authenticated derivative
page-cache entries. Actual translated execution, pre/post-fork TLS, private COW,
SIGBUS/address, data/BSS and corrupt-original EIO checks pass. The current ABI
also compiles across all targets, including test and executable initializers.
The owned protected fixture declaring 4KiB alignment now retains valid
16KiB-congruent load offsets and separate code/RELRO pages; actual original
linker execution passes TLS, private COW, fork, SIGBUS and authenticated composed
page checks. Its former packed main-executable layout is rejected with EINVAL:
the RX load offset is not 16KiB aligned, and the original linker would remove
EXEC while protecting the overlapping RELRO page. Genuine packed 4KiB DSO
compatibility remains unrun. Kernel user-buffer copies now resolve cold verified
pages before Mach copies; three real memory/proof/error checks pass. These
results do not establish image CTS/app acceptance.
The in-progress pager is now exercised by an actual NDK/original-bionic guest:
normal mmap, closed source FD, private COW, growth/move, protection changes,
MADV_DONTNEED revalidation, corrupt-page/EOF SIGBUS with exact address, shrink
and fork COW isolation pass in the bounded fixture. These use genuine host-built
Merkle proofs. A real native ENABLE issuer now performs authenticated remote
prepare, durable publication and queries, with verified reads, writable-open
refusal and repeated-ENABLE rejection. The same fixture retains an ioctl-only
mode3 descriptor through ENABLE, verifies logical access flags, and rejects
read/write/truncate/allocation/mmap access. The complete new image/CTS replay and
native writer coverage remain incomplete.
Five native SCM owner checks now pass, including actual Client/server Mach
create/resolve/drain RPCs, kernel-queued rights, bounded reply failure and final
writer release. The authenticated one-descriptor carrier uses synchronous EOF
drain; no worker-held lease remains beyond that admission point. New control
commands retain IOCTL wire4; ABI consumer/admission integration is pending.
A real SCM syscall fixture also verifies one-FD carrier queue/reexport/discard,
actual writer lifetime and read-only access, with explicit synchronous drain
before admission. Automatic production admission and coherent deployment remain
required; neither result substitutes for the complete fs-verity gate.
Prepared inode proofs now expose a complete authenticated read-only metadata
view before publication, and commit publishes that same blob inode without a
header/tree rebuild. Seven storage checks pass, including pre-publication reads,
same-inode commit, abort cleanup, corruption and directory-sync rollback. This
supports mapping transition transactions. Actual authenticated remote transitions
pass two-pager preparation/publication, unpublished-receipt rejection and failure
after durable publication. Production startup now joins the real data mount before
the first guest, constructs the same persistent proof store, and starts both the
native coordinator and process worker with that store. The guest-init compile
check passes; this startup ordering has not yet been booted. Corrupt or mismatched
metadata recovery now quarantines prepared mappings instead of restoring
unverified pages. Two actual subprocess checks pass for corrupt and valid but
mismatched metadata after EOF: dirty and clean mappings both resolve to SIGBUS,
and each worker exits within its three-second bound. A separate pager check
confirms retained private COW bytes through a private Mach alias, blocked guest
access, bounded waiter wakeup and released VM epoch. Separate inode leases now
let read-only ENABLE callers share write denial while competing for the distinct
ENABLE slot; eight primitive tests pass with writer/alias/SCM lifetime checks.
Native ENABLE dispatch is implemented and its real issuer checks pass. The
complete image/CTS gate remains pending.
Native POSIX lock startup now has an explicit `host/aim-lock-holder` dependency
under `host/guest-init`; isolated template inputs require and hash that same
helper beside the frozen linux-run/guest-init binaries. The actual build graph
loads both nodes and compile-checks successfully; the explicit helper build
completes in 3.0 seconds with no failures. The Source release helper exists,
and the actual frozen common runtime now includes that helper with verified hashes (#1193).
The initial coherent image build completed 27 nodes but boot-image/oat workers
failed to execute dex2oat/profman with ENOENT. Their ordinary scratch path-map
has neither a proof-store locator nor a native coordinator owner. Optional
configuration now applies only when both entries are absent; declared, dangling
or malformed stores remain errors. Its focused owner-configuration check passes,
and actual unchanged profman profile extraction exits successfully with 1,078
output lines. The original-PMS derived image rebuild then completes with four
rebuilt nodes, 29 fresh nodes and no failures (51.5 seconds). Native-C image
staging uses the same committed source and retains its explicit native service activation.
Its build completes with 29 rebuilt nodes, four fresh nodes and no failures
(123.2 seconds). Both images have verified overlay receipts and share a detached,
read-only common host runtime. The first empty-data native template stops before
guest launch because its long runtime control socket exceeds Darwin SUN_LEN.
A short-output replay passes that setup, then stops during init signal collection:
Darwin returns EINVAL even for read-only sigaction queries on SIGKILL/SIGSTOP
(#1200). These uncatchable dispositions are now excluded, and all 26 guest-init library
checks pass. A fresh short-path diagnostic replay publishes native init PID
readiness and starts eight original guest services, then times out after 300
seconds with settings, permission capture and boot completion all absent. An
actual read-only vdc trace waits for servicemanager.ready; all owned boot/display
processes are gone after the failed template. The separate long-runtime
control owner now allocates a retained private short native directory (#1201);
authenticated attach, admission, real ENABLE publication and explicit/Drop
cleanup pass against a runtime path beyond SUN_LEN. A separate bounded syscall-
traced boot reaches zygote-start and launches 26 guest services, but an untraced
readiness failure remains unresolved (#1203); tracing is not an acceptance fix.
Its processes are reaped. The coherent corrected host rebuild completes five
nodes with four fresh nodes and no failures in 63.1 seconds. A detached common
runtime records all five actual binary hashes. Its fresh long-output native
template still times out at 300 seconds before readiness; no terminal socket
cleanup errors occur. Short raw runs can progress without syscall tracing, so
trace alone is not the distinguishing cause. Their multiple complete/partial
boot reports are consistent with native init's explicit same-PID reboot exec.
The CLI now reports the actual reboot request and fatal reason even in quiet
mode; the new runtime replay must identify that request (#1203). No template
is published or counted from these diagnostics. Darwin terminal socket shutdown can return ENOTCONN
without discarding queued rights (#1202). The native owner now drains actual
recvmsg rights to EOF under private descriptor admission, with bounded retries
only before consumption. The five-by-1,000 concurrent close regression passes;
the full storage suite passes 53/0 with three ignored and the host suite 50/0.
The actual original setprop property transport exposes a separate generic socket
I/O regression (#1204): connect succeeds, then writev returns EBADF. File I/O
had already captured a guest-authorized pin but passed its private alias through
a second guest-FD admission. Socket read/write now borrow that retained pin,
without publishing private aliases. Two actual scalar/vector/offset-minus-one
and close/reuse checks pass, including hidden-FD rejection; the full default-
parallel ABI suite passes 234/0 with thirteen ignored. The rebuilt immutable runtime passes the original, untraced setprop/getprop
and bionic serial proof at both short and deep socket paths: real peer
PID/UID, publication, response and file/guest values/serials agree. A repeat
against the immutable `aee732eb` runtime passes both 82-byte and 281-byte
paths (one actual integration test, 1.61 seconds). The fresh
long-output native template now advances to the BPF stage but repeatedly
requests reboot,bpfloader-failed (#1206). The original netbpfload launches
successfully, then aborts via bionic fdsan when a returned BPF object FD is
rejected by generic close as EBADF. A bounded original trace verifies OBJ_GET
returns FD11, no intervening close, then close(11) fails and fdsan reports
double-close. BPF FD producers lacked guest publication. The owner now initializes map,
program and BTF objects privately, then publishes a new CLOEXEC guest duplicate
under lifecycle admission. Input object commands retain the admitted pin, and
invalid info buffers return EFAULT. Three focused object/pin/reopen/close and
error-cleanup checks pass. A concurrent raw rights receipt intentionally lacking
CLOEXEC is tested in a fresh kernel process, preserving the actual EOF assertion
without unrelated helper inheritance. The coherent default-parallel ABI suite
passes 235/0 with thirteen ignored. The rebuilt original netbpfload completes its load stage: actual
bpf.progs_loaded is 1 and original loader logs contain the pinned maps/programs.
The fresh empty-data native boot launches 69 services without the former BPF
reboot, but reaches 300 seconds with no settings capture, permission capture or
sys.boot_completed. The captured zygote SIGKILLs follow the original restart chain: composer
exits 1, its onrestart restarts SurfaceFlinger, and that onrestart restarts
zygote (#1210). Actual managed compositor syscall logs show display event
read(fd4,32) returns EBADF after a successful display handshake and service
registration. The host-call event FD had no guest publication. Its owner now
retains the native connection privately, allocates real socket identity at
creation, and publishes only a new guest duplicate. A real native HELLO/WINDOWS
proof passes unchanged 32-byte events through guest read/poll/stat/close and
rejects the private original. Standalone original zygote using copied boot
linker configuration and classpaths also reaches ART threads and remains live
through its 15-second diagnostic bound; this is not native boot acceptance.
The memory watcher (#1211) and Bluetooth wake (#1212) had the same missing
publication pattern. Both now use private endpoints and lifecycle-protected
guest handoff, including fork-child hook setup. Actual guest pipe IO/private
denial checks pass for memory and an injected Bluetooth backend (no physical
radio claim). The coherent default-parallel ABI suite passes 238/0 with thirteen
ignored. The rebuilt managed composer remains connected with native vsync delivery;
zygote completes 18,367-class preload and accepts command sockets. SystemServer
fork then fails with Broken pipe (#1213). A real 257-inode owner probe reproduces
that error before any child spawn: the old per-inode fork lease opens more than
the admission listener's 256-worker cap. Fork admission now sends one typed vector
on one authenticated connection and reserves the whole set atomically. Real
257-inode abort/EOF, malformed vector, complete-child handoff and missing-one-
child rejection checks pass. The default-parallel suites pass storage 57/0 with
three ignored and ABI 238/0 with thirteen ignored. Final-close lease fixtures
run in fresh processes to exclude unrelated raw-fork aliases while retaining
the original actual lock/close assertions. The rebuilt batch protocol runtime
(`aee732eb`) launches 69 services in a fresh native empty-data template attempt,
but reaches the 300-second bound without settings/permission captures or
`sys.boot_completed`. Its zygote log reports failed fork descriptor inheritance
for descriptors starting at 10240. A separate managed repetition captures
repeated netd aborts in the original `DnsProxyListener` constructor before the
original BPF loader publishes its configuration map (#1214). The replacement
netd now waits for the real `bpf.progs_loaded=1` property before original DNS
resolver initialization, restoring the pinned startup dependency. Android
compilation and two readiness-loop checks pass; actual resolver startup with
this change remains unverified. The managed repetition does spawn a SystemServer child, which reaches original
zygote JNI specialization and aborts when `createProcessGroup` finds no
initialized cgroup controller and tries `/system/uid_1000` on the read-only
system filesystem. The original libprocessgroup first fails to read
`/proc/mounts`: procfs symlink resolution omitted the dynamic mount-table node.
That owner now resolves the link through the actual process mount namespace;
a focused test verifies equality with `/proc/self/mounts`, live mount addition
and removal, unchanged readlink and write rejection (#1215). The committed
`6ba935d0` C snapshot builds coherently (22 built, 11 fresh, zero failed;
57.4 seconds). A new disposable managed boot reaches `sys.boot_completed=1`
and original SystemServer boot phase 1000. Original shell reads both mounts
paths with the actual cgroup2 entry; `netd` is found after real BPF readiness.
`dumpsys --pid package` and `package_native` return native init PID 1, while
`activity` returns original SystemServer PID 43196. This isolated C diagnostic
runtime is not the final matched CTS cohort. GMS persistent reports an ANR
after boot. A cold original Settings launch returns `Status: timeout` after
21.284 seconds; the original ActivityTaskManager subsequently reports it
displayed and fully drawn after 23.598 seconds. Media and Google companion
processes also report ANRs. This is observed rendering, not a passing bounded
launch or functional app acceptance. A second fresh managed C diagnostic with
the existing Binder trace enabled reaches boot completion and cold-launches
Settings with `Status: ok` (TotalTime 14.291 seconds, WaitTime 14.294 seconds).
Its actual transaction graph attributes the 15-second startup lifecycle wait
to the delivered original Java dexopt callback; sampled native PM delivery is
sub-millisecond rather than a saturated native queue. This does not establish
idle GMS stability, functional Settings checks, comparative performance,
matched CTS or template acceptance. Both owned diagnostic boots and displays
terminate and are reaped normally. Fork inheritance now captures only canonical
guest descriptors and explicit private owner receipts instead of enumerating
every host descriptor. Spawn aliases stay below Darwin OPEN_MAX; high private
targets are restored before runtime allocation, with fallible actions and
rollback. Event/timer/inotify, ptrace, sync-fence and vfork private endpoints
retain their actual owners; closing them clears visibility before slot reuse.
An inotify watch now owns and closes its watch/entry descriptors on Drop.
Two actual spawned-child tests verify private high-slot IO, guest flags/close,
event/timer/inotify behavior, pending fences, closed-slot reuse and vfork parent
blocking until real child exec/exit. Both parents verify child completion
receipts. The coherent default-parallel ABI suite passes 241/0 with fifteen
ignored, including two controlled helpers executed by their parent tests.
The rebuilt runtime and full matched acceptance remain pending. Original Perfetto traced_probes
asserts after a stdout pipe close returns EBADF (#1207); both standalone and
managed syscall-traced repetitions succeed, and a real active-POSIX-holder
buffered pipe EOF/close proof passes, so POSIX presence alone is not the cause.
Native accepted-then-closed verity transport sockets produce EINVAL while setting
timeouts before peer authentication (#1208). Authentication now precedes setup,
with actual ENOTCONN for a disconnected peer and no admission-policy relaxation.
Closed-peer and authenticated transfer checks pass; storage passes 54/0 with
three ignored. These transport proofs do not establish the zygote failure cause.
Broader BPF execution/hook gaps remain #202. Full image/CTS acceptance remains pending.
Darwin GC also flushes message-only socket references behind live unregistered
receive queues under concurrency (#1191). The class5 carrier now uses a regular
inode/OFD lease; the native backing is retained exactly while that lease remains
in an actual queued/imported descriptor. Completed synchronous drain releases
retired backing. Native host tests pass 50/0, and the focused guest syscall
fixture passes real 4-by-4 concurrent init/refresh, I/O, reexport and immediate
EOF after queued discard. The full ABI default-parallel suite passes 231/0
with thirteen ignored definitions. Each invoked subprocess helper verifies its
actual execution; unexecuted ignored definitions remain not run.
Private SCM receivers and Mach fileport conversions now set CLOEXEC before
retaining their descriptors; real exec-child checks prove final backing/writer
release while that child is still alive (#1194). The source host all-targets
compile check passes. The storage default-parallel suite passes 50/0 with three ignored definitions;
controlled helpers are checked for actual execution. Raw-fork ownership fixtures
create their resources in separate exec processes so unrelated tests cannot
inherit socket/lease references. No test-suite serialization was used.
Concurrent verified reads exposed native lock-node openat(O_CREAT) returning
ENOENT during simultaneous opens of a retained existing directory (#1186).
Atomic existing-open/create-exclusive admission with a bounded EEXIST reopen
fixes the actual race; seven primitive checks and the strict four-thread
duplicate verified-read/position test pass without skips. No filesystem error
is swallowed or converted to successful admission.
Original AppSecurity retains thirty fs-verity ENOTTY failures (#226). A shared
storage core now streams Merkle construction, measures from a fixed header,
authenticates returned bytes and binds durable metadata to actual inode
incarnation. Nine focused checks plus a real short-read temporary cleanup
regression pass; five actual writer/enable/alias/fork/crash/SCM lease checks pass.
This core does not expose ENABLE ioctl success. Guest FD/Binder carriers, mmap
page verification, native writer admission, signature policy and private descriptor
lifetimes must be connected before activation or CTS acceptance. Shared private
descriptor wrappers now enter owner serialization before allocation and close the
actual FD before releasing its hidden role; registered carrier extraction cannot
detach that role. One genuine isolated lifecycle test and five lease regressions
pass without skips. ABI startup registration and complete descriptor publication/
pinning remain required; callbacks alone do not protect raw syscall races. Native init now
records a versioned locator for persistent inode proofs outside mapped /data; a
real disposable-layout test verifies runtime reset preserves the store, custom
runtime layout resolves it correctly, and foreign symlinks are rejected.
The original staged-install baseline also cannot execute its Linux x86_64
deapexer helpers on Darwin (#1166). A nonboot build node now produces pinned
original AOSP deapexer/protobuf with real Darwin arm64 debugfs and fsck.erofs,
using isolated pinned source copies and statically linked compression libraries.
Actual ext4 and EROFS LZ4/LZMA/ZSTD/deflate extraction preserves content, modes
and symlinks; corrupt inputs reject, and an unchanged original APEX extracts
successfully. Provenance and validation receipts retain input/output hashes.
The pinned original Tradefed module metadata option selects a detached Darwin
host-tool ZIP through its actual dependency resolver; a real Java fixture and
selected-bundle original APEX extraction pass. CLI selection records component,
Python runtime, original dependency and selected dependency hashes. Four focused
selection checks and six existing comparison checks pass. Affected official CTS
replay remains pending; no official CTS archive, JAR, APK or image is modified.
The native network validation discrepancy exposes original DnsResolver socket
fchown failing with EINVAL before its address-family source probe (#1162). Socket
allocation now owns durable Linux sockfs UID/GID/mode keyed by the retained
Darwin socket cookie, including native init-created inherited sockets. Actual
fstat/fchown, duplicate/SCM transfer and guest fork preserve that inode owner;
unknown unregistered adoption fails explicitly. Four focused checks pass with
no skips, including a different-UID SCM receiver denied foreign chown and native
init socket inode ownership distinct from its bound pathname. Actual network
validation on the newly deployed runtime remains pending.
An identical-signed-bytes executable control runs original sh successfully from
a new vnode while the frozen original executable path dies before main. This
narrows #1146 to executable publication/vnode generation behavior; it does not
prove a permission workaround or a resolved reboot. The host build graph now
compiles to a private Cargo directory and publishes detached executable inodes
by atomic rename at the existing public CLI paths. Four focused checks pass,
including actual signed Mach-O verification/execution and retention of an old
open generation. This fixes publication lifetime ownership, not a proven Darwin
denial mechanism or a complete boot-cohort upgrade (#1146). Originals and frozen CTS
binary bytes remain unchanged; controls are cleaned after verification. Latest whole native library verification
passes 805/0, eleven ignored; the ignored cases are not counted as run. Original PMS on a cloned native
data image boots, preserves Chrome/Calculator UIDs and APK paths/permissions,
launches Chrome cold in 1517ms and renders example.com without observed ANR.
The historical original campaigns each record Parsing 11/0, Preferred 2/0
and PackageSetting 2/0, but their image receipts differ from the current image.
A fresh final original 66-module campaign now runs against frozen inputs with
new disposable data, preserved binary/image/template SHA provenance and
first-boot evidence. Its first boot completes in 52.645s; no module outcome
is claimed until the official XML and campaign receipt complete.
C14 passed shared and nonshared APEX permission UID handling.
The absent projection requires matching parsed APEX name/path and uid -1;
ordinary missing owners still fail, and the focused regression passes.
C12 passed the pinned Settings suspension
policy connection; C11 passed separate restriction/Settings APEX inventories.
The compressed-stub factory runtime retention fix passed controlled C10/C11.
Focused restriction inventory and factory code/sparse-user retention regressions
both pass. Full native unit verification passes 785 tests, zero failures, with
eight ignored tests still not run.
The host parsing module has run and failed; full C CTS/app acceptance
remains incomplete.
On the M4 C branch, the native Binder
endpoint has source bodies and dispatches for all 298 public methods across
IPackageManager (224), Native (14), Installer (26) and Session (34). The source
owner inventory records 192 conditionally connected methods and 106 partial;
no public-method producer factory is entirely absent. The full C handoff and
runtime activation reach boot completion in controlled C46. Image,
app-data, permission, staged/stream and completion producers are being integrated.
The post-scan runtime constructor now calls the native persistence, installer,
preferred, archive, metadata, mutation, maintenance, verification and user owners
in one sequence and retains their workers. Native mutation reservations apply
original consumer records through the shared Store's persistence/publication gate.
The install environment now has one production callback owner path; app-data,
permission reservation metadata and query publication are constructed together,
without rebinding a second owner in pipeline attachment. Historical SessionInfo
transport uses typed arrays so Binder and FD capability tables survive delivery.
NativeServices now retains that runtime and can publish bootstrap-bound native
endpoints under `package` and `package_native` after assembly; partial construction
and shutdown detach the owner before worker teardown. This entry is not invoked
by the image's current boot path. Initial scan and Java BootstrapFactory source
are connected through generated BootSession dispatch and a native service-host
begin entry. The native entry now derives boot policy/resources from retained original
leaves after UM creation, retains actual Settings readers, and applies accepted
scan facts before runtime construction. Production Java factory assembly
remains under integration; the default image does not invoke this
entry and activation is incomplete. The original PMS still runs.
guest-init now recognizes an explicit `package` native-service request and
provides its canonical image/mounted data owner without publishing package
endpoints early. The default native-services manifest does not request it.
Compressed system stub admission now runs native gzip extraction, ABI library
copy and signed full-code scanning before Capture. It retains the same
destination receipt owner through runtime and invokes original F2fs release
on only those owned paths before systemReady. Controlled C10 boot passes compressed-stub runtime completion; full service
publication and CTS/app acceptance remain unverified.
The native initial scan now separates admitted raw code/Settings visibility from
permission-projected Capture construction. The raw graph remains available to
the original permission owner's startup queries while the projection is consumed
once outside its mutex. This is implemented source, with no live boot evidence.
The web-instant Settings watcher now publishes its actual UM-scoped disabled
map into immutable native capture generations; boot-state records read the
requested retained capture, rather than a mutable current-policy fallback.
The latest source inventory finds concrete producer bodies for Computer 153/153
and Internal 134/134; production Java construction and capture attachment
are still being connected, so these are not validated running interfaces.
These counts are source progress,
not boot or conformance results. Implementation has moved to consolidated
build and test verification; original-image linkage and C acceptance remain
unproven. Native-interface
audio playback capture reads actual visible ApplicationInfo flags; the module
metadata provider getter reads its configured owner. Shared/declared library consumers, paths, APEX directories, protected
broadcasts, initial non-stopped packages, sandbox name, user stopped/suspended/
quarantined state, harmful warnings and app metadata source are exposed. Activity
filter enumeration uses registered filters with current MIME groups in manifest
order. Permission-controller selection is explicit at the complete policy gate;
unselected captures reject that getter. Library equal-hash ordering follows the registry owner insertion order (#992).
Module-provider XML and selected-language scalar resources are decoded from the
accepted provider APK; overlays, isolated splits and styled labels remain #993. App metadata missing-package errors
now preserve the original ParcelableException cause and envelope. Generic opaque
exception payloads reject Binder/FD objects until a capability owner exists;
class/message bytes are forwarded without native class lookup. Three binder-host
envelope/capability refusal tests pass. Actual original
Parcel reading and reverse write/decode/rewrite pass in the full package oracle
(103.62s, no skips), including suspension extras, all 128 page-size compatibility
flag combinations against original framework strings, provider XML/text/APEX module values and
ArrayMap equal-hash insertion order. That comparison exposed a common reply helper that rebuilt exceptions
from only code/message; it now retains the complete exception.
SystemConfig UID permission assignments use actual bionic/partition UID names and
exact UID values; they do not grant root unconditionally. Missing external owners remain
explicitly unsupported (#987). Splash-theme, minimum-aspect and update-available
setters use an installed disk owner, validate the exact generation, prepare a
complete replacement, persist, publish committed changes and invalidate the
original package-info cache outside the publication lock. Old captures remain
immutable. Harmful-warning and installer-owned category-hint setters use the same durable
publication owner; warning text consumes original spans before persisting its
string, and category no-ops keep the current generation. Update ownership can
be relinquished by its actual owner, system or shell; a removed owner is a no-op
and original root has no exemption. Durable settings preserve installer fields. Shell-debugging policy
now comes from a current UM-only leaf, independent of DPM startup, for harmful
warning reads/writes. Each call checks the retained bootstrap before publication. Scalar request decisions now run after acquiring the installed disk
owner, against the latest generation. Concurrent splash/aspect setters both
persist and publish, with two version updates and cache invalidations (#989). Enabled/stopped/MIME
decisions and persistence exist, but their required effects are not connected
(#988). Installer/session state and pinned metadata codecs exist with explicit
policy/storage/callback owner contracts. A retained bootstrap can install a native
installer Binder with real session storage, five oneway callbacks and per-call
original UM/DPM policy reads. It handles 15 of 26 top-level methods and 29 of 34
session methods; default publication and durable install/commit recovery remain
#986. The original policy/FileBridge oracle passes in 20.16s without skips using
fixture UM/DPM owners and the original FileBridge output stream. A separate
original ART test passes in 57.66s with an explicit native true-mode writer:
Binder imports its typed proxy FD, writes and reads back 17,003 bytes, duplicates
its shared offset, transfers and retransfers it through original SCM_RIGHTS,
executes a new original ART process image with the inherited capability and
rejects every writer with EPERM after abandon.
The image selector remains unchanged; this is not default installer publication. This does not prove live SystemServer policy or SELinux
restorecon. The complete derived-image build passes (118.5s, 12 rebuilt/30 fresh nodes); the original
first-system-scan test passes all six restart cases (174.25s, no skips). Session names, real read-only file descriptors, split markers, sealing, transfer
and loader metadata use concrete stage/record owners; child graphs persist and
recover. Thirty-five installer owner tests pass; APKLite projection has two passing tests.
Domain limits and APKLite platform inputs require their original owner bindings. Legal guest symlink and complete virtual
credential/mode handling remain #1001; host symlinks are refused. Other FD-bearing
installer records reject before allocation until their capabilities are owned. Actual original ART
comparison passes five SessionParams/SessionInfo cases (20.47s), including nullable
fields, maps, three URI classes, DataLoader, inline Bitmap and populated Gainmap.
The comparison found and fixed the Java Bitmap gainmap tail; related
notification/media codec work is #991.
A read-only native publication page now accompanies facade captures. Native
publication release-stores its version before returning; bootstrap teardown clears
it. Java reads with acquire ordering, refreshes only on version changes and keeps
old snapshot scopes. Host tests verify read-only fd transport and mutation-driven
version changes. A separate system-only IPackageComputer lease retains the exact
capture after the scan lease closes and newer versions publish. Typed metadata
queries keep actual caller UID (permissions) separate from filter UID (visibility).
Unused scan leases release their query capture without creating a child endpoint;
claimed endpoints release their graph on close. Java Data/Store/scope reference
counts keep an old computer alive until the last owning scope closes. This is a
typed adapter with 22 private Binder methods. Concrete facades match the actual
image API (153 Computer declarations and 134 abstract PackageManagerInternal
methods); unbound methods require explicit typed owners. The retained IPM query
transport refuses writes. Full facade lifecycle integration remains incomplete. Names normalize through the captured
rename table and visible library owner; UID target SDK uses actual retained/shared
UID owners. A paged, version-bound UID registry supplies current, shared and
retained slots; Java getters do not infer those selections from names. The
integrated service units pass 688 tests (six input-dependent tests
not run); the explicit retained native scan passes in 2.33s without skips.
Its public UID/name/instant/SDK/installer calls and explicit uninstall filtering
are implemented separately from internal UID lookup (no public user/flag checks)
and original caller-based name normalization. Java internal read helpers use
captured parsed code/state and preserve their distinct defaults.
The full derived-image build passes with 9 rebuilt/33 fresh nodes and no failures.
Native registration state preserves instrumentation/property/provider collision
order and normalized provider authority aliases across capture publication and
scalar updates (#997/#999/#1000). Provider PackageInfo/facade base normalization
remains #1002; profile AppOps preflight remains #998. Concrete installer policy,
AtomicFile session XML and stage owners now create/open/list/recover/abandon real
sessions with mandatory creation metadata and SELinux label providers. Inode/byte
claims reject external replacement, post-rename failures report committed state,
and recovery denial overrides grants independently of XML ordering. Nineteen
focused installer tests pass, including same-byte/inode substitution refusal,
committed claim-refresh failure and grant/deny order/legacy priority; complete live policy, callback ownership, async
persistence, original XML interoperability and install/commit remain #986.
Original ART installer codecs pass five cases (20.47s). The complete original
first-system-scan passes (184.79s), including snapshot/version-page/expanded Java
Computer adapter assertions and all six reused-data native-file restarts with unchanged
120s bounds. Earlier restart failures (#990) coincided with host free space below
the syscall layer's 1GiB write reserve and repeated keystore exits. Removing 24.7GiB
of verified inactive task-owned test images cleared the failures without changing
settings/guest code. APFS TRIM retention remains #771. These are component/native-file
checks; native default, full SystemServer facade, CTS and app gates remain
incomplete (#798). The native Store retains the original ABX restriction
file backup/reserve and system ownership protocol.
Two original-PMS boots on disposable native-written data (2026-10-02)
reached `sys.boot_completed=1`: Settings read as disabled (`enabled=2`),
then after a native reset to default, Settings launched successfully
(`am start -W`, 85 ms). This checks file compatibility; the native
PackageManager service and SystemServer facade are not activated. The
native restrictions reader retains each suspension's owner, original dialog
fields, quarantine flag and typed PersistableBundle extras, including the
legacy format (#706). Extras retain scalar/array types, nested bundles,
nullable keys/string-array slots and raw double bits. XML errors preserve
previously read SuspendParams fields; runtime errors escape. User ownership
is resolved with an explicit cross-user image policy before duplicate
replacement and quarantine aggregation. A disposable original-PMS oracle
(2026-10-04, 11.16s) verifies 66 dialog restore/save cases and 248 extras/parameter
cases across text/ABX parsers, including malformed inputs, partial arrays,
defused types, CDATA, NaN payloads and Java floating-point syntax/rounding.
The native PersistableBundle Parcel writer also passes 130 original ART
read/write roundtrips, checking every decoded value, the exact consumed byte
range and byte-identical reserialization after forcing original unparceling.
This includes Java hash order/collisions, null/empty keys, nullable array slots,
nested bundles and raw NaN payloads. Caller-built duplicate keys fail explicitly.
Native enabled-state persistence preserves text CDATA extras across an ABX
write and reread. XML document identity compares floating-point payload bits,
allowing unchanged NaNs and rejecting external signed-zero changes. All 386
units pass (3.14s), as do 11 XML units; the ignored
integration test was not run. Unpaired UTF-16 string code units remain
unsupported (#843). A private snapshot lease now carries persisted and runtime user-state
inputs through generated AIDL methods 9/10 in 64 KiB chunks, with active/factory
scope and user ID pinned to its captured version. The real Binder test retains
a large old user state after publication and rejects invalid request ranges.
An original ART Proxy/Stub oracle (2026-10-04, 16.17s) reads the native user
records, checks identity/cache reuse and rejects short chunks and mismatched
versions. Native runtime owners retain package overlay paths, shared-library
paths and component label/icon overrides in the captured user state. Mutations
preserve original empty-map allocation, merge order/deduplication, collision
ordering and null/empty-label/zero-icon distinctions; active runtime changes
update only the disabled setting's existing user aliases. The original
PackageUserStateImpl agrees with 15 mutation states, and ART reads actual native
runtime DTOs with immutable lists. The real Binder capture retains old runtime
state after a newer version changes its overrides. PackageUserStateReplica now
implements every pinned PackageUserStateInternal method and the inherited
framework Set-returning interface, with one cached object per captured user and
explicit image suspension policy. Original ART exercises populated scalar,
component, suspend, archive and runtime getters; the adapter recreates detached
original return objects, seals watched sets/maps and retains saved archive time.
Suspension maps distinguish null from allocated-empty owners (#847), and
cross-user owner keys/last duplicate/quarantine aggregation use the supplied
pinned policy. Native owner updates refresh existing disabled-user aliases.
Native PackageSetting owners now retain nullable old-path sets with ordered,
deduplicated Java File normalization and detached copies; removal preserves
allocated-empty sets. Loading is derived from monotonic progress rather than
an independent XML boolean (#851). Native restoration applies the original
setter/default getters to active settings and retains constructor loading
fields for disabled factories. Original ART agrees on 36 runtime owner states
and 32 active/factory text/ABX restoration cases. Captured scan versions retain
old paths and loading progress after a newer publication changes them. A private setting-record lease now pages actual captured ABI, flags, timestamps,
loading, domain/app metadata, restrict-update hash and nullable old paths through
generated AIDL methods 11/12. Java validates name/version/active-factory identity
and trailing bytes before caching an immutable PackageSettingData record; hash
getters are detached and old-path lists preserve explicit null entries. The real
native Binder test retains a multi-page setting after newer publication and
checks range/null/trailing/closed requests. ART exercises native captures through
original Proxy/Stub framing, failure/retry, short pages, version/scope/trailing
rejection, cache identity and original loading/old-path getter restoration.
Install-source records now retain original package identities, installer UID,
attribution, package source, orphan/uninstalled flags and optional initiating
signing lineage in that same capture. Java recreates the original InstallSource
through its package factory with detached signing objects; ART checks populated
signed and empty-orphaned inputs, mutable signature isolation and original empty
normalization/precondition behavior. Native Settings normalizes only unspecified
empty sources and rejects signatures without an initiator (#853), while capture
publication rejects invalid/non-normalized sources without replacing the prior
version. Native text/ABX owner tests retain explicit nondefault source values.
Captured settings also retain native SDK library names/major versions/optional
bits, static library names/versions and MIME groups. Java getters return detached
arrays and an immutable map of immutable original-order sets. Actual original
PackageSetting getters agree for empty and populated native captures, including
64-bit versions; mutation of returned arrays or original rebuilt objects cannot
change a capture. The native Binder test retains earlier library/MIME state
across newer publication; duplicate group graphs fail before publication.
Settings MIME restoration now merges repeated groups, deduplicates types and
keeps signed UTF-16 Java hash/collision order (#855). A pinned reader-loop port
using actual original PackageSetting.addMimeTypes agrees on 12 text/ABX cases,
including known-tag nesting, ignored unknown subtrees, missing names/values,
empty strings and negative/colliding hashes. The complete package ART oracle
passes (28.44s); all integration targets compile and device-services builds
against original APIs (15.6s). Native PackageSetting runtime MIME ownership and
capture now preserve nullable group names and type members, distinct from empty
strings, with signed Java hash/collision ordering and duplicate suppression
(#856). Original ART agrees with actual native populated captures, original
PackageSetting copy isolation and immutable Java map/set values. Known XML
restoration continues to ignore missing names/values; original text/ABX MIME
writer loops throw on null names or members. The native Binder fixture keeps
prior null owners after a newer publication removes them, and setting updates
retain null members of preserved declared groups. The original-state feed now writes MIME maps/sets in their original iteration
order through the production PackageMimeGroups serializer, preserving nullable
names and members without TreeMap/TreeSet sorting (#857). ART serializes actual
original PackageSetting maps through this helper and matches native Parcel
bytes exactly. Full native package-record reads keep null distinct from empty;
absent collection owners and duplicate names reject before publishing records.
Component registration now rejects a referenced missing group or null type,
matching the pinned applyMimeGroups loop and original IntentFilter exceptions;
malformed nonnull types remain ignored and unused nullable groups remain valid.
Failed builds do not replace the cached resolver, and query endpoints report a
null-pointer exception for this owner failure. This is component validation;
full native mutation/notification/bootstrap ordering and C acceptance remain
unproved. Resolved library graphs, transient/legacy-permission inputs and full
facade assembly/import/live producers remain #836/#834/#837; #856 remains open
for complete facade integration.
Native keyset owners now deduplicate upgrade IDs in insertion order and retain
nullable aliases separately from empty names, replacing duplicate aliases in
signed Java hash order (#852). Native registration uses those same owner
transitions. Captured setting pages carry proper/upgrade/defined keysets;
Java reconstructs detached original PackageKeySetData objects, with null upgrade
arrays for default/cleared owners. Original ART agrees on 12 text/ABX keyset
reader cases and actual native captured getters; returned arrays/maps can change
without affecting the capture. Original text/ABX serializers fail on null alias
writes; native commits reject them before changing files or published state.
Keyset import retains a runtime reference owner (#854): each proper/defined
XML occurrence increments before replacement, including nested known keyset
entries. Upgrade IDs and disabled factories hold no refs. Registration acquires
new alias refs before releasing every prior alias role; signing replacement and
package removal decrement actual counts. Overwritten XML roles can leave sets
and keys alive after removal, matching the original manager. Counts are not
serialized: native commits preserve the live owner, while restart derives counts
from saved roles and prunes newly orphaned sets/keys without rewinding IDs.
The original parser/keyset/update-owner ART fixture passes (15.08s), including
11 exact owner states and original KeySetHandle reference counts, shared roles,
replacement, removal, orphan pruning and restart. Disposable resilient ABX store
tests verify residual refs survive removal and unrelated commits until restart.
All integration targets compile; device-services links against original APIs
(14.2s). This is component ownership evidence; full PackageState adapters and
live producer/import wiring remain #836. Native PMS activation and all native
CTS/app/template/APEX/rollback acceptance gates remain incomplete.
Live overlay/component producers, original-state import and full PackageState
metadata/callback wiring remain #836. Native suspension records now distinguish
explicit null parameter values from missing keys/maps (#848); runtime put/remove
retains allocation and freezes absolute UserPackage keys independently of XML
read policy. Native and Java quarantine evaluation follow signed UserPackage
hash order, preserving original null-parameter failure and true-value short
circuit behavior. ART verifies null map entries under both read policies and
original removal behavior. A port of the original null-parameter writer loop
emits the named empty XML tag; native reread creates the original-defined default
parameter object. Full native suspension persistence/notifications remain #706;
The native archive XML reader now traverses descendant activities, expands relative
component classes and drops missing attributes or invalid names as the original
reader does (#849). It preserves ordered duplicate activities, saved timestamps
and optional monochrome paths; malformed timestamp attributes default to zero,
while a negative timestamp in a constructed archive fails original validation.
A Settings reader-loop port running with original ComponentName, Path and
ArchiveState APIs agrees on 32 text/ABX cases. Runtime primary archive icons now retain null values through the original
Java feed, native state, snapshot Parcel and original Path getters (#850). ART
compares a two-activity runtime archive with original PackageUserStateImpl;
immutable replica lists retain it after an original object's activity list is
mutated. An original Settings writer-loop port omits the null primary icon
attribute, and native XML reread drops that activity while retaining the other
activity, its icon and saved time. The real Binder capture keeps the old null
icon after a newer published state replaces it with a path. Native archive
persistence integration remains #706. Native PMS activation and its acceptance gates remain
unestablished. Component ownership
now distinguishes original default null sets from Settings-initialized empty
sets (#845), including missing-file initialization and third-party new settings.
The original ART oracle confirms that the ArraySet setter overloads initialize
empty owners even for null inputs; native tests verify nullable transport and
empty-set persistence/reread. Component XML owners traverse descendant items,
ignore missing names, deduplicate and retain signed Java UTF-16 hash order
with stable collision insertion order (#846). A Settings reader-loop port using
the original ART XML parser and ArraySet agrees for 14 text/ABX inputs, including
nested/unknown nodes, empty names, negative hashes, supplementary characters
and collisions (2026-10-04, shared suspension oracle 11.16s). Enabled-state
publication takes the written component fields from the parsed owner, preserving
other captured fields and matching reread order. Native suspension operations and native PMS
activation are not established. The C branch also has Binder query
receivers for `package` and
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
(#799). The same resolver now accepts explicit native scan code, saved signing
and user owners, without constructing a partial query PackageState. Native
SigningScan computes dependency candidates from finalized active code, then
commits nested library metadata and static-provider installed-user changes
together. Missing policy/signing/provider/user inputs reject before mutation.
Captured assignments retain code/signing/user-inventory/installed-state and
registry inputs; changed inputs reject queries and publication until recomputed.
Original captures remain usable after a new graph publication. Unit tests compare
native and query-input multihop results, code retention and static-user effects;
all 392 units pass (3.22s; one ignored/not run), and all integration targets compile.
The actual first-system image fixture passes (42.31s), exercising candidate/commit, policy failure
atomicity, stale-user rejection and old/new publication under controlled native
enforcement policy. Finalized active dependency owners now have generated snapshot
length/chunk methods and detached original SharedLibraryInfo/file-list assembly.
Unknown packages remain distinct from unresolved owners; version/name/UID checks,
strict ranges and closed-lease checks guard transport and restoration. Native Binder
checks retained captures after publication. A controlled declaration/path fixture
exceeding 64 KiB exercises original generated Java Binder paging and concrete
PackageSetting restoration under original ART. The native SharedLibrary query
model and original ApplicationInfo writer retain nullable optional-dependent,
certificate, raw code-path, dependent and nested-library owners, including
allocated-empty lists and null elements. The shadow feed exports original
SharedLibraryWrapper.getInfo() parcels rather than inferring hidden fields from
interface getters. Native import/re-encoding preserves exact source bytes,
including a path-backed owner's explicit code array and SDK certificate
placeholders without a declaring package. Strict count/class/value-length checks,
truncation and trailing-byte rejection protect import. Native declaration producers
write present elements; graph expansion retains nullable nested records. File
resolution requires the actual provider/code owner instead of guessing paths from
nullable metadata. All 395 units pass (3.15s; one ignored/not run), all integration
targets compile, and the actual first-system scan passes (42.57s) with the existing
controlled enforcement policy.
Java PackageLibraryState now restores original optional/certificate owners as well
as declaration owners. It keeps byte-identical original CREATOR objects where
possible, restores fresh nested dependency owners through their original list,
and reconstructs original full/SDK certificate constructor owners when the Parcel
constructor normalizes null optional lists. Raw code paths and null/empty dependent
lists are read from the captured parcel, not inferred from convenience getters.
Every returned original object must reproduce its complete captured bytes;
non-reproducible input rejects. The complete original package ART oracle passes
(18.49s): seven original owner forms, nested nullable constructor owners inside
populated optional/certificate records, exact bytes, original concrete setting
restoration, mutation isolation and malformed-owner rejection. It also verifies
that changing category on the same detached original PackageImpl changes its
cache bytes and restoring the category restores the complete original bytes.
The feed now serializes parsed code inside the unfiltered snapshot and uses
content hashes for all records: retaining an AndroidPackage object's identity
cannot suppress a changed code record. Original Java linkage passes (5.06s);
device-services builds in the full boot build, including the original-PMS
userdata template. The final disposable 90-second original-PMS shadow with
publication history and separate current/historical caches exits successfully
without read/publication/configuration errors or owned processes/data mounts.
Final generation 76 retains 285 active packages and five disabled factories.
Its 21,714 calls include 20,447 matches, 428 classified races, 17 differences
and 822 not modelled; write checks include 49 matches, two differences and two
not modelled. Eleven GMS getPackageInfo category mismatches (-1/7) remain (#865),
so publication history does not establish category parity. Enabled-state and
eligibility/write differences remain #866, provider eligibility #867 and
install-parse lifecycle comparison #868. Temporary owner/publication diagnostics
show the original feed publishing a new APK with override -1 before subsequently
publishing override 7; a queued worker can skip that intermediate publication.
The retained image's PackageInfoUtils dex confirms the native computation's
base-category/fallback/saved-override order. All diagnostic instrumentation was
removed; the diagnostic boots exited without owned processes or data mounts.
An implementation using a shared current/historical parse cache produced hundreds
of live differences and was replaced: historical comparison must not evict the
current cache or rebuild the same previous joined state on every call.
FallbackCategoryProvider's newly constructed AssetManager includes system asset
overlays. Native selection retains GoogleConfigOverlay's 387-entry CSV; removing
those overlays caused widespread live category differences and was reverted.
The original ART oracle now directly loads FallbackCategoryProvider and compares
all 387 native values plus two undefined values, rather than inferring selection
from addAssetPath alone. Its original BufferedReader/String/Integer/ArrayMap
owners verify duplicates, trailing commas, CR/LF/CRLF, Unicode digits and partial
malformed input; original SystemProperties verifies true, false and default
variants. Native import uses those semantics: duplicate replacement, stopping at
the first numeric error while retaining preceding values, and explicit failure
for blank lines or missing resources. Ignoring categories bypasses those resource
errors. An explicitly run image integration passes (1.63s), checking the selected
bytes against the actual overlay APK. All 398 units pass (5.40s; one ignored/not
run), and all integration targets compile. These resolve #869 without a per-app
category rule. No library-field differences were
observed in this boot workload; targeted original owner fixtures prove nullable
optional/certificate reproduction. This is original-PMS feed validation, not
native PMS conformance. Native-to-SystemServer lease bootstrap and complete
facade import remain #836. Live policy delivery, overlay effects and
factory graph handling remain #707/#808/#836.
The feed retains complete publications independently of the shadow worker:
a ten-second history plus the newest older publication selects only states
published strictly before the call. This preserves an intermediate new-APK state
when a queued comparison worker observed only the old APK and then the later
category override. Historical joins use already-observed user unlock context;
missing user context rejects, and current UserManager is never queried for past
state. Current and historical parsed-package/join caches remain separate, so a
comparison cannot evict the current cache or repeatedly rebuild the same prior
state. Deterministic tests verify intermediate version/category selection,
strict timestamps, failed-digest exclusion and bounded retention, plus complete
previous-response equality when the worker missed the intermediate version.
All 400 units pass (5.34s; one ignored/not run), all integration targets compile
(14.51s), and the full boot build passes (15.5s; three rebuilt nodes).

The original PlatformCompat install-time native-library policy
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
is still pending. Bootstrap facade wiring, native boot dependency-stage
integration and native PackageManager
activation remain pending (#707);
no native PackageManager CTS result is claimed.
The scan now retains each fully completed native AndroidPackage in immutable
active/disabled-factory inventories, separate from saved settings and user state
(#798). Active objects enter only after ABI, code, flags and keyset completion;
factory refresh enters only after its metadata succeeds. Scan withdrawal drops
active code while retaining saved UID/settings and disabled code; saved-setting
removal rejects still-loaded objects. Disabled copies share the accepted active
object, and stale/reenabled disabled settings retire that factory object. Old
scan snapshots retain their objects through later changes. All 344 units pass
(3.49s), and the original-image first-system-scan integration passes (42.94s),
including exact completed-object contents, failed re-scan preservation and
active/factory separation and rejection of a detached mutated completion object.
The disposable original-PMS saved scan also passes (388.74s): all 243 active APK
objects and three disabled factories equal their completed native scan records;
active UID and collected signing fields and factory collected signing are checked,
with distinct active/factory code paths and owned process/mount cleanup verified.
This inventory is not yet a published query replica:
permission/overlay/platform inputs and facade transport still require their
owners (#798). Native AndroidPackage now writes the pinned PackageImpl cache
format, preserving enriched metadata, pooled strings, nullable collections,
component/process map keys, properties, keysets and SigningDetails (#723).
A disposable original-PMS oracle passes (19.23s): all 285 cache entries from
the current template, 285 enriched variants and two actual native first-system
scan objects are read and written by original PackageImpl, with every decoded
field compared. Only unordered map iteration
is normalized; list/array order and values remain exact. Original getters also
verify the supplied UID, ABI, native root and page flags. Variants carry an
actual original saved signer and serialized public keys. Null certificate sets
normalize to empty, with duplicate removal and Java hash ordering (#832).
Unsupported pooled IntentFilter PersistableBundle values and unmodeled manifest
query context reject explicitly; the parser/read models still need unification
(#723). Verified certificate collection now populates parsed SigningDetails before
identity/signing reconciliation; the collected package lineage stays distinct
from the reconciled saved setting's lineage. Active scan completion sets the
assigned appId as PackageImpl.uid after all metadata/keyset gates, before
retaining the immutable object. Disabled factory metadata refresh keeps the original UNKNOWN signing
without active UID finalization; saved/strict-recollected setting signing remains
a separate owner. Original Java getters verify actual first-scan
UIDs 1000/10000, v3 signers/public keys and GSF's two-certificate history.
Failed rescans retain prior objects/state; malformed serialized keys reject
without mutating the raw input. Facade callback delivery and publication remain
incomplete (#833). The new oracle checks image
linkage before boot, and its owned processes and mount are cleaned on completion.
The package bootstrap bridge now has a separate generated AIDL and a synchronous
IServiceHost attachment (#834); existing late-service methods retain their
original transaction codes and oneway behavior. It retains the typed Binder
endpoint without mapping nonce memory or starting late-service listeners. Only
the matching endpoint is removed on death, so an old server cannot erase its
replacement. Attachment reads and retains the pinned original boot-classpath
policy before publishing the endpoint. Native clients reuse that build policy
and query native-library policy and complete active-user permission GIDs, retaining owner/transport
errors and GID ordering/duplicates. Java's early endpoint and normal IBridge
share the same original PlatformCompat, PackageBackwardCompatibility and
PermissionManagerServiceInternal implementations, with system-UID checks and
explicit unavailable-owner errors. The early attachment entry point is
implemented and attached synchronously from the C branch's PMS-main redirect.
The one startBootstrapServices call now enters PackageBootstrapBridge before
original PMS scanning, after original PlatformCompat registration. The wrapper
returns original PMS until complete C gates pass; attach failure aborts bootstrap.
The early wrapper now hands the actual original DomainVerificationService to
that retained endpoint (#878). Generated generateNewDomainId returns its UUID
as two big-endian words; the native bootstrap rejects null, non-16-byte and
trailing replies and propagates owner/transport errors. No local replacement
ID is substituted. Real Binder tests cover exact values, malformed replies,
owner rejection and endpoint replacement. Original ART constructs the original
domain service and exercises the production exporter against actual random UUID
version/variant and missing-owner behavior. The generated bootstrap also now
exports Settings.getAllUsers' actual UserManagerService inventory, with partial
users excluded, dying/pre-created users retained, and each user's original ADB
installation restriction (#879). An uninitialized owner is distinct from an
initialized empty list; strict decoding rejects malformed flags/counts, duplicate,
negative or unordered IDs, null payloads and tails. Native setting admission tests
consume these decoded policy inputs and verify null/USER_ALL/explicit-target rules.
Feed kind 12 captures this same original user owner for live parity and retains
it under each immutable feed generation. Generated getApexBootInventory and feed
kind 13 now export the same original ApexManager all-package and active-mount
inventories (#882), through a typed same-package helper. Factory and updated
versions may share a module name; raw nullable names, original ordering, code and
preinstalled paths, 64-bit versions and factory/active/changed flags remain
separate. Null all-packages differs from an empty list. Strict native decoding
rejects missing paths, invalid counts/booleans, null payloads and tails; Binder
owner errors propagate. Native scan partition inputs use preinstalled partition
path boundaries as InitAppsHelper does, preserve active-owner order and raw
module names, and omit unrecognized origins. No mount-name inference is used.
Real Binder tests cover decoded origin, malformed payload/envelope, owner errors
and immutable old inputs; feed tests verify generation detachment. Original ART
uses the original ActiveApexInfo constructor and actual ApexManager inventory,
and confirms factory/update duplicates, null names, versions and detached bytes.
Native initial-scan registration now assigns the raw active APEX module name,
including null, before publishing the accepted setting (#881). Without active
APEX info it uses the disabled owner even when its module is null, then the
existing owner. APK-in-APEX commits set apkInUpdatedApex from the factory flag;
ordinary APK commits preserve its retained value. Disabled factories refresh
only their module name from raw parsed identity before version/signature
selection; that refresh survives a later rejection or selection of the data
copy. New, retained and original-name-adopted completion update the accepted
setting and returned candidate together. Source-rule regressions cover nullable
owner precedence, unrelated retained bits and refresh before identity selection.
The explicit unchanged-original-APK initial scan gate passes (75.21s), with a
disposable APEX mount matrix for null/non-null modules and factory/update state,
retained-data selection and a later domain error preserving the disabled refresh.
Initial-scan static-library identity now retains the raw package name for APKs
whose scan location carries active APEX info (#883); ordinary static libraries
receive the 64-bit version suffix. The guard uses physical owner context rather
than parsed isApex. UID, signing, disabled-factory, data and boot revalidation use
the same location-aware selector; saved-code verification uses its physical
PARSE_APK_IN_APEX flag. UID regressions keep ordinary/APEX identities distinct
and retain the APEX provider's UID across static-library versions. Original ART
calls the retained PackageManagerService.renameStaticSharedLibraryPackage with
the pinned addForInitLI guard; full PackageImpl fields agree for both contexts.
The native full-scan gate also checks controlled parsed static-library DTOs on
unchanged original verified APKs at ordinary and disposable APEX locations,
including final loaded code and module ownership. This is identity-stage evidence,
not a claim that those fixture APK manifests declare a static library.
Native APEX input preparation now reads the original ApexManager module paths
before APK scanning (#884), preserving last metadata per unique path and stable
factory-first ordering. All 42 original archives parse and verify; their complete
PackageImpl fields and SigningInfo parcels match original PackageParser2 and PMS,
including signer keys and lineage capabilities. Parsing uses the initial system
flags for both factory and updated containers; updated scan flags clear SYSTEM_DIR
after parsing. Null inventory is empty as in the original; missing archives and
unsigned synthetic containers fail explicitly. Native disable_system_package now
performs Settings.disableSystemPackageLPw(replaced=true)'s loaded-factory transition
(#886): it copies the setting before marking active transient updated state,
retains active UID/signing flags, shares the loaded code and existing user states,
and excludes users added after the copy. Disabled settings retain shared UID
membership without contributing active flags. Missing, unloaded, non-system,
already-updated or already-disabled packages return the original false result;
incomplete metadata/user/shared owners reject before mutation. The unchanged
original-image scan gate exercises the actual transition and user alias behavior
(67.72s). SigningScan.new_after_apex retains verified non-shared container settings at
INVALID_UID (-1) without reserving APK slots (#887). Original PMS MATCH_APEX
reports -1 for all 42 containers; mixed APK/APEX restoration preserves APK slots
and their allocation cursor and rejects unknown negative IDs and mismatched
identity/path/version inputs.
Native initial APEX registration now constructs Settings and loads verified code
before APK admission (#888). Policy uses the actual container code partition,
always sets SYSTEM/APEX and does not inherit an updated container's preinstalled
partition mask. Original scan parse flags determine signature/time origin,
including updated /data/apex containers; APK physical-path rules remain separate.
New container code retains INVALID_UID, raw module ownership and full signer details;
retained containers preserve their user owners. The APEX ABI branch leaves payload
libraries to apexd, and APEX keysets never enter the persistent pool. Completed
page/code/application/seInfo metadata enters loaded ownership, and inactive
factories invoke the actual disable transition. Original typed notification
records are retained separately from APK results. First boot invokes its deferred APK image loader only after the caller's
notification succeeds (#892); rejection prevents all APK directory reads as well
as registration. Loader errors retain their original phase/path/message and
abort without returning a scan result.
The full original ART package oracle passes (29.50s): all 42 original archives
parse, verify and complete native registration without APK UID allocation, with
loaded code, signatures and assigned seInfo. Controlled inactive-factory and
updated-container views use unchanged original archive bytes in disposable paths,
preserve disabled code and UID-free active ownership, and verify that a later
domain rejection preserves only the original disabled-module refresh. The initial
scan gate passes (65.53s), including actual container registration, first APK UID
10000, exactly one deferred image read after notification, no image read after
notification rejection, and loader failure before any APK domain admission. Compatibility,
user/domain and notification owners in these native phase fixtures are controlled;
these results do not prove live original callbacks or complete facade publication.
Native Settings now distinguishes application appId from runtime sharedUserAppId
(#893), preserving the separate group relationship through signing, seInfo,
retained updates, disabled copies, removal and scalar/signing captures. Persisted
sharedUserId still supplies both IDs until a runtime owner changes appId. The
original ART oracle verifies native paired-ID records with appId 10123 and group
1000, typed original signing restoration, and preservation of group 1000 when the
original setting changes appId to INVALID_UID. Native owner checks retain active
and disabled group references without reserving an independent package slot and
reject missing/invalid groups. Native initial APEX registration now resolves a
new or retained declared shared UID and allocates only the group ID (#889).
Parsed/final code keeps INVALID_UID (-1); subsequent Settings registration sets
the shared container setting appId to the group ID while retaining sharedUserAppId
(#895). Non-shared container settings remain at -1. Signing and privilege policy use that
owner; active and disabled container restoration validates the declared group and the
committed setting appId. seInfo retention validates the effective UID owner,
preserving labels across this scan-to-registration setting ID transition.
Changing an existing container's declared group now creates a fresh setting,
preserves pendingRestore and withdraws the old loaded scan only within the staged
candidate. Successful completion removes the old group membership and prunes its
UID only when no active/retained member instance or disabled reference remains
(#889/#928); an incoming request left in the Settings map does not keep that
group alive. A non-shared replacement with a disabled factory now copies its saved signer
and captured LegacyPermissionState; only the enumerated users' enabled/disabled
component sets are copied, with fresh constructor defaults for other user fields.
Missing migration/component owners reject before commit. A fresh setting records
its constructor's installPermissionsFixed=false even when other fixed bits remain
unresolved. Old disabled-user aliases detach only for fresh replacements (#897),
so later active user changes preserve the old factory; retained updates keep their
aliases. Original Settings.createNewSetting agrees on signer/legacy/fixed-bit
inheritance, and the original component copy setters agree in a controlled user
loop. This does not construct a live original UserManager in the standalone
oracle. Native initial APEX commit now accepts an explicit SharedUidMigration
policy and converts a retained leaving singleton under BestEffort (#898). New
leaving declarations bypass group allocation, NewInstallOnly retains existing
membership, and multiple active members or a non-leaving disabled factory prevent
conversion. Conversion clears the active/factory shared relationships without
copying group permissions or changing installPermissionsFixed. Active settings and
code keep appId=-1; a previously registered disabled factory keeps its positive
appId. The original shared UID slot continues to reference the converted package,
so a following APK allocation does not reuse it. seInfo retains its assigned
transient labels and rebinds the validated identity dependency. A later retained
rescan keeps the converted relationship, and a rejected domain owner preserves the
full previous scan. Verified converted factory restoration accepts the original
positive disabled appId only alongside its negative non-shared active container
and a leaving declaration. Original Settings.isSingleUser, conversion and
registration agree for no factory, leaving/non-leaving factories and a second
saved member without parsed code. The original conversion also preserves populated
legacy permissions and fixed bits. The system-UID-guarded bootstrap bridge now
exports the actual original SharedUidMigration.applyStrategy(BEST_EFFORT) decision
through generated AIDL (#898), and the native client rejects transport/owner errors,
truncated replies and trailing data. The original ART oracle makes a generated
Binder transaction against the actual derived bridge as UID 1000, compares its
reply with the original policy owner, decodes the original frame natively and
supplies that decision to the required APEX scan input. UID 2000 is denied; no
migration property is changed. The policy and four native conversion cases pass
in 32.57s; 437 service units pass (one ignored/not run), and the constructor
resolution unit passes. The checked image/template build passes in 63.2s, followed
by a 2.2s host rebuild after constructor resolution was tightened (#900).
Compile-only private constructors are accepted only for image classes with no
constructors; executable constructor references resolve on their own class, never
an ancestor. Captured legacy owners now follow actual constructor transitions
(#899/#889): newly allocated shared groups retain their empty constructor state
on a later rejection; fresh or group-changing settings receive empty permissions
and fixed=false unless the original non-shared disabled-factory branch inherits
permissions. Retained settings rebind IDs without replacing their permissions or
fixed bits. Actual group pruning removes its legacy owner, and factory disable
copies the captured active owner and any known fixed bit. An absent import remains
absent. Typed original Settings construction, factory copies and group pruning
agree with the native populated/empty records in one passing ART run (44.22s).
437 units pass (one ignored/not run), the checked image build passes (71.6s), and
the complete initial scan passes (71.18s). Subsequent ART attempts fail with
SIGKILL after the policy Binder reply, including one reaching the file-write
phase (#902).
Another attempt fails earlier with ENOSPC while the host has 1.4 GiB available
(#903). After freeing only this task worktree's incremental caches, an ART run
with syscall tracing passes (33.39s), including fresh groups in a complete captured
inventory. These failures remain recorded; SIGKILL is not claimed fixed.
Full daemon boot orchestration/publication remains #836/#885. Retained renamed
APEX identities now use the saved declared system rename map for source admission
and verified active/factory restoration (#890). Code keeps its manifest identity
while settings, loaded code and permission owners retain the original internal
name and INVALID_UID. Without the matching saved map, negative-UID restoration
rejects. Typed original rename-map and Settings.updatePackageSetting owners agree
with native code names and populated legacy/fixed state (ART 33.93s); 437 units
pass (one ignored/not run), the host image build passes (19.7s), and the complete
initial scan passes (66.93s). A separate typed original ART probe (32.37s) pins
initial original-package adoption: createNewSetting clones the original setting;
APEX registration gives the adopted copy appId=-1 but getSettingLPr(10000) still
returns the distinct prior positive-UID setting. Both retain independent populated
legacy permission state and fixed=true. Native UID slots now represent a distinct
DetachedPackage owner with the prior setting, explicit captured users and optional
permission/fixed/runtime imports (#904). Detachment validates the current non-shared
owner, leaves allocation extent/cursor unchanged, rejects forged detached owners,
and preserves older cloned versions. Exact slot removal or replacement releases
only the current record; snapshot validation rejects missing or foreign owners.
Initial non-shared original APEX adoption copies supplied users and setting,
rebinds copied legacy state to INVALID_UID, retains the prior positive slot and
commits the rename map only after metadata succeeds (#890). A rejected domain
owner leaves the whole scan unchanged. Existing user states follow the original
non-sealed PackageSetting copy: current-version mutations update both prior and
adopted settings, while newly created users have no alias (#906). Actual setting
removal/replacement ends the aliases; prior captured versions remain unchanged.
Legacy permissions and fixed bits remain independent copies. The original ART
oracle compares present/absent user aliases and sealed captures for both detached
UID slots and retained shared members (32.68s), alongside populated positive/negative
UID permission frames. Shared adoption now retains the actual prior unparsed
setting alongside the new parsed setting (#905): cardinality two, no singleton
conversion, shared UID slot, final setting appIds 10000, and group SDK
CUR_DEVELOPMENT (10000) despite new code targeting 36. Removing the new setting
keeps the prior member, its public/private flags, group SDK and UID slot, matching
original Settings removal. Shared-group capture now distinguishes current and retained instances with the
same name (#905), carrying the retained setting, saved signing, legacy/fixed,
explicit users, transient fields and captured runtime in independent framed
records. Java reconstructs a distinct unparsed PackageStateReplica; the shared
replica preserves both object identities instead of resolving both through the
current name cache. Full Java snapshot assembly now validates the current members
against the active package map using the captured current/retained role, while
keeping validated retained instances solely in their shared group. A complete
native shared-APEX-adoption capture is decoded through the generated Binder
Proxy/Stub and PackageScanLease.captureData on original ART: the snapshot contains
one active setting and two distinct same-name shared members (one loaded, one
unparsed), preserves both after lease close, and returns detached member
containers. Removing the current member from the advertised package inventory
still rejects assembly. The complete package Parcel oracle passes (27.20s), and
the checked Java/image/template build passes (56.0s). This verifies that captured
graph; the six displaced-request captureData graphs also pass (below). Native
daemon publication remains #836/#798. The displaced-setting owner and recovery
contract is verified (#929, below); full request parity remains #919. Missing retained runtime/legacy/fixed/leaving inputs reject.
The original ART oracle compares retained flags, UID, sparse users, unparsed code,
shared snapshot cardinality/SDK, detached containers and exact record roundtrip
(34.84s); ordinary shared lease reads still pass. Native Store, library dependency inputs and Java collected-code restoration now
follow the original APEX UID sequence (#836): code remains INVALID_UID; active
settings hold the shared group ID or INVALID_UID; no slot is allocated at -1.
Native acceptance requires the original APEX scan origin; ordinary APK UID
mismatches still reject, as do wrong shared registration IDs. Retained converted
factory settings keep their prior positive registration IDs. The ART oracle
builds a real native shared-adoption snapshot and restores its code/setting UID
pair (-1/10000) through original PackageImpl/PackageSetting; a foreign setting UID
rejects. The complete oracle passes without syscall tracing (31.89s), 440 units
pass (one ignored/not run), checked host/image/template builds pass (68.3s, then
23.8s for the final host refresh), all integration targets compile, and complete initial scan passes (67.58s).
Shared process inputs now distinguish current and retained setting instances by
same-snapshot object identity, alongside path/version/appId/code-presence
fingerprints (#905). The native owner keeps both same-name entries, imports the
actual incremental aggregate, and rebuilds in the original reverse ArraySet order,
skipping the prior unparsed setting. Exact current removal leaves the retained
member and an empty rebuilt aggregate; stale settings and forged member roles
reject without changing prior captures. A real native shared-adoption Store
snapshot accepts the two process inputs. The original ART oracle validates cached
PackageSetting snapshot identity, two distinct same-name members and the
retained-only removal capture (30.58s). Unit tests pass 441 with one ignored/not
run; the image/template build passes (65.7s), all integration targets compile,
and the complete initial scan passes (68.05s). The original shared process
union/overwrite/rebuild/removal and immutable facade snapshot oracle also passes
(11.42s). The native daemon now exposes an initial scan entry point through its
retained early package bridge (#836/#885). BootOwners captures original APEX,
resolved users, migration policy and library compatibility against the same
bridge and immutable SystemConfig; caller image/ABI/installation policy remains
explicit. The scan parses original containers, completes native APEX registration,
notifies the original owner before APK directory loading, and uses that same
bridge for domain IDs and seInfo compatibility. Owner/malformed notification
failures abort without publishing a snapshot. The same daemon bridge also accepts a restored SigningScan, saved user
states and explicit resource/stub/incremental owners for the system phase of a
saved boot. It registers APEX and completes the original notification before
reading APK directories, then invokes the existing factory/version/signature and
metadata reconciliation, returning retained factory/data candidates separately.
The caller's mutated owner remains authoritative after earlier effects; the
phase does not discard it on a later error. Initial and saved phases share their
exact original input/callback construction. The expanded controlled-owner Binder
scan test passes (9.85s): unchanged original framework APK, restored native settings,
retained registration UID and per-user enabled/installed/hidden values, fresh
original domain owner output, unloaded-to-loaded transition, old-owner isolation
and missing-domain/notification/denied-owner rejection. The expanded ignored test
ran explicitly; the other ignored test remains not run. These entry points
are not invoked by the default boot and do not complete full data reconciliation,
metadata owners, facades or service publication. The early bridge now exposes
AndroidTestBaseUpdater's original PlatformCompat change 133396946 through generated
AIDL, rebuilding full ApplicationInfo from native parsed-package cache. Rust skips
the query for a classpath containing test.base and for effective system ownership;
other scans require the original decision. Owner errors and malformed replies
remain errors. The original ART oracle compares bridge and original IPlatformCompat
results for SDK 29/30 pre-registration non-system metadata (UID -1), validates
untrusted-caller rejection and generated reply framing (30.70s). Ordinary units
pass 441 (3.20s); all integration targets compile; the latest host/image/template
build passes (17.4s, three rebuilt, 39 fresh). Data admission now resolves current
factory/system ownership and applies manifest restrictions before querying the
original test-base owner (#908). System updates, factory recovery and test.base
on the boot classpath never query it. Ex-system rescans query after disabled
ownership is removed; required owner errors abort without deleting valid code.
The expanded original-APK native scan fixture passes (67.90s), including SDK
29/30 decisions on policy-adjusted ordinary data metadata, zero queries for
classpath/system/factory cases, and ex-system owner failure after demotion.
BootOwners retains the same original bridge, users and compatibility through
system and data phases. The daemon's restored-boot entry point derives platform
signing from scanned original android code, then loads data/private-volume APKs
and runs admission, ex-system rescans and factory recovery with the same domain,
seInfo and test-base owners. Earlier effects remain on the caller's owner after
failure. The controlled Binder fixture verifies restored system followed by an
empty data inventory (9.85s); it does not prove nonempty full-boot persistence or
publication. Native Store now persists a validated scan capture's complete
active/disabled package metadata, live shared UID inventory, signatures and
registered keysets into the original resilient ABX document (#798). Unowned
global changes, missing domain IDs, unfinished captures and original-writer
nullable MIME failures reject before writing. Unknown extensions survive and
cleared ABI fallback fields do not reappear. Package, install-initiator and
shared-group signatures share one rewritten certificate table (#909). Shared
UIDs retain original ArrayMap signed UTF-16 hash order and collision insertion,
including removal/recreation and restored inventories (#910). Active and disabled
package vectors preserve insertion slots through replacement/removal; native
persistence emits stable signed Java hash order without mutating the captured
owner (#911). Units cover colliding names, negative hashes, replacement,
removal/recreation, restoration and old captures. Original ART compares the
complete two-package/live-group inventory with pinned Settings.writePackageLPr
and ArrayMap values order using one certificate table. Saved-setting signer
lineage flags remain separate from parsed-code signing. The oracle also reads
reindexed real certificates through PackageSignatures.readXml and checks
ArrayMap signed hash/collision behavior (2026-10-05, 24.02s), starting from
an absent native settings file. Store::create claims first-boot absence without
writing a seed document and uses the original related user/access/list readers.
Construction rejects every existing main/backup/reserve artifact, including
malformed files; another claimant's write is rejected. Initial writes retain
cloned descriptors for every file actually opened, pinning their inodes through
failure. Failed main writes retain only their own exact uncommitted output;
changed bytes and replacement files are rejected before retry. Partial-write
and descriptor-acquisition failures leave prior settings/state unchanged, repeat
failures release removed descriptors, and successful retry clears pending ownership.
A late reserve-finalization failure publishes the committed main and the next
write restores the reserve. Original ResilientAtomicFile agrees on partial main
failure, empty retained reserve, descriptor-open failure and successful retry. Domain validation failures leave the directory unwritten, complete
user XML survives, and a successful main commit changes the owner to restoration
mode. An explicit exclusive recovery plan claims regular settings inputs with
pinned descriptors/exact bytes and directory roles with pinned identity/timestamps,
then follows original backup/main/reserve selection and failRead cleanup (#912).
Backup directories fail open and fall through to main; unused reserve directories
are not parsed. Backup selection attempts main/reserve cleanup before parsing:
empty directories are removed, while nonempty directory deletion failure is
recorded and the directory is preserved, as original File.delete does. Selected
main/reserve directories expose their open error. Four additional original ART
cases verify directory selection/cleanup and resulting path kinds (full oracle
42.17s, no skips), while native units verify external directory changes reject
before cleanup (500 passed, 0.53s; five excluded). Production build passes
(3 rebuilt, 39 fresh, 2.5s). Other nonregular/permission/open-I/O behavior and
complete Settings frontend/boot orchestration remain #912/#914/#798.
Backup cleanup now treats deletion as best effort for regular files too, reporting
RemoveFailed while continuing the selected backup read; failRead's required
selected-file deletion still fails visibly. A readonly-parent test exposed a
lower-layer gap: guest chmod retains physical host owner access, and unlinkat
had no guest parent DAC/sticky checks (#967). unlinkat now checks recorded guest
directory write/search and path-walk search using fs IDs, supplementary groups
and capabilities, plus sticky file/directory ownership or CAP_FOWNER. Search
checks include directories traversed before symlinks or dot-dot alter the final
path; fd-relative paths start from their actual base without checking ancestors
above it.
The original ART fixture temporarily changes effective UID to 1000 and restores
it; root-only comparisons initially removed the files, and nonroot comparisons
also removed them before the syscall correction. The corrected original recovery
matrix agrees on readonly cleanup failure (full oracle 31.43s, no skips).
501 service units pass (0.50s; five excluded), 124 Linux ABI units pass (0.03s;
two excluded), image/template build passes (6 rebuilt, 36 fresh, 56.3s), and
runtime/translation build passes (3 rebuilt, 39 fresh, 6.7s). Mapped syscall tests
now cover denied symlink prefixes/dot-dot traversal, relative dirfd access below
an inaccessible ancestor, sticky symlink ownership/rmdir/CAP_FOWNER, and ENOENT
versus EACCES. A dirfd test exposed canonical host aliases in inverse mount
lookup (#968); lookup now recognizes canonical roots while preserving deepest,
shortest-name and hidden-mount selection. 127 Linux ABI units pass (0.04s; two
excluded), full original ART oracle passes (52.76s, no skips), and runtime/
translation build passes (3 rebuilt, 39 fresh, 6.8s). Host build passes (2 rebuilt,
11 fresh, 4.0s). Canonical host alias mapping is verified; broader namespace
mutation/mountpoint/error validation remains #967, and recovery/open-I/O gates
remain open. unlink/rmdir now retain the original final-name kind before mount
write/DAC checks: root, dot and dot-dot return their distinct Linux errors; a
trailing slash on unlink uses no-follow target kind and never removes a file or
symlink. Mapped tests verify file/directory/symlink/missing slash paths, root and
dot/dot-dot errors, and unchanged targets on failures. Results follow Linux
v6.12 namei do_unlinkat/do_rmdir control flow. 128 Linux ABI units pass (0.05s;
two excluded), full original ART oracle passes (52.77s, no skips), and runtime/
translation build passes (3 rebuilt, 39 fresh, 17.9s). Active own file/directory
mounts and visible mapped directory roots now reject removal without touching
backing nodes: type/DAC checks precede EBUSY, and writeability is checked on the
parent mount, as namespace removal changes its directory. Tests verify readonly
target mounts under writable parents, readonly parent precedence (EROFS), final
symlink removal, stacked mounts/unmount visibility, and preservation of backing
files/children. 129 Linux ABI units pass (0.06s; two excluded), full original ART
oracle passes (52.33s, no skips), and runtime/translation build passes (3 rebuilt,
39 fresh, 17.4s). Other namespace mutation/error cases remain #967.
No-start-tag documents remain, whereas malformed reads remove
the selected file and retry. Changed or replaced claimed files reject before
cleanup. Optional XML root parsing distinguishes empty/whitespace and complete
rootless ABX from truncated ABX. A real child process exits during its first
partial write; a new recovery owner cleans the partial main, claims the empty
reserve and completes a write that reopens successfully. Ten disposable
regular-file recovery cases agree with actual original ResilientAtomicFile and
Xml parsing in the ART oracle (24.02s). Its first-boot flag uses the pinned
Settings control-flow rule; that file-only matrix does not exercise original
Settings.readSettingsLPw. A separate version-owner oracle now directly calls
original Settings.readSettingsLPw on 28 disposable text/ABX cases (24.02s):
required-field failure retains earlier SDK assignments and existing DB/fingerprint
fields, a newly referenced volume exists before its first failing attribute,
legacy defaulted getters tolerate malformed values, nested attribute-only tags
remain visible, and unknown subtrees are skipped. First-boot results and remaining
main/reserve files agree. The fixtures supply a valid empty reserve after failures;
absent-file forceCurrent and complete package/global recovery are not covered.
Native version events mutate the existing volume owner in place, and the normal
whole-document reader uses the same owner for modern and legacy version records.
The recovery callback preserves frontend mutations before failure. A native incremental
XML event reader now exposes completed starts/attributes and ends before later
errors and lets the owner stop at the outer root end (#914). It follows original
text/ABX EOF behavior and text coalescing; required-root whole-document readers
remain strict. Actual original Xml event traces match 234 cases (text and ABX
at every truncation boundary, attributes/depth, text/CDATA/comments, malformed
input and outer-root stop), in the full original ART oracle (24.02s). A native permission-container owner now applies each completed item start before
skipping its subtree, retains prior definitions after later XML errors and merges
repeated containers by name. The whole-document reader shares that item owner.
Actual original Settings.readSettingsLPw agrees on 159 additional permission
cases (text/ABX, every ABX byte truncation boundary, duplicate/repeated definitions,
unknown/nested subtree skips and reserve retry), including first-boot flags and
remaining files. These cases use the incremental version/permission frontend and
preserve applied permissions across failRead retry. Native legacy permissions now carry immutable manifest/config/dynamic owner types
and configured UID/GIDs. A repeated XML definition updates protection while
retaining a configured owner's package/type/UID/GIDs; only incoming dynamic XML
changes its icon/label. An additional 167 original Settings cases seed configured
permission/tree objects and check exact Java object retention, native UID/GIDs,
owner types, metadata and failed-read/reserve-retry behavior. Actual original ART
passes all 411 signature/permission/keyset-owner cases (85 default, 159 incremental
permission and 167 configured-owner cases). Bootstrap delivery of configured
owners, nullable configured PermissionInfo identities and complete permission
writer integration remain #917; this fixture supplies explicit nonnull identities. An incremental SignatureReader now owns one read attempt's shared certificate
table separately from its signing targets. Completed cert starts append table
entries/flags before subtree skip; XML failures retain table effects and the prior
signing target, while completed reads publish the built result. It returns current
certificate flags for its caller. Invalid DER clears the signing target after
table mutations. Direct original PackageSignatures comparisons match 2,544 cases:
all ABX byte truncation boundaries, malformed text/default counts, certificate
append/clone flags, repeated/nested histories, invalid DER and a following signature
read sharing the same table. This is a within-attempt signature-owner comparison,
not Settings failRead file retry (each new file attempt needs a fresh table).
The normal AST importer now shares the signing builder with SignatureReader;
Native Settings.read_package now registers active package headers before consuming
their body with Package.read_children. One PackageReadAttempt owns certificates,
keyset references, pending shared-UID packages and deferred first-install times;
retry creates a fresh attempt without erasing registered package/UID owners.
Same-name/same-UID reads retain construction fields and accumulated children while
updating metadata; different-ID duplicates and occupied UID slots are explicit
rejection outcomes. Shared-UID packages enter pending before their body with appId zero and a
separate sharedUserAppId, without registering a separate slot. Loading progress remains monotonic; page-size flags
merge on the registered target, so an invalid mode retains preceding header/UID
effects. First-install timestamps publish only after the body completes.
The body connects the shared SignatureReader/keyset table and delegates legacy
permission/domain children to their owners; external errors propagate without
rollback. Library/split changes precede subtree skips, keyset changes leave nested
entries visible, and MIME groups publish only after their container completes.
Actual original Settings.readSettingsLPw agrees on 866 text/ABX package cases,
including post-header truncations, accumulated children, retained path/flags/domain
ID, updated version, monotonic loading, merged/invalid page modes and partial
effects across empty-reserve retry. Native units additionally check UID rejection,
pending-before-failure and deferred first-install timestamps. Native
Settings.read_shared_user reserves its group slot before signatures/legacy
permission events, retains existing same-ID group flags and reports conflicting
name/UID records. PackageReadAttempt.resolve_pending processes the surviving
attempt in order: missing/non-group slots reject, valid groups publish a package
with the group's appId/sharedUserAppId. The binding callback receives the UID owner
and displaced package so it can preserve prior objects and update group/user
state; errors propagate with pending records retained. A native unit retains the
previous non-shared UID object before replacing its package-map entry. Actual
original Settings.readLPw agrees on package/group inventory projections for 238
additional text/ABX cases (registration order, collisions, same-ID flag retention,
truncation and empty-reserve retry). These use no users and exclude group
membership/aggregate flags, legacy permission and user-binding side effects.
Package, body and shared-UID reader callbacks now return a shared typed ReadError.
Exclusive recovery's recover_with_owner sends File failures through failRead
cleanup/retry, while Owner failures emit OwnerFailed and return without deleting
the selected input or retrying. An actual package-reader owner failure test retains
its registered package, UID slot and earlier version mutation for main and backup
selection. Backup openRead cleanup of unused main/reserve still precedes frontend
execution, as in the original. A separate File-error test removes corrupted main
and selects reserve. The string-error recover entry remains for file-only parsers;
native callbacks use the typed entry. Full error-source classification in the
remaining native frontend owners is still required (#914/#912).
Native Settings.read_events now supplies the top-level readSettingsLPw dispatcher.
It applies version, permission and rename records in event order, leaves
attribute-only/deprecated subtrees visible, skips unknown subtrees and stops at
the outer root end without consuming trailing input. Package/shared/global
records use explicit owner callbacks; an unhandled recognized owner returns a
typed Owner error, preserving earlier mutations rather than skipping valid data
or entering file cleanup. The actual package-reader owner-failure recovery test
now runs through this dispatcher. Package/shared-UID and version/permission ART
projections now use the production dispatcher, with 34 original version cases
including nested preferred-packages, renamed-package and read-external-storage
records. Unit checks cover rename replacement, unknown subtree skip, trailing
malformed input, no start tag and missing-owner classification. The pull reader now optionally captures every consumed token, including
helper/skipped subtrees, typed attributes, comments and processing instructions,
without a second parse or reading past root end. EOF closes the consumed event
snapshot as the original reader permits; XML failures prevent document export.
Settings.read_document runs the dispatcher and returns the full captured document
only after a successful attempt. Version/permission recovery projections now use
this actual document rather than a synthetic root marker. A native owned
read/commit/reopen test retains unknown attributes/extensions, comments and an
in-element processing instruction in packages.xml. Text AST reads previously
lost in-element processing instructions (#969); they now retain the token,
matching pull/ABX capture. Mixed text/comment/CDATA/entity and typed ABX capture
comparisons pass. Remaining global/keyset/domain/verifier/migration owners,
group/user/permission binding and complete boot/retry/write orchestration remain
#914/#912/#798; full native Store frontend and default boot are still unproved.
Incremental Settings.read_key_sets now updates pool records/counters as each start
completes, retains partial mutations on later read errors, exposes nested records
under unknown tags, replaces duplicate keyset slots and deduplicates key IDs in
ArraySet order. Unversioned containers clear active package roles only after
successful subtree consumption and retain an existing pool. Public-key decoding
and runtime reference finalization are explicit owner callbacks; finalization runs
only after successful container completion. The missing-current-mapping case
follows the original uncaught NPE path through typed FatalInput, preserving the
selected file and preceding counter effects rather than entering failRead.
Native restore now tolerates absent saved package-role references as the original
warn/continue path does (#970), while mutation validation remains separate.
Actual original Settings agrees on 158 additional keyset pool/counter/package-role
and handle-reference projections, including post-package ABX truncation, nesting,
duplicates, malformed counters, unversioned/repeated containers and fatal input.
These projections use empty public-key mappings with the actual native restore;
full public-key factory/live reference/mutation/retirement integration remains
#914/#916/#824/#823. A native fatal-input recovery test preserves the selected
file; a missing-role restore regression creates no fake handles.
Keyset retirement now validates pool structure separately from saved package
roles: decrementing an absent role handle does nothing, then proper/alias/upgrade
roles are cleared, matching original removeAppKeySetDataLPw. No handles are
synthesized and counters do not rewind. Native units cover absent roles mixed
with live/shared sets, an absent-only owner and unknown package rejection.
The boot error-retention test now injects a genuine pool/counter invariant error
rather than treating an absent saved role as failure; earlier domain removal
still survives, and restoring the pool permits later retirement. Each of the 158
original keyset read-order fixtures now also invokes actual original retirement
and compares the resulting pool, counters, package roles and handle references.
These are empty-public-key mappings; full live public-key count/retirement and
boot/uninstall integration remain #823/#824/#914/#798.
System.recover_package_settings now places exclusive recovery under the retained
early package bootstrap owner used by scan. It validates bridge identity before
file selection, before/after each frontend attempt and before returning the
unpublished Store. A replaced/dead/stale owner aborts; typed frontend errors retain
partial state. The Binder bootstrap fixture exercises actual captured-document
recovery, native owner failure, bridge replacement during the read (selected file
preserved, no Store returned), and stale-owner rejection before backup selection
(main/backup unchanged). Complete record owners are supplied by the caller;
guest-init/SystemServer orchestration, full frontend and original-name activation
remain #914/#912/#798. This entry does not run or replace original PMS.
The verifier tag now has a native VerifierDeviceIdentity.parse owner in both
incremental dispatch and AST import. It validates the 64-bit Base32 value,
normalizes arbitrary separators/case and ambiguous 0/1 letters to the canonical
form, and publishes only after successful decoding. Invalid characters/start
bit overflow/length errors enter file recovery; a null attribute is FatalInput
as the original uncaught NPE, retaining earlier identity. Actual original Settings
agrees on 392 ABX read/retry cases with synthetic fixture values, including all
ASCII characters in first/middle/last positions, aliases, Unicode, null and
length/overflow failures. The previous raw-string AST import is superseded.
Compile-only VerifierDeviceIdentity/Settings API declarations were checked against
the pinned image; an initial implicit no-arg constructor mismatch was corrected
to the original long constructor. No identities are generated/spoofed and no
original guest code is modified. Domain SettingsXml cursor/error/merge semantics
and full remaining owner/bootstrap integration remain #914/#957/#798.
Native legacy domain reads now follow DomainVerificationLegacySettings's shared
SettingsXml cursor/depth stack. Unknown tags expose nested user-states/user-state
starts; each user value changes before later reads, duplicate IDs replace, and
malformed/default attributes use the original defaults. Section parser exceptions
end that move without popping the original stack, retain prior state and return
explicit depth/message diagnostics. Accepted section errors resume document
capture, while unaccepted errors still prevent export; the dispatcher logs the
section diagnostics. AST legacy import delegates to this same event owner rather
than its previous direct-child-only traversal. Actual original legacy owner
read/write agrees on 133 text/ABX cases: nested unknown tags, duplicates,
null/empty package names, defaults, malformed text, trailing input and ABX
truncation. The projection excludes empty/null SparseIntArray distinctions and
legacy attached/info lifecycle; full modern domain cursor/merge and native
bootstrap integration remain #957/#914/#798.
Modern DomainVerificationPersistence.readFromXml now has a native event parser
using the same shared section cursor, including expected-name filtering that
exposes matching nested records. It builds detached active/restored ReadResult
maps, domain/user state and URI-relative groups, replaces duplicate package/user
keys, skips empty package identities and retains section error diagnostics.
UUID parsing is an explicit owner callback; invalid UUID aborts before result
publication. AST import delegates to the same parser and stages its destination
updates, superseding the previous direct-child/in-place mutation path. A regression
keeps the existing destination unchanged if a later package UUID fails. Actual
original persistence read/write agrees on 333 text/ABX projections, including
unknown nesting, duplicate maps, null/default domain/user data, URI groups,
partial input, invalid UUID and ABX byte truncation. Map iteration order is
normalized in the projection; live attached/pending/restored merge, full order,
process UUID policy delivery and runtime publication/default activation remain
#957/#914/#798. The dispatcher still requires the modern domain owner callback;
it does not silently publish detached persistence as live state.
Native domain Owner.read_settings now applies completed read results in original
ArrayMap package order. Same-ID attached state is retained without querying the
code owner; changed IDs consult current valid auto-verify domains and promote
only absent/no-response domains whose incoming status is success/restored to
STATE_RESTORED. Fresher statuses, attached UUID/signature/URI groups remain owned
by the live object. User hosts are unioned in ArraySet order, new users attach in
SparseArray order, and incoming link handling replaces the previous value.
Unattached active entries update pending state; restored entries update their
separate map after active processing. Collector-owner errors propagate and retain
earlier processed effects. Actual original DomainVerificationService.readSettings
agrees on 32 two-stage live read cases across same/changed IDs, domain responses,
code absence/presence and link-handling values, with pending/restored records.
The projection normalizes map order; full identity/alias, all state combinations,
shared publication, original-name registration and bootstrap execution remain
#957/#914/#798. Initial fixture failures were an unavailable Files.readString
API (replaced with image-supported readAllBytes) and an incorrect 64-case loop
bound (corrected to the generated 32 cases), then the same oracle was rerun.
Capture.prepare_domain_settings_read now merges a completed domain read against
that exact scan's loaded code and captured collector policy, then prepares a new
runtime-only version. It carries section diagnostics and clones the prior owner;
preparation does not publish or write disk. Existing System runtime publication
compares bridge/base/version before swapping scan/query owners. The explicit
retained-owner framework-res scan test now publishes a changed-ID read result:
user query state changes in the new capture, the old capture remains unchanged,
live UUID is retained, settings bytes/persistence owner/nonce invalidations stay
unchanged, and a stale prepared read is rejected. A later actual domain commit
still uses the current generation. Parser-to-bootstrap orchestration, full
identity/order/state-matrix coverage, all owners and original-name activation
remain #957/#914/#798; this API does not activate original-name native services.
Native boot Settings recovery now takes explicit current VersionInfo values.
openRead absence force-updates internal and primary-physical versions, including
recursive failRead retry after earlier partial version mutations. Other successful
reads/no-start-tag retain existing version owners and add only missing core
volumes before related user state restoration. The outer finally path also
initializes missing versions after uncaught input/native owner errors. Generic
file-only recovery keeps its separate semantics. System.recover_package_settings
uses current build-version values from the retained original bridge and this
boot lifecycle while retaining its bootstrap identity guards. Units cover absence/no-start/success,
Store/settings coherence and selected-file preservation after fatal verifier input.
Actual original Settings.readLPw agrees on eight lifecycle cases: absent/empty/
valid data, saved internal/external versions, failed-read retry with absent or
empty reserve, and fatal input. Fingerprints are normalized to equality with the
original forceCurrent values; the oracle checks the pinned SDK/database versions.
Complete user/permission/group/global state and SystemServer orchestration
remain #914/#912/#798; native PMS construction/default activation is unproven.
Current build values now come from retained original bootstrap AIDL ownership.
PackageBootVersion.capture calls the image's Settings.VersionInfo.forceCurrent
and emits SDK/database/build/partition values in a Parcel frame. The bridge's
getCurrentPackageVersion is system-UID-only; native Bridge decodes the generated
transaction reply and rejects null/truncated/trailing/misaligned frames.
System.recover_package_settings requests these values before file selection,
revalidates the same bridge after the call and uses them in boot recovery;
caller-supplied fixture/current values are no longer its input. Actual original
system-UID policy oracle verifies all four payload values against forceCurrent,
and the untrusted-UID policy path verifies denial. Native frame tests retain null
strings and reject every truncation/trailing value. Initial fixture failures were
package-private generated transaction constants, missing bridge classpath and
UID-0 security rejection; the raw transaction helper now lives in the AIDL package
and runs through the existing system-UID policy oracle, without weakening the gate.
System.initialize_package_shared_users now connects constructor shared UID
inputs to fresh native Settings, AppIds and permission-owner hooks under the
retained early bridge. Settings.initialize_shared_bootstrap consumes the existing
platform/OEM Bootstrap owner in its original map order, registers each UID and
constructs its shared setting/legacy state before XML reads. It rejects reuse of
non-fresh package/shared/UID tables, returns OEM rejection records through System
and preserves earlier constructor effects if an owner later fails. Native metadata
retains seed flags on repeated shared-user reads. This initializes the read
frontend; full group/member/runtime lifecycle and default PMS construction remain
#803/#914/#798. Eight text/ABX seeded-read cases compare actual original
addSharedUserLPw/readSettingsLPw flags/private flags, seeded UID identity and
LegacyPermissionState byte frames. They cover untouched seeds, retained system
flags despite XML system=false, package permissions before an XML group exists,
conflicting saved IDs and a permitted vendor UID. Fresh-table reuse is rejected;
System units validate retained-bridge initialization and its repeated-call error.
The full original ART oracle passes without skips (69.53s); 535 units pass
(5 excluded, 0.52s), and runtime build passes (3 rebuilt/39 fresh, 15.5s).
Exclusive recovery now also exposes ReadStage::Complete through
Plan.recover_boot_frontend/System.recover_package_settings_frontend, after core
version initialization and before related user/runtime files or Store publication.
Initial absent/no-start continuation does not call completion; corrupt-read retry
retains the outer continuation semantics. Completion must return no document;
owner failure reports CompletionFailed and preserves selected persistence bytes
and prior native mutations. The retained bridge is checked before/after file and
completion stages. Caller pending/group bindings remain explicit and incomplete;
this orders their required lifecycle without claiming complete membership/user
side effects (#914/#798). A disposable native test fails binding while the user
file is invalid, preserves pending/input, then completes attachment before reading
its corrected restriction and publishes the attached package state. System units
verify stage order, current versions and completion owner failure preservation. Full original ART comparison passes without skips (69.53s),
including earlier recovery/version contracts; 536 units pass (5 excluded, 0.59s).
Initial false continuation now builds constructor package-user defaults without
reading restriction/runtime-permission files, including Store document loading
(#977). Access/list state stays with its separate owners. Recursive failRead keeps
its outer true continuation and restores package-user files; related errors remain
visible. Users whose restriction documents were skipped cannot be mutated by
enabled/removal/preferred writes until actual owner readiness. Store now exposes
claim_unread_restrictions and commit_initial_restrictions: the claim opens regular
main/backup/reserve files without XML parsing (O_NOFOLLOW), pins descriptors and
bytes, and verifies directory/file identity and contents before the initialized
write. The completed document must have exactly the current package inventory;
external changes reject before mutation. The original atomic-write owner handles
backup/main/reserve, and failed main writes retain recognized output claims for
retry. Main commit with reserve failure publishes state and clears unread guards.
Native fault checks cover malformed input, old backup preservation, successful
retry, same-inode outside writes and committed reserve failure. Real native scan/
user readiness, whole first-boot owner handoff and original reboot/app checks
remain #978; ordinary writes remain guarded before this explicit initialization. Native initial-write interoperability now has two controlled
initialized user documents over the previously unread malformed main/reserve
files. Main/reserve bytes match, native Store reopen agrees with its published
state, and the original Settings.readPackageRestrictionsLPr getter probe checks
installed/stopped/not-launched/hidden/enabled, CE/DE inodes, first-install time,
enabledCaller and enabled/disabled components. Actual scan/user generation and
full native boot/reboot/app readiness are still separate #978 gates. RuntimePermissions.serialize now provides the permission module's text XML
format with explicit version/fingerprint, package/shared owner order, empty owners,
hex flags and one-time grants written as false without clearing their flags.
Attribute escaping preserves tabs/newlines/CR; invalid XML characters reject.
Negative flags retain the original unsigned-hex writer behavior and signed-reader
failure. The direct original persistence oracle uses disposable user 42 files,
reads native XML, invokes the actual original writer and compares the resulting
state through original getters and native parsing. This codec does not yet supply
live version/fingerprint or synchronized permission-owner state; that whole owner
is tracked in #981. Store now explicitly claims runtime main/.bak/.new/reserve
inode identities and bytes. commit_runtime_permissions requires current state and
complete guest creation metadata, restores a legacy backup, writes/syncs .new,
renames to main and then writes/syncs the reserve. Main-write failure removes the
owned temporary file and retains the recovered old main; a reserve failure
publishes the committed disk projection and reports committed=true. Retained
claims recognize native outputs for retry and refuse external byte/inode changes.
The original runtime oracle now consumes this native atomic writer's main output
and checks identical reserve bytes before original reading.
Bridge.runtime_permissions now builds package/shared rows from the native capture
roles, querying each current UID permission state through the retained original
permission service. It does not read saved migration grants. Non-shared empty
package states are included only when install permissions are fixed; every shared
owner is included. System.commit_runtime_permissions_from_scan checks the exact
committed scan, validates the bootstrap before capture and immediately before/after
atomic persistence, and uses runtime_metadata::State for version/fingerprint plus explicit inode
metadata. That native sparse owner restores captured read metadata, retains null
fingerprint presence, implements original version-0/upgrade-true constructor
defaults, version mutations, controller upgrade comparisons, user removal and
mandatory pending write requests. Updating a fingerprint before controller setup
fails without changing state. System configures the extended fingerprint from the
retained original PackagePartitions identity and supplied controller version;
callers no longer pass arbitrary version/fingerprint values to the disk commit. Saved boot restoration now
creates runtime metadata and legacy migration roles together from the Store's
captured read state. User inventory must match exactly; missing captured metadata,
foreign setting identities and false-read/skipped user restoration reject. The
candidate scan is published only after the retained bootstrap identity recheck.
First-boot constructor metadata remains separate from saved restoration; no file
is reopened to fabricate a skipped read result. The restore unit verifies
saved version/fingerprint, missing-file rewrite requests, changed package identity
rejection and skipped first-boot immutability; System bridge tests cover restored
metadata transfer and missing-user inventory refusal. 551 service units pass
(6 excluded, 0.56s), host build passes (3 rebuilt/10 fresh, 19.8s). Original ART
metadata/atomic comparison previously passed (61.42s); no new original rerun was
made for this restore coordinator. Metadata write requests
now remain queued until a complete successful write acknowledges the user.
System.flush_runtime_permission_requests provides a synchronous persistence
boundary in user order, returns already completed users with a later failure,
and retains both failed and unattempted users. A main commit with reserve failure
also remains queued for repair/retry. The destructive take-all request API is
removed. Controlled scan integration persists user 0, fails user 10 on missing
creation metadata/producer responses, retries user 10 and reopens disk state;
a separate queue test covers committed reserve failures with a later user still
pending. Timed asynchronous scheduling and whole-boot invocation remain #981. Latest checks:
550 service units pass (6 excluded, 0.55s), retained-bootstrap/framework-res
scan passes (1.16s), host build passes (3 rebuilt/10 fresh, 16.5s). Original ART
metadata/atomic comparisons passed on the preceding metadata revision (61.42s);
they were not rerun for this request-drain coordinator. The
original Settings metadata fixture checks constructor defaults, missing-controller
failure, version -1/7, current partition fingerprint extension and controller
upgrade transitions. Its setters schedule asynchronous writes, so its empty graph
supplies the required legacy producer and actual original persistence factory;
UID lookups on that empty graph reject. Initial null producer/persistence
dependencies caused signal 11 failures after metadata comparisons (#984); the
corrected full original run passes without skips (61.42s), including the async
writer. 549 service units pass (6 excluded, 0.57s), retained-bootstrap/framework-res
scan passes (1.25s), API/image/template build passes (6 rebuilt/36 fresh, 70.9s).
Malformed/null producer records abort before file creation; persisted one-time
grants are false while the live producer projection remains true. Per-UID queries
are not yet one coherent permission-service generation. Production metadata and
metadata owner lifecycle/scheduler binding, controller-version authority,
producer-wide synchronization, directory creation
context, fs-verity policy and whole boot invocation still require #981 integration.
The retained-bootstrap/framework-res scan passes with controlled live permission
replies (1.15s), including malformed/null response refusal, role-aware projection,
one-time disk grants and native reopen. 548 service units pass (6 excluded, 0.55s),
including fixed-empty versus unfixed-empty package rows and unknown-user rejection;
host build passes (3 rebuilt/10 fresh, 17.8s). The original ART codec/atomic/metadata oracle
passes without skips (61.42s); coherent producer-wide capture remains unverified.
The runtime reader now applies ArrayMap duplicate replacement: package/shared
rows keep the last value, sort by signed Java hash and retain collision insertion
slots (#982). A direct original fixture checks repeated BB/Aa collision keys,
a later z key, replacement permission contents and an empty final shared row;
original reserialization and native parsing agree.
Runtime package/shared keys and permission names now retain Option<String>,
so null and empty-string identities stay distinct, including their hash-zero
collision slots. Migration receives nullable permission names without coercion;
known package/shared restoration matches only Some(name). Null owner or
permission attributes return a serialization error rather than inventing an XML
name. The direct original fixture checks nullable identities, repeated null keys
and main-file preservation after original null-attribute writer failures (#983).
Both null-owner and null-permission writes leave the original main bytes intact.
Host build passes (3 rebuilt/10 fresh, 24.5s), including runtime atomic
rollback/retry, external byte mutation refusal and committed reserve failure.
Native IPackageManager now handles generated getRuntimePermissionsVersion and
setRuntimePermissionsVersion transactions against an explicitly installed runtime
metadata owner. A bootstrap may install that owner only once. Access is tied to
the exact current query capture; absent metadata or stale captures reject. The
endpoint rejects trailing data and negative arguments before checking original
ADJUST_RUNTIME_PERMISSIONS_POLICY/UPGRADE_RUNTIME_PERMISSIONS permissions,
including root/system app-ID and isolated-UID Context rules. Setters enqueue real
runtime writes; no missing-owner version is invented. The installed-owner flush
now consumes that same metadata Arc, so Binder-enqueued changes reach atomic
persistence. Foreign bootstrap checks also run on empty queues; completed effects
are retained if bootstrap identity changes after flushing. Binder operations clone
the owner under bootstrap lock, release it before waiting on metadata, then
recheck current capture/owner under consistent metadata-to-bootstrap order. This
removes the earlier bootstrap-to-metadata inversion (#985). A bounded concurrency
probe holds metadata while a reader waits and confirms query capture remains
available; both worker threads join. Controlled Binder setter-to-flush checks
persist version 13 and acknowledge the installed queue while foreign flushes leave
requests intact. Runtime metadata mutations now assign
monotonic per-user deadlines with the pinned 700–1299ms integer jitter. Repeated
changes preserve the first mutation and cap postponement at two seconds; user
removal and successful acknowledgement cancel deadlines. The installed-owner
process_due_runtime_permission_requests boundary processes only elapsed requests,
retaining failed requests. Clock-injected policy tests cover debounce/cap/removal;
controlled Binder tests show no early write, failed due-time producer retention
and successful due-time persistence. The owned runtime timer now subscribes to metadata mutation wakeups, waits for
monotonic deadlines and invokes the installed-owner persistence boundary. Its
caller-held guard stops and joins the thread on drop; failed writes remain queued,
are logged and exposed as structured errors, and await mutation or explicit retry
instead of spinning. Joined guards detach so the owner can start a new timer.
System.start_runtime_permission_worker supplies current query capture, retained
bridge, explicit Store and creation metadata. Controlled Binder mutation to version
14 now persists automatically without a manual deadline pump; worker unit coverage
includes failure/retry and restart after join. The retained bootstrap now owns a
cancellation handle: replacement/death stops its timer without joining under the
bootstrap lock. Explicit stop rejects foreign bootstrap owners; the caller-held
guard joins outside that lock. Idle cancellation and Binder-driven persistence
followed by owner stop are verified. Production bootstrap must still retain and
join the guard; coherent producer snapshot and creation/controller policy remain
#981, and native default/CTS/app gates remain #798. 555 service units pass (6 excluded,
0.77s), retained-bootstrap/framework-res/Binder automatic-worker scan passes (2.24s),
host build passes (3 rebuilt/10 fresh, 23.1s). No new original differential or
CTS/app gate was performed for the timing policy.
 Controlled Binder tests
cover default/updated versions, request creation, install refusal, stale captures,
invalid input and permission denial. These endpoints remain test aliases pending
the native default/CTS/app gates (#798); metadata scheduling and coherent grant
capture still require #981. 552 service units pass
(6 excluded, 0.55s), retained-bootstrap/framework-res/Binder scan passes (1.29s),
host build passes (3 rebuilt/10 fresh, 16.4s). Source permission/argument rules were
checked at the pinned version; no new original Binder differential run or CTS/app
gate was performed for these two endpoints.
System.commit_package_list_from_scan now generates rows from the immutable native
scan/query capture, in signed Java-hash setting order. Unloaded settings and APEX
are omitted; system-user data paths come from setting volume/storage and captured
installed/dataExists policy, with the original space-path omission. Parsed code
supplies UID/version/debug/profile flags, setting owners supply seInfo and installer.
The exact committed scan projection is checked before querying GIDs from the
retained bootstrap permission owner for the caller's resolved active users.
Bootstrap identity is checked again before and after commit; list read-back retains
GID order/duplicates. The legacy IBridge GID reply also rejects trailing Parcel
words (#980). This list entry point is not yet production first-boot orchestration;
full kernel mapping/runtime permission writes and native PMS switch remain #798. The retained
bootstrap/framework-res scan passes (1.19s), including generated platform rows,
foreign-owner rejection, malformed GID refusal before writing and persisted
multi-user GID duplicates. 542 service units pass (6 excluded, 0.54s), including
legacy GID reply tail rejection and unloaded-setting omission; host build passes
(3 rebuilt/10 fresh, 23.6s). Broader original packages.list comparisons and
native boot/CTS/app acceptance are not established by these checks.
Store.commit_initial_scan_restrictions now generates scalar/component
package rows from the exact SigningScan.scanned_user_states and current package
inventory, rejecting missing user owners or mismatched global settings. The
settings check compares the actual scan writer projection against committed
state, so legitimate old-path/legacy-time normalization and APEX omission do not
reject a committed scan. Uncommitted metadata still rejects before user writes.
Resolver/domain sections must be supplied by their separate owners. Main commit publishes
the original scan user objects rather than parsed disk normalization, preserving
null component sets and runtime-only state. Suspension/archive serialization
emits resolved owner rows under explicit cross-user policy, nullable params,
quarantine, dialog and typed PersistableBundle extras. Suspension children precede
component lists, matching the pinned writer; nullable params retain the owner row
without a fabricated params body. Archive state emits
installer/time/activity/component and guest absolute icon paths.
PersistableBundle.save covers primitive/null/string/array/nested bundle XML;
null string-array items reject like the original serializer. Relative icon paths
still require guest absolute-path ownership. Full policy/nullable/complex state
conformance remains #979; production scan/user readiness remains #978. The
pinned original Settings DEX contains no suspending-user read or write: its
31-unit readSuspensionParamsLPr forwards the current user to UserPackage.of,
and writePackageRestrictions writes only suspending-package. An explicit image
inspection checks the reader instructions/argument registers and both writer
overloads. The native pinned policy is therefore false, rather than a fallback
for missing Flags classes or aconfig entries. System.commit_initial_package_restrictions
uses that policy with retained bootstrap checks; replacement after a successful
commit reports committed=true. Production first-boot invocation remains #978. The
explicit pinned DEX inspection passes (0.01s); 540 service units pass (6 excluded,
0.55s), host build passes (3 rebuilt/10 fresh, 17.3s). These checks do not run
production native first boot or the CTS/app gates.
The original read fixture supplies captured SigningScan user states and uses
this generation API rather than supplying a completed pkg XML document. Three
controlled cases verify scalar/component state and actual original suspension/
quarantine, dialog title, nested bundle/array/launcher extras, archive installer/
time/title/component/icon getters. The reader uses the actual domain service,
existing controlled connection bound to the actual setting name and real
framework Context. Existing BaseBundle.get checks exact extras types and values.
The fixture commits scan-generated packages.xml before user restrictions, and
keeps that native file for original reading instead of replacing it with a sample.
Native initial-write/reopen and original getters agree. The original ART oracle
passes without skips (64.19s); 541 units pass (6 excluded, 1.86s), including bundle
save/restore types, null-string-array write rejection, absent user-owner rejection,
live component-state preservation, main-write rollback/retry, external byte
mutation and committed reserve failure, committed scan normalization and rejection
of uncommitted metadata. Host build passes (3 rebuilt/10 fresh, 16.0s).
The first ART attempt hit ENOSPC (#903); removing this worktree's regenerable
release incremental cache allowed the complete latest fixture to pass. The
retained-bootstrap/framework-res scan previously passed (1.17s) and was not
rerun for this writer check. This verifies the writer/read boundary; complete native
first-boot scan generation and app/reboot acceptance remain unverified.
Actual original absent/no-start probes with malformed user files return false,
preserve both files and run finally VersionInfo initialization. Native units cover
that skip, write refusal and the contrasting failed-read retry continuation.
System.recover_owned_package_settings now connects recovery to the native
Settings record dispatcher. Package/shared UID registration and keyset records
share one AppIds owner and one PackageReadAttempt. Every file read resets
attempt tables; File retry and absent input clear transient tables while keeping
registered settings and UID effects. Owner/fatal errors retain current attempt
inputs and preserve the selected file. Required package legacy permission/domain
and shared permission children reject missing handlers. Boot modern domain
containers now decode through the native persistence reader
and merge into pending/restored maps before scan attachment. The UUID mode comes
from the retained original bootstrap bridge before file selection. Prior legacy
and modern maps survive retries; an invalid UUID prevents publication of its
whole detached container. Keyset read finalization now runs directly in the native keyset owner using
per-attempt role counts, including pending and replaced aliases. It reports
missing saved keyset references, applies known counts, rejects missing public-key
mappings as uncaught input after earlier count changes, and prunes orphaned sets.
The external finalization callback and its RefCell adapter are removed from the
owned frontend. Public-key factory decoding, full runtime public-key reference
counts/repeated-read handle lifetime and other global records remain incomplete
(#824/#916/#914). Pending
binding and related user restoration are not yet a complete boot lifecycle.
The boot frontend agrees with the actual-original modern domain persistence
(333), shared UID (238) and keyset (162) projections; the full original ART
oracle passes without skips (69.53s). Retained-bridge units exercise corrupt-file retry with registered UID
effects retained, missing child/global owners, fatal public-key input, absent files
and stale bridges.
Full original-name native PMS construction, remaining frontend/group/user/
permission/global owners and SystemServer orchestration remain #914/#912/#798.
17 XML tests passed on the preceding revision. The current full original ART
oracle passes without skips (73.32s), preserving eight boot version cases and
earlier domain/verifier/package/shared/keyset/permission checks. Preceding original
API/image/template build passed (9 rebuilt/33 fresh, 85.7s), including the new
compile-only original Settings install-permission declaration. Final host build
is fresh (0 rebuilt/13 fresh, 0.2s); 538 service units pass (5 excluded, 0.54s).
The retained-owner framework-res scan passes (1.27s), including owned recovery,
domain and fatal keyset mapping checks. Compile-only persistence/state-map API declarations
were checked against the pinned image after making the unused constructor private,
as supported by the existing image-link verifier for shrunk static-only classes.
Source: pinned DomainVerificationPersistence.java, DomainVerificationSettings.java,
DomainVerificationLegacySettings.java and SettingsXml.java.
 An original
PackageSetting.getPathString compile-only API declaration was added after the
oracle's compilation failed on the missing declaration; the image API verifier
checks it against the pinned original. Only worktree incremental cache was removed
to recover test space (#903). This is a registered-package owner comparison:
domain-ID policy delivery, shared UID membership/aggregate flags and retained
object/user/legacy binding callbacks, global/keyset owners and legacy
permission/domain callbacks, complete incremental frontend and default boot
integration remain #914/#798. Original PMS still runs. Native Signatures now stores current capability flags (an empty vector represents
all zero); the feed retains them instead of discarding them, and the AST reader
inherits flags from its certificate table. Verified APK/shared-owner constructors
use the original zero current flags. SignatureReader retains generated Java
public-key serialization in its target; all 2,544 original comparisons now check
stored flags and public-key serialization class/byte hashes as well. A feed
regression checks nonzero flags, and two additional original Settings text/ABX
cases check a following package's current signer inheriting an earlier table
entry's past flags. Native public-key serialization errors reject explicitly;
they do not masquerade as an invalid certificate/SigningDetails.UNKNOWN. Native sign::SigningDetails validates and retains saved current flags; lineage
merges retain the descendant signer flags, and shared/APK owners preserve them.
The native snapshot stream now carries DER and current flags together for package,
shared UID, retained and install-initiator signing records. The vendor Java bridge
constructs original Signature objects with these flags. Actual original ART
roundtrip and getter-isolation checks pass with package flags 7, shared flags 13
and initiator flags 11. Original lineage comparisons pass 507 merge combinations
and 2,197 shared UID combinations with nonzero current flags (27.82s).
Original XML writers do not explicitly persist current flags: write preflight
compares their persisted projection, while a separate original PackageSignatures
reader compares actual restored flags and serialized public-key class/byte hashes
for all 243 packages and 16 shared UIDs.
Inputs::load/load_verified_code and the default Image/DataImage loaders still
verify code afresh. Apks::collect_signing_details now implements the pinned
certificate collection choice: matching saved code path/time, no forceCollect,
DB version at least SIGNATURE_MALFORMED_RECOVER, and nonempty saved signers with
nonzero scheme reuse the saved signing; misses collect from APKs. Pre-N-MR1
upgrades compare the code directory timestamp; ordinary clusters compare the
latest base/split APK timestamp. Its timestamp reads follow File.lastModified's
zero on stat failure, while missing native path mappings reject explicitly.
Actual original ScanPackageUtils.collectCertificatesLI agrees on 78 cases with
signed code on disposable data: monolithic/cluster/split/missing paths, cached
current flags 37, DB boundaries/future version, forceCollect, skipVerify, absent/
empty/unknown saved signing, path/time mismatch and legacy timestamps, plus
missing base/split APKs and a directory base with target SDKs 28/30/36 and both
verification modes. Mapped signing-source open failures now return package
INSTALL_PARSE_FAILED_NO_CERTIFICATES (-103); absent native mappings remain fatal
owner errors. A disposable data scan fixture confirms rejection cleans the outer
scan path and recovers its retained system factory (7.40s). The fixture now
parses the real data inventory, removes its owned base APK symlink, then submits
the uncollected inventory to the sequential owner. Complete
signatures, lineage flags and public-key serialization class/byte hashes agree
in the AppIds/signing oracle (27.94s). Original PrepareFailure constructor checks
also confirm verifier IO maps to INSTALL_FAILED_INTERNAL_ERROR (-110), while an
existing PackageManagerException preserves its package error. Explicit Image/DataImage::load_collected
entry points now consume the collection choice before reconciliation; their
scan partition fixes the permitted skipVerify input. Immutable original system
and data scan fixtures retain cached flags 41 and preserve paths/rejections
(0.57s and 0.39s); mismatched skipVerify/partition inputs reject. DataImage::parse
returns Code<()> inputs without certificate verification. The original boot
bridge's data phase uses these inputs and collects signing only after selecting
the current setting and volume owner. Only Code<SigningDetails> exposes collected
package conversion. Parsed and verified inventories agree on paths, locations,
rejections and complete parsed packages in the immutable-image fixture.
The sequential initial-system and data scan now resolve the current selected/
original-rename setting, volume VersionInfo, upgrade state and strict data
allowlist before collecting signing. Scan settings persist those volume owners
in original ArrayMap order, replacing legacy version tags, before the keyset
commit. The first-system scan fixture passes (9.93s), including settings commit
and reopening the persisted owner. Updated-system KeepData selection now fully
verifies strict-allowlisted factory APKs instead of copying input signing, after
committing the disabled metadata refresh. That refresh accepts uncollected
Code<()> and finalizes the factory package with SigningDetails.UNKNOWN, regardless
of input signing or a later strict setting recollection. Native facade and
snapshot validation preserve the singleton parcel tag separately from an empty
known signer array with scheme zero. The actual original PackageImpl cache oracle
checks singleton identity and preserves every decoded field for both variants
(24.01s). Its inventory fixture now supplies detached volume owners alongside
package/shared-UID owners (#922); version emission follows the pinned writeLPr
block, while full boot writer execution remains #914. Source-conformance checks retain
non-strict saved signing, discard input current flags 37 for strict signing, and
compare the complete owner after a strict verification failure with the completed
metadata-only scan. That failure preserves earlier metadata and reports the
original preparation error -110. Updated-system factory restoration now collects
after resource cleanup and setting enable, using the pre-enable selected active
setting for cache decisions and explicit upgrade/pre-N-MR1 policy. Input flags
37 are replaced by fresh factory signing; a mapped invalid signing source fails
with -103 while earlier cleanup/enable effects remain. Missing-update factory
recovery reparses its physical code after enable, then collects using its current
selected setting. Guest parse/certificate/validation/authorization failures and
ABI selection failures are returned in DataImagePackages.factory_rejected while
later factories continue, as checkExistingBetterPackages does (#921). Native
mapping/serialization/owner failures stop the scan. A source-conformance fixture
uses real GSF and framework-res APKs: GSF parse or signing failure is reported,
then the platform factory recovers; missing parse/signing mappings stop before
that recovery, retaining earlier enable effects and preserving system code.
Image::parse now returns uncollected Code<()> system inputs as well. The boot
bridge supplies these to first_boot_parsed and scan_saved_parsed_system_image;
platform signing is established by the actual framework scan rather than an
inventory verification pass. The shared sequential loop begins with UNKNOWN
signing, collects normal admissions after current owner selection, and leaves
non-strict KeepData factories uncollected. A real GSF source mapped to a directory
fails the explicit fresh verifier but passes parsed inventory and non-strict
KeepData metadata refresh; strict KeepData fails with -110 after committing the
UNKNOWN factory package. Parsed and verified system inventories preserve all
locations, rejections and parsed fields (0.71s). Explicit fresh-verification
loaders remain diagnostic APIs. Initial request-stage shared UID preparation now precedes collection (#923).
It selects the installed setting for leaving-shared-UID policy, allocates with
zero flags/private flags, and records the setting without active membership or
signatures. An allocation failure reports the original -4 error without creating
a partial group. Normal system certificate failures are reported and later
candidates continue; missing native inputs stop scanning. A controlled request
DTO backed by unchanged GSF code verifies failed collection retains an unused
UID 10000 and later GSF gets 10001. Fatal mapping failures preserve that prepared
owner while stopping before GSF admission. Inconsistent disabled settings are
also removed before collection, retaining removal on both guest and native
failures (#924). Actual original Settings.getSharedUserLPw confirms zero defaults,
lookup instance/flag stability and -4 exhaustion across 10,000 allocated slots;
the AppIds/signing oracle passes (28.69s), including the 78 collection cases and
259 saved signature owners. Its Settings test uses explicit process exit because
the constructor starts BackgroundThread (#925). Broad source/admission failure
continuation and full request/frontend ownership remain incomplete (#914/#918).
Initial system source selection now resolves the original independently of an
incoming setting, and uses the original name for disabled-factory lookup (#919).
The normal unchanged-shared-UID admission still copies the incoming setting,
as scanPackageNew does: a controlled request over unchanged GSF code verifies
incoming UID 10002 and its user states survive while the original UID 10003,
setting and unloaded identity remain unchanged, with no renamed-package entry.
Disabled factory refresh also selects that original name while retaining the
incoming parsed APK name, as the copied disabled ScanRequest does. Its ABI and
page-size metadata carry an explicit setting/parsed owner binding, preserving
saved factory ABIs without permitting unrelated setting updates. Source version
and path use the original setting; the old shared UID still comes from the
incoming installed setting. Controlled unchanged-GSF requests verify both
KeepData (original factory UID 10003 and UNKNOWN code signing, no incoming
admission) and RestoreFactory (enable original UID 10003 before admitting incoming
UID 10002 and its user states). With no incoming setting, KeepData retains the
original factory without active admission; RestoreFactory now creates the accepted
package from the enabled original, retaining UID 10003 and its user states,
recording the incoming name as realName and renaming the parsed package to the
original internal name. Both restored and ordinary original-name creation derive
ABI afresh from the null installed-setting request instead of reusing the
original's saved ABI. Successful creation records renamed and transferred
packages; a final metadata rejection preserves the original setting without
publishing either record. The controlled first-system scan covers all four
incoming-present/absent and KeepData/RestoreFactory combinations and ordinary
creation (12.17s), and release units pass 463 cases. Disabled loaded code now retains a
private binding of the selected setting name, UID, path and version to its parsed
name (#926). Snapshot validation and code capture require that binding for
unequal names and reject foreign identities or active-scope use; copied captures
retain their original binding. The private code envelope carries both names and
the factory scope and bound setting UID, and the Java lease/object restorer checks these without
renaming PackageImpl. A complete native Store/capture/publish test accepts the
selected pair and rejects a changed parsed name. The original Android parcel
oracle passes (25.25s), comparing original PackageSetting.setPkg with restoration
of known and UNKNOWN code under a different setting name, plus scope/version/name
rejections (including a foreign factory UID) and exact frame roundtrips.
An original ART constructor oracle now exercises Settings.createNewSetting with
all four independent/shared original and null/shared target-group combinations.
It verifies that the original branch retains the source UID/shared owner even
when a different target group is passed, preserves existing user states without
applying new-install instant/preload flags, resets its signature wrapper to
UNKNOWN and replaces its ABI/path/version metadata without changing the source
metadata. Existing per-user state objects remain aliased to the original;
mutations propagate to that instance. The full setting/helper comparison passes
(68.15s). The focused original constructor/registration oracle also verifies
sealed snapshot isolation, independence of new users and final UID-slot owners,
plus three old shared-group removal cases and six shared-to-independent/shared
original recreation/registration/removal cases, plus six original readLPw recovery
cases (9.34s). Ordinary scan admission shares the APEX prior-instance owner during
candidate construction (#927), then matches Settings.addPackageSettingLPw:
registered ordinary APK slots point to the final accepted setting, discarding the
prior independent slot instance; INVALID_UID/APEX admission leaves the original
slot intact; shared UIDs keep both setting instances in the group. The actual
original registration oracle checks the active Settings map and UID slot for all
four source/target combinations, including two same-name shared members. Existing
users alias retained shared instances, new users do not, and older native captures
stay unchanged. The first-system fixture verifies both UID forms through Store
capture/publication, rejects a foreign UID owner without replacing its capture,
and keeps the original slot untouched when metadata fails. Retained slot removal
preserves prior captures. The actual original
UID/signing oracle passes (28.87s); release units pass 463 cases. When an incoming
independent setting declares the original setting's shared UID, initial system
scan now recreates/adopts the original setting instead of rejecting the changed
relationship (#919). UID 10003, old users and the prior shared member stay with
the original; the incoming UID 10002 setting stays unchanged and unloaded. The
accepted copy preserves pendingRestore from the incoming request. A validated
Original ABI binding reads the incoming saved ABI, which ScanPackageUtils caches
before setting recreation; upgrade/stub requests derive afresh. Wrong parsed,
UID, non-system and updated scopes reject the binding. Native fixtures verify
successful recreation, Store capture/publication and final metadata failure
without a retained member or renamed/transfer record. These are controlled native
requests, not complete original PMS request-execution comparisons. Ordinary
original recreation also accepts an incoming shared setting when the requested
shared owner changes (#919/#929). After all metadata gates succeed, it removes
the incoming instance from the old group while retaining that exact unparsed
setting in the Settings map. A private binding pins its metadata, old group,
accepted original code and rename/transfer record; capture validates that binding
instead of inventing membership or a UID slot. Shared-group records omit the
displaced incoming setting. Changed incoming metadata and foreign UID owners
reject publication, and previous captures remain unchanged. Native first-system
fixtures cover shared-to-independent/shared recreation with last-member pruning,
another active member or a disabled-system reference, and metadata failure
without membership removal. The original ART oracle executes the corresponding
six constructor/prune/final-registration sequences and checks Settings-map and
UID-slot instance identities. The old-group pruning primitive counts actual
active/retained members and disabled references (#928). On reread the original
resolves saved shared references: an absent old group drops the incoming setting,
while a surviving group registers it again. The native settings reader already
drops unresolved shared references. The private signing frame now preserves the
saved shared UID separately from a possibly absent group and carries the accepted
original name for displaced active settings. The native captured binding supplies
that name; unbound settings still require their real UID/group owner. Java rejects
factory scope, a nonpositive shared UID or an original name equal to the incoming
name, and signing restoration requires that the displaced setting has no loaded
code. Snapshot assembly checks the named original's loaded code and real name,
and rejects continued old-group membership. Native first-system fixtures verify
the emitted signing headers for all six successful shared-origin transitions;
original ART validates two group-present/absent frames, exact reserialization,
UNKNOWN signing restoration, rejection after loading code and three malformed
markers. Shared process ownership validates the displacement binding and excludes
the incoming Settings-map entry from actual group members. All six successful
shared-origin fixtures complete process assignments before capture, persistence
and removal; the complete first-system scan, including Java assembly and six
original-service boots from native-written settings, passes (88.02s). The full package
Parcel oracle passes (27.20s); release units pass 466 cases (0.47s), with three
ignored/not run. Native scan persistence now writes the displaced incoming setting
and its keyset references even if its shared group is absent; read-roundtrip
validation accounts for the original pending-package drop after proving the
private displaced binding. The scan keyset writer compares the restored inventory
while preserving every written package's keyset nodes. Standalone keyset commits
retain their existing full-inventory check. Disposable native write/reopen/UID
restore fixtures verify all six shared-origin cases, and the original readLPw
oracle verifies the corresponding absent/active/disabled group cases. A surviving
group gains the saved incoming member on reread; an absent group drops it. Native
setting removal clears the accepted displacement binding, keeps other/disabled
UID owners, returns the original removal result when the old slot is already
absent, and rejects a foreign UID owner without changing state. Original ART
checks deletion and repeated deletion after each of its six recreation cases;
previous native captures retain their incoming signing records. Complete native
replica-input fixtures now capture every active/factory setting's users,
legacy/fixed permission inputs and saved runtime before recreation, resolve
loaded library/runtime owners after scanning, and emit scalar/runtime/user
records for all six shared-origin transitions (#930–#932). Original-setting
retention checks the exact unparsed source and its runtime identity even when
an earlier scan has made unrelated whole-scan assignments stale; final captures
still require the full runtime inventory to match. Actual shared-group pruning
removes its captured legacy owner, and active setting removal forgets its own
legacy/fixed/runtime inputs while preserving factory scopes and previous
captures. Setting removal stages all changes so a missing group input leaves
the earlier owner unchanged. APEX replacement uses the same group cleanup once.
The complete first-system scan now passes the six native accepted captures through
generated Binder Proxy/Stub and original ART's PackageScanLease.captureData
as part of the complete first-system test (88.02s). Pruned/active/disabled old
groups and independent/shared original targets
preserve the incoming unparsed setting outside group membership, the loaded
original's real-name/UID binding, distinct current/retained original instances,
and factory inventory. Captured replicas remain usable after lease close. Each
case rejects an advertised inventory missing the accepted original. The full
original ART Parcel oracle, including shared APEX adoption/replacement, passes
(27.20s). Original Settings.readLPw now reads the actual native-written ABX for
all six cases: missing groups drop incoming settings, surviving groups rejoin
them, and original UID/factory inventories match. Each actual file is also
injected byte-for-byte into the stopped disposable data image, with old resilient
settings copies removed, before a complete original-service boot. All six boots
reach sys.boot_completed, expose framework and GSF paths through cmd package,
and persist both core package entries. Owned guest/keeper PIDs and mounts are
checked after every stop. The mount point now sits under the temporary Data
guard so sibling image/runtime artifacts are removed too (#933). This completes
#929's owner/recovery acceptance. Reopening the native data and replaying each
saved rename now also records the accepted internal package in the transfer
owner (#934), matching InstallPackageHelper's realPkgName commit branch. All six
independent/shared and pruned/active/disabled cases produce exactly that one
transfer; an earlier metadata failure produces neither a transfer nor loaded
original code. Transfer recording occurs only after metadata completion. The
expanded first-system test passes (88.02s), and release units pass 466 cases
(0.47s), with three ignored/not run. This does not compare the complete later boot
scan request stream. Full original request parity and transfer publication remain
#919,
and daemon frontend publication remains #798/#836. A complete-replica Store
constructor now requires captured legacy/fixed permissions, sparse users, scoped
runtime and shared process owners before the first version is visible (#935).
Every later publication enforces the same contract, including loaded seInfo and
library dependencies and scalar/signing/code/user/shared record projections.
Incomplete updates preserve the prior version; partial diagnostic Stores retain
their existing behavior. All six accepted displaced graphs use this complete
constructor before original ART assembles them. This is a publication prerequisite;
the live daemon scan producer and SystemServer replica consumer remain unwired.
System now retains the complete replica Store with the exact original bootstrap
bridge that supplied its policy owners (#936). Publication checks that bridge
and the captured base under one lock; incomplete/stale/foreign candidates leave
the prior state intact. Bridge replacement clears the current Store, and death
clears it only for that exact attachment. Generated IServiceHost.capturePackageScan
exports a separate immutable IPackageScanSnapshot lease to the system UID;
missing publication, wrong tokens and trailing arguments reject. Issued leases
retain their captured version across newer publication and bridge replacement.
The real native Binder test verifies missing/incomplete publication, old/new
versions, caller/token/tail rejection, close, replacement and old/current bridge
death. The publication sequence now belongs to the daemon rather than the
bridge attachment (#938): replacement and current bridge death retain the last
successful version, and the next complete graph uses its successor. Rejected
publication does not advance it. Zero and values above signed-long MAX reject;
publishing at MAX fails without replacing the previous snapshot. The real Binder
fixture verifies replacement, death/reconnection and old lease versions. Release
units pass 466 cases (0.46s), with three ignored/not run; the explicit pinned-image
bootstrap/publication test passes (0.68s). The production host build passes
(15.3s; three rebuilt, 39 fresh). The
full image/original-PMS template build passes (65.1s; nine rebuilt, 33 fresh). This
connects complete native publication to the service-host transport; the running
boot flow does not yet invoke publication, and the default SystemServer bootstrap
does not yet invoke the replica consumer. PackageBootstrapBridge.captureSnapshots
now binds a Java PackageSnapshots.Store to IServiceHost.capturePackageScan and
requires a complete initial refresh before returning it (#939). The Store
publishes only after complete assembly and successful lease close, reuses
same-version replicas, rejects backwards versions and preserves earlier scopes.
Missing endpoints and capture/assembly/close failures keep the prior Data graph.
PackageScanLease also closes an endpoint when its initial version read fails or
returns a nonpositive version, preserving close failures as suppressed errors.
For all six accepted displaced native graphs, the fixture exports authentic
native versions 1 and 2. Original ART's generated Proxy/Stub page sources verify
initial/same/newer refresh, failed newer assembly, close failure/retry, stale and
invalid versions, initial version/close failure, missing endpoints and retained
old-scope identity as part of the first-system test (88.02s). The full original
ART Parcel oracle passes (27.20s), and the image/original-PMS template build
passes (57.6s; six rebuilt, 36 fresh). This remains a
controlled Java page source over actual native records; the live daemon producer,
facade registration and native visibility callbacks remain #798/#836. PackageLocal
now implements the pinned seven-method PackageManagerLocal interface over the
published replica Store (#944), including explicit and Binder-derived filtered
callers and an uncommitted ART scope. Construction rejects an uninitialized
replica or missing SDK-data owner. SDK reconciliation forwards every argument and
its IOException unchanged to the required owner. PackageBootstrapBridge now binds
that owner to generated IServiceHost.reconcilePackageSdkData (#945). The native
System serializes the exact eight-field stable Parcelable with its package
install lock, shared by code/storage cleanup, and calls original installd through
generated transaction codes. Only system UID callers are accepted; malformed
requests and missing owners fail. Installer exceptions retain their original
code/message and Java's original IOException message conversion. The replacement
vold now calls original vold_prepare_subdirs after preparing each user's storage
and propagates helper failure (#948); the missing call had left SDK CE/DE roots
absent. On a disposable original-PMS boot, original ART verifies native SDK
creation, CE/DE selection, same-UID pruning, SDK/cache directories, exact request
bytes, invalid-user exception type/message and actual UID 19001 denial. Pinned
installd's invalid-user guard returns its Binder worker's current errno, so
independent direct/native requests can have different numeric codes (#955).
Controlled generated/native Binder tests preserve each returned code (0, 2, 22)
and message exactly. A direct
original IInstalld control runs the same operations and agrees on final UID/GID.
Both paths reject SDK UID migration after partial ownership changes: pinned
installd's chown_app_dir changes the SDK root group to its UID, then its strict
prepare rejects the group instead of AID_NOBODY (#949). This original failure is
preserved, not converted to success. The full original ART Parcel oracle passes
(26.88s), also checking known-empty signing with both null and empty public-key
sets (#954), actual PlatformCompat query policy and absent permission-owner
failure. Its diagnostics include Java stdout (#947). Release units pass 476 cases
(0.45s), with four ignored/not run, including shared install-lock blocking of SDK
execution and owned code cleanup. The image/original-PMS template build passes
(57.1s; eight rebuilt, 34 fresh). Explicit service-host tests pass all six cases
(2.93s), including framework-APK completion and exact SDK failure forwarding. The three signing test APIs preserve
original Build.isDebuggable enforcement and require an explicit signing owner.
The production PackageLocal factory routes add/remove/clear through generated
IServiceHost methods to the native System's service-lifetime Overrides owner
(#946/#950). The pinned early bootstrap bridge supplies Build.isDebuggable;
policy changes reject replacement before changing the current bridge. Same-policy
replacement retains the owner, version and nonempty override table across bridge
death/reconnection. Default Apks collection consumes its bound owner; System's
three boot scan entries reject missing/foreign owners before scan effects.
Native Apks certificate collection accepts this versioned signing override owner.
The owner preserves prior immutable versions, add/replace/remove/clear and
release-build denial; version exhaustion fails before mutation. For native
verified SigningDetails, current signer flags are excluded from key equality,
current signatures/public keys compare as sets, and ordered past certificates
include capabilities. Lookup also retains original ArrayMap's hash behavior:
SigningDetails hashes current signatures in array order even when equals ignores
that order. Overrides apply once after each APK verifies and before base/split
signer comparison; verification failures cannot be overridden, and saved signing
cache reuse bypasses the map as original collectCertificatesLI does. The explicit
app-ID/original-owner fixture verifies 169 original
SigningDetails.equals/ArrayMap lookup pairs, the original release verifier's
unchanged results after add/remove/clear, cache/force/unsafe-collection choices,
and missing APK failure on disposable code. A separately configured native debug
owner verifies real APK add/remove/clear and override-before-split comparison;
the pinned image's Build.isDebuggable is false, so original debug-build
application remains unrun. Native SigningDetails now preserves null, empty and
populated public-key sets plus null entries (#950), with original signed-hash
ArraySet ordering and duplicate elimination. Settings/PackageImpl/SigningDetails
Parcel projections retain those states rather than deriving replacement keys
from certificates. Original constructors verify seven owners (including UNKNOWN,
an empty owner with null keys, and a mixed key/null set), 49 equality/ArrayMap
lookup pairs and exact Parcel bytes. Native PackageImpl cache roundtrips and
actual APK verification followed by each of the seven override replacements
also pass in the expanded app-ID/original-owner fixture. The original
public-key serialization/deserialization fixture passes (10.64s). Null keys
remain invalid at keyset registration instead of being silently removed; the
signature comparison tool reports absent/null keys as mismatches. All aim-services
test targets compile; compilation does not count those tests as executed.
Full signing owners now
use versioned Java encoding that retains current and past capability flags,
UNKNOWN, nullable key sets/elements and nullable past arrays. Native decoding
rejects malformed versions, lengths and trailing bytes; release policy and
foreign-UID failures precede mutation. Original ART loads the production encoder
from aim-services.jar and matches all seven full owners against native decoding,
including original ArraySet hash ordering. The production Java signing factory's
add/remove/clear calls reject actual system UID on release policy and actual app
UID through the native Binder endpoint. The complete app-ID/original-owner
fixture passes (160.78s, debug Rust build). A controlled debug bootstrap plus
native Binder mutations verifies each of seven nullable owners through default
collection of a real original APK, then remove/clear restoration; the explicit
service-host fixture passes five cases (35.14s). Normal units pass 475 cases
(3.31s), with four ignored/not run; the image/original-PMS template build passes
(60.7s; nine rebuilt, 33 fresh). The final production host build passes
(10.7s; three rebuilt, 39 fresh). UNKNOWN override collection retains the native
PackageImpl UNKNOWN representation rather than requiring a known parcel body.
The pinned original image remains a release build: original debug-build override
application remains unrun (#946). Default native package activation and its
conformance gates are still unproved. Original ART verifies all
seven methods over each of the six authentic native graphs: current/old scope
identity, explicit and Binder callers, distinct uncommitted replicas, required
inputs and SDK argument/error identity. The complete first-system fixture,
including those checks, native-written original ABX reads and six seeded
original-service boots, passes (90.92s). It requires both SDK and signing
owners; missing owners and uninitialized replicas fail explicitly. The fixture
injects the original verifier as its controlled signing owner; production uses
the native owner described above. The C query endpoints now capture the native System's query owner instead of
accepting an independent production model state (#951). Query projection binds
package identities and external permission/domain/compatibility inputs to one
scan version. Atomic scan/query publication validates both before replacing
either; staged scan publication invalidates the old query owner until a matching
context is supplied. Controlled native Binder and real framework-APK tests pass,
including failed-publication preservation and older captures. Native query UID
lookup now uses the captured AppIds slots, preserving detached settings and
same-name retained shared members separately from current packages (#952).
Declared shared IDs survive pruning of their live group. Original ART matches
core package getters and per-user defaults/overlays/signing on all six actual
native displaced graphs; the complete first-system fixture passes (91.38s).
Release units pass (476; four ignored), including missing UID slots and retained
member installation filtering; explicit service-host tests pass (six, 3.02s).
Completion validates complete active/factory/retained package and user identities
before resolving permission definitions, per-user grants, GIDs and
FILTER_APPLICATION_QUERY from the retained bootstrap bridge (#953, #956).
Owner callbacks run outside the publication lock; missing owners,
malformed/trailing replies and changed attachment reject without replacing
either capture. Uninstalled users receive no grants and do not invoke the grant
owner. The original permission API resolves a current package by name, so the
production bridge validates its appId and retains the exact PackageManagerLocal
state across the grant callback; a changed state or UID rejects publication.
Controlled Binder tests verify arguments, nullable/duplicate replies and atomic
failure preservation. Original ART invokes the production Java bridge at system
UID over six real native snapshot graphs, checks sorted permission replies,
rejects mismatched and changed owners and nullable replies, and rejects actual
app UID. Its permission service is a controlled fixture: live AccessChecking
and SystemServer permission/GID integration, exact retained old-UID grant
ownership, full original UID-query/visibility parity and production domain/global
context publication remain unproved. Native domain collection now implements
modern and legacy filter selection, linked-app policy, wildcard validity, Java
ArraySet order and the 1 MiB UTF-16 estimate (#957). Active query publication
checks each supplied user selection against the current native APK's complete
web-host set, rejects duplicate hosts and invalid public selection states, and
requires current code when selection is present. Factory and retained packages
still require domain-owner publication. Release units pass 482 cases (four
ignored/not run), including modern/legacy policy and byte-limit boundaries;
explicit service-host tests pass six cases (2.94s), including real framework-APK
selection publication and failure preservation. The original-ART parcel fixture
now compares 84 collector results over seven native parsed-package inputs:
modern/legacy and linked-app policies, wildcard/Unicode/invalid hosts, empty
filters, the real 1 MiB bound and duplicate authority accounting, including
exact ArraySet order. Native SystemConfig now reads `<app-link>` declarations
with ALLOW_APP_CONFIGS: system/system_ext/product are accepted, vendor/ODM
(including SKUs) only at initial API 27 or earlier, and OEM/APEX are rejected.
Missing package attributes are skipped; empty/space strings are retained and
duplicates eliminated in signed Java ArraySet order. Original SystemConfig
executes the same XML inputs with pinned partition flags for initial API
27/28/36 and matches the native lists exactly. This supplies the native linked-app
configuration owner. Native attached domain state now owns pending-first
attachment, restore signature matching, legacy approvals, pre-verification,
immutable linked-app approval and update state migration (#957). The owner
returns verifier requests separately from broadcast delivery and explicitly
reports the original missing-owner recovery path. Its logical persistence
projection keeps pending/restored and legacy user state distinct. The
query-context resolver now replaces current-package verification, user
selection and URI-group inputs from this native owner, requiring the exact
Settings domain UUID and the complete current compatibility-policy inventory.
Factory/retained query domains remain separate. The original-ART fixture compares
all eight public domain-info maps and sixteen user selections directly with
DomainVerificationService, including verified-over-selected precedence, selected
failed domains, disabled link handling and absent-user defaults. The native boot
scan publisher fixture now derives domains through this resolver instead of
supplying host states, rejects UUID mismatch/missing owner/missing policies, and
preserves previous scan/query captures on later publication failures (one explicit
ignored test executed, 0.71s). This fixture now constructs the attached owner
through the bootstrap builder from the actual native scan: current code,
Settings UUIDs and verified signer arrays. Missing/UNKNOWN signing, mismatched
code/signing owners and incomplete policies reject before attachment; candidate
failures leave the scan unchanged. The builder retains pending/restored state
and returns verifier requests separately from delivery. Its policy inventory now comes from the retained
bootstrap bridge's original PlatformCompat owner. The bridge supplies the same
mock ApplicationInfo name/target-SDK fields as DomainVerificationUtils and reads
RESTRICT_DOMAINS (175408749), preserving original configuration decisions. The
original-ART fixture at system UID compares this producer with original
isChangeEnabledInternalNoLogging at SDK 28/30/31/36; null/empty names, negative
SDK and an actual foreign UID are rejected. Native tests reject empty/trailing
replies and invalid identities. Runtime override changes/invalidation have not
been exercised. Native Settings now imports old per-package
`<domain-verification>` global statuses separately from legacy per-user state,
using the Settings package name rather than the child's packageName. Original
Settings.readSettingsLPw followed by DomainVerificationService.addPackage
matches eight native imports: foreign/missing child names, last duplicate wins,
missing/malformed/negative status and verifier-defined status. The boot builder
uses these actual Settings inputs, not an assumed empty legacy map. Daemon
publisher now retains the immutable boot owner, configuration, policies and
verifier request plans in the same query capture as its scan. A composite daemon
entry completes native runtime, creates domains and resolves query owners before
one atomic publication. Capture validation recomputes domain projections and
rejects input changes after binding. The native publisher fixture confirms
unchanged scan/query/domain capture pointers after a rejected publication,
bootstrap replacement clearing the current domain owner and retained old-domain
capture isolation. first_system_scan and package_parcel compile; compilation is
not execution. Automatic startup through this composite entry and native domain
Binder registration remain unwired. The attached owner now implements
clearPackage, clearPackageForUser and clearUser: current, pending and restored
state are removed at the original scope, the UUID index follows package removal,
and separate legacy migration state is retained. The original service executes
sixteen cleanup-state comparisons across the four attachment scenarios, with
pending/restored-only packages and users 10/11. It schedules one settings write
for every clear request, including missing entries, while queries schedule none.
The fixture also executes fresh legacy-user approval migration (status ALWAYS),
inherited/immutable paths that skip it, and legacy state surviving user removal.
Logical package maps are compared independently of the persistence writer's
ArraySet value order. The owner now ports internal link-handling changes for
one package, all attached packages and USER_ALL using the supplied actual user
inventory. It preserves selected hosts, creates absent user rows and leaves
pending/restored state unchanged. Twelve original-service state comparisons
cover these changes across the attachment scenarios; missing package mutation
rejects without a state change or write request. Each scenario records eleven
successful link/cleanup persistence requests and none from queries. This is the
state owner only: caller authorization, durable writes and nonce invalidation
must be connected before serving this mutation through native Binder. The
verifier update owner now matches public-state validation (SUCCESS or verifier
errors), invalid UUID/domain result codes and original requested-set filtering.
It preserves immutable approvals/denials/system approvals, updates modifiable
states and removes newly verified hosts from attached user selections, except
users where the verifying package has link handling disabled. Eight original
SUCCESS/error state comparisons include an attached competing app; invalid UUID,
unknown/empty domains and invalid state do not mutate the owner or request writes.
Each scenario now schedules thirteen successful verifier/link/cleanup writes. Native URI-group ownership now implements partial Bundle-key
updates, exact requested-key lookup and null/empty-list deletion. Validity uses
the server's separate ASCII/max-length/label rules, not the collector's Unicode
Patterns matcher; valid undeclared domains remain permitted as in the original.
The original service compares eight update states and four group lookup replies,
covering wildcard/numeric/hyphen labels, undeclared hosts, Unicode/underscore and
long-label rejection. Empty updates return before package lookup, nonempty
missing owners fail, and the original setter schedules no settings write.
Private intent wildcard fallback and native Binder permission/transport wiring
remain separate unproved paths. Domain persistence defaults now match original
SettingsXml: missing/malformed legacy user IDs/statuses and URI-group/filter
numbers use -1, domain states explicitly use NO_RESPONSE (0), and boolean
fields use false. URI-group action comes from the current group cursor, not
its parent domain. Five original-service read/write cases compare omitted,
malformed, -1/0/1 attributes with differing parent/group actions (#960 fixed).
The writer's omission of a -1 domain state and the subsequent NO_RESPONSE
read default are included in the comparison. The native domain writer now writes
active/restored verification, user selection, backup signatures, legacy user
state and URI groups through Store.commit_domains and the existing resilient
packages.xml backup/reserve protocol. It preserves unrelated settings and
future XML on surviving records, including unknown container data after the
last owned child is removed. Invalid/duplicate identities and external writer
conflicts reject before commit. Native tests verify ABX main/reserve equality,
restart state, precommit rollback and committed-state publication after reserve
failure. Five native-written ABX cases are read and rewritten by the original
DomainVerificationService and match the resulting logical state. The daemon now prepares a validated domain replacement before disk IO and
commits it under the scan/query generation lock. Exact bootstrap/capture base,
package identity/signers and persisted domain base must match. A precommit
failure preserves current captures; main-committed reserve errors publish the
replacement before returning the error. Native publisher tests exercise a real
Store-backed link change, query state and facade lease generation advancing,
rejected stale candidates leaving the file intact, and outside-writer conflict
leaving the current capture intact (0.76s). Old captures remain isolated. The
reserve-error daemon branch has not been exercised directly; Store-level main
and reserve failures are tested. After a successful or main-committed domain write, the daemon now releases its
publication lock and calls the retained bridge's original
PackageManager.invalidatePackageInfoCache. Precommit/stale failures do not call
it. A malformed/failing invalidation reply reports committed=true while retaining
new disk/query state; an explicit retry invokes only invalidation. Native tests
count successful invalidation calls and exercise that failure/retry path (0.74s).
Original ART's PropertyInvalidatedCache probe retains an entry before the bridge
call and recomputes it afterward, with actual foreign-UID denial. This uses the
original mutable shared-memory owner in the disposable client process; a live
system_server shared nonce mapping was not exercised end-to-end. Notification
delivery, automatic startup and Binder mutation serving remain unproved. Native domain authorization now ports the original
Enforcer's internal/query/verifier/user-query/user-select/owner-query and legacy
checks, with explicit permission, verifier identity, user-existence and visibility
owners. Missing owners are not treated as grants or ordinary permission denials.
The captured adapter uses the same query graph's user map/AppsFilter (including
uninstalled/archived filtering) and explicit current permission and UID-specific verifier callbacks.
Original ART compares 7,680 controlled-owner cases over root/system/shell/app
UIDs, verifier status, permission combinations, user existence, cross-user access,
hidden packages and nullable query/selection targets. Allow/false/SecurityException
outcomes match, including the legacy selector's silent cross-user denial.
These are controlled owners; production verifier selection remains unproved.
A native IDomainVerificationManager endpoint now serves
queryValidVerificationPackageNames, getDomainVerificationInfo and
getDomainVerificationUserState using generated
pinned AIDL codes/reply writers. Each request captures one published native
domain/query generation. The package-list method applies approved-verifier
authorization and returns attached auto-verification packages in original
name-map order; Info applies approved-querent authorization. It rejects wrong tokens/trailing data,
unknown codes and unimplemented methods. The native publisher fixture registers
it under a test alias and covers actual Binder replies, same-process permission-owner
denial and bootstrap replacement clearing current captures (0.84s). Original ART
compares eight add/migrate package lists, including exclusion of pending/restored
records. This endpoint is not registered by guest-init under the original name;
remaining six methods and migration gates are tracked in #957. Domain approval
now has a separate retained-bridge producer for SETTINGS_API_V2 (178111421),
using the original PlatformCompat config for package name/target SDK. It is not
inferred from SDK or the collector's distinct RESTRICT_DOMAINS (175408749)
decision. Both generated transactions enforce system UID and reject missing/empty
names or negative SDK. Original ART compares each change with original
isChangeEnabledInternalNoLogging at SDK 28/30/31/36, with invalid-input and actual
foreign-UID denial. Native bridge tests prove the decisions can differ and reject
empty/trailing replies (publisher 0.85s). The native attached owner now computes positive approval levels from explicit
package-user state: absent/not installed/disabled/suspended exclude approval;
legacy ASK/ALWAYS/NEVER apply before V2 link handling; instant-app exact auto
hosts outrank verified states; verified exact/wildcard hosts outrank user
selection. Disabled link handling excludes instant/verified/selected paths.
Wildcard matching retains the original suffix comparison. Original ART's public
approval API matches 1,600 native cases across eight add/migrate states, both
V2 modes, missing users, enabled overrides, install/suspend/instant state and
ten hosts. The same-package test helper accesses original user state directly,
without reflection or changing APKs. The native query endpoint now also serves
getOwnersForDomain (2026-10-06) from the captured attached state and explicit scan user state.
Its V2 decisions come from the original compatibility bridge; capture identity
is rechecked after those calls. Positive approvals group in ascending level,
then use the shared pinned libcore TimSort with the original truncated int32
installation-time subtraction and stable case-insensitive UTF-16 name order.
The name mapping derives from pinned ICU data, with its license retained;
original ART matches all 65,536 UTF-16 unit mappings and 960 Unicode-name sort
cases (0–128 entries), including four comparator-contract rejections. Those
failures become IllegalArgumentException, as original sorting does. Original
service results match 1,600 native Owners reply byte sequences across the
add/migrate and approval cases, plus eight mixed-package/group/user/V2 queries
covering levels 1–5, case ties, hash collisions and missing settings/users.
The reply uses original DomainOwner's 0x2 overrideable flag and String16 name.
Null domains fail before authorization; empty results are non-null typed lists.
Original ART reads actual native Binder nonempty/empty Owners replies and the
null-domain exception, alongside the existing 4,000-host FD-backed Info/UserState
checks (12.50s). The publisher also checks permission denial and trailing input
rejection. The full original oracle passed (30.84s, no skips), with 488 units
passed (0.48s; five excluded by default). The original PMS remains enabled;
Method 6 now serves public link-handling changes when supplied the shared native
PackageManager Store (2026-10-06). User-selector authorization and exact attached
name lookup precede the write; null/missing names return the original name-not-found
status. It uses the same prevalidated update, inventory/generation/disk checks,
atomic persistence, capture publication and original cache invalidation as the
native daemon write path. The pending query-only factory has no writer and rejects
writes explicitly; binding the shared writer during full native PMS startup remains
open (#798). Real Binder tests cover disable/repeated-disable/enable, persisted
flags and generation/cache changes, denied callers, absent users/packages, trailing
input and outside-writer conflict with no write/publication. A malformed postcommit
cache reply returns committed=true while retaining new disk/capture state; an explicit
cache-only retry precedes further mutations. Original ART invokes disable/enable
over actual Binder and reads each published value through its original UserState
creator (13.51s). A stale publication base triggers fresh capture and authorization
before retrying; four concurrent Binder changes each publish their own generation
and invalidate the cache. Authorization-time capture changes now also retry from
fresh capture and permissions, provided the bootstrap bridge is unchanged (#963,
2026-10-06). A real Binder permission fixture publishes a competing change during
its first answer: a fresh grant commits the request, while fresh denial preserves
only the competing change. Both make exactly two permission calls and retain
matching Store/capture state. An unchanged-capture error or changed bridge still
fails explicitly. The deterministic old-code reproduction returned IllegalState
instead of retrying (0.99s); the fixed publisher passes (0.98s). Units pass (488, 0.49s; five excluded by default), as does
the publisher (0.89s). Method 9 now returns URI relative filter groups from the captured attached owner
(2026-10-06), without adding permissions or package visibility filtering. Missing/null
packages return a non-null empty Bundle; a null domain list throws only for an
attached package, with the original NullPointerException message. Requested keys
retain original ArrayMap hash/collision order and duplicate replacement. The Bundle
writer includes length-prefixed List/Parcelable values and sized stable AIDL group
and filter records. Original ART matches 36 request outcomes across four attached
states, including 32 exact success replies and four null-list errors. Six-filter
groups cover path/query/fragment, mixed patterns, Unicode and hash collisions.
Native UriRelativeFilterGroup insertion now preserves the original signed-hash
ArraySet order (#962); the hand-built parcel unit uses the original canonical order.
The first expanded oracle exposed ART's four-byte supplementary ABX strings.
Native ABX now reads both those and legacy six-byte surrogate pairs, and writes the
pinned ArtFastDataOutput form. Original ABX contains f0 9f 98 80 for the emoji;
the full original oracle passes with those group states and exact Bundle replies
(40.76s, no skips). Unpaired UTF-16 remains unsupported (#843). XML tests pass (15),
as do 488 service units (0.50s; five excluded by default) and the native publisher
(0.89s), including unrestricted foreign-UID reads and trailing-input rejection.
Original ART reads the actual native Bundle and all six filter values through its
own creators (14.80s). Incoming DomainSet decoding is now available for the remaining verifier/user
mutation frontends (2026-10-06): direct sets, inline blobs and immutable/mutable
ashmem FD blobs. It preserves null/empty host distinction, deduplication and
ArraySet hash/collision order; impossible counts, invalid blob frames and short
FD backing fail explicitly. An original ART sender executes generated method 5
against a test-only decoder endpoint, verifying empty/single-host sets, a forced
small inline blob, a 4,000-host FD blob and a nullable duplicate/collision frame
(13.85s). The endpoint explicitly accepts FDs; the first fixture attempt rejected
the large transfer at the Binder node, before decoding. This establishes incoming
codec/transport behavior, not verifier-state method implementation. Native units
pass (490, 0.50s; five excluded by default). The UUID mode needed by the verifier/user mutation frontend is now an explicit
original bridge policy (generated bootstrap method 20, 2026-10-06). It asks the
original process's VMRuntime SDK and Compatibility change 263076149, the same
selection UUID.fromString makes; native code does not infer it from an APK SDK or
reuse the domain-settings V2 policy. Only system UID may ask. Native tests reject
empty/trailing replies, and the original runtime policy oracle confirms the same
boolean and real foreign-UID denial. The full original oracle passes (30.56s,
no skips), and 490 units pass (0.68s; five excluded by default). This establishes
mode ownership. Native UUID parsing now implements current/Java8 normalization,
field truncation/overflow, plus and trailing-dash rules, UTF-16 hexadecimal digits
and the original numeric error messages (2026-10-06). An original-runtime fixture
selects each mode through Compatibility's public behavior delegate, calls original
UUID.fromString and restores that delegate on exit; no reflection or patched JDK
classes are used. All 1,648 cases agree in values and errors, and all 65,536 UTF-16
Character.digit results agree. The full oracle passes (29.83s, no skips); 491 units
pass (0.51s; five excluded by default). The port retains OpenJDK notices/Classpath
exception, with pinned ICU digit data under its own license. Method 5 integration
now serves generated method 5 (2026-10-06). The endpoint accepts incoming FDs,
uses the original UUID mode with native parsing, resolves DomainSet, validates the
public state code before verifier authorization, then checks UUID/package/domain
ownership. Invalid ID and unknown host statuses do not persist or publish; absent
code returns name-not-found. Empty sets and invalid state codes retain original
IllegalArgumentException messages. A valid request applies the existing original-
compared native state transitions and cross-package user-selection revocation,
then commits through the shared Store/CAS/cache-invalidation path. Authorization
and commit capture races recapture/retry; uncommitted and committed failures stay
explicit. Native Binder tests exercise malformed UUID/state, empty/null set,
unknown ID/host, null host, denied verifier and trailing input, preserving file,
capture and invalidation count. Original ART sends 4,000 hosts by ashmem FD to the
actual DomainQueries endpoint and reads every newly published SUCCESS value with
its original Info creator (14.62s). Units pass (491, 0.51s; five excluded by default),
as does the publisher (1.00s). Runtime shared-writer binding remains part of full
native bootstrap. The native user-selection state model now mirrors the public service's two-pass
revocation (2026-10-06): existing exact selections bypass owner conflict checks;
otherwise verified/instant approval blocks enabling. Lower-priority approvals
revoke only the latest-install packages at the highest approval level, retaining
all latest-time ties. All requested domains must pass before any revocation; disable
removes hosts without priority checks. A new attached user is allocated before a
blocked approval, matching the original in-memory side effect. Original public
API comparisons agree in 120 status/state transitions across five target packages,
two users, single/multiple hosts, enable/disable, higher-owner link handling and
legacy/V2 modes (full oracle 30.67s, no skips; 491 units 0.56s, five excluded by
default). The comparison canonicalizes logical legacy maps independently of output
iteration order. This is state-model evidence; method 7 requires a nonpersistent
capture path for failed-approval allocation, before its Binder frontend can be
connected. A separate RuntimeDomainUpdate now provides that no-write publication
path (2026-10-06): the attached domain owner/query projections change while scan
settings retain the last disk domain base. The disk commit API takes a different
update type. Publication validates bridge, captured base and next generation;
stale updates return without publishing. It performs no file write or original
cache callback. Native tests retain the old capture, verify disk/Store/settings
base and invalidation count unchanged, reject a stale candidate, then commit a
normal update including the runtime URI-group change. The subsequent disk base
check and file reread pass; only that persisted update invalidates the cache.
Publisher passes (0.98s), 491 units pass (0.50s; five excluded by default), and
actual original ART Binder queries/mutations still pass (15.46s). Runtime publication
is implemented. Generated method 7 now serves public user selection (2026-10-06):
UUID/DomainSet parsing precedes selector authorization; ID lookup and captured app
visibility precede current code/host validation. The visibility step adds no duplicate
permission request. The existing original-compared model applies highest-level/latest-
install revocation. Success commits via shared Store; blocked approval publishes only
new user allocation, and an unchanged blocked state publishes nothing. Original-policy
callbacks and publication races recapture/retry. Native Binder tests cover bad UUID,
null/empty set, unknown/null hosts, missing user, denied caller, invalid ID and trailing
input, leaving file/capture/cache count unchanged. The actual endpoint's verified-host
conflict returns ERROR_UNABLE_TO_APPROVE while allocating an empty attached user in
memory: old capture, Store state and scan disk base remain unchanged. Original ART
then selects two hosts, reads selected values, and sends a 4,000-host FD set to disable;
original UserState confirms each new state (15.53s). Units pass (491, 0.50s; five
excluded by default), as does publisher (0.96s). Sized AIDL URI group/filter DTO codecs now retain nullable strings, nullable
filter elements and a nullable filter list, with default fields for short records
and skipping future fields (2026-10-06). Original creators confirm 68 read/write
roundtrips across null/empty/ordinary/Unicode filters, numeric fields, empty/null
lists and null elements, with exact reserialization. The full original oracle
passes (29.78s, no skips), as do 493 units (0.55s; five excluded by default). This
is lossless wire DTO evidence, not completed method 8. Original group conversion
preserves null filter strings, whereas the current domain/intent filter model
requires String (#964); model preservation remains open. A checked nullable DTO matcher now retains bool,
null-pattern and invalid-pattern outcomes instead of suppressing errors (2026-10-06).
Original matchData agrees in 672 cases over null/empty/ordinary/malformed patterns,
path/query/fragment and unknown URI parts, all pattern kinds and six URI forms.
A trailing opening set in an advanced glob throws StringIndexOutOfBoundsException
in the original, which the native parser previously categorized as an unterminated
set. Native checked construction now retains the UTF-16 length/index bounds error
(#965); the original oracle records that category distinctly. Full original oracle
passes (30.85s, no skips); 493 units pass (0.53s; five excluded by default). This
is matcher/DTO evidence: the domain/intent model now stores nullable filter strings (2026-10-06).
Cache and intent-filter XML reads preserve missing versus empty, ArraySet hashing
uses zero for null, and cache/Bundle writers preserve null strings. Domain updates
normalize without substitution. Domain persistence has different XML semantics:
SettingsXml omits a null filter attribute and DomainVerificationPersistence skips
that filter on read. Native domain writing/reading follows that behavior while
retaining null in the live runtime owner. Matching returns typed errors through
intent resolver, component queries, preferred candidates, visibility construction
and domain filtering; NPE/IAE replies preserve the original exception, and bounds
failures preserve the original Binder transport status.
The original 672-case matcher corpus now also goes through real model groups; full
oracle passes (33.42s, no skips), 496 units pass (0.52s; five excluded by default),
publisher passes (0.98s) and actual ART Binder passes (25.61s). New regression cases
retain null/empty Bundle/XML metadata and reject a nullable matcher resolver query.
Generated method 8 now decodes a retained lazy Bundle after URI-agent permission
checks (2026-10-07), returns early for an empty bundle even with no package, and
looks up the exact attached name before applying keys in ArrayMap order. Invalid
domain keys are ignored; null/wrong-typed/empty lists remove that key. Sized group
and filter DTOs convert without nullable-string substitution. Changes use runtime-
only publication, with no Store write or cache callback; an error after earlier
keys retains those runtime changes. Native Binder tests confirm null group metadata,
removal, denied callers, framing rejection, empty/missing-name behavior and unchanged
file/Store/disk base/invalidation count. Original Bundle inputs cover twelve empty,
wrong-type, null/empty list, nullable DTO, CharSequence and null Parcelable-name forms. Full original oracle passes
(30.21s, no skips), units pass (496, 0.67s; five excluded by default), publisher passes
(0.96s), and actual original ART invokes the setter then reads action/numeric/null
filter fields with its own creators (15.45s). A stub putString declaration initially
failed pinned-image linkage; it now belongs to BaseBundle, as original does.
All nine domain transaction paths are present, but broad Bundle type handling,
exact conversion/domain-error replies (#964/#966), service CTS and native startup
writer binding remain unproven. Owned lazy Bundles now retain relative Binder object
offsets alongside bytes (2026-10-07), allowing unrelated IBinder values to decode
as typed-getter mismatches. A prefixed Bundle unit verifies offset rebasing, a later
key and call trailer. Original service oracle removes the URI key for Binder input
without scheduling a write; actual ART sends the Binder plus a nullable URI group
and confirms removal and the later group's survival (17.15s). Full original oracle
passes (31.24s, no skips), 497 units pass (0.50s; five excluded by default), and
production/final host builds pass. URI Bundle CharSequence values now use the
existing TextUtils span reader, including arrays and null text. Four original
Parcel inputs (ForegroundColorSpan, an array, null text and a non-plain kind)
produce the original typed-getter mismatch; the original service removes the
URI key without a persistence write. Full original oracle passes (29.85s, no
skips), 497 units pass (0.52s; five excluded), and actual original ART sends a
styled Bundle to the native setter while retaining another URI group (15.60s).
The image/template build passes (9 rebuilt, 33 fresh, 54.7s). URI-group DTO
conversion now returns the original NPE messages for a null group, filter list
and filter element. A three-case original constructor oracle compares exact
messages; actual ART receives the matching Binder NPEs and rereads the unchanged
URI group after all three failures (15.53s). Full original oracle passes (30.90s,
no skips), 497 units pass (0.57s; five excluded), and the production build passes
(3 rebuilt, 39 fresh, 1.3s). A null Parcelable class name in a URI list now
retains the null element, matching original creator behavior, and continues
reading a following normal group. The original oracle verifies both list forms
and the service's NPE with no group removal or settings write. The added input
reproduced a native typed-getter mismatch before the fix; full original oracle
now passes (32.12s, no skips), 497 units pass (0.57s; five excluded), and actual
ART sends the null-named Parcelable and receives the original NPE while retaining
the group (15.29s). Production build passes (3 rebuilt, 39 fresh, 1.3s).
A three-domain Bundle inserted in reverse order now verifies partial updates on
a null-group failure: the earlier signed-Java-hash key changes, the failing key
retains its prior group, and the later key is untouched. Original service
oracle confirms no persistence request; native publication advances one version,
retains the previous capture and leaves file/Store/disk-base/cache unchanged
(1.15s). Full original oracle passes (41.19s, no skips), actual ART sends the
partial update and reads the same outcome (15.78s), and production build passes
(42 fresh, 0.3s). These are conformance checks of the existing runtime-only
publication. Matcher errors now retain the null pattern kind and expose the original Java
exception class/message; null literal reports Object.equals, while the other
null pattern paths report String.length. The original matching corpus expands
to 1,680 cases, including six malformed advanced patterns, and compares exact
classes/messages alongside results. Visibility construction retains the typed
matcher failure rather than replacing every error with a generic NPE; its
NPE/IAE replies use the original category/message, and failed construction
retains the prior resolver. Bounds errors cannot be serialized by original Parcel. Original Java Binder
lets them escape to JNI, which returns UNKNOWN_TRANSACTION; native visibility
construction now returns that transport status too.
Ordinary intent query paths now retain typed matcher failures through component
search, preferred activities, domain filtering and safer-intent enforcement.
The shared shadow/native Binder query frontend serializes the original NPE/IAE
instead of reporting these errors as NotModelled. Non-serializable bounds
failures now return UNKNOWN_TRANSACTION from native PackageQueries. Shadow
answers now retain Answer::Status through intent queries and visibility
construction, allowing the existing comparator to compare TF_STATUS_CODE
replies instead of counting these paths as not modelled. A query test covers activity,
service, receiver and provider queries, intent/service resolution, and a foreign
app's explicit activity query through safer-intent checks, for null literal,
prefix and advanced patterns and a malformed advanced pattern. Full original
oracle passes (31.96s, no skips), 499 units pass (0.64s; five excluded), and
production build passes (3 rebuilt, 39 fresh, 1.5s). A real original ART Binder
callback throws a StringIndexOutOfBoundsException and returns UNKNOWN_TRANSACTION
(-74) to native; a following IAE callback succeeds, proving the connection
survives. Actual ART then calls native PackageQueries against controlled parsed
metadata: bounds produces transact=false with an empty reply, while malformed
advanced pattern returns the original IAE/message (16.55s). Resolver tests also
cover transport status across all six query/resolve methods and explicit
safer-intent matching, and visibility-construction bounds failures. Full original
oracle passes (32.23s, no skips), 499 units pass (0.67s; five excluded), and
production build passes (3 rebuilt, 39 fresh, 1.5s). Shadow bounds tests now
assert status answers from all six query/resolve methods, explicit safer-intent
matching and visibility construction; the comparator's status-reply comparison
also passes. Current units pass (499, 0.53s; five excluded) and host build passes
(3 rebuilt, 10 fresh, 15.0s). The original ART integration was not rerun for this
shadow-only change; its preceding 16.55s result remains the native transport
proof. Original XML persistence is now verified for null, empty and ordinary
filter strings across four owner cases: writing succeeds and retains three live
filters; restoring/re-writing omits the null filter while preserving empty and
ordinary strings. The added oracle reproduced the previous native reader's extra
null filter (three instead of two). Native XML writer no longer rejects null;
scan settings keep the read-back XML projection while the live owner retains
nullable filters, preventing the next disk write from failing its base comparison.
Native publication verifies both projections, old capture immutability, sequential
writes, concurrent writes and committed-error publication. Full original oracle
passes (30.90s, no skips), publisher passes (1.12s), 499 units pass (0.55s; five
excluded), actual ART Binder regression passes (15.97s), and production build
passes (3 rebuilt, 39 fresh, 23.4s). Nullable metadata/matching compatibility
tracked by #964 is verified; full domain/PMS activation remains gated below.
URI setter key errors now match original isValidDomain: a null key returns the
exact String.length NPE; an empty key produces the original StringIndexOutOfBounds
transport outcome, UNKNOWN_TRANSACTION, instead of a synthetic IAE. Original
service and actual ART sender tests use reverse-inserted Bundles to confirm that
a preceding negative-hash domain changes before the failure, while a later domain
is unchanged. Native publication also verifies one epoch per partial change and
unchanged disk/Store/base/cache (1.13s). Full original oracle initially failed in
APEX notification with SIGKILL (#902), then SDK data setup with ENOSPC (#903,
host available 1.9GiB). Removing only this task worktree's debug/release incremental
caches increased available space to 5.7GiB; the next full oracle passed (29.24s,
no skips). This is not a fix for either intermittent environment issue. Actual
ART Binder passes (15.85s), 499 units pass (0.51s; five excluded), and production
build passes (3 rebuilt, 39 fresh, 1.6s). Other #966 generic type/error cases and
full native PMS/facade/CTS/app gates remain open. URI list Parcelable class
validation uses immutable bootclasspath DEX type relations captured with the
native scan/query context; no class initialization or guest reflection is added.
Missing classes and non-Parcelable classes return the original BadParcelable
category/message, while existing unrelated Parcelables produce the typed-getter
mismatch. Twenty original lazy-Bundle cases cover the same ten class names as both list
elements and top-level Parcelable values: missing classes, String, filter DTO,
Bundle, normal group DTO, slash names, primitive/object arrays, absent array
components and invalid array descriptors. The top-level path now checks class
availability/Parcelable implementation before rejecting the ArrayList type,
instead of treating every Parcelable value as a mismatch. Original service and
actual ART native Binder tests verify ten class cases, including unchanged state
on error and removal on mismatch (17.40s); original service schedules no settings
write. Full original oracle passes (30.89s, no skips), 499 units pass (0.49s; five
excluded), and production build passes (3 rebuilt, 39 fresh, 1.1s).
Unconfigured class metadata/unsupported creators fail explicitly. Serializable
values now preserve null-name behavior and malformed ObjectInputStream header
errors in both top-level and list-element forms. Twelve original cases show
that this Bundle path reads the stream before resolving/checking classes: even
an unknown name with a malformed stream yields the original IOException-based
BadParcelable message. An initial class-before-stream implementation failed that
oracle and was replaced. Six original service/actual ART cases verify exact
error messages, unchanged groups on I/O/NPE failure, and group removal for a
null top-level Serializable; original service schedules no write (18.12s).
Full original oracle passes (29.15s, no skips), 499 units pass (0.53s; five
excluded), and production build passes (3 rebuilt, 39 fresh, 1.2s). Valid serialization now reads short/long modified-UTF strings and the pinned
libcore ArrayList descriptor, size field, compatibility capacity block, null/string
items and known references. Eight original ObjectOutputStream cases verify string,
empty ArrayList, null-item ArrayList and string-item ArrayList in both top-level
and list-element forms. Original typed getter retains a serialized ArrayList's
string item; service conversion then throws ClassCastException, whose native
transport outcome is UNKNOWN_TRANSACTION. Null-item lists retain the original
constructor NPE; wrong list-element object types remain typed-getter mismatches.
Original service and actual ART verify all eight outcomes, error-state preservation
and successful group removal (22.95s); no original settings write is scheduled.
Full original oracle passes (30.88s, no skips), 499 units pass (0.50s; five excluded),
and production build passes (3 rebuilt, 39 fresh, 1.3s). Other object schemas,
noncanonical serialization forms, creators and malformed Bundle behavior remain
#966 and fail explicitly where unsupported. Class metadata now comes from the request's retained domain capture rather than
an endpoint constructor argument. Context.with_boot_classpath loads the native
scan image's boot class relations, and query/domain publications retain the same
immutable metadata; a publication test asserts identity across an update. The
ART fixture's separate capture path also supplies its image metadata. 499 units
pass (0.55s; five excluded), publisher passes (1.11s), actual ART Binder passes
(24.31s), and production build passes (3 rebuilt, 39 fresh, 21.5s). The initial
ART run failed with SIGKILL (#902); no cause or environment fix is claimed.
Production first-boot orchestration/service registration and full domain/PMS
conformance remain #957/#798; original services stay active.

User-state lookup
applies the original user-existence/cross-user/visibility contract against the
same capture, retaining nullable package names. Visibility denial returns the
original name-not-found service-specific error; missing users/cross-user
permission deny with SecurityException. Native projections preserve all-web-host
hash order, verified-over-selected priority and per-user link handling (default
allowed), including an attached package with an empty web-host map. The UserState
parcel uses the original 0x8 flag byte, UUID/name, typed UserHandle and shared
map/blob codec. Sixteen original add/migrate user-state replies for users 0/10
match exact native bytes. Native Binder tests cover empty-map/default-allowed
replies, null/missing names, tails and app cross-user denial (0.91s). The original
ART Binder client now reads both large Info and UserState replies with an FD,
checking all 4,000 user domains, user ID and link handling (14.08s, no skips). Info uses the same
captured package-name resolution (Settings renames and visible static libraries)
and native attached owner/policy, with requested-name lookup kept distinct from
resolved code. It returns null without auto-verification domains and the original
service-specific name-not-found code with a null message for unavailable code.
The publisher fixture covers null info, missing/null package names, malformed
requests and app DUMP denial. Native info keeps signed Java name-hash order and
hash-collision insertion order. Eight original add/migrate Info reply parcels
match exact native bytes. Host maps use the original UTF-16 size estimate including
reply headers, inline maps/short blobs and FD-backed blobs. The new aim-ashmem
crate shares backing-file flags/state encoding with the existing syscall owner;
only a disposable unlinked region is created. Five ashmem ABI tests pass,
including native region size/device/protection and rejected writable mappings.
The original ART creator reads 4,000 entries from a native FD-backed Info packet:
the test passes its FD to a disposable client and installs the file object at its
native Parcel offset. A separate original ART client now resolves the actual
native DomainQueries endpoint through ServiceManager, calls its generated Info
transaction and reads all 4,000 domains through the original creator. It asserts
the reply contains an FD (12.76s, no skips). This uses a test-only native code
metadata copy with added auto-verification hosts; no APK/image/extracted tree is
modified. seInfo/shared-process values are recomputed at their owning stages,
then the normal runtime/domain completion and validated capture publication run.
The test registry implements pinned checkService2's Service/ServiceWithMetadata
framing. Failure-only AndroidRuntime logging identified the earlier SIGKILL as
this registry's missing checkService2 response; that scaffold failure is fixed.
This proves real Binder FD transport; it does not prove full native PMS startup
or the named app/CTS migration gates. Native codec tests cover the exact 32 KiB boundary, UTF-16 surrogate pairs,
small inline blobs with a large prefix and empty-map behavior. The full original
parcel fixture passes (30.55s, no skips); service units pass 488 cases (five
excluded from the default run, 0.49s); binder-host units pass 14 cases (0.02s).
The image/original-PMS template build passes (six rebuilt, 36 fresh, 53.5s);
final host build is fresh (13 nodes, 0.3s). Same-process service
lookup now retains and dispatches the returned local object (#961), with the
original BBinder calling identity rule: an incoming caller's UID/PID is retained
through a nested local call, then restored; outside a call, the host service's
own Android credentials apply. Local request FDs close after dispatch and reply
FDs close with the received reply; retained files remain valid. Native tests
exercise app identity through the actual domain-to-permission call, local service
replacement and malformed replies; binder-host tests cover owned/inherited
identity, file retention/close and transaction failures. The complete binder-host
suite passes (14 unit cases, one backend integration, six transport cases).
Remote handle references and death notifications retain their driver path;
local objects have no remote process death registration. Local lookup is repeated
so replacement does not keep a cached old node. The existing native node release
gap remains #568. The retained bootstrap bridge now delegates
UID-specific verifier checks to the current original domain owner's proxy,
rejecting missing owner/proxy and negative UIDs instead of inventing identity.
Native authorization combines this producer, original permission front-end checks
and captured users/AppsFilter, checking bootstrap/query identity before and after
callbacks outside the publication lock. Native tests cover current root/system
queries, denied app internal calls, stale-capture rejection, per-user verifier
UID replies and malformed responses; publisher fixture passes (0.74s). Original
ART verifies seven controlled-proxy UID delegations, the actual original unavailable
proxy, missing owner/proxy failures, negative UID and actual foreign-UID denial.
Active installed verifier component selection and app permission-front-end
integration against live system_server remain unproved. The fixture runs the image's original collector with a controlled
compatibility override and original mutable ApplicationSharedMemory owned by
the disposable oracle process.
The original ART fixture now invokes DomainVerificationService.addPackage and
migrateState over one native parsed package and four persisted-state inputs:
pending state, immutable
system linked-app state, and restored state with matching/mismatched signatures.
All eight persisted attached states match the native owner, including migrated
state codes, filtered user hosts and link-handling booleans. Six signature-digest
cases match original PackageUtils (zero/one/multiple signers, reversed order,
duplicate signer and empty certificate bytes). The complete parcel fixture passes
(30.18s, no skips); release units pass 488 cases (four excluded from the default
run, 0.49s). The full build passes (17 nodes rebuilt, 25 fresh, 0.7s);
the image/original-PMS template build passes; final host build is fresh (13 nodes, 0.2s).
URI-group migration, pre-verified transitions and missing-owner recovery
are unit-tested/source-compared but have not run against the original service.
Verifier delivery, production boot domain-state publication and native domain
Binder registration remain unproved. Native cache read/write and facade serialization
now reject null max/min aspect ratios for activities and receivers, matching
the original ParsedActivityImpl Float-unboxing failure (#958). The original
parcel fixture checks zero/nonzero values with exact roundtrip bytes and both
independently null fields for both component kinds. Release units pass 488
cases (four excluded from the default run, 0.49s). The full build passes
(17 nodes rebuilt, 25 fresh, 0.7s); final host build is fresh (13 nodes, 0.2s).
The earlier original policy fixture
compares actual PlatformCompat at SDK 28/29/30/36. All test targets compile;
compilation does not count them as executed. The image/original-PMS template
build passes (57.7s; nine rebuilt, 33 fresh).
The facade is not registered, and
native visibility/version-page refresh and default native activation remain unproved;
original debug-build application remains unrun (#946). System.complete_package_scan now resolves
native library dependencies from the exact retained bootstrap bridge, completes
runtime from native usage/seInfo/dependency owners plus explicit retained inputs,
and publishes through that same bridge/base gate (#937). Original policy calls
run without the publication lock; failure does not replace the previous version.
The pinned-image bootstrap fixture scans the actual framework APK through saved
system/data phases, supplies explicit fixture migration/fixed/process owners,
then verifies complete publication and actual service-host Binder code access.
Missing permission inputs and original policy rejection preserve the prior Store.
The daemon's initial, saved-system and saved-system/data scan APIs now require
the caller's explicitly captured bootstrap bridge (#943), carrying the same
identity to completion/publication. They reject missing or changed owners before
policy reads and recheck that identity before returning a completed scan; they do
not recapture a newer bridge between phases. The fixture rejects a distinct
wrapper of the same Binder in all three entry points without changing the saved
candidate. Actual Binder replacement/current-owner death fail the same identity
check. The explicit pinned-image integration passes (0.66s); ordinary release
units pass 470 cases (0.47s), with three ignored/not run. The host build passes
(14.7s; three rebuilt, 39 fresh). This is an invoked native completion boundary in
the bootstrap fixture, not the running NativeServices boot producer or native
package activation. This does not prove original-service adoption or CTS. Mapped APK signing
source IO classification is verified (#920). The runtime scan diagnostic now performs
collection before its saved-record reconciliation as well: it retains original
GSF current flags 23 and compares complete package settings. Original PMS runtime
comparison passes for 243 active/3 disabled packages, 16 shared UIDs and 23
libraries, with saved sequential loops covering 240 system and 3 selected data
APKs from parsed-only system and data inputs (39.87s). This is an owner diagnostic, not native `package` default boot or
CTS acceptance. The normal AST importer now derives public keys after completing the current
certificate list. Invalid DER clears prior package/shared UID signing after
table mutations; missing counts retain existing targets, and each initiator
container gets a fresh signing owner. Original Settings comparisons add 24
text/ABX cases for invalid current/past certificates, repeated replacements,
shared UID and initiator owners, and later certificate-table references. All
85 default-owner cases compare serialized public-key class/byte hashes as well
as signatures and current flags. Public test certificates replace invalid
placeholder bytes in signature persistence units; no private keys are retained.
Broader CertificateFactory encoding acceptance remains unproved (#916). Full frontend error ownership must distinguish native failures
from guest XML/certificate failures before file cleanup (#914/#912). Active/disabled package attributes, shared UID headers,
library versions/optionality and split revisions now use only the default-value
getters the pinned owner uses (#915), including malformed values. Public/private
flags use original string parsing rather than typed integer conversion. Actual
Settings.readSettingsLPw matches 174 text/ABX cases (24.02s): package metadata,
install-source fields, flags, loading, library/split arrays, shared groups, and
main/reserve retention agree; valid values remain intact. Signature/permission/keyset readers now match the original explicit default
getters, certificate-table append and cloned past-signature flags, repeated
signature/history replacement, permission protection/icon defaults, and
keyset-version presence checks. Keyset public-key identifiers are read before
optional bytes, and invalid public keys are ignored while valid keys are
canonicalized. Actual original Settings.readSettingsLPw matches 85 additional
text/ABX cases with real certificates and public keys, including original keyset
writer output (24.02s). Required-getter failures in these cases occur before
other owner mutations; the whole-document callback does not prove partial
package/permission/keyset mutation recovery. The pre-M public flags now migrate hidden/cannot-save-state/privileged bits
to private flags, and the older system string uses original case-insensitive
true/default semantics. The original comparison adds 42 text/ABX cases for
legacy flags, negative values, malformed and typed attributes, and precedence
over older fields. Remaining getter edges still need audit (#915). Full Settings recovery, current-version initialization
and nonregular/open-IO cases are not proved. Passive Store::open
and Store::create remain strict and do not adopt malformed artifacts.
Ordinary units pass 455 (0.51s; three ignored, including the crash helper explicitly
run by its parent), XML units previously passed 15, and all current integration targets compile.
The current full image/template build passes (69.0s, nine rebuilt, 33 fresh);
the final production build passes (14.6s, three rebuilt, 39 fresh).
The prior original-APK initial scan fixture passed (68.36s), writing/reopening
both fresh and existing native settings and checking immutable input metadata,
ABX/reserve equality and repeat-write stability; it was not rerun for recovery.
The prior controlled Binder fixture (9.52s) was not rerun either. This is global
setting persistence, not a complete Settings.writeLPr boot transaction: remaining
global owners, user writers, packages.list and coherent publication still need
integration. Disabled serialization is not covered by the original ART writer
comparison. Original PMS remains live; native CTS/apps/template/APEX/rollback
acceptance remains unrun.
The syscall layer translates Darwin directory-unlink EPERM to Linux EISDIR
without following symlinks (#913). This lets original bionic remove fall back to
directory removal. Expanded NDK file tests actually run and pass (1.55s), covering
empty/nonempty directories, directory symlinks, invalid flags, missing paths and
libc remove. The original ResilientAtomicFile failure/retry ART oracle now removes
its own reserve directory and passes with the complete package oracle (24.02s).

Failure-only AndroidRuntime log collection now exposes original crash-handler
SIGKILL after ENOSPC (#902/#903): the latest policy WRITE failure is a
Files.write IOException, not evidence of an external kill. Crash reporting also
fails service lookup with BinderProxy EBADF (#907). Prior failures without these
logs remain causally unproven. Task-owned Rust compilation intermediates had
accumulated under debug/deps; removing those rebuildable objects recovered host
free space from about 1 GiB to 40 GiB before the passing untraced oracle. The host
storage reserve and guest image capacity remain unchanged. Transfer ownership remains
#890; native PMS is still inactive. First shared code uses the supplied
compatibility owner for its scan label when no group member has loaded code,
then seeds the group SDK when the first parsed member commits (#894). Retained
loaded groups keep their SDK; compatibility failure rejects the candidate while
preserving the original group allocation. The original ART oracle passes in
31.59s, including typed original Settings.addPackageSettingLPw final registration,
SELinuxMMAC labels and SharedUserSetting commit SDK, unchanged archive bytes with
controlled shared identity, unread policy and
a declared-target compatibility decision. Original Settings group removal/pruning
and re-registration also agree for changed groups with and without disabled factory
references, and for a non-shared replacement. Native scans use a disposable updated
path to unchanged original archive bytes, preserve pendingRestore, query the new
group's compatibility owner, restore distinct active/factory group identities and
retain old settings/loaded code on seInfo rejection while keeping only the newly
allocated empty group. The full initial scan regression
passes in 68.24s, including grouped container registration and the first APK
allocation at 10001 after group 10000. All integration targets compile, 436 units
pass (one ignored/not run). The original-image API check and full image/template
build pass in 71.3s (nine rebuilt, 33 fresh). These fixtures do not prove a live PlatformCompat call
or a manifest-declared shared APEX in the original image. The generated, system-UID-guarded bootstrap bridge now delivers completed native
containers to original ApexManager.notifyScanResult (#891). It restores original
PackageImpl objects and signer capability flags, validates the complete frame
before invoking the owner, and propagates transport and owner failures. The
original ART oracle uses a fresh original ApexManager implementation and verifies
module and active-package mappings for all 42 results; it does not notify the
running original PMS singleton. Missing counts, unaligned and trailing payloads
reject. Complete daemon ordering remains #885/#836/#798; the running daemon still
does not invoke native initialization. All 436 units pass (one ignored/not
run), including generated Binder notification and reply/error handling. Actual original ART also continues to verify the user/APEX
inventory exporters, static-library identity, complete package parcels and signer
lineage. The full image/template build, including the original-image API reference check,
passes for the typed notification boundary. All integration targets compile after
the separate shared-ID capture change. Its original-image API reference check and
full image/template build pass (67.2s; nine rebuilt, 33 fresh); subsequent host
rebuilds also pass. The full original ART oracle passes in 29.50s, and the system
scan regression passes in 65.25s.
A disposable default window boot reaches sys.boot_completed at 4.870s; generation
60 contains 42 all-package and 42 active APEX inputs, one resolved scan user,
290 package user scopes, 16 shared-process aggregates and 290 runtime owners
with no feed errors. No owned probe processes
or mounts remain. This is user policy inventory, not complete PackageUserState
value export (#862). Original UserManager construction/facade boot ordering and
permission/writer callbacks remain #858/#798; full native scan initialization and
publication are not yet invoked by the running daemon (#836/#798). Restored
legacy-domain migration inputs also now discard packages unknown to Settings,
matching the existing user-state ownership filter (#877), and a missing user
restrictions file produces no migration inputs.
Production device-services image API checking passes (14.0s), and system-server
symbolic call-count/signature and DEX verification pass (1.1s). The complete
derived image/oat/template build passes (55.8s), with disposable template settings
at 7.4s, sys.boot_completed at 10.2s and permissions at 22.7s. The template
builder cleans its owned processes/mounts. The native bootstrap now reads the
original PackageBackwardCompatibility policy over its attached Binder, rejects
owner errors/malformed/trailing replies before replacing any current endpoint,
and uses the retained image property for LibraryCompatibility. All 356 units
pass (4.90s), including one-time reads, cached input use, failed-policy attach
preserving the current endpoint, replacement/death and no late listeners. The
full build passes (15.2s; template cached), followed by an explicitly run
disposable original-PMS boot and snapshot/usage/shared-UID runtime oracle
(11.35s) using the rebuilt native guest-init. Attachment cannot succeed without
the original policy response, and failed attachment aborts the PMS-main wrapper;
boot completion therefore verifies the real early policy transaction. Owned
process/mount cleanup completes. Native scan consumption, actual native-library/
seInfo compatibility and GID calls remain unverified (#834/#838/#798).
A Binder-driver test passes
(0.02s), exercising synchronous attach, invalid/null/foreign callers and tokens,
wrong endpoint rejection, policy/GID inputs and failures, replacement, actual
endpoint death, cleanup and absence of late listeners. All three native
resource-client tests pass (0.01s); AIDL generation and image API linkage pass
(14.2s combined). These protocol/linkage checks supplement the verified early
original-PMS handoff. Native facade boot ordering,
other booted owner transactions and callback delivery remain unverified, and
original PMS remains active. The full late bridge now retains its Binder and
nonce mapping as one owned source (#835/#841). Death only clears the current
source when its retained Binder identity matches; old deaths cannot erase a
replacement. Reattaching the same endpoint reuses its single Binder death
registration while replacing the mapped source. Invalid mappings, owner errors
and trailing shared-memory replies leave the current source/listeners unchanged.
Permission caches match both mapping identity and numeric nonce (#840), so equal
nonces across system_server generations cannot reuse an old answer; an in-flight
old query cannot insert into a newer source's cache. Without a map every check
still queries its owner. Real fileport/mapped-store and Binder-release tests
verify replacement, same-endpoint reattach, old/current death and retaining the
current endpoint after the caller drops its temporary handle. Controlled mapped
cache tests verify equal-nonce replacement and a blocked old reply. Regression
controls using the previous unconditional cleanup and nonce-only cache both fail
on these tests. All 358 regular units pass (3.39s). The full host/image build
passes (19.0s; template cached), followed by the explicitly run disposable
original-PMS snapshot/usage/shared-UID runtime oracle (11.59s) with the rebuilt
guest-init; original boot completion and owned process/mount cleanup pass.
This verifies the restart/cache infrastructure and original boot, not native PMS
facade activation or its CTS/app/template/rollback gates.
The facade now has Java PackageManagerLocal unfiltered/filtered snapshot scopes
(#836), constructed from captured immutable replica records and a native owner
bound to that version. Maps are copied and immutable; package objects retain
identity. Closing a parent makes its children unreadable, and child close leaves
siblings/parent usable. Single-package lookup delegates internal-name resolution
and filtering to the owner and rejects a returned key absent from the captured
replica. List filtering is cached per scope and delegated with the exact captured
version, candidate, caller UID and user. ART's uncommitted package bypasses
single-lookup filtering but still participates in list filtering; it is not added
to the list if absent from the captured inventory, matching the pinned original.
An explicit original-runtime test passes (14.42s), using original sealed
PackageSetting records/interfaces and controlled owner replies to check lifetime,
identity, immutable maps, old/new version isolation, aliases, missing names,
uncommitted candidates, owner errors and replica mismatch. This verifies snapshot
plumbing, not native visibility-policy conformance or complete native records.
All Java oracle classes, including these scopes, compile/dex/link against the
original image (3.04s), and device-services API linkage passes (13.4s). The first
attempt could not boot because its disposable display socket path exceeded
Darwin SUN_LEN; a shorter owned fixture prefix fixes that test setup (#795).
Native replica DTO/adapters, version-page capture/publication and original
permission/ART callback integration remain incomplete (#836/#833); these scopes
are not installed as PackageManagerLocal in SystemServer. Original PMS is active.
Cache transport does not preserve historical Signature capability flags: original
Signature.writeToParcel carries only certificate bytes. Native to_facade_entry
now pairs the cache with collected certificate/capability metadata, rejecting a
package/signing mismatch. PackageObjects reconstructs original PackageImpl and
restores a new SigningDetails after checking lineage presence, lengths and each
certificate. Signature arrays and the key set are detached from decoded inputs.
SigningScan's active and disabled inventories now retain a single
Arc<LoadedPackage> containing final AndroidPackage and complete collected
SigningDetails. Record.signing is collected code; saved package and shared-UID
signing remain in their separate owners. Final metadata registration rejects a
package/signing mismatch before changing retained state. Active-to-disabled
copy shares the entire Arc, and withdrawal removes both fields together while
older snapshots retain both. Detached candidate changes do not alter them.
The 572-parcel original-image oracle reads these retained entries directly and
passes (19.29s), including two native first-scan objects after their scan owner
is dropped. The collected GSF lineage flags are 21/23; original
checkCapability confirms individual/combined grant and denial masks, and a
revoked copy denies installed-data access. Malformed metadata rejects and input
mutation does not change restored flags or decoded objects. All 344 units pass
(3.15s); the first-system scan passes (43.64s), including real metadata-gate
rejection and unchanged retained state for mismatched signing. The disposable
original-PMS saved-scan also passes, retaining collected signing/capabilities for
all 243 active packages and three disabled factories, with owned process/mount
cleanup. All oracle DEX
linkage passes (2.92s), and device-services builds against original APIs (13.7s).
This verifies retained-code reconstruction, not callback delivery. Native query
replica publication and actual permission callbacks remain unimplemented
(#837/#833/#836). The private captured lease now adds method 8 for saved package
and shared-UID signing, separate from the collected PackageCode envelope. It
reads the captured active/disabled setting and actual shared UID owner, retaining
scheme, all current certificates and nullable historical certificates with exact
capability flags. Unknown saved signing remains original UNKNOWN, not a collected
code fallback. Native certificate/public-key reconstruction failures are explicit.
Java PackageSigningState is immutable and reconstructs detached original
SigningDetails through the checked original constructor, including its derived
public-key set. Lease reads validate version/name/active-or-factory scope and
certificates before caching; rejected and remote-failed reads can retry. Saved
setting restoration verifies version/name/UID/shared UID/scope before changing
its signing. This restoration never overwrites PackageImpl's collected signing.
All 358 regular units pass (3.11s), including native method-8 unknown/absent/null/
trailing framing and old captured version retention. The explicitly run original
runtime oracle passes (15.68s): all 288 cache entries and enriched variants plus
two native scan objects (578 parcels), with exact native/Java saved-signing bytes,
generated Proxy/Stub framing, original public-key reconstruction, unknown handling,
invalid-certificate retry, closed leases, and rejected restore isolation. Controlled
native versions change GSF's saved package historical capability from 21 to 20
and its shared UID to 17 while retaining the older 21 and unchanged collected
code. Original checkCapability verifies the independent grant/denial; mutation of
returned original Signature arrays does not mutate the DTO. All oracle references
link (3.20s); device-services image API checking passes (13.7s). Owned runtime
process/mount cleanup completes. These native and original-Java transport tests
remain separate; full PackageState/SharedUserApi construction, bootstrap export,
permission callbacks and native PMS activation are still incomplete.
Native scan_snapshot::Store now captures the whole SigningScan owner in a
versioned Arc, retaining settings, UID/group signing, keysets, libraries,
scanned user state and active/disabled code together. Capture validates duplicate
settings, UID membership, saved-group signing, loaded name/path/UID/signing and
user-state availability. Admitted scans are marked pending until metadata
finalization; pending candidates cannot be captured. Publication requires the
exact captured base and increments the version without wraparound. Stale/foreign
bases, invalid graphs and version exhaustion leave the current capture intact;
concurrent writers from one base have one winner. The 346 unit tests pass (3.13s),
and the original first scan passes (43.65s), checking real retained Arc identity,
old/new owner isolation, invalid code paths and unfinished metadata rejection.
The final disposable original-PMS saved scan passes (373.79s), validating one
capture over all 243 active packages and three disabled factories, preserving
their retained Arc identities and checking owned process/mount cleanup.
A private IPackageScanSnapshot Binder endpoint now leases one such capture,
serving its version, active/factory names and code envelopes in chunks of at most
64 KiB. Encoded envelopes are cached per lease; close releases the capture and
buffers and subsequent reads fail. System UID, interface token, argument tail
and range checks precede state changes. PackageScanLease reassembles and validates
length/version/name before caching immutable PackageCode DTOs; owner/transport,
short-chunk and wrong-version failures do not publish a partial cache entry.
PackageObjects restores original PackageImpl and collected capability flags from
the envelope. Native Binder-driver verification passes (0.02s), including a
150,000-character payload across multiple chunks, old-version pinning after new
publication, caller/token/range rejection, idempotent close and actual capture
release. The 572-parcel original-image oracle passes (19.57s), including real
native framework/GSF envelopes, original Java generated Proxy/Stub framing with
a controlled page owner, failures/retries, DTO identity/input isolation and exact
native/Java DTO bytes. All 347 units pass (3.13s); all oracle classes link against
the original image (2.84s), and device-services builds against original APIs
(15.6s). Fixtures generate the private Java AIDL into their disposable directory;
they do not depend on the builder's temporary work directory.
These tests verify native Binder and original Java transport separately. The
service host can now export a complete published scan lease (#936), but the
running bootstrap producer and SystemServer consumer are not yet connected. Complete
PackageState DTO/adapters, metadata/visibility pages, lease/snapshot-scope wiring
and actual permission/ART callbacks remain #836/#837/#833. The private bootstrap
AIDL now queries the original LegacyPermissionDataProvider for an appId over an
explicit user inventory, including pre-created users. Native immutable inputs
retain each user's missing marker, nullable/empty permission names, original
ArrayMap hash/collision order, runtime/grant bits and signed API flags. Null,
malformed, truncated, noncanonical and owner/transport failures reject; they do
not become empty state. The native Binder-driver test covers endpoint replacement,
owner rejection/death, malformed replies and retained earlier inputs. The actual
original ART LegacyPermissionState codec/copy oracle passes (16.07s with the
complete package oracle), including all truncated lengths and detached mutable
replicas; device-services links against original APIs. This validates the
transport and original objects separately, not a live SystemServer permission
query. The live exporter differs from SettingBase's legacy migration state.
A separate native Migration owner now ports original user-ID checks, permission
name replacement/hash ordering, missing markers, reset/copy behavior and install,
legacy-runtime and decoded-runtime merges. Install permission reads now consume
the original pull stream through Migration.read_install_events: each item mutates
all enumerated users immediately, nested items remain visible, unknown subtrees
are skipped and later XML errors retain earlier grants/missing markers. The AST
install entry uses this same owner instead of a separate recursive reader. Legacy
runtime XML keeps its distinct unknown-subtree behavior; both retain original
default-valued attribute getters. The install oracle now invokes the original
Settings.readInstallPermissionsLPr through a compile-only package helper; runtime
migration still uses a ported loop over original owners. Complete incremental
constructor/factory/detached UID binding and user-file restoration remain
#914/#858/#798. Package child handlers now receive the actual AppIds owner after
registration through read_package_with_ids. Native InstallRead routes package
perms to its own non-shared SettingBase or the UID owner visible at that event
(including a non-shared UID target), and shared-user perms to its supplied group
owner. Missing UID targets leave children in the original package loop and do not
replay grants after a later shared-user tag. Fixed markers are assigned only after
complete permission reads. Registration now invokes required package/shared owner
hooks before page-size validation, signature reads or other body events. Native
InstallRead constructs empty original SettingBase migration owners for newly
registered records and retains existing owners on repeats/retries. Missing
retained owners or constructor collisions report Owner errors, rather than infer
an empty prior state. Controlled fixtures start with empty owner maps; constructor
pre-seeding is removed. Pending package records now retain their individual
install-permissions-fixed marker in Package itself, instead of a name-keyed set.
They do not occupy the active UID permission map before attachment; duplicate
pending names and a pending record shadowing an active package no longer collide
with its constructor owner. Attempt reset drops pending records/markers directly,
retaining active/shared migration owner effects. Repeated active records preserve
that object's marker. Persistence comparisons exclude this runtime marker.
Factory updated-package records now have a native incremental reader and
InstallRead permission owner. The reader constructs locally with SYSTEM/priv-app
flags and the original disabled domain ID, does not register a UID, reads only
permissions/static/SDK-library children, and replaces the disabled setting only
after the body completes. Each factory own migration state is fresh; shared
permissions affect the UID owner visible at that event even if a later file error
prevents factory publication. Factory fixed markers remain false. Missing code
paths are FatalInput; nullable factory names remain an explicit unavailable owner
(#975). Actual original Settings reads null and empty factory names as separate
keys and replaces duplicate null keys with the last path/appId/version. Original
PackageSetting.getPackageName retains null; writeDisabledSysPackageLPr fails on
that name with NullPointerException. A direct original read/get/write probe now
records this contract. Native nullable identity representation is still missing
and #975 remains open; the native Owner error preserves selected input. The
expanded original ART contract probe passes without skips (58.79s), including
null/empty identity separation, last-null-key replacement and original serializer
NullPointerException. This turn changes fixture/contract evidence only; native
read/query representation still requires an explicit nullable factory model. Complete factory/group post-read binding, detached/retained object/user
identity and native PMS/SystemServer lifecycle remain #914/#858/#798.
Fifteen text/ABX factory fixtures agree with actual original Settings.readSettingsLPw
on SYSTEM/priv-app flags, appId/shared status, disabled domain ID, version/path,
false install-fixed markers, own permission bytes and active UID-owner bytes.
They cover prior/later shared targets, a non-shared UID target, active/factory name
separation, duplicate factory replacement, negative IDs, ignored signature children
and partial group mutations retained without failed factory publication. The
complete original ART oracle passes without skips (69.53s); 534 units pass
(5 excluded, 0.50s). AST imports and the stream now share the same factory publication owner: later
same-name records replace the old value without moving unrelated records (#976).
The text/ABX unit checks replacement path/version/appId/private flags and stable
neighbor placement. Successful factory oracle inputs compare complete AST/stream
factory records after excluding constructor domain/shared-ID fields; those
runtime IDs are independently compared against actual original Settings.
The native Store opens a disposable duplicate-factory settings file as one last
version and preserves its original bytes. 534 service units pass (5 excluded,
0.50s); the full original ART oracle passes without skips (69.53s), including
factory metadata/permission parity and successful AST/stream record comparisons.
Runtime build passes (3 rebuilt/39 fresh, 15.5s); final host is fresh (0.2s).
Pending fixed markers now compare through actual readLPw attachment as well as
readSettingsLPw. The native probe mirrors readLPw's second file read before
binding, with no user-file effects; the initial one-read probe diverged when a
group became visible between reads. Retained positive-userId reads now copy the existing shared relationship along
with its shared appId. Allocation status comes directly from the record reader
before retained metadata is copied, rather than inferring a new object from its
shared flag (#974). The reread probe checks no-change, ABX/text permission writes
and XML failure after a completed permission container; actual original Settings
retains the same PackageSetting object and updates the UID-owned group state.
The expanded actual-original reread variants pass in the full ART oracle (63.27s);
#974 is verified within this retained relationship/permission-destination scope.
Twenty-two controlled text/ABX fixtures now compare this binding adapter against actual Settings.readSettingsLPw
UID-owned LegacyPermissionState bytes and the active package fixed marker. They
cover prior/later shared-user records, a shared package targeting a non-shared UID,
nested items/unknown subtree skipping, partial permission retention after
file-error retry, repeated retained packages, rejected UID registration and
constructor state retained after invalid page-size input, and the same pending
package recreated from reserve after a corrupt main read. The complete original
ART oracle passes without skips (69.53s);
538 service units pass (5 excluded, 0.54s). Current runtime build passes
(3 rebuilt/39 fresh, 15.5s); the prior image/API/template build passed (85.7s).
Eight text/ABX install cases now compare the native event reader with the actual
original Settings method; the four legacy-restoration graph cases use that same
original install method. Eight runtime migration cases still use the ported loop.
A native truncated-subtree unit retains earlier grants for users 10 and 0, nested
items and the missing marker while excluding an unknown subtree. The first Java
fixture compile failed on duplicate helper signatures; the redundant install loop
was removed. Full original ART comparison passes without skips (69.53s), 532
service units pass (5 excluded, 0.50s), and the original API check/full image and
first-boot template build pass (9 rebuilt/33 fresh, 85.7s).
Its explicit active/factory/shared owner graph can now be assigned to SigningScan
with resolved users and captured by the existing Store. Missing/foreign owners,
invalid user inventories and later package/UID inventory mismatches reject without
replacing prior state. Older captures retain earlier permissions. Setting pages
carry the migration projection and Java rebuilds detached original
LegacyPermissionState objects; an unresolved marker makes the getter fail instead
of supplying an empty substitute. Detached Migration/State and Java permission
capture/restore now admit the exact INVALID_UID (-1) SettingBase identity (#896).
A non-shared APEX still owns real legacy permissions without an application UID;
negative live PermissionManager lookups remain rejected before Binder/provider
calls. The original ART oracle passes in 30.73s, including populated original
PackageSetting legacy state at appId -1, native/Java byte equality and restoration,
foreign IDs, lower negative IDs and trailing-frame rejection. The native verified
non-shared APEX owner captures a complete explicitly supplied package/shared
legacy inventory and retains the negative-ID projection. The first fixture omitted
policy-seeded shared owners and was rejected; it now supplies every controlled
owner rather than infer empty permissions. All 436 units pass (one ignored/not run),
all integration targets compile, and the original-image API check/full image and
template build pass in 73.0s (nine rebuilt, 33 fresh); final host rebuild passes in 16.7s
(three rebuilt, 39 fresh). The initial system scan
regression also passes in 67.78s. The real native Binder test now retains a
150,000-character legacy permission across a changed publication. Original ART
agrees on 16 text/ABX pinned reader-loop ports using original parsers/owners,
original SettingBase/PackageSetting copies,
empty/populated native captured getters and unresolved-owner rejection (16.17s
with the complete package oracle); device-services original API linkage passes
(14.2s). Native restoration now reads those migration owners from the original
saved files, preserving XML event-time UID visibility, separate active/factory
SettingBase state, install-permissions-fixed markers and per-user runtime
version/fingerprint. Modern runtime maps use the last duplicate named entry;
missing markers follow the saved internal SDK and shared role. Legacy fallback
selects AtomicFile backup only when the main file exists and retains the original
asynchronous rewrite request without writing permission files. Changed Settings
or scan identities reject atomically. Settings metadata restoration now includes
configured platform/OEM UID seeds and traverses the original non-consuming
attribute-only/deprecated XML branches (#859/#860). Original ART agrees on four
text/ABX Settings reader-loop ports with original PackageSetting/SharedUserSetting
owners, including fixed markers and complete initialized group inventories
(16.51s with the complete package oracle). Unit tests pass 389, with one ignored
integration test not run by that command; all integration targets compile and
original Java oracle linkage passes (3.72s); device-services builds against
original APIs (13.8s). The explicit saved-scan diagnostic boots original PMS on
disposable data, freezes it, restores migration owners using the actual image
SystemConfig, rescans 243 active/3 disabled packages and publishes the captured
Store successfully (393.16s). These are pinned reader-loop ports,
not direct calls of private original Settings methods.
Install-permissions-fixed now has a separate native current-state owner for each
active/factory setting. Saved restoration supplies the original consumed marker;
explicit imports must supply the whole setting inventory, and a new migration
import invalidates old bits until supplied again. Updates preserve older Store
captures and do not alter restoration provenance. Existing setting pages carry
known true/false or an unresolved marker; Java rejects missing/invalid markers
and its original-named getter rejects unresolved state. Original ART validates
both boolean values, original PackageSetting getter/copy semantics and absent,
invalid and unresolved transport markers (19.25s with the complete package
oracle); the real native Binder-driver capture test retains its old true value
across changed publication. Units pass 389 (3.16s; one ignored/not run), all
integration targets compile and device-services original API build passes
(18.7s).
A production CapturedPackageSetting constructor now assembles the captured
metadata into detached original concrete PackageSetting owners: identity/flags,
raw ABIs, times/version, update/query/restore bits, loading, domain/metadata,
install source/signing, keysets, SDK/static arrays, nullable MIME groups, retained
paths and migration permissions/fixed bits. Missing active domain or permission
ownership, wrong version/scope and setter values that cannot reproduce the
capture reject. Factory disk inputs use original DISABLED_ID when their metadata
has no domain UUID. Null and allocated-empty path sets stay distinct. The pinned
image removes setKeySetData, so construction populates its original keyset
object with the already validated pinned methods. Original ART compares concrete
getters, independent mutable replicas, active/factory metadata, legacy
permissions, null/empty paths and version/scope/unresolved rejection (18.72s
with the complete package oracle); full original Java oracle linkage passes
(3.72s), and device-services builds against original APIs (13.6s).
The captured scan AIDL now enumerates explicit sparse PackageSetting user IDs
(method 13). Unknown settings return null, initialized empty owners return an
empty array, and unresolved owners reject. Native Binder tests retain the old
inventory after publication, distinguish empty/unknown state, reject invalid
requests and refuse publication with missing loaded user ownership. A lease
fetches all explicit records, validates sorted distinct IDs and each
version/name/appId/user/scope, then constructs detached original
PackageUserStateImpl objects with scalar, nullable component/suspension, archive,
overlay and label/icon inputs. Absent users use the original default without
adding sparse entries. Original ART validates concrete getters, sparse ordering,
mutable replica isolation, duplicate/missing/foreign inventories, transport
failure and closed leases (16.36s with the complete package oracle). Units pass
389 (3.16s; one ignored/not run), all integration targets compile, all original
Java oracle linkage passes (4.19s) and device-services builds (14.4s).
Original library overlay getters return initialized WatchedArrayMap owners,
whose equality and sealed mutation contracts differ from ordinary Java maps;
the user replica now returns detached original snapshots for initialized maps,
including allocated-empty, and the original empty getter for uninitialized maps
(#863). ART compares original class/equality and sealed mutation rejection while
values remain detached. The pinned image removes bulk library/label setters;
assembly uses retained original mutations. Captured raw states that those APIs
cannot reproduce (including allocated-empty label owners) reject and remain
#862. PackageScanLease.newScannedSetting now combines active metadata and all
explicit users with freshly decoded original PackageImpl code, saved signing,
eight usage timestamps, complete base/override seInfo and finalized active library
dependency metadata from the same capture.
Code name, active UID, path and long version must match the setting before code
attachment; absent collected owners reject, while unknown settings return null.
Collected code signing remains distinct from saved setting signing. Original ART
checks combined concrete getters, detached mutable code/usage/user state,
missing code/signing/usage/seInfo/dependencies, UID/path/version mismatches and
closed leases. Dependency assembly additionally checks detached lists/objects,
empty-owner clearing, malformed/canonical-byte rejection, paged failures and retry
(16.91s with the complete package oracle). All original Java oracle linkage
passes (4.12s), and device-services builds against original APIs (14.9s).
The current PackageSetting leaving-shared-user bit now has a separate nullable
native owner (#861). Original disk-reader/new-setting construction initializes
false, copies retain the current value, and ScanPackageUtils enrichment explicitly
sets or clears it from accepted parsed code. Complete active/factory import
validates the owner inventory before mutation; unknown input remains unresolved.
It is transported as -1/0/1 and restored with the original setter; unresolved,
truncated and invalid markers reject. Disk persistence excludes this runtime bit
and does not alter the current owner. Native Binder pages retain the old true
value after a false publication. Original ART checks true/false concrete getters,
copied mutable owners and unknown/malformed rejection (16.71s full package oracle).
Units pass 392 (3.14s; one ignored/not run), all integration targets compile,
original Java linkage passes (4.02s), device-services builds (16.6s), and the
actual first-system image scan checks every accepted setting's enriched bit
(42.90s). Live original-state export/import wiring remains #861/#836.
The runtime flags owned by original PackageStateUnserialized now have a native
setting owner: hidden-until-installed, updated-system-app, APK-in-updated-APEX
and nullable APEX module name. Fresh settings use the original constructor's
false/null state, copies retain current values, and XML reads reconstruct fresh
owners; saved ApplicationInfo flags do not stand in for these fields. Complete
active/factory imports validate the inventory before any mutation. Scan completion
retains the current update bit for ABI/application metadata; factory enabling and
ex-system data rescanning explicitly clear it at the original owning stage.
Disabled-factory scans read the runtime owner rather than a saved flags bit.
A generated captured-snapshot method transports version/name/UID/factory and these
fields. Java leases retain immutable inputs and reject identity/scope/trailing,
invalid boolean, truncated and unaligned data. Original Parcel rounds unmarshalled
storage to words, so byte alignment is checked before unmarshalling. Native Binder
checks old-capture retention, absent names/factories, null/trailing requests and
closed leases. newScannedSetting restores the captured fields through original
PackageStateUnserialized setters after joining the existing code/signing/user/
usage/seInfo/library owners. Original ART checks 48 active/factory combinations
per package (all booleans and null/empty/populated APEX names), original getters,
exact bytes, original copy isolation, rejected UID/version/scope mutations,
malformed transport and detached combined settings with populated runtime fields.
The actual first-system image scan passes (42.08s), including retaining an update
and clearing it when its factory disappears. The full original package ART oracle
passes (18.99s), and original Java oracle linkage passes (3.91s). All 402 units
pass (3.56s; one ignored/not run), all integration targets compile (5.97s), and
the full boot build passes (66.0s; nine rebuilt nodes), including the original-PMS
userdata template. No owned runtime processes or disposable data mounts remain;
the reusable derived system image stays mounted read-only.
Captured settings expose a typed PackageStateReplica implementing the
pinned PackageStateInternal interface. Each lease retains one record per package
and suspension policy; its original parsed object and immutable user replicas
retain identity. Original getters supply scalar/ABI/page-size semantics from a
privately assembled setting. Returned arrays, MIME sets/maps and sparse user
containers are detached; mutable signing, install, permission, keyset, library
and transient owners are reconstructed from retained immutable inputs. Closing
the lease releases its caches without invalidating a caller's retained replica.
Hidden API enforcement is calculated by the native scan from captured code,
system ownership and the image SystemConfig allowlist, with the original
missing-code, platform-signing and system/non-SDK-request rules. Its generated
snapshot method rejects unknown/null names, trailing requests and closed leases;
the Java lease accepts only the original disabled/enabled policy values.
All 403 native units pass (3.16s; one ignored/not run), and all integration targets
compile. Original ART compares the active replica's scalar/array/collection
getters against the assembled original setting, retained identities, missing-user
defaults, later policy changes, mutable permission/keyset/usage/transient owners,
invalid policies and use after lease close (20.12s full package oracle).
Original Java linkage passes (4.33s); the actual first-system image scan passes
(44.90s), and the full boot build passes (60.0s), including the original-PMS
userdata template. No owned runtime processes or disposable data mounts remain.
Shared UID process aggregation now has a native owner ported from
SharedUserSetting.addProcesses/updateProcesses and ParsedProcessImpl.addStateFrom.
The value's process name selects the aggregate; the enclosing map key does not.
Denied permissions union in original ArraySet hash/collision order, class values
overwrite by package with ArrayMap ordering, and incoming memory/tagging/embedded
DEX modes replace the previous values. The original immutable empty class-map
failure rejects a native candidate before publication; null process names also
reject atomically. Complete captures require every known group and every active
member, including unparsed entries, in the owning operation's actual iteration
order. Both shared-UID membership and code/process inputs are retained; changed
inputs reject queries and snapshot publication until recomputed. Old captures
remain usable. Sorted package names do not substitute for original identity-set
iteration. The disposable original ART snapshot/usage/shared-owner oracle passes
(12.19s): exact process fields/ordering after additions, no-op input, incoming
updates, original exported reverse iteration, member removal, absent removal and
empty rebuild, plus the original immutable-empty-map error. The retained image
has no SharedUserSetting.getProcesses convenience method; the oracle accesses its
original declared processes field, checked against the image. All 407 units pass
(3.19s; one ignored/not run), including complete/duplicate/foreign inventories,
atomic rejection, parsed/unparsed distinctions and stale code/membership checks.
All integration targets compile (13.87s); original Java linkage passes (8.84s),
the actual first-system image scan passes (50.66s), and the final full build passes
(18.5s; three host nodes rebuilt, 39 fresh). The original-PMS userdata template
also rebuilt successfully in the preceding full build (61.9s). No owned runtime
processes or disposable test mounts remain.
Shared UID captures now transport the complete native group inventory and each
record's name, app ID, privilege, saved signing, active member names and seInfo
SDK projections through generated snapshot methods 18-20. Native records reject
membership differences against active settings. Java binds loaded active member
PackageStateReplica identities into the image's retained six-method SharedUserApi;
missing or foreign members reject construction. Returned ArraySet containers and
signing objects are detached, and replicas retain their inputs after lease close.
The pinned original SharedUserSetting snapshot copy omits seInfoTargetSdkVersion,
so its frozen getter is zero independently of the live boot-label SDK; both are
captured separately, as confirmed by original ART. The original package oracle
passes (28.12s): native record roundtrips, original frozen getters for populated
and empty groups, member identity, container isolation, unknown/missing owners,
short chunks, incorrect versions and use after close. All 408 native units pass
(3.21s; one ignored/not run), including real Binder inventory, bounded chunks,
unknown/null names and invalid ranges. All integration targets compile. Original
Java linkage passes (4.03s). The full build passes (68.8s; nine rebuilt nodes),
including the original-PMS userdata template. The native updateProcesses operation can now consume the original feed's forward
ArraySet member order after checking complete group membership, setting identity
and collected process/code inputs, then rebuild in original reverse traversal.
Failures retain the prior owner; captures invalidate on setting path/version or
collected-code changes. A typed com.android.server.pm bridge now directly exports
SharedUserSetting's current processes field from the same original unfiltered
snapshot, preserving incremental history, ArrayMap/ArraySet ordering, nullable
class values, denied permissions, all three modes and embedded-dex state. Native
record decoding and batch publication reject malformed frames, duplicate keys,
wrong group identities/order and incomplete aggregate inventories. Native import
validates member/code ownership and retains the original aggregate independently
of reverse rebuild; validation no longer reconstructs a captured incremental owner.
All 418 units pass (3.23s; one ignored/not run). Original ART exports actual
aggregates at empty, incremental add, noop, rebuild, removal and cleared phases;
native decoded values equal the original owner at every phase (13.88s full facade
oracle). All integration targets compile; the full image/original-PMS template
build passes (73.5s; nine rebuilt nodes, final host rebuild 12.7s). A disposable
original-PMS template window boot reaches boot_completed at 5.022s and publishes
68 generations without feed errors, with all 16 shared process aggregates and 290
runtime records (285 active, five factory). No owned runtime processes or probe
mounts remain. Runtime daemon wiring, broader shared owners and full snapshot map
publication remain #870/#836. Native PackageImpl cache decoding/encoding now rejects null feature
flag arrays, null feature flag strings and null ParsedProcess value names, matching
the original constructors (#871/#872). Snapshot validation applies the same
required-field checks to active/factory loaded code before publication; an invalid
candidate retains the previous capture. Allocated-empty feature arrays, canonical
true/false/unknown flag states, empty process names and nullable enclosing map keys
remain distinct and preserve exact original read/write bytes. Controlled fixture
packages now supply the original constructor's allocated-empty feature state;
production code does not replace missing inputs with defaults. All 408 units pass
(3.14s; one ignored/not run), including malformed cache decoding, encoding rejection
and publication atomicity. The full original package ART oracle passes (24.10s):
three invalid constructor inputs reject with the original NullPointerException,
while three valid owner forms reproduce their complete cache bytes. All integration
targets compile (16.22s), original Java linkage passes (7.03s), the actual first-system
image scan passes (51.39s), and the full build passes (22.1s; three host nodes rebuilt,
39 fresh, including the retained original-PMS template). No owned runtime processes
or disposable test mounts remain. Native PMS activation and acceptance remain
unproved.
PackageStateInternal assembly now covers active/factory settings and genuine
unloaded code through explicit scoped runtime captures (#873). The native owner
accepts the complete setting inventory atomically, retaining usage, nullable base/
override seInfo, library objects and nullable file slots separately per scope.
Identity/code changes or disagreement with assigned active usage/seInfo/libraries
reject publication until recaptured. Generated snapshot methods 21/22 page these
records with an explicit code-presence bit; Java rejects a missing code record
for a loaded setting, restores actual runtime inputs through retained original
setters, and caches replicas by name, scope and suspension policy. The image has
no array-wide usage setter; its retained reason setters restore all eight values.
A complete sparse user import now takes original factory-to-active aliases
explicitly, rejects partial/foreign/negative inventories or differing alias
values, and preserves independent factory states while updating actual aliases.
The Java lease assembles complete active, factory and shared maps into
PackageSnapshots.Data only after every listed owner resolves; duplicate/null names,
missing records and shared members outside the active inventory reject the map.
Original ART verifies unloaded active/factory getters against independently
populated original runtime owners, distinct usage/version/SDK/user scopes, null
seInfo, nullable library-file slots, detached usage, full immutable maps, malformed
inventories, short chunks, version/scope mismatches and use after lease close
(20.49s full package oracle). All 411 units pass (3.13s; one ignored/not run),
including atomic import/stale publication, explicit user aliases and real Binder
paging of large factory records retained across a newer publication. All integration
targets compile, original Java linkage passes (4.47s), the actual first-system
image scan passes (42.62s), and the full build passes (60.6s; ten rebuilt nodes),
including the original-PMS userdata template. The running original PackageFeed
now exports separate active/factory runtime records from the same unfiltered
snapshot as its package/code records. Each carries identity, code presence, raw
base/override seInfo, all eight usage values, exact original library owner parcels,
nullable file slots and transient flags. Native decoding rejects malformed counts,
booleans and tails, and complete-batch assembly requires matching package/code
identity, getter values and every setting scope before publishing. A native import
matches the scan's path/UID/version/code scope, imports runtime and transient owners
atomically, and retains the prior owner on partial/foreign sources. Original ART
exports populated loaded and unloaded active/factory owners; Rust reads the actual
bytes, compares complete runtime values and imports them into a validated native
capture (19.00s full package oracle). All 413 units pass (3.17s; one ignored/not run),
including full runtime batch completeness, code/scope mismatches and atomic import.
All integration targets compile, original Java linkage passes (5.11s), and the
full build passes (67.9s, including the original-PMS template; final host rebuild
13.2s). A 60-second disposable template-backed window boot with package shadowing
reaches sys.boot_completed=1 at 4.888s and publishes 66 feed generations without
feed errors; the final inventory has 290 runtime owners (285 active, five factory).
An earlier probe excluding bpfloader never received the feed because netd repeatedly
aborted and restarted zygote; the default boot configuration succeeds. No owned
runtime processes or probe mounts remain. Native boot runtime assembly now joins
completed seInfo/dependency owners with the usage owner, preserving all eight
reasons, raw base/override labels, nullable file slots and original library records.
Factory and unloaded scopes require their own retained identity-bearing inputs;
missing, extra, foreign and usage-inconsistent sources reject before any owner
changes. The actual image scan publishes this completed runtime through Store (45.04s);
the original ART replica oracle uses the same boot assembler (21.81s). All 414
units pass (3.39s; one ignored/not run), and the host/image build passes (18.6s;
three rebuilt, 39 fresh). Running daemon bootstrap wiring, runtime copy/update
ownership, complete live user value export, native visibility and publication into
running SystemServer remain #873/#862/#836. The unfiltered original snapshot now
exports every active/factory sparse user ID and actual factory-to-active object
alias (#862). Native import binds that provenance to complete existing user
values with exact package/path/UID/version and user inventories; partial, foreign
or inconsistent sources reject atomically. The original ART oracle verifies
sealed snapshots retain actual aliases and distinguishes equal independent
objects (22.42s). All 421 units pass (3.47s; one ignored/not run), Java oracle
linkage passes (4.19s, including the shared-process producer dependency fix
#875), and the full image/template build passes (85.1s). A disposable default
window boot reaches sys.boot_completed at 6.729s and generation 74 contains
290 user scopes, zero factory aliases, 16 shared-process aggregates and 290
runtime owners, with no feed errors or remaining owned processes/probe mounts.
This source feed does not export full raw user values or component label maps
(#862), and native daemon bootstrap remains unwired. Legacy domain migration
inputs now stay in the restrictions reader separately from PackageUserState
values (#876); the resolver reads that separate inventory. They no longer
participate in user alias equality/copying or user-state snapshot transport.
Aliases between identical user states with different retained migration inputs
accept and update without changing those inputs. All 422 units pass (5.72s;
one ignored/not run), original ART verifies the revised captured-user transport
and original user-state assembly (19.75s), and the full image/template build
passes (59.9s; nine rebuilt, 33 fresh). Nullable library file slots now retain their distinction through the query feed,
native dependency owners and ApplicationInfo parcels (#874). Null, empty strings,
paths and repeated null slots survive original ART decoding (19.38s package
oracle); provider dependency deduplication retains the first null occurrence.
All 414 units pass (3.50s; one ignored/not run), all integration targets compile,
the actual first-system image scan passes (42.78s), and the host/image build
passes (16.5s; three rebuilt, 39 fresh). Other live query/write differences remain #865-#868. Live APEX/
hidden-state delivery, permission boot ordering, original writer requests,
callbacks and full PackageState/SharedUserApi export remain #858/#836. The native
service switch is not activated and original PMS remains active.
The native usage owner now reads PackageUsage's original v0/v1
package-usage.list formats for known package settings, preserving each of the
8 reason timestamps and the original partial-update behavior on malformed rows.
Missing files mark historical usage unavailable; an empty file does not, and
legacy AtomicFile backups take precedence. Latest/foreground time, invalid reason
handling and omission of zero/nonpositive-only usage rows match the original.
The disposable original-runtime snapshot/usage oracle passes (14.39s): original
PackageUsage reads native v1 bytes and legacy/malformed/missing inputs, then its
v1 output is read natively with exact timestamp equality. The oracle overrides
only its file location with a disposable file; it does not write PMS's live
usage file. Native scan captures now require this usage owner at construction
and publication, validate exact active package membership and retain the same
version for scan/code and timestamps. Caller mutations cannot alter old captures;
stale or invalid usage publication preserves the current graph. The original
first-system scan passes (43.24s), including old/new timestamp isolation; all
350 units pass, and all integration tests compile. The saved-scan fixture now
loads its real disposable usage file before capture; that updated fixture has
not been run in this change. IPackageScanSnapshot now also transports a usage
envelope with version/name, historical availability and all eight timestamps.
PackageScanLease caches immutable PackageUsageState objects only after complete
version/name/trailing-data validation. PackageObjects binds the envelope to an
original PackageSetting before restoring it through the image's reason-specific
setters; detached arrays cannot mutate either owner. Native Binder verification
passes (0.03s), preserving old timestamps after a new scan version is published,
with absent-name/null-name/trailing-data checks. The 572-parcel original runtime
passes (21.36s), including generated usage Proxy/Stub framing, owner failure and
retry, trailing-data/version/name rejection, DTO identity/input isolation and
exact native/Java bytes. Original usage getters match the transported values,
and rejected restoration preserves the original state. All 350 units pass
(3.17s), all oracle classes link (2.83s), and device-services builds (14.2s).
The compile-only PackageUsage declaration now matches its original
AbstractStatsBase inheritance, also checked by the production build. These are
separate native-driver and original-Java tests; the bootstrap has not exported
the endpoint into SystemServer, and full PackageState replicas, live
notifyPackageUse and persistence scheduling remain #836/#798.
The native seInfo policy owner now reads the original platform and optional
system_ext/product/vendor/odm MAC permission files, validates signer stanzas and
matches current or rotated collected certificates with package-specific rules
before global rules. Label composition preserves privilege/target SDK/partition
suffixes and partition precedence. Compatibility decisions and a nonempty
shared UID's boot-fixed target SDK are explicit caller inputs, not inferred
from missing metadata. Ordering now follows the full pinned Java TimSort
comparison sequence, including run-stack collapse, low/high merges and adaptive
galloping. Duplicate detection occurs on compared pairs, matching the original
rather than rejecting every repeated selector pair. The prior 32-rule limit is
removed; large policy sets preserve specificity and acceptance. All 353 regular
units pass (3.13s), with the input-dependent TimSort oracle separately executed:
it passes (12.59s), comparing exact sorted indices and every comparator pair for
128 original-libcore inputs of 0–4096 elements, using binary policy keys and
17-way keys. Shared fixture cleanup confirms owned process/mount release. The
native TimSort port retains GPLv2 with the Classpath exception and its upstream
notices/provenance; independent native code remains under its existing terms.
The pinned image's seven rules load successfully, including its repeated
platform/vendor global selector; no partial policy is published on errors. The 572-parcel original-runtime oracle
passes (19.59s), comparing policy load success and assignments for actual
native-collected framework/GSF objects with original SELinuxMMAC. Oracle classes
link against the original image (2.94s); device-services remains fresh. The
original utility's static-only compile declaration is fixture-local, and every
actual fixture reference is checked against the image before boot. This policy
has a native boot-assignment phase but has not populated Java PackageState
replicas. Native shared
UID ownership now retains the original seInfo target SDK lifetime: initial
CUR_DEVELOPMENT, first parsed-member admission, boot minimum over actual active
code, and no recomputation on runtime additions/removals. An explicit boot
finalization rejects pending scan metadata before mutation; saved-only settings
and disabled code cannot substitute for active parsed packages. Captures retain
their own SDK values. All 355 regular units pass (3.12s); an original
SharedUserSetting oracle verifies admission, boot, removal, empty-group reuse
and reboot behavior (25.52s with the snapshot/usage oracles). The actual first
system scan verifies every group's active-code minimum (42.66s), and
device-services builds with the original API declarations (14.6s). Full boot
orchestration must call finalization after data selection as well as system
scanning. The synchronous package bootstrap bridge now exposes non-shared
seInfo target-SDK decisions as appended AIDL method 4. It reconstructs original
parsed code from the native cache, generates original ApplicationInfo without
state, and asks original PlatformCompat for latest/R changes in original order;
shared UID callers retain their own boot-fixed SDK. The native client reports
cache encoding, transport and original-owner failures separately. Its driver
test verifies cache framing, the returned SDK and owner-denial propagation
(0.03s); all 355 regular units pass (3.12s), all oracle references link (2.86s),
and device-services passes production image API checking (13.8s). This new
method has not been called by actual SystemServer bootstrap, nor compared with
live original compatibility decisions. Native `assign_seinfo_at_boot` now
finalizes shared SDKs on a detached candidate and composes every loaded active
package's label from its collected signing, original setting privilege and
partition flags, and either the shared SDK or an explicit compatibility-owner
result. All queries complete before changing SDKs or labels. Absent assignments
return an explicit unfinished-phase error; only the caller's explicit unread
policy uses the original default label. Assignments retain their input graph;
changed code, UID membership or setting flags reject reads/publication until
the owning phase resolves them. Old captures retain their labels after a new
version is published. All 356 units pass (3.10s), checking failure atomicity,
shared compatibility precedence, partition precedence, missing assignments,
stale-input rejection and capture isolation. The original SharedUserSetting
oracle also verifies boot label agreement and the lower-SDK adjustment
(15.64s with snapshot/usage oracles). These are boot assignments only:
live compatibility invocation and complete boot orchestration remain
#838/#836/#798. Captures without this phase
remain incomplete C state. The private snapshot AIDL now appends method 7 for
the captured version/name and nullable base/override fields. Its private payload
format is migrated on the C branch together with the Java decoder.
Missing assignments return an explicit exception, unknown active code returns
null, and null names/trailing arguments are rejected. The native Binder unit
checks old-label retention after newer publication and envelope framing. Java
PackageSeInfoState is immutable; the lease validates version/name/trailing data
before caching, and closed leases reject reads. PackageObjects restores a
shared boot label through original setOverrideSeInfo (retaining the base) and
an ordinary label through original setSeInfo, after name/version validation.
Its complete restoreSeInfo requires an assigned base and restores both fields,
including an explicit null override to clear old state; a missing base or
wrong name/version fails before changing either original field.
All 356 units pass (3.72s), and the 572-parcel original-runtime oracle passes
(20.14s), including exact native/Java bytes, generated Proxy/Stub framing,
owner-error retry, invalid envelope/version/name rejection, lease identity,
original base/override/effective getters and rejected restoration isolation.
That oracle uses explicit target SDK 36 for its compatibility input; it does
not prove live PlatformCompat decisions. All oracle references link (3.17s),
and device-services builds with checked original setters/getters (14.7s).
These remain separate native-driver and controlled original-Java endpoint
tests. Native seInfo state now separates nullable base and override fields.
The per-scan assignment API copies a retained transient state's override while
recomputing its base, or explicitly starts fresh transient state for a new
setting/initial disk restore. It does not run the shared boot SDK minimum or
relabel other members. UID ownership changes cannot masquerade as a retained
state. Group privilege/SDK changes alone do not invalidate an old package's
owned labels; code, package flags and membership still require the owning scan
phase. Compatibility failure retains previous state. Boot finalization preserves
an already assigned base and sets shared overrides. A boot-only override's
missing base remains explicit. Transport retains both transient fields;
effective selection uses a nonempty override before the base, as the original
does. The original replica oracle verifies restoring complete fields, clearing
a stale override, rejecting an incomplete base without mutation, and preserving
both fields on rejected name/version restoration. The fixture scans establish
base assignments before boot overrides, using explicit compatibility target
SDK 36; this still does not prove live compatibility decisions. Final oracle
linkage passes (3.29s).
All 356 units pass (3.14s), including runtime lower-SDK update, retained override,
fresh replacement, subsequent boot minimum, owner-error atomicity and captured
value isolation. Original PackageSetting copy/base/override/effective getter
behavior passes in the shared-UID runtime oracle (14.83s with snapshot/usage
oracles). The accepted scan completion now requires explicit seInfo policy and
compatibility owners and assigns the base before publishing loaded code. System,
saved and data-image inputs propagate these owners. Retained UID/shared ownership
preserves the transient override; fresh/disk-restored settings start without it.
A previously loaded object missing its assignment rejects completion instead of
being treated as fresh state. Only the target must have finished metadata during
an individual scan; capture and boot finalization still reject any pending scans.
The actual original-image first-system scan passes (43.91s), including a
nonshared retained re-scan whose compatibility query fails exactly once and
leaves all owner state unchanged, plus complete base assignments for loaded code.
The original runtime round-trip passes (15.67s), comparing all 288 discovered
cache entries and enriched variants plus two native scan objects (578 parcels).
Boot-generated cache counts no longer impose a stale 285-entry assumption;
framework/GSF coverage and exact returned count/decoded-field checks remain (#839).
The parcel fixture now obtains base labels from the real scan completion, using
its explicit compatibility target SDK 36, before shared boot overrides. Native
Bridge implements the scan compatibility interface and rejects trailing SDK
reply data. The protocol test checks this owner adapter and malformed responses.
All 356 regular native units pass (3.14s). The disposable original-PMS saved
scan passes (374.26s): all 243 active APKs and three disabled factories retain
their accepted code; every active scan owns an assigned base without premature
boot overrides, and the captured version preserves those fields. Saved system,
data update and first-system selection paths complete; owned processes/mounts
are cleaned. All integration targets compile.
These fixtures use controlled compatibility inputs and do not prove live
PlatformCompat decisions. Actual native boot orchestration, bootstrap export and
complete Java PackageState replica construction remain unwired
(#838/#836/#798).
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
and complete image/data version selection and publication remain
unimplemented (#702). This does not activate the native scan.
`scan::Image` supplies the first-boot image inputs without settings:
overlay directories in reverse partition order, the framework, then each
partition's priv-app/app directories and active APEX directories in the
owner's reported order. Records retain the partition, privilege and
factory/changed APEX origin; duplicate package names are preserved for
reconciliation. An APK inside an APEX retains its origin partition and
PARSE_APK_IN_APEX without being classified as the APEX package itself (#880).
Only explicit APEX-package scan policy sets that classification. A regression
checks both setting and clearing it; the original ART PackageImpl/applyPolicy
oracle confirms the same distinction (28.56s). The 432 native unit tests pass
(3.82s; one integration test ignored and not run by that command). Unsupported parser behavior and signature failures abort
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
`SystemImageScan::first_boot` now consumes that physical image order through
native scan ownership: shared-UID privilege and manifest/library policy,
constructor library/MIME/domain inputs, UID/signing reconciliation, bundled ABI,
page policy, actual code time and final application flags. Native APEX registration
now supplies its Settings and loaded code from verified original containers (#888);
its notification owner must succeed before invoking the deferred APK directory
loader (#892). Containers retain
INVALID_UID and reserve no APK slots. The
original platform is available to policy only after its scan completes. Required
framework booleans resolve through the image's static overlays and reject missing
or non-boolean resources. SystemConfig now reads initial-package-state exemptions
with the original partition-independent and Boolean.parseBoolean semantics;
initial stopped state requires an enabled/exported launcher-category activity
and excludes the platform and static overlays (#813). User/clock/domain inputs
remain supplied by their owners, not guessed from a package feed.
A disposable original-PMS comparison (2026-10-03) matches the boot-classpath
policy, image stopped-state boolean and complete initial exception set. With no
saved APK settings, the native phase completes all 243 image system APKs in
physical order and preserves the five no-APK directory rejections. Final public/
private flags, version, target SDK, file time, update hash and current signing
certificates match all 240 original packages at unchanged code paths. The saved
scan still matches 243 active/3 disabled packages, 16 groups and 23 libraries;
the whole comparison passes (376.52s), with unchanged disposable disk state and
owned process/mount cleanup. A separate original-APK fixture passes (22.28s),
including reserved APEX UID preservation, invalid seed rejection, domain-owner
failure, missing framework/image policy and unavailable pre-platform privilege
policy. `SigningScan::scan_existing` also stages retained-UID reconciliation
through all metadata gates before committing the scan owner. The fixture
checks whole-owner rollback on an actual code-time read failure, then successful
domain-setting replacement with unchanged UID, users, libraries and shared
membership. Files copied before rejection still require install-owner cleanup;
this transaction does not select image/data versions or persist settings.
`scan_disabled_system` refreshes a retained factory setting through native
ABI/page/time/application metadata without active signer reconciliation, library
registration or shared-UID admission. It uses the original factory scan's -1
clock and saved updated-system state, and preserves saved signatures. In a
fixture with original signed framework/GSF code and synthetic updated-data
settings, read failure preserves the whole owner, and successful cold refresh
changes the disabled setting and its own users. `scan_updated_system` follows that refresh with
the original changed-path and strictly-newer-version/shared-UID source decision.
SystemConfig reads require-strict-signature without a partition gate and ignores
only empty package names. Configured factory signatures refresh from verified
code only when data is selected; the fixture checks higher/equal/lower data
versions, unchanged paths, stale factory certificates and their effect on the
later native data authorization gate. A restore outcome requires cleanup,
enabling the factory setting and active scanning; it does not change the active
package. This is not yet an original disabled-scan oracle or complete boot
selection integration. The disposable comparison also matches the complete
strict-signature configuration against original SystemConfig and selects the
original data source for WebView, Chrome and versioned Trichrome, preserving all
active settings, identities and libraries. It passes raw Image code to identity
selection, rather than already renamed saved Record input. The scan owner now
initializes restored disabled user states independently, as the original XML
reader does. Cold factory scans use their own empty state and -1 clock, producing
first-install/update time -1 without changing active restrictions. A live copy
requires a matching completed factory and retains aliases to its existing users;
successful active scans update those aliases, while later user insertions or
removed/recreated states are not shared. The original PackageSetting constructor
and shallow-copy APIs confirm default time 0 and shared changes (123 to 456);
the native original-APK fixture checks retained live times, alias updates, cold
isolation, stale copy rejection and rollback. The native settings owner now ports enableSystemPackageLPw: same-UID active
settings retain code path, domain ID, signers, user restrictions and unselected
fields, while factory ABI/version/library/MIME/install metadata is copied. A
fresh setting reserves the exact UID with constructor defaults and a new
domain ID. Duplicate active IDs or occupied slots retain the first owner and
still remove the disabled setting, matching addPackageLPw failure semantics.
Public flags are preserved at this phase; the later active scan computes the
updated-system flag. Complete disable transitions, user mutation integration
and native reboot/template proof remain #815/#702/#798.
New-system scan entry points now remove a stale disabled factory setting when
its active data setting is absent, before UID/signature/metadata preparation.
Its users are removed too, while the shared UID survives until normal pruning.
Like the original initial scan, that recovery remains committed if the new scan
fails. The original-APK fixture verifies failed-scan cleanup followed by a
successful fresh scan at the retained shared UID, with the updated-system bit
clear. Active settings and eligible original-name adoption prevent stale
removal; new-package allocation rejects an eligible adoption until the caller
uses its existing-setting path (#804). Data locations reject without mutation.
The resource owner now serializes ordinary /data/app cleanup: directories use
generated IInstalld.rmPackageDir calls, including a random ~~ parent and its
matching parser cache entries; monolithic files are removed from the checked
writable data root. It rejects traversal, symlinks, escaped cache roots and
unimplemented incremental storage before deletion. Directory failures retain
pending cleanup for retry after the child disappears. Factory restoration
checks both selected factory and active settings before deletion, and enables
the factory setting only after cleanup succeeds. The original signed
framework/GSF fixture actually deletes a disposable monolithic update and
completes the active factory rescan, preserving UID/users and clearing the
updated-system bit. Stale selection and incremental errors preserve code and
settings. `complete_updated_system_boot` now consumes that selection and raw
image code, checks active/factory snapshots and identity/path/version/signers,
then either retains data without an active image candidate or runs cleanup,
domain-ID allocation, setting enable, fresh non-update manifest/library policy
and complete active metadata scanning. ABI inputs reference the newly enabled
setting rather than the stale data candidate. The fixture also checks that
retaining data allocates no domain ID, incremental rejection allocates none,
mismatched raw code rejects before deletion, and post-cleanup scan failure
retains deletion/enabled settings without committing candidate metadata. The system-image loop is now shared by first boot and restored settings.
scan_saved_system_image retains a mutable caller-owned scan, handles existing
system packages and eligible original-name adoption, routes updated factories
through source completion, and returns retained factory records plus their raw verified code for data
scanning/fallback. Missing-data factory settings are removed before
policy/metadata preparation. Initial scans reject already admitted package
identities (#820): a rejected system duplicate keeps the earlier package and
leaves image code untouched. The compressed-sibling fixture now verifies that
its first accepted stub/ABI state survives the later duplicate's rejection,
rather than claiming the original admits both. The disabled-factory request
retains the original null-oldPkg semantics. The original-APK fixture also checks
repeated boot with preserved settings/users, data version retention, actual
factory restoration, missing-data recovery and non-system promotion rejection.
The whole original saved-image comparison completes 240 active system APKs
and retains the three original data updates, preserving UID/path/version/users
and the retained active records. Its resource owner is idle for those retained
data selections; the separate fixture proves actual recovery deletion. This
validates the saved system phase, not complete native boot. Native DataImage now
collects /data/app and explicitly supplied mounted private-volume app roots,
filters installation stages, descends single-child containers with the native
parser and verifies signatures. Data origins carry no system parse/policy flags;
accepted code locations use the parsed inner path. DataCode also retains the
outer scan path for accepted candidates; rejected inputs retain that path too
(#818). Invalid volume names, duplicate volumes and
unmapped/unreadable directories reject explicitly. Missing directories are
empty inventories. Parser descent propagates directory-entry errors and rejects
lossy guest path conversion (#817). All nine original-APK scan-input tests pass
(6.21s), including unchanged signed GSF code in data and private-volume fixtures,
container descent, stage filtering, parse rejection without deletion, missing
roots and directory failures. The 301 units pass (3.04s); the final APEX-origin
validation passes both focused image tests. A non-UTF8 filesystem fixture could
not be created because host APFS rejects that name; no such runtime test is
claimed. scan_known_data now admits an existing data APK through canonical physical
location and known-setting/path checks, with InitAppsHelper's explicit
expectingBetter exception. It inherits system/partition privileges from the
current refreshed factory record, rejects mismatched factory metadata and
combines manifest/library policy, signer/UID reconciliation and complete scan
metadata. ABI saved-setting/system/update inputs are rebound to current
ownership. Unknown candidates, unexpected paths, wrong signers and stale
factory records leave candidate settings unchanged. The unchanged signed APK
fixture passes (41.45s), including the expectingBetter path exception and
ordinary data scans without system/update flags; all 301 units pass (3.04s).
The full disposable original-PMS comparison (374.43s) now completes the saved
system phase's 240 APKs and all three selected data updates (WebView, Chrome and
TrichromeLibrary) through physical DataImage inventory and scan_data_image.
UID, code path, long version, public/private flags, actual code file time and
user states match the original for the three data candidates; no copies,
rejections or factory recovery are needed in this unchanged-image case. The
same run verifies 243 first-system APKs, 16 shared UID groups and 23 libraries.
The owned data/image are cleaned after the run. This does not register native
PMS or exercise native cleanup against the original installd. scan_data_candidate now removes actual code at
the preserved scan path after known-path, manifest validation, authorization or
ABI-selection rejection. Other failures stop without deletion. Stale factory,
missing user ownership, policy inputs and domain-ID failures are fatal (#819).
The APK adapter retains typed input failures separately from verifier rejection;
a missing base/split read mapping therefore aborts inventory rather than
returning a deletable rejection. The signed-APK fixture verifies actual
monolithic disposable cleanup, incremental cleanup failure/retry, preservation
on user/domain/factory failures and successful candidate completion. Parse and
signature rejections have a separate cleanup boundary; the Binder filesystem
endpoint test verifies fatal parent failure, retained pending cleanup and
parent-only retry. scan_data_image now iterates data admission/cleanup and recovers retained
factories missing from the admitted package set. It derives expectingBetter
names from retained factories, merging explicit owner entries (#821). Fallback
validates raw factory identity/path/version/signers and the current disabled
setting, enables it, and reuses fresh non-update manifest/library and complete
active metadata scanning. It makes no extra data-code deletion. Earlier accepted
packages and resource effects survive a later failure. The signed-APK fixture
proves accepted updates, changed-path admission with UID/users preserved,
missing inventory recovery, wrong-identity deletion followed by recovery,
duplicate data deletion preserving the first accepted package, and preservation
of earlier admission on a later policy-owner failure. Surviving data updates
whose system factory disappeared now follow cleanupDisabledPackageSettings:
initial admission inherits the restored disabled setting, then that setting is
removed and the loaded scan/declarations are withdrawn before a fresh ordinary
data rescan. UID, code path and user state are retained; system/update and
partition flags are recomputed, with the original shared-UID privilege exception
still applied. A failed rescan preserves disabled-setting removal and withdrawal.
The signed fixture verifies demotion and this failure order. An absent update
removes the disabled entry and now reaches actual per-user installd.destroyAppData
requests for CE/DE/external storage, using saved CE inodes (default zero for a
user without a state). The boot owner must supply the complete resolved user
inventory and saved inode state before deletion starts. Installer failure stops
the scan and preserves earlier user deletions. After successful storage deletion
the scan clears active/restored domain state (retaining legacy migration state),
then retires the package's signing/alias keyset references. Shared sets and public
keys remain until their last owner disappears; upgrade aliases add no separate
references and last-issued IDs never rewind (#823). KeySetData now restores
KEYSET_UNASSIGNED=-1, including the original unversioned-settings reset. A keyset
owner failure leaves the earlier domain removal in place. The scan then stops at
unconnected filter/preferred/keystore and setting/permission cleanup stages
(#798/#822). Between keyset retirement and that boundary, the scan now removes
the deleted provider from native update-ownership denylist relations, preserving
other providers for overlapping targets. Repeated additions accumulate, matching
UpdateOwnershipHelper; empty additions do not withdraw old contributions. Accepted
metadata queues eligible system/update providers by the pinned property and
INSTALL_PACKAGES/INSTALL_PACKAGE_UPDATES declaration. The queue retains commit
order and repeated provider posts, capturing each post's parsed package rather
than looking it up after later scans. complete_next_apk_read consumes only the
captured head; out-of-order reads are rejected before resource access, and
failed reads keep that head and all later posts intact. A completed empty read
retires one post without withdrawing prior contributions. The compiled resource
oracle now posts all reads through the actual original HandlerThread before
releasing its queue, verifying FIFO output and worker shutdown (16.19s); native
fixtures retain both repeated provider posts and their accumulating contents.
The added compile-only HandlerThread API passes image linkage (13.6s).
Pending provider reads make
denylisted/provider queries fail explicitly until the resource owner supplies
validated contents. The original UpdateOwnershipHelper oracle
compares seven add/overlap/accumulate/empty/remove/repeated-remove/last-remove
transitions with the native owner: denied-target and provider/null queries match.
The scan's native setting owner now implements removePackageAndAppIdLPw's
setting/UID stage (#822): it rejects removal while code is still loaded or UID
ownership disagrees, then removes the active setting and installer status.
Independent UIDs are released; shared members are removed with their flag
recomputation, and the shared group/UID survives any active or disabled factory
member. Unrelated UID owners and disabled settings remain intact. Installer names
are restored from installer/initiator/originator roles, excluding update-owner-only
references. Removing a registered installer clears remaining active packages'
installer/originator/update-owner references and attribution, retaining the
initiator name/signatures with its uninstalled flag and marking installer
orphans. Original Settings compares independent/shared/disabled-reserved UID
removal, repeated missing removal and installer source effects (19.35s). All 338
units pass (3.10s), and image API linkage passes (13.1s). This in-memory stage is
not yet called by boot cleanup: full side-owner ordering, permission uninstall
reconciliation and publication remain #822/#798. The native store now persists
that completed setting removal and its installer-source effects, rejecting
unrelated metadata changes before writing. A last shared-UID group is removed
only without an active or disabled factory member; unknown XML stays intact.
Certificate references are materialized before deleting their defining owner,
including retained packages, past signers and install-initiator signatures.
The writer accepts both restored and retained live installer-registry history
without permitting reassignment. User restriction entries have a separate
resilient removal commit after global settings commit; other users, future XML,
permissions and preferred choices remain with their respective owners.
Unit tests cover reopening, reserve copies, shared UID reservation, external
writer rejection and unrelated-state rejection. Original PackageSignatures
reads native-written current/past/initiator certificates in the combined fixture
(19.35s). All 338 service units pass (3.10s). This verifies saved-setting deletion
and original signature parsing, not a full native uninstall or original-PMS
reboot after deletion; full package-list/permission integration and graph
publication remain #798/#822. Store.commit_package_list now writes the final
loaded non-APEX inventory supplied by the graph owner, with all active-user
GIDs supplied by the permission owner. It validates setting UID/version and
unique whitespace-free row fields, preserving GID order and duplicates.
PACKAGE_INFO_GID comes from the original image's Process constant. The journal
writer refuses an external writer, recovers a temporary-only old list before
replacement, fsyncs the new file with guest uid/gid and 0640 metadata, atomically
renames it and reports/cleans failures. Partial and first-write failures preserve
the old or empty-placeholder main file and support retry. Original JournaledFile
reads, rolls back and commits native-formatted rows in the combined fixture;
this is journal/format evidence, not native libpackagelistparser coverage or
proof of complete permission-GID generation. The compiled SystemServer bridge
now exposes the original PermissionManagerServiceInternal/LegacyPermissionDataProvider
getGidsForUid call for system-UID clients, rejecting negative UIDs and an absent
owner. The native GID client validates supplied active-user IDs, queries
full UIDs in supplied order and preserves every returned GID, including repeats.
Null/negative replies and owner/transport failures are explicit errors. The store
can resolve all rows through this client before a package-list write; any failed
query leaves the old file/state intact and never returns a partial inventory.
The three Binder resource-client tests independently encode UID/int-array wire
fields and cover invalid inputs, user order, duplicates, empty legitimate replies,
owner failure and final file persistence. New image API linkage passes (13.0s).
This is native transport and compile/link evidence: an actual DeviceServices
GID transaction in a booted image and early facade bridge exposure are not yet
verified (#798/#808/#822). The scan/store also remove only a
renamed-package real-name key, preserving mappings whose values name the same
old package, after permission uninstall/shared-UID conversion. Named, absent,
null and repeated map removals match original Settings; disk reopening preserves
the native change and unrelated XML. All 338 units pass (3.10s), the combined
original fixture passes (19.35s), and new image APIs link (14.0s). These stages
are not yet orchestrated by boot deletion (#798/#822). All eight Java-oracle DEX
build paths now check references against the original boot classpath and the
client's explicit services.jar before starting a guest (#830). A separate
all-oracle compilation/linkage test passes (2.83s), and a JDK API regression
accepts original readAllBytes but rejects unavailable readString(Path) without
launching the runtime (1.96s). All six original-runtime fixture paths pass through
preflight: combined manifest/keyset/removal/keystore (19.31s), UID allocation
(148.77s), new settings (83.96s), public keys/signing (15.12s), resource XML
(16.94s), and saved package scan (379.44s). Their owned process/mount cleanup
checks pass. The failed oracle previously exposed original crash
reporting's Binder EBADF, still unresolved (#831).
Native ComputerEngine-style visibility handles a sandbox's client before
code, installed/archived and instant-app checks (#940), as the pinned original
does. The target UID must match the sandbox's client UID in the requested user;
a missing target is filtered. The original feed also captures PMS's selected
SDK sandbox package through its pinned IPackageManager API (#941), with a
singleton nullable record. Query snapshots distinguish an uncaptured owner from
an original null selection; sandbox same-app checks compare the selected name
without requiring a parsed package or per-user inventory. Missing owners return
an explicit unsupported error. Ordinary same-app checks retain parsed package
and app-ID ownership. Malformed or foreign-key records reject publication;
replacement/removal does not change older snapshots. Non-client sandbox targets
now follow the original uninstall/archived and selected-package same-app checks,
then AppsFilter's force-queryable and ordinary implicit grants (#942). Retained
grants alone do not expose targets to sandboxes; full UID direction and user
scope remain enforced. Missing selected owners and instant-app visibility still
return explicit unsupported errors.
Release units pass 470 cases (0.46s). A real original-PMS Binder client runs under
the corresponding sandbox UID, checks its actual Process.myUid, retrieves the
original GSF client ApplicationInfo and PackageInfo with the saved client UID,
and sees null for a missing package. It exports the production SDK-owner capture
and checks it against original PMS's selection; native decoding and same-app
checks use that actual name. The same original sandbox client retrieves the
selected SDK package's ApplicationInfo/PackageInfo and android ApplicationInfo;
the native visibility predicate admits those targets using the captured name and
saved selected app ID. The full original ART Parcel test passes (38.46s). The
image and original-PMS template build passes (63.5s; nine rebuilt, 33 fresh);
subsequent native visibility changes rebuild all three host owners successfully
(15.2s; 39 other nodes fresh).
This owner feed is not the native boot scan producer. Native filtered replica
callbacks and the native package activation gates remain #836/#798/#724.

Native query snapshots now own AppsFilter's ordinary and update-retained
interaction grants (#724). Full recipient/visible UIDs preserve direction and
user scope; duplicate grants and self-grants follow original return values.
Ordinary queries use both classes, while SDK sandboxes use ordinary grants only.
Package removal clears both directions in both classes for every resolved user,
including shared app IDs. Replacement retains the retained class and preserves
ordinary grants only when requested. Snapshot clones retain their prior grants.
The original AppsFilterImpl grant oracle, with its own original mutable
ApplicationSharedMemory for cache invalidation, passes in the combined fixture
(19.35s). All 338 units pass (3.10s), and added image APIs link (13.1s).
ActivityManager/WindowManager producers and boot removal/query publication are
not connected to this owner yet (#724/#822).
The native keystore cleanup owner captures full per-user UIDs when requests
are posted and preserves FIFO order, repeated posts and a failed head for retry
(#822). It calls the original android.security.maintenance service with generated
clearNamespace transactions and the image's Domain.APP constant. Pinned AIDL
enums now resolve through package/import scope to their primitive int/long
backing, preserving array shape and rejecting unsupported backing types (#829);
the generator recipe was advanced to regenerate the bindings. The Binder
transport fixture independently checks primitive wire fields, captured user
scope, unrelated keys and retry order (two resource-client tests pass). The
original maintenance wrapper acknowledges empty namespaces 19001 and 1019001
with success in the disposable combined fixture (19.35s), after checking no
package owns app ID 19001 and user 10 has no saved state. This does not prove
existing-key deletion; the native queue has no boot deletion executor yet
(#822). All 31 build-generator tests pass, all 338 service units pass (3.10s),
and compile-only image API linkage passes (13.1s).
The native settings store now clears one user's saved preferred activities under
Settings.clearPackagePreferredActivities rules (#822): a named package removes
only always choices, while a null package clears all valid choices. Last choices,
other packages' candidate sets, persistent policy choices, cross-profile filters
and unrelated XML remain intact. Writes use resilient ABX and reject an external
writer; reopening preserves the result and other users remain unchanged. The
native owner also clears all restored preferred resolvers in ascending user
order and returns the original changed-user inventory. The pinned Settings
method carries its removal list between users: after its first removal, later
existing resolvers are reported changed even if no choice was removed there.
Empty or invalid-only documents create no resolver; an emptied in-process
resolver remains known until reopening. The original Settings oracle verifies
named, repeated and null clearing for both one user and USER_ALL, including
this propagation and an absent empty resolver (combined fixture 19.35s).
The persistence failure test proves that earlier user writes remain committed,
a conflicting external writer is preserved, later users remain untouched and
the returned error identifies users requiring publication. All 338 service
units pass (3.10s). This owner is not yet connected to the boot removal
transaction; home updates and broadcasts remain with that integration (#822).
APK and cluster parsing now distinguish an absent resources.arsc from a present
entry failing archive reads (#826). Disposable code-only, bad-CRC and invalid
deflate inputs verify absence acceptance and explicit read-error rejection.
The original PackageParser2 accepts the fixture whose stored resource entry
claims 512 MiB + 1 for an eight-byte payload; native reading rejects it at its
existing entry bound. This classification difference remains tracked in #828.
The combined parser/keyset/update-owner fixture passes (17.89s), and its added
compile-only APIs pass image linkage (13.4s). The resource reader now follows
resource references/configuration to their selected asset table and consumes
binary XML start/end/text events. It retains raw text, uses Java isBlank and
ArraySet order, deduplicates entries and stops after the original's 501st distinct
entry. Compiled APK fixtures compare mixed content, whitespace, hash collisions,
resource aliases and truncation with the actual original
UpdateOwnershipHelper.readUpdateOwnerDenyList on disposable boot data; locale
selection and explicit missing/malformed-input errors are also checked natively.
SystemConfig now retains valid update-ownership package/installer declarations
without a partition gate, with last-valid declaration winning and raw whitespace
preserved. The fixture compares those policies with the actual original
SystemConfig reader (15.04s); its added compile-only API passes image linkage
(14.3s). A completed pending provider read records contributors and clears active
targets' saved update owners unless SystemConfig names an owner; disabled factory
records and unrelated install-source fields stay intact. Failed resource reads
preserve the pending provider and settings, and unqueued completions are rejected.
All 322 service units pass (3.10s). Queued providers now resolve their manifest
property directly through guest-owned base/all split APK files and the framework's
system assets. The original uses PackageImpl.toAppInfoWithoutState: app-state
resourceDirs, overlayPaths and sharedLibraryFiles are null on this path. The
native reader likewise does not import those state-derived inputs. It uses the
boot owner's full ResourcesManager resource configuration; resource-less splits
must still be readable ZIP archives and do not shift the chosen XML asset source.
The compiled split/config/code-only cluster matches the actual original helper
after an explicit original ResourcesManager language update (17.42s). A direct
original generateAppInfoWithoutState call also verifies null app-state assets
and both parsed split resource paths. Missing
APK sources, corrupt code-only splits and null split paths preserve pending
providers and settings. The new compile-only APIs pass image linkage (15.1s).
Store.commit_update_owner_clearings now persists completed opt-out effects on
retained active packages as resilient ABX, removing only the updateOwner
attribute. Assigning a new owner, changing disabled records or other metadata,
removing a package or another writer changing the document is rejected before
writes. Unknown XML, installer fields, UID/users and reserve copies are checked;
all 324 service units pass (3.06s). A disposable original-PMS install requests
com.android.shell as update owner and verifies it in the live package dump;
after native persistence clears it, original PMS reboots, reports no update owner
and retains installer/signing/keysets/shared UID state (18.44s). Executing pending
reads at the original asynchronous commit phase and publishing the changed
records remain #825. Contributor relations are distinct from saved
InstallSource.update_owner, and this is not a complete removal commit.
Store.commit_removed_boot_metadata writes just the completed
domain/keyset stage as resilient ABX, retaining UID/users, unrelated XML and
legacy domains; unit tests reopen the files and check reserve copies and failure
without writes. This is a component writer, not a complete deletion commit. Scan completion now registers verified signing public keys, reuses unchanged
sets and shared canonical RSA/EC/DSA keys, and allocates monotonic IDs. Restored
settings prune unreferenced sets with the original active-package reference
rules, retaining unrelated unused public keys. Defined-keyset ownership supports
alias replacement and upgrade references. Store.commit_key_sets persists this
owner for retained active packages as resilient ABX, preserving disabled settings,
UIDs, restrictions and unrelated XML. Reopen tests verify signer/alias IDs, shared
keys, reserve copies and rejection of unrelated state or counter regression. This
component writer does not commit new package metadata or publish the scan. A
disposable original-PMS reboot (19.29s) reads native-written registrations: the
test retires a singly-owned signing set, registers the same verified public keys
under a new monotonic ID, writes ABX, then verifies original PMS preserves every
package keyset and the complete global key/set/counter table. Package identities,
signatures and shared UIDs remain intact; Settings launches and owned processes/
mounts are cleaned. Declared-key Parcel DTOs now decode the pinned Conscrypt
RSA/EC and Bouncy Castle DSA serialization schemas to canonical SPKI before scan
registration. Exact descriptors/handles, integer fields and end markers are
validated; null names/keys, truncation, foreign schemas and trailing data reject
before owner mutation. The original-runtime public-key oracle (14.82s) verifies
native decoding of original RSA-1024/2048, EC-256/384/521 and DSA-1024 streams
against their SPKI bytes. Native XML keyset parsing now applies named-key reuse/
conflict checks, distinct key/set names, nested-tag rejection, omission of empty
or invalid sets and retained upgrade definitions. Nullable public-key names and
Android Base64.DEFAULT skipping/padding are modeled. Defined aliases/public-key
sets serialize in Java collection order and roundtrip through AndroidPackage.
A compiled disposable APK fixture (0.95s) verifies the real TypedArray path,
reused keys, alias/upgrade output, missing first values, name collisions and
invalid-key omission. A disposable original PackageParser2/KeySetManagerService oracle (17.89s)
compares eleven identical compiled APKs: key reuse, nullable public names,
conflicts, key/set name collisions, invalid keys, first-use errors, empty upgrade
sets, repeated set names, multiple keyset sections, Base64 skipped characters
and multiple public keys. Parse acceptance, alias/key iteration, canonical SPKI,
Java serialization bytes and upgrade output all match. Owned processes and mounts
are cleaned. Added compile-only parser APIs link against the original image
(device-services build, 13.4s). Unsupported EC curves fail explicitly; broader
key-factory and template/app-install API conformance remain #824. The same oracle
compares six global-owner transitions: signing/upgrade alias registration, shared
signing keys, rotation to the alias key with alias release, shared-owner removal,
last-owner removal and fresh allocation after retirement. Original package
proper/defined/upgrade IDs and the entire XML-exported global public-key/set/
counter table match the native owner after every step. A seventh saved-pool
restoration step matches the original reader: it prunes an orphan set and its
exclusive public key, keeps a key shared with a live set and a wholly unused
public key, and preserves last-issued counters. Separate ART-service
profile clearing is omitted during PMS construction, as in AppDataHelper;
installd owns profile/SDK storage cleanup within destroyAppData. The pre-data
phase also detects disappeared non-updated system settings in reverse order,
preventing their data APKs from being admitted as known before full removal
(#822). Loaded component/permission/property publication and library dependency
overlays are not owned by the scan registry. Stub expansion, complete removed-system package
cleanup, certificate/library/copy error classification, publication and failure
persistence remain pending (#707/#702/#810/#816/#798).
Filesystem tests cover directory order, parent/cache cleanup, app
data preservation and partial installer failure/retry; the directory backend
in those tests deletes disposable files, not the original installd daemon.
A native Binder client test now sends generated rmPackageDir requests through
servicemanager to a disposable filesystem endpoint: system UID, real directory
deletion, returned installer exceptions and parent-only retry all pass. The
same generated Binder client test deletes real disposable CE/DE directories for
user 0, propagates user 10 installer failure, retries successfully and preserves
other packages. It verifies saved 64-bit CE inodes/default-zero inodes, flags 7,
and validation before any call for invalid names/users/private volumes. Its
endpoint is a test filesystem service, not original installd. The
original-runtime comparison separately invokes the image's IInstalld Java
interface as system UID against the original daemon, checks the child and
random parent disappear after each call, and verifies an invalid /data/local/tmp
root rejects with unrelated data intact. A system-UID Java call also uses the
original daemon to delete diagnostic CE/DE directories, retries absent data,
rejects USER_ALL (-1) as an unresolved installer user, and preserves unrelated
data (388.29s full runtime). The original image API linkage for IInstalld/Stub,
including destroyAppData, is checked too (13.2s). These are separate native-client and original-
daemon proofs, not an integrated native restoration boot. That integration,
incremental/external-volume storage, durable partial cleanup recovery and
old-path bookkeeping remain #816/#798.
Changed factory shared UID and required extraction reject
pending their owners (#804/#810); full strict-policy boot acceptance remains
#814.
All 319 units pass (3.10s), including keyset rule, nullable-name, Base64 and
Parcel roundtrip tests. The original signed framework/GSF fixture passes
(42.49s), including saved-image iteration, duplicate rejection, integrated source
completion, disposable cleanup, the data loop, ex-system demotion and factory
fallback, the pre-data missing-system input gate, owner failures and preserved
earlier effects and scanned signing-key registration. A separate declared-key
DTO fixture exercises decoder-to-alias/upgrade registration and fatal rejection
of a damaged key stream, without changing the original APKs. The full saved-system/data
comparison passed (388.29s), including exact per-package keyset IDs and the
entire restored public-key/set/counter table against original PMS. Fresh scans
also verify registered public keys against each APK signer. Ex-system demotion
and the pre-data missing-system gate remain covered by the signed fixture. The nine
original-APK scan-input tests passed (6.21s) before this loop change; the Java
image API linkage build passes (13.2s). These are system/data APK phases, not a
complete native boot or template: native APEX preparation, removed-package
reconciliation, stub expansion, graph/permission/commit side effects,
persistence and replica publication remain #707/#702/#798/#810/#812/#813.
Original PMS still runs.
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
assumptions. APKs remain read-only. Package units pass (203, 3.17s), including
corrupt-payload rollback, metadata and reuse. The disposable original runtime
matches admission errors, extraction content/mtime and repeated-copy reuse in
48 combinations at 16 KiB guest pages (2026-10-03, 84.27s); all earlier checks,
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
ScanPackageUtils entry point.
`Apks::copy_native_libraries_for_supported_abi` and
`copy_native_libraries_with_override` now orchestrate ordinary-storage package
copies over a held base/split union. They select supported-list preference,
create the native root/ISA directory and copy APKs in input order. Multiarch
copies 32-bit before 64-bit and reports ignored overrides; single-ABI copies
apply clear/explicit overrides and RenderScript policy. Missing/mismatched ABIs
retain the original helper's distinct return codes and wrapper handling.
Writable roots, guest ownership, ZIP clock conversion and existing-directory
restorecon come from their owners, independently of APK read mapping. Directory
failures propagate; unresolved final symlinks require filesystem-owner resolution.
The same runtime matches 64 layout/multiarch/override cases with original helper
return codes and complete directory/file trees, bytes, modes and timestamps,
including split overwrite order. Unit tests cover directory creation metadata,
existing-mode preservation and restorecon failure.
`SigningScan::finish_native_library_install` now completes required ordinary-storage
copies before committing accepted ABI/path settings. It checks the retained
identity/UID/signer candidate and binds the writable guest root to the derived
path before writes; copy flags come from parsed metadata. Extraction ABIs retain
32-before-64 ordering before the platform ABI override. Typed failures preserve
the accepted setting and report both the manager error and the native copy cause.
The disposable runtime verifies stale candidates, missing completion, root mismatch,
corrupt compressed payload cleanup and successful copy followed by metadata commit
using generated ZIP seam inputs; this is not a complete signed APK install or the
original full scan entry point. The expanded runtime passes in 81.47s, including
earlier original helper comparisons, Settings launch and owned cleanup; all 286
aim-services unit tests pass. A failed copy can retain created directories or earlier
successful files, as the copy helper does; whole-install rollback remains with the
install owner. Incremental copies, production filesystem/clock owner delivery and
boot/install publication remain #810/#798/#702; native PMS is not activated.

`SigningScan::finish_scan_metadata` now stages accepted ABI/copy, page-size and
code-time/version metadata through one completion path. It validates the current
candidate and required image page policy, returns copy reports and both ABI and
alignment diagnostics, and accepts the staged settings only after all stages
succeed. Factory/saved reuse needs no writable destination; required extraction
still rejects without one. Page alignment now binds handle flags to the parsed
package and its root to the accepted setting's legacy native path (#811).
The runtime verifies compressed extracted ELF alignment despite contradictory
caller flags and a divergent parsed root, direct-mapping and missing-file
diagnostics, copy-then-code-time failure preservation, and successful copied and
saved-ABI completion. All 288 aim-services units pass (3.25s); the expanded original
runtime passes (83.02s), with prior component comparisons, Settings launch and
owned cleanup. Generated ZIP completion tests isolate these seams; they do not
prove a complete signed APK install or the original full ScanPackageUtils entry
point. Files copied before a later error remain with the install cleanup owner.
`finish_application_metadata` now completes factory-test and final public/private
ApplicationInfo flags. Factory mode comes from the boot owner and requires the
exact requested FACTORY_TEST permission; an old factory flag is cleared otherwise.
Flags are recomputed from the adjusted parsed package and updated-system state,
replacing retained bitfields. The accepted owner rejects stale candidates and
refreshes a retained shared UID member's removal inputs without re-ORing cached
group flags. The completion path includes this stage after code metadata.
Original PackageImpl/PackageInfoUtils/SettingBase APIs match 16 factory/permission/
old-flag/updated-system cases. All 289 aim-services units pass (3.71s); the expanded
runtime passes (84.77s), including earlier checks, Settings launch and owned cleanup.
The Java image API linkage build passes (13.8s). This checks the final flag helpers,
not the whole original scan entry point. New system shared-UID candidates now
reconcile signing without first membership admission. `scan_new_system` retains
UID preparation through ABI/page/time/final-flag completion and admits the member
with its final flags; a late failure preserves prior settings/libraries/signers,
releases an independent slot with the original cleanup cursor, and retains an
empty shared group until pruning. Retained members still refresh removal inputs
without re-ORing cached flags. All eight original-APK scan-input tests pass
(6.06s), including new/retained factory flags, removal, late code-time failures
and UID retry behavior. All 289 units pass (3.08s), and the original runtime
comparison passes (84.48s), including earlier shared-UID flag sequences and final
flag helpers. This stages metadata ownership, not a complete install transaction:
filesystem cleanup, integration into actual boot/install commit ordering (#812),
full result persistence and replica publication remain #702/#810/#798.

`Store::commit_native_library_metadata` now persists completed scan ABI/path
metadata for retained active and disabled-system packages through the existing
ABX backup/reserve owner. Active packages also persist page-size compatibility;
the pinned disabled-system writer does not write that field. Optional values
are removed when cleared, including the old `requiredCpuAbi` fallback. The writer
rejects package additions/removals, UID/signer/code-path and unrelated changes,
invalid page flags, duplicate owners and concurrent document changes before
writing. Unit tests verify typed values, cleared attributes, retained unknown
nodes and no mutation on rejection; all 288 aim-services units pass (3.10s).
An explicit stopped-volume write and original-PMS reboot passes (17.85s): an
actual arm64 package retains its ABI/library paths and native-written page-size
settings; its install-only ABI override is cleared by the original ordinary-boot
scan, as ScanPackageUtils specifies. All original package UID/code/signers,
shared users and key sets survive, and Settings launches. This is persisted
metadata compatibility, not native boot ownership or complete install publication
(#798/#810/#702).

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
calls in a boot averaged 2.9 ms). Guest-init now sweeps unused backing files
once at instance startup and after stopping its processes (#1008);
`linux-run --sweep-memfds` performs the same owner cleanup between batches.
It requires the memfd mark, an exclusive nonblocking lock and matching inode
before unlinking; live descriptors and cross-process reopening retain their
backing names. The creator takes its shared lock before marking the file, so
cleanup also collects newly released files without a five-second delay.
The accumulated 93,754 unused files occupied 65.568 GiB; owner cleanup
reclaimed them and restored approximately 78 GiB of free space.

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
later. Linkerconfig runs for both `perform_apex_config --bootstrap` and
post-fs-data's full APEX activation, as pinned init does. The native bootstrap
and default mount owners select different APEX inventories; skipping the
second helper left the default configuration at its one-byte placeholder in
the failed `7f837348` boot (#1245, related to #1228). The owner regression now requires both
helper invocations and propagates a failed second helper. A fresh boot has
not yet verified the correction. A first boot
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
- `/proc/mounts`, `/proc/<pid>/mounts` and `mountinfo` read the actual target
  process's registered mount owner. Source now registers native guest-init's
  host incarnation as guest PID 1, retains host identities for authentication,
  and translates public PID boundaries consistently (#1158). Separate-process
  tests cover PID inputs/outputs, init signal immunity and real Binder/Mach
  identity. Root combined verification passes ABI180/0 (seven ignored entries, three
  subprocess helpers actually run), Binder driver7/0 and host30/0,
  guest-init22/0, storage14/0 (its helper actually run), and PM805/0
  (eleven ignored). A further invalid-signal precedence regression passes.
  These source gates do not establish runtime conformance.
  A fresh official Apex CTS run remains required; the frozen original
  campaign's recorded failure is unchanged. Queued signals to native init return
  EOPNOTSUPP rather than discard siginfo; its caught ordinary signals use Darwin.
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
  `/proc/<pid>/stat` give 0 for a parent outside (#364), and the authenticated
  native guest-init parent is exposed as PID 1 (#1158), and `sysinfo` counts the namespace's processes (#366). A `linux-run` started without a table (tests, a debugging
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
- Mount namespaces use shared, generation-published owner journals. Fork and
  exec inherit the owner; `unshare(CLONE_NEWNS)` clones its current mount state
  and propagation settings. Shared/slave/private propagation, bind/FUSE policy,
  tmpfs, move and `umount2` retain actual ownership. Native init's live path-map
  changes update that owner, including writable data mirrors (#1158).
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
