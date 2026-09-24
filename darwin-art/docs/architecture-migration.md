# AIM compatibility runtime

This is the active work index. Keep only current scope, verified state, open
failures and acceptance commands here; Git history holds retired diagnostics.

## Current goal

Run unchanged Android APKs on macOS. The current acceptance baseline is stable
Chromium `example.com` rendering with physical input, navigation/reload, Retina
output and Graphite/Dawn → Vulkan → MoltenVK → Metal. GL, direct Dawn Metal,
disabled GPU/Graphite and CPU fallback do not satisfy acceptance.

Android-owned orientation/resize (ADR 0008) and Blue Archive's native startup
fault are fixed; see the verified state below. The service inventory and AOSP
migration are in [aosp-service-migration.md](aosp-service-migration.md).

## Ownership and completion gates

Android framework/ART/Binder own lifecycle, policy, resources, input, ViewRoot,
HWUI, SurfaceControl, BufferQueue and fences. Darwin providers own AppKit events,
process/filesystem transport, Metal/IOSurface presentation and host services.
Android pixels, macOS points and backing scale remain distinct.

Goal exit requires:

1. Production source/header/link closures exclude `probes/` and fixture JNI in
   both runtime flavors.
2. Entry, registration, input, GPU and shutdown have explicit policy, transport
   and lifetime owners.
3. Both runtime flavors build/link and the exact pinned Ninja repeat is no-work.
4. Fresh unchanged Calculator, DeskClock and Chromium pass required physical
   input and Retina GPU interactions.

No APK/profile changes, fabricated framework state or fallback count as proof.
Component tests are not application acceptance. See [AGENTS.md](../AGENTS.md),
[ARCHITECTURE.md](../ARCHITECTURE.md) and [ADRs](adr/).

## Verified state

| Area | Current evidence / remaining gate |
| --- | --- |
| Production closure | TextureView/SurfaceTexture, upstream TextureLayer, SCM/Binder/FS and shared sensor JNI/NDK build/link in both flavors. Graphics/headless audits and exact no-work repeat pass. |
| Chromium | Unchanged Browser renders Example Domain; physical URL entry, three IANA link/back cycles, reload, restart restoration and Gemini sheet close/reopen pass. Fresh 2× capture confirms Graphite/Dawn Vulkan → MoltenVK on Apple M2 Pro. Longer soak remains open. |
| Calculator / DeskClock | Physical Calculator pointer and keyboard arithmetic and DeskClock Stopwatch start/pause pass. Localized window labels remain incomplete. |
| Input / WMS | Exact-root ingress, readiness/focus fences, bounded first-key queue and receiver lifetime are adopted. Parent/process-death cleanup remains open. |
| Orientation / resize | Per-task revisioned geometry: launch orientation, `setRequestedOrientation`, real AppKit edge resize → DisplayManager callback, config/relaunch/`WindowStateResizeItem` transactions, WMS frames/insets and host backing on one revision. Physical: Calculator/DeskClock relaunch, Chromium/Blue Archive config change, five-point click map, popup through resize, two-app isolation, close mid-resize. |
| Blue Archive | Unchanged full-split APK starts landscape `1280×720` (2×), renders Unity frames, survives a 24-drag live-resize stress (240 revisions, 67 swapchain recreations, no fault) without relaunch, and takes physical clicks on Android dialogs and Unity UI. GMS is absent (game shows its notice). |
| Services | Exact-client connection ledger, process-shared demand and service lifecycle lanes are adopted. Real remote death and reusable shutdown are not proven. |
| Graphics / SCM | Retained backing, fences and scanout diagnostics pass. ABI2 SCM/Binder callbacks and focused lifetime tests pass; unchanged-APK managed-transfer acceptance remains open. |
| Source licensing | Original work uses Apache-2.0; upstream notices and OpenJDK GPLv2 + Classpath scope are recorded in the repository's licensing documents. Binary distribution/source matching remains a separate gate. |

## Active failures

- **Activity visibility:** an Activity behind a new top Activity is paused but
  never stopped or hidden, so translucent/unfinished top windows show it.
- **Popups:** `ACTION_OUTSIDE` is not delivered; outside taps reach the parent.
- **Blue Archive:** gameplay beyond the title/download notice is unverified
  (GMS unavailable; the 657 MB download was not started).
- **SCM:** authenticated custody, carrier propagation, all consumer variants,
  discard/truncation, close/crash and alias semantics remain before adoption.

## Next work

1. Add Activity stop/visibility transitions and popup `ACTION_OUTSIDE`.
2. Let density follow the host backing scale through the same revision path.
3. Run locale/label checks, then extend Chromium focus/tab/soak coverage.
4. Close relevant WMS/Binder/SCM lifetime gaps and audit changed files for mixed
   ownership before declaring migration complete.

## Known limits

Acceptance remains APK-scoped. Camera, Bluetooth, biometrics, complete host-font
integration, external sensor mapping and long memory/performance soak are later
work. Ordinary app/service processes still use host `_exit()`; reusable sessions
and complete VM/image quiescence are unproven. Provider manifests do not yet
cover every dynamic shell/build input. Parallel Binder mapping/shared-lock and
Runtime foreign-copy flock failures remain unresolved.
`android-window-menu/keyboard-acceptance.sh` still drive fixed-duration launches
and `DARWIN_ART_TEST_*` pointer hooks that only the fixture GPU loop reads;
Android app processes ignore both, so those suites do not complete.

## Acceptance commands

Serialize shared native/DEX builds and use pinned Skia Ninja.

```sh
bash tools/bionic-process-state-facade/audit.sh
cargo run -q -p art-bootstrap -- audit-runtime-graphics-link-fast
bash tools/aosp-core-apps-graphics-acceptance.sh
bash tools/android-window-menu-acceptance.sh
bash tools/android-window-keyboard-acceptance.sh
bash tools/android-window-geometry-acceptance.sh
bash tools/audit-art-jit.sh
bash tools/audit-profile-daemon.sh
cargo test --workspace
```

## Latest progress

- **Geometry:** ActivityTask/Display/WMS own per-task orientation and resize
  revisions; AppKit reports points and applies revisions (ADR 0008).
- **Blue Archive:** guest `dlopen` accepts `RTLD_GLOBAL`, so Unity uses the media
  NDK instead of its NULL-base JNI fallback; landscape frames and input verified.
- **Graphics/transport:** WSI extent semantics, BLAST producer dimensions,
  destination frames and one-way Binder delivery now follow AOSP/kernel rules.
- **Live resize:** SF aliases replaced output ids to the live owner and commits
  superseded transactions without presenting (ADR 0003); the producer queue
  retires consumer-held generations instead of failing (no generation cap —
  release completion lags composition); the signal trampoline honours
  `SA_RESETHAND`; native traps name their `dladdr` symbol.
