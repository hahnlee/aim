# AIM compatibility runtime

This is the active work index. Keep only current scope, verified state, open
failures and acceptance commands here; Git history holds retired diagnostics.

## Current goal

Run unchanged Android APKs on macOS. The current acceptance baseline is stable
Chromium `example.com` rendering with physical input, navigation/reload, Retina
output and Graphite/Dawn → Vulkan → MoltenVK → Metal. GL, direct Dawn Metal,
disabled GPU/Graphite and CPU fallback do not satisfy acceptance.

AOSP PackageManagerService owns packages (#30, ADR 0009): the system process
runs `PackageManagerService.main` with its AOSP owners (Settings in the
profile's `/data/system`, UserManager, PermissionManager, AppOps, domain
verification, ART Service) and PMS itself serves `package`. Only installd,
apexd, artd and vold preparation are Darwin providers. Installs are
PackageInstaller sessions (`darwin-art install` → `cmd package install`);
launches take the launch activity, code paths, uid, label and icon from
PackageManager. The install ledger, Rust manifest inspector and package
endpoint are gone; existing profiles migrate into PMS Settings with their uids
and app data.

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
| Uid process state | Each process's Activities give its OomAdjuster state (resumed/paused → TOP with all capabilities, stopping → LAST_ACTIVITY, stopped → CACHED_ACTIVITY, none → SERVICE); the minimum per uid goes to `AppOpsService.updateUidProcState` on every change, and broadcasts and `getRunningAppProcesses` use the same state. `dumpsys appops`: DeskClock `state=top capability=LCMNFUAT`, `cch` after its process dies. |
| Activity visibility | Activities hidden behind an occluding Activity (window style from `AttributeCache`) lose app visibility and are stopped when the new top Activity reports idle (10 s idle timeout); their windows release their layers. Finishing the top Activity restores visibility and restarts them. Physical: DeskClock → city list → back. |
| Orientation / resize | Per-task revisioned geometry: launch orientation, `setRequestedOrientation`, real AppKit edge resize → DisplayManager callback, config/relaunch/`WindowStateResizeItem` transactions, WMS frames/insets and host backing on one revision. Physical: Calculator/DeskClock relaunch, Chromium/Blue Archive config change, five-point click map, popup through resize, two-app isolation, close mid-resize. |
| Blue Archive | Unchanged full-split APK starts landscape `1280×720` (2×), renders Unity frames, survives a 24-drag live-resize stress (240 revisions, 67 swapchain recreations, no fault) without relaunch, and takes physical clicks on Android dialogs and Unity UI. The Nexon patcher downloads and verifies all 1464 files (~650 MB), preprocessing completes and the login title screen renders; a CoreAudio process tap measures non-silent output there (peak 0.12). GMS is absent (game shows its notice). |
| Package manager | PMS scans `/data/app` and the pinned image's system partition (79 system packages); permissions, signatures (v3), privileged flags, features and PM queries are PMS answers. `default` was migrated in place; all eight APKs launch from PMS and Blue Archive reaches its title screen on its existing downloads. System features describe the host: faketouch (mouse/trackpad, no touchscreen), both screen orientations, audio output, GLES 3.0 (ANGLE Metal) and Vulkan 1.3 level 1 (MoltenVK, the image's vendor XMLs). |
| Services | Exact-client connection ledger, process-shared demand and service lifecycle lanes are adopted. Real remote death and reusable shutdown are not proven. |
| Graphics / SCM | Retained backing, fences and scanout diagnostics pass. ABI2 SCM/Binder callbacks and focused lifetime tests pass; unchanged-APK managed-transfer acceptance remains open. |
| Source licensing | Original work uses Apache-2.0; upstream notices and OpenJDK GPLv2 + Classpath scope are recorded in the repository's licensing documents. Binary distribution/source matching remains a separate gate. |

## Active failures

Open failures are GitHub issues; the ones blocking the current goal:

- Window close and Cmd+Q have no Android lifecycle (#15); popup
  `ACTION_OUTSIDE` (#17).
- Broadcasts beyond unordered registered delivery (#3); framework-compat
  class replacements (#45).
- SCM managed-transfer adoption (#18); process-death cleanup (#20).

## Next work

1. Deliver popup `ACTION_OUTSIDE` and touch-modal consumption (#17).
2. Let density follow the host backing scale through the same revision path (#27).
3. Run locale/label checks, then extend Chromium focus/tab/soak coverage (#19, #22).
4. Close relevant WMS/Binder/SCM lifetime gaps (#18, #20) and audit changed
   files for mixed ownership before declaring migration complete.

## Known limits

Acceptance remains APK-scoped. Camera, Bluetooth, biometrics, complete host-font
integration, external sensor mapping and long memory/performance soak are later
work. Ordinary app/service processes still use host `_exit()` (#20); reusable
sessions and complete VM/image quiescence are unproven. Provider manifests do
not yet cover every dynamic shell/build input. Parallel Binder mapping and
foreign-copy flock test failures need a reproduction (#21). The window menu
and keyboard suites cannot complete against production processes (#29).

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

- **AOSP PackageManagerService (#30):** bootstrap mirrors SystemServer
  (RuntimeInit, AppOps, SensorPrivacy, PMS, UserManager, ART Service,
  boot phases, user unlock); `ActivityManagerInternal` is published for PMS
  broadcasts and isolated uid owners. Install path: session staging on the
  writable `/data/app` mount, `Linux.mkdir`/`fsync`/`posix_fallocate`/
  `readlink` through the fs facade (`/proc/self/fd` names), StrictJarFile and
  SecurityLog JNI, guest `user.dir`, artd with dexopt off.
- **Launch from PackageManager:** `launcher-info` gives the launch activity,
  `/data/app` code paths, uid, label and icon; the native loader and
  androidfw open guest `/data/app` paths through the process namespace.
  Uids for process identity come from `packages.list`.
- **Ledger migration:** installs move into the `/data/app` layout and
  `packages.xml` keeps ledger app ids; copied or renamed profiles relocate.
- **Blue Archive download:** app processes run `RuntimeInit` (FATAL
  EXCEPTION pre-handler, kill-on-crash, `http.agent`); the patcher downloads
  and verifies all files and the title screen renders.
- **System services:** AMS receiver registration, sticky redelivery and
  unordered broadcasts with PMS permissions and visibility allow-lists,
  `BatteryService` from IOPowerSources, NMS channels.
