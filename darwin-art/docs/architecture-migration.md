# Darwin ART compatibility runtime

이 문서는 현재 구조, 완료 조건, 최신 검증 결과만 유지하는 living document다.
상세한 실험·진단·과거 체크포인트는 Git 이력에서 찾는다. 작업자는 시작할 때
`Current status`와 `Next boundaries`만 읽고, 종료할 때 검증된 변화만
`Latest progress`에 짧게 추가한다. 항목이 10개를 넘으면 기존 항목을 현재
상태에 합쳐 다시 압축한다.

## Product goal

macOS에서 수정하지 않은 Android APK를 Android 계약에 가깝게 실행하는
호환성 계층을 만든다. 현재 앱의 호환성을 실제 APK로 증명하되, 한 목표에서
Android 전체 지원을 주장하지 않는다.

- ART, ActivityThread, Binder, ViewRoot, Choreographer, HWUI,
  SurfaceControl과 Android 서비스가 앱이 보는 정책과 수명주기를 소유한다.
- AppKit, Metal, CoreAudio, Network.framework, CoreText 등은 좁은 Darwin
  provider에서 macOS 기능을 제공한다.
- 렌더링은 HWUI/SurfaceFlinger/Metal GPU 경로와 IOSurface 공유를 우선한다.
  CPU fallback이나 앱별 그림 합성은 호환성 완료 조건이 아니다.
- 호스트와 Android의 계약이 충돌하면 Wine 수준의 OS 결합을 목표로 경계를
  격리하고, Android와 다른 결정을 해야 할 때는 ADR을 남긴다.
- Retina logical/physical 좌표, 다국어 resource fallback, 물리 키보드,
  macOS 폰트·카메라·블루투스·보안 저장소 통합을 공통 계약으로 다룬다.

## Engineering rules

1. APK, framework 또는 AOSP 앱을 검증 편의 때문에 수정하지 않는다.
2. Probe, 앱 이름 분기, 강제 click, 고정 좌표 보정은 production 해법이 아니다.
3. AOSP가 소유해야 할 lifecycle·policy를 Darwin bridge가 대신하지 않는다.
4. macOS adapter는 사실과 mechanism만 제공하고 Android policy를 만들지 않는다.
5. 새 host resource ownership은 가능하면 Rust로 구현하고 C/C++ ABI는 좁힌다.
6. 하나의 파일이나 service가 policy, transport, storage와 resource lifetime을
   함께 소유하지 않도록 기능을 추가하기 전에 책임을 분리한다.
7. `DarwinServiceBridge` 같은 기존 monolith는 목표 종료 전에 변경된 범위의
   책임을 전용 모듈로 이동한다. 단순 파일 분할이나 이름 변경은 완료가 아니다.
8. shared Cargo/native/DEX build는 직렬화하고, stale artifact를 acceptance로
   인정하지 않는다.

## Architecture boundary

```text
Unmodified APK
  -> Android framework / ActivityThread / system services
  -> Binder + InputChannel + SurfaceControl contracts
  -> ART + HWUI + app native libraries
  -> narrow Darwin providers
       AppKit input/window | Metal/IOSurface | macOS network/media/filesystem
```

### Android-owned

- package/resource/configuration and locale resolution
- application, activity, service and process lifecycle
- Looper, Binder callback and UI-thread affinity
- ViewRoot input delivery and Choreographer frame scheduling
- SurfaceControl hierarchy, transaction, crop, alpha, latch and fence semantics
- permissions, AppOps, shortcuts, restrictions and connectivity callback policy

### Darwin-owned

- NSWindow/NSEvent and Retina backing-scale mechanisms
- Metal drawable and IOSurface import/export
- host process spawning, executable memory/W^X and Mach signal context
- macOS network, audio/video, fonts, camera, Bluetooth and secure services
- profile storage and daemon transport, without duplicating Android policy

## Current status

The bounded Android-owned APK lifecycle goal is complete as of 2026-09-16.
Production execution no longer depends on APK modification, profile reset,
CPU rendering or a Chromium-only runtime branch for the accepted flows.

### Verified applications

| Application | Verified behavior | Boundary |
| --- | --- | --- |
| AOSP Calculator | physical CGEvent input, `2+3=5`, History/menu/resize | Android ViewRoot/HWUI/SurfaceFlinger/Metal |
| AOSP DeskClock | launch, labels, Timer and popup/input | Android resources/lifecycle plus common GPU path |
| AOSP Calendar | Day/Week/Month and keyboard popup selection | Android input and window/service contracts |
| Chromium | unchanged APK renders `example.com`, toolbar/bottom bar, menu and contentful tab grid; three physical tab round trips; process set stable for over seven minutes | Activity/service lifecycle, child Surface, ANGLE/Metal, Binder and signal recovery |
| Blue Archive | unchanged APK title/login/Notice UI and synthetic Cancel transition | first-frame/nativeRender only; login and gameplay are not claimed |
| SolitaireCG | custom View drag consumed without crash | input compatibility smoke test |

### Critical completed contracts

- Android arm64 signal handlers can change `ucontext_t` and resume guest code.
  Darwin thread state restores the changed mask, x0-x30, SP, PC and PSTATE,
  including authenticated pointer-bearing registers.
- SurfaceFlinger resolves parent-relative transform, crop, visibility and alpha,
  then projects logical Android coordinates to Retina scanout.
- Chromium renderer/GPU service processes reach Android create/bind/publish
  lifecycle and submit a child Surface under the real BLAST parent.
- Framework services now include the goal-required connectivity, AppOps,
  restrictions, shortcuts, UsageStats, thermal, UI mode and process contracts.
- Native-thread JNI attachment has explicit ownership and shutdown ordering.
- The native graph owns headless and graphics runtime artifacts and verifies
  warm no-op rebuilds instead of accepting stale dylibs.

## Acceptance commands

Run focused tests after changes to their boundary and the full app gate before
claiming the goal remains green.

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

Chromium evidence from the final run is kept outside Git under
`_build/goal-evidence/chrome-signal-fix/`. It includes `escape.png`,
`menu-open.png`, `tab-grid-4.png` and `three-transitions.png`. Acceptance logs
for Calculator and DeskClock are under
`_build/aosp-core-apps-graphics-acceptance/`.

## Known limits

- This is APK-scoped compatibility evidence, not full Android compatibility.
- Blue Archive login, download and gameplay require external account/server
  state and remain unverified.
- macOS camera, Bluetooth, Touch ID and font unification remain provider work.
- Chromium and other large apps still need longer soak, memory and performance
  gates; a short feature PASS is not sustained-use acceptance.
- Unsupported Binder transactions must continue to fail honestly rather than
  return invented data.

## Next boundaries

1. Turn the accepted flows into repeatable clean-profile soak/performance gates.
2. Continue extracting mixed policy/transport/lifetime responsibilities from
   `DarwinServiceBridge` and comparable large files as each area is changed.
3. Add macOS-integrated providers one capability at a time, with an ADR where
   behavior intentionally differs from Android hardware.
4. Expand unchanged-APK coverage only after the preceding common contract is
   tested; do not accumulate app-specific compatibility layers.

## Latest progress

### 2026-09-16 — Android signal recovery and bounded goal acceptance

- Fixed the common Bionic signal ABI so a guest `SA_SIGINFO` handler can update
  Android arm64 PC/registers and continue after a generated-code fault.
- Added an Android ELF regression that faults, advances guest PC and resumes;
  the Bionic process-state audit and graphics-link audit pass.
- Unchanged Chromium rendered `example.com`, exposed menu and tab grid through
  physical CGEvent input, completed three round trips and retained the same
  main/GPU/renderer PIDs for more than seven minutes without fatal faults.
- Re-ran unchanged Calculator and DeskClock acceptance on the common
  HWUI+SurfaceFlinger+Metal path; Calculator produced `2+3=5` and DeskClock
  reached Timer.

### 2026-09-16 — documentation compaction

- Replaced the append-only checkpoint ledger with this current-state document.
  Git history remains the authoritative archive for removed diagnostic detail.
- Future entries are capped and periodically folded into `Current status` to
  prevent progress logs from becoming a second source tree.
