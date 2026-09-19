# Remote SurfaceTexture producer ownership

Status: implemented 2026-09-19; physical Chromium Gemini close/reopen passes.
Broader Example Domain, reload/navigation and Retina acceptance remain open.

## Context

An unchanged Chromium Gemini-button click parcels a `Surface` backed by a
Browser-owned `SurfaceTexture` to its GPU process. The Browser writes real
720×1280 geometry but no `SurfaceControl` owner/layer. Previously the GPU
recreated a 0×0 `ANativeWindow` and Vulkan rejected it. Preserving parcelled
geometry removes that error, then Dawn requests MAILBOX and the GPU facade has
no path to the Browser's consumer. Its independent buffer queue and display
root cannot deliver TextureView frames or release producer slots.

Android owns Surface/BufferQueue policy and Binder object lifetime. Darwin owns
IOSurface-backed AHardwareBuffer storage, native fences and the narrow host
transport beneath the Android ABI. The existing build contains parts of
`IGraphicBufferProducer`, but not a complete AOSP `GraphicBuffer`/gralloc and
BufferQueue consumer integration for this custom AHardwareBuffer owner.

## Decision

Use a private, versioned native `BBinder` producer capability in the ordinary
`Surface` Parcel through the existing AOSP `/dev/binder` runtime. The Browser's
`SurfaceTexture` and `NativeWindowBufferQueue` remain the sole consumer and
slot authority. The GPU receives a retained Binder proxy and a genuine local
`ANativeWindow` facade; it forwards geometry, dequeue, queue, cancel and
disconnect. A pointer-derived Parcel token is not authority or remote storage.

Each dequeue creates a unique lease bound to the endpoint incarnation and
queue generation. The owner retains the exact IOSurface-backed AHardwareBuffer
until the GPU imports that backing and acknowledges it. Queue/cancel consume the
lease once; producer fences remain dependencies until complete. Browser
SurfaceTexture publication and frame notification occur only after successful
queue. Consumer release returns the exact slot/generation/frame and release
fence before reuse. Abandon, Binder death and disconnect retire outstanding
leases without treating EOF or callback removal as successful presentation.
MAILBOX is advertised only when that consumer can replace pending frames and
return displaced slots. A transported producer without a live endpoint must
fail instead of creating a substitute SurfaceControl root.

The endpoint belongs under the window subsystem, separate from Surface JNI,
the queue's slot policy and IOSurface storage ownership. Binder authenticates
capability possession; protocol fields are validated against owner state.
No Chrome-specific branch, APK/profile change, CPU/GL fallback or fabricated
frame callback is permitted.

## Owner contract prerequisite

The owner-side queue now exposes the identity needed by that endpoint without
making the native-window facade an authority. `Dequeue` returns a borrowed
buffer together with a monotonic lease `{slot, generation, lease}`. The
lease-bearing `Queue` and `Cancel` operations validate all three fields and
consume the lease exactly once; the existing native-buffer overloads remain
only as an in-process compatibility path. A successful queue returns the
immutable frame token `{slot, generation, frame}`. `Cancel` transfers its
owned fence to the slot as a reuse dependency, so a cancelled slot is not
reused until that fence signals.

The ANativeWindow queue callback carries the same generation and frame token.
SurfaceTexture retains that exact token with every pending/current buffer and
returns it with its release fence, including publication replacement, abandon,
consumer handoff, and teardown. This removes slot-only return ambiguity on the
SurfaceTexture path while retaining the old slot-only callback for legacy
ImageReader consumers until their owner contract is upgraded.

The owner contract is used by the Binder endpoint below. MAILBOX remains valid
only for a producer with a live consumer callback; an independent GPU window
continues to advertise FIFO.

## Binder endpoint boundary

The owner-side `remote_surface_producer` module now supplies the first
versioned private Binder protocol. `CreateRemoteSurfaceProducerEndpoint`
publishes a retained owner capability; `CreateRemoteSurfaceProducerClient`
returns the existing remote-native-window hook table. A dequeue response
contains the exact lease and exported IOSurface identity. The endpoint retains
the owner AHardwareBuffer until the client imports the identity and sends an
explicit ACK. Queue and Cancel consume the lease once, and malformed or stale
keys are rejected. Imported or uncertain work is sent through owner
quarantine on client death, transport failure, or an unfinished producer
fence; a lease which was never imported can be cancelled and reused.

The initial endpoint deliberately waits for and closes fences at each process
boundary instead of transferring file descriptors through Binder. This keeps
fence ownership correct while the Darwin Binder FD adapter is still being
validated, at the cost of synchronous producer stalls. The client returns the
real imported AHardwareBuffer native-window ABI pointer to ANGLE; a process
local lease object is never used as a native buffer. Frame callbacks remain on
the owner SurfaceTexture route because the current remote hook table has no
callback field.

## Alternatives and migration

Direct AOSP `IGraphicBufferProducer` marshalling is the target to return to
once its `GraphicBuffer`/gralloc and consumer owners can wrap the exact retained
IOSurface backing. The current partial libgui archive does not make that path
operational. A separate Unix FD endpoint would duplicate bootstrap, custody and
shutdown machinery already supplied by Binder and managed SCM. This private
protocol is limited to the current Android 16 compatibility boundary and must
be removed when the pinned AOSP producer path is fully adopted.

## Verification gate

Test production objects across two actual runtime processes over `/dev/binder`:
same backing identity and visible consumer frame, fence-delayed reuse, duplicate
or stale queue/cancel, failed import, producer death, abandon with in-flight
frames and repeated resize/reconnect. Same-process Binder fixtures are component
evidence only. Finish with unchanged Chromium physical Gemini interaction,
stable Example Domain body/input/reload and a fresh Retina Vulkan capture.
