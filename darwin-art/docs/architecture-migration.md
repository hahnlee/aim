# Darwin ART compatibility runtime

This is the active work index. Keep only current scope, verified state, open
failures and acceptance commands here; Git history holds retired diagnostics.

## Current goal

Run unchanged Android APKs on macOS. The current acceptance baseline is stable
Chromium `example.com` rendering with physical input, navigation/reload, Retina
output and Graphite/Dawn → Vulkan → MoltenVK → Metal. GL, direct Dawn Metal,
disabled GPU/Graphite and CPU fallback do not satisfy acceptance.

Next compatibility work is Android-owned orientation/resize and Blue Archive's
native startup fault. The service inventory and intended AOSP migration are in
[aosp-service-migration.md](aosp-service-migration.md).

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
| Input / WMS | Exact-root ingress, readiness/focus fences, bounded first-key queue and receiver lifetime are adopted. Parent/process-death cleanup and full resize/configuration settlement remain open. |
| Services | Exact-client connection ledger, process-shared demand and service lifecycle lanes are adopted. Real remote death and reusable shutdown are not proven. |
| Graphics / SCM | Retained backing, fences and scanout diagnostics pass. ABI2 SCM/Binder callbacks and focused lifetime tests pass; unchanged-APK managed-transfer acceptance remains open. |

## Active failures

- **Blue Archive:** provider lookup and ABI split-name projection now reach Unity
  initialization. Full-split launch then SIGSEGVs at
  `darwin_art_bionic_memcpy`; no playable frame is verified. Diagnose the
  source pointer's allocation/mapping owner rather than masking `memcpy`.
- **Orientation:** Blue Archive's `screenOrientation=11` reaches `ActivityInfo`,
  but display/WMS and the host window remain fixed portrait `360×640dp`.
- **Resize:** AppKit reallocates IOSurface backing while Android ViewRoot, Unity,
  WMS frames and input retain old geometry. Implement one root/task-scoped,
  revisioned Android geometry transaction; do not add a mutable global size.
- **SCM:** authenticated custody, carrier propagation, all consumer variants,
  discard/truncation, close/crash and alias semantics remain before adoption.

## Next work

1. Implement ActivityTask/Display/WMS-owned orientation and resize using the
   pinned Android 16 client transactions; keep AppKit as a narrow provider.
2. Trace and repair Blue Archive's invalid native source pointer, then verify an
   unchanged full-split APK with a real captured frame and physical input.
3. Re-run Calculator/DeskClock resize and locale/label checks, then extend
   Chromium focus/tab/soak coverage.
4. Close relevant WMS/Binder/SCM lifetime gaps and audit changed files for mixed
   ownership before declaring migration complete.

## Known limits

Acceptance remains APK-scoped. Camera, Bluetooth, biometrics, complete host-font
integration, external sensor mapping and long memory/performance soak are later
work. Ordinary app/service processes still use host `_exit()`; reusable sessions
and complete VM/image quiescence are unproven. Provider manifests do not yet
cover every dynamic shell/build input. Parallel Binder mapping/shared-lock and
Runtime foreign-copy flock failures remain unresolved.

## Acceptance commands

Serialize shared native/DEX builds and use pinned Skia Ninja.

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

## Latest progress

- **Runtime:** shared Java/NDK sensor ownership removed the missing JNI crash;
  graphics/headless and exact no-work gates pass.
- **Blue Archive:** PM provider lookup and actual ABI split names remove two
  startup failures; Unity now exposes the native `memcpy` fault.
- **Physical:** Chromium navigation, reload, restart, Gemini sheet, fresh Retina
  capture and Vulkan/MoltenVK path pass on unchanged APK bytes.
