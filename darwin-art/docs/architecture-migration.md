# Darwin ART compatibility runtime

Read goal/status/progress before work; replace stale facts. Limit: 150 lines, five progress items.

## Current goal

Run unmodified Android APKs on macOS. First priority is stable normal Chromium
`example.com` rendering: visible Example Domain heading/body, physical input,
reload/navigation and a fresh Retina capture. Intermittent recovery is not repair.

Chromium must use Graphite/Dawn → Vulkan → MoltenVK → Metal. GL, direct Dawn
Metal, disabled GPU/Graphite and CPU fallback cannot satisfy acceptance.

Original migration gate: installed-APK production paths exclude `probes/`
source/header/link dependencies and fixture JNI. Fixtures stay test-only.

## Ownership and completion gates

Android framework/ART/Binder own policy, lifecycle, input, resources, HWUI,
SurfaceControl, BufferQueue and fences. Darwin providers own AppKit events,
process/filesystem transport, Metal/IOSurface and real host services. Keep
Android pixels/density distinct from macOS points/backing scale.

[AGENTS.md](../AGENTS.md) contains engineering rules; [ARCHITECTURE.md](../ARCHITECTURE.md) describes subsystem boundaries.
Custom Binder services and AOSP replacement: [service migration](aosp-service-migration.md).
Decisions: [ADRs](adr/), [foreground authority](adr/0004-fresh-desktop-foreground-authority.md),
[managed SCM lifetime](adr/0005-managed-scm-transfer-lifetime.md), [remote SurfaceTexture](adr/0006-remote-surface-texture-producer.md), [sensor inventory](adr/0007-sensor-inventory-boundary.md).

Goal exit requires all of:
1. Production source/header/link closures exclude fixtures in both flavors;
   tests exercise production objects without linking test code into the runtime.
2. Entry, registration, input, GPU and shutdown have explicit policy/transport/
   lifetime owners; audit changed paths for remaining mixed responsibilities.
3. Both runtime flavors build/link and the exact pinned Ninja repeat is no-work.
4. Fresh unchanged Calculator, DeskClock and Chromium pass physical pointer/key,
   Retina GPU rendering and required app interactions on those product bytes.

No APK/profile changes, fabricated framework state or fallback for acceptance.
Component tests are separate from app evidence.

## Current status

| Area | Verified state / remaining gate |
| --- | --- |
| Production closure | TextureView/SurfaceTexture, upstream TextureLayer, SCM/Binder/FS and shared sensor JNI/NDK adoption build/link in both flavors. Graphics/headless audits pass with exact no-work repeat. |
| Chromium | Unchanged normal Browser opens Example Domain with heading/body, physical URL entry, three Learn more→IANA→back cycles, menu reload and restart restoration. Fresh 2× capture and Graphite/Dawn Vulkan→MoltenVK Apple M2 Pro device are verified. Gemini sheet click/close/reopen also passes. Longer soak remains open. |
| Calculator / DeskClock | Latest physical Calculator pointer `1+2=3`, keyboard `4+5=9`, and DeskClock Stopwatch start/pause pass. Titles still show package IDs instead of localized labels. These checks predate TextureView changes. |
| Input / WMS | Exact-root key ingress, readiness/focus fences, bounded first-key queue and receiver lifecycle owners are adopted; global key enqueue and Init focus shortcut removed. BinderProxy root capture now uses genuine AOSP nodes and native terminal metadata. |
| Services | Exact-client/SYSTEM connection ledger, process-shared demand, launch capability owner, lifecycle lanes and ordered notifications are adopted; transport runs outside the ActiveServices monitor. Race fixtures and isolated genuine BinderProxy recipient-query JNI pass, not wire-obituary or reusable-shutdown proof. |
| Graphics lifetime | Immutable retained backing, allocation/fence/scheduling owners and scanout diagnostics are adopted. Real Skia/Metal callback retention and failure cleanup pass; concurrent context teardown is not proven. |
| SCM | ABI2 SCM and Binder capability callbacks pass genuine RPC tests. Atomic central publication, FS group admission, native framing and retained Binder paths pass focused checks. Pair/consumer/Host bundle adoption now builds/links in both flavors; unchanged-APK operation and physical acceptance remain open. |

### Chromium diagnosis to retain

- Historical and current Root tracing join exact channel/guest FD/native SO+PCB
  to direct SCM handoff: a reciprocal endpoint exported at state258 arrives in
  GPU at state290, then browser EOF invalidates Root and GPU callback. This
  establishes the observed failure chain, not the exact kernel-GC mechanism.
- User confirms Gemini-logo clicks triggered prior crashes. Physical tracing
  found missing TextureView JNI, lost parcel geometry, incomplete remote
  BufferQueue/MAILBOX transport, and HWUI's Android-only layer-consumption
  guards. With those repaired, HWUI consumed real GPU frames and exposed the
  final crash: direct Skia EGL calls bypassed Android AHB import, then macOS
  ANGLE rejected `GL_TEXTURE_EXTERNAL_OES`. The consumer image attached to a
  private staging texture, leaving Skia's own texture black. Direct imports now
  use 2D and attach to Skia's name; the Gemini sheet renders and reopens.
- Repeated link navigation previously crashed at unregistered Android 16
  `SystemSensorManager.nativeClassInit`. The sensor-owned JNI now registers
  the pinned table and shares an honest zero-mapped-sensor inventory with NDK;
  the same physical navigation succeeds three times without the exception.

## Next boundaries

1. Preserve passing two-flavor/headless and no-work build gates for the
   repaired SurfaceTexture/HWUI/Skia and sensor paths; retain physical evidence.
2. Extend Chromium tab/focus/soak coverage after the verified Example Domain,
   reload, link/back, Retina and Vulkan path; distinguish GPU content frames
   from native UI/input acknowledgements in future failures.
3. Managed SCM adoption must meet ADR0005: authenticated bounded custody before
   sender release; managed-carrier/Binder propagation; full-control intake for
   every consuming variant; discard/truncation, close/crash and alias semantics.
   Peek rejection requires unchanged-APK operation evidence proving it unused.
4. Rerun fresh Calculator/DeskClock input, resize and locale/label checks after
   runtime changes; preserve the already verified physical target-owner guards.
5. Close remaining relevant input/Binder/WMS settlement and audit mixed owners
   before declaring the original migration complete.

## Known limits

- Acceptance is APK-scoped. Blue Archive's declared provider lookup and ABI
  split name now reach Unity initialization, but the full-split launch still
  faults in native `darwin_art_bionic_memcpy`; no playable frame is verified.
  Orientation 11 survives PM mapping but display/WMS stays portrait; AppKit
  resize changes only backing geometry, so Android layout/input diverge.
- Ordinary APK/service processes use host `_exit()`. Reusable macOS sessions
  are rejected; joined Binder workers, sealed admission, callback/task/code
  retention and complete quiescence before VM/image unload remain unproven.
  Live-input `DARWIN_ART_FORCE_EMBEDDED_SHUTDOWN` is unsupported.
- WMS parent-subtree/process-death cleanup, real remote Java death callbacks,
  directional Binder authority-loss settlement and future input EOF/ACK
  obligations remain incomplete. Dormant zero-interest peer-close detection,
  owner-only retry and offscreen active-pointer cancellation also remain debt.
- Standalone host graphics audit still lacks six existing ImageDecoder host
  providers; both real runtime flavors link. Test-only failfast SurfaceTexture
  seams do not establish this standalone gate or complete platform support.
- Provider manifests cover reviewed current recipes, not all dynamic shell,
  build.rs, Cargo inheritance or symlink reads. A probes-free graph alone does
  not prove complete invalidation; liblog is externally materialized.
- Parallel Binder VM-mapping/shared-lock failures and the Runtime foreign-copy
  immediate-flock test failure remain unresolved; isolated/serial PASS does not
  establish a green parallel workspace suite.
- Camera/Bluetooth/biometrics/complete host-font integration and long app soak,
  memory/performance testing remain follow-up work outside this goal.
- Sensor inventory means no mapped Android sensors, not no host hardware;
  IOHID mapping, event queues and direct channels remain future integration.

## Acceptance commands

Serialize owner tests/full gates from `darwin-art/`; use pinned Skia Ninja.

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

Chromium physical Graphite/Dawn Vulkan acceptance is recorded above; rerun
on subsequent graphics changes rather than treating live processes as proof.

## Latest progress

- **Runtime:** Shared Java/NDK sensor owner removed fake NDK queue creation
  and the missing JNI crash. Graphics/headless audits and exact no-work pass;
  Direct EGL import and Skia 2D AHB sampling remain in production closure.
- **Blue Archive:** PM provider query and inspected ABI split names remove two
  startup failures; full-split Unity now starts, then SIGSEGVs in native memcpy.
  Landscape/resize remain open at Android display/WMS configuration ownership.
- **Ownership:** ABI2 SCM and Binder callbacks pass genuine RPC. Binder receipt,
  manifest/session and broker/FS/engine checks pass. Native tests cover guardian
  retention, lost ACK cleanup and zero-width record semantics; provider/FS ports
  remain labeled component evidence.
- **Physical:** Unchanged normal Chromium displays Example Domain, survives
  three IANA link/back cycles, reload and restart. Fresh 2× window capture,
  Graphite/Dawn Vulkan and MoltenVK Apple M2 Pro are evidenced; Gemini sheet
  close/reopen also passes.
