# HWUI display-list Metal replay gate

`tools/run-android16-hwui-metal-replay-gate.sh` is the G2/G3 acceptance gate
for the first GPU-backed HWUI boundary. It links the pinned Android 16 HWUI
static archive and the GPU-enabled Ganesh Metal Skia archive, records Android
canvas operations with `SkiaRecordingCanvas`, installs that display list on a
`RenderNode`, and replays it through `RenderNodeDrawable` into a Metal-backed
drawable texture.

The probe exercises the same stock-style primitives used by the Material UI
fixture: a white background, rounded card, rounded button, and an expanding
pressed-state ripple. Each frame records a fresh display list and presents the
same `CAMetalLayer` drawable without a CPU readback or a full-frame blit.

The expected result is:

```text
hwui-metal-replay: frames=8 recording=1 rendernode=1 ripple=1 cpu-readback=0 full-frame-blits=0 drawable-direct=1
```

This gate deliberately does not claim that the host `SkiaGpuPipeline` is
complete. The Android host pipeline class is still a platform stub. The gate
locks down the reusable G2/G3 seam—`RecordingCanvas`/`RenderNodeDrawable` to a
Ganesh Metal surface—so the production RenderThread integration can replace
the current CPU bitmap path without changing display-list recording semantics.
