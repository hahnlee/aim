# ADR 0008: Each desktop task observes its own logical display 0

Status: accepted for the current Android 16 compatibility slice

## Context

Every Android application process owns one macOS window and one SurfaceFlinger
output. Before this decision all processes reported display 0 as a fixed
portrait `360×640dp` display, and AppKit resizing only reallocated the host
IOSurface. Android `Configuration`, `DisplayInfo`, ViewRoot frames, WMS input
frames and application buffers kept the old geometry. A mutable global display
size cannot fix this: resizing one window would resize every other app.

Android offers two representations for independent windows: one logical
display per window, or one display with per-task bounds (freeform). Apps in
this runtime assume `Display.DEFAULT_DISPLAY` in application contexts, there is
no shared desktop surface that several tasks compose into, and each process has
exactly one root task.

## Decision

- The system `TaskDisplayRegistry` keeps one revisioned `DisplayGeometry` per
  attached application process. `IDisplayManager.getDisplayInfo(0)` answers
  with the calling process's task geometry, and display callbacks registered by
  that process receive `EVENT_DISPLAY_BASIC_CHANGED` for it. Other processes'
  `DisplayInfo`, task bounds and input mapping are unaffected.
- ActivityTask (`TaskGeometryController`) is the only writer. It resolves each
  Activity's own `screenOrientation`/`configChanges` from PackageManager, swaps
  the task extent for fixed orientations at launch, on
  `setRequestedOrientation` and when a finish reveals another Activity, and
  adopts user resizes reported by the AppKit root in content points.
- Each revision is published in one order: DisplayManager, then a single
  `ClientTransaction` with `ConfigurationChangeItem`, per-Activity
  `ActivityConfigurationChangeItem` or `ActivityRelaunchItem` (+ lifecycle
  item) when the Activity does not handle the change, and a
  `WindowStateResizeItem` for every other window. Last comes the host root.
  WMS relayout computes frames, `SurfaceControl` sizes and the merged
  configuration from the same revision; `MATCH_PARENT` follows the task bounds,
  and `ClientWindowFrames.seq` carries the revision so ViewRootImpl discards
  older frames.
- The Darwin root is a provider only. It reports host facts, applies a revision
  by sizing the window (for Android-initiated extents) and the scanout backing
  to the window's real backing pixels, publishes the Android extent to
  SurfaceFlinger, and maps input through that same backing. Closed roots,
  older revisions and failed allocations never advance its applied revision.

## Consequences

Display 0 is process-relative. System services that need a global desktop view
(none exist in the current slice) must not read `TaskDisplayRegistry` as a
physical display inventory. Multi-window Activities within one process share
one task extent. A future Android desktop-windowing adoption can replace the
per-process display with per-task bounds on one display without changing the
host provider protocol, because the provider only exchanges revisions, content
points and Android pixel extents.

The Android density remains `160 × DARWIN_ART_WINDOW_SCALE`. Moving a window to
a screen with a different backing scale changes only the host backing, not the
Android raster; a later change can make density follow the reported backing
scale through the same revision path.
