# Darwin ART compatibility runtime

This living document records the current architecture, acceptance scope, and
verified checkpoints. Read **Current status** and **Next boundaries** before
working; append only verified progress, and keep **Latest progress** to at most
ten compact items. Detailed experiments remain in Git history.

## Product goal

Run unmodified Android APKs on macOS behind an Android-compatible runtime.
Current evidence is APK-scoped; it does not claim full Android compatibility.

- ART, ActivityThread, Binder, ViewRoot, Choreographer, HWUI,
  SurfaceControl, and Android services own Android-visible policy and lifetime.
- AppKit, Metal, CoreAudio, Network.framework, CoreText, and related APIs are
  narrow Darwin providers of host mechanisms.
- Rendering uses HWUI/SurfaceFlinger/Metal and IOSurface sharing. CPU fallback,
  app-specific composition, GL fallback, and direct-Dawn-Metal fallback are
  not completion evidence.
- When host and Android contracts differ, isolate the decision at the provider
  boundary and record an ADR.
- Retina logical Android coordinates and macOS backing/physical pixels remain
  separate; locale/resource lookup, physical keyboard input, and host
  font/camera/Bluetooth/secure-service integration preserve their respective
  owners.

## Current goal — production code out of `probes/`

Priority (2026-09-18): user requests real Chromium example.com rendering first.
Do not defer the blank webpage body behind unrelated migration/refactoring.
Priority reaffirmed by the user: normal unchanged Chrome must visibly render
the Example Domain heading and body at example.com, with physical reload/input
and a fresh Retina window capture. A URL in the address bar, a live process,
or passing transport fixtures is not rendering acceptance. Keep unrelated
Probe migration/refactoring behind this gate; retain those final goal gates.

Chromium acceptance requires its real Graphite/Dawn → Vulkan → MoltenVK → Metal
path. The launcher must reject GL, direct-Dawn-Metal, disabled-GPU, and disabled
Graphite alternatives. Android HWUI's separate EGL/ANGLE provider is not this
Chromium gate.

JIT/JNI/ELF/smoke fixtures may remain under `probes/`, but ordinary installed
APK product code must not depend on them. Split process entry, registration,
lifetime, graphics session, input, and shutdown by Android subsystem or narrow
Darwin provider; do not promote fixture Activity/View assembly into production.

Completion gates:

1. Production source/header/link-map closure contains no `probes/`
   dependency or fixture JNI export.
2. Tests exercise production objects and test code is not linked back into the
   runtime dylib.
3. Entry/input/GPU/shutdown owners are split by policy, transport, and lifetime;
   renaming or moving a monolith is insufficient.
4. Both runtime flavors and an exact warm no-op build pass, followed by fresh
   physical Calculator, DeskClock, and Chromium acceptance with Retina GPU
   rendering and physical input.

## Engineering rules

1. Never modify APKs, Android framework sources, or AOSP apps to ease acceptance.
2. Probe classes and app-name branches, forced clicks, fixed-coordinate
   corrections, reflection fabrication, swallowed exceptions, and success
   stubs are not production solutions.
3. Preserve AOSP ownership: framework lifecycle/policy, ViewRoot input delivery,
   Choreographer scheduling, SurfaceControl transactions, BufferQueue/fences,
   permissions, AppOps, restrictions, shortcuts, resources, and locale resolution
   stay Android-owned.
4. Keep Darwin code at genuine host boundaries (window/events, Metal/IOSurface,
   filesystem/process, network/media/fonts/camera/Bluetooth/security); it
   supplies mechanism, not Android policy.
5. New host resource/transport ownership should use Rust where compatible, with
   a narrow C/C++ ABI and explicit state/lifetime interfaces.
6. Do not add subsystem policy to broad Bridge/Manager/Utils/Service owners;
   retire duplicate implementations instead. `DarwinServiceBridge` remains
   retired.
7. Pin AOSP/versioned upstream owners and use narrow per-release adapters.
   Before a materially different ownership decision, write an ADR.
8. Test actual production translation units, descriptor/resource ownership,
   callback reentry, bad_alloc paths, and pending-exception behavior. Shared
   Cargo/native/DEX builds are serialized.
9. Use the repository's pinned
   `_aosp/external/skia/third_party/ninja/ninja`; stale dylibs, copied
   archives, and warm-build claims from another Ninja are invalid.
10. Do not reset profiles, modify APKs, add CPU fallback, or hide physical
    interaction failures. A component PASS is not product acceptance.
11. Keep test fixtures outside production source ownership and make runtime vs
    test-only inputs explicit in the graph.
12. Clean up mixed owners before declaring the goal complete, but do not expand
    this APK-scoped goal into unrelated platform support.

## Architecture boundary

```text
Unmodified APK
  -> Android framework / ActivityThread / system services
  -> Binder + InputChannel + SurfaceControl contracts
  -> ART + HWUI + app native libraries
  -> narrow Darwin providers
       AppKit input/window | Metal/IOSurface | macOS network/media/filesystem
```

Android owns package/resource/configuration and locale semantics, application
and activity/service/process lifecycle, Looper/Binder callback affinity,
ViewRoot input and Choreographer scheduling, SurfaceControl hierarchy/crop/
alpha/latch/fence contracts, and permission/AppOps/shortcut/restriction/
connectivity policy.

Darwin owns NSWindow/NSEvent and backing-scale mechanisms, Metal drawables and
IOSurface import/export, host process spawning/W^X/Mach signal context, macOS
network/audio/video/fonts/camera/Bluetooth/secure services, and profile/daemon
transport without duplicating Android policy.

Related ownership decisions:

- [macOS network path projection](adr/0001-macos-network-path-projection.md)
- [virtual system native image paths](adr/0002-virtual-system-native-image-paths.md)
- [SurfaceFlinger display-output ownership](adr/0003-surfaceflinger-display-output-ownership.md)

## Current status

DarwinArtSurface now publishes immutable retained backing through SurfaceBackingOwner;
the former mutable IOSurface/texture/extent/stride tuple is removed. AppKit caches
one immutable actor-only handle; ANGLE, size/id and GPU readers Acquire exact
snapshots. Producer map retains the exact allocation through unlock; GPU frames
and Skia texture-release callbacks retain their snapshots. Allocation, OutputOwner
IPC, scanout and context shutdown remain separate. Final product gates and actual
product mapping/resize verification are recorded below; source adoption does not
prove fresh APK acceptance or concurrent mapping/GPU/context-teardown safety.
Actual pinned Ganesh/Metal callback-lifetime execution now passes owner Close
retention, synchronized draw/retirement and genuine factory failure cleanup.
This is shared snapshot/payload lifetime proof, not concurrent GPU teardown.
The remaining inline scanout diagnostic state/readback/PNG responsibility is now
owned by graphics/ScanoutDiagnosticCapture. PresentSurfaceOnMain passes its exact
immutable backing and existing encoder, retaining presentation/commit control.
Real Metal/PNG component verification and Astra review pass; this does not
establish fresh APK or physical input acceptance.

Production AndroidKeySink now enters the exact retained surface-root ingress.
Root decisions/readiness are value-fenced through proposal, action reservation,
transport completion and final Java admission; first keys have a bounded FIFO,
fixed decision ownership, scheduled expiry and genuine readiness/capacity wakes.
The global key enqueue and receiver Init focus shortcut are removed. Untargeted
embedding key ABI rejects rather than selecting another root. Process shutdown
closes and polls ingress task/timer/notification/factory/Submit completion before
VM/image teardown. Astra approves this component adoption, not APK acceptance.

Canonical acquisition and process Close/Poll now have a separate registry owner
behind opaque ingress-lifetime ports; registry code cannot access queue State.
AppKit title/icon projection is isolated in window/application_identity. These
are bounded ownership extractions, not the remaining mixed-owner exit audit.

Actual-object ASan/UBSan components pass, including delayed readiness, packet
completion retry, stale-head/successor ordering, frozen fences, bounded capacity,
submit exceptions and immediate timer cancellation. Fresh unchanged Calculator,
DeskClock and Chromium physical Retina/Vulkan acceptance remains OPEN. User
approved three-app restarts; old app processes were closed/terminated without
resetting profiles. Registered startup diagnosis found pre-VM SurfaceSession creation installing an
Android sink before a genuine ART owner exists. The host now creates an inert
display target and borrows that exact surface through the additive process ABI;
ProcessEntry activates its existing Android input owner after Runtime::Start and
native registration, before ActivityThread entry. A fresh run rejected activation
at the earlier CreateVm boundary: upstream Runtime::Start, not Runtime::Create,
restores kNative. The state gate remains unchanged. Astra approves the corrected
ordering. Subsequent physical core acceptance PASS Calculator2+3=5 and ClockTimer
on one normal shared system generation. Actual physical keyboard still fails:
native root decision attach rejects server capture, leaving server_live=0; main
diagnoses that Binder boundary without fabricating focus. Chrome fresh body is
blank and Vulkan webpage acceptance remains OPEN. Scanout scheduling/readiness/
fence lifetime now has a JNI-free narrow owner; Astra approves its source, not
APK acceptance. Latest both products compile/link PASS, final source/link
boundaries clean and exact pinned Ninja repeat no-work. Final three-APK physical
keyboard/Vulkan webpage verification remains OPEN. See the latest
checkpoint for exact evidence and remaining boundaries.

Root-server capture now decodes genuine AOSP BinderProxy nodes rather than the
legacy-only wrapper. Native node death and the exact Rust Client terminal flag
jointly fence authority. The new hooks retain only atomic metadata, not the
transport; pre-VM failure and independent resource retirement are explicit.
A started Binder pool still lacks joined-worker proof, so reusable unload
honestly remains NOT_READY; Android OS process exit is a separate boundary.
Both final product link audits and source/fixture closures pass; the exact pinned
two-product Ninja repeat is no-work. Native concurrency, host adapter and ABI
tests pass. Fresh physical acceptance has not been rerun on these changes.

ActiveServices now uses an explicit exact-client/SYSTEM connection ledger in
production. Client death detaches all its records before owned-service restart
decisions; link reentry and delayed obituary retain exact record/pin fences.
Independent production support DEX compilation passes. The four service lifecycle
IPC calls now run outside the ActiveServices monitor through exact-owner lanes;
ServiceConnectionDeathRegistration now owns exact link/unlink resource tails;
pending connections grant lifetime pins, not lifecycle demand. Direct bind
admission runs link outside its own synchronized block. ServiceNotificationController
now captures immutable publications and serializes transport per exact connection
outside that monitor; the standalone unlocked-notification gate passes. Phased
process transport now runs outside that monitor through shared process demand.
The pinned Binder JNI
owner now exposes a read-only recipient-presence query after failed public unlink;
native compilation, controlled resource tests and the isolated genuine ART/BinderProxy
JNI-list boundary now pass. This does not prove wire obituaries or reusable VM
shutdown. Shared-demand race fixtures pass; none of this is final
physical APK acceptance.

BoundServiceProcessLauncher now exposes opaque prepared launch phases; exact
retirement and terminal cleanup pin real activation/cancel/registry-claim tails.
Controlled launcher races pass and Astra reviewed the frozen owner. ActiveServices
now uses ServiceProcessLaunchController for process-keyed shared demand; the
production start wrapper is removed. One-epoch fixed-point draining dispatches
launch/death resources, lifecycle IPC and deferred unlinks outside its monitor.
Terminal settlement requires genuine Binder liveness, not exception type;
captured owner snapshots and adoption revisions fence delayed failure tails.
Final Astra source review finds no new must-fix. Component tests and new product
gates are recorded below; neither source approval nor old running PIDs establish
fresh physical APK acceptance.

Daemon bound-service admission now verifies the exact current system child when
returning its immutable launch template, after gate/reap waits and before both
Existing and spawn paths. Rust profile tests114 PASS/fixtures5 ignored; actual
server Existing-path tests reject wrong incarnation and allow genuine attached
Starting. This is an authority fence, not completed parent/shared-demand cleanup.

BoundServiceProcessLauncher now owns the production AMS prepare/activate ledger;
ActivityManagerEndpoint delegates launch, exact attachment handoff and captured
Binder obituary. Rust owns opaque capability lifetime behind a narrow JNI/FFI
provider, including failed unpublished cancellation retry and immutable profile
socket scope. This completes that launch-owner slice, not fresh physical APK acceptance.

ServiceLifecycleController owns latest-state planning and typed Android callback
settlement; immutable ServiceLifecycleOperation owns only captured transport.
Exact-owner lanes reserve one operation under lock, dispatch outside it, and
settle transport without fabricating framework completion. Unchecked transport
failure seals its lane; registry-current checks reject stale attachment snapshots.
Fresh physical acceptance and the remaining lifecycle boundaries stay open.

## Prior subsystem checkpoints — not final-source acceptance

The following notes preserve older extraction evidence and sequencing. Their
"current", "not adopted" and pending-shortcut statements describe their original
checkpoints, not the Current status above; do not reuse old product hashes/PIDs.

The exact desktop-root provider now exposes an independent weak native stamp
subscription. Every effective stamp mutation is published outside the provider
lock before existing context/fact/cancellation tails; separate publication and
activation revisions prevent old facts from authorizing a new key interval.
RetainEvents exposes only the original retained root. The installed Android
surface input sink now retains the canonical exact-root RootKeyAuthority,
which subscribes to this hook. This is ownership/lifetime adoption, not first-key
acceptance: AndroidKeySink still uses the existing routing path. The typed WMS
decision callback now calls a separate WM JNI admission owner, which retains
that exact root/authority/server generation and resolves original channel tokens
outside input locks. A separate root_key_routing owner now joins immutable
root decisions with exact receiver readiness; action reservation, transport
send/completion and final pre-Java local FIFO admission preserve its value
fence. This conditional gate chain is component-verified, not activated host
key routing: AndroidKeySink still takes the old path, and bounded first-key
progress plus Init shortcut removal remain open.

Input action reservation/send/completion and stream/CANCEL retirement now have
an independently compiled Android input owner. Publication/history/local FIFO
remain routing-owned behind named locked ports; the routing file is 908 lines,
not the previous 1691. Both products and eleven sanitizer regressions pass.
This is ownership/lifecycle integration, not authenticated native key authority
or fresh physical APK acceptance; those goal gates remain open.

RemoteBinder JNI identity decoding now has its own narrow typed owner. The real
wire caller retains trusted-class lookup and exact-generation registry policy,
with original-owner checks before and after callback-capable JNI. The WM decision
adapter now captures this exact lifetime before handshake/early drain; this is
not yet adoption by actual key-send admission. The remaining vertical-slice
implementation order is in Next boundaries.

Installed Android surface input bindings now retain the exact surface-owned
DesktopRootEvents through a guarded native callback context. The existing Rust
SurfaceSession installation caller adopts it without changing the exported ABI.
This is identity/lifetime integration only: authenticated focus/readiness joining
and synchronous native revocation are still not adopted.
The production AppKit resign delegate now commits the validated local stamp
before pointer cancellation, then delivers its retained fact only if the full
stamp and binding remain current. Reentrant close/rekey/rebind/destruction cannot
resume a stale fact. This fixes provider ordering, not native input authority.

Legacy baseline/button fixture builders now project canonical production WMS
sources and package the actual emitted WMS class inventory. Baseline owns that
compilation; button reuses it. Compile-only ServiceManager signatures remain
outside D8, with an original bootclasspath definition guard. Final serialized
build-button-dex passes (baseline193/methods3548; button270/methods4042).
This repairs fixture preservation, not fresh physical APK acceptance.

Document open/save/path-free API ownership now lives in the narrow macOS
filesystem provider, not the surface bridge. Actual product link/graph and
component tests pass; no internal APK picker caller or interactive acceptance
is established by this extraction. Latest checkpoint records the evidence.

Current source adds authenticated per-openSession WMS sessions: checked
identified attachment snapshots support legitimate bind-in-progress callbacks,
and every recognized operation rechecks PID/UID/app-thread/start sequence.
Service-wide canonical IWindow registrations reject duplicates/foreign tokens;
exact per-registration admission serializes IO without holding the ownership
monitor. ADDING/LIVE/RETIRING state prevents ordinary relayout from resurrecting
partially removed resources. These owners are adopted by production endpoints
and SystemServiceFactory; the shared-session bypass is removed. Process-death
cleanup is not connected yet.

Typed geometry/focus publication JNI and an exact retained batch delivery owner
are present, but production WMS focus integration remains incomplete:
Delivery now quarantines terminal/null/thrown outcomes with an opaque exact
attempt instead of replaying them. Explicit terminal abandonment requires the
recorded transport's original-TX terminal/quiescent postcondition; default false
cannot discard work. Mixed batches settle with per-record dispositions, not an
all-accepted acknowledgement. Production WindowSession now adopts the shared
WindowPublicationController for ADD/RELAYOUT/REMOVE geometry. Separate
WindowPublicationDriver and WindowInputEndpoint own a genuinely dispatched WMS
HandlerThread and the original native lease respectively. Geometry status is no
longer discarded; accepted-prefix flush and terminal/quiescent retirement keep
the original alive beyond queue acknowledgement, including the last REMOVE.
Window snapshots start explicitly UNBOUND. The shared desktop-root binding
owner now pairs canonical LIVE registrations using exact original channel
tokens, full authenticated attachments and BIND_WINDOW_V1. Binding preserves
geometry and grants no activation. Original close/death and stale own attachment
seal admission and unbind paired windows without retiring their WMS resources.
Authenticated root REGISTER now pins host PID plus kernel process birth through
DesktopForegroundAuthority. The Darwin provider uses fresh synchronous public
foreground queries with birth checks, not cached AppKit activation properties.
Capture and WMS activation reconciliation are adopted. Separate
WindowRootActivationPolicy consumes exact retained facts on the actual WMS
handler, freshly validates the captured process/attachment, and feeds real
activateRoot/resignRoot into original-channel publication. Window mutations
validate an existing activation before selecting a new gain; they do not infer
one from visibility. ADR0004
records why the undriven system main runloop rules out cached isActive here.
Parent-subtree retirement and process-death cleanup remain OPEN;
the genuine AppKit root event provider is now wired to NSWindow delegates, but
root-event/channel/epoch first-key correlation is not wired yet. Historical physical
acceptance below predates the session changes; do not reuse those captures as
acceptance of this source family.
Actual receiver Init now pins the original Java channel token against its exact
captured native core through admission-controlled cleanup. WMS decisionSnapshot
retains per-display loss epochs. Typed capability subscription and authenticated
process-owned decision retention are adopted by the actual DesktopRootClient;
client lifetime admission links the exact capability before its trusted
handshake. Component tests establish terminal death/close handling, same-link
retry and tracked outside-lock best-effort cleanup. Actual imported capabilities
are RemoteBinder endpoints, which inherit local Binder's no-op death methods:
production remote death notification is NOT implemented. Do not treat those
fixture callbacks or link returns as real server-lifetime proof. Native input
authority must not adopt these methods as a working death fence. Actual native
wire ownership now supplies a separate immutable channel-generation lifetime:
proxies capture it, exact transactions/imports/exports reject retired owners,
and retirement seals it before catalog removal/shutdown/JNI cleanup. Readers
retain duplicates of the original socket, not reusable numeric FD authority.
Startup attach failure is cleaned by the creator's valid JNIEnv. Native input
has NOT adopted this lifetime or an independent revocation hook yet; no input
quiescence or remote Java Binder death callback is established by these changes.
Retained decisions do not yet gate native first-key input.

DesktopRootEvents owns exact original-window identity, main-thread host root
incarnation, checked serials, local key-interval revisions and retained observer
bindings. Any-thread immutable LocalStamp snapshots record validated transitions
even before/without an observer. Bind assigns an emitted serial without replacing
an unchanged interval; Unbind clears that serial, and PrepareClose seals before
deferred callbacks. This cached host mechanism does not yet invoke an independent
native input-revocation hook. Separate AppKit delegates report current observed
key-window facts and terminal close through the narrow surface observer API.
The production delegate is connected. DesktopRootClient now has a real
post-commit receiver hook and a MainLooper host-fact transport; fresh APK
acceptance of this source path is not established.
Serials order emitted validated facts, not every host transition. A CLOSED
event does not imply Android pointer/focus obligations have settled. Neither
this provider nor its independently passing tests establishes WMS focus or the
first-key barrier; the Init-time focus shortcut remains pending removal.

DesktopRootTarget now retains the exact original AppKit window and root event
owner, with any-thread acquisition from an attachment-independent process
catalog that rejects ambiguous live roots. Actual surface initialization,
delegate close and destruction publish/retire this owner; background final
release marshals AppKit cleanup to main. DesktopRootRegistry/Endpoint are
registered by SystemServiceFactory as `darwin.desktop_root` and listed in the
production Java manifest. They authenticate full application attachments,
retain unsigned ordered host facts, and reject stale/dead capabilities without
granting focus. DesktopRootClient's native owner gates queued AppKit work,
admitted JNI calls and retained callback contexts before shutdown commits;
live-env class/callback cleanup precedes ELF unload and DestroyVM. Java queue
settlement is separately gated. These are host-fact transport prerequisites,
not completed canonical WMS focus publication or physical first-key admission.

Latest current-source acceptance: root10596 terminal0 passes physical
Calculator76274 2+3=5 and DeskClock76642 Timer at 720x1280 backing, shared
system76272. Chrome77172 is reopened on that same runtime. Its restored
example.com toolbar/title appears over a blank body; physical tab click does
not visibly switch. Root50868 terminal0 diagnostic confirms physical pointer
Java invocation/delivery/handled=1, then hardware-key sink status2 (NO_TARGET).
Omnibox composition still fails a central compose transaction. Toolbar/title
text is NOT proof of loaded webpage body, tabs or keyboard navigation. Both
core app host titles are package names, unresolved localized-metadata failures.
Only test-artifact destination inheritance was removed from system provisioning;
actual forwarded configuration/debug flags remain canonically hashed. Corrected
Chrome and both core apps now coexist without replacing their shared runtime.
Current graphics SHA256 47531ee2fba5e71a984694ac5fb38343fa5191460fab301b920351e1edd69231
and headless 8e900b8e4f66bfd188550204cecb9cf5a7dfbc58145de1bdb1d24880fac01fd2.
Built support DEX SHA256
7afbe1393989bc4be818593b21ac9feffd15fcdcc405104c66462168943e4f9c
(168 classes / 1184 methods). The previously copied profile DEX remains
2803b2377286b691ddee52d8c0fdf2e61e2b43b559b4559afa883fb5a956184a;
no profile copy or APK restart was performed on 2026-09-18.
No full three-APK acceptance is established.
These products include separate AppKit hardware-key, root lifecycle and delegate providers;
the physical captures above predate those extractions and do not verify their new
product bytes. Existing APK windows were not stopped or restarted this turn.

AppKit content-view ownership now lives in window/appkit_content_view.h/.mm.
CreateSurfaceOnMain creates that real owner; the surface bridge no longer
implements the view or owns its input stream state. ARC retains layer/view
resources. Successful product-linked tests use exported offscreen create/destroy
and retained sink callbacks for reentrant cancellation/replacement/detachment/
destruction; independent actual Metal layer tests cover sRGB and Retina resize.
These do not establish fresh APK behavior or complete pointer teardown.

The production-native separation goal remains active, not complete. User-approved
coordinated restarts are only for fresh ordinary APK acceptance, never profile
reset or modified APKs. The latest core acceptance above passes on current
session owners; Chromium body/tab/focus gates fail/open. Earlier product and
physical results are historical, not substitutes for current-source acceptance.

Current WMS publication owner, focus wire and exact-recipient focus consumer
pass both product link audits and an exact Ninja warm no-op. The actual receiver
Looper pump now calls the epoch executor and original onFocusEvent callback;
authoritative WMS focus production remains unconnected to the AppKit root source.
The existing Init-time focus shortcut is still pending removal. Earlier APK
restarts supplied the historical evidence above, not the current AppKit products.
Read-only sample of live Calculator76274 on 2026-09-18 reports loaded graphics
UUID63EB52FE-7638-3A92-A917-CF21CDCE11EF, unlike the file checked alongside it:
F88A4CD9-3E03-3A6D-971D-56B3DF6A6D01. Existing windows cannot verify the new bytes.

Latest Astra reviews are read-only (CLI and subagent). Directional TX/RX terminal
state now preserves authorized RX after TX EPIPE and default/typed-reader healthy TX
after RX EOF; real socket/Looper and callback-reentrant interest tests pass.
RX framing now has an explicit FrameInputReader owner; typed buffered/consumed
progress is published only after its admission unwinds, including exceptions.
Admitted input execution now uses InputTransportReadiness, a strong invocation
snapshot and value-only transition requests. InputTransportPumpLease retains
registration authority, exact completion commit, progress-before-fresh-TX
retirement ordering and quiescence. The new service does not own Looper entries
or decide drain retirement. This separation is integrated in both products;
EOF future-obligation ownership and remaining oversized/mixed-owner cleanup
are still incomplete.
Packet/window framed consumers now have explicit deferred/consumed/stop ports;
the actual receiver JNI now adopts ReceiverInputConsumer for ordered Java
invocation and exact local queue head leases, removing remote enqueue-only
success and destructive dequeue/requeue delivery. The ordered regression is
now in the default channel suite and passes with budget-limited EOF retries;
component success is not fresh physical APK acceptance. Reader parking,
coalesced non-budget recovery and remote completion through a local
continuation remain incomplete. FinishLedger now owns bounded, no-eviction
finish reservations independently of endpoint descriptors; actual JNI reserves
before Java invocation and separates observation closure from transport ACK
acceptance so delayed finishes release capacity safely. Actual JNI now uses a
receiver-epoch ReceiverFinishOwner, retaining each original recipient/sink and
full packet sequence before Java invocation; local finishes owe no wire ACK.
Java sequence selection now atomically skips live ledger identities across
uint32 wrap, including zero; failed admission leaves cursor/output unchanged.
ViewRoot no longer increments its cursor before attempting a reservation.
Imported finishes use ACK v2 with uint64 original packet identity; v1 decoding
remains separate, not interoperability with old framework-sequence peers.
Inbound ACKs no longer mutate Java receiver finish state. The live-reader JNI
callback now retries retained ACKs after resource-pump TX flush, using bounded
per-attempt claims, original metadata and post-drain budget wake checks.
Uncertain provider exceptions terminalize only the exact TX before retry claim
release. Empty-TX allocation/counter rejection still needs non-writable owner
recovery. Remote admission now seals in the actual receiver callback only after
its original framed pump returns terminal, including local-wake continuation.
FinishLedger exposes one mutex-coherent sealed/outstanding snapshot; the count
includes imported events awaiting Java finish and retained/claimed ACKs, not
local events or accepted/terminal ACKs. An unsealed empty ledger is not EOF
settlement. EOF completion ownership remains incomplete for both already-recorded
unbuffered ACKs and not-yet-finished Java events.
This is not full directional settlement: parking/owner-only retry,
future/asynchronous ACK obligations and ordered packet/control delivery
remain required before authoritative retained WMS output adoption. See the last
checkpoint for current product/review evidence.
The actual receiver JNI now reports remote reader completion separately from
registration retirement through binding and claimed pump. Completion removes
INPUT/readiness while private matching buffered TX flushes; legacy integer0
still retires unconditionally. Real socket and production-owner component tests
cover EOF ACK bytes/fence, stale disable/HUP and reentrant disposal. Fatal
ERROR/INVALID wins even on the first completion transition. This is buffered
TX retention, not future ACK settlement or fresh APK acceptance.
The framed output endpoint is now immutable and the actual receiver packet
caller supplies its original receiver/token to the typed JNI port. Both changes
pass current product links and bounded Astra source reviews. Exact-target unit
tests now use real admission/JNI resource cleanup, but still substitute binding
and routing; full callback/ledger integration remains open. Legacy mutable-
ViewRoot dispatch was removed from the product. Its genuine fixture migration
now links both clients without the five private receiver imports. The test-only
imported-channel adapter uses guest Os I/O, retained wire exchange, owner Looper
progress and independent ACK completion; failed progress cannot become successful
delivery or trigger duplicate fallback. Scoped MotionEvent recycling and sidecar
invocation retirement preserve JNI ownership during callback reentry. Mocked
JNI, resource-lifetime and wire tests are component evidence, not actual receiver
ledger integration. Latest VM attempt fails before input construction: reusable
fixture mode does not install the ordinary APK Binder process endpoint, and
DisplayManager bootstrap reports BinderProxy BadFD (status27). Host execution
selection, typed VM/process lifetime and value-only pre-exit verification now
have separate modules. AndroidProcess exit requires the AppKit actor to seal
its pump phase before authorizing the owner; actor/runtime/cleanup/assertion
failure cannot publish successful completion. Real fixture process identity,
service readiness and reusable-session actor quiescence remain unconnected;
no fake response or APK-mode flag bypass was added. Fresh unchanged three-APK
physical acceptance remains outstanding.

AppKit execution now has a separate Rust actor owner and an exclusive main-thread
session lease acquired before worker spawn/image loading. Actual-owner component
tests cover wrong-thread admission, pumping affinity, worker panic propagation,
sequential reuse and one-shot process completion rejection. This is not reusable
runtime retirement evidence: hosted macOS ReusableSession is now rejected before
worker/image startup because raw pump-vs-teardown and failed-pump join are unsafe.
All four public APIs preserve duration/completion validation order; neither
headless nor secondary-image selection bypasses admission. Ordinary APK and
service-child AndroidProcess execution stays admitted. Existing reusable fixture
and real-graphics-acceptance execution cannot pass this gate; do not silently
promote them or count static-callback tests as reusable VM evidence. The raw
engine pump getter now has an explicit unsafe lifetime contract.
Astra requires explicit registration retirement plus an owned main
task executor retaining task code/target/arguments through settlement. The shared
CFRunLoop drain helper lives under tools/tests/support and is deliberately not
connected to runtime shutdown; servicing it cannot prove image quiescence.

The Darwin scanout composition-fence monitor now owns its FIFO, broker FDs,
worker and generation publication behind a separate resource interface. Surface
creation/scanout/destruction use that owner; surface state no longer exposes its
queue/mutex/thread flags. Stop rejects new admission, settles polling/callbacks
and joins before closing pending descriptors. Actual-object sanitizer tests and
both product link maps verify this integration, not fresh APK behavior. The
Display backing allocation now has a move-only Metal/IOSurface resource owner
used by actual creation and resize; existing texture reimport remains scanout
mechanism. Real GPU ownership/failure tests and both product builds pass, not
fresh physical APK acceptance. The remaining surface bridge is still 1,500 lines and needs further responsibility
splitting; this extraction alone does not satisfy the oversized-owner exit gate.

Chromium launch policy is verified: the normal APK launcher enforces mandatory
Graphite/Dawn Vulkan and a regular absolute MoltenVK provider, rejecting GL,
direct Metal, disabled GPU, and disabled Graphite. This proves launch policy
only. Fresh webpage body, tabs, menu, and physical Vulkan rendering are still
open; earlier renderer `_exit(0)` captures are not webpage evidence. Core window
package-name titles also remain a resource/metadata defect.

Current source status is narrower and more recent:

- The graph boundary gate now rejects every nested `probes/` path, not only
  `probes/runtime_*`. The current recipe/Cargo-derived provider manifest makes
  both product closures PASS with reachable probes inputs=0; actual-read
  completeness review remains required. Earlier narrower gates alone did not
  prove this completion requirement.
  UnixFileSystem, async-close, libcore Linux and SurfaceFlinger production/audit edges are
  separated and independently verified (latest checkpoint). These transitive
  inputs are not evidence of fixture code linkage.

- Receiver JNI ownership is now extracted into receiver_jni.cc/.h. The actual
  RegisterInputNatives caller delegates with three immutable scheduling hooks;
  channel_owner.cc is 107 lines and does not inspect receiver-private state.
  Astra approves registration reentry, atomic publication, rollback and local-
  reference cleanup. The later receiver_lifecycle adoption also removes numeric
  Init/Dispose/rollback mutations; current two-flavor build/link gates pass.
  Fresh APK acceptance remains OPEN (latest checkpoint).
- Channel resource/JNI split is product-linked in both flavors:
  InputReceiver/policy retain JNI-free InputChannelResources; weak core registry
  is independent of Binder wrapper globals. Root added a weak Java-token/core
  catalog with explicit admission/clear before VM destruction and reconstructed
  wrappers around retained cores. Native channel/Parcel ownership now lives in
  input_channel_jni.cc; receiver/root adapter still has integration debt.
  Captured normal RegisterNatives invokes actual read/write/dispose/finalizer;
  ASAN/UBSAN44130 passes receiver-only retained-core reimport, duplicate-FD
  cleanup, mismatch/closed rejection and provider exception/failure handling.
  Astra found the two Parcel exception gaps now fixed. Serialized43433 terminal0
  compiles shutdown/catalog changes and links both flavors; exact warm8e1967
  is no-op and source/link/fixture-export guardsdd7db6 pass both flavors.
- ViewRoot JNI geometry/focus/dispatch use scoped JNI
  references and receiver admission, preserve pending exceptions, reject
  reentrant disposal before event handoff, and use opaque routing/finish/
  progress helpers. Mocked-JNI behavioral test98303 passes.
- Receiver binding's independent retirement Control handle and quiescence
  checks pass sanitizer test75453 after Astra's member-lifetime and skipped
  cleanup corrections. This is component evidence, not product handoff.
- Admission now has a resource-only retirement handle that waits for elected
  JNI cleanup completion. Gate tests1437 and real JNI-adapter sanitizer12733
  pass (FD binding mocked there, verified separately); Astra approves this
  bounded component, not coordinator/FD handoff integration.
- Graph917 explicitly includes current ViewRoot, EGL error and transport authority
  sources (digest `6af2eae926e41218`). Serialized Ninja32473 links both flavors
  at terminal0; current source/link/fixture-export guards and exact warm
  no-op61784 pass. These bytes have not passed fresh APK acceptance.
- Exact retirement handoff is now adopted by receiver JNI. Typed publication, opaque exact
  tickets, reserved history, allocation-failure rollback and local/remote CANCEL
  FIFO retry now have production-object tests. JNI-free lifecycle, resource-only
  finite-TX barrier and independent pending retention pass bounded tests. Init,
  Dispose and rollback now use receiver_lifecycle and independently retained
  driver ownership instead of numeric routing mutations. Astra source review,
  actual published-epoch rollback and resource-only hook retention tests pass.
  Fresh APK behavior and a CANCEL-only OOM sensitivity proof remain open.
  OUTPUT-only pump leases now preserve initial read authority and never consume
  RX. Writable/rearm preserves that mask; observed terminal readiness retires
  even no-remote resources. Dormant zero-interest peer-close detection remains
  open: actual Darwin poll ignored a closed socket with events=0.
  Pump quiescence now includes admitted input/progress/terminal callback work;
  once-only metadata completion is outside locks. Binding's weak completion
  hook preserves retirement notification after callback-tail work ends.
- Existing default profile was mounted with ordinary `ctl ensure`; no reset
  or APK modification. Fresh Calculator acceptance71771 failed: visible window
  is black, WMS publishes720x1280 but SurfaceFlinger has layers0 and no app
  setBuffer. Physical click57875 reaches framework and is handled; UI Looper
  and RenderThread are idle in sample83118. App remains open for diagnosis.
  Astra identified native HWUI initialize bypassing the EGL display owner,
  followed by an untracked pbuffer fallback. Root removed this fallback and
  native-dequeue-to-host substitution; backend sanitizer82513 passes. These
  edits and standard lifecycle dispatch are linked in product66344; both guards
  and warm no-op79058 pass. DeskClock96660 was rejected because changed
  system-runtime settings cannot replace an active runtime; no app was stopped.
  Pre-fix `nm` confirmed `_eglInitialize` imported directly from libEGL;
  product66344 now defines the runtime wrapper. Physical output remains unverified.

## Acceptance commands

Run focused boundary tests after changes, then the serialized full gates:

```sh
bash tools/bionic-process-state-facade/audit.sh
cargo run -q -p art-bootstrap -- audit-runtime-graphics-link-fast
bash tools/aosp-core-apps-graphics-acceptance.sh
bash tools/android-window-menu-acceptance.sh
bash tools/android-window-keyboard-acceptance.sh
bash tools/audit-art-jit.sh
bash tools/audit-profile-daemon.sh
cargo test --workspace
```

For Chromium, also run the launcher-policy regression and fresh physical
Graphite/Dawn Vulkan webpage/tab/menu acceptance. Do not infer webpage content
from device creation, native NTP, process startup, or a stale screenshot.

## Verified applications and completed contracts

| APK / area | Verified evidence or bounded contract |
| --- | --- |
| AOSP Calculator | Physical CGEvent input, `2+3=5`, history/menu/resize on the earlier product family |
| AOSP DeskClock | Launch, labels, Timer popup/input on the earlier product family |
| AOSP Calendar | Day/Week/Month and keyboard-popup selection baseline |
| Chromium | Earlier toolbar/menu/tab-grid/process stability baseline; fresh Vulkan webpage acceptance remains open |
| Blue Archive | Unmodified title/login/Notice UI and synthetic Cancel transition only |
| SolitaireCG | Custom View drag consumed without crash |
| Runtime contracts | ARM64 signal/ucontext resume, SurfaceFlinger transforms/Retina projection, framework services, JNI attachment/shutdown ordering, and native graph fixture-closure guards |

## Known limits

- Offscreen surface destruction has no NSWindow willClose callback and does not
  automatically cancel an active synthetic pointer stream. Product-linked
  destruction tests prove callback-tail lifetime safety, not cancellation
  settlement. Explicit-cancellation regressions do not close this separate debt.
- Evidence is APK-scoped, not full Android compatibility.
- Normal installed-APK and service processes end through host `_exit()`, not
  DestroyJavaVM/ELF teardown. Retained input owners therefore live until kernel
  reclamation. `DARWIN_ART_FORCE_EMBEDDED_SHUTDOWN` with live input is unsupported:
  reusable teardown still needs sealed Init admission, in-flight callback/init
  completion and full pending/claimed-task quiescence before ART/provider
  destruction; an empty pending list alone cannot prove this safely.
- Provider inputs are a reviewed manifest for the current recipes, not a generic
  compiler dependency proof. Literal shell paths are checked; dynamic paths,
  build.rs reverse coverage, alternate Cargo TOML forms/workspace inheritance,
  and symlink closure still need stronger fail-closed validation. Missing
  materialized trees and Cargo path canonicalization can currently be skipped.
  Audit dispatch recognition does not prove shell control-flow exclusion.
  Re-review actual reads when changing those forms; a probes-free graph alone
  does not prove complete rebuild invalidation. liblog remains an externally
  materialized graphics-foundations input, unlike graph-owned ICU archives.
- Blue Archive login/download/gameplay needs external account/server state.
- macOS camera, Bluetooth, Touch ID, and complete host-font unification remain
  provider work.
- Chromium and other large apps need longer soak, memory, performance, and
  physical-interaction gates.
- Unsupported Binder transactions must fail honestly, never return invented data.
- Device creation is not Chromium webpage acceptance; required NDK input/sensor
  imports remain explicit debt and cannot simply be removed.
- Binder authority-loss shutdown still needs seal/admission rejection, pending
  request settlement, DEAD_REPLY drain, reader wakeup and permanent Closed ioctl
  mapping before unregister. Process error-exit ownership is not this contract.
  Serial Binder suites pass; earlier parallel VM-mapping/shared-lock failures
  remain unresolved and are not a green workspace-test claim.

## Next boundaries

1. Preserve the actual patched JNI recipient-query acceptance with genuine
   BinderProxy actors (`tools/tests/binder-recipient/run.sh`, explicit real boot
   inputs and production runtime-support DEX). Link/presence, failed unlink
   retention, retry/absence, local/unsupported and pending-exception identity
   now pass in isolated ART. This is not wire-obituary proof.
   Phased shared process demand/capability handoff now passes current-source race
   fixtures and both production closures. Captured, per-connection ordered
   notifications and unlocked phased transport are adopted;
   `bash tools/tests/am/run-active-services.sh --unlocked-notification-boundary`
   now passes in the controlled component fixture. Preserve separate genuine
   inbound completion and stale-failure fences. Include absence, presence,
   unsupported and pending-exception cases; injected query tests alone are not that proof.
   Exact-client ledger and ordered death detach
   are adopted; preserve live/shared/system bindings and cancel unneeded prepared
   replacements. See the newest checkpoint; no daemon PID sweep.
2. Fresh unchanged Calculator, DeskClock and Chromium physical GPU/key/pointer
   acceptance; restart only the apps explicitly approved by the user. Existing
   PIDs are not fresh-runtime evidence. Chromium must remain Graphite/Dawn Vulkan.
3. Diagnose actual Chrome body/tab/menu/focus failures at Android owners, without
   APK/profile resets, synthetic-input acceptance or a rendering fallback.
4. Preserve the actual pinned Skia borrowed-texture callback execution gate
   (`tools/tests/skia-borrowed-backing-test.sh`); it now passes. Rerun it when the
   callback/owner changes, not as a substitute for fresh APK verification.
   Final product map/resize/source/link/graph/warm gates now pass (latest checkpoint).
   Verify fresh physical resize/input Retina behavior rather than repeating these
   unchanged component gates. Preserve caller quiescence; snapshots do not make
   concurrent context destruction safe.
   Bounded Astra source audit finds zero additional must-fix mixed owners or
   Probe bypasses in changed entry/registration/input/GPU/shutdown paths. Do not
   split coordinating files solely for length; prioritize fresh app interaction.
   Audit other oversized mixed owners before goal exit; canonical ingress
   registry/lifetime and AppKit title/icon extraction are adopted components.
5. Complete retained parent/process cleanup and genuine wire-terminal progress
   where required by this APK-scoped path. Sealed wire gates already reject;
   timer expiry is not a wire-terminal notification or Java death callback.

### Retained design constraints / older sequencing

The constraints below remain useful; earlier "next"/unconnected statements are
superseded by Current status and the latest checkpoint, not separate new goals.

The shared exact-attachment binding owner is now injected into desktop-root and
window-manager endpoints. Exact original InputChannel tokens support ADD before
root registration and later windows without selecting an ambiguous/latest root.
Binding preserves effective attrs, canonical parents, display and ordering.
Adopt original endpoint leases and retained publication batches only with exact
terminal-undelivered settlement, real progress scheduling and safe rollback;
Release alone is not TX settlement. WMS activation uses fresh host authority;
physical keys still need root-event/selected-channel/receiver correlation.
The Java terminal-settlement/quarantine contract and real native original-lease
TX termination/quiescence, flush and accepted-prefix query ports are adopted by
actual Session geometry/focus publication and a real WMS HandlerThread. Next
complete first-key decision correlation, plus parent-subtree and process-death
cleanup; do not count component WMS publication as physical APK acceptance.
A terminal progress hint is not proof of
quiescence; false/busy still needs retry. Do not substitute dispose/release/true.

Immediate next slice: native decision/local-root/readiness joining and bounded
first-key retention, then removal of Init's focus shortcut. The explicit
ATTACH_FOCUS_DECISIONS_V1 subscription is now adopted through
the authenticated root capability. A typed lifetime Binder callback pins server
PID/UID from that trusted handshake; immutable decisions contain root incarnation,
fact serial, per-root decision sequence, original channel token (null revokes)
and epoch. Snapshot after every focus-changing mutation; send outside WMS locks.
The trusted reply uses Process.myPid/myUid, not Binder's attaching caller.
Keep the same typed callback across observe retries; reject replacement for now.
Bound early callbacks with observed caller identity until handshake validation,
without advancing highwater for untrusted records. Capture grant fact serial
from exact policy activeFact, never stamp selection with a newer unreconciled
root.latestFact. Notify displaced roots as well as the mutation's root; retain
subscriptions independently of bound windows. Null selection carries retained
per-display loss epoch. Callback death must not retire unrelated WMS resources.
Use existing native ChannelIdentityCatalog to resolve the exact original token,
whose Java token is now pinned for receiver lifetime (the catalog reference is weak).
Join the decision with existing delivered-recipient/epoch/cache readiness,
retry on either arrival, and retain bounded initial keys until that exact join.
Revoke locally on resign/close; no fallback to another ready channel. Only then
remove Init SetInputRoutingFocus and verify fresh unchanged APK physical input.
Reviewed native adoption contract: retain the exact surface-owned root in the
input sink context, rather than reacquiring an ambiguous process/current-GPU
root per key. That retention is now adopted by the installed Android sink;
it does not yet gate routing. A JNI-free input owner must consume authenticated exact-channel
decisions and local stamps; token resolution stays outside input locks. After
publishing a changed local stamp, the provider must synchronously invoke an
independent input hook outside its mutex, before Java/cancellation tails.
Resign stamp preparation is now adopted before cancellation with Java fact
delivery afterward. The provider now supplies independent synchronous stamp
subscription, now consumed by the installed sink's retained input authority.
Its local state cutoff is implemented; key-routing adoption remains missing.
The actual Binder transport now supplies an exact channel-generation lifetime
captured by NewRemoteBinder and sealed by terminal retirement. Next retain it
in native input authority and establish input revocation outside wire locks;
existing generation checks do not themselves provide that cutoff. Never
reconstruct authority from an FD-only
lookup, recreated connection entry or permanent retained-history archive. This
requires no new wire messages or full Binder-node death semantics. RemoteBinder's
inherited local linkToDeath is not a native death fence; Java notification can
supplement the lifetime object but must not be its source of truth.
Reviewed implementation order (Astra): action reservation/send/completion and
stream/CANCEL retirement now have their own compiled owner behind named ledger
interfaces. Next add a JNI-free per-exact-root key authority and a narrow typed
WMS decision JNI adapter; do not add those responsibilities to either ledger.
The exact root provider's weak native subscription and RetainEvents port are
now adopted and tested. The input authority must subscribe once per exact root,
filter obsolete stamp_revision and make closed sticky even at saturation.
Capture the proxy's exact wire lifetime before subscription/early-callback drain;
resolve original channel tokens outside input locks. Publish decisions outside
Java monitors; newer unresolved selections revoke the old grant immediately.
Carry one immutable authority ticket through proposal, reserve, send, completion
and the local FIFO's pre-Java admission; never retarget queued keys to a successor.
Independent root-stamp hooks run after provider unlock but before cancellation/
Java tails. Wire terminal notifications run after the outermost registry unlock,
including nested retirement and temporary wait-unlock paths; no callbacks in Seal.
Bound pending keys by activation revision, capacity and scheduled monotonic expiry,
with coalesced retries from genuine decision/readiness/capacity progress. Finish
one complete production-root vertical slice before removing Init's shortcut;
component identity decoding alone is not key-admission adoption. The canonical
JNI-free RootKeyAuthority is now retained by the installed sink; exact client
target retention and typed server-generation capture ports now have a production
caller in the separate WM decision JNI adapter. Newer scalar records revoke old
routes before JNI; exact-record retries pin their original token envelope.
Unresolved identity returns RETRY, retaining real WMS delivery/backoff rather
than acknowledging without a native progress driver. Retention/resolution is
not receiver readiness; actual key ticket carry/admission remain next.
Revocation linearizes at the synchronous hook's input-domain -> authority update,
not at its earlier provider-mutex stamp commit. A send/Java admission winning
the domain first is concurrent with notification and may finish; after the hook
cutoff every send/pre-Java admission must reject the old ticket. Proposal,
reservation or FIFO claim alone is not irreversible delivery admission. Pin the
exact continuously subscribed facade before the domain transaction and retain it
through admission. Initialization/recreation samples the provider outside input
locks; a provider snapshot parameter is not required on every routing guard.
Input-domain -> key-owner locking establishes the revocation cutoff. Routing
proposal, reservation, send admission and completion must carry/recheck root
revision, decision sequence, server-authority generation and the existing
successful recipient/epoch/cache fence. Brief provider snapshots precede
channel locking; endpoint IO stays outside locks. Already-admitted IO is not
retroactively unsent. Initial keys have explicit capacity/time bounds and exact
activation-revision ownership; assigned keys cannot retarget to a successor
channel/epoch. Retry coalesces on decision/readiness/capacity changes, with hooks
after routing locks release. This is reviewed design, not adopted native code.
The receiver hook uses a committed/current resource and the real MainLooper;
native shutdown tracks queued work, admitted JNI and context retain tails.
Do not confuse scheduling/FACT retention acknowledgements with focus grants,
skip cross-process foreground freshness, or bypass the first-key barrier below.
Receiver disposal alone must not retire the process root. Uncertain REGISTER
transport outcomes must remain explicitly terminal, not blindly re-register
against the non-idempotent endpoint. Actor-failure reusable recovery remains
unsupported; normal shutdown retries require the AppKit pump to keep running.

Priority from fresh acceptance and architecture review: adopt native first-key
admission on the real authenticated WMS decision producer, not another
speculative EOF prerequisite. The existing
WindowFocusRegistry has a production Session geometry/root-binding caller and
an authoritative activation caller in WindowRootActivationPolicy. Typed
geometry/focus JNI and retained batch delivery are adopted for WMS focus
publication, not root-correlated first-key admission. Physical pointers reach
Java; historical AppKit keys failed at AndroidKeySink with NO_TARGET. Genuine
host-root activation now feeds retained batches and native PublishFocus;
fresh real receiver/first-key delivery remains unverified. Do not fabricate
focus at add/init or bypass the dispatcher. Recheck real URL entry, webpage body/tab transitions and
localized host labels. EOF retention remains required independently below.

Reviewed vertical path: genuine AppKit become-key/resign/close events carry the
bound desktop-root incarnation and ordered event serial to the original owner
Looper, then an authenticated Binder WMS activation owner. Keep title metadata
separate from focus. Bind roots and sessions to checked application attachment
snapshots (PID/UID/app-thread/start sequence); existing ApplicationProcessRegistry
checked snapshot methods are the starting point, not PID-to-package lookup.
Checked session identity and canonical per-registration admission are now
adopted by production. Genuine desktop-root registration, display/attached-parent
validation and relayout frames/flags/order are now adopted by the actual shared
WMS owner; activation consumption is adopted and first-key correlation remains OPEN. Retained batches
must keep predecessor server channels alive through loss/hide settlement;
remove/replacement must not immediately dispose their pending endpoints. Drain
outside WMS locks to the recorded channel, not the current window map. A terminal
endpoint needs explicit undelivered settlement, never ackAccepted; successor
focus must not remain blocked forever. Wire existing receiver onFocusEvent,
then remove Init-time SetInputRoutingFocus. No activation from add/visibility or
metadata title selection. Physical first-key admission must order behind genuine
activation, not synthesize a provisional grant. These links remain OPEN.

Latest Astra source review identifies an additional first-key barrier: AppKit
currently enqueues keys directly. Receiver readiness already commits the exact
delivered focus notification (recipient/epoch/cache revision); do not implement
that gate again. It does not correlate a desktop-root activation with shared
host foreground truth. Owner-Looper delivery of activation alone therefore
cannot establish input ordering. Correlate the
exact root event serial with the authenticated WMS-selected original channel/
epoch and committed receiver notification; resign/retirement must revoke that
root's key admission immediately. Epoch-only focus control is not root-event
correlation. An authenticated asynchronous decision callback may supply that
mapping without changing the wire version. The implemented AppKit root-lifecycle
event owner supplies immutable incarnation, checked
monotonic serial, become-key/resign/close, bind-time actual isKeyWindow snapshot
and retained callback context. This supplies host facts, not Android focus
policy, and alone is not a Chrome NO_TARGET fix.
The surface is created before ActivityThread.main prepares its Looper; native
registration therefore cannot synchronously attach an Android owner task.
The first genuine app InputEventReceiver initialization is the reviewed next
attachment hook: marshal a narrow WM root-client operation to the real process
main Looper, register the existing root with an attachment-bound WMS capability,
and do not grant focus there. Graphics pump and metadata are not startup/focus
authorities. Cross-process activation needs a shared, freshly validated host
foreground authority; registration scaffolding alone cannot supply that order.
Per-root serials do not order delayed activation reports from different app
processes. Define authoritative cross-process host ordering/revalidation before
adopting Binder arrival order as foreground truth. Exact AMS attachment death
must settle its original root/windows without touching a reused PID successor.

2026-09-18 Astra vertical-path review pins the genuine attachment hook to
InputReceiverInit only after binding.Activate and initialization.Commit succeed
and the exact receiver remains current. A WM client must coalesce onto
Handler(Looper.getMainLooper()), never infer process-main affinity from that
receiver's message queue. Register the checked WMS capability before async
AppKit binding because Bind emits a real snapshot synchronously. Queued binding
must retain exact root authority, not g_active_gpu_surface/raw surface; avoid
synchronous cross-loop waits. Root lifetime is process-owned, not first-receiver
owned. These ports are not yet implemented. Shared host foreground validation
must tie the host process identity to the Android attachment and provide global
freshness/revalidation, not a cached frontmost PID. Terminal publication
settlement must also allow explicit undelivered progress without ackAccepted.

Current input work: receiver-epoch original-sequence ACK v2 and local no-wire
settlement are integrated. Bounded retained ACK retry is now adopted by the
live-reader JNI callback, with exact per-attempt tickets and uncertain-send TX
termination. Complete independent EOF completion ownership (both recorded but
unbuffered ACKs and future Java finishes), empty-TX non-writable recovery and
parked/resumable reader progress. These remain independent contracts, not a
demonstrated cause of the live-channel WMS focus failure. Atomic live-sequence collision/wrap admission is now adopted by
the actual ViewRoot caller; current component/product evidence is below. Do not
repeat the removed inbound-ACK-to-Java-finish or channel-global sequence bypass.
EOF completion review: a progress callback alone is insufficient. Coherent
remote-obligation accounting and admission sealing are now integrated; do not
repeat the old recorded-ACK-only completion predicate. An independent JNI-free receiver-epoch
completion owner must remain serviceable after INPUT removal without a strong
receiver/binding cycle. Parked ownership is not retirement/quiescence, and a
resume task must target the original slot. Empty-TX recovery requires bounded
reserved admission or delayed retained recovery, not diagnostic-only handling
or immediate self-requeue. This is a reviewed next contract, not implemented
completion-service evidence.

Immediate genuine-fixture boundary: finish authenticated real-service process
registration/readiness without APK identity spoofing. Explicit AndroidProcess
lifetime and pre-exit observer now exist, but fixture CLI selection is not wired.
Hosted macOS reusable execution is explicitly unsupported until an AppKit
pump/callback quiescence protocol covers
normal shutdown, guard/error teardown and failed-pump cancellation; the current
failure-then-join behavior is unsafe. Do not install process-lifetime Binder in
that reusable path or weaken exit-before-unregister guards.
Ordinary APK RX/WMS focus adoption and fresh three-APK Vulkan/Retina acceptance
remain product priorities; generic reusable embedding is not a substitute for
those gates. Preserve genuine fixture contracts rather than converting them to
one-shot execution or weakening their assertions merely to obtain a PASS.

1. Channel JNI adapter, Parcel regression and shutdown product link now pass;
   domain selection/focus/capture extraction and its current product gates now
   pass (latest checkpoint). Receiver JNI owner extraction now also passes current
   product link. Finish remaining retirement handoff below; preserve domain →
   channel lock order, accepted-only
   capture and atomic ledger revocation. Notifications and final promoted-pin
   release must stay outside both locks; never expose mutable ledger internals.
2. Typed publication and atomic exact-recipient retirement now pass focused
   contracts (latest checkpoint). Registry identity reservation and immutable Init
   preparation now precede visibility. Reserved epoch-history capacity now lets
   exact logical revocation complete without successful allocation; CANCEL
   allocation failure retains a retryable FIFO obligation. JNI-free pinned
   lifecycle component now passes exact-token/deferred-close/failure contracts;
   it is now used by Init/Dispose through receiver_lifecycle. The resource-only retirement barrier now
   combines exact ticket/admitted-send, JNI cleanup, binding quiescence and one
   finite TX-prefix fence; it is a proof gate, NOT a progress coordinator.
   Pending retention now has prepared process-lifetime storage, allocation-free
   enlistment and retained-before-publication guards. Bounded allocation-free
   discovery now resumes stable passes and restarts on membership changes.
   OUTPUT-only pump now preserves initial read authority; real socket/Looper
   tests cover ACK drain, unread RX preservation and observed remote/local HUP.
   Darwin poll ignores zero-interest peer close: do not depend on dormant HUP
   detection for finite-TX completion or claim that contract implemented. The
   isolated CANCEL-only OOM regression and old-predicate negative sensitivity
   now pass (latest checkpoint). Next verify fresh APK behavior on the adopted owner-Looper driver/Init/Dispose/
   rollback path. Retain() must outlive receiver removal and unexpected
   operation failure, not merely exist as a receiver member.
   Never capture then retire across transactions or assign the predecessor
   ticket to its successor (Astra review).
   Live binding now delegates to the reviewed claimed-pump owner; source and
   sanitizer regressions and both product build/link gates pass; fresh physical
   APK acceptance remains OPEN.
   Prepare/reserve before visibility,
   accept deferred intent without publishing readiness, recheck BeginRegistration
   before AddFd, release only after actual pump quiescence.
   Claimed Control must be independently enlisted before Reserve; task and pump
   contexts retain only weak Control tokens. Binding/JNI now expose two phases:
   prepare immutable claimed slot owners before registry exposure, activate after
   routing publication. Actual binding Prepare now reserves claimed slots;
   retired-output driver/JNI adoption is still pending, so full epoch handoff
   is not yet established.
   Retire requests close/work from any thread; Claim operations stay original-
   Looper-only. Root added task-tail on_quiescent metadata to avoid lost final
   binding completion; failed Prepare never announces a task it did not return.
   A receiver arriving during admitted orphan OUTPUT needs an explicit yield hint;
   SubscribeAvailable
   does not wake that orphan. Durable prepared wake/dirty work is required:
   allocating RoutingTransportScheduler plus logging cannot guarantee OOM retry.
   Old product broker EventFdWrite silently accepted failed host signaling.
   Extracted eventfd owner now rolls back failed writes, preserves semaphore
   readiness and uses the actual errno provider; focused real-socket tests pass.
   Astra approves the bounded source; actual guest-broker dup-close-original
   audit76285 passes ASan/UBSan/TSan; current graphics product link passes.
   Headless and exact warm gates also pass for the latest pre-driver snapshot.
   Neither reusable-task mocks nor a later poll-setup retry establish
   durable wake in a running product built from the old provider.
   Driver retains only routing/original transport/resource handles and weak record
   linkage; no receiver/channel/JNI cycle. RetiredTransportDrain must not be adopted
   unchanged: no authority/quiescence integration and terminal events mark only
   drain-local state, leaving the resource's TX fence potentially pending.
   Finish ticket historical retention, capacity/complete notifications and OOM,
   then integrate/test the exact retirement ticket, one-FD authority, successor/
   orphan routing, receiver-admission plus binding quiescence, and outside-lock
   settlement handoff. Preserve FIFO head progress and retired CANCEL semantics.
3. Recheck the latest Astra handler review and run real receiver-destruction/
   callback-reentry tests against production translation units.
4. Repeat both-flavor build/source/link/fixture-export and exact warm no-op
   gates from current bytes; historical passes are not current verification.
5. Repeat fresh physical Calculator/DeskClock Retina input and Chromium
   Graphite/Dawn Vulkan webpage, tab, menu, and exit acceptance. Do not reset
   profiles or alter APKs.
6. Diagnose Chromium renderer `_exit(0)` using the opt-in exit provider/socket
   traces before claiming webpage acceptance.
7. Keep unrelated camera/Bluetooth/font/full-platform work as follow-up rather
   than expanding this goal.
8. Preserve outstanding keyboard/WMS focus and Java pointer-publication barrier
   work; never infer focus from visibility. Compile-only InputChannel signatures
   still retain IOException for the current WMS caller, unlike pinned AOSP;
   resolve caller/signature isolation without altering genuine fixture contracts.
9. Exercise support-manifest source addition, tool-identity invalidation and
   negative support build-graph cases; current warm no-op alone does not prove
   those invalidation boundaries.

## Historical baseline (not final-source acceptance)

The following earlier checkpoints remain useful architecture evidence but are
superseded for final acceptance by later source edits: products228/guards230/
no-op232/core acceptance234; native owner and graph guards; ART overlay/JIT and
JavaVM provider audits; BLAST/surface/transaction, EGL/fence, audio/media,
InputChannel endpoint/transport/pump, routing/dispatcher/scheduler, receiver
registry/binding, shutdown, and Chromium launch-policy component tests. Their
component sanitizer/PASS logs remain in `/tmp` or Git history where cited by
the original turns; never treat their old link maps or APK captures as current.

## Latest progress

**Live BeginFrame completion stall isolated (2026-09-18, PROGRESS):**
Both product boundary audits PASS source/link closure clean, fixture exports0;
reachable graph probes0 and negative-control graph test PASS. Actual retained
layer, SurfaceControl occurrence ASan/UBSan and registry fixtures PASS, not full
APK acceptance. Astra review confirms system14744-owned root1 is legitimate
WMS creator identity preserved through Parcel/BLAST, not browser ownership bug.
Physical CtrlT96626 changes count1→2; repeated tab clicks eventually show real
grid (`/tmp/darwin-art-chrome-current-producer-stale.png`), return89955 shows
Example Domain. First short-delay captures alone do not prove input rejection.
GPU source108 full-byte hash083042d79a8341cf persists through transaction107
while native root UI changes; actual producer progression remains OPEN.
GPU debugger60804 proves native callback3390d18b4 fires now on Viz thread,
impla0008b2e0/sourcea00382140. Debugger25326 catches common entry335bdafb8
and observer call335bdb1e8: source observer count1, observera00443b80,
actual callback3390520d0 (installed ELF c5ea0d0, CFS support OnBeginFrame).
Debugger89173 proves needs flag+198=1, suppression+199=0, pending uint32
tracker+200=100; ShouldSendBeginFrame return/branch33905229c gives w0=0.
Thus clock→source→support is alive, but outstanding BeginFrame tracking blocks
forwarding; do NOT raise/reset the threshold or force frames. Missing completion
cause is not yet established. Browser debugger65311 late resolved slim receiver
318418f54/request helper315a8e310 both hit0; late observation is not startup proof.
Exact installed decrement ELF c5e3260 (live33904b260), x0=observer+200;
DidNotProduceFrame ELF c5e8614 and submit c5e88c0 call it. Debugger11766 reads
client observer+20=a000d7a00, vptr32d38a200 (ELF922200), client+8=a0034d700;
relative virtual slot+c target330f57a90 (ELF44efa90). Next inspect that actual
IPC forward endpoint and browser/native Looper completion, not root PID/clock.
Final bounded Astra review: proxy44efa90 serializes async Mojo ordinal1/flags0,
then calls endpointa0034d700 through wrapper ELF7ee63a8; actual relative Accept
call at ELF7ee6440/live33494e440, return33494e444. Next read endpoint vptr and
signed int32 slot+c, or observe its bool return during fresh admitted sends.
Support increments pending after a void proxy regardless of receiver Accept
success; current evidence does NOT distinguish rejected/lost send from browser
consumer/ACK loss. Do not claim completion responses alone are the root cause.
Review agent retired. Four guests revalidated normal S/PPID47588; no LLDB live.
All five debuggers detached+quit terminal0. Launcher89972 and four guests remain
live; no APK/profile mutation, restart, behavior patch, commit/push or completion.

**Example.com first-priority physical verification (2026-09-18):**
Re-read AGENTS/current goal/latest checkpoint; indefinite launcher89972 remains
live with system14744/browser14748/renderer14800/GPU14804 under daemon47588.
Fresh real-window capture `/tmp/darwin-art-chrome-priority-example-before.png`
OCR confirms Example Domain paragraph/link and example.com address. Original-
detail `/tmp/darwin-art-chrome-priority-tabs.png` visually confirms page,
address bar and bottom controls together. Physical menu click14392 exit0 opens
actual Chrome menu (New tab, History, Downloads, Bookmarks). First tab click
80172 exit0 produced no observed switch; retry17117 instead exposes Find in
page, so tab/input correctness is NOT accepted. Keyboard29139 and49775 exit0
retain Example Domain body, but native address/bottom controls disappear in
`/tmp/darwin-art-chrome-example-priority-final.png` and repeat OCR; native/child
surface composition stability remains OPEN. No source behavior change, APK or
profile reset, fallback, restart, commit or push. Real page render is proven,
not stable whole-Chrome acceptance or complete Probe migration. Chrome stays
running; prioritize native controls/composition regression over unrelated cleanup.

**Repeat capture includes native bars and page (2026-09-18):**
Physical Ctrl-L86616 exit0, Enter70555 exit0; new captures
`/tmp/darwin-art-chrome-example-address-focused.png`, `...-example-repeat.png`
and `/tmp/darwin-art-chrome-example-repeat-latest.png` all OCR Example Domain.
Latest PNG visually inspected with view_image detail=original: actual address
bar/example.com, bottom Chrome controls, Example Domain paragraph and Learn more
link are present together. Default resized tool preview showed black body for
some captures despite source PNG RGBA235/235/235/255 at body pixels and successful
OCR; do not diagnose runtime blackness from that preview alone. Exact pixel check
used read-only NSBitmapImageRep. Native system14744/browser14748/renderer14800/
GPU14804 remain live under daemon47588, no attached debugger/startup-stop setting;
indefinite launcher89972 confirmed live. Renderer14862 subsequently exited, not
counted live. Native page-render priority has concrete evidence; whole Probe
migration goal remains ACTIVE. Earlier blank-generation cause/startup reliability
and full physical Chrome tab/menu acceptance still need verification; do not
attribute recovery to a diagnostic-only logging change.

**Actual example.com rendered on normal Vulkan path (2026-09-18):**
Normal indefinite launcher89972 WITHOUT DEBUG_STOP_AT_CHROME confirmed live.
Prior diagnostic launcher86532 terminal0; fresh system14744/browser14748/
renderer14800/GPU14804/renderer14862 live under daemon47588, no stopped guests.
Native clock registers/dispatches normally on GPU Viz TID39412492 /
Looper0x60000136c320, first64 accepted tasks and progressing timestamps. Physical
URL input92237 exit0/13 events. Real window capture
`/tmp/darwin-art-chrome-normal-clock-active.png`, visually inspected, contains
Example Domain body and Learn more link; OCR independently matches. Native GPU
14804 Vulkan create-device result0/device0x12f85cc18 and real imported AHB
MetalTexture bindings match=true (e.g. buffer0x12bf05660/texture0x12be2e5b0).
Unchanged APK/profile and Graphite/Dawn Vulkan policy preserved, no CPU/GL/
direct-Metal fallback. This is real page-render evidence, NOT whole-goal closure:
capture has empty toolbar region and missing native address/bottom bar; composition
and repeated navigation acceptance remain incomplete. This turn changed diagnostics
only; root cause of earlier blank generations not yet proven, so do not claim a
behavioral fix or stable startup solely from this successful run. Keep Chrome alive.

**Decisive startup observation: native source works (2026-09-18):**
Existing opt-in DEBUG_STOP_AT_CHROME used only for attach-before-entry, no
rendering/scheduling changes. Exact owned97967/97970/98018/98022 TERM; launcher8350
polled terminal0. Diagnostic launcher86532: system8865/browser8867/GPU9239 and
renderer9234; actual ELF image logs give browser base0x300000000 and GPU
0x329750000. Browser/renderers resumed via CONT after exact owned stopped-state
inventory, including later9860/13146. GPU debugger20584 caught VizCompositorThread
root factory0x335d53e5c, provider virtual target0x335d0f2b8, and output result
0x335d53f50: x0=0xa002b7c00 NONNULL. Source ctor0x335dba2c8 then hit with
source0xa00382140/restart1/requires_align=false/refresh60. Post-ctor source+0x188
nativeImpl0xa0008b8e0, Java+0x190=0. SetEnabled0x335dba504 hit true. Native impl
enabled+0x30=1/pending WeakPtr+0x38=0. Real opt-in logs show getInstance,
accepted native postVsync tasks and dispatch on same Viz TID39381942 /
Looper0x600001d95400; first64 bounded schedules/dispatches progress ~16-18ms.
Therefore missing source construction/native timer is NOT established in this
startup. Exact0xc66a700 is the Java/VJJJ-side OnVSync entry, not proof of native
callback failure when hit0. Browser debugger65251 actual slim Mojo receiver
0x309ee4f54 resolved/hit0 during physical URL input39975 exit0/13 events.
Screenshot `/tmp/darwin-art-chrome-source-startup.png` still lacks Example Domain.
Both debuggers detached/exited. Debug-stop guests8865/8867/9234/9239/13146 then
TERM after exact inventory; normal indefinite restart WITHOUT stop setting is
in progress, handle recorded next. Next diagnose Viz-to-browser request/source
association and Mojo delivery; do not replace this functioning clock or force
frames. Goal ACTIVE, no rendering acceptance/commit/push claimed.

**Native clock diagnostics deployed, Chromium still blank (2026-09-18):**
Previous turn PROGRESS; this turn PROGRESS through real link/deployment and
fresh process/capture evidence. Re-read AGENTS/current goal/latest checkpoint.
`cargo run -p art-bootstrap -- audit-runtime-graphics-link-incremental`
session37856 exit0: graphics bootstrap436 objects compiled1/cached435, final
registrar51/fake-symbols0. Same command37349 exit0, exact Ninja no-work.
Inventoried old system and seven Chrome guests under daemon47588; TERM
47984/47986/48034/48038/57751/63532/65656/82338, all disappeared and registry
emptied. Old launcher43786 polled terminal exit0. Profile/APKs preserved.
Fresh indefinite normal launcher8350 confirmed live; system97967/browser97970/
renderer98018/GPU98022 live under same daemon, child98062 exited subsequently.
Installed APK SHA remains2aaea8419d955677313f8b6dae3f0666916243ec55c3607a8711f46c9123b731.
Native Choreographer diagnostics now linked and enabled at startup. GPU98022
MoltenVK create-device result0/device0x11b03fa18; required Graphite/Vulkan flags
preserved. Fresh startup contains no native Choreographer diagnostic entries or
AChoreographer dlsym requests observed; absence does not prove source ownership.
Physical URL keyboard28349 exit0/13 events. Fresh captures
`/tmp/darwin-art-chrome-example-fresh-native.png` and `...-fresh-latest.png`
show example.com and WHITE blank body, NOT Example Domain. Retinal mapping
remains720x1280 raster for360x640 logical content. sample73465 exit0: browser
CrBrowserMain and GPU VizCompositorThread39315876 both await Java MessageQueue
->ALooper poll; no missing-thread/Looper conclusion supported. Browser ELF base
0x30e534000, fresh GPU0x31ca68000 (not prior0x32ca68000). Late selector64 helper
break9419 resolved/hit0 during physical tab input; detached/exited. No behavior
fix/acceptance claimed. Next: exact Viz root/source construction and request
delivery before changing enablement, frame clocks or scheduler policy.
Read-only Astra exact-ELF review found root factory0xc603e5c; after its provider
CreateOutputSurface virtual call,0xc603f50 tests x0 and exits without a source
when null. GPU98022 live addresses: factory0x32906be5c/result0x32906bf50;
source ctor0xc66a2c8/live0x3290d22c8, SetEnabled0xc66a504/live0x3290d2504,
OnVSync0xc66a700/live0x3290d2700. Source+0x188 nativeImpl/+0x190 Java reference;
nativeImpl+0x30 enabled/+0x38 pending callback. Late debugger33053 resolved
ctor/enable/vsync all hit0 during physical tab input; detached/exited. Startup
output-surface result remains unobserved, NOT proven null. Next decisive check
must catch factory entry/result rather than changing a possibly uncreated clock.

**Chrome priority: attached sink, missing observed BeginFrame (2026-09-18):**
User priority remains actual example.com rendering, not unrelated migration.
Exact unchanged APK disassembly identifies tree0xa00742ee0 as cc::slim
LayerTreeImpl; previous checkpoint's LayerTreeHost label was imprecise.
Browser47986 tree+0x20 holds sink0xa032a7f80, sink+0xc0 binds tree+8, and
sink+0xb8 holds the previously verified raster context. needsBeginFrame+0x268=1,
unacked frames+0x264=0; LocalSurfaceId+0xc8 has nonzero sequence numbers3/2
and token. Scheduler0xa004b2bc0 flags+0xa0/+0xa1/+0xa2=1/0/0;
cached BeginFrameArgs still contains default invalid timestamps/sequence.
Sink+0x70 is a Mojo remote, NOT a Java begin-frame source or local scheduler.
Actual Mojo client interface is sink+0x18. Exact ELF OnBeginFrame thunk
0x9ee4f54 subtracts0x18 and forwards through body0x9ee4df4 to scheduler+0xc.
Browser debugger79300 resolved this receiver, hit0 during physical URL typing;
fresh capture `/tmp/darwin-art-chrome-example-priority.png` shows native UI
and example.com text but BLACK page body, NOT Example Domain acceptance.
Astra review corrected Java-only assumptions: upstream source can choose
native AChoreographer, with Java/VJJJ as fallback; exact APK branch/owner
still needs verification. GPU debugger25519 resolved native postVsyncCallback,
postFrameCallback64 and DispatchCallbackTask, all hit0 during bounded physical
tab input; this does not establish construction/enabled state or process owner.
All debuggers detached/exited; browser/GPU preserved live. Luna completed bounded
opt-in native provider diagnostics (callback/data/Looper/thread identities,
enqueue result and rejected schedules, first64 cap), without scheduling changes.
Main `xcrun clang++ -std=c++20 -fsyntax-only -Icompat
-I_aosp/frameworks/native/include compat/looper/android_choreographer_owner.cc`
PASS; diagnostics not yet linked/deployed, so no new runtime evidence claimed.
Next: trace actual source construction/enablement and Viz request delivery,
correlating callback/data/Looper identities; do not inject frames or force-enable.
No rendering fix or product acceptance claimed; goal remains ACTIVE.

**Chrome native visibility verified — Viz delivery fork (2026-09-18):** previous
goal turn PROGRESS; this turn PROGRESS (fresh native state excludes hidden host).
Re-read AGENTS/current goal/latest checkpoint; launcher43786 confirmed live by
same handle. Actual selector64 helper0x315a8e310 hit CrBrowserMain, not an
unfiltered multiplex call. Browser47986 nativeView0xa000b37d0 -> compositor
0xa00573180: Surface format-3/720x1280, nativeWindow0x1330ef710, surfaceHandle1,
rasterContext0xa00618350, LayerTreeHost0xa00742ee0. Host+0xbc=1 (visible),
drawPaused=0, pendingFrames=0, frameSinkRequestPending=0, submittedSinceVisible=0.
Later OnGpuChannelEstablished/InitializeViz/DidSubmit breakpoints had no hits
during physical input; this late observation does not prove startup binding
failed. Exact ELF0x5150bfc..0x5150c84 only stores rasterContext+0x58 after
BindToCurrentSequence returns0, then calls InitializeViz; nonzero context is
stronger than allocation alone, but does not prove an attached usable sink.
Astra review points to actual APK ExternalBeginFrameSourceAndroid setEnabled ->
own Choreographer -> J.N.VJJJ selector2. GPU sample20240 exit0 confirms
VizCompositorThread in Java MessageQueue/ALooper poll, not missing Looper setup.
GPU image base0x32ca68000 confirmed by matching exact VJJJ export instructions
at0x32fc6aa44 to unchanged ELF. GPU debugger66349: VJJJ and receiver schedule
breakpoints both resolved/hit0 during bounded physical input. Whether a Viz
source is enabled remains UNKNOWN; do not fix clocks solely from no late hits.
Both browser45597 and GPU66349 debuggers detached/exited; latest physical
capture `/tmp/darwin-art-chrome-viz-check-current.png` returns native NewTab UI
and focused example.com text, NOT Example Domain. Browser/GPU remain live.
Found acceptance debt: tools/chromium-android-acceptance/run.sh passes
APK_APP_INTENT_ACTION/URI, but only old probes/runtime_app_activity.cc consumes
them. Current production ScheduleActivityLaunch constructs MAIN/LAUNCHER.
Do not count this obsolete script as real VIEW navigation or restore Probe
environment/lifecycle bypasses. Next decisive check is initial usable sink /
enabled begin-frame source, plus actual omnibox Enter navigation. No source
behavior change/native build/commit/push this turn; full goal stays ACTIVE.

**Chrome rendering first — wait-owner deployment (2026-09-18):** previous
turn PROGRESS; current turn PROGRESS, not webpage acceptance. Main took over
the interrupted Luna wait-owner work. Real child tests exposed proc_pidinfo
arg=0 treating an unreaped zombie as absent; arg=1 preserves its birth/state
and permits actual reap. Every wait/signal rechecks birth and parent ownership;
foreign-parent observations do not permanently revoke a returning child.
Completion remains attached through worker creation failure; retained transfer
slots cancel startup gates before owner Drop, while successful bound-service
handoff no longer cancels its healthy gate. Separate read_live rejects zombies
for peer authentication, identity and readiness without losing child resources.
Latest full profile tests9893: exit0,125 passed,5 ignored (real subprocess
fixtures); release profile binaries43616: exit0. Astra final bounded source
review found no must-fix in this split or the three transfer-slot paths.
Old Chrome/system children were gracefully terminated, daemon shutdown caused
one overlapping launcher failure77992. Profile/APKs were preserved; ensure
restored the mount and new release daemon47588 is live. Normal indefinite
Chrome launcher43786 remains active; system47984, browser47986, GPU48038
and renderer48034 are registered. Vulkan VkDevice creation returned0 on
Apple M2 Pro. Physical keyboard38226/27506 delivered14/16 key events but
captures show NewTab/focused URL, not Example Domain. Physical resize restored
720x1280; latest `/tmp/darwin-art-chrome-priority-current.png` is black,
not successful render. Both browser debuggers23324/51019 detached/exited;
browser ownership returned to daemon47588, GPU remains live. No fresh
rendering success claimed.
Priority remains unchanged APK example.com through Graphite/Dawn/Vulkan/
MoltenVK/Metal. Next: fresh browser native LayerTreeHost visibility (+0xbc),
frame-sink/frame submission evidence, physical URL input and Retina capture.
Do not substitute component PASS, native NewTab UI or HTTPS sockets for actual
Example Domain body rendering. Existing pre-registration cleanup remains debt;
full production probe-removal goal stays ACTIVE. No commit/push this turn.
Diagnostic correction: VJ64 means multiplex J.N.VJ(selector64,nativePointer),
NOT every call at exported VJ wrapper0x31fc60c. Unfiltered hit was selector473
on Chrome_IOThread with a Java-like object, so it cannot establish compositor
state. Exact unchanged APK JADX CompositorView maps selector64; ELF jump-table
entry0x71f2a4=0x23c resolves branch0x31fcf2c to helper0x755a310, which reads
compositor at nativePointer+0x18. Use this helper (current live0x315a8e310,
x1 nativePointer) or selector-filtered wrapper; no target function execution.

**Immediate resume — application wait owner connected (2026-09-18):** previous
turn PROGRESS (retained layer9 and draft safety review). Main replaced the
OP_DAEMONIZE unconditional Child::wait/on_exit path with
daemonized_child_wait.rs: same incarnation as registry lease, sole raw waiter,
terminal-only callback, retained transfer on worker creation failure. Template
registration failure requests cancellation but does not fabricate child exit.
Astra bounded review found no additional must-fix in this new happy path;
pre-registration Child kill/wait remains explicit debt, not claimed fixed.
New genuine child success and injected worker-spawn failure tests are added.
Main cargo check85969 exit0 for its snapshot. Targeted tests10757 exit101 while
Luna was editing PPID observation (process_incarnation.rs:71 wrong return type);
no tests ran, no acceptance PASS. Cargo is now released to Luna for stable
handoff/testing. Astra corrected permanent ECHILD/live probe-only advice: it
strands returned zombies. Same kernel birth/state/PPID snapshot must allow
actual reap when the original child returns to this parent; foreign-parent
state remains nonterminal/no signal. Do not attach debugger before that fix.
Current Ninja -t inputs BOTH products succeeded:10063 rows, probes-input0;
nm exported Fixture/Probe/probe_/fixture_ search had no matches. This verifies
current product input/export boundaries, not all final cleanup or APK behavior.
Real browser96603 HTTPS sockets are established, but network/page success is
unproven. Read-only sample72630 completed at
`/tmp/darwin-art-chrome-render-priority-browser.sample`: CrBrowserMain is in
native MessageQueue/ALooper poll; do not call that a demonstrated deadlock.
Physical Ctrl+T/Ctrl+L/typed example.com/Enter54814 PASS, fresh captured screen
still reports New-tab/example.com/attachment sheet; NOT webpage acceptance.
Chrome remains live; no APK/profile changes, debugger or GPU fallback.
Goal ACTIVE, actual example.com body first; fresh C/LayerTree+0xbc read next.

**Immediate resume — native page versus retained sheet (2026-09-18):** prior
turn PROGRESS (fresh physical navigation capture); this turn inspected current
processes and exact wait-owner draft, not acceptance. Browser96603/GPU96663 and
system96598 remain live; old launcher7010 is now absent, do not describe it as
live or restart merely because its observation handle ended. Real physical
mouse drag changed the focused omnibox back to Ask-Google state; screenshot
`/tmp/darwin-art-chrome-sheet-physical-drag.png` was freshly inspected. Attachment
sheet still appears. Current transactions84–90 retain global layer9/z11000 at
[0,767,720,1280], same buffer hash; this identifies retained composition state,
not proof of why Chrome did not dismiss it. Renderer7848 was created following
typed navigation; actual document-load success remains unverified. Root/HWUI
EGL window queues must not be mislabeled as Chrome Vulkan compositor frames.
Luna shared process-wait implementation is in progress. Main draft review
flagged lost callback ownership on quarantine spawn failure, signaling after
live ECHILD, and post-terminal re-wait/PID reuse; sent corrections before tests
or adoption. No debugger attached, no builds overlapped, no APK/profile changes.
Next remains exact LayerTreeHost+0xbc visibility / frame-sink submission once
host wait safety is integrated; original webpage failure is not fixed.

**Immediate resume — example.com remains first (2026-09-18):** user reiterated
real Chrome webpage rendering priority; goal remains ACTIVE, not acceptance.
Read-only VJ64 inspection of the previous browser generation confirmed native
child window `0x116c24e20`, surface handle2, raster context nonnull, draw-paused0,
frame-sink request-pending0 and submitted-since-visible0. Exact APK vtable and
disassembly establish LayerTreeHost::IsVisible at host+0xbc; that byte has NOT
yet been read. Next decisive check is actual visibility/frame-sink/begin-frame
delivery, not another fabricated Java callback or alternative GPU backend.
Debugger incident is tainted: GPU received SIGKILL during target evaluation;
browser later aborted with DeadSystemException. Astra confirmed three real
host-wait defects (live ECHILD deregistration, error-triggered kill, zombie
misclassified as reaped), but the actual incident errno was not recorded.
Luna is implementing shared raw waitpid/incarnation ownership with terminal-only
lease retirement; this is debugger/lifetime safety, NOT a black-page fix.
Old task-owned daemon23785 and its terminal children were gracefully retired
after exact inventory; APK/profile unchanged. Normal launcher7010 is live;
fresh ctlps system96598/browser96603/GPU96663, renderer IDs may change. Daemon
restart replaces darwin-artd.log, so old 128588/131466 line references are not
current log positions. Fresh JNI lookup logs do not prove page-load callbacks.
Physical Ctrl+L/typing/Enter returned HID PASS, but newly inspected capture
`/tmp/darwin-art-chrome-priority-example.png` shows focused example.com text,
New-tab title and attachment sheet, NOT rendered example.com. Physical
navigation/input failure remains open. No shared native build this checkpoint;
previous both-products58336/no-work evidence is unchanged. Do not claim page
success from component tests. No APK changes, profile reset or GPU fallback.

**Immediate resume — Chrome native Surface delivery verified (2026-09-18):**
PROGRESS, not webpage acceptance. Chrome example.com rendering remains first.
Normal separately signed development host permits late LLDB attach without
changing deployment entitlements or APKs. Previous browser52424 VJ64 breakpoint
41144 hit with nonzero compositor-view pointer `0xa000b5350`; exact APK
SurfaceChanged disassembly establishes offsets+0x38/+0x3c/+0x40. Read-only
LLDB68825 reads twice returned `-3,720,1280`, compositor+0x18=`0xa00543180`.
Thus native SurfaceChanged DID run; do not resume assuming Java early-return.
Both debuggers detached/exited; diagnostic physical resize restored720x1280.
Found diagnostic blind spot: cached ANativeWindow_fromSurface returned without
logging. Added existing-debug-flag-only cached-return trace, no behavior change.
Both native products58336 exit0, graphics registrar51/fake0; exact frozen target
repeat reports no work. Graph inputs1128 digest
`d3d0343009b91981afedd1587167473c27f59799cc61f4bdf4ece0de5540f995`.
Astra confirmed real one-way Binder PID0 versus ActiveServices PID lookup bug;
Luna implemented incarnation-scoped callback capability (details below), Astra
scoped review found no must-fix. Main additionally verifies pinned framework
Proxy null reply/FLAG_ONEWAY in run-active-services-contract.sh63823 exit0.
Fresh normal launcher56503 remains live after verified task-owned graceful
restart; ctlps system85539/browser85541/GPU85647/renderers85641/85693.
New cache trace proves actual browser child Surface delivery at log128588:
window=`0x112a3a300`; root/HWUI window=`0x116919f60`. Child has no observed queue
submission; root continues queueing. Fresh log range127400–132500 has no old
Application-process-not-attached/serviceDoneExecuting error (rg no-match exit1
is not sample failure). Vulkan GPU85647 create-device result0.
Physical HID example.com reload41266 exit0/14 key events; fresh screenshot
`/tmp/darwin-art-chrome-service-capability-example.png` shows URL and bars, body
still black. This disproves treating the service fix as rendering success.
Actual GPU sample13966 sampling succeeded:
`/tmp/darwin-art-chrome-capability-gpu.sample`; CrGpuMain conditional wait,
VizCompositorThread native MessageQueue/ALooper poll; no proven deadlock.
Next: native SetSurface→Viz child producer/Surface handoff and frame submission,
not unrelated migration or another Java compositor fabrication. Preserve real
Graphite/Dawn→Vulkan→MoltenVK path. Agents completed/retired; Chrome left live.
Goal ACTIVE; Calculator/DeskClock/latest full Probe-closure acceptance pending.

**Service lifecycle callback capability (2026-09-18):** ActiveServices now
keeps the canonical ServiceRecord token system-server-only and issues a fresh
guest Binder capability for each exact owner lane `(pid, startSequence, thread,
UID)`. CREATE/BIND/UNBIND/STOP transports use that lane token; death retires
the capability before same-PID replacement. `serviceDoneExecuting` accepts the
real framework one-way PID-zero path only when authenticated UID and the live
registry tuple match; positive PIDs retain exact-owner admission. Synchronous
publish/unbind require the live capability and positive owner PID. PID-zero
CREATE/STOP/UNBIND(false), duplicate, wrong-UID, unknown/retired, same-PID
replacement and synchronous rejection fixtures pass in
`run-active-services.sh`; this remains component evidence, not APK acceptance.

**Immediate resume — Chrome SurfaceHolder trace (2026-09-18):** previous turn
PROGRESS (ready cutover/build/live failure); this turn PROGRESS (diagnostic
evidence narrows next action). Astra bounded source/log review completed/retired:
no proven Looper deadlock; Java vsync differs from NDK path; Java FD-listener
no-op is debt, not established cause. Exact verified Chrome children terminated
gracefully for diagnostic restart; no APK/profile reset/backend substitution.
Normal launcher12036 remains live; ctlps system43798/browser43801/GPU43854,
renderers43849/43901. Existing SurfaceView/BLAST/vsync/DSO traces enabled.
Actual callbacks logged for two SurfaceViews: created/changed720x1280/redraw,
then old-format Surface destroyed; no observed Surface parcel/fromSurface.
Thus exclude absent SurfaceView creation, investigate callback→native compositor
attachment next. Physical HID example.com reload12734 PASS; actual screenshot
`/tmp/darwin-art-chrome-holder-trace.png`944x1560 body sample RGB0 alpha1, NOT
webpage acceptance. JADX single-class31087 terminal0 reads unchanged APK into
`/tmp/darwin-art-chrome-compositor-view-20260918.java`: actual callback j lines296+
returns if native y==0, then queries Window root InputTransferToken (303) before
native N.OIIIJOOZ(350). Determine which boundary stops before proposing a fix.
Upstream main comparison shows surfaceCreated format0 is normal UNKNOWN before
surfaceChanged, not evidence of corrupt format; pinned APK remains authority.
Graphics uses original android.util.Log registration; headless log stub is not
this live graphics failure. No source fixes/build this turn. Chrome left live.
Goal ACTIVE, real example.com rendering remains first priority.

**Immediate resume — Chrome rendering first (2026-09-18):** previous turn
PROGRESS (main component gates); this turn PROGRESS. Ready transaction now submits
the prepared immutable snapshot before allocation-free local Finalize, instead
of ApplyAcceptedTransaction/CaptureSnapshot before submission. Healthy explicit
rejection cancels without producer transfer; Unknown, committed error or unhealthy
restoration remains fatal, never retried. Actual ready-owner fixture25961 PASS:
frontend rejection, central rejection, candidate visible while registry unlatched,
accepted ownership transfer and Unknown fatal. Component-only; native products
not rebuilt/deployed yet. ctlps confirms system65595/Calculator65598 still live.
User prioritizes Chrome example.com rendering. Serialized graph6928 PASS
inputs1128/digest311958c19d8ae2d3761b53d4033c4b395d706f33716bf4bad98f9dcc437c4315;
both products32032 terminal exit0, headless ABI undefined0, graphics closure
registrar51/fake0. Exact same two absolute targets repeated: no work. Verified
old65598/65595 stopped with TERM; profile/APK unchanged. Normal indefinite Chrome
launcher99340 remains live, fresh system34362/browser34365; guest GPU34419 and
renderer children34415/34472 live in ctlps. Physical address click+14-key HID
example.com/Return63864 PASS; `/tmp/darwin-art-chrome-prepared-example.png` OCR
only chrome/address, NOT webpage. NSBitmapImageRep body samples RGB0 alpha1:
genuine black body persists (initial restored capture was white). Latest sampled
central frames78–89 each contain only one layer. Vulkan VkDevice creation logged
for GPU34419; this alone does not prove selected rendering backend or cause.
Actual GPU process sample6600 terminal PASS; Looper/poll/Binder waits alone are
not deadlock proof. Next trace webpage command-buffer/child Surface production
and delivery; do not spend next turn on unrelated cleanup. Chrome left open.
Goal ACTIVE; black body is NOT acceptance.

**Immediate resume — prepared-plan gates (2026-09-18):** main reran actual
prepared registry ASAN/UBSAN fixture76591 PASS (candidate projection, canonical
predecessor simulation, cancel safety, allocation-free finalize), and Darwin
submit actual-TU fixture49516 PASS including exception classification/health.
Inspection confirms ordinary Create remains allowed during preparation; owned
touched pins and staged-stat references are acquired into separate ledgers;
transaction buffer ownership transfers only at Finalize. Submit exceptions
preserve Unknown/Committed classifications and do not trigger another request.
These are component gates, NOT production ready-transaction cutover or APK
acceptance. Next: wire prepared snapshot/typed receipt at ready transaction,
then serialized products/warm/audits and fresh physical APK gates. No deployed
runtime changed this turn. User reiterates task-owned shutdown/restart needs no
confirmation; preserve profiles/APKs. Goal ACTIVE.

**Immediate resume — actual client/reply interoperability (2026-09-18):** prior
turn PROGRESS (payload/registry audit); this turn PROGRESS. Added isolated fixture
linking actual client_transport_darwin.mm/client_receipt/socket_transport/
TransactionReply/region codec, with only guest FD-provider seams substituted.
Main ASAN+UBSAN54121 PASS: typed present/submit/commit and legacy wrapper,
protocol/kind/PID/transaction/layer/region/logical360x640 payload, one request,
explicit reject, lost reply Unknown, committed header missing FD/import failure
stays Committed-error, whole-run native FD balance. Invalid required layer rejects
before connection (listener poll empty). This is transport evidence, NOT real
Android commit or APK acceptance. No shared native/DEX rebuild/deployment.
Luna registry WIP now separates simulated occurrence VALUES from stable actual
element pointers, retains only extra canonical AHB lease and reserves one active
plan. Main inspection still blocks adoption on raw touched_pins released before
Acquire when prior preparation allocation fails; requested acquired ledger/count.
Also requested candidate_relatives clearing on absolute-z before relative delta,
and finalize→CopyViews regression gate so old relationship cannot reappear.
Agent still running; fixes are not yet accepted/tested by main. Existing
system65595/Calculator65598 revalidated live and unchanged. Goal ACTIVE; ready
cutover, current-source products/warm/audits and fresh physical APK gates pending.
Main git diff --check PASS.

**Immediate resume — payload gate and prepared-plan audit (2026-09-18):** prior
turn PROGRESS (typed transport adoption); this turn PROGRESS. Completed Luna
composition payload integrity: failed required lease/append allocation makes
whole turn sticky-rejected, releases ownership, exports no producer fence and
sends zero requests. Main reran actual CompositionConsumer TU30806 PASS, then
extended typed-port rejection/unknown/committed-error fixtures: exact one request
and preserved classification, repeated End makes no second request;50725 PASS.
Restore failure preserves owned committed FD; lease/vector failure balance PASS.
Completed payload agent retired; no shared native rebuild/deployment this turn.
Main registry WIP audit found pre-adoption blockers and sent exact sources to
Luna: simulated_occurrences stores actual canonical map element pointers then
writes buffer/cookie during Prepare; use separate simulated values, stable actual
pointers only for finalization. touched_pins/staged previous-buffer maps may
release unacquired refs on allocation failure; stage raw/owned separately.
Actual builder already retains transaction buffer, so two more prepared retains
then update.buffer=null requires transferring/releasing original owned lease.
Exact active-plan reservation still required. Do NOT wire or accept this WIP
until Prepare+Cancel immutability, allocation-failure ownership, canonical alias
order and final reference-balance fixtures prove the fixes. Main leaves running
APK generation unchanged. Ready integration/native builds/physical acceptance
remain pending; Goal ACTIVE. git diff --check PASS.

**Immediate resume — typed client receipt adoption (2026-09-18):** prior turn
PROGRESS (socket owner/tests). Main removed client serialization/request/socket
ownership from service_darwin.mm into client_transport_darwin.mm; server now
1281 lines. Public C-compatible 12-byte receipt and typed present/submit/commit
siblings retain legacy int APIs as wrappers over the same implementation.
Actual client_receipt parser preserves Unknown after lost/malformed response,
explicit rejection, and Committed-with-error after missing/import-failed FD.
Verified actual bionic pipe importer consumes FD even on failure; removed
duplicate close and tested FD-number reuse. Parser validates pipe before handoff.
Main ASAN/UBSAN receipt fixture99244 PASS, including throwing consuming provider;
C11 header and actual client Objective-C++ syntax PASS. Actual server syntax
PASS. Build-contract cargo99723 PASS; both catalogs/common list graph v42.
Main connected typed backend -> CompositionEndResult -> ANGLE receipt sibling
-> SurfaceControlSubmitResult; restoration health independent of commit, owned
FD preserved. SurfaceControl returns after End/direct commit, never retries an
attempted submission. Safe pre-Begin direct fallback remains; invalid required
buffer rejects entire direct payload. Explicit empty local no-work is not a
fabricated remote commit. Main actual Submit TU fixture80904 PASS including
restoration-after-commit/unknown-no-retry/Retina logical target contracts.
Luna registry PreparedSurfaceCommit and separate composition payload integrity
fixtures are still running; no shared native products built for these changes.
Astra design reviewed earlier; follow-up review dispatch hit agent thread quota,
not a reason to fabricate approval. Ready owner still pre-latches until prepared
plan integration freezes; no-output/release contracts and APK gates OPEN.
ctl ps revalidates existing system65595/Calculator65598; left live, no APK/profile
changes. Main git diff --check PASS. Goal ACTIVE, not acceptance-complete.

**Immediate resume — transport owner extraction (2026-09-18):** prior turn
PROGRESS (atomic imported-parent creation and actual-source tests). Main
extracted byte/SCM/endpoint connection from oversized service_darwin.mm into
socket_transport.{h,cc}; server and client now call the same narrow transport
owner, with no layer/Android commit policy. Connect owns CLOEXEC/SO_NOSIGPIPE.
Added actual socketpair/Unix endpoint ASAN+UBSAN fixture; it caught bounded-read
overflow and invisible-rights leak with undersized Darwin ancillary storage.
Receive now sizes for XNU's full rights batch, bounds parsing, closes all extras,
rejects invalid marker/count and sets returned right CLOEXEC. Final fixture PASS
invalid 1/2/6-right batches, expected-no-right mismatch, valid receipt rights,
byte reassembly, descriptor balance and disconnected-client SIGPIPE suppression.
Both runtime catalogs/common adapter list include owner; graph version v41.
Main cargo check build-contract8836 exit0. No native products built since edit.
Astra typed-receipt review complete: preserve disposition through direct and
Begin/End paths; committed header + missing FD stays committed-error; context
restoration independent, no post-submit retries. Failed leases/append allocation
must reject whole batch before transport. Main next owns client serialization
extraction and typed siblings/ready integration; Luna prepared registry remains
running. Unknown/committed-error fatal policy and full APK gates remain OPEN.

**Immediate resume — prepared commit ownership (2026-09-18):** previous turn
NO PROGRESS (termination permission acknowledgment); this turn PROGRESS.
Main reran actual CompositionQueue + TransactionReply sanitized fixture: PASS
rejected enqueue retains caller reply, blocked producer never sends success,
Abort gives unresolved EOF, descriptor reuse stays valid. Atomic imported
parent identity now initialized by Registry::Create before publication; removed
sole SetImportedParent mutator/caller. Main actual registry/occurrence fixtures
PASS; registry fixture corrected stale canonical-buffer lease counts against
actual implementation (canonical + wrapper + snapshot are independent leases).
No shared native rebuild after these latest edits: previous two-product/warm
evidence below is not acceptance of current unbuilt source.
Astra completed PreparedCommit design review: candidate graph before transport,
canonical per-layer predecessor simulation, preallocated stats/retirement,
touched-wrapper pins/stable map element pointers, allocation-free finalize and
reentrant resource retirement outside lock. Luna implementing registry/state
and isolated fixtures; main owns ready-transaction integration. Astra next
reviews typed receipt propagation (Rejected versus Unknown versus Committed);
existing int-FD ABI loses rejection classification, so unknown still fatal.
Calculator/system revalidated live as 65598/65595; no restart/termination this
turn. Read-only NSBitmapImageRep verification of actual Calculator click/key
PNGs finds opaque correct title/white result/gray keypad, despite black inline
viewer rendition; do not report a Calculator black-frame regression from that
viewer. Prior Chrome PNGs have genuine opaque black body pixels while title
and toolbar remain visible; Chrome rendering gate remains OPEN. Goal ACTIVE.

**Immediate resume — real commit receipt (2026-09-18):** previous turn PROGRESS
reproduced physical close failure and fresh APK rendering gaps. This turn removes
enqueue-as-commit ACK: protocol11 CommitDisposition + narrow TransactionReply
transport owner transfer duplicated socket/read-fence into CompositionJob.
ProcessRequest sends Committed only after genuine original AOSP flush/retained
publication, outside State mutex; ingress never waits for queued receipt.
Unresolved destruction closes only (EOF/unknown), no safe-discard claim. SIGPIPE,
bounded250ms response, one-shot and independent completion ownership are tested.
Astra-found exception FD leaks fixed with immediate original producer/pipe RAII,
released only after successful enqueue; final Astra review confirms resolved.
Android change-bit merging/region copy extracted into independently compiled
retained_layer_state.{h,cc}; no Darwin transport/allocation policy in that owner.
Main actual-source tests PASS `/tmp/darwin-art-{transaction-reply-test,
transaction-reply-ingress,retained-layer-state-main}.log`; receipt ASAN/UBSAN test
also proves output EOF processing while another receipt is unresolved.
Cargo check76949/graphs87313+88217/product1330 completed exit0. Both closure audits
PASS; exact same pinned-Ninja two-target repeat reports no work; both native
boundary audits18513 PASS fixture-exports=0, graph reachable probes/=0.
Actual new module objects present in both link maps. Logs
`/tmp/darwin-art-commit-receipt-{cargo,graph,products,warm}.log`.
Exact old guarded runtime25341/27702/27797/27804/27892/24137 TERM before replacement;
no profile/APK modifications. New normal Calc launch44982 live, system65595 and
Calculator65598. Inspected physical2+3=5 and hardware2,3,Return→23 PASS after
receipt cutover, captures `/tmp/darwin-art-calculator-commit-receipt-{click,key}.png`.
Calculator left running. Goal ACTIVE, not complete: local pre-latch remains,
no-output commit/release-occurrence fences, title and Chrome rendering still OPEN.
Next PreparedCommit cannot merely reorder CaptureSnapshot: its current builder
filters old buffer/visibility, omitting a first pending buffer. Finalize currently
allocates stats/maps/relationship tail/regions after latch; preparation must
construct candidate state and allocate before external submission. Astra planning
review and Luna pure queue+reply rejection/abort test are running; no shared builds.

**Immediate resume — fresh guarded APK acceptance (2026-09-18):** prior turn was
PROGRESS (both products, warm build and original JNI lifetime verified). This
turn gracefully terminated exact old85400/85397 and shut down the profile daemon.
First launch77686 exited1 on missing socket; explicit normal `ctl ensure` then
launch98271 succeeded. Current daemon23785/system24137 use the guarded build.
Calculator24140 physical2+3=5 and hardware2,3,Return→23 PASS; Retina captures
`/tmp/darwin-art-calculator-guarded-{click,key}.png`. Physical red-close reproduces
status70/ESTALE then post-commit abort; Calculator is gone, not normal-close PASS.
Clock25341 launch50529 is live; physical20s timer shows19 in inspected capture
`/tmp/darwin-art-clock-guarded-timer.png`. Window title remains package name;
missing media provider/ringtone cache warning observed, full expiry not verified.
Chromium27702 plus children27797/27804/27892 launch15620 live. Real Graphite/Dawn
Vulkan flags, MoltenVK instance/device on Apple M2 Pro observed; physical menu
visible, but example.com body is black, not webpage PASS. Original-map Control+A
URL replacement yields one example.com, not duplicate/appended a; broader shortcut
acceptance remains open. Captures `/tmp/darwin-art-chrome-guarded-{page,menu,
replace-url}.png`; accidental Find-in-page capture is not tab-switcher evidence.
Tab retry12443 completed exit0; OCR lists example.com, but inspected image is
black below the native titlebar, not a visible tab grid. This is a rendering
failure, not tab-switcher PASS. Apps remain live; no APK/data resets.
Astra confirms required next structural correction: prepare local registry
without latch, central real Android commit receipt (not enqueue ACK), optional
Darwin presentation and previous-occurrence release fences covering actual reads.
ADR0003 updated to this decision, explicitly not implementation. Main must
implement/integrate this owner split; do not convert ESTALE to success or delay
OutputOwner retirement. Goal ACTIVE; fresh rendering/lifecycle acceptance fails.

**Immediate resume — original keymaps integration (2026-09-18):** ASCII/DAKM
source bypass is removed; both native link paths and explicit isolated graph
producer now use original keymap/JNI archive. Java registry calls Generic1/
Virtual-1 factory. Support DEX PASS111 sources/234 classes; DEX263/1752.
First native link FAIL missing ReadGuestConfig, now added with GuestFile to both
common production catalogs. Serial retry session95150 completed exit0: both
production products link PASS, log `/tmp/darwin-art-keymaps-products-serial-retry.log`.
Astra review found an inherited original JNI allocation handoff leak. Guard patch
is now pinned at 3f5656e05c01ffad48f2d8a20eb5ce500094520d9416e6843610fe9c42700da4;
isolated compile session59854 failed exit3 on the previous stale patch pin.
That pin is corrected. Guarded archive builds18848/6012 completed exit0; repeated
bytes SHA2561cb2dbd989de9ee2dbc1c597516a8eb52ae0335cdcff4807b2284982f98455e6
and mtime1789710743 are identical (second publication reports unchanged).
Regenerated graph14915 and serialized products81916 completed exit0; headless
undefined=0 and graphics registrar=51/fake-symbols=0. Exact pinned-Ninja repeat
of the same two absolute targets says `ninja: no work to do.`
Both product source/link boundary audits75175 completed exit0, fixture-exports=0;
actual graph reachable probes/=0 and contaminated/transitive/query-negative
tests PASS. Logs `/tmp/darwin-art-keymaps-guarded-{products,warm,graphics-boundary,
headless-boundary}.log` and `/tmp/darwin-art-keymaps-graph-negative-current.log`.
Original JNI factory failure/transfer test92948 completed exit0 with main-reviewed
real provider linkage and the same checksum-verified production compile context;
no unresolved suppression or fake destructor. Null Java allocation, pending
exception preservation, successful transfer/delete and C++bad_alloc PASS:
`/tmp/darwin-art-keymaps-guarded-factory-main.log`. Registrar sanitizer test PASS.
Next: fresh post-guard unchanged APK interactions/device identity; proper
transaction/output retirement settlement and localized-title regression remain.
Normal system-image build PASS; fresh unchanged Calculator85400/system85397
physically computes2+3=5 and hardware2,3,Return produces23 before the guard patch.
Calculator remains running; package-name window-title regression remains OPEN.
Fresh post-guard APK/JNI/device-identity acceptance and retirement settlement
remain OPEN. Do not restart apps merely because an observation times out.

Checkpoint (2026-09-18, termination policy): user reiterates that task-owned
app/runtime/daemon termination or restart must proceed without asking again.
Exact-target checks and graceful shutdown still apply; preserve profiles/APKs
and unrelated apps. No termination was needed for this policy update.

Checkpoint (2026-09-18, guarded original input integration): previous turn was
PROGRESS (corrected pinned ownership patch/checkpoint). This turn verifies both
guarded products, exact warm no-work, actual JNI factory allocation lifetime and
clean source/link/graph fixture boundaries. Goal remains ACTIVE: these gates
do not establish fresh full Calculator/DeskClock/Chromium physical acceptance,
localized titles or correct close/retirement fence settlement. Existing
Calculator85400/system85397 remain live on the pre-guard generation; no profile
reset or APK modification. Completed bounded implementation agent is idle.

Current resumption (2026-09-18): goal ACTIVE. User explicitly authorizes
task-scoped app/runtime/daemon stop/restart without repeated confirmation;
preserve installed APKs/profiles and unrelated applications. AGENTS records it.
Original system/system_ext compat XMLs now have pinned, strict, read-only atomic
artifact production; actual artifact tests and system-root packaging PASS.
Genuine ActivityThread.systemMain/getSystemContext and original CompatConfig
evaluation are integrated into authenticated AMS binding: empty disabled/loggable
arrays are removed. Strict complete-document preflight precedes original schema
parsing; actual framing/topology tests PASS, not ART policy acceptance.
Fresh real Clock startup exposed DisplayManager Binder lookup before context
manager registration. Both failed system processes exited; Clock was not visible.
Corrected order starts the internal service directory/kernel Binder before real
system Context/policy and publishes external readiness afterward. Separate
admission owner permits only system-PID bootstrap lookups; isolated admission
and production-entry tests PASS. Both runtime products and support DEX rebuild
PASS (110 Java sources, 233 classes; DEX 262 classes/1748 methods).
Logs: `/tmp/darwin-art-compat-ordered-{entry-test,products,dex}.log`.
Fresh normal unchanged Clock launch now runs system48520/application48522;
physical HID selects TIMER, enters20s and starts it without the old mutability
exception. Genuine Retina captures show20 then12 seconds:
`/tmp/darwin-art-clock-compat-ordered-{start,running}.png`. Actual system Context,
catalog evaluation and authenticated binding therefore pass this interaction,
not full alarm expiration/service lifecycle acceptance.
Launch log: `/tmp/darwin-art-clock-compat-ordered-launch.log`.
Next goal turn is PROGRESS: fresh unchanged Calculator53270 physical2+3=5 and
hardware2,3,Return→23 PASS; captures
`/tmp/darwin-art-calculator-compat-{click,key}-result.png`. Calculator closed.
Fresh Chromium55114/GPU55168 has the required actual Graphite/Dawn Vulkan flags;
physical menu and tab-list UI render, but NTP body is black and restored webpage
body blank. `/tmp/darwin-art-chrome-compat-{menu,tabs-current,newtab-later}.png`
prove only those states, not webpage acceptance. That Chrome generation closed.
Astra finds one real terminal-root lifecycle defect, no additional must-fix
mixed-owner/Probe bypass in bounded native paths. WMS now retires the exact root
decision subscription before terminal unbind capture and fences capture/send/
settlement on bindingOpen; live-root retries remain. CLOSED already locally
revokes native key authority; do not invent client death or require terminal ACK.
Both products and support DEX rebuild PASS (262 classes/1749 methods).
Source/link boundaries and zero-Probe reachable graph PASS; exact pinned warm
repeat no-work. Logs `/tmp/darwin-art-root-terminal-{products,dex,boundary-graphics,
boundary-headless,warm-repeat}.log`. Luna actual-policy fixture and main frozen
rerun PASS, including real blocked-send/monitor-side close/DeadObject completion,
terminal before unbind, rejected callback, successor preservation and open-root
null-selection revocation retries. This is component proof, not physical close.
New normal Chrome diagnostic launch runs system74131/Chrome74136/GPU74190;
actual Vulkan Android-AHB/sync-fd/device creation logs and physical URL key entry
are verified. Fresh NTP has visible Google/search/tiles:
`/tmp/darwin-art-chrome-root-terminal-current.png`. After URL/Return,
`/tmp/darwin-art-chrome-root-terminal-example-later.png` still has blank body
and old New-tab title. No actual Vulkan AHB image import/child-content queue
activity observed; device creation does not prove displayed webpage content.
Launch log `/tmp/darwin-art-chrome-root-terminal-debug-launch.log`;
browser/GPU samples remain observational, not proven deadlock.
Latest continuation is PROGRESS after the preceding authorization-only turn.
Physical Ctrl+A preserves native key29/meta4096 but inserts `a`; Shift:semicolon
key74/meta1 produces `:`. The migrated ASCII evaluator, not modifier transport,
is defective. Astra approves original pinned KCM/JNI/Parcel adoption and explicit
system Generic/Virtual selection, not Ctrl suppression or another copied parser.
The keyboard follow-up document now records that boundary; main added an
original-evaluator/Parcel regression source, not yet executed or a runtime PASS.
Main also prepares SystemKeyboardMaps/system_keyboard_maps_jni as a narrow
system input-device factory: Generic/Virtual selection in Java, guest descriptor
reads and original loadContents/create in JNI, no host pathname or ASCII
fallback. It is NOT registered, in the product graph, or called by the current
InputDeviceRegistry; original JNI/KCM adoption and verified cutover are required
before replacing obtainEmptyMap. Original Input.h itself has Linux-only generated
MotionEventFlag/IInputConstants includes, despite using these types on Darwin;
this is an upstream host assumption, not evidence of an invented partial-header
policy. Main pins/fetches those original AIDLs and supplies genuinely generated
headers at the Darwin compilation boundary without changing matching policy.
`tools/tests/aosp-key-character-map-compile-test.sh` PASS original evaluator,
new factory and regression syntax with matched headers; not link/JNI/Parcel
execution or APK proof. Log `/tmp/darwin-art-kcm-original-compile-test.log`.
Luna completed pinned source/resource acquisition, isolated space-path/repeat/
symlink/hash negatives; main reviewed it and made packaged resources read-only.
Canonical materialization PASS, `/tmp/darwin-art-kcm-canonical-materialize.log`;
main persisted artifact regression PASS pinned inputs, space paths, repeat,
read-only keychars and pre-write symlink rejection:
`tools/tests/key-character-map-artifact-test.sh`,
`/tmp/darwin-art-kcm-artifact-test.log`. Native/JNI/product graph and
system-root packaging integration remain OPEN. Existing EGL logs prove config0 ES3.0 creation succeeds;
higher-version rejection is not missing no-config support and warrants no clamp.
Physical Delete then `http://example.com` was captured exactly before Return:
`/tmp/darwin-art-chrome-http-before-return.png`; later actual capture
`/tmp/darwin-art-chrome-explicit-http-later.png` still has blank body/old New-tab
title. The earlier Ctrl+A attempt was a search, not valid explicit HTTP evidence.
Actual physical red-close of Chrome74136 at14:01:08 exits it and its children;
system74131/daemon48377 remain live. No new infinite root-decision retry observed,
but close FAILS gracefully: local transaction767 latches before central output
admission; already-retired target19 returns Darwin ESTALE70 and triggers abort.
Astra requires independent Android transaction acceptance, optional Darwin
presentation and exact prior-use buffer fences; do not delay retirement or turn
ESTALE into success. Main fixes the separate actual GPU completion callback:
retirement suppresses publication, not proof of completed reads. This does not
fix precommit/queued retirement or prove shutdown. Both native archives/dylibs
build and link PASS. Both source/link boundaries report clean/fixture exports0;
reachable Probe graph inputs0. Exact frozen pinned two-product repeat says
`ninja: no work to do.` Logs
`/tmp/darwin-art-completion-separation-{products,links,boundary-graphics,
boundary-headless,graph,frozen-warm}.log`. Actual callback/retirement race
execution and changed-runtime APK acceptance remain OPEN.
Next continuation is PROGRESS; preceding authorization-only reply was no-progress,
revalidated by continuing the original native implementation immediately.
The original seven-unit Input/Keyboard/labels/KCM closure now compiles and links
against real production Binder identity/transport, Rust runtime and native/Rust
Bionic provider archives, with no syscall/UID/Parcel fixture replacements.
Missing input.h-labels.h is generated by the pinned original system/core tool
from the exact two original Bionic kernel_input_headers inputs, in Soong order.
Actual evaluator/Parcel execution PASS: Ctrl/Meta exact matching, Shift/Caps,
explicit Alt character, Control-Space/Meta-Space fallback modifier consumption,
and full original Generic/Virtual payload round-trips. Log:
`/tmp/darwin-art-kcm-original-runtime-test-current.log`. This does NOT establish
JNI device identity or product cutover. Main fixed immutable acquisition metadata
expansion by retaining existing manifests and adding separate Bionic/system-core
provenance; canonical materializer terminal0 and updated space-path/repeat/
read-only/symlink artifact test PASS, log
`/tmp/darwin-art-kcm-artifact-test-current.log`. Shared compiler-context syntax
and scoped diff checks PASS. System74131/daemon48377 revalidated live; no restart,
APK/profile modification, commit or push. Original JNI/product graph, resource
packaging, physical acceptance and retired-output settlement remain OPEN.
Original runtime/artifact execution handles terminal0; bounded acquisition worker completed.
Following continuation is PROGRESS: original matched framework JNI headers
(jni_wrappers, AndroidRuntime and runtime Log) are now pinned/acquired, not
borrowed from another partial source tree. Original KeyCharacterMap/KeyEvent JNI
and the narrow guest-file factory compile as real objects PASS, log
`/tmp/darwin-art-kcm-original-jni-compile.log`. Main adds a dedicated input-keymaps
archive producer; its nine original native/JNI units build PASS, log
`/tmp/darwin-art-input-keymaps-archive.log`. Archive member/symbol inspection
confirms original factory and both original registrars. This archive is NOT yet
a product graph/link input; existing ASCII/DAKM evaluator remains production.
The original evaluator/Parcel execution was proven in the preceding turn;
JNI registration/device identity, native product integration and fresh physical
APK acceptance remain OPEN. New shell compiler/producer syntax and scoped diff
checks PASS. Main native handles terminal0; no app termination, APK/profile reset,
commit or push. No completion claim from object/archive compilation.
Next continuation is PROGRESS: actual production input registrar now calls
original KeyEvent/KeyCharacterMap and the factory. The ASCII evaluator, fake
FULL empty-map policy, DAKM format and two int32 Parcel bridge functions are
removed, not relocated. InputDeviceRegistry uses original Generic/Virtual
factory; support source manifest and baseline/button payload catalogs include
SystemKeyboardMaps. Production support DEX PASS111 sources/234 classes and
verified DEX263 classes/1752 methods, log `/tmp/darwin-art-keymaps-support-dex.log`.
Actual native link paths refresh/link original archive and isolated graph owner
tracks its source/pin/factory inputs. Reachable Probe inputs0 PASS, log
`/tmp/darwin-art-keymaps-graph-boundary.log`. Thin registrar sanitizer ordering/
pending-exception/failure tests PASS; original evaluator/Parcel regression remains
separate actual-original coverage. Original resource packaging and main frozen
signed-bundle/package regression PASS, byte-identical maps, log
`/tmp/darwin-art-keymaps-system-root-test.log`.
Detected concurrent shared audit producers were deliberately interrupted; exact
Ninja64202/children67891/68362 verified terminal before regeneration/retry.
Producer/audit rules now share the existing serialized pool; main uses -j1.
First serial link terminal1: actual undefined ReadGuestConfig exposes missing
guest_file/config production inputs. Main adds these actual modules and required
Bionic includes to both common catalogs, not fixtures. New graph emits inputs1117.
Current serial product retry session95150 is live; no product PASS claimed.
Astra bounded integration review pending. Source keyboard document now separates
removed source bypass from still-old live runtime generation. Cargo check before
the provider correction, scoped diff/shell checks PASS. No app termination,
APK/profile reset, commit or push. Next: poll exact build/review, address failures,
then both boundary/warm gates and fresh physical unchanged APK acceptance.
Fresh graceful close and Chromium webpage acceptance remain OPEN. No APK/profile
reset, CPU/GL fallback, commit or push. Next: integrate original key maps and
separate committed transaction settlement from retired scanout eligibility.

Earlier evidence: Calculator ARC/physical key acceptance and native graph/link
audits passed before compatibility-policy integration. Historical Chrome device
creation and NTP captures did not prove webpage acceptance. Superseded PIDs and
restart-permission blockers must not be reused; detailed experiments remain in
Git history and the checkpoint bullets below.

- **2026-09-18 — External waiter race correction (PROGRESS):** Previous turn
  made verified product ARC/fatal-handler progress. Current turn rereads AGENTS
  and goal, then revalidates ctl registry system32868 against kernel zombie
  PPID1; no ordinary APK process is live. Astra final changed-source review
  identifies one remaining race: an external waiter can reap after initial
  try_wait but before final wait. Main routes that wait ECHILD through exact
  birth-identity terminal settlement, without another signal or invented status.
  Second genuine regression performs initial live try_wait, external waitpid,
  actual Child::wait ECHILD and the same production settlement helper; live
  ECHILD proof rejection and exact callback-once checks remain. Initial targeted
  result5 passed/1 ignored; final frozen-source rerun also5 passed/1 ignored,
  command exits0. Corrected release darwin-artd build exits0; git diff check
  PASS. Logs: `/tmp/darwin-art-echild-frozen-lib-tests.log` and
  `/tmp/darwin-art-echild-release-daemon-build.log`. No APK/profile
  mutation, app launch/stop, daemon restart, commit or push. Existing daemon
  watcher cannot be resurrected by changing source: requested management-daemon
  restart authority remains absent. Fresh three-APK physical acceptance OPEN;
  goal ACTIVE, not complete. Next: obtain management-daemon restart approval,
  then normal unchanged APK acceptance.
  **Subsequent restart-authority audit (BLOCKED, not completion):** Previous
  turn is progress (race correction, frozen tests, release build), not a live
  build wait. Re-read AGENTS/current goal; current ctl still lists32868 while
  kernel reports zombie PPID1, with no ordinary APK process. Astra is terminal;
  other agents are interrupted and no build remains pending. The same required
  management-daemon restart authority has remained unanswered for three
  consecutive goal turns. Further source edits or repeated component tests
  cannot restore the ended watcher or prove fresh physical APK acceptance.
  Goal is now BLOCKED pending that specific approval; unchanged APK/profile
  preservation and the full completion gates remain in force. No process
  signal, profile/APK mutation, commit or push in this audit turn.

- **2026-09-18 — Actual product ARC ownership correction (PROGRESS):** User
  approved the normal three-app restart; the earlier approval blocker is
  resolved. Old Calculator/Chrome exited. Old Clock required scoped forced
  termination after an actual sample proved ART fatal `exit()` re-entered
  SurfaceTransactionSubmission finalization on the interrupted Submit stack.
  Pinned ART patch0195 stages pristine runtime_linux.cc and uses Darwin `_exit`
  only after existing fatal diagnostics; real ART fixture exits1 within its
  deadline without running the registered finalizer. Normal shutdown remains
  unchanged; underlying committed-transaction failure is not claimed fixed.
  Fresh Calculator then exposed a genuine production ARC recipe omission:
  SurfaceBackingHandle did not retain its Metal texture. The actual old product
  object fails the weak-texture lifetime assertion; standalone ARC fixtures had
  missed this producer mismatch. Three Metal/backing/capture providers now
  compile with ARC and reject non-ARC compilation. Both products rebuild PASS;
  actual product object has objc_retain/release and the same lifetime test PASS.
  Graphics/headless source/link boundaries, reachable graph zero-Probe gate,
  graph negative controls PASS; exact pinned two-product repeat reports no work.
  Fresh Calculator now stops before app creation: normal generation replacement
  times out because old system32868 is a kernel zombie but daemon Drop received
  ECHILD and skipped exact registry/instance terminal cleanup. This is not a live
  window or successful app launch. Identity-stamped host terminal corroboration
  was reviewed by Astra; bounded Luna implementation was frozen and main
  integrated exact-incarnation terminal observation plus supervisor retry.
  Its regression consumes an actual child wait status externally and checks
  exact callback/instance cleanup. Targeted library tests report4 passed/1
  ignored, including actual external waitpid and live-identity rejection;
  encompassing Cargo command now exits0. Final git diff check PASS. Astra's
  changed-source follow-up review remains pending, so no fresh deployment claim.
  Management-daemon restart approval was requested
  and remains unanswered. Do not kill a PID on ECHILD or invent a wait status;
  next implementation must retain supervision until exact birth-identity kernel
  terminal proof, then run the existing exact-lease cleanup once.
  Genuine ART BinderProxy JNI-list fixture still PASS after the fatal-tail patch.
  Genuine Metal diagnostic-capture fixture also PASS after the ARC correction:
  physical720x1280 pixels, retained allocation, disabled path and write failure.
  Current fresh APK launch/physical acceptance remains OPEN. APK/profile bytes
  are unchanged; no commit/push. Goal ACTIVE, not complete.


- **2026-09-18 — Structural exit audit / live Chrome failure (PROGRESS):** Astra
  rereads changed production entry/registration/input/GPU/shutdown paths; zero
  additional must-fix mixed owners or Probe bypasses identified. This is scoped
  structural judgment, not entire-goal acceptance. Main revalidates old live
  system60597/Calculator60599/Clock61497/Chrome61937 without restart/kill. New
  external CLI capture shows Example Domain title/address but black webpage body.
  Physical HID tab click (logical252,610, content height640) exits0, yet before/
  after PNG hashes are identical: `/tmp/darwin-art-chrome-migration-{current,tabs}.png`.
  Three process samples complete; actual61954 is renderer and61955 GPU (sample
  filenames gpu/renderer are reversed). Browser/Viz Looper poll and condition
  waits are observations, not proven deadlock. Vulkan device creation is logged;
  EGL error belongs to Dawn OpenGLES adapter discovery. Neither proves selected
  webpage backend or explains blank body. Old failures are not new-source
  acceptance. No product source changes/build duplication/APK/profile mutation/
  commit/push. Next is approved normal three-app restart and fresh physical
  key/pointer/page tests; asynchronous restart answer still absent. Goal ACTIVE.

- **2026-09-18 — Scanout diagnostic resource owner (PROGRESS):** Astra identifies
  production PresentSurfaceOnMain's inline configuration/throttle/readback/PNG
  responsibilities as a genuine mixed-owner seam. Luna implements the narrow
  ScanoutDiagnosticCapture module; main integrates the production caller, removes
  the inline block and owns graph/build/fixture verification. The move-only job
  retains the exact immutable backing and Metal buffer; its encoder is borrowed.
  Bridge still owns drawable presentation and command commit. Configuration is
  copied once per process; disabled jobs allocate no Metal buffer and do not wait.
  One-shot completion waits only for an admitted capture, retires its backing,
  converts BGRA to RGBA and reports genuine PNG write failures. Main's actual-object
  ASan/UBSan fixture PASS: real Metal copy, physical720x1280/logical360x640,
  four-corner channels/orientation, retained job after owner Close, moved-from
  inactivity/double Complete, disabled/throttled jobs and visible written=0.
  Log `/tmp/darwin-art-scanout-capture-test.log`. Astra frozen-source review has no
  must-fix; this closes the identified capture seam, not all oversized owners.
  Graph inputs1068 generated. Both final-source pinned products rebuild exit0;
  graphics/headless source/link closures clean, fixture exports0, reachable
  graph probes0 and graph isolation/negative controls PASS. Exact two-product
  warm repeat reports no work. Logs `/tmp/darwin-art-scanout-{graph,products,graphics-boundary,headless-boundary,graph-boundary,graph-negative,warm}.log`.
  Scoped Rust formatting, shell syntax and git diff checks PASS.
  No app stop/restart, APK/profile mutation, commit or push. Calculator60599,
  Clock61497 and Chromium61937 still run the old generation; fresh physical
  three-APK Retina/key/Vulkan/Chrome webpage acceptance remains OPEN.
  Normal three-app restart confirmation requested asynchronously; no new
  approval is assumed and no profile/system PID sweep is performed.

- **2026-09-18 — Genuine Ganesh/Metal borrowed callback execution (PROGRESS):**
  Main extracts the existing release payload into BorrowedTextureBacking;
  production GPU caller and native fixture use the identical callback, with
  no test hook/export or fake GPU backend. Fixture links the exact product
  `_build/skia-metal-gpu/libskia.a`/libskcms and actual AOSP FreeType/PNG/z/JPEG/log
  providers. Initial compile/link failures were missing fixture includes and
  archive dependencies; no stub or upstream change is used to satisfy them.
  Actual Metal context, IOSurface and Ganesh draw PASS. Owner Close drops its
  snapshot while actual SkImage retains the shared payload; caller's extra
  successful backend texture descriptors are cleared. After image/target/GPU
  context retirement, the weak snapshot expires. Actual pinned factory's
  null-context failure also releases the same retained payload. Source review
  by Astra no must-fix, with honest scope: synchronized draw and eventual context
  retirement do not prove close concurrent with outstanding GPU submission.
  Log `/tmp/darwin-art-skia-borrowed-test.log` exit0; shell syntax/diff checks PASS.
  Typed graph includes the new production-only callback header. Both pinned
  products rebuild exit0 on final source. Graphics/headless source/link closures
  clean, fixture exports0; reachable graph probes0, negative controls PASS and
  exact two-product warm repeat no-work. Scoped Rust formatting PASS. Additional
  logs `/tmp/darwin-art-skia-borrowed-{graph,products,graphics-boundary,headless-boundary,graph-boundary,graph-negative,warm}.log`.
  No app restart/kill, APK/profile modification, commit or push. Fresh unchanged
  physical three-APK Retina/Vulkan/key/body and mixed-owner exit gates stay OPEN.
  Live Calculator60599, Clock61497 and Chromium61937 are revalidated but retain
  old runtime generation; they are not fresh source acceptance evidence.

- **2026-09-18 — Backing owner production adoption (PROGRESS):**
  Main replaces raw backing tuple fields with authoritative SurfaceBackingOwner
  and AppKit-only immutable cache. Production ANGLE acquire retains one coherent
  IOSurface/extent snapshot; size/id and AppKit pointer logical coordinates use
  the same owner. Creation, resize, logical-only extent and texture reimport
  construct/validate retained candidates before external output replacement.
  Allocation failure leaves old publication; unexpected post-IPC rejection
  retires output rather than presenting stale authority. Exact mapped allocation
  is retained through matching unlock. Bounded Luna GPU adoption uses one snapshot
  per begin/composite, frame retention and pinned Skia texture-release context.
  Main restores original composition readiness gate after Astra detects an
  unintended branch-order change; prior completion-marker release moves outside
  submission-only lock with objc_precise_lifetime. Caller quiescence is unchanged.
  No mapping/GPU exclusion or concurrent GrDirectContext destruction is claimed.
  Build contract24 PASS, typed graph regenerated, actual owner ASan/UBSan PASS
  including validated factory; common source syntax PASS. First product build
  caught old AppKit coordinate field reads; corrected source then builds both
  products exit0. Final ARC-lifetime source rebuild exit0; actual graphics and
  headless product ABI fixtures PASS real GPU backing creation, mapping while
  resize rejects, matching unmap, invalid candidate preservation and destruction.
  Graphics additionally proves retained IOSurface across replacement; headless
  explicitly asserts GPU surface-id remains unexported. First shared fixture
  attempt failed that absent headless export; fix scopes tests to each real ABI,
  without adding exports or weakening graphics retention acceptance.
  Both final source/link boundaries clean, fixture exports0; each link map has
  one backing-owner object. Reachable graph probes0, negative controls PASS and
  exact two-product warm repeat no-work. Logs `/tmp/darwin-art-backing-adoption-*`.
  Astra common/GPU reviews inform those fixes; production runtime caller exists
  for ANGLE acquire, while direct begin/composite remain fixture-only callers.
  Actual deferred Skia release execution and changed-path fresh APK behavior
  still need acceptance; source callback retention review is not that execution.
  No app restart/kill, APK/profile modifications, commit or push. Fresh unchanged
  three-APK Retina/Vulkan/key/body acceptance remains OPEN; goal stays active.

- **2026-09-18 — Retained backing owner staging (PROGRESS, adoption OPEN):**
  Luna implements narrow immutable IOSurface/Metal/physical+logical extent/stride
  snapshots behind exact expected publication and sticky Close. Astra source
  review identifies and fixes malformed geometry/stride/backing publication
  validation and destructor reentry sealing; retirement stays outside the lock.
  Main syntax check of actual Objective-C++ owner PASS. Main actual native
  Metal allocation ASan/UBSan gate PASS and composition IOSurface lifetime gate
  PASS; runtime shutdown8 PASS confirms ingress/native readiness before surface
  destruction. Logs `/tmp/darwin-art-backing-{owner-syntax,allocation,existing-lifetime,shutdown}.log`.
  Main takes over the frozen fixture, removes GPU-missing false success and adds
  real-resource concurrent coherent snapshot reads, stale/repeated/invalid tuple
  rejection, same-IOSurface reimport and logical-only immutable publication,
  retained allocation lock/unlock after Close
  and actual Close/destructor resource-release reentry. Actual owner ASan/UBSan
  fixture exit0 PASS (`/tmp/darwin-art-backing-owner-test.log`), not a fake Metal
  object or metadata-only test. This proves retained allocation usability, not
  adoption of the producer mapping or Skia release context.
  Astra frozen prototype/fixture source review finds no remaining must-fix;
  bounded Luna worker is interrupted after main takeover, reviewer completed.
  Actual surface/ANGLE/GPU adoption, exact mapped identity in production
  and deferred Skia retirement gates remain OPEN. Existing product
  source/link/graph and warm evidence is the previous checkpoint, not new owner
  adoption evidence. No app restart/kill, APK/profile changes, commit or push;
  Calculator60599, Clock61497 and Chrome61937/61954/61955 remain old generation.
  Goal stays active, including unchanged physical Retina/Vulkan/key/webpage gates.

- **2026-09-18 — Genuine ART recipient-list acceptance (PROGRESS):**
  An isolated fixture image relinks the validated headless product's exact
  production objects/archive order, without loading a second product runtime
  or adding a fixture export to production. Versioned lossless compiler-argv
  publication follows successful product validation; transformations replace
  only output/map/install-name and append test object/export. Codec5 PASS covers
  order, non-UTF8 arguments, malformed flags, missing inputs and output aliases.
  Genuine ART run exit0 PASS public BinderProxy link/PRESENT, failed public
  unlink/retained recipient, retry/ABSENT, local Binder/UNSUPPORTED and original
  pending-exception identity. Initial exit26 was a fixture classpath omission;
  runner now requires explicit actual product support DEX, not fake registration.
  Both products' real pre-VM entry-failure resource gates PASS. Logs:
  `/tmp/darwin-art-binder-recipient-{product-recipe,build,run,recipe-tests,early-entry}.log`.
  Both pinned products rebuild exit0; exact two-product repeat says no work.
  Post-build graphics/headless source/link closures clean, fixture exports0;
  reachable graph probes0 and negative controls PASS. Scoped new Rust formatting,
  runner shell syntax and diff checks PASS. Additional logs share the same prefix:
  `warm`, `warm-repeat`, `graphics-boundary`, `headless-boundary`,
  `graph-boundary`, `graph-negative`. Test link map contains one BinderJNI object;
  its dylib does not load either product runtime alongside the isolated closure.
  Astra remaining-owner review identifies a narrow retained SurfaceBackingOwner;
  currently unlocked exported GPU reads have probe-only callers, whereas actual
  production ANGLE acquisition already retains surface/extent under its lock.
  Do not claim an observed APK resize race. Snapshot/borrowed-texture retirement,
  mapping identity and caller quiescence need separate gates before extraction.
  No app restart/kill, APK/profile change, commit or push. Fresh physical three-APK
  Retina/Vulkan/key/body acceptance, wire-terminal and mixed-owner exit gates
  remain OPEN; this is JNI-list proof, not goal completion or reusable VM unload.

- **2026-09-18 — Phased AMS shared-demand adoption (PROGRESS, physical OPEN):**
  ServiceProcessLaunchController owns process-keyed admitted service demand;
  production legacy start wrapper is removed. Opaque prepare/activate/retire
  and captured genuine obituary resources dispatch outside ActiveServices.
  One-epoch fixed-point draining settles nested lifecycle/launch/unlink work
  without immediate failed-tail retry. ServiceConnectionResourceController
  separately owns deferred recipient cleanup. Main integrates and fixes delayed
  prepare failure versus later adoption, cancellation/failed retirement tails,
  replacement already attached during terminal settlement, and captured owner
  cleanup after registry retirement. Genuine sticky Binder liveness—not
  DeadObjectException type—proves terminal ownership; sealed lanes preserve
  original/suppressed diagnostics. Astra final source review no new must-fix.
  Launcher/index/registry regressions and full process-gone fixture PASS. New
  phased integration fixture PASS blocked prepare/last unbind, shared two-service
  launch, synchronous real registry handoff/nested bind, old failure after
  alternate adoption AND subsequent death, and live Remote/heuristic DeadObject
  sealing with genuine obituary replacement adoption. These are controlled
  Android component fixtures, not physical APK acceptance.
  Support DEX PASS106 sources/229 classes; verified DEX38 classes257/methods1688.
  Both pinned products compile/link PASS; graphics/headless source/link closures
  clean, fixture exports0, reachable graph probes0, negative controls PASS and
  exact two-product warm repeat no-work. Fixture-input Rust8 PASS; scoped Rust
  formatting, unlocked-notification regression, shell syntax and diff checks
  PASS. Logs `/tmp/darwin-art-launch-adoption-*`.
  No app restart/kill, APK/profile modification, commit or push. Revalidated
  system60597, Calculator60599, Clock61497 and Chrome61937/61954/61955 retain
  the prior runtime generation. Genuine patched-JNI recipient-query actors,
  parent cleanup/mixed-owner exit audit and fresh Retina/Vulkan/key/webpage
  acceptance remain OPEN. Goal stays active; earlier phase logs remain
  `/tmp/darwin-art-launch-phases-*`.

- **2026-09-18 — Exact system template authority / connection resource owner (PROGRESS):**
  Main diagnoses a pre-gate authority race and fences immutable launch-template
  access by exact RuntimeInstance PID/incarnation under its data lock, after the
  launch gate/reap wait and before Existing or spawn. Astra actual-source review
  no must-fix for that Rust slice. State18 and actual-server Existing2 tests PASS;
  full profile114 PASS/fixtures5 ignored. Luna fixes an invalid activation test
  with real exec/read/reap; production StartGate is not weakened. Logs:
  `/tmp/darwin-art-generation-{template-tests,admission-tests,profile-tests}.log`.
  Luna drafts the Java split; main takes over all production integration and
  exact pending/admitted/pin predicates, typed lifecycle demand, callback reentry
  tail guards and primary/suppressed cleanup. ServiceConnectionDeathRegistration
  owns exact recipient resources; pending rows grant lifetime only, not execution
  or publication delivery. Main fixes Astra's synchronous-death-before-install
  leak and tests late unlink/original failure/retained failed tail. Standard AS,
  launcher, registry and index component runners PASS, not APK evidence; log
  `/tmp/darwin-art-connection-component-tests.log`. **Unlocked exit gate FAILS:**
  `run-active-services.sh --unlocked-notification-boundary` proves connected()
  retains an outer monitor through reentrant bind/link; log
  `/tmp/darwin-art-connection-notification-open-gate.log`. Notifications and
  launcher must move outside lock; no rejection/early-success workaround.
  Ambiguous link failure also needs proven typed absent-unlink outcome at Binder
  JNI: generic NoSuchElementException is not success. Support DEX PASS102/208,
  verified DEX38 classes233/methods1582, no fixtures. Both pinned native products,
  graphics/headless source/link closures (fixture exports0), reachable graph
  probes0 and negative controls PASS; exact two-product repeat is no-work.
  Logs: `/tmp/darwin-art-connection-{products,graphics-boundary,headless-boundary,
  graph-boundary,graph-negative,warm}.log`. Scoped Rust formatting and diff check
  PASS. These product/audit results precede the following notification slice.
  Current turn: ServiceNotificationController owns immutable publication admission,
  per-exact-record in-flight ordering and unlocked connected transport. Genuine
  inbound BIND completion settles before outbound callbacks. Old failure tails
  cannot erase replacement rows; publication invalidation retains real in-flight
  claims until transport returns. Latest publication wakes after plain Remote,
  suppressed Remote and unchecked failures while preserving the primary error.
  Full AS and the unlocked gate, connection index, launcher and registry fixtures
  PASS. Null publication and admitted detach-before-claim are covered. Astra final
  source review reports no remaining must-fix in this notification slice; launcher
  outer-monitor retention remains OPEN. The pinned Binder JNI now owns a read-only
  ABSENT/PRESENT/UNSUPPORTED query after failed public unlink; archive compile and
  controlled query fixtures PASS, genuine JNI actors still OPEN. Production support
  DEX PASS: sources103/classes215, verified DEX38 classes243/methods1616, no fixtures.
  Both current pinned native products rebuild PASS; graphics/headless source/link
  closures PASS with fixture exports0. Reachable production graph probes0 and
  negative controls PASS; exact two-product warm repeat reports no work. Scoped
  diff and shell syntax checks PASS. Scoped rustfmt check exposes a pre-existing
  build-contract test formatting difference; no blanket formatting claim or
  unrelated rewrite. Logs `/tmp/darwin-art-notification-*` and
  `/tmp/darwin-art-recipient-presence-native.log`. No current-turn runtime restart,
  APK modification, profile reset, commit or push; live apps still use the previous
  generation and do not validate these newly built sources. Revalidated live
  actors: system60597, Calculator60599, Clock61497, Chrome61937/61954/61955;
  child61958 exited naturally. Next Astra review calls for an Android-owned
  process launch controller with shared exact process demand and opaque
  prepare/activate/retire handles. Cancellation must retain an admitted activation
  tail until it returns; attachment-owned reservations hand off, not kill.
  Cancellation ACK is not reap proof. This remaining design is not implemented.
  User approves one-time default development runtime replacement. Revalidated
  live system72784 + Chrome children74509/10/11, all daemon57879-owned; TERM those
  four exact actors and normal ctl shutdown retire them. No APK/data/profile
  reset. Fresh debug host and release daemon/ctl build PASS; daemon59996/system60597
  own Calculator60599, DeskClock61497 and Chromium61937 (+service children61954/
  61955/61958). Physical macOS HID Calculator2+3=5 and ClockTimer switch PASS;
  Retina composition720x1280 for logical360x640. Calc/Clock title still leaks
  package name, an OPEN localized-resource regression. Initial Clock launch
  rejects generation replacement because main omitted two forwarded environment
  values; identical-environment retry reuses system60597 and succeeds. No guard
  bypass or extra kill. Chromium menu physical click PASS; fresh Dawn/MoltenVK
  VkDevice log exists, but Example Domain body is visibly blank: **FAIL**, not
  Vulkan webpage acceptance; physical menu reload still leaves blank body.
  Screenshots `/tmp/darwin-art-rollover-*.png`; launch
  logs `/tmp/darwin-art-runtime-rollover-{calculator,clock,clock-retry,chromium}.log`.
  All three apps left running without timed shutdown. This authorized legacy
  replacement is not automatic cleanup acceptance. Fresh full APK acceptance
  remains OPEN; no commit, push or goal completion.






Cross-checkpoint status: these ten checkpoints are component progress, never
goal completion. Physical three-APK Retina/Vulkan/input acceptance, remaining
parent/process, shared-demand, wire-terminal and mixed-owner gates stay OPEN.

- **2026-09-18 — Composition payload preparation integrity (PROGRESS):**
  The actual production composition consumer now retains each
  `CompositionBufferLease` before exposing its layer, reserves both payload
  vectors, and treats required lease or append allocation failure as a sticky
  whole-turn rejection. Structural SurfaceControl state appends use the same
  sticky allocation gate. `IsSubmissionPrepared()` is a narrow no-allocation
  status seam for End's typed local rejection; ordinary clear/new-turn paths
  reset it, while failed turns clear all retained payloads without transport.
  The isolated actual-TU fixture passes lease-failure and forced vector-
  allocation-failure cases with balanced ownership and zero producer export or
  remote requests. Existing typed restore-failure coverage now verifies a
  committed completion descriptor remains caller-owned and is explicitly
  closed. No shared native/Cargo/DEX build or APK/profile mutation was used.

**Chrome example.com first priority — live endpoint evidence (2026-09-18):**
User reaffirms actual example.com rendering as priority one. Keep the larger
migration goal active; unrelated cleanup does not precede this acceptance.
Installed Chromium's CompositorFrameSinkClient endpoint error flag (+0x188)
is set, so its real Accept path rejects sends. Pending BeginFrame count100 is
an effect; do not reset the counter or force callbacks. A fresh filtered GPU
observation (52357, browser52090) catches the first endpoint closure at ELF
7eceda0 on VizCompositorThread, with no optional disconnect reason. Installed
ELF stack 7ed47cc/7ed34d0/7eca0e0/7eca9e4 identifies whole-pipe error fanout
after a nonzero Connector handle-ready result, not an isolated validator
rejection. Next observe Connector ELF7eca990 w2 and original trap ELF7eeac10
result/signal state to distinguish genuine peer closure from transport errors.
An earlier unfiltered GPU pause induced a GPU-channel timeout; that run is
measurement perturbation, not root-cause or acceptance evidence. All debugger
sessions detached and quit. Startup-stop observation was removed for normal
indefinite launcher98999. Exact live daemon47588 owns system56091/browser56096/
renderer56143/GPU56147 (all normal S), with no APK/profile reset or fallback.
Latest real macOS window capture `/tmp/darwin-art-chrome-priority-current.png`
is visually inspected at original Retina resolution: Chrome controls and
example.com address are present, but webpage body is white and OCR has no
Example Domain. **Fresh rendering acceptance FAILS**; older successful captures
are not this run's acceptance. Actual Looper owner component runner PASS is
component-only. No behavior fix, shared rebuild, commit, push or completion is
claimed for this checkpoint. Completion of the priority requires a fresh real
page body on the Vulkan path and continued physical reload/navigation updates.

**Named Viz pipe failure captured — browser lifetime next (2026-09-18, PROGRESS):**
Previous priority turn supplied a fresh failing capture and checkpoint, not a
fix. Main revalidates the exact live actors before each authorized TERM/restart.
An initial broad Connector observation63786 sees CrGpuMain result9 twice, but
those are NOT correlated to CFS; do not reuse them as cause. That paused run
has no observed CFS callbacks. Normal no-startup-stop69444 does have real Viz
OnBeginFrame (ELFc5ea0d0): observera00413b80 needs1/suppress0/pending45,
clienta00017800→endpointa0034f000, CLOSED+188=1, exact CFSClient type+1c8.
Thus closed sends precede the counter100 threshold, not caused by that limit.
Filtered fresh73445 (ELFbias334a68000), Connector callback7eca990 restricted
to VizCompositorThread/nonzero w2, yields x0a0033b860, w2=9 and x1 static
string `viz.mojom.CompositorFrameSink` (ELFname17c68b). Stack7eca990/3730598/
7eeb5d8/7eeb744/7eeac3c/33ac724/33acfe4. Pause was brief; no new GPU-channel
timeout observed. This identifies a named primary pipe failure, not yet its
causal browser close or exact associated CFSClient fanout. Astra bounded review
corrects x3: it is NOT signal state at Connector callback. Original trap7eeac10
event x0 has flags+4/context+8/result+10/satisfied+14/satisfiable+18 (hex).
Next startup browser Connector ctor7ec9660 filter x3==browserBias+17c68b;
record this x0 and 64-bit handle[x1]. Follow exact-this destructor7ec99a8,
CloseMessagePipe7ec9db4, PassMessagePipe7ec9df4 and HandleError7ec9f00 to
capture earliest owner stack. Transfer alone is not closure. Main independently
verifies ctor: handle→this+10, watcher+28, interfaceName+158. Router embeds
Connector+60; ProcessNotifyErrorTask7ed6808 can correlate InterfaceEndpoint
at task+80, its client+50 and client name+1c8. Do not assume correlation solely
from pipe names or change frame clocks/counters. Review completed and retired.
All debugger sessions65035/43621/79950/5089/90219 are terminal and detached.
Observation stop removed; current indefinite normal launcher52704 confirmed
live, daemon47588-owned system76117/browser76119/renderer76172/GPU76176,
plus normal spare76217, all S. Real keyboard77960 PASS13 events, fresh capture
`/tmp/darwin-art-chrome-viz-pipe-normal-latest.png` original Retina inspected:
address example.com/controls visible, page body white, no Example Domain OCR.
Fresh render acceptance remains FAIL. No APK/profile reset, fallback, target
function invocation, production behavior edit, shared rebuild, commit or push.

**Root client ownership verified — closure cause still open (2026-09-18, PROGRESS):**
Priority remains unchanged APK example.com rendering, not migration cleanup.
Fresh browser88027 (ELF bias30e534000) hits actual Connector constructor7ec9660
with this=a00374fe0, x3=30e8354e7 (CFSClient), handle[x1]=a0010f6f0.
Actual stack includes9ee3f44/9eeac1c/5151284/5150c88. Astra's bounded installed-
ELF review identifies Android root/slim FrameSink initialization, not a renderer
widget. Crucial topology correction: root CFS remote is associated, while its
CFSClient receiver has a separate new message pipe. The previously observed GPU
primary CFS result9 does NOT by itself correlate to this root client closure.
Next decisive evidence is this root client's first close/error owner stack and
its peer identity. Do not infer that pending100, GPU context loss or clock phase
is causal without that evidence.

Debugger compiled conditions stalled unrelated constructors. Python read-only
filters in `/tmp/darwin_art_mojo_readonly_filter.py` pass bounded mock tests, but
live native breakpoints still stopped unrelated connectors despite false
callbacks and auto-continue. Exclude those unrelated stacks and debugger-induced
latency from causal/performance evidence; live recording-only integration is NOT
verified. No production tracing helper or runtime behavior change was added.
Both LLDB sessions83685/50553 detached and quit; bounded agents retired.
Exact diagnostic actors were gracefully terminated after inventory, including
94024 startup-stop resumed before TERM. Startup-stop is absent from replacement
indefinite normal launcher6409 (shell97884). Daemon47588 owns system98307,
browser98310, renderer98374, GPU98376 and spare98416, all S at final inventory.
GPU98376 logs genuine Vulkan create-device result0, Android AHB/foreign-queue/
sync-fd extensions; no fallback selected. Physical keyboard99090 passes13 events.
Fresh `/tmp/darwin-art-chrome-root-client-current.png` inspected at original
Retina resolution shows Chrome controls/example.com address but white webpage
body, no Example Domain OCR. **Rendering acceptance still FAILS**. Chrome remains
running indefinitely and no debugger is attached. No APK/profile reset, target
function call, shared native/DEX/Cargo rebuild, commit, push or completion claim.

**Exact-object hardware observation preparation (2026-09-18, PROGRESS):**
Previous turn made ownership evidence progress, not a runtime fix. Main reads
the goal/checkpoint and verifies launcher6409 and exact normal actors live before
authorized shutdown. Astra reviews a narrow alternative to unreliable global
software-breakpoint filters: after full root Connector construction, watch base
8 bytes WRITE-ALWAYS, +10 pipe handle8 bytes MODIFY, +f8 error1 byte MODIFY.
Installed destructor7ec99a8 writes the SAME vptr at7ec99c8 and closes the handle
without first clearing it; ordinary modify at base or handle-only watching would
miss destruction. LLDB SBWatchpointOptions write-always support is verified by
review, not yet exercised against the live Connector. Arm only after ctor,
verify all hardware slots, capture first exact-object stack, disable promptly.

Fresh startup-stop55404/browser1708 (bias30e534000) hardware execution BP at
318417f40 (ELF9ee3f40 root receiver-binding call) does NOT capture root lifetime.
Initial non-TTY LLDB51568 EOF detaches; subsequent tty20904 stops at ART null
memory EXC_BAD_ACCESS1165d2dec. SIGSEGV/BUS pass settings do not bypass Mach
exception stop, and changing ignored-exceptions AFTER attach is ineffective.
Browser then logs GPU-channel timeout and aborts; this run is debugger/startup-
stop perturbation, NOT evidence of the normal application's closure cause.
Next observation must configure `platform.plugin.darwin.ignored-exceptions
EXC_BAD_ACCESS` BEFORE attachment and avoid delaying child startup. No absence
of a constructor or unrelated stopped stack is accepted as causal evidence.
Sessions51568/20904/98164 terminal and detached; diagnostic launcher terminal0;
remaining owned system1704 gracefully TERM. Review retired. Broker poll/close
inspection found no proven causal defect; no speculative production fix added.

Restored no-startup-stop indefinite launcher41324 remains live. Daemon47588 owns
system3706/browser3708/renderer3762/GPU3766/spare3806, all S at final inventory.
GPU3766 actual Vulkan create-device result0 with required AHB/foreign-queue/
sync-fd support. Physical keyboard46220 PASS13 events; new real window capture
`/tmp/darwin-art-chrome-after-hardware-observation.png` inspected original Retina
shows example.com address/controls, white body, no Example Domain OCR. **Fresh
render acceptance FAILS**; full migration goal ACTIVE. No debugger attached,
APK/profile reset, target call, production behavior change, shared build,
commit, push or completion claim. `git diff --check` PASS before this append.

**First exact root error captured (2026-09-18, PROGRESS):**
Pre-attach Mach EXC_BAD_ACCESS ignore fixes the previous observation obstacle.
Fresh browser6554/bias30e534000: hardware root call9ee3f40→Connector ctor7ec9660
captures this=a003c2a60, name=30e8354e7 CFSClient, LR316406f70 (7ed2f70).
After ctor return, execution BPs removed and all three exact hardware watches
successfully armed. FIRST event is error+f8 0→1 at7eca0b0; base WRITE-ALWAYS
and handle+10 MODIFY have no preceding hits. Exact x19=a003c2a60. Stack:
7eca0b0/7eca9e4/36ef43c/3730598/7eeb5d8/7eeb744/7eeac3c/33ac724/33acfe4,
then actual ALooper_pollOnce/Java MessageQueue. This proves root client error
before local destruction or handle transfer. Direct caller return7eca9e4 plus
unchanged handle strongly agrees with callback result9; saved w20/trap result
not read, so do not claim direct numeric-result observation. Watches disabled,
resume and detach/quit84034; no GPU timeout observed in this run.

GPU11899 root-ctor-return c6062ac HWBP has0hits; fresh17358 Root factory entry
c603e5c/resultc603f50 HWBPs also0hits. Debugger asynchronous resume reporting
required explicit SIGSTOP; these misses are NOT proof of absent execution or
output-surface failure. Actual ELF review proves factory null-output path can
dispose PendingRemote+50 before ctor, but it is NOT observed as this cause.
Duplicate-root/provider-null server preconditions are fatal CHECKs, not silent
disposal; no corresponding causal CHECK evidence. WSI android-wsi=0 log tests
native extension BEFORE adding the Android shim, not guest unavailability.
Diagnostic sessions85477/96606 detached/quit; exact observation actors gracefully
TERM after inventory, stopped spares resumed first. APK/profile untouched.

Normal browser20347/bias30e534000 read-only snapshot: inline HostFrameSinkManager
at ELF e56e260, +70 proxy=a0008dd60, proxy+8 endpoint=a012b7bc0; endpoint+188=0,
type+1c8 points ELF4c1823 `viz.mojom.FrameSinkManager`. Session49077 detached/quit.
This contradicts a CURRENT closed-parent claim; historical Root-send rejection
still unproven. Next decisive browser observation: root-specific serializer
479f214 capture endpoint x0/message x1; thread-scoped immediate Accept return
7ee6444 verifies x20==endpoint/x19==message and reads w0. Do NOT read bool at
479f218 after tracing cleanup. False proves local send rejection; true proves
only this boundary's acceptance. Browser root send51510d8→9f2267c/9f22784
precedes root-client binding; helper unconditionally chooses Root method, not
non-root. Astra review retired; no speculative production change.

Current indefinite no-startup-stop launcher85685 confirmed live, daemon47588
system20345/browser20347/renderer20394/GPU20398 all S; no debugger attached.
GPU20398 Vulkan create-device result0. Physical keyboard62531 PASS13 events.
Fresh `/tmp/darwin-art-chrome-root-watch-normal-current.png` original Retina
inspection shows address/controls but white body, no Example Domain OCR.
**Render acceptance FAILS; full migration goal ACTIVE.** No fallback, target
function call, shared build, runtime behavior edit, commit, push or completion.

### Checkpoint: example.com priority — exact first Root send accepted

User priority is unchanged Chromium example.com rendering, before unrelated
migration work. Fresh diagnostic browser29318/bias300000000 stopped at actual
Root serializer479f214: endpoint=a036e7980, message=16f8f8570, endpoint+188=0.
Immediate native Accept return7ee6444 had w0=1, x20 matching that endpoint and
x19 matching that message. This proves acceptance of THIS first Root request
only, not delivery, output-surface initialization, frame submission or scanout.
GPU29963 actually imported Android Surface29318:6 at720x1280. No cause/fix claim.
Earlier browser27105 capture missed the return because polling with a blank
newline repeated LLDB continue. Exclude that run from return-value evidence;
poll with empty chars, never newline. Diagnostic debuggers detached/quit and
exact owned actors gracefully terminated after inventory; APK/profile preserved.

Astra installed-ELF review: natural Support OnBeginFrame c5ea0d0 x0=S;
manager=*(S+28), ID at+40/+44, is_root at+1ac. Current root map manager+3b0
data/+3b8 count has16-byte entries {ID8,Root*8}; Root+d0 points to Support.
Verify map ID/support identity before treating first startup error as CURRENT
root failure. +198 client requests frames, +19a effective need, +19b registered;
+199 is NOT general suppression. Review finished without source edits/builds.

Normal indefinite launcher62640: daemon47588 owns system35040/browser35043,
renderer35098/GPU35103 (GPU bias31ca68000), additional child35144 observed.
GPU35103 Vulkan device creation result0. Real Ctrl-L/example.com/Return keyboard
PASS13 events; fresh `/tmp/darwin-art-chrome-example-priority-current.png` OCR
contains example.com/address controls but no Example Domain body: FAIL.
Post-startup GPU one-shot Support breakpoint session74217 had0hits; asynchronous
resume/interrupt required exact SIGSTOP. Therefore this miss is NOT evidence
of absent BeginFrames/root factory. Breakpoint removed, detached/quit terminal0.
Current-root map/error correlation remains outstanding. No runtime behavior edit,
shared build, fallback, fabricated callback, APK patch, commit or push this turn.
Rendering acceptance and migration goal remain ACTIVE; leave normal Chrome live.

### Checkpoint: post-startup Viz observer sampled, rendering still fails

Previous turn classified progress: exact first Root Accept boundary proof changes
next action to downstream ownership/output/frame initialization. Re-read AGENTS,
Current goal/latest checkpoint; normal launcher62640 revalidated live this turn.
GPU35103 role independently confirmed by Vulkan device-result0 log. Attach59321
configured Mach/SIG handling before attach; explicit continue reported resuming
and subsequent interrupt stopped normally (no external SIGSTOP needed this run).
Natural physical tab/new-tab clicks and13-event example.com keyboard navigation
occurred while observing. One-shot HW Support OnBeginFrame3280520d0 and Android
OnVsync3280d2180 each0hits. Do not infer absence globally or root failure from
bounded hits/misses. No current Root map snapshot obtained; correlation pending.

Actual paused VizCompositorThread40226294 stack: poll → OwnerPollMany →
darwin_art_fd_broker_poll_wait → darwin_art_bionic_socket_broker_poll →
ALooper_pollOnce → framework message_queue_native_poll_once → ART JNI trampoline.
GPU/Compositor/Viz threads exist; this is a single idle snapshot, not proof of
deadlock. Existing NDK choreographer log has no35103 lines; logging is bounded
and this alone is not a missing-contract diagnosis. Reviewed narrow
compat/looper/android_choreographer_owner.cc without changing behavior.
All breakpoints deleted, debugger59321 detached/quit terminal0. Fresh physical
capture `/tmp/darwin-art-chrome-example-viz-poll.png` OCR only Chromium/example.com
and controls, no Example Domain: acceptance FAIL. Indefinite62640 confirmed live;
system35040/browser35043/renderer35098/GPU35103 daemon47588-owned S, additional
41576/42085 S observed. No debugger attached. No runtime edit/build/APK change,
fallback, commit/push or completion. Next decisive observation remains actual
Root factory output-result/current ownership, with confirmed debugger resume.

### Checkpoint: physical resize tool and Root factory observation

Previous turn included a verified live wait62640 and a real idle Viz stack
sample, but did not establish a render cause. Re-read AGENTS/current goal/latest
checkpoint. Inventory then graceful TERM exact old actors; APK/profile unchanged.
Diagnostic launcher2412 browser44995/bias30e534000, GPU45159/bias324a68000.
Browser/renderer45154 resumed via exact CONT; GPU attached87953 with pre-attach
Mach/SIG handling, explicit continue reported resuming. Vulkan result0. Root
factory output-return ELF c603f50/live33106bf50 HW one-shot0hits; server
CreateRoot c5f5d18/live33105dd18 HW one-shot0hits. Not an absence/failure proof:
startup debugger can perturb transport/registration and no matching delivered
request was observed in THIS run. Browser snapshot80412 (detached/quit0):
HostManager31caa2260 proxy=a0005df00 endpoint=a014b8b80 error+188=0,
type30e9f5823 exact FrameSinkManager. No closed-parent cause established.

Luna added test-only tools/macos-window-resize.swift; main reviewed and corrected
target corner for drag inset. No guest callbacks/API/state writes: exact PID
CGWindow bounds plus10-step physical CGEvent HID drag, macOS points only,
finite argument/permission checks and independently measured resulting bounds.
swiftc -typecheck PASS; live44995 outer360x668→420x748 PASS exact bounds, then
420x748→360x668 PASS. Captures root-factory-before-resize.png show controls/
example.com; `/tmp/darwin-art-chrome-root-factory-resized.png` original inspection
shows black content, restored capture also no body. Rendering FAIL; geometry
PASS is NOT GPU/layout/render acceptance. Both HW breakpoints remained0hits
during physical resize; no inferred Root recreation. Breakpoints removed and
87953 detached/quit terminal0. New diagnostic stopped49994/49997 resumed before
exact owned44991/44995/45154/45159/49994/49997 TERM;2412 terminal0.

Fresh normal indefinite no-startup-stop launcher50374 live (bootstrap52587 R
verified); keep same handle until actual actors start, not timeout-triggered
restart. Passive installed-ELF current GPU manager locator review dispatched to
existing helper; no runtime edit, native/DEX/Cargo build, fallback, APK mutation,
commit/push or completion. Root ownership/output result still unresolved.

### Checkpoint: CURRENT GPU Root CFSClient error finally correlated

Normal no-startup-stop launcher50374 finished bootstrap: daemon47588 owns
system52819/browser52827/renderer52880/GPU52884. GPU bias325404000, Vulkan device
result0, actual Android Surface imported52827:6 at720x1280, and Vulkan image
creation result0 observed. Therefore this normal run demonstrably progresses
beyond Surface import/resource creation; earlier startup debugger0hits do NOT
describe this run. Physical example.com keyboard PASS13 events; fresh
`/tmp/darwin-art-chrome-root-factory-normal-current.png` lacks Example Domain.

Post-startup attach8731 preconfigured Mach/SIG handling. Natural HW Support
OnBeginFrame ELF c5ea0d0/live3319ee0d0 hit immediately on VizCompositorThread.
Read-only SB snapshot: S=a00443800 ID(0,1), is_root1, M=*(S+28)=a00547e00,
current root map D=a000bac20/count1 contains (0,1)→R=a00382140.
R+d0==S; proxy S+20=a000b86c0==R+40; endpoint proxy+8=a0034ed80==R+20;
endpoint+188=1, type3257054e7 exact CFSClient, pending+200=60,
needs flags+198=01000101. This is CURRENT GPU-owned Root, not merely first
startup CFSClient or an uncorrelated observer. It proves its errored client
state, NOT why the peer closed or when. A physical new-tab click happened
after first breakpoint stop; resume then second read-only interrupt snapshot
still shows SAME map Root/Support/endpoint error1, pending now89. Persistence
across resumed processing excludes attributing this solely to a replaced Root
visible only at the first pause. No ACK reset/forced callback/target call.

8731 detached; next poll reports missing terminal handle, ps confirms no
lldb/debugserver and GPU52884 restored daemon-parent S. Normal50374 live poll
verified; additional54959 S. Fresh `/tmp/darwin-art-chrome-current-root-error.png`
has no Example Domain. Rendering FAIL, goal ACTIVE. Locator helper retired;
its broad startup lifecycle suggestion is not evidence of browser peer closure.
Next cause investigation: browser-side receiver ownership/close matching this
Root identity, not another uncorrelated startup Connector. No runtime behavior
change/shared build/commit/push or completion this turn.

### Checkpoint: browser owns Root receiver; only callback path errored

Previous turn PROGRESS: CURRENT GPU Root identity/error correlated twice,
pending60→89. This turn read AGENTS/current/latest and revalidated normal50374
live; same actors remain daemon-owned. Browser bias300000000 confirmed log.
Read-only installed-ELF/snapshots establish browser counterpart ownership:
inline HostManager30e56e260 unordered-map at+140 has3 nodes; chain head+10,
node key+10, client+18. Root nodea01283330 key(0,1), clienta00543720;
root/create/wait flags are NODE+40/+41/+42=010101, NOT data+40. Preliminary
incorrect data-relative flag read020000 is excluded. Constructor/search helper
9f254a4 returns node base;9f22740/44 writes flags at node+40/+42.

Root client is CompositorImpl subobject B+20, B=a00543700: IDB+40==(0,1),
*(B+60)=LayerTree a006f4380. Actual LayerTree vtable e0e2b0 slot+10 resolves
9eeabc8 SetFrameSink; installed code stores owned FrameSink atLayerTree+20.
Current ownedF=a00e18200/vptr e0de08, BindToClientfn9ee3d58; F+c0==LayerTree+8
actual FrameSinkImplClient, context_provider F+b8=a005b8510 nonnull. F+20 is
task_runner, NOT context; preliminary guessed clientF+b8 excluded. F+b0 variant1
is DirectReceiver, F+40 pending_receiver consumed0. Thus simple browser object
destruction/unbound pending receiver is not established as this cause.

Direct nodeF+78=a016fcea0; bound Receiver stateF+80 routera0039cd40,
F+88 endpointa01271300. Endpoint+188=1/type3003014e7 CFSClient,
router+389=1 and embedded Connector+f8=1/type3003014e7; handlea000fa5b0.
SameF still owned in later snapshot: associated submission proxyF+68=a015ce0f0
matchesF+70, endpointa012710c0 error+188=0/type30017c68b CompositorFrameSink;
HostManager parent endpoint error0. Separate callback receive path is errored
while associated submission/manager endpoint states remain non-error. This is
not proof of wire delivery or why callback transport failed.

Pinned Chromium154.0.8024.0 upstream source retrieved read-only from Gitiles
(web open failed; curl format=TEXT/base64 succeeded): cc/slim/frame_sink_impl.cc,
layer_tree_impl.cc, mojo/public/cpp/bindings/{receiver,direct_receiver}.h,
lib/binding_state.h and direct_receiver.cc. Confirms BindToClient uses
DirectReceiver and ThreadLocalNode::AdoptPipe asynchronously transfers/merges
a portal between process-global/thread-local nodes; async failure can blackhole
messages. This is a mechanism candidate, NOT observed merge-failure proof.
Bounded helper maps installed ThreadLocalNode offsets for next read-only pending
merge snapshot; main retains debug/acceptance. Sessions3013/81359/88089/91622/
72545 all detached/quit terminal0; ps no debugger, all same actors S. No runtime
edit/build/fallback/APK change/commit/push/completion; rendering remains FAIL.

ThreadLocalNode layout mapped by helper and independently verified ctor7ec6e50:
node+8/local_portal+10/global_portal+18/next_merge_id+20/map_size+38.
Read-only snapshot80970: SAME RootB/F, nodea016fcea0 has node_handlea0030a080,
local_portala000f9a50/global_portala000f1510,next_merge_id1,pending_merges0;
receiver error remains1. Detached/quit terminal0. No pending asynchronous merge
is left in this node, so do NOT diagnose an unfinished AdoptPipe transfer from
the earlier mechanism candidate. In pinned production source only successful
retrieval/merge erases the pending entry (non-success CHECK aborts); retained
node/next1/count0 agree with completed first transfer, not proof of later wire
health. Helper finished; no background build/debugger. Next decisive boundary
is FIRST browser callback Connector error/validation after binding, preserving
Root identity; current opaque error cannot distinguish peer closure, validation
failure or transport loss. Avoid repeating the already-excluded missing-bind or
pending-merge hypotheses as causes. Normal50374 remains running.

### Checkpoint: FIRST current Root callback failure is peer-closed

Priority remains real unchanged Chromium example.com rendering. Read current
goal/latest checkpoint and AGENTS. Fresh diagnostic launcher59010, browser74767
(ELF bias30e534000), system74765, renderer75686/GPU75693. Exact Root(0,1)
owned FrameSinkF=a010e7080, LayerTree ownership and F+c0==LayerTree+8 checked;
DirectReceiver variant1. Captured its Connector constructor/return, type
CFSClient, C=a003d9ea0, handlea0004d000, error initially0. Hardware watches on
C base (WRITE_ALWAYS), handle, error: FIRST error watch transitions0→1 at
ELF7eca0b0 on CrBrowserMain. Saved handler result atsp+40 is9; native saved
return is7eca9e4 (OnHandleReadyInternal→HandleError(false,false)). Authoritative
SB GetHitCount: base0/handle0/error1. Same F remains Root-owned and bound router
F+80 points to ConnectorC through router+60. This is not a generic startup hit.

Pinned Connector source and installed code identify result9 as peer-closed
watcher outcome, not local validation/reset branch. ThreadLocalNodea01a53f20
pending merges1 at constructor return→0 BEFORE first error, next_merge_id1.
Initial AdoptPipe merge completed; do not repeat unfinished-merge diagnosis.
After propagation, SAME Root callback endpoint error1; associated submission
and parent manager endpoint errors0. This does NOT identify why the opposite
peer/transport closed. Next causal boundary: matching GPU Root callback portal
and ipcz/transport closure, not missing BindToClient or local handle reset.

Deleted all three watches, detached74767, quit LLDB20079; terminal missing and
ps confirm no debugger. Inventoried exact four diagnostic actors (all S), sent
graceful TERM, preserved APK/profile/daemon47588. Started indefinite normal
launcher74819 WITHOUT DEBUG_STOP_AT_CHROME; restoration/capture pending below.
No runtime behavior edit, shared build, fallback, APK change, commit or push.
Real page rendering remains FAIL; goal ACTIVE.

Normal restoration verified: launcher74819 live, daemon47588 owns system82337,
browser82340, renderer82391 and GPU82395, all S with no debugger or STOP env.
Physical HID CLI delivered13 Ctrl-L/example.com/Return events PASS. Fresh
capture `/tmp/darwin-art-example-current-peer-closed.png` OCR has Chromium:
New tab/example.com, NOT Example Domain. Page body remains black; acceptance
FAIL. Leave indefinite normal Chrome running for user, no expiry/termination.

### Checkpoint: real socket EOF evidence, Root correlation still required

Previous turn PROGRESS: first exact Root callback peer-closed narrowed next
boundary. Re-read AGENTS/current/latest; revalidated normal74819 and actors.
Read-only GPU82395 bias32fd04000 attach29635: one-shot hardware
OnBeginFrame(c5ea0d0/live33c2ee0d0) explicitly resumed; physical browser tab
click CLI41357 PASS, breakpoint hit count0 during observed interval. Thread
inventory confirms CrGpuMain/VizCompositorThread/CompositorGpuThread. This is
NOT proof of absent callbacks in general or wrong GPU identity. Deleted BP,
detached, quit; ps confirms no debugger. No target calls/field edits.

Inventoried four exact remaining actors, graceful TERM; preserved APK/profile,
daemon47588. Fresh indefinite launcher19629 WITHOUT STOP/debugger, existing
DARWIN_ART_DEBUG_SOCKET=1 enabled (no rebuild). Current system87455/browser87459
(bias30e534000), renderer87514/GPU87518(bias31d378000), spare87556 all daemon
parent S. Actual socket logs: browser87459 fd1073753126 recvmsg result0 errno0;
GPU87518 fd1073743887 initially110 bytes then result0 errno0, epoll DEL follows.
Other channels continue successful SCM_RIGHTS (including2 rights/206 bytes).
No negative sendmsg/recvmsg result for these browser/GPU IDs in checked log.
These EOF sockets are NOT YET matched to current Root callback; temporary
transport closure may be legitimate. Do not label this root cause. Next probe
must connect current Root portal identity to actual closing route/transport.

Source audit: active broker adapter translates ancillary FD ownership; the
older socket facade's no-control support is not evidence of this failure.
Broker recvmsg copies raw Darwin output flags (observed0x80); Android/Darwin
MSG_CTRUNC/TRUNC/EOR values differ. ABI debt observed in source, NOT established
cause; no speculative patch/extension of oversized adapter. Bounded read-only
Luna helper maps installed ipcz close/disconnect symbols; no shared build.

Fresh physical13 HID URL events94845 PASS; capture
`/tmp/darwin-art-example-socket-eof.png` OCR Chromium/example.com, no Example
Domain. Body remains black: rendering FAIL, goal ACTIVE. Launcher19629 live
poll verified; log18MiB, no debugger, leave indefinite app running. No runtime
behavior change, APK reset, fallback, commit/push or completion this turn.

Helper completed bounded ELF mapping: ThreadLocalNode destructor7ec7504,
AdoptPipe7ec75ec, transferred-portal handler7ec7848, indirect Get site7ec78ac,
Merge site7ec794c and pending erase7ec7f18; no reliable direct ipcz.Close or
transport-disconnect symbol recovered from stripped ELF. Main independently
checked getter33a6b68: it loads global pointer at ELF **e54ad00**, NOT helper's
e54a0d00 (extra-zero typo). Main verified indirect Merge return350006e0 checks
nonzero before pending erase. Do not treat an indirect callsite as a resolved
underlying close/transport function. Existing production source confirms local
DirectReceiver transport pair can establish in-process nodes; raw EOF without
Root/route identity does not prove causality. Helper done, no background worker.

### Checkpoint: FIRST exact Root route disconnection originates socket EOF

Previous turn PROGRESS (socket EOF/address map), not completion. Re-read
AGENTS/current/latest and live19629/actors. Read-only browser87459 session58370
locates SAME Root(0,1) owned/bound FrameSinka00e14600, DirectReceiver1. Router
a003ba200 contains embedded Connector **r+60**, NOT *(r+60); preliminary wrong
dereference returned a vptr/static address and is excluded. ActualC=a003ba260,
typebias+3014e7,error1, endpoint1, handleH=a00140c10/vptrELF875df0. Node next1,
pending0. Detached/quit, no debugger. APIObject source says handle is object
pointer; pinned ipcz has NO Portal wrapper class: Router implements kPortal.
H+20 is TrapSet, NOT Router pointer; preliminary guessed router read excluded.
Main checked relative vtable875df0 slot0→Router::Close34d6024→CloseRoute34d6044;
compiled mutex+10 matches source; +18peer/+19disconnected/+1cstatus confirmed
below. Do not silently preserve earlier helper's global API-table attribution:
33a6b68 loads e54ad00 pointer but getter semantics need independent proof.

Inventoried four actors, graceful TERM (19629 terminal0), APK/profile unchanged.
Fresh diagnostic launcher36711, system96442/browser96445 bias30e534000.
Read-only45670 attaches with Mach faults ignored/pass-through before attach.
One-shot HW Direct bind9ee3f40 hits CrBrowserMain: F=a00e68480 is exact owned
Root and F+c0==LayerTree+8. Connector ctor7ec9660/return7ed2f70 hits exact
CFSClientC=a00326de0,H=a000564b0,vptr875df0, Cerror0,H peer/disconnected0.
Children97184/97191 were exact-owned T and continued, not modified guest state.

HW watchH+19 thenC+f8 captures FIRST **H disconnected0→1** at PC34d77b0,
installed store34d77ac: strb1,[x19,#19]. x19==H, linktypex20==0. Cerror still0;
watch counts1/0; Root still owned/handle unchanged, nodepending0/next1.
Source Router::AcceptRouteDisconnectedFrom agrees exact installed entry34d772c;
ordinary Router::Close/AcceptRouteClosure differs. Saved nativeFP chain:
34b9ce0→34d7c04→34b9ce0→34d7c04→34dbf80→34c78dc→34b8f7c→33b5008→7ab8e30.
This is actual disconnection propagation, not inferred from stale error flags.

Main traces33b5008 to ChannelPosix fd-read callback33b4db0 through error dispatch
33a1e54. OuterFP16b5adf60; saved caller x19 in next innerFP16b5adf00+18 yields
Channela002eef80/vptrELF79c398, ch+70==1073746986. Same watcher caller saved
fd atouterFP+48 matches. ACTUAL same-run log browser96445 tid40535222 recvmsg
fd1073746986 result0 errno0. Thus this specific EOF is now correlated to actual
Root callback disconnection. OtherEOF1073744927 remains uncorrelated. It does
NOT yet establish opposite process/socket identity or who closed it.

After resume, Cerror watch0→1 PC7eca0b0, savedresult9/caller7eca9e4, counts1/1;
sameRoot ownership+handle preserved. Router disconnection precedes Connector
peer-closed, initial merge complete. Deleted watches/detached/quit; no target
calls, forced ACKs, runtime edits or shared builds. Channeldelegatea0005fd80
(Transport delegate subobject, not assumed full object) retained atch+28.
Next decisive boundary: originating channel's opposite endpoint closure and
Transport/NodeLink identity; do not re-investigate missing-bind/unfinished merge.

Inventoried actors96442/96445/97184/97191/98265, CONT exact spare98265 then
graceful TERM; spare naturally gone before TERM (no such process), others sent.
Started normal indefinite44604 WITHOUT STOP env/debugger; capture pending below.
Rendering remains FAIL, goal ACTIVE; no fallback/APK edit/commit/push.

Helper completed independent pinned source/ELF layout confirmation: Router
+18peer/+19disconnected/+1cstatus/+20TrapSet/+38outward primary/+40decaying/
+48inward/+50bridge. Source transport ownership chain is Channel→Transport
error activity→DriverTransport→NodeLink transport error→matching sublink Router
link disconnection→AcceptRouteDisconnectedFrom; not why remote endpoint closed.
Beware raw ch+28 is Channel::Delegate subobject address, not automatically full
Transport base (relative vtable negative offset-to-top=-10 at79c2c4). Main
retains narrow lifetime interpretation rather than treating raw delegate as
unadjusted ObjectBase pointer.

Normal44604 live restored without STOP/debugger: daemon47588 owns system2628,
browser2632, renderer2684/GPU2688/spare2730 (all S in inventory). Physical13 HID
URL events80857 PASS; fresh capture `/tmp/darwin-art-example-root-route-eof.png`
OCR Chromium/example.com, no Example Domain. Rendering still FAIL. Leave
indefinite app live. Helper finished, no background workers or debugger. No
runtime behavior edit/shared build/commit/push this turn. Next read-only target
is corresponding opposite endpoint close/route metadata, now grounded in exact
originating Root fd EOF rather than uncorrelated socket observations.

Astra high read-only review `root_transport_eof_review` completed: approved
mapping reciprocal kernel Unix socket/PCB identities BEFORE EOF and retaining
all native aliases. SDK proc_pidfdinfo exposessoi_so/soi_pcb/unsi_conn_so/
unsi_conn_pcb; these are opaque equality keys, not dereferenceable addresses.
Main independently verifies lsof2632 Unix DEVICE→peer keys available and SDK
definitions. recvmsg host_fd is temporary exported dup, not persistent ownerFD;
numeric FD reuse or routine duplicate-close is NOT causal evidence. Main read
export_for_scm implementation3370 and broker CloseImpl430/deferred loop129:
final owner close can be deferred until active operations end, so retirement
worker stack alone loses initiator. Next correlate token/generation→description
object→kernel identity, capturing first opposite close/shutdown/exit. Source
does not yet show premature ownership loss; do not suppress EOF or extend
lifetime speculatively. Additional diagnostics must be narrow host-socket
identity/lifetime owner, not grow mixed adapter or Chromium-specific policy.
Luna is implementing test-only `tools/macos-unix-socket-peers.cc` for read-only
exact-PID enumeration; no runtime edits/shared builds. Its validation pending.

Test-only CLI completed/reviewed by main. Strict clang++ C++17 Wall/Wextra/Werror
libproc compile PASS to `/tmp/darwin-art-unix-socket-peers`; usage/zero/overflow/
9-PID rejection64 verified by Luna. Main actual missingPID99999999 exposed
proc_pidinfo returns0+ESRCH, not negative: fixed zero-with-errno handling for
list queries and zero-error socket query handling. Recompile PASS, missingPID
now status2/PIDmissing. Current exact2632/2688/2684/2628/47588 enumeration
status0,108 Unix socket rows, no errors. Reciprocal BOTH socket+PCB identity
match yields browser2632 fd104↔GPU2688 fd73, fd117↔fd67, fd189↔fd101. These
are baseline live channels, NOT the already-closed Root channel. All identity
strings kept losslessly (no numeric JS64-bit rounding). CLI is diagnostics-only,
outside production source/build graph; no APK/runtime behavior change.

Additional read-only native ownership locator: sessions14461/44260 exact2632,
native module scoped globals/types unavailable (no DWARF), but g_process symbol
resolves1099eeb90. g_process points1416754d0. Main compile-only static_assert
UnixEndpoints size88 PASS; Process broker+88 points138008000. Compile-only
fd_broker.cc record-layout dump confirms broker slots+112/Slot size88,
generation+0/live+5/description+24; Description object+152/refs+176/active+184.
No native binaries rebuilt. Kind SOCKET is4; preliminary filter3 yieldedempty
map and is excluded. Correct filter4 yields canonical guest→host map including
40000413→104,40000418→117,40000821→189, which match reciprocal CLI live channels.
This provides token→description→HostFdObject→persistentFD mapping for next
PRE-EOF capture, not proof of premature close. Native symbols retained for
CloseImpl fdd6dc, OwnerClose fe4340, guest socket close1007fd4 (DSO-relative).
Both debugger sessions detached/quit, no target evaluation/calls or mutations.
Workers completed. Normal44604 stays live; fresh page capture remains FAIL.

### Checkpoint: example.com remains first — Root EOF endpoint identified

2026-09-18: user explicitly reaffirmed real example.com rendering as first
priority. No unrelated migration work or runtime behavior change this turn.
Exact diagnostic browser16582 (ELF bias30e534000), Root frame sink a00e17580
owned/bound, Connector a002a6a20, Router a00058050. Constructor-return native
FD and kernel snapshots were taken before resume. First Router+19 watch fired
at ELF34d77b0; Connector+f8 watch count remained0. Manual FP chain reproduces
34b9ce0→34d7c04→34b9ce0→34d7c04→34dbf80→34c78dc→34b8f7c→33b5008→7ab8e30.
Origin Channel a002f8180 has guest socket4000801b (1073774619), independently
matching caller saved fd and daemon recvmsg result0/errno0. Native slot27,
generation32, description13ff90988/object600001c55440 maps to persistent
hostFD203, refs1/active0. Kernel reciprocal socket AND PCB keys identify
browser16582 fd203 ↔ GPU17619 fd108; GPU native token4000080d maps108.
Both descriptors still existed at first EOF. Browser socket state290 vs GPU258.
This identifies the opposite process, NOT proof of premature close: shutdown,
including local read-side shutdown, must be traced before assigning causality.
Origin token was created after the pre-constructor snapshots, so those do not
capture its earlier lifetime. Next capture first shutdown/close initiating
stack for this channel, not uncorrelated recvmsg temporary dup closures.

Luna completed test-only tools/lldb_socket_broker_readonly.py. Main actual LLDB
test exposed hyphen module import rejection and GetName demangled-name filter
failure; Luna renamed and checks either mangled/demangled name. Syntax check
PASS; corrected actual stopped-process invocation still pending. Diagnostic
only, no production link dependency or native rebuild.

Both debugger sessions38740/64586 detached and quit. Exact diagnostic actors
gracefully terminated (spare21742 had already exited). Normal indefinite31055
restored WITHOUT STOP env/debugger: system24593/browser24598 and children
24645/24649/24694 all S. Fresh capture
`/tmp/darwin-art-example-priority-current.png` OCR Chromium/example.com, no
Example Domain. Rendering still FAIL; leave app live. Goal ACTIVE; no fallback,
APK/profile reset, commit or push. Read this checkpoint for next continuation.

### Checkpoint: read-only shutdown tracing validated; causal order pending

2026-09-18 continuation: prior turn classified as progress (reciprocal GPU
endpoint identification changes next experiment). Re-read AGENTS/current goal/
latest checkpoint; authoritative inventory confirmed normal24593/24598 live.
No production behavior change, native/DEX build, APK/profile reset or fallback.
Luna added test-only shutdown_trace callback to lldb_socket_broker_readonly.py;
main reviewed, suppresses unrelated PerfettoTrace events and records x30 at
function entry (x29 chain alone omits direct caller). Callback performs only
register/memory reads and always returnsFalse, avoiding startup pauses. Main
removed invalid command registration: callback is installed through
SBBreakpoint.SetScriptCallbackFunction, not invoked as an LLDB command.
Syntax check PASS. Actual stopped browser27362 snapshot emitted correct native
socket rows, resolving prior import/symbol errors. Actual browser40943 callback
emitted native ownership and bounded savedLR chains without stopping.

Rejected diagnostic27362: GPU startup delayed by attach/Perfetto stops; fatal
browser_gpu_channel_host_factory.cc49 GPU channel timeout. This is diagnostic
perturbation, not original rendering cause. Exact debugger sessions detached;
dead diagnostic launcher26970 explicitly inventoried then TERM, terminal143.
Diagnostic32362 bias31ca68000 differed from earlier30e534000: reused ELF
hardware breakpoint address was wrong, so missing hits are excluded. Always
resolve current bias before setting custom-loaded ELF hardware breakpoints.

Improved diagnostic40943 bias31ca68000, correct Root Direct bind32694bf40,
frame sink a034e3700; ctor324931660 captured C=a003d7ce0/H=a00055f00 and exact
viz.mojom.CompositorFrameSinkClient. Browser native shutdown observer saw
Chrome_IOThread token40005421 host125 and40005821 host128, how2; direct caller
ELF33b4c4c and saved chain7a434bc→7a5dfe0→7a5db3c→7aea2e0→7a5e680→7a1ed58→
7a835c0→4956030→7a8374c→7a9b0e0. CrBrowserMain token40004419 host195/refs1,
how2, direct caller33b4c4c, chain7a434bc→7a5dfe0→7a5db3c→7ab80bc→7ab8058→
7ab7c88→native. Daemon independently has EOF same40004419. This may be normal
Channel error cleanup, NOT proven initiating shutdown. Constructor RETURN
address was miscalculated: correct relative7ed2f70 (live32493af70), NOT
7eccf70/live324934f70. No first-disconnect watch captured in this run; no causal
claim from shutdown events. At ctor ENTRY Router H already exists: next install
H+19 watch there, avoiding unnecessary constructor-return breakpoint.

Next decisive experiment: before browser resumes Root Direct bind, attach the
already-initialized GPU and install same native shutdown callback there. Keep
browser callback on ALL threads (Chrome_IOThread matters; CrBrowserMain-only
filter misses events), skipping PerfettoTrace. Resume then ctor ENTRY→H+19
watch. At first disconnect correlate kernel reciprocal endpoint/token and
observer events from BOTH processes; trace first close if no prior shutdown.
Continue startup-paused child actors promptly to avoid 10sec GPU timeout.
All diagnostic debugger sessions detached/quit; actors resolved then graceful
TERM. Normal indefinite71225 launched without STOP env; final inventory and
capture appended below. Rendering remains unproven/FAIL; goal ACTIVE.

Final normal71225 inventory: daemon47588 owns system45073/browser45083 and
children45138/45142/45185, all S, no debugger/STOP env. Physical13 HID URL events
PASS. Fresh `/tmp/darwin-art-example-shutdown-trace-current.png` OCR Chromium/
example.com only, no Example Domain. Rendering still FAIL; leave Chrome live.
Helper completed; no background agent work/shared build/commit/push.

### Checkpoint: Root portal closure differs from transport EOF

2026-09-18: example.com remains first priority; rendering NOT accepted. No
production change/build, APK/profile reset, fallback, commit or push. Read
AGENTS/current goal/latest checkpoint. Two startup diagnostics captured the
exact Root CFSClient first Router+19 transition and native FP chain. First
browser46897 mapped originating token40003c1a to canonical host204; opposite
endpoint had already disappeared. Second browser51597 token40003420/host197
was kernel-reciprocal to GPU52779 token40000412/host112. Before GPU close both
had state290 (receive-closed; NOT proof both directions shutdown). GPU recvmsg
result0 preceded its observed close: that close is cleanup, not a proven cause.

Third diagnostic59881/61096 armed actual host shutdown observers at Chrome
load, before entry, plus guest close observers. Exact Root(0,1) F=a032a8e80,
C=a03e70060, H=a00062470 began peer/disconnected/status0. After physical URL
navigation H+18=1, H+19=0, status3 and C+f8=1; +19 watch count0. Browser host122
and GPU97 retained reciprocal socket AND PCB identities, both state258 even
after portal error. GPU95 was a transferred duplicate of browser122; its close
left both real endpoints open. Thus orderly portal-peer closure, not physical
EOF, initiates this observed Root failure. Other runs' EOF evidence remains
valid but is not a universal cause. Screenshot
`/tmp/darwin-art-example-host-shutdown-diagnostic.png` has no Example Domain.

Test-only lldb_socket_broker_readonly.py now records bounded256 host-Python
events; actual host shutdown and guest close callbacks emitted native broker,
manual saved-LR and bounded kernel peer metadata. Main review retained all
aliases including disconnected local endpoints, rejects CLI errors, and keeps
64-bit kernel keys as strings. py_compile and git diff --check PASS. No runtime
link dependency introduced. Both debugger sessions82704/12543 detached/quit;
exact diagnostic actors59879/59881/61088/61096 gracefully TERM, launcher94788
terminal0. Normal indefinite73968 launched WITHOUT STOP/debugger; bootstrap
observed live. Final normal capture/actor inventory remains pending below.

Astra pinned Chromium154 review: Root::Create may return null after
CreateOutputSurface failure BEFORE binding pending CFSClient; params destruction
then closes it. This is a candidate, NOT proven this run. Next watch exact H+18
and +19 at constructor entry; GPU factory exact request/client handle, nullable
result and first matching portal Close distinguish declined factory from bound
Remote destruction. Prior verified ELF factory c603e5c/result c603f50 and
manager c5f5d18 are being independently revalidated, no guessed breakpoints.
Goal ACTIVE; real Graphite/Dawn/Vulkan/MoltenVK example.com body is the gate.

Final normal73968 inventory: daemon47588 owns system72254/browser72257/
renderer72307/GPU72311/spare72360, all S, no debugger. Launcher defaults to
target/debug/darwin-art-host (diagnostic used separately signed _build host);
do not assume the executable byte hashes are identical. Physical13 HID URL
events64018 PASS. Fresh `/tmp/darwin-art-example-priority-normal-latest.png`
original Retina image inspected: bottom Chrome controls and white empty body,
no Example Domain/address visible; rendering FAIL, not a shortcut PASS. Leave
normal app live indefinitely. Astra bounded ELF mapping followup active; no
implementation/build running. Next watch orderly portal peer-close with exact
GPU factory ownership/result; earlier +19-only experiment cannot capture it.

### Checkpoint: precise GPU factory lifetime boundaries prepared

2026-09-18 continuation: preceding turn PROGRESS (orderly portal closure with
live physical transport changed the causal experiment). Re-read AGENTS/current
goal/checkpoint; normal73968 handle confirmed live, actors72254/72257/72307/
72311/72360 all S. Priority remains actual example.com, larger goal unchanged.
Astra independently verified installed ELF manager entry c5f5d18 x0=M,x1=&P;
request IDs P+0/+4, pending client handle P+50/version+58. Output-result c603f50
tests x0 null; manager return c5f5e10 exposes nullable Root BEFORE params
destruction at c5f5e24. Destructor479e6b8/+6bc closes client P+50. RouterClose
34d6024 exact handle filters pending cleanup versus bound Remote lifetime.
Created test-only tools/lldb_chrome_root_lifetime_readonly.py: reads these
boundaries, bounded128 host-Python events, manual saved-LR chain; never writes
target state or invokes guest code. py_compile/diff-check PASS; actual callback
integration still pending, do not claim factory observed.

First diagnostic46239 browser74599 stopped at libchrome load. Fresh vmmap plus
ELF header/instruction read validated bias31ca68000 (NOT prior30e534000).
RootDirect BP32694bf40 hit0. GPU75707 was alive but had NOT loaded libchrome;
readonly sample shows Java MessageQueue poll/Binder pool, no Viz thread.
Browser genuinely fatal browser_gpu_channel_host_factory.cc49 waiting for GPU
channel, so this run cannot establish output-factory decline. Debugger66750
deleted BPs/detached/quit. Exact old actors terminated, late stopped spares
75984/76139 inventoried then CONT/TERM. Second diagnostic24932 started without
verbose socket tracing, explicit ELF-image bias logging; same handle must be
polled, not timeout-triggered restart. No runtime/DEX/native builds or APK/profile
changes, fallback, commit/push. Restore normal app before final handoff.

Second24932 browser78159 bias300000000/GPU78691 bias324a68000. RootDirect
309ee3f40 hit, F=a010e4380; exact CFSClient constructor307ec9660 hit.
IMPORTANT x1 is POINTER to pending handle, not Router: dereference yields
H=a00062950, allflags0 before resume. Incorrect initial stack watches removed
before resume. Correct H+18/+19 watches captured +19 first at3034d77b0 with
H+18=0/+19=1 and Connector error still0; this is again a transport-disconnected
run, not proof all generations take the previous orderly path. FP chain matches
prior34b9ce0→34d7c04→34dbf80→34c78dc→34b8f7c→33b5008→7ab8e30.

GPU initial manager/result addresses had arithmetic error (330a... instead of
3310...), corrected before browser ctor resume, but manager entry already
passed. Correct manager-entry count0, output-result1/manager-result1/Close1;
callbacks emitted no owned-request events because request was never pinned.
Do NOT infer result null/success from these counts. Added read-only output
recovery from installed factory x21=&params/x28=M, independently confirmed
by bounded disassembly c603e88/e8c; client P+50 not moved at output return.
This prevents silently discarding the decisive result if entry observation is
missed, with explicit recovered_at metadata and exact(0,1) gate. Syntax/diff
checks PASS. Debuggers71092/29984 removed watches/BPs, detached/quit. Exact
owned old actors TERM,24932 terminal0. Third6797 startup live pending; compute
all live addresses programmatically from current bias to avoid transcription.

Third6797 decisive scope: browser82867 freshbias300000000, GPU83476
freshbias326a88000. All four GPU HW BPs computed programmatically/installed
before Chrome entry: manager33307dd18/result33307de10, output33308bf50,
RouterClose329f5e024. Before first browser error manager/output/result counts
all0; unfiltered RouterClose count10, no pinned Root request/events. GPU really
created Vulkan device and has VizCompositorThread/Chrome_ChildIOThread, so
startup absence is not inferred from a missing thread. Browser exact Root
F=a010e6e00, C=a01d70420, H=a00062540 constructor verifies CFSClient/allflags0.
H+19 first watch3034d77b0: peer0/disconnected1, Cerror0. In THIS run channel
error precedes observed Root factory entry; do not implement output-decline
fix merely because source permits it. Manual originating Channel=a01e11200,
token4000ac1a/host182, native refs1/active0. Kernel reciprocal SO AND PCB
matches GPU106/native token4000080d/refs1/active1. Browser182 state290 but
GPU106 state258. This suggests tracing browser-local receive shutdown as well
as peer teardown, NOT proof of a particular raw-FD owner. Astra review requested.

Next combine browser AND GPU actual-host shutdown software observer pre-entry
with verified GPU factory hardware observations and browser H+18/+19 watch.
Current run had no early shutdown observers: no negative shutdown inference.
All debug sessions16387/24674 removed watches/BPs/detached/quit. Exact owned
82864/82867/83469/83476/83936 inventoried; stopped spare CONT before TERM;
6797 terminal0. Normal indefinite62326 started same signed _build host WITHOUT
STOP/debugger/verbose socket trace; samehandle live startup, await actors then
capture before handoff. No runtime behavior changes/native build/commit/push;
new tools strictly diagnostic. example.com remains FAIL/unproven; goal ACTIVE.

Final normal62326 live: system86858/browser86861/renderer86909/GPU86911/
spare86951 all S under47588, same signed _build host, no debugger/STOP env.
Physical13 HID events38996 PASS. Fresh original Retina screenshot
`/tmp/darwin-art-example-factory-trace-normal.png` inspected: address example.com
and controls visible, WHITE blank webpage body, no Example Domain. Rendering FAIL;
leave app indefinite. Astra notes XNU peer-send-shutdown would normally mark
GPU CANTSENDMORE too; browser290/GPU258 better fits local read shutdown, but
an in-progress SHUT_RDWR transient also fits. Need syscall return and post-state
before attribution; review official main, not exact host XNU tag, remains caveat.
Read actual RetireWireConnection1163: raw shutdown(owner->fd,SHUT_RDWR)1168
and registry const int fd (no owned kernel descriptor pin). This is a candidate
resource-lifetime boundary, NOT proven initiating call in this run; don't patch
it or grow Binder monolith on speculation. Early host-shutdown+factory trace
next. Syntax/diff checks PASS, no runtime build/behavior fix/acceptance claimed.

### 2026-09-18 — First priority: unchanged Chromium example.com rendering

Combined early browser/GPU shutdown observers with exact Root factory capture:
browser89513/GPU89946. Owned Root(0,1) output surface42952522752 and
Root42953351488 were NONNULL before browser Router disconnected at34d77b0.
Output-factory decline is not the initiating observed failure in this run.
Browser186 EndA state290/GPU113 reciprocal EndB state258 joined by socket AND
PCB identities. Old browser159/186 closes include reused FD numbers and transfer
aliases: proximity alone is not causality. No matching shutdown observed in
armed browser/GPU interval; other actors/bounded history remain coverage limits.

Private test-only tools/macos-socket-alias-lifetime-test.cc reproduces EOF
without Android/Chrome or shutdown: send SCM_RIGHTS, close last sender alias,
delay receive yields alias290/peer258, peer send succeeds but recv0/POLLHUP.
Both stream and datagram carriers reproduced; timing-dependent, not proof of
Chromium's exact cause. Each control completed30 separate-process trials: dup,
watched-dup, receive-before-close, retained sender alias until import THEN release
and delayed positive-byte exchange. All120 controls PASS; private C++ compile
warnings-as-errors PASS. Latest Root helper host-only mock tests5 PASS; actual
GPU trace preceded latest nested-request bookkeeping fixes.

Astra found no private-test ownership/control-buffer confound. Published
XNU12377.121.6 unp_gc can sorflush message-only socket references without
userspace shutdown; exact running12377.161.14 source unavailable. Chrome GC
attribution remains unproved. Next join actual EndA transfer owner (direct Mojo
SCM or runtime Binder carrier) before narrow provider fix/ADR. Bounded sender
lease until receiver import acknowledgment must remain runtime-owned protocol;
never inject application ACK bytes, modify APKs or permanently retain extra FDs.
Preserve Android sendmsg/close semantics. Direct broker SendMessageOnHostSocket
currently closes exported host aliases immediately after host sendmsg; source
inspection identifies a possible boundary, not proof it carried this EndA.

Diagnostic debuggers detached/quit and exact diagnostic actors retired. Normal
signed host restored without STOP/debugger/verbose trace: daemon47588,
system99953/browser99967/renderer120/GPU124/spare162 all S, left indefinite.
Physical13 HID events PASS. Fresh original Retina image
/tmp/darwin-art-example-combined-shutdown-normal.png inspected: example.com
address/controls visible, BLACK blank body, no Example Domain. Rendering FAIL,
goal ACTIVE. No runtime/DEX/native runtime build, APK/profile modification,
fallback, commit or push this checkpoint.

### 2026-09-18 — Separate-process lifetime controls and transfer attribution

Previous turn classified progress: native EOF reproduction and successful
release-after-import controls changed the next diagnostic boundary. Re-read
AGENTS/current goal/latest checkpoint. First priority remains Chromium webpage.
Private test now records both endpoint SO/PCB and reciprocal identities before
transfer/after import; warnings-as-errors compile PASS. A further30 trials per
carrier classified actual recv0/byte0/POLLIN|POLLHUP/errno0 separately from other
errors: stream5 PASS/25 verified EOF, datagram4 PASS/26 verified EOF, OTHER0.
Earlier suppressed-output stress25 nonzero percarrier did not classify individual
errors; do not substitute those counts for the explicit classification above.
Release-after-import delayed exchange with reciprocal identity capture PASS;
previous120 positive controls remain verified. Root host mocks5 PASS again.

Source attribution: Binder deposit retains image through publication response;
Binder TAKE prepares owned TransferDelivery, sends response then SCM envelope,
and drops its owned descriptors before client receive/import. Direct socket
broker also drops exported aliases after sendmsg. Neither is yet joined to the
failing Chromium EndA. Astra reviewed proposed generic SO/PCB/FIFO receipt
correlation and rejected it: concurrency, repeated same-FD sends, PEEK, stream
batching/partial send, truncation and native consumers prevent exact matching.
Only explicit private transfer generations with authenticated incarnation,
bounded ownership, exact import receipt and terminal cancellation can support
safe lease retirement. Existing dedicated Binder TAKE stream could carry such
a receipt if actual endpoint attribution selects it; no guest-stream metadata
injection or APK modification. No production patch chosen on speculation.

Normal actors99953/99967/120/124/162 remain daemon47588-owned and runnable.
Fresh read-only all6-PID kernel snapshot succeeded136 lines; no state290 sockets
remain and prior diagnostic Root FD numbers absent. This is NOT proof the current
Root works: it forbids reusing an older incarnation's mapping or inferring every
blank page has the same live EOF. Luna is implementing bounded test-only host
sendmsg/recvmsg identity observers and mocks; next fresh startup trace must join
carrier/payload before selecting the provider owner. No native/runtime/DEX build,
APK/profile reset, runtime behavior change, fallback or commit/push. Rendering
acceptance remains FAIL; goal ACTIVE and normal Chrome left running.

Passive normalbrowser99967 LLDB batch35102 terminal0, no breakpoints/target
calls or writes: actual host __sendmsg entry1841fd978, successret1841fd9a0(+40),
errorretab1841fd99c(+36); __recvmsg entry1841fc7c4, successret1841fc7ec(+40),
errorretab1841fc7e8(+36). Native +8 is before cerror and must not be decoded as
signed return. Detached verified; actors runnable. These dyld load addresses
are current-host observations, revalidate before another host/run. Luna helper
work remains running; requested fail-closed completion decoding, no recv-entry
garbage attribution, signed ssize_t, bounded snapshots and overflow accounting.

### 2026-09-18 — Chrome webpage priority reaffirmed; non-stopping observer validated

User explicitly reaffirmed example.com rendering as priority1. Current goal now
states visible Example Domain heading/body plus physical input/reload and fresh
Retina capture, not URL/process/fixture success. Rendering remains FAIL.

Read-only LLDB helper host mocks5 PASS. Actual browser54/daemon110 native syscall
events carried non-socket image descriptors; Root hardware breakpoint count0.
This trace did not attribute the failing Root endpoint. Python callbacks require
`command script import`, verified by breakpoint commands; plain Python import
did not install them. Native completed return is +40(success)/+36(error), not +8.
Attaching stalled browser progress; detach restored progress. Diagnostic children
inherited STOP and were continued only after browser GPU-channel timeout, so that
failure is timing-confounded, not ordinary Chrome acceptance. Debuggers detached
and quit; diagnostic actors retired, normal signed-host launch restored.

Private tools/diagnostics/scm_transfer_trace.cc interposer forwards real syscalls
once, preserves incoming/returned errno, parses only successful calls, caps logs
with explicit overflow, and emits 64-bit kernel identities as JSON strings.
Warnings-as-errors private compile PASS. Fixture nine configurations (single,
300 sequential, two concurrent threads ×150; absent/disabled/enabled injection)
all terminal0 PASS, including errno, positive FD exchange, no duplicate payload,
EAGAIN/EBADF/EFAULT and capped overflow. Local malformed-buffer fixture checks
are not an actual interposer-decoder malformed-input gate.

Separate-process observer perturbation check,10 per configuration: no injection
stream1 PASS/9 verified EOF, datagram1/9, release-after-import10/0; injected
stream2/8, datagram3/7, release-after-import10/0. OTHER0 throughout. Small sample
does not prove equal failure rates or absence of timing perturbation; it verifies
the EOF remains observable and positive ownership control passes with injection.
Test-only exec wrapper compiled, but actual APK/daemon injection coverage remains
unverified. Do not weaken signing/security or infer coverage from a marker alone.
Next trace must join actual Root payload identity to its transfer owner before
choosing a bounded receipt/ownership provider fix; no speculative generic ACK.

Normal launcher9730 remains indefinite; daemon47588, system28011/browser28030/
renderer28080/GPU28084 verified runnable. Physical URL13 HID events PASS. Fresh
/tmp/darwin-art-example-scm-observer-normal.png original inspected: example.com
controls present, WHITE blank body, no Example Domain. No debugger/STOP/injection
on normal launch. No production runtime/native/DEX build or behavior patch, APK/
profile modification, fallback, commit or push. Goal ACTIVE, not completed.

### 2026-09-18 — Actual non-stopping SCM handoff loses receive capability

Previous turn classified progress: nine private observer configurations passed
and perturbation controls determined the next actual-path trace. Current turn
read AGENTS/current goal/latest checkpoint before work. First diagnostic run
proved actual host and daemon interception but image traffic exhausted the shared
256-record budget. Test-only observer now has independent256 socket-rights and
256 other-event budgets, explicit category overflow, one FD metadata snapshot
per event. Private warnings-as-errors compile and concurrent fixture terminal0
PASS:256 socket records/one socket overflow/zero decode errors. No production
graph or runtime changes. Second real run logs preserved in
/tmp/darwin-art-scm-v2-live.log and launch log in /tmp/darwin-art-scm-v2-launch.log.
Actual loaded markers plus intercepted calls proved daemon42937/browser43384/
renderer43431/GPU43435/spare43493 coverage; system43381 has its separate daemon
log. Socket events31; socket overflow0/decode errors0/CTRUNC0. Other budget
overflow exists, so do not infer absence of other transfers. No debugger/STOP.

Decisive event: browser43384 tid41448578 sends EndA SO3528892784393000589,
PCB13694057591159359876, peerSO10296103978177551690/PCB12503447006483645192,
state258 at wall1789734204215422000. Same browser imports exact EndA SO+PCB
state290 at4226394000 (~10.97ms later), via carrier
SO10340525507116587207/PCB8143881480517515607 →
SO9071745832913397618/PCB10470282743045337757. Browser also sends reciprocal
EndB to GPU43435, which imports it258 at4215784000. Read-only vmmap browser
runtime image base105054000 and offline atos conclusively resolve sender
1060408a8 to SendMessageOnHostSocket+1264, receiver1060410ec to
darwin_art_bionic_socket_broker_recvmsg+624. Source closes exported aliases after
native sendmsg. Two additional browser socket imports already290 observed.
Later kernel snapshot /tmp/darwin-art-scm-v2-kernel.tsv reused same SO with
DIFFERENT PCB: unrelated lifetime, not an identity join. Exact Root channel
correlation remains pending; this is actual platform handoff failure evidence,
not proof it is the webpage Root. Do not claim kernel GC causal attribution.

Astra reviewed exact trace: acquiring receiver alias is too late; protecting
ownership must precede enqueue/last sender release. Broker descriptor-generation
alone is not a transfer identity. A private receipt-bearing transport with exact
transfer/incarnation and consumption semantics, bounded resources, discard/
PEEK/partial-send/close/crash handling is needed if direct managed SCM is selected.
No guest ACK bytes, SO/FIFO inferred receipt, or permanent descriptor retention.
Next gate joins Root portal→Channel→guest generation→canonical host FD→this exact
SO+PCB event before production owner/design choice. Review completed, no edits.

Private truncation fixture initially failed because Darwin with ZERO control
space silently discards rights without CTRUNC. Changed to eight-byte nonzero
space: actual syscall fixture terminal0 PASS; observer records CTRUNC=true,
rights=[], truncated_cmsghdr, result1 without fabricating imports. This is native
diagnostic behavior evidence, not Android truncation compatibility acceptance.
Both LLDB host mock suites5 tests each PASS; scoped diff check PASS.

Exact diagnostic actors/daemons gracefully retired. Normal launcher21337 live,
daemon47409/system47693/browser47696/renderer47766/GPU47770 runnable, indefinite.
Filtered environment check has no DYLD injection, SCM trace config or STOP.
Physical URL13 HID events PASS; fresh original Retina
/tmp/darwin-art-example-scm-v2-normal.png inspected: WHITE blank body, no Example
Domain. Rendering FAIL, goal ACTIVE. No production native/runtime/DEX build,
APK/profile reset, fallback, behavior workaround, commit or push this checkpoint.

### 2026-09-18 — Root channel joined to direct SCM handoff failure

Previous turn classified progress: actual non-stopping SCM transfer lost receive
capability and native callers resolved. Read AGENTS/current/latest before work.
Fresh combined trace /tmp/darwin-art-scm-root-join-live.log, launch log same
root-join prefix, native kernel snapshot /tmp/darwin-art-scm-root-join-kernel.tsv.
Browser52785 bias300000000: hardware Direct-bind9ee3f40 F=a034e8700; exact
HostManager root nodea03913640 ID(0,1), clienta00574220, LayerTreea02e12760
owns SAMEF at+20; F+c0==LayerTree+8. Exact CFSClient Connector ctor
C=a003cd060, H=a0005da80, typebias3014e7, ctor-return handleunchanged/error0/
portalflags0. Removed execution breakpoints, watched H+19 only.

FIRST H+19 watch flips0→1 atbias34d77b0, x19==H, Cerror still0. Saved FP chain
34b9ce0→34d7c04→34b9ce0→34d7c04→34dbf80→34c78dc→34b8f7c→33b5008→7ab8e30.
FP16d635f00+18/+28 both yield Channel a04010000/vptrbias79c398, token1073743910.
Live native broker maps EXACT token→canonicalhost133 refs1 active0. Kernel
host133 SO1580482387237288277/PCB2588198400303917816 state8496/peer0.
Native trace sends THIS EndA258 browser at1789734639461208000, receives8496 at
9518811000. Reciprocal EndB SO12439923733529486950/PCB4374447610502542926 sent
browser258 at9461279000, imported GPU53389 **290** at9461492000. Root failure
now joins actual direct-managed SCM handoff; not Binder TAKE attribution.
GPU receives damaged EndB before provider import, browser later sees peer gone.
Do not claim exact kernel GC mechanism proved by these states alone.

Read-only vmmap browser runtime base105864000 plus offline atos resolves send
1068508a8→SendMessageOnHostSocket+1264, recv1068510ec→broker_recvmsg+624.
Socket records39, socket overflow0/decode errors0/CTRUNC0; other overflow exists.
No GPU timeout/FATAL observed. Child CONT monitor initially resumed actors, then
sed hit non-UTF8 illegal-byte errors; monitor terminal0 is NOT an all-iteration
PASS. Use LC_ALL=C for future diagnostic log parsing. Actual renderer/GPU were
runnable/loaded; no missing-child inference from debugger-altered PPID.
Watch removed, debugger4798 detached/quit; no debugger remained.

Astra proposed bounded feasibility test: per-rights-batch pipe write-end guardian
travels in private provider ancillary metadata; narrow Rust authority holds
payload aliases and guardianREAD before enqueue. Native last-writer EOF retires
lease after receiver ownership, queue discard or channel close—not timeout/FIFO/
socket identity guessing. Requires exact envelope recognition and control/PEEK/
raw-consumer semantics; NOT production-approved yet. Luna implementation task
mojo_readonly_debug_filter is active on independent private kernel fixture.
Daemon hard-crash cannot promise userspace cancel-before-release ordering:
post-import same-incarnation validation must prevent publishing damaged FDs and
terminally reject managed carrier on authority loss. ADR/design remains pending
feasibility evidence. No production implementation or new runtime fallback.

Exact diagnostic actors/daemon gracefully retired; normal launcher34762 live,
daemon61038/system61329/browser61332/renderer61380/GPU61382 runnable, indefinite.
Filtered environment check: no SCM/DYLD injection or STOP. Physical/capture check
pending below. Full goal ACTIVE; example.com remains first priority. No runtime/
native/DEX/Cargo build, APK/profile modification, commit or push this checkpoint.

Root-join continuation verification: physical13 HID events terminal0 PASS;
fresh original Retina /tmp/darwin-art-example-root-join-normal.png inspected:
WHITE blank body, no Example Domain. Normal remains live without injection/STOP.

Added proposed ADR0005-managed-scm-transfer-lifetime.md following Astra review,
explicitly NOT production-approved. Luna added diagnostic kernel guardian
fixture only, did not compile/run. Main fully read/corrected it: native recvmsg
argument order, actual FD-list fetch (not sizing estimate), positive exchange
using imported FD AFTER protecting alias retirement, actual receiver `_exit`
FD teardown, strict CTRUNC observation. Private warnings-as-errors compile PASS.
Initial all-cases run terminal1: failed cases polluted later FD baselines; do
not reuse that combined count. CLI now isolates each case in a fresh process.

Native isolated results: protected delayed consume PASS with balanced3→3 FDs
and positive imported-endpoint exchange after protection retired; carrier close,
sender death and receiver death PASS3→3. Native PEEK returned [0,0], NOT owned
FDs; initial fixture wrongly closed stdin. Corrected interpretation leaves those
integers untouched: queued guardian remains until real consume, then EOF,
baseline3→3 PASS. This is native queue retention, NOT Android peek compatibility.
Native zero-control discard, nonzero undersized CTRUNC and plain `read` each
FAIL: guardian EOF timeout, baseline3→5 (two undisclosed installed FDs). Astra
published-XNU review agrees native PEEK zeroes rights and ordinary receive
externalizes before copyout; main native tests authoritative, exact-running XNU
source not verified. Full private native receive followed by manual discard
PASS10 separate processes3→3; protected import/post-retirement exchange PASS10
separate processes. Capacity/concurrency/authority-death/adapter integration and
shared-alias virtual peek remain unverified. Guardian-only thin wrapper is NOT
ready for adoption. No new Root attribution gate is needed; preserve this join.
Next choose explicit managed receive ownership/peek contract, route every
consuming path through full-control intake, and only then implement the narrow
Rust lease owner/native ancillary adapter. Production remains unchanged and
example.com FAIL; full migration goal ACTIVE. Both reviews/tasks completed.

### 2026-09-18 — Example.com first: receive contract selected; owner work started

User reaffirms real Chromium example.com rendering as priority1. Current goal,
AGENTS and latest checkpoint reread; preserve the proven Root/direct-SCM join,
do not repeat attribution or defer it behind unrelated Probe cleanup.

Astra approves implementation START of capability-bounded managed SCM v1:
every consuming read/readv/recv/recvfrom/recvmsg uses full native ancillary
intake plus explicit Android discard/publication; kernel retains shared-alias
ordering/concurrent receive ownership. No receive cache. Managed ancillary peek
may return genuine EOPNOTSUPP before consumption ONLY if actual unchanged APK
operation audit establishes it unused. Required observed peek blocks adoption.
ADR0005 now explicitly records that scope; production adoption remains gated.

Bounded Luna tasks started: new Rust ScmTransferLeaseOwner crate and independent
native ancillary intake ownership/actual-object tests. Workspace member added.
Main initial owner review identifies identity equality, retired-ticket replay,
enqueue/import race, terminal cleanup and CLOEXEC requirements; corrections are
requested before acceptance. No component/production integration PASS yet.
Authenticated acknowledged deposit, managed-carrier propagation, complete
consumer routing and real Root/Chrome acceptance remain OPEN.

Private trace now records operation flags and observes successful AND failed
peek requests through recvmsg/recv/recvfrom, preserving native syscall/errno.
Native ancillary peek placeholders are not interpreted as owned FDs. Private
warnings-as-errors builds PASS. Actual private peek fixture PASS: six records,
each operation failure(flags130,result-1) and success(flags2,result1); subsequent
consume verifies queue remains intact. Two-thread300-iteration regression PASS,
256 socket events plus explicit overflow marker, decodeErrors0. These are
diagnostic-object tests, NOT the acceptance APK flags audit or production fix.

Normal indefinite daemon61038/system61329/browser61332/renderer61380/GPU61382
revalidated runnable with daemon parent, no debugger/STOP/restart this turn.
Fresh /tmp/darwin-art-example-priority-current.png original Retina inspected:
address example.com, WHITE blank body, no Example Domain. Rendering FAIL and
full goal ACTIVE. No APK/profile change, GPU fallback, commit or push.

Continuation review: early cargo check of the unfinished new Rust crate fails101
(missing AsFd, BorrowedFd references, duplicate errno pattern); implementation
agent owns corrections, no Rust PASS claimed. Both new ownership modules are
drafted but unadopted. Main native review finds CTRUNC/late-malformed-control
paths must acquire visible externalized FDs BEFORE failure cleanup, preserve
errno, and resolve multi-record capacity/flag bounds. Hidden native truncation
FDs cannot be recovered by claiming visible cleanup. Rust review additionally
requires sealed-incarnation admission, exact identity matching and non-reused
tickets with safe enqueue/import transitions. Agent corrections/tests remain
pending. These are concrete implementation review gates, not new Root tracing.

### 2026-09-18 — Real export owner extracted; native receive objects verified

Previous goal turn PROGRESS: selected v1 receive contract, drafted production
resource owners and established flags-observer behavior. Current turn rereads
AGENTS/current goal/latest checkpoint and revalidates live normal actors.
Example.com visible-body rendering remains priority1 and FAIL; no Root trace
repeat, APK/profile reset, fallback, commit or push.

Main extracted Android ancillary layout decoding/native FD export ownership
from SendMessageOnHostSocket into android_scm_exports.h/.cc. Production caller
now uses ExportedRights with an explicit guest-FD export port and RAII cleanup;
the old inline parser/manual rollback and close loop are removed. Unaligned
Android headers/integers use memcpy, reserve occurs before acquiring a new FD,
failure rolls back prior exports while preserving errno. Production recipe,
archive object list, conservative input/header manifest and existing adapter/
sync-fence test link recipes updated. This is source-level caller integration,
NOT a rebuilt product or the pending guardian protocol adoption.

Actual exported-rights production objects ASan/UBSan PASS: unaligned control,
live dup FDs, exactly-once cleanup, callback failure/errno and malformed-later-
record rollback. Adapter plus new export module warnings-as-errors syntax PASS.
Provider input-manifest tests PASS7 including real recipe coverage; shell syntax
and targeted diff whitespace checks PASS. Both-flavor product/link/warm gates
must rerun after final integration, not inferred from these component tests.

Luna native ancillary intake module completed; main fully read its object and
tests, reran ASan/UBSan PASS. Main added actual15 queued18-rights groups with
WAITALL and write-direction shutdown: all15 bytes/270 rights consume with
balanced FDs, PASS. Tests also cover254-right native intake, take/discard,
pre-consumption peek/unknown-flag rejection, EAGAIN, and unaligned malformed
decoder. Native bound/private single-group protocol and every managed consumer
remain adoption gates. Visible returned rights are streaming-owned before
CTRUNC/error cleanup; hidden truncated native FDs are not claimed recoverable.
Module remains unadopted until managed envelope/identity coverage is complete.

Rust lease-owner corrections remain under implementation. Intermediate Rust
check fails101 on unfinished delimiter revision, not a crate PASS. Sealed
admission/ticket replay/identity/race gates are reviewed; ready final tests are
pending. Astra managed-carrier review running: initial loss ports are descriptor
export/import, Binder client capture, TransferImage files/take_files and received
FD installation. Proposed typed FD+opaque capability attribute preserves Parcel
bytes and guest descriptor count; exact authenticated bundle/ordinal admission
is mandatory. Also review daemon authority epoch versus sender process epoch;
do not conflate those before wiring. One earlier reviewer resume hit thread
limit; resumption after native task completion succeeded, not an external block.

Normal daemon61038/system61329/browser61332/renderer61380/GPU61382 remain live,
runnable without debugger, STOP or injected trace; Chrome not terminated this
turn. Real body acceptance and full Probe-removal goal remain OPEN/ACTIVE.

Continuation: main reran finalized first Rust core tests PASS5/doc-tests0,
then Astra found its authority/sender identity model unsuitable for production.
That PASS is the pre-rewrite component revision, NOT final-source acceptance.
Reviewed correction: immutable daemon AuthorityEpoch, authenticated sender and
receiver ProcessEpochs, carrier/endpoint sides independent of holder PID,
owner-minted tickets and sender-based reconnect-stable quotas. Import requires
opposite side of the same carrier and fresh authority admission; guardian stays
receiver-local, admission does not retire aliases, sender death must preserve
already queued delivery. Luna core rewrite active; no production adoption.

Astra FD bundle review completed: preserve opaque per-FD grants through
DescriptorApi → capture_remote_fds → TransferImage → dispatcher install,
bind exact offset/ordinal/source-session/transfer token before export. Pair
registration precedes publication, native FD and metadata export share one
operation lease, managed bare-FD escapes cannot silently become unmanaged.
Initial Binder delivery also needs retained-acquisition lifetime ownership;
unchanged routing and TAKE response alone are insufficient. ADR0005 updated.
Luna bounded TransferImage typed-bundle/versioned-manifest implementation active;
legacy APIs must reject nonempty attributes instead of silently dropping them.

Main full adapter audit11203 terminal0: real Android-ELF HTTP/pipe-poll, broker
owners, close/dup and100 teardown races PASS under ASan/UBSan and TSan; the new
export object is linked into those real adapter runners. Their13-import network
fixture is not a Chrome renderer or sendmsg acceptance; separate new export
object tests cover its codec/rollback. No both-flavor product rebuild yet.
Normal daemon61038 and four APK actors remain runnable and unmodified.

### 2026-09-18 — example.com first, typed Binder export integration

User priority reaffirmed: real example.com body rendering precedes unrelated
migration. Fresh normal window capture
`/tmp/darwin-art-example-priority-20260918.png` inspected at original Retina
resolution: address/toolbars visible, webpage body WHITE, no Example Domain.
Rendering acceptance remains FAIL; daemon61038 and actors61329/61332/61380/61382
remain live without termination or diagnostic injection this turn.

Production Binder client now allocates its transfer token before exporting FDs
and captures typed descriptor bundles with exact source connection, transfer,
ordinal and object offset. New narrow descriptor_transport provider port
preserves bounded opaque attributes and acquires owned exported FDs before
attribute validation so errors close them. Main Binder-process component test
terminal0 PASS1. Host still has bundle=None; native managed provider/grants,
daemon acknowledged deposit, bootstrap acquisition lifetime and direct SCM
consumer closure are NOT adopted or rebuilt into the live runtime.

Luna TransferImage v4 opaque per-FD manifest implementation completed;
main workspace transfer_image test session61406 still running, not a PASS.
Luna finalized distinct authority/process/carrier-side Rust lease model;
main rerun terminal0 PASS5 plus doc-tests0. Admission retains sender aliases
until guardian EOF, including queued delivery after sender death. This is
component evidence, not Chrome acceptance. Typed dispatcher import wiring and
provider-output/rollback tests delegated to Luna in descriptor_transport.rs
and dispatcher.rs; shared native/DEX/product builds remain main-only.

Continuation: previous turn PROGRESS (bound production export wiring and fresh
blank-body evidence). Main workspace TransferImage61406 terminal0 PASS7.
Common Darwin fd_passing sender/receiver now use native single-group bound254,
not guest manifest257. Actual254-right intake/CLOEXEC and255 pre-enqueue rejection
plus existing ordered/one-FD tests PASS4, command87765 terminal0 including bin0.
Public legacy try_clone_files now rejects nonempty provider attributes rather
than dropping them; main revised image5208 terminal0 PASS7. Targeted rustfmt
and diff whitespace checks PASS. No live-runtime rebuild/adoption inferred.
Luna typed dispatcher source reviewed: authenticated transaction sender/reply
source binds import; requested preservation of FD-free fastpath and precise
callback failure ownership. Its final tests pending. Astra delivery-receipt
review resume hit agent thread limit; safe component work continued, not an
external blocker. Normal daemon61038/four actors remain live. example.com body
and all broad Probe-removal acceptance gates remain OPEN/ACTIVE.

Continuation: previous turn PROGRESS (native FD bound/metadata-clone guard and
terminal tests). Luna V2 dispatcher completed; main current-source full
BinderProcess terminal0 PASS19. Main hardened callback tests against parallel
shared-state races and raw-FD-number reuse: serialized fixture observations,
actual socket-peer EOF proves closure. Revised43374 terminal0 PASS5.

Astra bootstrap review completed: separate common HostFdDeliveryOwner outside
Binder policy; private offered/acquired/admitted daemon-epoch ticket/count,
guardian-first native group, central EOF-only retirement beyond handler exit,
global/per-destination FD AND delivery quotas. ADR0005 updated; owner/wire/caller
implementation still pending. Followup Luna owner task could not resume under
thread limit; do not mistake failed dispatch for an implementing agent.

Actual production peer_process identity/registration now delegate to new narrow
peer_credentials Darwin provider. LOCAL_PEERTOKEN PID/version is matched against
kernel unique-process snapshots bracketing live birth lookup; removed old
numeric LOCAL_PEERPID-only port. SDK audit ABI and Apple XNU private-info layout
checked; fixed32/56-byte layouts validated, no weaker identity fallback.
Main47515 terminal0 PASS4 includes actual socket-token/live-kernel match, pure
reused-version/changed-unique rejection, real peer/direct-child registration
regression and queued-listener wakeup. This is source-level caller integration,
not a daemon/product rebuild or actual PID-reuse experiment. Whitespace/fmt
checks PASS. Normal61038 daemon/four actors remain live and unchanged.
Luna actual SCM queue + finalized Rust core joined-fixture source in
tests/scm_rights_queue.rs is pending final review/tests; no PASS claimed.
example.com body and full goal acceptance remain OPEN/ACTIVE.

### 2026-09-18 — example.com priority: shared inheritance boundary

User explicitly reaffirmed real Chrome example.com body as priority1. No body
success claimed; normal daemon61038/browser61332 and child actors remain live.
No APK/profile modification, debugger injection or actor restart this turn.

Main shared Host/Profile all-bin cargo check4515 terminal0 PASS. Added one Rust
inheritance guard shared by guardian creation, Profile FD recvmsg-to-CLOEXEC and
owned Profile/Host child spawn ports; waits occur outside it and pre_exec never
acquires it. C++ intake adoption/same-instance closure remain pending. Current
Profile fd_passing33850 terminal0 PASS4; corrected actual SCM queue fixture88528
terminal0 PASS2 (aligned254-right capacity, queued abandon and positive imported
payload survival). Current SCM full92619: unit5 and inheritance-window fixture1
PASS; queue/doc stage still running at this checkpoint. Fixture proves protected
pipe configuration excludes concurrent owned spawn and real EOF while exec child
is alive; it is not an actual production pipe-window injection test.

Luna HostFdDeliveryOwner moved to correct Profile crate and split into six narrow
modules, reviewed/registered by main; final owner uses hard bounded quotas,
fallible reservations and direct guardian EOF polling. Agent standalone4 PASS;
registered workspace tests and actual protocol callers are still pending.

Astra confirms direct-SCM fix requires capability-bearing export/operation ports,
authenticated daemon prepare/admit, actual SendMessageOnHostSocket lease BEFORE
native enqueue, and full managed native intake BEFORE guest import/discard across
recvmsg/plain-read aliases. Binder TAKE-only guardian rollout is not this fix.
Next: coherent production caller integration, serialized product rebuild, then
physical reload and fresh Retina Example Domain heading/body evidence. Do not
resume unrelated Probe refactoring or further Root-attribution diagnostics first.

### 2026-09-18 — actual Binder bootstrap delivery callers joined

Previous turn PROGRESS (shared inheritance guard and native fixtures). SCM92619
terminal0 PASS: unit5, actual queue2, inheritance-window1, docs0. Registered
owner42025 terminal0 PASS4. Main added narrow host_fd_delivery/transport.rs:
server TAKE hands routed owned descriptors into centralized retained ownership
and arms BEFORE offer/enqueue; client strips guardian, validates/imports image,
exchanges actual private acquired/admitted frames against the fresh daemon epoch
and exact socket-bound destination, then closes guardian. Removed old
TransferDelivery.send; Binder owns authorization/one-shot token, not host lease.
Central EOF scan keeps pending deliveries beyond handler exit and blocks idle
exit. This is necessary bootstrap SOURCE integration, not direct SCM adoption.

Astra review found shutdown/reservation race, EINTR authority abort and first-
destination quota bypass. Main fixed handler intake/shutdown under same gate,
BUSY for other handlers/pending FD ownership, EINTR NotReady (never EOF), and
checked first-delivery FD accounting. Actual shutdown-handler and quota tests
added; full listener/reservation race and interrupted-native-poll injections
remain unproven. Unexpected scan errors still fail authority closed; do not
claim guaranteed delivery across daemon loss.

Main Host/Profile all-bin check15455 terminal0 PASS. Final Profile lib74166
terminal0: 141 PASS, 5 ignored subprocess fixtures, 0 FAIL. Covers actual private
handshake, socket payload byte exchange AFTER originals/central aliases close,
guardian excluded from guest manifest, malformed image/count, and control EOF
while received native group still holds guardian (handler exit is not release).
Compilation/framing errors in initial draft tests corrected before this result.
Whitespace checks PASS; completed agents interrupted/retired.

Normal61038 daemon and actors61329/61332/61380/61382 freshly verified live; no
restart/injection/APK/profile modification. They still use OLD product objects.
Next FIRST: capability-bearing native descriptor operations/exports/imports,
authenticated SCM prepare/admit service, actual direct send caller lease before
enqueue, complete managed receive/discard aliases with shared CLOEXEC boundary.
Existing ancillary primitive has no CLOEXEC adoption yet. Then coherent
serialized product rebuild/restart and unchanged Chrome physical Example Domain
body/capture gate. Body acceptance and full Probe-removal goal remain OPEN.

### 2026-09-18 — example.com remains priority1; typed SCM batch preparation

Read Current goal/latest checkpoint and AGENTS before work. User reaffirms
real unchanged Chrome example.com body first; unrelated migration stays behind
this gate. Fresh normal-window capture51653 terminal0, original Retina image
`/tmp/darwin-art-example-priority-current.png`, still shows WHITE webpage body.
No heading/body acceptance. Daemon61038/browser61332/renderer61380/GPU61382 live;
no restart, debugger injection, APK/profile changes or commit/push this turn.
These actors still run old product objects, not the new source contracts.

Earlier unrecorded core changes: guardian native syscalls extracted into their
own provider module; fallible reservations precede native ownership acquisition;
EINTR guardian poll/read is NotReady, never EOF. Core12963 terminal0 PASS5 unit,
2 queue, 1 inheritance; foreign synchronous native-operation callback uses the
same Rust inheritance mutex and preserves result/errno (98146 terminal0 PASS2).
Actual C++ installer/consumer adoption of that callback remains pending.

Native intake now owns all visible rights before setting CLOEXEC, retries
F_GETFD/F_SETFD EINTR and closes owned rights on configuration failure. Final
standalone48085 terminal0 ASan/UBSan/warnings-errors PASS, including two queued
254-right batches (508 aggregate rights, native MSG_WAITALL, real FD baseline).
Initial55077 fixture assertion failed because baseline included the socketpair
then compared after closing it; main moved baseline before pair creation and
reran. This is NOT proof of one native recvmsg delivering 508 rights or hidden
MSG_CTRUNC cleanup, nor adoption into actual managed read/recv aliases.

Registered narrow capability registry: bounded fresh epochs, authenticated
opaque holder lookup, exact Binder/SCM bindings, same-side payload claims,
Pending -> Committed(None) -> Committed(Some(receiver)) -> Claimed; committed
unrouted exports survive source death. Main added atomic SCM capability batch:
validate every source holder/ordinal/quota, reserve and mint every ID before
insertion, then commit the whole batch. Repeated-description rights retain
distinct ordinals. Tombstone saturation/allocation failure seals minting while
death cleanup still runs without allocations. Main final95056 unit stage
PASS15; full inheritance/queue/doc stages pending at this initial checkpoint.
This is capability-component integration, NOT an authenticated daemon service,
actual native send adoption or Example Domain rendering success.

Astra reviewed the remaining source-close race: native typed FD+capability
snapshots must retain one exact broker Description operation pin through the
real prepare_scm acknowledgement; acquire-pin then separately relookup guestFD
is invalid under dup2. Carrier pin survives native send. No broker/I/O/spawn
mutex spans daemon RPC. Luna implementing bounded broker snapshot ABI/test
source; not built/adopted at this checkpoint. Next: complete this port, compose
atomic capabilities with guardian aliases and private authenticated daemon
prepare/admit, connect native V2 exports/imports and all managed intake aliases,
then serialized coherent product rebuild and physical Chrome reload/capture.

Final core95056 terminal0: 15 unit + 2 inheritance + 2 native queue PASS,
docs0. Earlier83328 terminal0 passed pre-saturation-test revision (14 unit);
only95056 is the final current-source gate. Main commands were Cargo-lock
serialized; no native/DEX product build occurred. Scoped git diff --check PASS.

Main Host/Profile all-bin59281 terminal0 PASS (existing unused/deprecated
warnings). Snapshot ABI source now ready: retain_description/release_description
pins exact description, owner validation and callback outside broker lock;
genuine close/dup2/quiescence tests and pinned header hash updated. Main central
broker audit started after all-bin check; no product consumer adoption yet.

Final snapshot audit11554 terminal0 PASS: native C ABI, pinned Android NDK ABI
fixture, genuine state-machine tests under ASan/UBSan/TSan and no-host-FD broker
closure. Initial audit rejected stale header hash; main formatted scoped broker
sources and updated the lock from the actual formatted header before rerunning.
Completed implementation agents retired. All main build/test handles terminal;
normal Chrome left running indefinitely, still OLD product/WHITE body. Next
FIRST is actual private SCM service + native typed/scoped caller adoption,
not more attribution or unrelated Probe cleanup. Rendering acceptance OPEN.

### 2026-09-18 — priority1 native FD boundary and atomic admission prerequisites

Read AGENTS/Current goal/latest checkpoint; example.com body remains first.
Fresh CLI capture1131 terminal0 `/tmp/darwin-art-example-priority-20260918.png`
viewed at original Retina resolution: URL present, WHITE body, acceptance FAIL.
Exact daemon61038/system61329/browser61332/renderer61380/GPU61382 remain live
on OLD product objects; no restart, injection, APK/profile change or commit/push.

Actual socket/socketpair/pipe and sync-fence producers now use a narrow native
immutable installer callback into the SAME Rust inheritance boundary used by
owned spawn. Host installs it before native-provider acquisition; missing
installer fails closed. SDK archive/link exports and checks updated in source.
Ancillary primitive now performs nonblocking recvmsg/ownership/CLOEXEC under
this boundary; waiting belongs outside it. Managed consuming aliases and the
authenticated SCM service are NOT adopted yet. No full product rebuild yet.

Main standalone FD factory test20db4f PASS ASan/UBSan/Werror; ancillary24869
PASS including adjacent 254-right groups (not one 508-right intake).
Adapter91385 PASS ASan/UBSan/TSan; fence97951 PASS two-input reverse-close.
Those isolated native fixtures install an explicit TEST-ONLY no-spawn callback,
not a production fallback or proof of Rust-lock closure. Fence fixture needed
FS/ioctl includes and an unused fail-closed Binder binding seam after initial
compile/link failures (last failed8441); only97951 is final fixture PASS.

Core registered read-only import preflight, whole-batch claim reservation,
allocation-free settlement and direct guardian polling (no scan-time FD dup).
Claim checks cover invalid final ordinal, stale reservation, mixed discard,
abort and full holder quota. Engine/Host check95173 terminal0 PASS. Initial
combined test3392 failed compiling a fixture reference-pointer cast; main fixed
it and joined native workers before unwrapping timed receives. Rerun79088 has
Engine16 and SCM20 unit PASS; final native/inheritance stages still pending at
this checkpoint. These are prerequisites, not Example Domain acceptance.

Astra service review complete: one authenticated SCM owner mutex must compose
aliases, capabilities and exact bounded ledger; all allocations/randomness
precede core admission; actual EOF alone retires aliases. Native payload pins
must survive genuine prepare ACK, carrier pin through send. Luna implementing
bounded exact-description retained native exports at the actual direct send
caller; no builds by agent. Next FIRST: complete scoped exports, then actual
authenticated prepare/admit/settle and all managed intake aliases, coherent
serialized product build and physical Chrome reload/body gate. Full goal OPEN.

Final79088 terminal0: Engine16 + SCM20 unit, inheritance2, actual C++/Rust
boundary1 and native queued-rights2 PASS. The actual boundary fixture pauses
AFTER native CLOEXEC and proves shared serialization/real EOF with child alive;
it does not inject the unsafe pre-CLOEXEC window. Scoped diff check PASS.
Fresh capture1131 is still WHITE body, so these tests do not close acceptance.

### 2026-09-19 — priority1 authenticated SCM ownership and retained native sends

Read AGENTS, Current goal/latest checkpoint and ADR0005. Fresh CLI capture93874
terminal0 `/tmp/darwin-art-example-priority-20260919.png`, viewed at original
Retina resolution: example.com URL present, body WHITE, rendering FAIL. Exact
daemon61038/system61329/browser61332/renderer61380/GPU61382 remain live on OLD
product objects; no restart, injection, APK/profile reset or commit/push.

Source now has private authenticated `scm_service` ingress and a separately
owned mutex composing native aliases, capability registry and exact transfer
ledger. Prepare arms before response; admission validates the entire manifest
and reserves before claim; only native guardian EOF retires aliases. Pair
installation echo has rollback. Native audit-token/birth identity, not wire PID,
authenticates peers. Exact-birth participant monitoring handles socket-only
process death; live holder/delegation refs block daemon idle exit. Native FD
intake uses a real monotonic deadline, not ineffective SO_RCVTIMEO on poll.
These are service/component integration, NOT actual SDK carrier adoption.

Actual native sendmsg/sendmmsg callers retain exact Description snapshots for
payload and carrier, including full sendmmsg batches. Broker failure cleanup
uses the narrow provider callback outside its lock; no host FD syscall was
added to the central broker. Audit11468 PASS central ABI/sanitizers/no-host-FD
closure; adapter65363 PASS ASan/UBSan/TSan after scoped carrier integration.
Retained export can now move only its owned transport FD while keeping its
local Description pin alive; final fixture56827 PASS ASan/UBSan/Werror proves
that split ownership and errno-preserving cleanup. This helper is not yet the
installed Binder export port.

Profile initial81209 suite failed one immediate EOF assertion (156 PASS,
1 FAIL, 5 ignored); isolated and unchanged full rerun passed. Cause of that
transient was NOT proven. Astra approved bounded fixture-only repeated actual
EOF scans: deadline only fails the test, never retires production aliases.
After this fix, final20919 and repeated quiet suite both PASS157/ignored5.
Actual accepted Unix connection authentication, lost pair-response rollback,
live same-birth reconnect and real child-birth death cleanup pass. Host/Profile
all-bin22999 terminal0 PASS (existing warnings); no full native/DEX product build.

Astra reviewed the next actual Binder boundary: V2 metadata alone cannot keep
the source Description pinned through genuine deposit ACK, and per-item import
cannot claim the entire receiver manifest atomically. Next FIRST: local retained
export RAII through both real deposit callers, whole-batch receiver claim and
rollback, then authenticated SDK client/pair attributes and full managed intake
aliases before enabling private framing. Bounded Luna retained-port source work
is in progress; main owns serialized integration/acceptance. No rendering
success or full migration completion claimed; full goal remains OPEN.

### 2026-09-19 — actual Binder source-retention caller integration

Previous goal turn classified PROGRESS (native code, real tests and fresh WHITE
capture). Re-read AGENTS/Current goal/latest checkpoint before work. Retained
Binder export is now a separate provider port with immutable-table release
callback; malformed successful output owns/cleans any FD or lease before
metadata validation. V2 layout unchanged. Both actual outbound transaction and
reply callers hold local, nonserialized guards through deposit. Main moved
release to genuine deposit ACK, before separate route/reply policy responses.
Provider install mutex is copied/dropped before export/RPC.

Main added tests around the actual submit caller with paused controlled deposit:
lease stays live while paused, ACK releases before route, deposit error releases
once. This is caller-contract evidence, not a real daemon ACK or native pin
integration proof. Final56838 terminal0 Binder25 PASS. Common raw FFI structures
now belong to EngineSys (not pointer-free ABI crate); shared BinderProcess and
Engine types avoid function-pointer transmutation. Engine requires native export
and release symbols; Host source installs this port; runtime/graphics audit
exports rooted. Combined59627 terminal0 Binder25 + Engine16 + EngineSys14 PASS,
including retained output arm64 layout272/leaseoffset264.

Main split the changed code: descriptor transport139 lines, separate local
retention owner102, test-only fixture376. Combined60437 rerun after this split
is in progress at checkpoint creation. Native provider implementation is bounded
Luna source work in progress; no product build/restart before it completes and
passes actual native gates. Full batch receiver claim, authenticated SDK carrier
attributes and full managed intake remain prerequisites. Chrome61332 freshly
observed live on OLD product; body acceptance remains FAIL/open. No commit/push,
APK/profile changes or fake frames. Continue these actual FD boundaries FIRST.

Final current-source gates: post-split60437 failed a duplicate test-only c_void
import; main removed it. Rerun71548 terminal0 Binder25 + Engine16 + EngineSys14
PASS. Host/Profile bins99660 terminal0 PASS. Native retained provider source
completed: separate `compat/binder/retained_export_lease` owns an acquired
adapter Process and exact central Description pin, releases pin BEFORE Process,
and transfers only native FD ownership to Rust. Unmanaged attributes stay empty;
no managed carrier/private framing enabled. Main preserves errno through release
and removes ineffective catch-all around a noexcept/nothrow acquisition path.

Main serialized actual Binder JNI/boundary archive build87043 terminal0 PASS,
including new provider object and symbol checks (existing upstream warnings).
Direct executable invocation failed permissions; bash execution succeeded without
changing script mode. This archive gate is NOT a full runtime link/load, actual
adapter Process-lifetime test or Chrome success. Scoped diff check PASS; normal
Chrome61332 still live/OLD product. All main handles terminal, completed source
agents interrupted. A new detailed receiver-publication review dispatch hit agent
thread limit; no new review obtained. Existing Astra whole-batch direction stands.
Next FIRST: actual native retained-provider fault/lifetime gates and receiver
whole-batch preflight/claim/publication rollback, then authenticated SDK attribute
propagation + full consuming aliases; coherent product build and physical Chrome
body acceptance remain OPEN. No goal completion, commit or push claimed.

### 2026-09-19 — native provider close/dup2 and allocation-free buffer ownership

Previous turn PROGRESS; read AGENTS/Current goal/latest checkpoint. Actual
dispatcher now uses inline native-close ownership in InstalledFd rather than
allocating a boxed callback AFTER native acquisition. Legacy closure API stays
for existing users/tests. Provider lifetime/no-unwind contract is explicit at
the unsafe constructor. Main30866 terminal0 BinderDevice153 + BinderProcess25
PASS. This removes one post-claim allocation, not per-item claim or whole-batch
publication; those remain incomplete.

New test-only fixture links actual retained provider, actual adapter Process
acquire/release and central broker. Export original socket, dup2 its guest slot
to a DIFFERENT pair, close all guest aliases: only the original pinned
Description survives; real native positive exchange succeeds. Lease release
drains the real deferred-close worker, then provider teardown succeeds while
the caller-owned exported FD remains valid and reverse exchange succeeds.
Missing Process acquisition fails with no FD/lease outputs. No fake capability
attributes, managed carrier or private framing enabled. Fixture uses existing
explicit TEST-ONLY no-spawn callback, not actual shared Rust inheritance proof.

Initial native48011 failed because broad compat -I shadowed host unistd under
Werror; use quote-only path. Rerun26809 failed an immediate object-count assertion
because broker close callbacks are asynchronous. Main changed fixture to observe
actual completion with a deadline that ONLY fails tests, never frees aliases.
Final73843 terminal0 actual native fixture + existing Android ELF/network/pipe/
quiescence gates PASS ASan/UBSan/TSan. Scoped diff check PASS. All main handles
terminal; exact original five actors freshly observed live/OLD product, no
restart/APK/profile change, commit or push. No new body capture or rendering
success claimed. Next FIRST: native guest-slot/Description batch reservation
before authoritative whole-manifest claim and allocation-free publication,
then real authenticated SDK/Binder attributes and all managed consuming aliases.
Keep unrelated Probe migration behind normal Chrome Example Domain body gate.

### 2026-09-19 — Chrome frame-path priority, not generalized FD expansion

Previous response was NO PROGRESS (intent only). User clarified that FD fixes
are permitted; the question was why they block Chrome, not a ban on FD work.
Do not read the preceding batch-reservation "Next FIRST" as authorization to
defer actual webpage diagnosis behind further generalized FD abstractions.

Exact normal actors61038/61329/61332/61380/61382 observed live, old product.
HID address click and keyboard navigation completed; actual service log records
new unchanged Chrome renderer57154 startup at00:33:00–01. No APK/profile reset,
manual child launch or product build. Fresh original Retina capture
/tmp/darwin-art-example-current.png still has example.com URL and WHITE body.
Transactions55–57 select only transparent system61329 root, no web child frame.
Old renderer log contains CreateCommandBuffer send failures after successful
GPU Vulkan device creation. Samples of old/new renderer and GPU show waits;
they do NOT identify the current Root's first failure or prove GPU deadlock.

Astra bounded review: previous exact Root/Mojo native SCM lifetime loss remains
the first causally established blocker. Retention during send/deposit is not
retention after enqueue until receiver ownership. Authenticated propagation
and consuming intake/publication remain unadopted; prerequisite builds alone
are not a rendering fix. No rendering-only workaround is justified by this
evidence. Preserve ADR0005 safety requirements when adopting the real path.

Bounded live GPU Root factory observation during physical tab clicks had ZERO
hits/events: this did not recreate the Root and cannot reconfirm its failure.
Initially guessed bias was invalid; removed those breakpoints, used vmmap's
actual32ca68000 bias and verified manager entry instructions before corrected
observation. All hardware breakpoints removed, LLDB68633 detached/quit terminal0;
GPU runnable afterward. This is NOT a successful Root trace. All HID/sample
handles terminal. Next: capture actual startup Root/channel ownership and first
disconnect with bounded existing trace, then integrate that handoff and verify
physical reload/visible Example Domain. No additional generic FD fixtures,
fallback or unrelated Probe cleanup before this gate. Goal remains ACTIVE.

### 2026-09-19 — private SCM client codec and owned request transport

Previous turn PROGRESS (actual navigation/new renderer/body evidence). Read
AGENTS, Current goal/latest and ADR0005. Actual adapter retains export objects
through sendmsg, then clears exported aliases; it does not yet adopt the daemon
queue-to-receiver ownership protocol. Keep this concrete gap distinct from
unrelated general FD improvements. Luna bounded codec delegation failed thread
limit; main implemented it without overlapping builds or further retries.

New Rust client_codec (185 lines) shares the real server's strict framing and
validation; encodes all five operations and decodes pair/prepared/admitted
responses. Raw response IDs remain untrusted DTOs, never minted capability
grants. Admission requires exact requested ordinal/order and distinct holders.
Separate client_transport (145 lines) performs genuine protocol/native FD I/O;
each operation consumes its dedicated connection so failed install/receipt
cannot leave a caller-held socket pending. Pair echo occurs AFTER installer
returns rollback-owned state, then requires actual empty confirmation. Prepared
metadata/guardian stay OwnedFd through full validation; no private FD is exposed
as guest payload, no carrier marked managed and no authority state fabricated.

Initial35893 compile failed missing unwrap on fallible registry constructor in
new test; corrected. Codec28570 terminal0 four tests PASS. First service50744
terminal0 21 tests PASS. Extended actual authenticated client/service fixture
now covers native metadata+guardian CLOEXEC/read-only metadata, actual opposite-
side admission, Finished settlement WITHOUT alias retirement, real writer EOF
retirement, both holder releases and no remaining authority refs. Install error
closes actual connection and genuine server rolls back grants. FINAL15962
terminal0 same21 tests PASS; scoped rustfmt/diff checks PASS. Test-only initial
attribute sink is NOT native Description installation or managed carrier proof.

Component integration only: native SDK callbacks, Description-owned attributes,
Binder propagation, full managed-consuming aliases/PEEK operation audit and
transactional native publication remain disconnected. No new product native/
DEX/daemon build or restart, APK/profile modification, capture, commit or push.
Original actors plus actual renderer57154 freshly observed runnable; last real
example.com body remains WHITE FAIL. Next connect this real client to the narrow
native provider and endpoint attribute ownership; preserve ADR adoption gates,
then verify actual Root handoff and visible Example Domain, not just fixtures.
Full Probe-removal goal remains ACTIVE with all exit gates unchanged.

### 2026-09-19 — owned Unix connection boundary and native installation review

Previous turn PROGRESS, real SCM client implementation/tests. Read AGENTS/current/
latest before work. Native HostFdObject has no endpoint lease yet; socketpair
still owns pair creation/publication in adapter. Main extracted native socket
creation/address/connect/readiness ownership from runtime_service_client into
unix_connect: service policy now69 lines, independent provider282 lines with
its existing real backlog/CLOEXEC tests. Socket creation→RAII→CLOEXEC shares
the EXACT Rust native-intake/owned-spawn guard, released before connect/poll.
No second mutex. Existing production runtime-start/readiness/loss exchanges
call this owner, removing their inline unprotected FD creation. New SCM client
connect and its actual authenticated fixture also use it. No Probe dependency.

Main52264 terminal0 all Profile library gates PASS:163 passed,5 ignored. Native
connection tests preserve blocking+CLOEXEC/peer confirmation, overflow/zero
timeout rejection and bounded full-backlog failure. Removed two extraction-
only unused imports; main27253 terminal0 Profile bins cargo check and scoped
rustfmt PASS. New SCM client dead-code warnings intentionally remain visible:
they accurately show its missing production native adopter, not a reason to
claim end-to-end integration or hide warnings. Scoped diff check PASS.

Astra reviewed concrete next native interface: immutable versioned callback
table, retained context/image, installed after inheritance boundary before
Network acquisition. Installed reference, in-flight operations AND final
Description leases retain context; drain before teardown. Real pair owner
preallocates two unpublished HostFdObjects/inline endpoint leases plus typed
caller-owned rollback receipt; private synchronous installer changes BOTH exact
objects before real echo/confirmation. No allocation/RPC in installer or waits
under broker/inheritance lock. Receipt owns rollback through publication and
handles lost confirmation by explicit holder release, not connection EOF alone.
Holder attributes live on exact object/Description, NOT guest-FD map and NOT
Binder wire delegation. Current empty Binder attrs cannot carry managed objects.

Review also identifies admission response/vector allocations AFTER server claim:
client admission is not production-ready until preallocated whole-manifest
publication/response ownership closes that gap. Inert provider/pair-install
ownership slice may precede generic Binder batch work; private framing and
managed production pair creation stay OFF until propagation/full consuming
alias/PEEK and publication gates pass. Reviewer completed/retired.

Original normal actors61038/61329/61332/61380/61382 plus57154 freshly runnable,
OLD product. No native/runtime/DEX/daemon rebuild/restart, new capture or rendering
success, APK/profile changes, commit or push. Last Example Domain body WHITE
FAIL, full goal ACTIVE. Next FIRST implement this actual private pair installer/
retained native provider boundary, then the actual Root handoff and physical
body gate; do not substitute another generic transport fixture for rendering.

### 2026-09-19 — native endpoint ownership boundary; rendering still unverified

Previous explanation-only turn NO PROGRESS; re-read AGENTS, Current goal and
latest checkpoint. Inspected current native and Rust ownership rather than
assuming an active installer. Engine-sys now describes a versioned private
SCM pair-install/provider ABI (C/Rust sizes56/32/48). Profile's
NativeScmEndpointProvider retains Rust context, calls genuine authenticated
pair registration, and clears native provisional attributes plus explicitly
releases both holders on failed/ambiguous confirmation. Cleanup errors remain
reported. Native EndpointLease lives inline on actual HostFdObject, not a
guest-FD map; PairInstallReceipt holds rollback through publication. This
boundary was present from preceding implementation but had not been checkpointed.

This turn disarmed native receipt BEFORE endpoint cleanup callbacks to prevent
reentrant rollback. Actual adapter.cc translation unit, including this owner,
passes arm64 C++20 syntax compilation with Wall/Wextra/Werror/Wpedantic. This
is NOT a linked/sanitized native adoption test. Fixed the known owner fixture's
immediate post-close assertion to use its existing actual-native-EOF observer;
the one-second deadline fails the test only, never retires production aliases.
Preceding run58218 failed that assertion; current run8652 terminal0: EngineSys
15 PASS, Profile164 PASS/5 ignored. Scoped rustfmt with repository edition2024
PASS (default-edition invocation initially reported formatting differences).

Production native provider installation, pair activation/atomic publication,
managed consuming aliases and Binder attribute propagation remain disconnected.
Do not equate ABI/component tests with actual Root handoff repair. Normal actors
61038/61329/61332/61380/61382/57154 freshly observed, unchanged OLD product.
No product rebuild/restart, APK/profile edits, fresh rendering capture, commit
or push. Last physical example.com body WHITE FAIL; no new acceptance evidence.
Next connect the reviewed native installer at its actual owner and close the
required propagation/publication gates, then verify Root handoff and real
Example Domain rendering. Full original Probe-removal goal remains ACTIVE.

### 2026-09-19 — independent native SCM installation owner

Previous turn PROGRESS (native rollback ordering and real EOF test correction).
Read AGENTS, Current goal/latest and full ADR0005. Astra reviewed actual Engine,
RuntimeOwners/ProviderBridge and native broker teardown. Approved independent
Instance wrapper with stop-acquire/EBUSY drainage, not an Engine Arc. Installed
original Rust context alone does not establish native code-image quiescence.
Reviewer completed and retired; bounded Luna implementation dispatch hit the
existing agent-thread limit, so main implemented the reviewed owner directly.

New scm_endpoint_provider.cc owns copied immutable original callback/context,
installed/acquired/receipt/endpoint/in-flight wrapper references and explicit
Empty/Installing/Installed/Stopping/Retiring transitions. No external callbacks
under its independent mutex. Busy uninstall stops fresh acquisition without
dropping the installation reference; existing owners may retain and release
grants while draining. Replacement is rejected until retirement completes.
Original context retain/release occurs outside lock with phase reservations,
including reentrant install/uninstall rejection. No pair/private framing was
activated. Production archive builder now explicitly compiles/archives this
ordinary provider module, NOT its genuine test fixture.

Actual-module lifecycle fixture45359 terminal0 passes ASan/UBSan/TSan builds
and execution: invalid/duplicate install, callback-table copy, in-flight
delegate EBUSY, stopping acquire rejection, existing-ref cleanup, reentrant
original callbacks and fresh install after retirement. Delegates deliberately
return ENOSYS, NOT fabricated registration ACKs. This is resource-ownership
evidence, not authenticated daemon/native pair or parallel stress acceptance.
Native module C++20 Wall/Wextra/Werror/Wpedantic syntax PASS; scoped shell
syntax and diff checks PASS. No full archive/product build was run.

Review found important next integration hazard: Engine.close skips native
shutdown when process was not entered, but Runtime shutdown adopts native
provider shutdown unconditionally and may erase lease accounting without
deactivating Network. Require real broker quiescence, not merely refcount0;
Engine Drop is the correct SCM uninstall boundary AFTER provider teardown,
and must refuse/abort unsafe unload on active broker or busy SCM owners.
Close is too early. Actual socket-broker OwnerClose destroys endpoint owner
before decrementing live object count; deactivate waits active callers and
owner callbacks. Host/Engine installation and the early-failure correction
are next, before managed pair publication/propagation adoption gates.

No APK/profile changes, product restart/capture, commit or push. The latest
physical Chrome body remains WHITE FAIL; full original goal ACTIVE. This
provider still has no Host/Engine installation caller; do not call it complete
production adoption or Root handoff repair. Preserve real example.com priority.

### 2026-09-19 — Host/Engine installation and pre-entry provider release

Previous turn PROGRESS; read AGENTS, Current goal/latest before work. Fixed
the reviewed pre-entry teardown hazard at Runtime ownership: NativeResource
now defaults to NOT claiming native provider teardown; EngineSession reports
it only after an entered process's successful actual shutdown. Runtime adopts
provider bookkeeping only with that evidence; otherwise real ProviderBridge
clear performs counted native release. Added actual-Bridge pre-entry regression.
Initial23058 test compilation failed on fixture Debug/default-G types; corrected.
39763 terminal0 Engine16/Runtime102 PASS (Runtime2 ignored).

Connected ordinary Host process_scm_endpoint -> Engine's independent
ScmEndpointInstallation -> required native versioned installation ABI. Trusted
profile socket selected once before Network acquisition, reused for Binder;
native retains context before local Rust owner drops. Engine Drop uninstalls
AFTER provider teardown; active native broker or busy owner aborts rather than
dlclose under live native users. Managed pair activation/framing remains OFF.
Both-flavor linker exports/checks and legacy genuine APK fixture link export
list now include exact install/uninstall/broker-status ABI. Production graph
manifest explicitly includes new module/headers and previously omitted compiled
inheritance/retained-export inputs, preventing stale warm provider archives.

Current integration93671 terminal101: Engine16/Host45 PASS (Host2 ignored);
Runtime101 PASS/1 FAIL/2 ignored. Failure is existing service_endpoint foreign-
copy test's immediate flock acquisition after last local close. Exact isolated
same test PASS; full Runtime serial gate terminal0 102 PASS/2 ignored. Do NOT
claim parallel gate fixed or silently replace its failure with serial success.
New pre-entry regression passes. Native lifecycle45397 terminal0 ASan/UBSan/
TSan PASS; scoped Rust formatting/diff checks PASS. Provider manifest test
53049 terminal0 seven PASS, including actual recipe conservative-input manifest.
Observed pre-main dyld wait without restarting; no full product archive/flavor
build yet.

No app restart/capture, APK/profile modification, commit or push. These are
source-level installation/lifetime changes, NOT native pair/Binder propagation
adoption or visible Root repair. Last physical example.com remains WHITE FAIL;
full original goal ACTIVE. Next verify provider graph gate, build the actual
native/product closure, then close native pair publication/propagation/consumer
gates and re-test real Root handoff plus fresh physical Example Domain rendering.

### 2026-09-19 — produced native provider archive and genuine acceptance

Previous turn PROGRESS; read AGENTS, Current goal/latest. No competing build
observed;116GiB available. Regenerated supported graph75906 terminal0 (digest
b25dd7a39b95e6216932de606d452c930353abeadd4b4681384395c8163aec07).
Pinned AOSP Ninja dry-run selected only ONE provider closure edge; actual5425
terminal0 ARCHIVE_PASS36, no ART/HWUI rebuild. nm confirms actual native archive
contains required SCM install/uninstall and broker-status functions. Exact
same archive target subsequently reports no work to do. This is SDK archive
warm evidence, not both complete runtime flavors' warm acceptance.

Downstream15103 failed; direct smoke exit23 after traversal located missing
fixture FD-boundary installation before Unix socket creation. Production
ENOSYS behavior stayed unchanged. Added explicit owner-PID/thread-sealed
functional fixture callback: actual native operation runs only on fixture
thread, never in fatal-check fork children. This is NOT production Rust
lock/spawn proof. First34767 compilation found missing cerrno; corrected.
Final39289 terminal0 genuine archive acceptance PASS36/routes798: Unix
connect/send, filesystem/pipe/poll, credentials/property snapshots, numeric/
errno isolation, streams, and real fdsan fatal child checks. Editing only this
fixture leaves production archive target no work to do; fixture is not a
production archive dependency. Scoped diff checks PASS.

Separate70191 terminal0 actual C++ FD-factory -> SAME production Rust guard/
spawn_owned native-boundary integration one PASS. No forwarding fixture is
substituted for that evidence. No product dylib/Host/daemon rebuild or restart,
fresh screenshot, APK/profile changes, commit or push. Existing Runtime parallel
flock fixture failure remains recorded, not fixed. Managed pair/framing/Binder
propagation remain OFF/incomplete; no actual Root repair or rendering acceptance.
Last physical example.com WHITE FAIL; full original goal ACTIVE. Next build
the product closure and verify actual required ABI loading, then finish native
pair publication/propagation/consumer adoption and real Chrome body validation.

### 2026-09-19 — fresh product Chrome visibly renders example.com

Previous explanation-only turn NO PROGRESS. Read AGENTS, Current goal and latest
checkpoint. Revalidated current default-profile actors through darwin-artctl ps:
41081 system, 41083 browser, 41130/41134 Chrome children. They are still live;
no timer, APK modification, profile reset or termination in this turn.

The preceding integration run produced the graphics runtime dylib with pinned
Ninja graphics-audit (registrar=51, fake-symbols=0), confirmed three SCM ABI
exports, and its exact dry-run was no work to do. Current Host and profile
daemon were rebuilt and normal actors gracefully replaced. Normal daemon
shutdown unmounted the profile; ensure remounted it before retrying unchanged
APK 2aaea8419d955677313f8b6dae3f0666916243ec55c3607a8711f46c9123b731.
These component build results are not both-flavor/full migration acceptance.

Fresh window capture /tmp/darwin-art-example-verified.png and original-pixel
inspection show Example Domain heading, body and Learn more, not a white web
body. Physical HID Ctrl+L, Ctrl+A, example.com, Enter (14 key events, CLI PASS)
followed by fresh /tmp/darwin-art-example-physical-nav.png shows example.com in
the address bar together with heading/body. Android scanout source/target logs
are 720x1280, with child layer owner41134 and central composition owner41081.
MoltenVK/Dawn activity is present; discovery also logs a rejected GLES adapter,
not proof of GLES fallback. The launcher enforces the Vulkan/Graphite contract.
Exact backend/device selection should still be audited before final exit.

Physical link click and Ctrl+R were delivered, but unchanged content alone does
not prove navigation/reload completion; do not call those acceptance PASS.
Initial capture had blank toolbar; subsequent address entry restored toolbar.
Consequently the old white-body failure no longer justifies generalized FD work.
Managed SCM framing remains OFF/incomplete, but is not a demonstrated blocker
for this visible page. Continue production closure/ownership migration and
physical navigation verification; preserve all original goal exit gates.

### 2026-09-19 — user-requested checkpoint and Calculator/DeskClock checks

User requests stop implementation here, commit and check only Clock/Calculator.
Read current instructions/checkpoint earlier in this work interval. Launched
unchanged installed Calculator SHA6f5e3d...776ae and DeskClock SHAac980b...7b87
with current target/debug Host and existing default profile, indefinite windows.
Actual registered actors49913 Calculator and51530 DeskClock render their screens.
Fresh captures /tmp/calculator-handoff.png and /tmp/deskclock-handoff.png;
Calculator keypad and DeskClock world-clock text are visible at Retina scale.
Both macOS titles incorrectly show package identifiers, not localized app names.

Physical HID Calculator 1,+,2,= sequence repeated after both launches finished:
/tmp/calculator-result-handoff2.png still blank result. Physical DeskClock
stopwatch-tab click remains clock list in /tmp/deskclock-stopwatch-handoff.png.
These interaction checks FAIL/unverified, not healthy-app acceptance. Do not
claim arithmetic or stopwatch operation works. No APK/profile modifications,
new fallback, or further implementation. Existing Chrome remains running.
Checkpoint stages current darwin-art subtree changes, excluding ignored build
outputs; no new APK/media/native binary extensions found among untracked files.
Tracked-only diff --check passed before staging, but full staged check reports
preexisting header EOF blanks and unified-patch context whitespace; retain patch
bytes rather than corrupt locked patches. Goal remains incomplete; user pauses
implementation at this checkpoint. Commit requested; push not requested.
