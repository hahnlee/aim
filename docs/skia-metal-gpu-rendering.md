# Skia Metal rendering path

The Darwin host now has a GPU-backed Skia smoke gate at
`tools/build-android16-skia-metal-gpu.sh`.

The gate builds the pinned Skia revision with Ganesh Metal enabled, obtains a
`CAMetalLayer` drawable, wraps its `MTLTexture` in a
`GrBackendRenderTarget`, and renders directly into that texture. The command
buffer presents the same drawable; there is no CPU bitmap readback, IOSurface
round-trip, or full-frame blit in this path.

The current gate is intentionally G0/G1: it proves the Metal device, Ganesh
context, drawable lifetime, direct render target, flush, and present contract.
The application UI is not silently switched to this path yet. The production
integration must next connect Android `RecordingCanvas`/`RenderNode` replay to
the host RenderThread and a Metal-backed HWUI pipeline. That work must retain
the same ownership rules: the RenderThread owns the Metal queue/context and
drawable, while the UI thread only submits display-list work.

“Zero-copy” here means zero application-level full-frame CPU copies and zero
display-path blits. Skia may still allocate GPU intermediates for effects such
as saveLayer, MSAA, or filters, and the window compositor remains outside this
contract. IOSurface remains an explicit capture/export fallback, not the normal
display target.

Run the gate on macOS with:

```sh
./tools/build-android16-skia-metal-gpu.sh
```

Expected output includes:

```text
skia-metal-gpu: frames=8 cpu-readback=0 full-frame-blits=0 drawable-direct=1
```
