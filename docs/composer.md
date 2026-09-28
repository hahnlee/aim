# Composer (ADR 0012, phase P4)

The original SurfaceFlinger composes the guest's display, and a macOS window
shows it. The derived image's vendor partition provides the composer HAL,
as a device vendor would:

| Piece | Where |
| --- | --- |
| `android.hardware.graphics.composer3.IComposer/default` V4 (Rust guest service) | `hal/graphics/composer`, `/vendor/bin/hw/android.hardware.graphics.composer3-service.aim` |
| Host module `display` (id 3) | `crates/aim-host-display` (library) |
| Display server `aim-display` (window, Metal layer, display link) | `crates/aim-host-display/src/bin/aim-display` |
| ABI (functions, argument blocks, events) | `aim_hostcall::display` |
| `GL_OES_EGL_image_external`, which RenderEngine needs | `hal/gles/src/external.rs` ([gles-driver.md](gles-driver.md)) |

```text
SurfaceFlinger ──binder──▶ composer HAL ──host-call──▶ module `display` ──socket──▶ aim-display
 (RenderEngine: GLES        (guest ELF)                (in the HAL's            (AppKit main thread:
  over ANGLE into the                                   linux-run)               NSWindow + CAMetalLayer,
  client target)            ◀── vsync records on an fd the HAL reads ◀────────── CVDisplayLink)
```

## Decision: a separate display server process

ADR 0012 left open whether the composer becomes a host-native exception. It
does not: the composer HAL is an ordinary guest service over the original
libbinder_ndk, like the other HALs. Its host side is the module `display`,
but the window lives in a separate host process, `aim-display`, not in
the composer's `linux-run`:

- **AppKit needs the main thread.** A `linux-run` process's main thread is
  the guest's main thread (Linux has the leader's tid equal to the pid, and
  guest code blocks it at will). Running AppKit there would mean moving
  every guest off its main thread.
- **The window outlives the composer.** The composer (and with it
  SurfaceFlinger, `onrestart`) may restart; the window and its display link
  stay, as a panel stays when the HWC process dies.
- **One AppKit owner.** Input (P5) arrives as AppKit events on this window
  and becomes the syscall layer's evdev devices, read by inputflinger in
  another process ([input.md](input.md)). The window's owner feeds them;
  it is aimd's role once that daemon exists, as the binder host is.
- **The cost is small.** A buffer crosses once, as its fd (`SCM_RIGHTS`);
  a present is one 56-byte record. Nothing is copied on the way.

`linux-run --display SOCKET` names the server. guest-init passes it (and
`--gpu`) to every service when `linux-run` lists the option
([guest-init-contract.md](guest-init-contract.md)).

## Buffers and presents

- **Import.** A client target buffer is a memfd, which on the host is a
  file. The HAL passes its fd, format, size and stride
  (`FN_IMPORT`), the module sends the fd to the server, and the server
  maps it once (`MAP_SHARED`, read-only), wraps the mapping with
  `newBufferWithBytesNoCopy`, and makes a linear `MTLTexture` over it, as
  the GPU module does ([graphics-buffers.md](graphics-buffers.md)). A
  buffer is named by its gralloc buffer id, which is unique across
  processes. The HAL releases it when its last client-target slot is
  reused.
- **Present.** One render pass samples the texture into the layer's next
  drawable. That is the one copy per frame, and a necessary one: drawables
  belong to Core Animation, and `CAMetalLayer` has no RGBA8 format, so the
  pass also swizzles. It scales when the window is resized (the display
  mode stays fixed, `resizeAspect`). The server waits for the pass to finish
  before it reads the next request, so a buffer is free again once the next
  present has been processed.
- **Display mode.** The window's content in backing pixels: by default the
  main screen's visible frame, `--size WxH` otherwise. Its density is the
  screen's (backing pixels per inch from `CGDisplayScreenSize`), its
  refresh period the display link's nominal one. Android pixels are backing
  pixels, never points.

## Vsync

`CVDisplayLink` on the main display (a display's refresh, not a timer;
#68). Its callback is late by a varying amount, and its `inNow` is the
callback's own time, with about 1 ms of jitter. Its `inOutputTime` comes
from the link's model of the display's timing and is exact to the tick. The
vsync reported is the model's last refresh before the callback. It is
converted to the guest's `CLOCK_MONOTONIC`, which is the host's.

Host code never calls guest code, so vsyncs travel as 32-byte
`display::Event` records on the server connection, whose fd the HAL
receives from `FN_CONNECT`. A thread in the HAL reads them and calls
`IComposerCallback.onVsync` while SurfaceFlinger has vsync enabled. The
server stops sending while it is disabled. The server declares its activity
latency-critical, so App Nap does not throttle the link while the window is
in the background.

## The HAL

- **One display**, id 0, `INTERNAL`, one configuration (id 0). Hotplug is
  delivered synchronously from `registerCallback`: SurfaceFlinger
  configures the displays hotplugged during registration.
  `getDisplayConfigurations` and the legacy `getDisplayConfigs` and
  `getDisplayAttribute` describe the same mode.
- **Client composition only.** Validation turns every layer that is not
  `CLIENT` into `CLIENT` (`ChangedCompositionTypes`) and states the client
  target format (`RGBA_8888`, brightness 1). SurfaceFlinger composes with
  RenderEngine (Skia on GLES, ANGLE on Metal) into the client target, and a
  present shows it. Device composition of layers (a Metal pass per layer)
  can come later without changing the protocol.
- **Fences.** The syscall layer has no sync_file yet. A present waits on
  the client target's acquire fence if one is given (our GLES driver
  finishes before it queues, so there is none). It returns no present
  fence or release fences. `getCapabilities` reports
  `PRESENT_FENCE_IS_NOT_RELIABLE`, and the emulator's vendor properties
  already set `debug.sf.vsync_reactor_ignore_present_fences`, so vsync
  prediction relies on the HAL's vsyncs.
- **Unsupported** (`EX_UNSUPPORTED`, and SurfaceFlinger takes its fallback):
  virtual displays, readback, identification data (SurfaceFlinger uses the
  port), content sampling, per-frame metadata, HDR conversion, overlay
  properties, LUTs, picture profiles and boot display configs.
- A new `createClient` (a restarted SurfaceFlinger) resets the display.

## Boot

With the vendor HALs built (`tools/build-vendor-hals.sh`) and the derived
image assembled (`android-image assemble`):

```sh
aim-display --socket /tmp/display.sock &
guest-init --image IMAGE --data DATA --run \
  --gpu _build/angle-source/out/AimRelease --display /tmp/display.sock \
  --only logd,servicemanager,hwservicemanager,vendor.graphics.allocator,vendor.hwcomposer-3,surfaceflinger,bootanim
```

- SurfaceFlinger needs `hwservicemanager` from the image: it registers
  the deprecated HIDL `IDisplayService` and waits for
  `hwservicemanager.ready`. Everything else it asks for is optional, and it
  continues without it: configstore, the power HAL, statsd and
  displayservice.
- SurfaceFlinger starts `bootanim` itself (`ctl.start`). The original
  `/system/bin/bootanimation` plays `/product/media/bootanimation.zip`
  in the window at 60 fps when it survives its start. It is often
  SIGKILLed about 0.25 s after it starts, while it preloads the zip. Nothing
  in the syscall layer or the guest sends that signal, the kernel gives no
  exit reason, and no crash report is written. The kill needs binder
  traffic, since it does not happen while the binder host is stopped. Until
  that is found, a manual `bootanimation` that survives its first second
  keeps running, and `screencap` works every time.

## Measured (M2 Pro, 60 Hz virtual display, 1080×1920)

| What | Value |
| --- | --- |
| Vsync interval (display link model), mean and SD | 16666.8 µs, 0.4 µs |
| Display link callback after the reported vsync | about 10 ms (the model's phase) |
| Vsync record sent to `onVsync` returned, in the HAL | 92–116 µs mean, 0.8–3.2 ms max (idle system) |
| Frames presented, bootanimation | 59.7–59.9 per second |
| Present, request to GPU done (drawable, pass, wait) | 0.62–0.74 ms mean, 2.4 ms max |
| GPU time of the present pass | 0.13–0.16 ms |
| RenderEngine shader cache (134 shaders), cold / warm ANGLE cache | 12.7 s / 0.43 s |

Before the App Nap fix, the display link dropped to about 5 Hz when the
window was in the background, and presents waited up to 400 ms for a
drawable.

## Evidence

`aim-display` prints its window number (`screencapture -l N`), and
SIGUSR1 writes the last presented buffer to its `--capture` file.
`crates/aim-host-display/tests/display.rs` covers the protocol: import,
present, capture back, and vsync intervals within 1% of the period.
