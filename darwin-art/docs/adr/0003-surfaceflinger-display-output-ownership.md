# ADR 0003: SurfaceFlinger owns display-output selection

Status: accepted for the current Android 16 compatibility slice

## Context

Android GPU and renderer service processes submit `SurfaceControl` buffers
after the application window exists. A host IOSurface environment variable is
therefore neither inherited reliably nor part of Android's producer contract.
Requiring each producer to select that host target caused valid Chromium child
buffers to be accepted locally but never latched by the central composer.

Retina also exposes an ownership hazard: Android layer geometry and producer
buffers use physical display pixels, while AppKit owns points and backing
scale. Producers must not apply a second macOS scale while choosing a target.

## Decision

- `SurfaceControl` producers submit target-independent layer transactions:
  owner/layer identities, current parent and relative-layer identities,
  buffer/crop/geometry, and acquire readiness.
- The central SurfaceFlinger-compatible service retains that state and selects
  an output by walking the current structural parent graph to an explicitly
  registered display root. Historical descendant buffers are not display
  anchors. Missing ancestors, cycles, detached trees, and ambiguous output
  membership fail instead of selecting an arbitrary target.
- A buffer transaction completes only when the selected output's Metal
  composition completes. Structural commits remain target-independent and do
  not masquerade as presentation fences.
- The Darwin output boundary owns the IOSurface and backing extent. Android
  source crops and destinations remain in Android pixels until final output
  projection; AppKit scans out backing pixels without applying another Retina
  scale.

The current cutover permits an explicit-target root presentation to register
the initial output anchor. A dedicated output registration/replacement/removal
API is follow-up work and will remove that launch-order dependency.

## Consequences

GPU service children no longer need `DARWIN_ART_HOST_IOSURFACE_ID`. Cross-
process layer identity stays in the existing Surface/SurfaceControl parcel
contract. Central SurfaceFlinger gains responsibility for output association,
actual completion notification, and rejection of detached or ambiguous
transactions.

This follows Android's guest contract and isolates the real macOS display
provider, so it is not an architectural divergence from AOSP. The ADR records
the Darwin provider boundary and the incremental path to explicit output
registration.
