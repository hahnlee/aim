# Runtime architecture requirements

Read the Current goal and latest checkpoint in docs/architecture-migration.md
before work. Append verified progress before ending a work turn.

- Production APK execution is an Android compatibility runtime, not a probe.
  Do not add production behavior to test fixtures or Probe classes. Existing
  production dependencies there are migration debt, not patterns to copy.
- Preserve AOSP ownership: ActivityThread/LoadedApk/ContextImpl own application
  state; framework transactions own activity/service lifecycle;
  ViewRoot/Choreographer/HWUI own traversal and rendering; SurfaceControl,
  BufferQueue and fences own buffer submission and composition contracts.
- Use AOSP implementations where available. Do not replace missing framework
  behavior with app-specific branches, reflection-based field fabrication,
  swallowed exceptions, success-returning stubs, or manual lifecycle callbacks.
  Diagnose the missing platform contract and implement it at its owner.
- Split production code by Android subsystem and ownership. Do not grow generic
  Bridge/Manager/Utils files spanning activity, window, display, input, storage,
  and service responsibilities. Separate platform transport from policy.
  Renaming or moving a bypass does not count as replacing it.
- Darwin-specific code belongs at genuine macOS boundaries such as AppKit
  events, process/IPC transport, filesystem and Metal presentation. Its presence
  alone is not a defect; duplicating Android framework policy there is.
- Keep test fixtures outside production source ownership. New build inputs
  must explicitly identify runtime modules versus test-only sources.
- Do not modify APKs, reset profiles or add CPU fallback to make acceptance pass.
  Report physical interaction failures even when process-level tests pass.
- Latest model policy (2026-09-14): ordinary work and debugging stay with the
  main agent (user-preferred model: GPT-5.6 Sol). Delegate bounded implementation
  to Luna high; seek Astra for architecture reviews, consequential programming
  decisions and higher-level judgment. Keep
  shared native/DEX builds serialized and integration/acceptance with the main
  agent. Parallel work does not relax Android ownership or acceptance criteria.
- Before adding production code, identify its Android subsystem owner and the
  narrow Darwin boundary (if any). Split unrelated responsibilities out of an
  oversized file before extending it; do not append another subsystem to it.
- A split must establish explicit interfaces, state/lifetime ownership and
  independently testable contracts, not merely textual includes in a monolith.
  Keep policy in its Android owner and native resource ownership behind narrow
  platform APIs. Prefer Rust for new host resource/transport ownership where
  compatible with the AOSP ABI; preserve upstream framework implementations.
- Do not introduce temporary production overlays, fake service responses or
  probe-only launch behavior as intermediate acceptance. An unfinished contract
  remains unfinished until the real production path and app interaction pass.
- Before reporting a migration complete, identify the production caller, the
  new subsystem owner and the old bypass removed. Component-only integration
  must be labeled as such; additional files or passing unit tests alone do not
  establish production integration.
- Treat Android pixels/density and macOS points/backing scale as separate
  coordinate systems. Every display, resize, input and capture change must
  preserve the Retina mapping explicitly; a 2x scanout must not be an upscale
  of a 1x Android raster.
- Preserve Android resource and locale semantics. Never expose a numeric
  resource identifier or package-name fallback when PackageManager/Resources
  should resolve localized text, and test at least one non-default locale when
  changing label or text lookup.
- Prefer Wine-style host integration: keep the Android API/ABI and policy at
  the guest-facing boundary, then implement the narrow provider with the real
  macOS service where practical (including Bluetooth, biometrics, camera,
  media, certificates and host fonts). Keep compatibility-only contamination
  out of those provider boundaries.
- Font ownership is split deliberately: APK-bundled fonts and Android generic
  family/metric contracts remain Android-owned, while installed host fonts may
  be exposed through a Darwin font provider and participate in fallback. Do
  not silently substitute a host face when Android layout depends on another
  face's metrics.
- Use Wine's guest-contract/host-provider split as the default precedent for
  ambiguous OS integration. Record an ADR under docs/adr/ before implementing
  a materially different ownership decision, including rationale, compatibility
  cost and the path back to host integration.
- Treat Android as a moving upstream target. Prefer version-pinned AOSP owners,
  generated ABI descriptions and narrow per-release adapters over copying
  policy into a permanent Darwin abstraction.
- Continuously audit file size and authority. In particular, do not add new
  production responsibilities to DarwinServiceBridge or another broad service,
  bridge, manager or utility. A file that acquires policy plus transport plus
  resource lifetime must be split before it is extended.
- Oversized-owner cleanup is a goal-exit gate, not an instruction to replace the
  active compatibility objective with a refactoring-only objective. Before the
  current goal is declared complete, audit the production paths it changed and
  extract any remaining mixed responsibilities from DarwinServiceBridge and
  similar monoliths behind explicit subsystem interfaces and tests.
- Keep each goal acceptance-scoped. Compatibility work for the APKs named in
  the current goal does not imply full platform support; implement the common
  Android contract exposed by their real failures, and record unrelated system
  integrations as follow-up work rather than expanding the active goal.
- Conserve cost by assigning bounded implementation to Luna high. The main
  agent performs ordinary debugging and integration directly, seeking Astra
  review for structural or higher-level decisions. The main agent retains
  shared-build serialization and verification of actual acceptance evidence.
  A preferred model in these instructions does not change the active model;
  never claim a model switch that has not occurred.
  Retire completed agents promptly and never duplicate a shared
  native or DEX build across agents.
